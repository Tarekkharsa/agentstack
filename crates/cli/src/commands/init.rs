//! Project-import writes are transactional and undoable; machine initialization
//! is a separate path described below.
//!
//! `agentstack init` — never a blank page. Detect installed CLIs, import their
//! existing MCP servers into one manifest, and lift inline secrets into
//! `${REF}`s whose values land wherever you choose (P2) — a gitignored project
//! `.env` (the default), the OS keychain, or skipped for you to provide later.
//!
//! Every file this writes — the manifest, a created/updated `.env`, and the
//! `.gitignore` line that keeps `.env` out of git — is captured in the same undo
//! ledger `restore` reads (P30). Keychain values deliberately never enter file
//! history; setup names their explicit `secret rm` recovery command.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use owo_colors::OwoColorize;

use crate::adapter::{extract_servers_with_skips, extract_settings, Registry};
use crate::cli::{InitArgs, SecretStore};
use crate::discover::{lift_secrets, merge_servers, Lifted};
use crate::manifest::load::MANIFEST_FILE;
use crate::manifest::model::{Manifest, Meta, Server, Targets};
use crate::secret::{env_file, keychain};

/// Store lifted secret values, collecting the references whose store write
/// failed instead of aborting init or silently dropping them. The manifest
/// holds `${REF}`s either way; an unstored value simply stays unresolved and
/// every use site fails closed on it by name (rule 5) — so the honest behavior
/// is to finish init and report the gap, never abort halfway (the old
/// interactive path) or pretend it stored (a non-interactive UI path).
#[cfg(test)]
fn store_lifted(lifted: &[Lifted], mut store: impl FnMut(&str, &str) -> Result<()>) -> Vec<String> {
    let mut unstored = Vec::new();
    for l in lifted {
        if store(&l.reference, &l.value).is_err() {
            unstored.push(l.reference.clone());
        }
    }
    unstored
}

/// In-memory rollback metadata for keychain writes made before the manifest is
/// durable. Values never enter file history; they live only for this call so a
/// later file/history failure can restore the credential store exactly.
struct KeychainChange {
    name: String,
    before: Option<String>,
}

fn store_lifted_reversibly(lifted: &[Lifted]) -> (Vec<String>, Vec<KeychainChange>) {
    let mut unstored = Vec::new();
    let mut changes = Vec::new();
    for lifted_secret in lifted {
        // Do not overwrite a value we cannot snapshot: without `before`, a
        // later rollback could destroy a pre-existing credential.
        let Ok(before) = keychain::get(&lifted_secret.reference) else {
            unstored.push(lifted_secret.reference.clone());
            continue;
        };
        if keychain::set(&lifted_secret.reference, &lifted_secret.value).is_err() {
            unstored.push(lifted_secret.reference.clone());
            continue;
        }
        changes.push(KeychainChange {
            name: lifted_secret.reference.clone(),
            before,
        });
    }
    (unstored, changes)
}

fn rollback_keychain(changes: &[KeychainChange]) -> Result<()> {
    for change in changes.iter().rev() {
        match &change.before {
            Some(value) => keychain::set(&change.name, value)?,
            None => {
                keychain::delete(&change.name)?;
            }
        }
    }
    Ok(())
}

/// Decide where lifted token values go (P2). Explicit flags always win; an
/// interactive run with no flag prompts; otherwise the non-interactive default
/// is the keychain — CI and scripts must never *start* writing plaintext files
/// just because init grew a new option. `allow_prompt` is false on the dry-run
/// path (a preview must never block on a prompt).
fn resolve_secret_store(args: &InitArgs, allow_prompt: bool) -> Result<SecretStore> {
    if let Some(store) = args.secrets {
        return Ok(store);
    }
    // `--no-keychain` is the deprecated alias for `--secrets skip`.
    if args.no_keychain {
        return Ok(SecretStore::Skip);
    }
    if allow_prompt && crate::util::confirm::is_interactive() {
        return prompt_secret_store();
    }
    Ok(SecretStore::Keychain)
}

/// The P2 storage menu, shown when init lifts tokens interactively. `.env` is
/// preselected as the maintainer's decided default: it is what users already
/// know, and the guard deny-list plus the managed gitignore are what make the
/// plaintext default defensible.
///
/// The full multi-line help prints once above the selector; on a real terminal
/// the choice is an arrow-key `dialoguer::Select` (matching the wizard's mode
/// fork). A non-TTY caller falls back to the numbered stdin prompt so a piped
/// run never panics inside dialoguer — this function is only reached after the
/// caller checked `is_interactive()`, so the fallback is belt-and-suspenders.
fn prompt_secret_store() -> Result<SecretStore> {
    print_secret_store_help();
    if crate::util::confirm::is_interactive() {
        // Each item carries the terse consequence; the full help is above.
        let items = [
            "Project .env  (default) — plaintext file next to the manifest, gitignored, guard-blocked",
            "macOS keychain — migrated into the system keychain (service `agentstack`)",
            "Skip / decide later — write only ${REF} placeholders; nothing runs until provided",
        ];
        let idx = dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
            .with_prompt("Where should these token values live?")
            .items(items)
            .default(0)
            .interact()?;
        Ok(secret_store_at(idx))
    } else {
        read_numbered_secret_choice()
    }
}

/// Print the three storage options' full help text plus the varlock note — the
/// context that prints once, above whichever selector runs.
fn print_secret_store_help() {
    println!("\nWhere should these token values live?\n");
    println!(
        "  {}) Project .env  (default) — Your tokens are written to .env next to the",
        "1".bold()
    );
    println!("     manifest, in plain text. agentstack keeps this file out of git and its");
    println!("     guard blocks agents from reading it. Edit it with any editor.");
    println!(
        "  {}) macOS keychain — Your tokens are migrated into the system keychain",
        "2".bold()
    );
    println!("     (service `agentstack`). Nothing secret sits in a file. View or change");
    println!("     them in Keychain Access, or with `agentstack secret set <NAME>`.");
    println!(
        "  {}) Skip / decide later — Only ${{REF}} placeholders are written. Nothing runs",
        "3".bold()
    );
    println!("     until you provide values (env, varlock, keychain, or .env) —");
    println!("     `agentstack doctor` lists what's missing.");
    println!(
        "\n  {}",
        "Already using 1Password or a secrets manager? Drop a .env.schema in the".dimmed()
    );
    println!(
        "  {}",
        "project and refs resolve through varlock instead.".dimmed()
    );
}

