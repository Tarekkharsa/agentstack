//! `agentstack gateway connect` / `disconnect` — register the zero-files bridge.
//!
//! One tiny global entry per harness — `agentstack mcp --auto-project` — and
//! every trusted repo brings its own servers, skills-over-MCP, firewall, and
//! audit log at runtime, with no per-project rendered files. This replaces the
//! manual "paste this JSON into your harness config" step the docs used to
//! prescribe. Dry-run by default, like every other mutating command.

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use serde_json::Value;

use crate::adapter::descriptor::Format;
use crate::adapter::{render_server, AdapterDescriptor, Registry};
use crate::cli::{ConnectArgs, DisconnectArgs};
use crate::manifest::{Server, ServerType};
use crate::render::{merge_json, merge_toml};
use crate::secret::MapResolver;

/// The reserved server name the bridge is registered under in harness configs.
pub const BRIDGE_ENTRY: &str = "agentstack";

pub fn run_connect(args: &ConnectArgs) -> Result<()> {
    let registry = Registry::load()?;
    let targets = select_targets(
        &registry,
        &args.harnesses,
        args.all,
        /*for_removal=*/ false,
    )?;
    let command = bridge_command(args.command.as_deref());
    let bridge = bridge_server(&command, args.transparent, None);

    let mut backups: Vec<crate::history::FileChange> = Vec::new();
    let mut touched: Vec<String> = Vec::new();
    let mut changed = 0;

    for desc in &targets {
        let Some((path, format)) = desc.config_for(crate::scope::Scope::Global, ".".as_ref())
        else {
            continue; // select_targets already filtered these out
        };
        let Some(mcp) = desc.mcp.as_ref() else {
            continue;
        };

        println!("\n{} ({})", desc.display.bold(), path.display());

        let rendered = render_server(desc, &bridge, &MapResolver::default());
        if !rendered.representable {
            println!(
                "  {} can't host a stdio MCP server — the bridge doesn't apply here",
                "↳".cyan()
            );
            continue;
        }

        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let entries = vec![(BRIDGE_ENTRY.to_string(), rendered.value)];
        let proposed = match format {
            Format::Json => merge_json::merge(&existing, &mcp.location, &entries)?,
            Format::Toml => merge_toml::merge_with_removals(
                &existing,
                &mcp.location,
                &entries,
                &[],
                mcp.headers_as_subtable,
            )?,
        };

        if !crate::util::diff::differs(&existing, &proposed) {
            println!("  {} already connected", "✓".green());
            print_limits(desc);
            continue;
        }

        changed += 1;
        print!(
            "{}",
            indent(&crate::util::diff::render(&existing, &proposed))
        );
        if args.write {
            backups.push(crate::history::capture(
                &path,
                format!("{} · connect", desc.display),
            ));
            touched.push(desc.display.clone());
            crate::util::atomic::write(&path, &proposed)?;
            println!(
                "  {} bridge registered (agentstack mcp --auto-project)",
                "✓".green()
            );
        } else {
            println!("  {} would register the bridge", "→".cyan());
        }
        print_limits(desc);
    }

    finish(args.write, changed, touched, backups)?;
    // P17: the trust-unlock teaching prints in the DRY-RUN too, not only after
    // `--write`. Deciding whether to register the bridge is exactly when the
    // user needs to know trust is the per-repo gate — so it belongs in the
    // preview they read first, not the confirmation they see after committing.
    if changed > 0 {
        println!(
            "\nEach repo now only needs a trusted manifest: `agentstack trust <dir>` unlocks its \
             servers for the bridge. Untrusted repos get control-plane tools only."
        );
    }
    Ok(())
}

