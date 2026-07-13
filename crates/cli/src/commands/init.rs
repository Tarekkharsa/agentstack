//! `agentstack init` — never a blank page. Detect installed CLIs, import their
//! existing MCP servers into one manifest, and lift inline secrets into
//! `${REF}`s (stored in the keychain).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use owo_colors::OwoColorize;

use crate::adapter::{extract_servers, extract_settings, Registry};
use crate::cli::InitArgs;
use crate::discover::{lift_secrets, merge_servers};
use crate::manifest::load::MANIFEST_FILE;
use crate::manifest::model::{Manifest, Meta, Server, Targets};
use crate::secret::keychain;

/// Non-interactive init for the dashboard: detect CLIs, import their servers,
/// lift secrets (best-effort to keychain), write the manifest. Returns a
/// summary. Errors if a manifest already exists or no CLIs are detected.
pub fn dashboard_init(manifest_dir: Option<&Path>) -> Result<String> {
    let base = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    // Create new manifests in `.agentstack/`; keep updating a legacy root one.
    let dir = crate::manifest::new_manifest_dir(&base);
    let manifest_path = dir.join(MANIFEST_FILE);
    if manifest_path.exists() {
        anyhow::bail!("already initialized ({} exists)", manifest_path.display());
    }

    let registry = Registry::load()?;
    let mut detected: Vec<String> = Vec::new();
    let mut servers: IndexMap<String, Server> = IndexMap::new();
    let mut settings: IndexMap<String, serde_json::Value> = IndexMap::new();
    for desc in registry.iter() {
        if !desc.detected() {
            continue;
        }
        detected.push(desc.id.clone());
        if let Some(value) = desc.read_config_value()? {
            merge_servers(&mut servers, extract_servers(desc, &value));
        }
        if let Some(value) = desc.read_settings_value(&dir)? {
            let imported = extract_settings(desc, &value);
            if !imported.is_empty() {
                settings.insert(desc.id.clone(), serde_json::Value::Object(imported));
            }
        }
    }
    if detected.is_empty() {
        anyhow::bail!("no supported CLIs detected on this machine");
    }

    let lifted = lift_secrets(&mut servers);
    for l in &lifted {
        let _ = keychain::set(&l.reference, &l.value); // best effort
    }

    let settings_count = settings.len();
    let manifest = Manifest {
        version: 1,
        meta: Meta { name: None },
        servers,
        skills: IndexMap::new(),
        profiles: IndexMap::new(),
        instructions: IndexMap::new(),
        settings,
        hooks: IndexMap::new(),
        plugins: IndexMap::new(),
        targets: Targets {
            default: detected.clone(),
        },
        policy: Default::default(),
        guard: Default::default(),
        experimental: Default::default(),
    };
    let toml_text = toml::to_string_pretty(&manifest).context("serializing manifest")?;
    crate::util::atomic::write(&manifest_path, &toml_text)?;

    Ok(format!(
        "Imported {} server(s) from {} CLI(s) ({} with settings); lifted {} secret(s).",
        manifest.servers.len(),
        detected.len(),
        settings_count,
        lifted.len()
    ))
}

