//! `agentstack more delivery` — state the routing, and set the one override.
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

use agentstack_core::paint::OwoColorize;
use anyhow::{Context as _, Result};

use std::path::Path;

use crate::cli::{DeliveryArgs, DeliveryCmd};
use crate::delivery::{HarnessPlan, Lane, Plan};
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
            serde_json::to_string_pretty(&crate::ui_contract::envelope(
                plan.to_json(&|id: &str| super::overview::bridge_registered(&ctx.registry, id))
            ))?
        );
        return Ok(());
    }

    println!("  {}  how capabilities reach each tool", "Delivery".bold());
    if plan.harnesses.is_empty() {
        println!("  {} no tools targeted here yet", "·".dimmed());
        return Ok(());
    }

    // The same probe doctor's zero-files section runs — one definition of
    // "is the bridge registered?", read PER HARNESS so a single connected CLI
    // cannot make the others claim live delivery.
    let unconnected = unconnected_live(&plan, &ctx.registry);
    let declares_live = declares_something_live(&ctx.loaded.manifest, &plan);

    let width = plan
        .harnesses
        .iter()
        .map(|h| h.display.len())
        .max()
        .unwrap_or(0);
    for h in &plan.harnesses {
        println!(
            "  {:width$}   {}",
            h.display,
            harness_sentence(h, super::overview::bridge_registered(&ctx.registry, &h.id))
        );
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
    // Only when something DECLARED travels live and some live harness has no
    // bridge: a project of pure instructions/settings needs no bridge at all.
    if declares_live && !unconnected.is_empty() {
        println!("  {} {}", "·".dimmed(), CONNECT_THE_BRIDGE);
    }
    if plan.has_dynamic_lane() {
        // Disk, not routing. A failed state read leaves the list empty, which
        // is the same fallback `status` takes: the ledger's own health is
        // `doctor`'s finding, not this screen's.
        let abandoned = crate::state::State::load()
            .map(|state| {
                super::apply::abandoned_live_renders(
                    &ctx,
                    &plan,
                    &state,
                    &[crate::scope::Scope::Project, crate::scope::Scope::Global],
                )
            })
            .unwrap_or_default();
        println!(
            "  {} {}",
            "·".dimmed(),
            super::apply::live_lane_artifacts_line(&abandoned)
        );
        for found in &abandoned {
            println!("  {}  {}", "⚠".yellow(), found.sentence());
        }
    }
    if let Some(line) = crate::delivery::rendered_lane_line(&plan) {
        println!("  {} {line}", "·".dimmed());
    }
    println!(
        "  {} {}",
        "·".dimmed(),
        "write files anyway: agentstack more delivery render-locally --write".dimmed()
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
            "no such tool: {id} — `agentstack more adapters list` names the ones this build knows"
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

    // `[delivery] render_locally` is a routing preference: it says WHERE the
    // capabilities already declared should land, and declares none of its own.
    // The write still moves the consent digest, though, so without this the
    // documented next step (`agentstack apply --write`) would be refused by a
    // gate this command's own bytes tripped. Captured before the write, spliced
    // in after it — see `crate::trust_carry::TrustCarry` for why that is the
    // only safe order.
    let carry = crate::trust_carry::TrustCarry::before_write(&ctx.dir);
    let was_trusted = carry.was_valid();
    let backup = crate::history::capture(&path, "agentstack.toml · delivery override");
    crate::util::atomic::write(&path, &updated)
        .with_context(|| format!("writing {}", path.display()))?;
    let _ = crate::history::record("project", "delivery render-locally", vec![], vec![backup]);
    let carried = carry.across_write(&path, &updated)?;
    if was_trusted && !carried {
        println!(
            "  {} manifest changed — review and re-run `agentstack trust .`",
            "·".dimmed()
        );
    }

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

/// The one recovery command for a plan that routes live with no bridge
/// registered. Shared so `status`, `delivery`, and `doctor`'s finding cannot
/// name three different commands for one state.
pub const CONNECT_THE_BRIDGE: &str =
    "register the bridge: agentstack more gateway connect --all --write";

/// `HarnessPlan::sentence` states the routing as if it were already happening.
/// That is true only once a CLI has the gateway registered: with no bridge,
/// nothing in the dynamic lane reaches any tool. Invariant 8 — a surface may
/// not claim a capability is delivered when it is not — so the live clause is
/// restated as a PLAN, and the rendered clause (which really is on disk) is
/// left exactly as it was.
///
/// Takes `&HarnessPlan` and returns an owned `String` for the same reason
/// `sentence` does: the text is assembled here, so there is nothing to borrow.
///
/// `bridge_registered` is **this harness's own** bridge state, never a
/// project-wide any-of reading: one connected CLI does not deliver anything to
/// the four that have no bridge, and saying so was an invariant-8 breach.
pub fn harness_sentence(h: &HarnessPlan, bridge_registered: bool) -> String {
    let live = h.kinds_in(Lane::Dynamic);
    if bridge_registered || live.is_empty() {
        return h.sentence();
    }
    let names = |kinds: &[crate::delivery::Kind]| -> String {
        kinds
            .iter()
            .map(|k| k.label())
            .collect::<Vec<_>>()
            .join(" + ")
    };
    let files = h.kinds_in(Lane::Rendered);
    if files.is_empty() {
        format!("{} planned live (not connected)", names(&live))
    } else {
        format!(
            "{} planned live (not connected) · {} written to files",
            names(&live),
            names(&files)
        )
    }
}

/// `Reason::why` states the routed rationale as if the live channel were
/// already carrying the capability: "served live, on demand". That is true only
/// once this harness has the bridge registered — with none, nothing in the
/// dynamic lane reaches any tool, so the clause claims a delivery that is not
/// happening. Same invariant (8) and same correction [`harness_sentence`] makes
/// for `summary`, applied at the one field that was left behind.
///
/// It stays RATIONALE — why this kind takes this lane — rather than turning
/// into a second copy of `summary` or into something to go and do: the reason
/// skills and servers route live is that the live channel here can carry them,
/// and that reason holds whether or not a bridge has been registered yet. Only
/// the *tense* moves. Every other reason is a physical fact about the kind or
/// the tool and reads identically either way, so only [`Reason::Routed`] is
/// restated.
///
/// Takes `&Route` rather than `Reason` so a caller cannot pair a reason with
/// the wrong route's bridge state; returns `&'static str` because both arms are
/// fixed copy, exactly as `Reason::why` is.
pub fn route_why(route: &crate::delivery::Route, bridge_registered: bool) -> &'static str {
    match route.reason {
        crate::delivery::Reason::Routed if !bridge_registered => {
            "the live channel here can carry it on demand"
        }
        other => other.why(),
    }
}

/// One line for surfaces that already print a per-harness list and only need
/// the lane summary appended (`init`'s plan screen, `status`).
///
/// Each harness's bridge state is read individually through
/// [`super::overview::bridge_registered`], the one definition every surface
/// shares.
pub fn summary_lines_for(plan: &Plan, registry: &crate::adapter::Registry) -> Vec<String> {
    plan.harnesses
        .iter()
        .map(|h| {
            format!(
                "{} — {}",
                h.display,
                harness_sentence(h, super::overview::bridge_registered(registry, &h.id))
            )
        })
        .collect()
}

/// The collapsed twin of [`summary_lines_for`], for the default `status`
/// screen: the same routing, grouped and counted instead of listed per CLI.
///
/// Harnesses that route the same kinds AND share a bridge state collapse into
/// one line, so a thirteen-CLI fan-out reads as one or two facts rather than a
/// table. The per-CLI list is one flag away (`agentstack status --verbose`,
/// which prints [`summary_lines_for`] instead) — disclosure, not omission.
///
/// **No harness is ever named on a line that carries a live claim.** A count
/// makes the sentence project-wide, which is the only shape that cannot be read
/// as "this named tool is being served"; naming a file-only CLI beside a
/// `served live` clause is exactly the invariant-8 misread
/// [`harness_sentence`] exists to prevent. The names live behind `--verbose`,
/// where each one carries its own honest verb.
pub fn summary_counts_for(plan: &Plan, registry: &crate::adapter::Registry) -> Vec<String> {
    // Keyed by (live kinds, this harness's own bridge state), in first-seen
    // order so the line order tracks the target order a reader already saw.
    let mut groups: Vec<((String, bool), usize)> = Vec::new();
    let mut file_only = 0usize;
    for h in &plan.harnesses {
        let live = h.kinds_in(Lane::Dynamic);
        if live.is_empty() {
            // A harness with nothing in either lane has nothing to report; it
            // is not a "files only" tool, it is an empty one.
            if !h.kinds_in(Lane::Rendered).is_empty() {
                file_only += 1;
            }
            continue;
        }
        let key = (
            live.iter()
                .map(|k| k.label())
                .collect::<Vec<_>>()
                .join(" + "),
            super::overview::bridge_registered(registry, &h.id),
        );
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => groups.push((key, 1)),
        }
    }
    let mut lines: Vec<String> = groups
        .into_iter()
        .map(|((kinds, bridged), n)| {
            if bridged {
                format!("{kinds} served live to {}", super::count(n, "CLI"))
            } else {
                // The same correction `harness_sentence` makes, in the
                // collapsed voice: with no bridge, nothing is reaching anything.
                format!(
                    "{kinds} planned live (not connected) for {}",
                    super::count(n, "CLI")
                )
            }
        })
        .collect();
    if file_only > 0 {
        let clause = format!("files only for {}", super::count(file_only, "CLI"));
        match lines.first_mut() {
            Some(first) => {
                first.push_str(" · ");
                first.push_str(&clause);
            }
            None => lines.push(format!("{clause} — those tools read files only")),
        }
    }
    lines
}