/// Non-TTY fallback: the numbered stdin prompt (the shape that predated the
/// arrow-key selector). Never panics on a pipe — a closed stdin reads empty and
/// falls through to the `.env` default via `parse_secret_choice`.
fn read_numbered_secret_choice() -> Result<SecretStore> {
    use std::io::Write;
    print!("\nChoice [1]: ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok();
    Ok(parse_secret_choice(&line))
}

/// The store at a 0-based `Select` index: 0 → `.env` (default), 1 → keychain,
/// 2 → skip. Pure, so the index mapping is unit-testable without a terminal.
fn secret_store_at(idx: usize) -> SecretStore {
    match idx {
        1 => SecretStore::Keychain,
        2 => SecretStore::Skip,
        _ => SecretStore::Env,
    }
}

/// Map the numbered-prompt input to a store. Bare Enter (empty), `1`, or
/// anything unrecognized selects the `.env` default — the safe, familiar choice
/// for a write; only an explicit `2`/`3` picks the alternatives.
fn parse_secret_choice(input: &str) -> SecretStore {
    match input.trim() {
        "2" => SecretStore::Keychain,
        "3" => SecretStore::Skip,
        _ => SecretStore::Env,
    }
}

/// Report `${REF}`s the keychain refused to store (unreachable credential
/// store), each with the exact command to store it later.
fn report_unstored_keychain(unstored: &[String]) {
    println!(
        "{}  {}",
        "⚠".yellow(),
        format!(
            "The OS credential store is unreachable — {} value(s) not stored:",
            unstored.len()
        )
        .yellow()
        .bold()
    );
    for r in unstored {
        println!(
            "      {}   agentstack secret set {r}",
            format!("${{{r}}}").yellow()
        );
    }
    println!(
        "      {}",
        "The manifest keeps ${REF}s. Provide values via env, varlock, or a project .env; apply/run block on unresolved refs by name.".dimmed()
    );
}

/// Report the values init deliberately did NOT store (skip path), each with the
/// one-liner to store it. This replaces `--no-keychain`'s old silent value-drop.
fn report_skipped(lifted: &[Lifted]) {
    println!(
        "{}  {}",
        "·".dimmed(),
        format!(
            "{} token(s) not stored — provide each before running:",
            lifted.len()
        )
        .bold()
    );
    let width = lifted.iter().map(|l| l.reference.len()).max().unwrap_or(0);
    for l in lifted {
        println!(
            "      {} {}  agentstack secret set {}",
            format!("${{{}}}", l.reference).yellow(),
            " ".repeat(width.saturating_sub(l.reference.len())),
            l.reference
        );
    }
}

pub fn run(args: &InitArgs, manifest_dir: Option<&Path>) -> Result<()> {
    // The TTY probe is injected so the non-interactive refusal below is
    // testable without a real terminal (the same seam as `trust::grant_gated`).
    run_gated(args, manifest_dir, crate::util::confirm::is_interactive())
}

/// The `init` dispatch with the interactive probe injected. `interactive` is
/// whether this is an attended terminal session; production passes
/// `crate::util::confirm::is_interactive()`.
fn run_gated(args: &InitArgs, manifest_dir: Option<&Path>, interactive: bool) -> Result<()> {
    if args.plan {
        // Read-only, so it bypasses the bare/TTY gating below by design; the
        // global template has no detection to plan over.
        anyhow::ensure!(
            !args.global,
            "--plan applies to project import, not --global"
        );
        return run_plan(args, manifest_dir);
    }
    if args.global {
        return run_global(args);
    }
    // A truly flagless invocation: no flag opts into either the guided path or
    // the scripted primitive. `--yes` counts as an init-shaping flag — it is
    // the explicit acknowledgement that the scripted import will write.
    let bare =
        !args.force && !args.dry_run && args.secrets.is_none() && !args.no_keychain && !args.yes;
    if bare {
        // P27 — one verb: a bare interactive `init` IS the guided wizard (the
        // former `setup`).
        if interactive {
            let wizard = crate::cli::SetupArgs {
                targets: Vec::new(),
                profile: None,
                scope: None,
            };
            return super::setup::run(&wizard, manifest_dir);
        }
        // Non-TTY with no flags: refuse before writing anything. A flagless
        // `init` here would import configs and lift live token values into
        // files with no prompt — the help promises scripts opt in via flags, so
        // honor it. Naming both escapes keeps the scripted path discoverable.
        //
        // But adapt to state first: when a manifest already exists, the
        // generic escapes mislead — `--yes` walks into the --force wall and
        // `--dry-run` previews a from-scratch replacement. The scripted next
        // steps for an initialized project are the render/activate commands.
        if let Some(path) = existing_manifest(manifest_dir)? {
            return Err(already_initialized(&path));
        }
        anyhow::bail!(
            "refusing to init without a terminal: a flagless `agentstack init` imports your \
             CLI configs and can lift live token values into files, so it never runs without \
             a prompt or an explicit flag\n\
             \n  \
             preview only (writes nothing):  agentstack init --dry-run\n  \
             import without prompts:         agentstack init --yes   (secrets → keychain)\n  \
             choose the secret store:        agentstack init --secrets <env|keychain|skip>\n\
             \n\
             (in a terminal, plain `agentstack init` is the guided wizard)"
        );
    }
    // Any explicit flag (or --yes) proceeds promptlessly as the scriptable
    // primitive: import, write, no prompts beyond what flags allow.
    run_impl(args, manifest_dir, true)
}

/// `init --plan` — Lane A's read primitive (UI control-plane §4): run init's
/// DETECTION only and emit the import plan as structured JSON. Writes
/// nothing, prompts nothing, stores nothing. Reuses the exact discovery and
/// secret-lifting code paths the real import runs — this is the same plan,
/// minus the writes — and emits only each lifted secret's `${REF}` name and
/// origin, NEVER its value: the values live in memory for the lifetime of
/// this call and are dropped.
///
/// The emitted `plan_digest` identifies this exact plan: a later scripted
/// apply may present it as `--consented-plan` and the write then refuses if
/// re-running detection yields a different plan — the same reviewed-bytes
/// binding `trust --preview` / `--consented-digest` gives the trust grant.
fn run_plan(args: &InitArgs, manifest_dir: Option<&Path>) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&crate::ui_contract::envelope(plan_json(
            args,
            manifest_dir
        )?))?
    );
    Ok(())
}