pub fn run(args: &InitArgs, manifest_dir: Option<&Path>) -> Result<()> {
    if args.global {
        return run_global(args);
    }
    run_impl(args, manifest_dir, true)
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
/// zero-files bridge deliberately never discovers this layer as a project
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
    if args.dry_run {
        println!("\n{} (preview — nothing written)\n", MANIFEST_FILE.bold());
        println!("{GLOBAL_MANIFEST_TEMPLATE}");
        println!("Would write {}", manifest_path.display());
        println!("Would create {}/", instr_dir.display());
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
    println!("{}  Wrote {}", "✅".dimmed(), manifest_path.display());
    println!("{}  Created {}/", "📁".dimmed(), instr_dir.display());

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
            "  {} skipped — `agentstack setup` will offer them again.",
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

fn run_impl(args: &InitArgs, manifest_dir: Option<&Path>, show_next: bool) -> Result<()> {
    let base = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    // Create new manifests in `.agentstack/`; keep updating a legacy root one.
    let dir = crate::manifest::new_manifest_dir(&base);
    let manifest_path = dir.join(MANIFEST_FILE);
    if manifest_path.exists() && !args.force && !args.dry_run {
        anyhow::bail!(
            "{} already exists — use --force to overwrite or --dry-run to preview",
            manifest_path.display()
        );
    }

    let registry = Registry::load()?;

    // Discover + import.
    let mut detected: Vec<String> = Vec::new();
    let mut servers: IndexMap<String, Server> = IndexMap::new();
    let mut settings: IndexMap<String, serde_json::Value> = IndexMap::new();
    let mut display_names: Vec<String> = Vec::new();

    for desc in registry.iter() {
        if !desc.detected() {
            continue;
        }
        detected.push(desc.id.clone());
        display_names.push(desc.display.clone());

        if let Some(value) = desc.read_config_value()? {
            let imported = extract_servers(desc, &value);
            let conflicts = merge_servers(&mut servers, imported);
            for c in conflicts {
                println!(
                    "{} server '{c}' differs between CLIs — kept the first definition",
                    "⚠".yellow()
                );
            }
        }
        // Import this CLI's existing native settings (catalog keys only).
        if let Some(value) = desc.read_settings_value(&dir)? {
            let imported = extract_settings(desc, &value);
            if !imported.is_empty() {
                settings.insert(desc.id.clone(), serde_json::Value::Object(imported));
            }
        }
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
# [servers.github]
# type = \"stdio\"
# command = \"npx\"
# args = [\"-y\", \"@modelcontextprotocol/server-github\"]
# env = { GITHUB_TOKEN = \"${GH_PAT}\" }
#
# Next steps:
#   agentstack search <query>          find servers/skills in the catalog
#   agentstack add from <id> --write   add one to this manifest
#   agentstack apply                   preview what renders into each CLI
#   agentstack connect --all --write   or skip rendered files entirely:
#   agentstack trust .                 serve this repo through the gateway
";
        if args.dry_run {
            println!("No supported CLIs detected — would write a starter manifest:\n\n{STARTER}");
            return Ok(());
        }
        crate::util::atomic::write(&manifest_path, STARTER)
            .with_context(|| format!("writing {}", manifest_path.display()))?;
        println!(
            "No supported CLIs detected to import — wrote a starter manifest instead.\n{}  Wrote {}\n\nAdd a server with `agentstack search <query>` + `agentstack add from <id> --write`,\nor edit the manifest directly (it has a commented example).",
            "✅".dimmed(),
            manifest_path.display()
        );
        return Ok(());
    }

    println!(
        "{}  Detected {} CLI(s): {}",
        "🔍".dimmed(),
        detected.len(),
        display_names.join(" · ")
    );
    println!(
        "{}  Imported {} MCP server(s) from existing configs",
        "📥".dimmed(),
        servers.len()
    );
    if !settings.is_empty() {
        println!(
            "{}  Imported settings from {} CLI(s)",
            "⚙".dimmed(),
            settings.len()
        );
    }

    // Lift inline secrets. This is the moment that matters: plaintext tokens
    // were sitting in live CLI configs — show exactly where each one was.
    let lifted = lift_secrets(&mut servers);
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
        plugins: IndexMap::new(),
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
            println!("Would store {} secret(s) in the keychain.", lifted.len());
        }
        return Ok(());
    }

    // Store lifted secrets (unless opted out).
    if !args.no_keychain {
        for l in &lifted {
            keychain::set(&l.reference, &l.value)
                .with_context(|| format!("storing '{}' in keychain", l.reference))?;
        }
    }

    crate::util::atomic::write(&manifest_path, &toml_text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;

    println!("{}  Wrote {}", "✅".dimmed(), manifest_path.display());
    if !lifted.is_empty() && args.no_keychain {
        println!(
            "{} secret(s) referenced but not stored (--no-keychain). Run `agentstack secret set <NAME>`.",
            lifted.len()
        );
    }
    if show_next {
        println!(
            "\nNext: review the manifest, then `agentstack bootstrap` for the guided preflight."
        );
    }
    Ok(())
}