pub fn run_disconnect(args: &DisconnectArgs) -> Result<()> {
    let registry = Registry::load()?;
    let targets = select_targets(
        &registry,
        &args.harnesses,
        args.all,
        /*for_removal=*/ true,
    )?;

    let mut backups: Vec<crate::history::FileChange> = Vec::new();
    let mut touched: Vec<String> = Vec::new();
    let mut changed = 0;

    for desc in &targets {
        let Some((path, format)) = desc.config_for(crate::scope::Scope::Global, ".".as_ref())
        else {
            continue;
        };
        let Some(mcp) = desc.mcp.as_ref() else {
            continue;
        };

        println!("\n{} ({})", desc.display.bold(), path.display());
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        if !has_bridge_entry(&existing, &mcp.location, format) {
            println!("  {} not connected — nothing to remove", "✓".green());
            continue;
        }
        let removals = vec![BRIDGE_ENTRY.to_string()];
        let proposed = match format {
            Format::Json => {
                merge_json::merge_with_removals(&existing, &mcp.location, &[], &removals)?
            }
            Format::Toml => merge_toml::merge_with_removals(
                &existing,
                &mcp.location,
                &[],
                &removals,
                mcp.headers_as_subtable,
            )?,
        };
        if !crate::util::diff::differs(&existing, &proposed) {
            println!("  {} up to date", "✓".green());
            continue;
        }
        changed += 1;
        print!(
            "{}",
            indent(&crate::util::diff::render(&existing, &proposed))
        );
        if args.write {
            backups.push(crate::history::capture(
                &path,
                format!("{} · disconnect", desc.display),
            ));
            touched.push(desc.display.clone());
            crate::util::atomic::write(&path, &proposed)?;
            println!("  {} bridge removed", "✓".green());
        } else {
            println!("  {} would remove the bridge", "→".cyan());
        }
    }

    finish(args.write, changed, touched, backups)
}

/// Which adapters to act on. Explicit ids must exist and support MCP; naming
/// none is the documented default — it acts on every eligible harness, exactly
/// like `--all` (for removal: any with the bridge present, detected or not —
/// leftover config should be removable).
fn select_targets<'r>(
    registry: &'r Registry,
    ids: &[String],
    all: bool,
    for_removal: bool,
) -> Result<Vec<&'r AdapterDescriptor>> {
    // No harnesses named falls through to the `--all` path (documented
    // default). `--all` stays an explicit way to ask for the same thing.
    let all = all || ids.is_empty();
    if !ids.is_empty() {
        let mut out = Vec::new();
        for id in ids {
            let desc = registry.get(id).with_context(|| {
                format!("unknown adapter '{id}' (see `agentstack adapters list`)")
            })?;
            if desc.mcp.is_none() || desc.config.is_none() {
                anyhow::bail!("{id} has no MCP config — the bridge doesn't apply to it");
            }
            out.push(desc);
        }
        return Ok(out);
    }
    if all {
        let mut out: Vec<&AdapterDescriptor> = Vec::new();
        for d in registry.iter() {
            if d.mcp.is_none() || d.config.is_none() {
                // Not a failure — this harness simply has no MCP config to
                // register a bridge in (e.g. Pi manages only skills/settings).
                // Say so for harnesses that are actually present.
                if !for_removal && d.detected() {
                    println!(
                        "{} {}: no MCP config support — the bridge doesn't apply, skipped",
                        "·".dimmed(),
                        d.id
                    );
                }
                continue;
            }
            if !for_removal && !d.detected() {
                continue;
            }
            out.push(d);
        }
        if out.is_empty() {
            anyhow::bail!("no installed harness with MCP support detected");
        }
        return Ok(out);
    }
    // Unreachable: the normalization above forces `all` true whenever no ids
    // were named, and named ids returned in the first block — so we never fall
    // through here.
    unreachable!("select_targets: either explicit ids or the --all path is taken")
}

/// The bridge, expressed as a manifest server so the existing per-adapter
/// renderer shapes it (transport tags, field names, command arrays).
///
/// With `grant` (a `run --locked` launch-scoped entry), the bridge consumes
/// the frozen run-grant artifact instead of discovering a project — the
/// artifact IS the project pointer, so `--auto-project` is omitted and the
/// two modes can never disagree about which project is served.
pub(crate) fn bridge_server(
    command: &str,
    transparent: bool,
    grant: Option<&std::path::Path>,
) -> Server {
    let mut args = vec!["mcp".to_string()];
    match grant {
        Some(path) => {
            args.push("--grant".to_string());
            args.push(path.display().to_string());
        }
        None => args.push("--auto-project".to_string()),
    }
    if transparent {
        args.push("--transparent".to_string());
    }
    Server {
        server_type: ServerType::Stdio,
        url: None,
        command: Some(command.to_string()),
        args,
        cwd: None,
        integrity_roots: Vec::new(),
        targets: crate::manifest::model::all_targets(),
        owner: None,
        headers: Default::default(),
        env: Default::default(),
        extra: Default::default(),
    }
}