/// The plan `--plan` prints, without the envelope: body plus `plan_digest`.
/// Public read API so integrations and the race witnesses exercise the exact
/// production plan/digest pair instead of re-deriving one.
pub fn plan_json(args: &InitArgs, manifest_dir: Option<&Path>) -> Result<serde_json::Value> {
    let base = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let dir = crate::manifest::new_manifest_dir(&base);
    let manifest_path = dir.join(MANIFEST_FILE);
    let already_initialized = existing_manifest(manifest_dir)?.is_some();
    let det = detect_import(&dir)?;
    let destination = store_label(resolve_secret_store(args, false)?);
    let digest = plan_digest(&det, &base, already_initialized, destination);

    // Imported names/targets come from other CLIs' config files — hostile
    // input; sanitize display strings exactly like the trust preview.
    let servers_json: Vec<serde_json::Value> = det
        .servers
        .iter()
        .map(|(name, s)| {
            let (kind, target) = match s.server_type {
                crate::manifest::ServerType::Stdio => (
                    "stdio",
                    format!(
                        "{} {}",
                        s.command.as_deref().unwrap_or("?"),
                        s.args.join(" ")
                    )
                    .trim()
                    .to_string(),
                ),
                crate::manifest::ServerType::Http => ("http", s.url.clone().unwrap_or_default()),
            };
            let mut entry = serde_json::json!({
                "name": crate::text::sanitize_line(name),
                "kind": kind,
                "target": crate::text::sanitize_line(&target),
            });
            // Operational context the digest binds — surfaced so a reviewer
            // sees what distinguishes two otherwise identical-looking plans.
            // Env VAR NAMES only: values may hold non-lifted plaintext.
            if let serde_json::Value::Object(map) = &mut entry {
                if !s.env.is_empty() {
                    let names: Vec<String> = s
                        .env
                        .keys()
                        .map(|k| crate::text::sanitize_line(k))
                        .collect();
                    map.insert("env".into(), names.into());
                }
                if let Some(cwd) = &s.cwd {
                    map.insert("cwd".into(), crate::text::sanitize_line(cwd).into());
                }
            }
            entry
        })
        .collect();

    Ok(serde_json::json!({
        "path": base.display().to_string(),
        "manifest_path": manifest_path.display().to_string(),
        "already_initialized": already_initialized,
        "detected": det
            .detected_ids
            .iter()
            .zip(&det.display_names)
            .map(|(id, display)| serde_json::json!({ "id": id, "display": display }))
            .collect::<Vec<_>>(),
        "servers": servers_json,
        "settings_from": det.settings.keys().collect::<Vec<_>>(),
        "conflicts": det
            .conflict_counts
            .iter()
            .map(|(name, extra)| serde_json::json!({
                "name": crate::text::sanitize_line(name),
                "other_definitions": extra,
            }))
            .collect::<Vec<_>>(),
        "secrets": det
            .lifted
            .iter()
            .map(|l| serde_json::json!({
                "reference": l.reference,
                "origin": crate::text::sanitize_line(&l.origin),
            }))
            .collect::<Vec<_>>(),
        "secrets_destination": destination,
        // Lossy-import honesty (Stage 1.2): entries the import must leave in
        // their CLI's own config, each with a plain-language reason. Purely
        // informational — they never enter the written manifest, so they do
        // not participate in the plan digest.
        "unsupported": det
            .skipped
            .iter()
            .map(|(cli, s)| serde_json::json!({
                "cli": cli,
                "name": crate::text::sanitize_line(&s.name),
                "reason": s.reason,
            }))
            .collect::<Vec<_>>(),
        "plan_digest": digest,
    }))
}

/// Everything one detection pass finds — computed ONCE and consumed by both
/// the plan (display + digest) and the consented write, so the plan a user
/// reviewed and the import the write performs are the same in-memory objects,
/// never two detections that could observe different disk states
/// (independent review, 2026-07-23).
struct DetectedImport {
    detected_ids: Vec<String>,
    display_names: Vec<String>,
    /// Display names of the CLIs that actually contributed servers or
    /// settings — the honest "imported from" list (a detected CLI with an
    /// empty config is not a source).
    contributing: Vec<String>,
    /// Post-lift: inline token values already rewritten to `${REF}`.
    servers: IndexMap<String, Server>,
    /// Full imported settings values per contributing CLI id — exactly what
    /// the written manifest will hold.
    settings: IndexMap<String, serde_json::Value>,
    conflict_counts: IndexMap<String, usize>,
    lifted: Vec<crate::discover::Lifted>,
    /// Entries a CLI's config declares that the import had to leave behind,
    /// as `(cli display name, skip)` — surfaced in the plan and the write
    /// output so a lossy import is explained, never silent.
    skipped: Vec<(String, crate::adapter::SkippedImport)>,
}

fn detect_import(dir: &Path) -> Result<DetectedImport> {
    let registry = Registry::load()?;
    let mut detected_ids: Vec<String> = Vec::new();
    let mut display_names: Vec<String> = Vec::new();
    let mut contributing: Vec<String> = Vec::new();
    let mut servers: IndexMap<String, Server> = IndexMap::new();
    let mut settings: IndexMap<String, serde_json::Value> = IndexMap::new();
    let mut conflict_counts: IndexMap<String, usize> = IndexMap::new();
    let mut skipped: Vec<(String, crate::adapter::SkippedImport)> = Vec::new();
    for desc in registry.iter() {
        if !desc.detected() {
            continue;
        }
        detected_ids.push(desc.id.clone());
        display_names.push(desc.display.clone());
        let mut contributed = false;
        if let Some(value) = desc.read_config_value()? {
            let (imported, skips) = extract_servers_with_skips(desc, &value);
            skipped.extend(skips.into_iter().map(|s| (desc.display.clone(), s)));
            contributed |= !imported.is_empty();
            for c in merge_servers(&mut servers, imported) {
                *conflict_counts.entry(c).or_insert(0usize) += 1;
            }
        }
        if let Some(value) = desc.read_settings_value(dir)? {
            let imported = extract_settings(desc, &value);
            if !imported.is_empty() {
                contributed = true;
                settings.insert(desc.id.clone(), serde_json::Value::Object(imported));
            }
        }
        if contributed {
            contributing.push(desc.display.clone());
        }
    }
    // Lifting rewrites the in-memory servers to `${REF}` placeholders and
    // returns the values; only reference + origin ever serialize.
    let lifted = lift_secrets(&mut servers);
    Ok(DetectedImport {
        detected_ids,
        display_names,
        contributing,
        servers,
        settings,
        conflict_counts,
        lifted,
        skipped,
    })
}

fn store_label(store: SecretStore) -> &'static str {
    match store {
        SecretStore::Env => "env",
        SecretStore::Keychain => "keychain",
        SecretStore::Skip => "skip",
    }
}

