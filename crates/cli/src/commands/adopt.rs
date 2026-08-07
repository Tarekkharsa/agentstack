//! `agentstack adopt` — pull hand-added servers AND hand-edited fields of
//! manifest-known servers from a target config back into the manifest, lifting
//! their inline secrets. The reverse of `apply`.
//!
//! Edited fields are detected by comparing each target's *rendered* form of a
//! server against its on-disk entry, both read back through the same adapter
//! lens ([`extract_servers`]) — so adapter transforms (cwd shell-wrapping,
//! renamed keys) can never masquerade as hand-edits.
//!
//! Uses the TOML merger to upsert `[servers.<name>]` tables into the existing
//! `agentstack.toml`, preserving its comments and formatting.

use std::fs;
use std::path::Path;

use agentstack_core::paint::OwoColorize;
use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde_json::Value;

use crate::adapter::descriptor::Format;
use crate::adapter::extract_servers;
use crate::cli::AdoptArgs;
use crate::discover::lift_secrets;
use crate::manifest::Server;
use crate::render::{merge_toml, plan_target_with_servers, resolve_targets, ruleset_for};
use crate::scope::Scope;
use crate::secret::keychain;
use crate::util::diff;

pub fn run(args: &AdoptArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    let scope = args.scope.unwrap_or_else(|| Scope::default_for(&ctx.dir));

    // Collect servers present on disk but absent from the manifest, plus
    // hand-edited fields of servers the manifest already knows.
    let mut collected: IndexMap<String, Server> = IndexMap::new();
    let target_ids = resolve_targets(manifest, &ctx.registry, &args.targets, &ctx.dir)?;
    // Detecting an edited field needs the rendered baseline, and rendering
    // resolves secrets under the effective policy — same gate as apply/diff.
    let ruleset = ruleset_for(manifest)?;

    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            continue;
        };
        let Some((config_path, format)) = desc.config_for(scope, &ctx.dir) else {
            continue;
        };
        let text = match fs::read_to_string(&config_path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "cannot read {} for {} — fix its permissions or omit this CLI with `--target <CLI>`",
                        config_path.display(),
                        desc.display
                    )
                });
            }
        };
        if text.trim().is_empty() {
            continue;
        }
        let value = parse_config(&text, format).with_context(|| {
            format!(
                "cannot adopt from {} ({}) — fix the file syntax or move it aside, then rerun `agentstack adopt`",
                config_path.display(),
                desc.display
            )
        })?;
        // What the manifest would put on this target's disk, read back through
        // the same adapter lens as the on-disk entries — the drift baseline.
        //
        // Deliberately NOT routed through `delivery::Plan`. `adopt` reads a
        // hand-edit back INTO the manifest: the only file it writes is
        // `agentstack.toml` (see the single `atomic::write` below), never a
        // rendered server config. So there is no lane for it to switch and no
        // write for the planner to withhold. The baseline here is a
        // comparison value, not an intent to render, so it stays the manifest's
        // full form under either lane: narrowing it by delivery would change
        // which hand-edits `adopt` can SEE, and a verb whose job is to notice
        // a user's edit must not go blind because the servers also travel
        // live.
        let rendered_by_name: IndexMap<String, Server> = plan_target_with_servers(
            desc,
            &ctx.resolver,
            &ruleset,
            &manifest.servers,
            &[],
            scope,
            &ctx.dir,
            crate::render::PriorTrust::STRICT,
        )?
        .map(|plan| parse_config(&plan.proposed, format))
        .transpose()
        .context("the internal adopt baseline did not parse — run `agentstack doctor` and report this as an AgentStack bug")?
        .map(|v| extract_servers(desc, &v).into_iter().collect())
        .unwrap_or_default();
        for (name, server) in extract_servers(desc, &value) {
            match manifest.servers.get(&name) {
                None => {
                    if !collected.contains_key(&name) {
                        println!("  {} {name} (from {})", "+".green(), desc.display);
                        collected.insert(name, server);
                    }
                }
                Some(existing) => {
                    // Hand-edited fields: any value where this target's disk
                    // disagrees with the manifest's rendered form. Owned
                    // servers are skipped — their refresh loop is `apply`'s
                    // job (see render::owned), not adoption.
                    if existing.owner.is_none() {
                        if let Some(rendered) = rendered_by_name.get(&name) {
                            let mut updated = collected.get(&name).unwrap_or(existing).clone();
                            let fields =
                                adopt_changed_fields(&mut updated, rendered, &server, &desc.id);
                            if !fields.is_empty() {
                                println!(
                                    "  {} {name}: {} (from {})",
                                    "~".yellow(),
                                    fields.join(", "),
                                    desc.display
                                );
                                collected.insert(name.clone(), updated);
                            }
                        }
                    }
                    // Hand-added native keys (per-target extras) the manifest
                    // doesn't carry yet.
                    for (target, new_keys) in new_extras(existing, server) {
                        println!(
                            "  {} {name}: extra.{target} {{{}}} (from {})",
                            "~".yellow(),
                            new_keys.keys().cloned().collect::<Vec<_>>().join(", "),
                            desc.display
                        );
                        let merged = collected
                            .entry(name.clone())
                            .or_insert_with(|| existing.clone());
                        merged.extra.entry(target).or_default().extend(new_keys);
                    }
                }
            }
        }
    }

    // Files dropped into this project's own intake dirs. Same verb, because it
    // is the same question — "something is here that your setup doesn't know
    // about, bring it in" — and one verb is one thing for a user to learn.
    let found = crate::intake::scan(
        &ctx.dir,
        &crate::manifest::project_root_of(&ctx.dir),
        manifest,
    );

    let dropped = &found.items;
    // A dropped file whose name a manifest entry already uses is reported, not
    // adopted: replacing a pinned declaration is not "bringing in something
    // new", and this slice will not do it behind a preview that says `+`.
    for c in &found.collisions {
        println!(
            "  {} {} '{}' in {} — that name is already declared; rename the file \
             or remove the existing entry",
            "!".yellow(),
            c.kind.noun(),
            c.name,
            c.rel_path
        );
    }

    if collected.is_empty() && dropped.is_empty() {
        println!(
            "Nothing to adopt — every on-disk server already matches the manifest, and no \
             undeclared files are waiting."
        );
        return Ok(());
    }

    // Lift inline secrets so the manifest stays commit-safe.
    let lifted = lift_secrets(&mut collected);

    // Insert into the existing manifest text, preserving comments.
    let entries: Vec<(String, Value)> = collected
        .iter()
        .map(|(n, s)| {
            let value = serde_json::to_value(s)
                .expect("an internal derive(Serialize) struct always serializes");
            (n.clone(), value)
        })
        .collect();
    let manifest_text = fs::read_to_string(&ctx.loaded.manifest_path).with_context(|| {
        format!(
            "cannot read {} — fix its permissions, then rerun `agentstack adopt`",
            ctx.loaded.manifest_path.display()
        )
    })?;
    let mut new_text = manifest_text.clone();
    if !entries.is_empty() {
        new_text = merge_toml::merge(&new_text, "servers", &entries, true)
            .context("cannot update the manifest — fix its TOML syntax with `agentstack doctor`, then rerun `agentstack adopt`")?;
    }
    // Dropped files become ordinary manifest entries through the same single
    // insertion path `add` uses, so there is no second way content enters a
    // manifest — and `toml_edit` keeps the file's comments and formatting.
    for item in dropped {
        new_text = super::add::build_manifest_with(
            &new_text,
            item.kind.section(),
            &item.name,
            &intake_entry(item),
            None,
        )
        .with_context(|| {
            format!(
                "cannot add {} '{}' to the manifest — fix its TOML syntax with `agentstack doctor`, then rerun `agentstack adopt`",
                item.kind.noun(),
                item.name
            )
        })?;
    }

    let mut what = Vec::new();
    if !collected.is_empty() {
        what.push(super::count(collected.len(), "server"));
    }
    if !dropped.is_empty() {
        what.push(super::count(dropped.len(), "dropped file"));
    }
    println!(
        "\n{} {} to adopt into {}",
        "→".cyan(),
        what.join(" and "),
        ctx.loaded.manifest_path.display()
    );
    // The preview names each dropped file, where it came from, and which path
    // its provenance puts it on — a classification the user cannot see is not
    // a consent story.
    for item in dropped {
        println!(
            "  {} {} {} ({})",
            "+".green(),
            item.kind.noun(),
            item.name,
            item.rel_path.dimmed()
        );
        if let Some(summary) = &item.summary {
            println!("      {}", summary.dimmed());
        }
        println!(
            "      {}",
            format!(
                "{} — {}",
                if item.provenance.is_local() {
                    "your own work"
                } else {
                    "came with this project"
                },
                item.provenance.reason()
            )
            .dimmed()
        );
    }
    print!(
        "{}",
        diff::render(&manifest_text, &new_text)
            .lines()
            .map(|l| format!("  {l}\n"))
            .collect::<String>()
    );
    if !lifted.is_empty() {
        let names: Vec<&str> = lifted.iter().map(|l| l.reference.as_str()).collect();
        println!(
            "  {} {}: {}",
            "🔐".dimmed(),
            super::count(names.len(), "lifted secret"),
            names.join(", ")
        );
    }

    // Strategy v2 / Moment 5: every material write names its undo IN the
    // preview, before it runs — not in the success summary afterwards, where a
    // user who wanted to back out has already had the write happen to them.
    // `adopt` is the one material write that named its undo nowhere at all.
    // The manifest is the only thing this writes (the keychain lift below is
    // named separately, because a stored secret is undone differently).
    println!(
        "  {} undo: `git checkout -- {}` restores the manifest; `agentstack restore` reverts the last write.",
        "↩".dimmed(),
        ctx.loaded
            .manifest_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    );
    if !lifted.is_empty() && !args.no_keychain {
        println!(
            "  {} the lifted {} above will be stored in this machine's keychain — remove with `agentstack secret rm <REF>`.",
            "↩".dimmed(),
            if lifted.len() == 1 { "secret" } else { "secrets" }
        );
    }

    if args.write {
        if !args.no_keychain {
            for l in &lifted {
                keychain::set(&l.reference, &l.value)
                    .with_context(|| format!("cannot store '{}' in the keychain — unlock it or rerun with `--no-keychain`", l.reference))?;
            }
        }
        crate::util::atomic::write(&ctx.loaded.manifest_path, &new_text)
            .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
        if args.to_library {
            save_to_library(dropped)?;
        }
        println!("\n{} adopted {}.", "✓".green(), what.join(" and "));
        if !dropped.is_empty() {
            // Declaring content does not deliver it: the lock still has to pin
            // the new bytes and the consent surface still has to be reviewed.
            // Saying so is the honest version of "adopted".
            println!(
                "  {}",
                "next: `agentstack lock --write` to pin it, then `agentstack trust .` to review it"
                    .dimmed()
            );
            // A skill that belongs to no toolset is declared but unreachable:
            // `use <toolset>` activates a toolset's members, so with toolsets
            // declared, adoption alone never makes it live. Say it here rather
            // than let the user discover it as a silent no-op. (Enrolling is a
            // choice, not a default — which toolset is the user's call.)
            let orphan_skills: Vec<&str> = dropped
                .iter()
                .filter(|i| i.kind == crate::intake::Kind::Skill)
                .map(|i| i.name.as_str())
                .collect();
            if !manifest.profiles.is_empty() && !orphan_skills.is_empty() {
                println!(
                    "  {}",
                    format!(
                        "{} in no toolset yet ({}) — `agentstack edit-profile --profile <toolset> --add-skill <name>` to include it",
                        super::count(orphan_skills.len(), "skill"),
                        orphan_skills.join(", ")
                    )
                    .dimmed()
                );
            }
        }
    } else {
        if args.to_library && !dropped.is_empty() {
            println!(
                "  {} {} would also be saved to the central library",
                "→".cyan(),
                super::count(
                    dropped
                        .iter()
                        .filter(|i| i.kind == crate::intake::Kind::Skill)
                        .count(),
                    "skill"
                )
            );
        }
        println!(
            "\nDry run. Re-run with {} to update the manifest.",
            "--write".bold()
        );
    }
    Ok(())
}