/// The plan stated as if every bridge were registered. For surfaces describing
/// a plan that has deliberately not been carried out yet (`init`'s preview),
/// where the un-registered state is disclosed separately and in full.
pub fn summary_lines(plan: &Plan) -> Vec<String> {
    plan.harnesses
        .iter()
        .map(|h| format!("{} — {}", h.display, harness_sentence(h, true)))
        .collect()
}

/// Display names of the harnesses this plan routes to the LIVE lane that have
/// no bridge registered. Empty means every live harness can actually receive
/// what the plan promises it.
pub fn unconnected_live(plan: &Plan, registry: &crate::adapter::Registry) -> Vec<String> {
    plan.live_harnesses()
        .iter()
        .filter(|h| !super::overview::bridge_registered(registry, &h.id))
        .map(|h| h.display.clone())
        .collect()
}

/// Does this manifest declare anything the LIVE lane actually carries?
///
/// [`Plan`] routes by what each harness *can* take, so it reports a dynamic
/// lane even over an empty manifest, and even for a project whose only
/// capabilities are instructions and settings — both served entirely from
/// files. The bridge question is only real when something declared would
/// travel live, so this is the single predicate `status`, `doctor`, and
/// `delivery` all gate the "register the bridge" hint on.
pub fn declares_something_live(
    manifest: &agentstack_core::manifest::Manifest,
    plan: &Plan,
) -> bool {
    plan.harnesses
        .iter()
        .flat_map(|h| h.kinds_in(Lane::Dynamic))
        .any(|k| match k {
            crate::delivery::Kind::Skill => !manifest.skills.is_empty(),
            crate::delivery::Kind::Server => !manifest.declared_server_names().is_empty(),
            crate::delivery::Kind::Instruction => !manifest.instructions.is_empty(),
            crate::delivery::Kind::Setting => !manifest.settings.is_empty(),
            crate::delivery::Kind::Hook => !manifest.hooks.is_empty(),
            crate::delivery::Kind::Extension => !manifest.extensions.is_empty(),
        })
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