/// The stable identity of a computed plan (v2): a domain-separated digest
/// over the COMPLETE import — full `Server` objects (env, cwd, headers, argv
/// as arrays), imported settings values, conflicts, secret reference names
/// and origins (never values), and the destination store. v1 hashed the
/// sanitized display summary, which omitted operational fields and flattened
/// argv with spaces, so two plans that would write different manifests could
/// share a digest (independent review, 2026-07-23).
fn plan_digest(
    det: &DetectedImport,
    base: &Path,
    already_initialized: bool,
    destination: &str,
) -> String {
    use sha2::{Digest, Sha256};
    let full = serde_json::json!({
        "path": base.display().to_string(),
        "already_initialized": already_initialized,
        "detected": det.detected_ids,
        "servers": serde_json::to_value(&det.servers)
            .expect("derive(Serialize) manifest types always serialize"),
        "settings": det.settings,
        "conflicts": det.conflict_counts,
        "secrets": det
            .lifted
            .iter()
            .map(|l| serde_json::json!({ "reference": l.reference, "origin": l.origin }))
            .collect::<Vec<_>>(),
        "secrets_destination": destination,
    });
    let mut hasher = Sha256::new();
    hasher.update(b"agentstack:init-plan:v2\n");
    hasher.update(full.to_string().as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

/// Template for the machine-level manifest. Deliberately NOT an import: the
/// personal layer starts empty and explicit — it carries intent that travels
/// with the user (instruction fragments, eventually more), not a copy of
/// whatever the CLIs happen to hold today (that's project `init`'s job).
const GLOBAL_MANIFEST_TEMPLATE: &str = "\
# Machine-level agentstack manifest — the personal layer.
# Cross-project intent that travels with YOU, not with a repo: instruction
# fragments compiled into each CLI's global CLAUDE.md / AGENTS.md.
#
# Declare a fragment, drop its markdown in ./instructions/, then compile:
#
#   [instructions.style]
#   path = \"./instructions/style.md\"   # relative to this directory
#   targets = [\"*\"]                     # or [\"claude-code\", \"codex\"]
#
version = 1

[instructions]
";

/// `agentstack init --global` — seed `~/.agentstack/agentstack.toml` (honoring
/// `AGENTSTACK_HOME`) with an empty `[instructions]` block and an
/// `instructions/` dir. This blesses the home layer as a first-class manifest:
/// `agentstack instructions` run from `$HOME` (or with `--manifest-dir`)
/// compiles its fragments into each CLI's global instruction file. The
/// zero-files gateway deliberately never discovers this layer as a project
/// (see `manifest::discover_project_base`).
fn run_global(args: &InitArgs) -> Result<()> {
    let home = crate::util::paths::agentstack_home();
    let manifest_path = home.join(MANIFEST_FILE);
    let instr_dir = home.join("instructions");
    if manifest_path.exists() && !args.force && !args.dry_run {
        anyhow::bail!(
            "{} already exists — use --force to overwrite or --dry-run to preview",
            manifest_path.display()
        );
    }

    // Preview before ANY filesystem write (and before the house-rules prompt).
    // The preview is the SEEDED template — [guard] + [policy.filesystem]
    // included — because seed_machine_toml is pure (A1 witness: --dry-run
    // shows the policy blocks and writes nothing).
    if args.dry_run {
        let seeded = super::guard::seed_machine_toml(GLOBAL_MANIFEST_TEMPLATE)?;
        println!("\n{} (preview — nothing written)\n", MANIFEST_FILE.bold());
        println!("{seeded}");
        println!("Would write {}", manifest_path.display());
        println!("Would create {}/", instr_dir.display());
        println!(
            "Would offer to install the host guard into detected CLIs \
             (never installed without an explicit yes)."
        );
        println!(
            "Would offer the agentstack house rules fragment ([instructions.{HOUSE_RULES_NAME}])."
        );
        return Ok(());
    }
    if manifest_path.exists() {
        // --force: start over from the template (ensure_global_manifest would
        // keep the existing file).
        std::fs::remove_file(&manifest_path)
            .with_context(|| format!("removing {}", manifest_path.display()))?;
    }

    ensure_global_manifest()?;
    // Seed [guard] + [policy.filesystem] deny through the SAME path as
    // `guard install` — one canonical default list, idempotent, and an
    // explicitly-empty user deny list is respected as an opt-out.
    super::guard::seed_machine_config()?;
    println!("{}  Wrote {}", "✅".dimmed(), manifest_path.display());
    println!("{}  Created {}/", "📁".dimmed(), instr_dir.display());
    println!(
        "{}  Seeded [guard] + [policy.filesystem] deny ({} default entries — edit anytime)",
        "🛡️".dimmed(),
        super::guard::DEFAULT_DENY.len()
    );

    // The guard-install offer (A1). Never silent: installing edits other
    // CLIs' config files, so it happens only on an explicit yes — and
    // `confirm` returns false without prompting when non-interactive, which
    // is exactly t3code/CI contract (report the pending offer, never
    // auto-install).
    println!(
        "\nThe host guard enforces that deny list inside each CLI's own hook system —\n\
         it blocks accidental secret reads and destructive commands; it is not a sandbox."
    );
    let detected = super::guard::detected_target_ids();
    if detected.is_empty() {
        println!(
            "  {} no hook-capable CLIs detected — run `agentstack guard install` after installing one.",
            "·".dimmed()
        );
    } else {
        println!("  Detected CLIs: {}", detected.join(" · "));
        if crate::util::confirm::confirm(&format!(
            "Install the guard into these {} CLI(s)?",
            detected.len()
        ))? {
            super::guard::install()?;
        } else {
            println!(
                "  {} skipped — run `agentstack guard install` anytime.",
                "·".dimmed()
            );
        }
    }

    // Offer the agentstack house rules — the fragment that teaches every agent
    // the manifest-first workflow. Opt-in (it steers the daily-driver agent),
    // like pack instructions. Non-interactive shells skip; `setup` re-offers.
    if crate::util::confirm::confirm(
        "\nInstall the agentstack house rules fragment (teaches agents the manifest-first workflow)?",
    )? {
        if seed_house_rules(&home)? {
            println!(
                "  {} installed [instructions.{HOUSE_RULES_NAME}] → {}/{HOUSE_RULES_NAME}.md",
                "✓".green(),
                instr_dir.display()
            );
        }
    } else {
        println!(
            "  {} skipped — `agentstack init` will offer them again.",
            "·".dimmed()
        );
    }

    println!(
        "\nNext: drop fragments in {}/, declare them under [instructions.*], then:",
        instr_dir.display()
    );
    println!("    {}", instructions_hint(&home).bold());
    Ok(())
}

/// Name and bundled source of the agentstack house-rules fragment, shared by
/// `init --global` and `setup` so both seed the same thing.
pub const HOUSE_RULES_NAME: &str = "agentstack";
const HOUSE_RULES_ASSET: &str = "instructions/agentstack/rules.md";

/// Ensure the machine-level manifest exists (seeding the template and the
/// `instructions/` dir if needed); returns the home manifest dir.
pub fn ensure_global_manifest() -> Result<PathBuf> {
    let home = crate::util::paths::agentstack_home();
    let manifest_path = home.join(MANIFEST_FILE);
    let instr_dir = home.join("instructions");
    std::fs::create_dir_all(&instr_dir)
        .with_context(|| format!("creating {}", instr_dir.display()))?;
    if !manifest_path.exists() {
        crate::util::atomic::write(&manifest_path, GLOBAL_MANIFEST_TEMPLATE)
            .with_context(|| format!("writing {}", manifest_path.display()))?;
    }
    Ok(home)
}

/// Install the agentstack house-rules fragment into the manifest at `dir`:
/// extract the bundled markdown to `instructions/agentstack.md` (an existing
/// file — possibly user-edited — is kept) and declare it under
/// `[instructions.agentstack]`, preserving manifest comments. Returns `false`
/// when the manifest already declares the fragment.
pub fn seed_house_rules(dir: &Path) -> Result<bool> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest: Manifest =
        toml::from_str(&text).with_context(|| format!("parsing {}", manifest_path.display()))?;
    if manifest.instructions.contains_key(HOUSE_RULES_NAME) {
        return Ok(false);
    }

    let dest = dir
        .join("instructions")
        .join(format!("{HOUSE_RULES_NAME}.md"));
    if !dest.exists() {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let body = crate::catalog::read_asset_file(HOUSE_RULES_ASSET)?;
        crate::util::atomic::write(&dest, &body)
            .with_context(|| format!("writing {}", dest.display()))?;
    }

    let entry = crate::manifest::Instruction {
        path: format!("./instructions/{HOUSE_RULES_NAME}.md"),
        targets: vec!["*".into()],
        from_user_layer: false,
    };
    let new_text = super::add::build_manifest_with(
        &text,
        "instructions",
        HOUSE_RULES_NAME,
        &serde_json::to_value(&entry)?,
        None,
    )?;
    crate::util::atomic::write(&manifest_path, &new_text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    Ok(true)
}

/// The exact `instructions --write` invocation for the machine-level manifest:
/// plain from `$HOME` when the layer lives at the default `~/.agentstack`,
/// spelled with `--manifest-dir` when `AGENTSTACK_HOME` relocated it.
pub(crate) fn instructions_hint(home: &Path) -> String {
    let default_home = dirs::home_dir().map(|h| h.join(".agentstack"));
    if default_home.as_deref() == Some(home) {
        "agentstack instructions --manifest-dir ~ --write".to_string()
    } else {
        format!(
            "agentstack instructions --manifest-dir {} --write",
            home.display()
        )
    }
}

/// The import step as `setup` drives it: `setup` prints its own guidance and
/// continues automatically, so the standalone "run bootstrap next" tail is
/// suppressed to avoid contradicting the wizard's flow.
pub(crate) fn run_for_setup(args: &InitArgs, manifest_dir: Option<&Path>) -> Result<()> {
    run_impl(args, manifest_dir, false)
}

/// The manifest this invocation would collide with, if one already exists:
/// the explicit `--manifest-dir`'s manifest, or the nearest ancestor
/// project's (the same walk every other command does).
fn existing_manifest(manifest_dir: Option<&Path>) -> Result<Option<std::path::PathBuf>> {
    Ok(match manifest_dir {
        Some(d) => {
            let path = crate::manifest::resolve_manifest_dir(d).join(MANIFEST_FILE);
            path.exists().then_some(path)
        }
        None => crate::manifest::discover_project_base(&std::env::current_dir()?)
            .map(|root| crate::manifest::resolve_manifest_dir(&root).join(MANIFEST_FILE)),
    })
}

/// The refusal for a scripted `init` against an already-initialized project.
/// Another import is almost never what the script wants — name the actual
/// next steps, and keep `--force` available but labeled as the destructive
/// path. (Interactive bare `init` never hits this: the wizard resumes.)
fn already_initialized(manifest_path: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "{} already exists — init has nothing left to do here\n\
         \n  \
         render it into your CLIs:  agentstack apply --write\n  \
         activate a profile:        agentstack use <profile> --write\n  \
         re-import from scratch:    agentstack init --force   (replaces the manifest)\n\
         \n\
         (in a terminal, plain `agentstack init` resumes the wizard: preview, apply, verify)",
        manifest_path.display()
    )
}

/// Refuse a bare `init` from inside an already-initialized project: every
/// other command walks up to that root's manifest (`commands::project_base`),
/// so silently creating a NESTED one here would fork the project into two
/// manifests that fight over the same tree. Nesting stays possible, but only
/// deliberately — `--force` or an explicit `--manifest-dir`.
fn refuse_nested_init(cwd: &Path) -> Result<()> {
    if let Some(root) = crate::manifest::discover_project_base(cwd) {
        if root != cwd {
            anyhow::bail!(
                "this project is already initialized at {} — commands run from here \
                 find that manifest; pass --force (or --manifest-dir {}) to nest a \
                 separate project in this directory",
                crate::manifest::resolve_manifest_dir(&root)
                    .join(MANIFEST_FILE)
                    .display(),
                cwd.display()
            );
        }
    }
    Ok(())
}

fn run_impl(args: &InitArgs, manifest_dir: Option<&Path>, show_next: bool) -> Result<()> {
    let base = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => {
            let cwd = std::env::current_dir()?;
            // Same escape hatches as the "already exists" check below:
            // --force nests deliberately, --dry-run only previews.
            if !args.force && !args.dry_run {
                refuse_nested_init(&cwd)?;
            }
            cwd
        }
    };
    // Create new manifests in `.agentstack/`; keep updating a legacy root one.
    let dir = crate::manifest::new_manifest_dir(&base);
    let manifest_path = dir.join(MANIFEST_FILE);
    if manifest_path.exists() && !args.force {
        if !args.dry_run {
            return Err(already_initialized(&manifest_path));
        }
        // The preview below is a fresh re-import, not the file on disk — say
        // so, or a reader assumes init merges and that their current servers
        // survived (they would not: a write replaces the manifest).
        println!(
            "{} existing manifest at {} — this preview shows a fresh re-import, not the file \
             on disk; writing it takes `agentstack init --force` and replaces the manifest",
            "⚠".yellow(),
            manifest_path.display()
        );
    }

    // ONE detection pass: the consent check below and the writes both consume
    // this same instance. A verify-then-redetect sequence would let a CLI
    // config that changed between the two reads be imported (and its token
    // stored) without ever being compared against the reviewed plan
    // (independent review, 2026-07-23).
    let det = detect_import(&dir)?;
    // Reviewed-plan binding: refuse before ANY print or mutation when this
    // detection no longer digests to the reviewed plan. The destination is
    // resolved non-interactively exactly as `--plan` resolved it, so both
    // digests describe the same store choice; the write path below reuses
    // this resolution instead of prompting to a different one.
    let preresolved_store = match args.consented_plan {
        Some(_) => Some(resolve_secret_store(args, false)?),
        None => None,
    };
    if let Some(consented) = args.consented_plan.as_deref() {
        let already = existing_manifest(manifest_dir)?.is_some();
        let store = preresolved_store.expect("resolved right above for Some(consented)");
        let actual = plan_digest(&det, &base, already, store_label(store));
        anyhow::ensure!(
            consented == actual,
            "refusing to apply: the detected setup changed since this plan was reviewed \
             (consented {consented}, current {actual}) — re-run `agentstack init --plan`, \
             review the new plan, and apply with its plan_digest"
        );
    }
    let DetectedImport {
        detected_ids: detected,
        display_names,
        contributing,
        servers,
        settings,
        conflict_counts,
        lifted,
        skipped,
    } = det;
    for (name, extra) in &conflict_counts {
        println!(
            "{} server '{name}' is defined differently by {} other CLI(s) — kept the first \
             definition imported (the others stay in their CLI's own config)",
            "⚠".yellow(),
            extra
        );
    }

    if detected.is_empty() {
        // A clean machine is a first-timer's machine. Refusing to create a
        // manifest here is a circular blocker — every other command's error
        // says "run `agentstack init`" — so scaffold a commented starter
        // manifest instead of importing nothing.
        const STARTER: &str = "\
version = 1

# Fresh manifest — no agent CLIs were detected to import from.
# Declare MCP servers here; secrets stay ${REF} placeholders (never values):
#
# [servers.filesystem]
# type = \"stdio\"
# command = \"npx\"
# args = [\"-y\", \"@modelcontextprotocol/server-filesystem\", \"./\"]
#
# Next steps:
#   agentstack search <query>          find servers/skills in the catalog
#   agentstack add from <id> --write   add one to this manifest
#   agentstack apply                   preview what renders into each CLI
#   agentstack gateway connect --all --write   or skip rendered files entirely:
#   agentstack trust .                 serve this repo through the gateway
";
        if args.dry_run {
            println!("No supported CLIs detected — would write a starter manifest:\n\n{STARTER}");
            return Ok(());
        }
        // Capture the pre-write state (the file is absent → `before: None`, so
        // undo deletes it) BEFORE writing, then record one undoable entry — the
        // same ledger `restore` reads. Best-effort: history never breaks init.
        let cap = crate::history::capture(&manifest_path, "manifest · starter");
        if let Err(err) = crate::util::atomic::write(&manifest_path, STARTER)
            .with_context(|| format!("writing {}", manifest_path.display()))
        {
            let _ = crate::history::rollback(std::slice::from_ref(&cap));
            return Err(err);
        }
        if let Err(err) = crate::history::record("project", Vec::new(), vec![cap.clone()]) {
            crate::history::rollback(&[cap]).context(
                "history recording failed and the starter manifest could not be rolled back",
            )?;
            return Err(err).context("recording the starter manifest for undo");
        }
        println!(
            "No supported CLIs detected to import — wrote a starter manifest instead.\n{}  Wrote {}\n\nAdd a server with `agentstack search <query>` + `agentstack add from <id> --write`,\nor edit the manifest directly (it has a commented example).",
            "✅".dimmed(),
            manifest_path.display()
        );
        return Ok(());
    }

    println!(
        "{}  {} CLI {} on PATH: {}",
        "🔍".dimmed(),
        detected.len(),
        if detected.len() == 1 {
            "binary"
        } else {
            "binaries"
        },
        display_names.join(" · ")
    );
    println!(
        "{}  Imported {} MCP server(s) from existing configs",
        "📥".dimmed(),
        servers.len()
    );
    // Lossy imports are explained, never silent: name each entry the import
    // left behind, why, and that nothing was deleted. Names come from other
    // CLIs' config files — hostile input; sanitize before display.
    for (cli, skip) in &skipped {
        println!(
            "{} not imported from {cli}: '{}' — {}; it stays in {cli}'s own config, \
             nothing was deleted",
            "⚠".yellow(),
            crate::text::sanitize_line(&skip.name),
            skip.reason
        );
    }
    if !settings.is_empty() {
        println!(
            "{}  Imported settings from {} CLI(s)",
            "⚙".dimmed(),
            settings.len()
        );
        println!(
            "      {}",
            "Only settings agentstack understands are imported; every other setting stays in its CLI's own file, untouched.".dimmed()
        );
    }

    // Inline secrets were lifted during detection. This is the moment that
    // matters: plaintext tokens were sitting in live CLI configs — show
    // exactly where each one was.
    if !lifted.is_empty() {
        println!(
            "{}  {} — lifted to secure references:",
            "🔐".dimmed(),
            format!(
                "Found {} plaintext token(s) in your live CLI configs",
                lifted.len()
            )
            .yellow()
            .bold()
        );
        let width = lifted.iter().map(|l| l.reference.len()).max().unwrap_or(0);
        for l in &lifted {
            println!(
                "      {} {}  {}",
                format!("${{{}}}", l.reference).green(),
                " ".repeat(width.saturating_sub(l.reference.len())),
                l.origin.dimmed()
            );
        }
        println!(
            "      {}",
            "The manifest stays commit-safe; real values resolve locally at apply time.".dimmed()
        );
    }

    // Counts for the closing summary — `servers`/`settings` move into the
    // manifest below.
    let server_count = servers.len();
    let settings_count = settings.len();

    // Assemble the manifest.
    let manifest = Manifest {
        version: 1,
        meta: Meta { name: None },
        servers,
        skills: IndexMap::new(),
        profiles: IndexMap::new(),
        instructions: IndexMap::new(),
        settings,
        hooks: IndexMap::new(),
        extensions: IndexMap::new(),
        workflows: IndexMap::new(),
        packs: IndexMap::new(),
        targets: Targets {
            default: detected.clone(),
        },
        policy: Default::default(),
        guard: Default::default(),
        experimental: Default::default(),
    };
    let toml_text = toml::to_string_pretty(&manifest).context("serializing manifest to TOML")?;

    if args.dry_run {
        println!("\n{} (preview — nothing written)\n", MANIFEST_FILE.bold());
        println!("{toml_text}");
        if !lifted.is_empty() {
            // A preview never prompts, so resolve the store non-interactively.
            match preresolved_store.map_or_else(|| resolve_secret_store(args, false), Ok)? {
                SecretStore::Env => println!(
                    "Would store {} secret(s) in .env (gitignored).",
                    lifted.len()
                ),
                SecretStore::Keychain => {
                    println!("Would store {} secret(s) in the OS keychain.", lifted.len())
                }
                SecretStore::Skip => println!(
                    "Would write {} ${{REF}} placeholder(s); values not stored (--secrets skip).",
                    lifted.len()
                ),
            }
        }
        return Ok(());
    }

    // Every file init writes is captured (pre-write) into `backups`, then
    // recorded as ONE undoable history entry below — the same ledger `restore`
    // reads (P30). Capturing before each write is what lets undo restore the
    // exact prior bytes (or delete a file that did not exist before).
    let mut backups: Vec<crate::history::FileChange> = Vec::new();
    let mut keychain_changes: Vec<KeychainChange> = Vec::new();
    let mut secret_notice: Option<String> = None;
    // `${REF}`s whose values are NOT stored anywhere after this init (the skip
    // store, or a keychain that refused a write) — the success summary names
    // each one so "what still needs a value" is never buried in scrollback.
    let mut refs_needing_values: Vec<String> = Vec::new();

    let writes = (|| -> Result<()> {
        // Store lifted secret VALUES in the chosen backend (P2). The manifest
        // only ever holds `${REF}` placeholders. File captures and temporary
        // keychain snapshots make every pre-commit mutation reversible if a
        // later write or the history record fails.
        if !lifted.is_empty() {
            // A consented apply must store into the digested destination —
            // never re-prompt into a different one.
            match preresolved_store.map_or_else(|| resolve_secret_store(args, true), Ok)? {
                SecretStore::Keychain => {
                    let (unstored, changes) = store_lifted_reversibly(&lifted);
                    let stored = changes.len();
                    keychain_changes = changes;
                    if stored > 0 {
                        secret_notice = Some(format!(
                            "{}  Stored {stored} token(s) in the OS keychain (service `agentstack`)",
                            "🔑".dimmed()
                        ));
                    }
                    if !unstored.is_empty() {
                        report_unstored_keychain(&unstored);
                        refs_needing_values = unstored;
                    }
                }
                SecretStore::Env => {
                    let entries: Vec<(String, String)> = lifted
                        .iter()
                        .map(|l| (l.reference.clone(), l.value.clone()))
                        .collect();
                    backups.push(crate::history::capture(
                        &dir.join(".env"),
                        ".env · lifted secrets",
                    ));
                    env_file::write(&dir, &entries)?;
                    let project_root = crate::manifest::project_root_of(&dir);
                    let is_git = project_root.join(".git").exists();
                    if is_git {
                        // Capture before attempting the write. If it was already
                        // ignored, remove the unused capture from the transaction.
                        backups.push(crate::history::capture(
                            &project_root.join(".gitignore"),
                            ".gitignore · .env rule",
                        ));
                        if !env_file::ensure_gitignored(&project_root, true)? {
                            backups.pop();
                        }
                    }
                    secret_notice = Some(format!(
                        "{}  Stored {} token(s) in .env{}",
                        "🔑".dimmed(),
                        entries.len(),
                        if is_git { " (gitignored)" } else { "" }
                    ));
                }
                SecretStore::Skip => {
                    report_skipped(&lifted);
                    refs_needing_values = lifted.iter().map(|l| l.reference.clone()).collect();
                }
            }
        }

        backups.push(crate::history::capture(&manifest_path, "manifest · import"));
        crate::util::atomic::write(&manifest_path, &toml_text)
            .with_context(|| format!("writing {}", manifest_path.display()))?;
        Ok(())
    })();

    if let Err(err) = writes {
        let file_rollback = crate::history::rollback(&backups);
        let keychain_rollback = rollback_keychain(&keychain_changes);
        if let Err(rollback_err) = file_rollback.and(keychain_rollback) {
            return Err(err).context(format!(
                "initialization failed and rollback also failed: {rollback_err:#}"
            ));
        }
        return Err(err).context("initialization failed; completed writes were rolled back");
    }

    // The history record is part of the commit contract. If it cannot be made,
    // restore the files and temporary keychain changes instead of claiming an
    // undo that does not exist.
    if let Err(err) = crate::history::record("project", detected.clone(), backups.clone()) {
        let file_rollback = crate::history::rollback(&backups);
        let keychain_rollback = rollback_keychain(&keychain_changes);
        if let Err(rollback_err) = file_rollback.and(keychain_rollback) {
            return Err(err).context(format!(
                "recording initialization history failed and rollback also failed: {rollback_err:#}"
            ));
        }
        return Err(err)
            .context("recording initialization history failed; writes were rolled back");
    }

    if let Some(notice) = secret_notice {
        println!("{notice}");
    }

    println!("{}  Wrote {}", "✅".dimmed(), manifest_path.display());
    if show_next {
        // The one concise success summary (Stage 1.2): manifest path, source
        // CLIs, what was imported, which secrets still need values, and the
        // exact next commands. The wizard has its own richer close, so this
        // prints only on the scripted primitive.
        print!(
            "{}",
            render_import_summary(
                &manifest_path.display().to_string(),
                // The honest source list: only CLIs that contributed content.
                // A run that imported nothing falls back to what was detected.
                if contributing.is_empty() {
                    &display_names
                } else {
                    &contributing
                },
                server_count,
                settings_count,
                &refs_needing_values,
            )
        );
    }
    Ok(())
}