/// The manifest body for a dropped file: a path entry, exactly what a
/// hand-written declaration of the same file would be. Nothing is invented —
/// adoption declares content that is already sitting in the project.
pub(crate) fn intake_entry(item: &crate::intake::Item) -> Value {
    match item.kind {
        crate::intake::Kind::Skill => serde_json::json!({ "path": item.rel_path }),
        // `targets` is left off deliberately: omitted means `["*"]`, and a
        // manifest that states its defaults back to the user grows noise.
        crate::intake::Kind::Instruction => serde_json::json!({ "path": item.rel_path }),
    }
}

/// Copy adopted skills into the central library so other projects can use
/// them. Instructions are project-local by nature and are skipped with a word,
/// not silently. A library failure does not undo the manifest write that
/// already succeeded — it is reported and the adoption stands.
fn save_to_library(dropped: &[crate::intake::Item]) -> Result<()> {
    let lib_home = crate::util::paths::lib_home();
    for item in dropped {
        if item.kind != crate::intake::Kind::Skill {
            println!(
                "  {} {} '{}' stays project-local — the library holds skills",
                "·".dimmed(),
                item.kind.noun(),
                item.name
            );
            continue;
        }
        match super::lib::add_skill(
            &lib_home,
            &item.name,
            super::lib::LibSource::Path(&item.abs_path),
            false,
            true,
            false,
        ) {
            Ok(_) => println!("  {} saved '{}' to the library", "✓".green(), item.name),
            Err(e) => println!(
                "  {} '{}' was adopted here but not saved to the library: {}",
                "!".yellow(),
                item.name,
                crate::text::sanitize_line(&format!("{e:#}"))
            ),
        }
    }
    Ok(())
}

