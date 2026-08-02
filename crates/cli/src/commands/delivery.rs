//! `agentstack delivery` — state the routing, and set the one override.
//!
//! The read half prints what [`crate::delivery::Plan`] decided, per harness, in
//! plain language. The write half records **Render locally** in the manifest —
//! the only delivery override there is (`docs/design/automatic-delivery.md`
//! §"The decision", amended 2026-08-02).
//!
//! The override is a manifest edit, so it goes through `toml_edit` like every
//! other manifest edit: a hand-written file keeps its comments and its
//! formatting, and clearing the override removes the key rather than writing
//! `false` — the default stays implicit, exactly as `[meta] gitignore` does.

use anyhow::{Context as _, Result};
use owo_colors::OwoColorize;

use std::path::Path;

use crate::cli::{DeliveryArgs, DeliveryCmd};
use crate::delivery::{Lane, Plan};
use crate::render::resolve_targets;

pub fn run(args: &DeliveryArgs, manifest_dir: Option<&Path>) -> Result<()> {
    match &args.command {
        None => show(args.json, manifest_dir),
        Some(DeliveryCmd::RenderLocally {
            harness,
            off,
            write,
        }) => render_locally(harness.as_deref(), *off, *write, manifest_dir),
    }
}

/// Build this project's plan over the manifest's declared targets.
fn plan_for(ctx: &super::Context) -> Result<Plan> {
    let ids = resolve_targets(&ctx.loaded.manifest, &ctx.registry, &[], &ctx.dir)?;
    Ok(Plan::build(
        &ctx.loaded.manifest.delivery,
        &ctx.registry,
        &ids,
    ))
}

fn show(json: bool, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let plan = plan_for(&ctx)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::ui_contract::envelope(plan.to_json()))?
        );
        return Ok(());
    }

    println!("  {}  how capabilities reach each tool", "Delivery".bold());
    if plan.harnesses.is_empty() {
        println!("  {} no tools targeted here yet", "·".dimmed());
        return Ok(());
    }

    let width = plan
        .harnesses
        .iter()
        .map(|h| h.display.len())
        .max()
        .unwrap_or(0);
    for h in &plan.harnesses {
        println!("  {:width$}   {}", h.display, h.sentence());
        if h.render_locally {
            println!(
                "  {:width$}   {}",
                "",
                format!("override: render locally ({})", h.override_source.slug()).dimmed()
            );
        }
        // Executable kinds are named explicitly, because "written to files" is
        // not the interesting fact about them — the ceremony is.
        let ceremony: Vec<&str> = h
            .routes
            .iter()
            .filter(|r| r.full_ceremony())
            .map(|r| r.kind.label())
            .collect();
        if !ceremony.is_empty() {
            println!(
                "  {:width$}   {}",
                "",
                format!(
                    "{} run code — reviewed in full every time",
                    ceremony.join(" + ")
                )
                .dimmed()
            );
        }
    }

    // The honesty rules, both of them, on their own lines.
    if plan.has_dynamic_lane() {
        println!("  {} {}", "·".dimmed(), crate::delivery::ZERO_ARTIFACTS);
    }
    if let Some(line) = crate::delivery::rendered_lane_line(&plan) {
        println!("  {} {line}", "·".dimmed());
    }
    println!(
        "  {} {}",
        "·".dimmed(),
        "write files anyway: agentstack delivery render-locally --write".dimmed()
    );
    Ok(())
}

fn render_locally(
    harness: Option<&str>,
    off: bool,
    write: bool,
    manifest_dir: Option<&Path>,
) -> Result<()> {
    let ctx = super::load(manifest_dir)?;

    // A harness id must name a real adapter. Silently recording an override for
    // a typo'd id would be an override that never applies, and the user would
    // have no way to tell that from one that does.
    if let Some(id) = harness {
        anyhow::ensure!(
            ctx.registry.get(id).is_some(),
            "no such tool: {id} — `agentstack adapters list` names the ones this build knows"
        );
    }

    let path = ctx.loaded.manifest_path.clone();
    let original =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let updated = set_render_locally(&original, harness, !off)?;

    let scope = match harness {
        Some(id) => format!("{id} in this project"),
        None => "this project".to_string(),
    };
    if updated == original {
        println!(
            "  {} delivery for {scope} is already {}.",
            "·".dimmed(),
            if off { "automatic" } else { "render locally" }
        );
        return Ok(());
    }

    if !write {
        println!(
            "  {} would {} {} for {scope}.",
            "·".dimmed(),
            if off { "clear" } else { "record" },
            "[delivery] render_locally".bold()
        );
        println!("  {} re-run with --write to apply.", "·".dimmed());
        return Ok(());
    }

    let backup = crate::history::capture(&path, "agentstack.toml · delivery override");
    crate::util::atomic::write(&path, &updated)
        .with_context(|| format!("writing {}", path.display()))?;
    let _ = crate::history::record("project", "delivery render-locally", vec![], vec![backup]);

    if off {
        println!("  {} delivery for {scope} is automatic again.", "✓".green());
    } else {
        println!(
            "  {} render locally recorded for {scope} — files are written even where the live \
             channel would have worked.",
            "✓".green()
        );
        println!(
            "  {} nothing is on disk yet: {}",
            "·".dimmed(),
            "agentstack apply --write".bold()
        );
    }
    Ok(())
}