/// Pure formatter for the scripted-import success summary, so its shape is
/// unit-testable without touching real CLI configs. One block, five facts:
/// manifest path, source CLIs, imported counts, secrets still needing values,
/// and the next commands (`apply --write`, then `doctor`).
fn render_import_summary(
    manifest_path: &str,
    sources: &[String],
    server_count: usize,
    settings_count: usize,
    needing_values: &[String],
) -> String {
    let mut out = String::new();
    out.push_str("\nImport complete.\n");
    out.push_str(&format!(
        "  Manifest:  {manifest_path}   (the source of truth your CLIs render from)\n"
    ));
    out.push_str(&format!("  From:      {}\n", sources.join(" · ")));
    let mut imported = format!("{server_count} MCP server(s)");
    if settings_count > 0 {
        imported.push_str(&format!(" · settings from {settings_count} CLI(s)"));
    }
    out.push_str(&format!("  Imported:  {imported}\n"));
    if !needing_values.is_empty() {
        out.push_str(&format!(
            "  Secrets:   {} still need a value before this setup can run:\n",
            needing_values.len()
        ));
        for name in needing_values {
            out.push_str(&format!("               agentstack secret set {name}\n"));
        }
    }
    out.push_str("  Undo:      agentstack restore --last --write\n");
    out.push_str("  Next:      agentstack apply --write   (render this setup into your CLIs)\n");
    out.push_str("             agentstack doctor          (check the result)\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Consent-fidelity witness (independent review, 2026-07-23): the plan
    /// digest must cover the FULL import the write performs, not the display
    /// summary. v1 flattened argv with spaces and omitted env/cwd, so plans
    /// that would write operationally different manifests shared a digest.
    /// NEVER weaken this to a display-derived digest.
    #[test]
    fn plan_digest_binds_operational_fields_the_display_summary_hides() {
        let base = Path::new("/tmp/proj");
        let mk = |args: &[&str], env_val: &str| {
            let mut servers: IndexMap<String, Server> = IndexMap::new();
            let s: Server = serde_json::from_value(serde_json::json!({
                "type": "stdio",
                "command": "npx",
                "args": args,
                "env": { "MODE": env_val },
            }))
            .expect("valid server literal");
            servers.insert("srv".into(), s);
            DetectedImport {
                detected_ids: vec!["claude-code".into()],
                display_names: vec!["Claude Code".into()],
                contributing: vec!["Claude Code".into()],
                servers,
                settings: IndexMap::new(),
                conflict_counts: IndexMap::new(),
                lifted: Vec::new(),
                skipped: Vec::new(),
            }
        };

        let baseline = plan_digest(&mk(&["a", "b"], "safe"), base, false, "keychain");
        // Same display target ("npx a b"), different argv boundaries.
        let joined_argv = plan_digest(&mk(&["a b"], "safe"), base, false, "keychain");
        assert_ne!(baseline, joined_argv, "argv boundaries must be bound");
        // Same display target, different env VALUE.
        let env_changed = plan_digest(&mk(&["a", "b"], "unsafe"), base, false, "keychain");
        assert_ne!(baseline, env_changed, "env values must be bound");
        // Destination participates too.
        let dest_changed = plan_digest(&mk(&["a", "b"], "safe"), base, false, "env");
        assert_ne!(baseline, dest_changed, "secret destination must be bound");
        // And the digest is stable for identical inputs.
        assert_eq!(
            baseline,
            plan_digest(&mk(&["a", "b"], "safe"), base, false, "keychain")
        );
    }

    /// Stage 1.2: the scripted import ends with ONE concise summary carrying
    /// the five facts a new user needs — manifest path, source CLIs, imported
    /// counts, secrets still needing values (with the exact command), and the
    /// next commands (`apply --write`, then `doctor`).
    #[test]
    fn import_summary_names_path_sources_counts_secrets_and_next() {
        let out = render_import_summary(
            "/tmp/proj/.agentstack/agentstack.toml",
            &["Claude Code".to_string(), "Codex CLI".to_string()],
            8,
            2,
            &["GITHUB_TOKEN".to_string()],
        );
        assert!(out.contains("Manifest:  /tmp/proj/.agentstack/agentstack.toml"));
        assert!(out.contains("From:      Claude Code · Codex CLI"));
        assert!(out.contains("8 MCP server(s) · settings from 2 CLI(s)"));
        assert!(out.contains("1 still need a value"));
        assert!(out.contains("agentstack secret set GITHUB_TOKEN"));
        assert!(out.contains("agentstack restore --last --write"));
        assert!(out.contains("agentstack apply --write"));
        assert!(out.contains("agentstack doctor"));

        // Nothing pending → no secrets section at all, not an empty one.
        let clean = render_import_summary("/m", &["Claude Code".to_string()], 1, 0, &[]);
        assert!(!clean.contains("Secrets:"));
        assert!(!clean.contains("settings from"));
        assert!(clean.contains("agentstack doctor"));
    }

    /// S1 witness (init-secrets design §7): a failing credential store must
    /// not abort init or silently drop values — failed refs are reported by
    /// name while the values that CAN store still do.
    #[test]
    fn store_lifted_reports_failures_by_name_and_keeps_storing() {
        let lifted = vec![
            Lifted {
                reference: "BROKEN".into(),
                value: "v1".into(),
                origin: "server 'a'".into(),
            },
            Lifted {
                reference: "OK".into(),
                value: "v2".into(),
                origin: "server 'b'".into(),
            },
        ];
        let mut stored = Vec::new();
        let unstored = store_lifted(&lifted, |name, _value| {
            if name == "BROKEN" {
                anyhow::bail!("no secret-service bus");
            }
            stored.push(name.to_string());
            Ok(())
        });
        assert_eq!(unstored, vec!["BROKEN".to_string()]);
        assert_eq!(stored, vec!["OK".to_string()]);
    }

    /// P2: the interactive menu preselects `.env` — bare Enter and `1` both
    /// pick it, and only an explicit `2`/`3` selects an alternative.
    #[test]
    fn parse_secret_choice_defaults_to_env() {
        assert_eq!(parse_secret_choice(""), SecretStore::Env);
        assert_eq!(parse_secret_choice("\n"), SecretStore::Env);
        assert_eq!(parse_secret_choice("1"), SecretStore::Env);
        assert_eq!(parse_secret_choice("2"), SecretStore::Keychain);
        assert_eq!(parse_secret_choice("3"), SecretStore::Skip);
        // Anything unrecognized falls back to the safe familiar default.
        assert_eq!(parse_secret_choice("garbage"), SecretStore::Env);
    }

    /// P28: the arrow-key selector maps its 0-based index to the same three
    /// stores, `.env` first (preselected). Item order must stay in lock-step
    /// with the numbered fallback above.
    #[test]
    fn secret_store_at_index_matches_menu_order() {
        assert_eq!(secret_store_at(0), SecretStore::Env);
        assert_eq!(secret_store_at(1), SecretStore::Keychain);
        assert_eq!(secret_store_at(2), SecretStore::Skip);
    }

    /// FIX D witness: a flagless `init` with no terminal must REFUSE before
    /// writing anything — otherwise it would silently import configs and lift
    /// live token values into files, contradicting its own help ("scripts get
    /// the promptless primitive via flags"). The TTY probe is injected
    /// (`interactive: false`) so the refusal path runs without a real terminal.
    #[test]
    fn non_tty_flagless_init_refuses_and_writes_nothing() {
        let dir = assert_fs::TempDir::new().unwrap();
        let args = InitArgs {
            global: false,
            force: false,
            dry_run: false,
            plan: false,
            secrets: None,
            no_keychain: false,
            yes: false,
            consented_plan: None,
        };
        let err = run_gated(&args, Some(dir.path()), false)
            .expect_err("a flagless non-TTY init must refuse");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("--yes") && msg.contains("without a terminal"),
            "the refusal names the scripted escape and the reason: {msg}"
        );
        // Nothing was written under either manifest layout.
        assert!(!dir.path().join(".agentstack/agentstack.toml").exists());
        assert!(!dir.path().join("agentstack.toml").exists());
    }

    /// Lane A witness (UI control-plane §10): `init --plan` is detection-only
    /// — it must write NOTHING, under either manifest layout, even when run
    /// non-interactively with no other flags (the read primitive an external
    /// wizard calls headlessly). Detection reads the real machine's CLI
    /// configs, which is fine: the assertion is about writes, not findings.
    #[test]
    fn plan_emits_json_and_writes_nothing() {
        let dir = assert_fs::TempDir::new().unwrap();
        let args = InitArgs {
            global: false,
            force: false,
            dry_run: false,
            plan: true,
            secrets: None,
            no_keychain: false,
            yes: false,
            consented_plan: None,
        };
        run_gated(&args, Some(dir.path()), false).expect("plan is read-only and never refuses");
        assert!(!dir.path().join(".agentstack").exists());
        assert!(!dir.path().join("agentstack.toml").exists());
        assert!(!dir.path().join(".env").exists());
    }

    /// T4 (third-pass DX audit): scripted `init` against an initialized
    /// project must recommend the real next steps (`apply --write`), not the
    /// generic escapes — `--yes` would hit the --force wall and `--dry-run`
    /// previews a from-scratch replacement. Both the flagless non-TTY path
    /// and the explicit `--yes` path land on the same adapted refusal.
    #[test]
    fn scripted_init_with_existing_manifest_names_apply_not_yes() {
        let dir = assert_fs::TempDir::new().unwrap();
        std::fs::write(dir.path().join("agentstack.toml"), "version = 1\n").unwrap();

        let flagless = InitArgs {
            global: false,
            force: false,
            dry_run: false,
            plan: false,
            secrets: None,
            no_keychain: false,
            yes: false,
            consented_plan: None,
        };
        let with_yes = InitArgs {
            yes: true,
            ..flagless.clone()
        };
        for args in [flagless, with_yes] {
            let err = run_gated(&args, Some(dir.path()), false)
                .expect_err("init over an existing manifest must refuse");
            let msg = format!("{err:#}");
            assert!(msg.contains("already exists"), "{msg}");
            assert!(
                msg.contains("agentstack apply --write"),
                "names the real scripted next step: {msg}"
            );
            assert!(
                !msg.contains("--yes"),
                "no escape that would just error again: {msg}"
            );
        }
        // The manifest survived untouched.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("agentstack.toml")).unwrap(),
            "version = 1\n"
        );
    }
}