/// Parse a target config's text into a JSON-shaped value tree. Existing but
/// malformed configs are errors: silently treating them as empty would make
/// `adopt` claim there is nothing to keep.
fn parse_config(text: &str, format: Format) -> Result<Value> {
    match format {
        Format::Json => serde_json::from_str(text).context("invalid JSON"),
        Format::Toml => {
            let value = toml::from_str::<toml::Value>(text).context("invalid TOML")?;
            serde_json::to_value(value).context("TOML value could not be represented as JSON")
        }
    }
}

fn has_ref(s: &str) -> bool {
    !crate::secret::refs_in(s).is_empty()
}

/// Any string leaf carrying a `${REF}` (extras may nest).
fn value_has_ref(v: &Value) -> bool {
    match v {
        Value::String(s) => has_ref(s),
        Value::Array(a) => a.iter().any(value_has_ref),
        Value::Object(o) => o.values().any(value_has_ref),
        _ => false,
    }
}

/// Pull hand-edited fields of a manifest-known server into `entry` (the
/// manifest definition being updated): every canonical field — and every
/// per-target extra key the manifest already carries — where `disk` (the
/// on-disk entry) disagrees with `rendered` (the manifest's rendered form for
/// this target). Both sides went through the same adapter lens, so adapter
/// transforms compare equal and only real edits surface.
///
/// A rendered value still carrying a `${REF}` (unresolved secret) is skipped:
/// without the secret, equality with the disk literal can't be judged, and a
/// false diff would copy a stale literal over the reference. Fields where the
/// manifest's ref DID resolve compare against the resolved form, so a rotated
/// on-disk token is picked up (and re-lifted by `lift_secrets` afterwards).
///
/// Returns the labels of the adopted fields. Idempotent across targets: a
/// value `entry` already carries is never re-adopted or re-reported.
fn adopt_changed_fields(
    entry: &mut Server,
    rendered: &Server,
    disk: &Server,
    adapter_id: &str,
) -> Vec<String> {
    let mut changed = Vec::new();

    if rendered.server_type != disk.server_type && entry.server_type != disk.server_type {
        entry.server_type = disk.server_type;
        changed.push("type".to_string());
    }
    adopt_scalar(
        &mut entry.url,
        &rendered.url,
        &disk.url,
        "url",
        &mut changed,
    );
    adopt_scalar(
        &mut entry.command,
        &rendered.command,
        &disk.command,
        "command",
        &mut changed,
    );
    adopt_scalar(
        &mut entry.cwd,
        &rendered.cwd,
        &disk.cwd,
        "cwd",
        &mut changed,
    );
    if rendered.args != disk.args
        && entry.args != disk.args
        && !rendered.args.iter().any(|a| has_ref(a))
    {
        entry.args = disk.args.clone();
        changed.push("args".to_string());
    }
    adopt_map(
        &mut entry.headers,
        &rendered.headers,
        &disk.headers,
        "headers",
        &mut changed,
    );
    adopt_map(
        &mut entry.env,
        &rendered.env,
        &disk.env,
        "env",
        &mut changed,
    );

    // Per-target extras: value edits and removals of keys the manifest already
    // renders for this adapter. Hand-ADDED extra keys are `new_extras`' job.
    if let Some(rendered_extra) = rendered.extra.get(adapter_id) {
        let disk_extra = disk.extra.get(adapter_id);
        for (k, rv) in rendered_extra {
            if value_has_ref(rv) {
                continue;
            }
            match disk_extra.and_then(|d| d.get(k)) {
                Some(dv) if dv != rv => {
                    let slot = entry.extra.entry(adapter_id.to_string()).or_default();
                    if slot.get(k) != Some(dv) {
                        slot.insert(k.clone(), dv.clone());
                        changed.push(format!("extra.{adapter_id}.{k}"));
                    }
                }
                Some(_) => {}
                None => {
                    let removed = entry
                        .extra
                        .get_mut(adapter_id)
                        .is_some_and(|slot| slot.shift_remove(k).is_some());
                    if removed {
                        changed.push(format!("extra.{adapter_id}.{k} (removed)"));
                    }
                }
            }
        }
        if entry.extra.get(adapter_id).is_some_and(|m| m.is_empty()) {
            entry.extra.shift_remove(adapter_id);
        }
    }

    changed
}