/// The binary to register: an explicit override, else the stable PATH install
/// when it resolves to this executable (the `agentstack self link` symlink —
/// so configs survive rebuilds instead of pinning e.g. target/release), else
/// this executable's real path. An absolute path matters either way — GUI
/// harnesses spawn MCP servers without a login shell's $PATH.
pub(crate) fn bridge_command(explicit: Option<&str>) -> String {
    if let Some(c) = explicit {
        return c.to_string();
    }
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    if let Some(exe) = &exe {
        for cand in
            crate::commands::self_cmd::find_all_on_path(&crate::commands::self_cmd::bin_name())
        {
            if cand.canonicalize().ok().as_ref() == Some(exe) {
                return cand.display().to_string();
            }
        }
    }
    exe.map(|p| p.display().to_string())
        .unwrap_or_else(|| "agentstack".to_string())
}

/// Whether a config already carries a bridge entry under (dotted) `location`.
pub fn has_bridge_entry(existing: &str, location: &str, format: Format) -> bool {
    if existing.trim().is_empty() {
        return false;
    }
    let value: Value = match format {
        Format::Json => match serde_json::from_str(existing) {
            Ok(v) => v,
            Err(_) => return false,
        },
        Format::Toml => match existing.parse::<toml::Value>() {
            Ok(t) => match serde_json::to_value(t) {
                Ok(v) => v,
                Err(_) => return false,
            },
            Err(_) => return false,
        },
    };
    let mut cur = &value;
    for key in location.split('.') {
        match cur.get(key) {
            Some(v) => cur = v,
            None => return false,
        }
    }
    cur.get(BRIDGE_ENTRY).is_some()
}

/// Honesty about harness limits: MCP servers, secrets, firewall, audit, and
/// skills-over-MCP go zero-files; anything the harness only reads from disk
/// does not.
fn print_limits(desc: &AdapterDescriptor) {
    let mut native = Vec::new();
    if desc.skills.is_some() {
        native.push("native skill folders");
    }
    if desc.instructions.is_some() {
        native.push("instruction files (CLAUDE.md/AGENTS.md)");
    }
    if !native.is_empty() {
        println!(
            "  {} zero-file limit: {} still need render mode (`apply`/`use`); \
             skills also load over MCP via agentstack_list_loadable/agentstack_load",
            "·".dimmed(),
            native.join(" and ")
        );
    }
}

fn finish(
    write: bool,
    changed: usize,
    touched: Vec<String>,
    backups: Vec<crate::history::FileChange>,
) -> Result<()> {
    if write && !backups.is_empty() {
        // One undoable history entry for everything this run wrote.
        let _ = crate::history::record("global", touched, backups);
    }
    println!();
    if write {
        println!("Updated {changed} harness config(s).");
    } else if changed > 0 {
        println!("{changed} harness config(s) would change. Re-run with --write to apply.");
    } else {
        println!("Nothing to change.");
    }
    Ok(())
}