/// Set or clear `[delivery] render_locally`, project-wide or for one harness.
///
/// Clearing removes the key (and the now-empty table) rather than writing
/// `false`, so an unset project keeps a manifest that says nothing about
/// delivery — which is what "Automatic" means. Pure over the TOML text, so the
/// edit is testable without a project on disk.
pub fn set_render_locally(text: &str, harness: Option<&str>, on: bool) -> Result<String> {
    use toml_edit::{DocumentMut, Item, Table};

    let mut doc: DocumentMut = text.parse().context("parsing the manifest as TOML")?;

    if !on {
        // Remove exactly the key that was set, then any container it left empty.
        match harness {
            Some(id) => {
                if let Some(h) = doc
                    .get_mut("delivery")
                    .and_then(Item::as_table_mut)
                    .and_then(|d| d.get_mut("harness"))
                    .and_then(Item::as_table_mut)
                {
                    if let Some(entry) = h.get_mut(id).and_then(Item::as_table_mut) {
                        entry.remove("render_locally");
                        if entry.is_empty() {
                            h.remove(id);
                        }
                    }
                    if h.is_empty() {
                        if let Some(d) = doc.get_mut("delivery").and_then(Item::as_table_mut) {
                            d.remove("harness");
                        }
                    }
                }
            }
            None => {
                if let Some(d) = doc.get_mut("delivery").and_then(Item::as_table_mut) {
                    d.remove("render_locally");
                }
            }
        }
        if doc
            .get("delivery")
            .and_then(Item::as_table)
            .is_some_and(Table::is_empty)
        {
            doc.remove("delivery");
        }
        return Ok(doc.to_string());
    }

    let delivery = doc
        .entry("delivery")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .context("[delivery] is not a table")?;
    match harness {
        None => delivery["render_locally"] = toml_edit::value(true),
        Some(id) => {
            let table = delivery
                .entry("harness")
                .or_insert_with(|| {
                    let mut t = Table::new();
                    // Implicit: `[delivery.harness]` never gets a bare header of
                    // its own, only `[delivery.harness.<id>]`.
                    t.set_implicit(true);
                    Item::Table(t)
                })
                .as_table_mut()
                .context("[delivery.harness] is not a table")?;
            let entry = table
                .entry(id)
                .or_insert_with(|| Item::Table(Table::new()))
                .as_table_mut()
                .context("[delivery.harness.<id>] is not a table")?;
            entry["render_locally"] = toml_edit::value(true);
        }
    }
    Ok(doc.to_string())
}

/// One line for surfaces that already print a per-harness list and only need
/// the lane summary appended (`init`'s plan screen, `status`).
pub fn summary_lines(plan: &Plan) -> Vec<String> {
    plan.harnesses
        .iter()
        .map(|h| format!("{} — {}", h.display, h.sentence()))
        .collect()
}

/// Does this plan put anything in the dynamic lane? Convenience for callers
/// that only need the one bit.
pub fn serves_anything_live(plan: &Plan) -> bool {
    plan.harnesses
        .iter()
        .any(|h| !h.kinds_in(Lane::Dynamic).is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_and_clearing_leaves_the_default_implicit() {
        let base = "version = 1\n\n[servers.a]\ntype = \"http\"\nurl = \"https://x\"\n";

        let on = set_render_locally(base, None, true).unwrap();
        assert!(on.contains("[delivery]"), "{on}");
        assert!(on.contains("render_locally = true"), "{on}");
        // The hand-written body is untouched.
        assert!(on.contains("[servers.a]"), "{on}");

        let off = set_render_locally(&on, None, false).unwrap();
        assert!(
            !off.contains("delivery"),
            "clearing removes the table: {off}"
        );
        assert!(!off.contains("render_locally = false"), "{off}");
        assert!(off.contains("[servers.a]"), "{off}");
    }

    #[test]
    fn a_harness_override_is_its_own_entry() {
        let on = set_render_locally("version = 1\n", Some("codex"), true).unwrap();
        assert!(on.contains("[delivery.harness.codex]"), "{on}");
        assert!(on.contains("render_locally = true"), "{on}");

        let parsed: agentstack_core::manifest::Manifest = toml::from_str(&on).unwrap();
        assert!(parsed.delivery.renders_locally("codex"));
        assert!(!parsed.delivery.renders_locally("claude-code"));

        let off = set_render_locally(&on, Some("codex"), false).unwrap();
        assert!(!off.contains("delivery"), "{off}");
    }

    /// Clearing one harness must not disturb the project-wide answer.
    #[test]
    fn clearing_one_harness_keeps_the_project_setting() {
        let both = set_render_locally(
            &set_render_locally("version = 1\n", None, true).unwrap(),
            Some("codex"),
            true,
        )
        .unwrap();
        let off = set_render_locally(&both, Some("codex"), false).unwrap();
        let parsed: agentstack_core::manifest::Manifest = toml::from_str(&off).unwrap();
        assert!(parsed.delivery.renders_locally("codex"), "{off}");
        assert!(parsed.delivery.render_locally == Some(true), "{off}");
    }
}