/// One optional scalar field of [`adopt_changed_fields`]' contract: follow
/// disk when it disagrees with the rendered form, unless the rendered value
/// still carries an unresolved `${REF}` or `entry` already has the disk value.
fn adopt_scalar(
    entry: &mut Option<String>,
    rendered: &Option<String>,
    disk: &Option<String>,
    label: &str,
    changed: &mut Vec<String>,
) {
    if rendered == disk
        || rendered.as_deref().is_some_and(has_ref)
        || entry.as_deref() == disk.as_deref()
    {
        return;
    }
    *entry = disk.clone();
    changed.push(label.to_string());
}

/// Per-key map (headers/env) counterpart of [`adopt_scalar`]: edited and added
/// keys follow disk; a key the render carries but disk dropped is removed.
fn adopt_map(
    entry: &mut IndexMap<String, String>,
    rendered: &IndexMap<String, String>,
    disk: &IndexMap<String, String>,
    prefix: &str,
    changed: &mut Vec<String>,
) {
    for (k, dv) in disk {
        match rendered.get(k) {
            Some(rv) if rv == dv || has_ref(rv) => continue,
            _ => {}
        }
        if entry.get(k) == Some(dv) {
            continue;
        }
        entry.insert(k.clone(), dv.clone());
        changed.push(format!("{prefix}.{k}"));
    }
    for (k, rv) in rendered {
        if disk.contains_key(k) || has_ref(rv) {
            continue;
        }
        if entry.shift_remove(k).is_some() {
            changed.push(format!("{prefix}.{k} (removed)"));
        }
    }
}