fn indent(s: &str) -> String {
    s.lines().map(|l| format!("  {l}\n")).collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_renders_into_claude_json_and_codex_toml() {
        let reg = Registry::load().unwrap();
        let bridge = bridge_server("/usr/local/bin/agentstack", false, None);

        // Claude Code: JSON, transport-tagged.
        let desc = reg.get("claude-code").unwrap();
        let r = render_server(desc, &bridge, &MapResolver::default());
        assert!(r.representable);
        assert_eq!(r.value["type"], "stdio");
        assert_eq!(r.value["command"], "/usr/local/bin/agentstack");
        assert_eq!(
            r.value["args"],
            serde_json::json!(["mcp", "--auto-project"])
        );
        let mcp = desc.mcp.as_ref().unwrap();
        let out =
            merge_json::merge("", &mcp.location, &[(BRIDGE_ENTRY.to_string(), r.value)]).unwrap();
        assert!(has_bridge_entry(&out, &mcp.location, Format::Json));

        // Codex: TOML, no transport tag.
        let desc = reg.get("codex").unwrap();
        let r = render_server(desc, &bridge, &MapResolver::default());
        assert!(r.representable);
        let mcp = desc.mcp.as_ref().unwrap();
        let out = merge_toml::merge_with_removals(
            "",
            &mcp.location,
            &[(BRIDGE_ENTRY.to_string(), r.value)],
            &[],
            mcp.headers_as_subtable,
        )
        .unwrap();
        assert!(out.contains("[mcp_servers.agentstack]"));
        assert!(out.contains("--auto-project"));
        assert!(has_bridge_entry(&out, &mcp.location, Format::Toml));
    }

    /// Naming no harnesses is the documented default (`ConnectArgs.harnesses`:
    /// "With none given, use --all") — it must select the same set as `--all`,
    /// not bail. Uses the removal path so the set is detection-independent and
    /// the assertion is deterministic across machines.
    #[test]
    fn bare_connect_selects_same_targets_as_all() {
        let reg = Registry::load().unwrap();
        let ids_of = |v: &[&AdapterDescriptor]| v.iter().map(|d| d.id.clone()).collect::<Vec<_>>();

        let bare = select_targets(&reg, &[], /*all=*/ false, /*for_removal=*/ true).unwrap();
        let all = select_targets(&reg, &[], /*all=*/ true, /*for_removal=*/ true).unwrap();

        assert!(!bare.is_empty(), "some harness supports the bridge");
        assert_eq!(ids_of(&bare), ids_of(&all));
    }

    /// After `self link`, connect must register the stable symlink path — not
    /// this process's build location — so harness configs survive rebuilds.
    #[cfg(unix)]
    #[test]
    fn bridge_command_prefers_stable_path_install_over_current_exe() {
        use assert_fs::prelude::*;
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let exe = std::env::current_exe().unwrap().canonicalize().unwrap();

        let tmp = assert_fs::TempDir::new().unwrap();
        let bin = tmp.child("bin");
        bin.create_dir_all().unwrap();
        let link = bin.path().join(crate::commands::self_cmd::bin_name());
        std::os::unix::fs::symlink(&exe, &link).unwrap();

        let old = std::env::var_os("PATH");
        std::env::set_var("PATH", bin.path());
        let picked = bridge_command(None);
        // A PATH entry that is some other binary must NOT be picked.
        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, "other").unwrap();
        let unrelated = bridge_command(None);
        match old {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }

        assert_eq!(picked, link.display().to_string());
        assert_eq!(unrelated, exe.display().to_string());
        // An explicit override always wins.
        assert_eq!(bridge_command(Some("/x/agentstack")), "/x/agentstack");
    }

    #[test]
    fn bridge_entry_detection_respects_location_and_absence() {
        assert!(!has_bridge_entry("", "mcpServers", Format::Json));
        assert!(!has_bridge_entry(
            "{\"mcpServers\": {\"other\": {}}}",
            "mcpServers",
            Format::Json
        ));
        assert!(has_bridge_entry(
            "{\"mcpServers\": {\"agentstack\": {\"command\": \"agentstack\"}}}",
            "mcpServers",
            Format::Json
        ));
        // Removal round-trip leaves other servers alone.
        let existing =
            "{\"mcpServers\": {\"agentstack\": {\"command\": \"x\"}, \"keep\": {\"url\": \"u\"}}}";
        let out = merge_json::merge_with_removals(
            existing,
            "mcpServers",
            &[],
            &[BRIDGE_ENTRY.to_string()],
        )
        .unwrap();
        assert!(!has_bridge_entry(&out, "mcpServers", Format::Json));
        assert!(out.contains("\"keep\""));
    }
}