/// The per-target extras in `imported` (a server extracted from a live config)
/// that `existing` (the manifest entry) doesn't carry yet — the adoptable
/// delta for an already-managed server.
fn new_extras(existing: &Server, imported: Server) -> IndexMap<String, IndexMap<String, Value>> {
    imported
        .extra
        .into_iter()
        .filter_map(|(target, fields)| {
            let have = existing.extra.get(&target);
            let fresh: IndexMap<String, Value> = fields
                .into_iter()
                .filter(|(k, _)| have.is_none_or(|h| !h.contains_key(k)))
                .collect();
            (!fresh.is_empty()).then_some((target, fresh))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    fn server(toml_str: &str) -> Server {
        toml::from_str(toml_str).unwrap()
    }

    #[test]
    fn malformed_target_configs_are_errors_not_empty_states() {
        let json = parse_config("{not-json", Format::Json).unwrap_err();
        assert!(format!("{json:#}").contains("invalid JSON"));
        let toml = parse_config("[broken", Format::Toml).unwrap_err();
        assert!(format!("{toml:#}").contains("invalid TOML"));
    }

    #[test]
    fn new_extras_reports_only_missing_keys() {
        // Manifest entry already carries one codex extra; the live config adds
        // startup_timeout_sec (hand-tuned) and repeats the one we have.
        let existing = server("type = \"stdio\"\ncommand = \"npx\"\n[extra.codex]\nnote = \"x\"");
        let imported = server(
            "type = \"stdio\"\ncommand = \"npx\"\n\
             [extra.codex]\nnote = \"x\"\nstartup_timeout_sec = 20",
        );
        let delta = new_extras(&existing, imported);
        assert_eq!(delta.len(), 1);
        assert_eq!(delta["codex"].len(), 1);
        assert_eq!(delta["codex"]["startup_timeout_sec"], serde_json::json!(20));

        // Nothing new → empty delta (adopt stays a no-op).
        let same = server("type = \"stdio\"\ncommand = \"npx\"\n[extra.codex]\nnote = \"x\"");
        assert!(new_extras(&existing, same).is_empty());
    }

    #[test]
    fn new_extras_never_touches_existing_values() {
        // A key present in both keeps the manifest's value: it is not part of
        // new_extras' delta. Edited values of manifest-known keys are
        // adopt_changed_fields' job (rendered-vs-disk, so an unresolved
        // ${REF} can't be clobbered by its stale literal).
        let existing =
            server("type = \"stdio\"\ncommand = \"npx\"\n[extra.codex]\nstartup_timeout_sec = 120");
        let imported =
            server("type = \"stdio\"\ncommand = \"npx\"\n[extra.codex]\nstartup_timeout_sec = 20");
        assert!(new_extras(&existing, imported).is_empty());
    }

    #[test]
    fn hand_edited_url_adopts_and_next_apply_is_a_noop() {
        // The reference.md promise: "edited on disk since last apply → adopt
        // pulls it into the manifest." Repro of the bug this guards against:
        // apply, hand-edit the url in .mcp.json, adopt — the manifest must
        // pick up the edit so the next apply no longer reverts it.
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());
        std::env::set_var("AGENTSTACK_HOME", home.child(".agentstack").path());

        let proj = assert_fs::TempDir::new().unwrap();
        proj.child("agentstack.toml")
            .write_str(
                "version = 1\n\n# docs server for the team\n[servers.docs]\ntype = \"http\"\n\
                 url = \"https://docs.example/mcp\"\ntargets = [\"claude-code\"]\n",
            )
            .unwrap();

        // Adoption is this test's subject, not consent: grant so the rendered
        // lane's trust gate (`render::apply::trust_refusal`) is out of the way
        // and the fixture's first write can stand in for a real `apply`.
        crate::trust::trust_unreviewed(proj.path()).unwrap();

        // `apply --scope project --write`, then a hand-edit of the url.
        let reg = crate::adapter::Registry::load().unwrap();
        let desc = reg.get("claude-code").unwrap();
        let resolver = crate::secret::MapResolver::default();
        let manifest: crate::manifest::Manifest =
            toml::from_str(&fs::read_to_string(proj.child("agentstack.toml").path()).unwrap())
                .unwrap();
        plan_target_with_servers(
            desc,
            &resolver,
            &Default::default(),
            &manifest.servers,
            &[],
            Scope::Project,
            proj.path(),
            crate::render::PriorTrust::STRICT,
        )
        .unwrap()
        .unwrap()
        .write()
        .unwrap();
        let mcp_path = proj.child(".mcp.json");
        let edited = fs::read_to_string(mcp_path.path())
            .unwrap()
            .replace("https://docs.example/mcp", "https://docs-eu.example/mcp");
        fs::write(mcp_path.path(), &edited).unwrap();

        // Adopt pulls the edited url back into the manifest…
        let args = crate::cli::AdoptArgs {
            targets: vec!["claude-code".into()],
            scope: Some(Scope::Project),
            write: true,
            no_keychain: true,
            to_library: false,
        };
        run(&args, Some(proj.path())).unwrap();
        let manifest_text = fs::read_to_string(proj.child("agentstack.toml").path()).unwrap();
        assert!(
            manifest_text.contains("https://docs-eu.example/mcp"),
            "{manifest_text}"
        );
        assert!(
            !manifest_text.contains("url = \"https://docs.example/mcp\""),
            "{manifest_text}"
        );
        assert!(
            manifest_text.contains("# docs server for the team"),
            "comments above the server table survive: {manifest_text}"
        );

        // …and the next apply proposes no change: the hand-edit survives.
        let manifest: crate::manifest::Manifest = toml::from_str(&manifest_text).unwrap();
        let plan = plan_target_with_servers(
            desc,
            &resolver,
            &Default::default(),
            &manifest.servers,
            &[],
            Scope::Project,
            proj.path(),
            crate::render::PriorTrust::STRICT,
        )
        .unwrap()
        .unwrap();
        std::env::remove_var("AGENTSTACK_HOME");
        std::env::remove_var("HOME");
        assert!(!plan.changed(), "apply must be a no-op:\n{}", plan.diff());
    }
}
