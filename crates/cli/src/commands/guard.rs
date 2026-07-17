//! `agentstack guard` — install and serve the destructive-command hook.
//!
//! `check` is the hook entrypoint each agent CLI invokes before a tool call
//! (stdin: the CLI's native payload; response: that CLI's native block
//! dialect — see [`crate::guard::Protocol`]). `install` wires the hook into
//! every *detected* hook-capable CLI's global config; `uninstall` removes
//! exactly what install wrote; `status` reports both sides. `test` judges a
//! command from argv, for humans and for the OpenCode/Pi bridge files.
//!
//! Failure posture, decided per failure class:
//! - stdin that isn't a shape we know → ALLOW (fail-open): the guard must
//!   never wedge a harness on a payload some new CLI version grew.
//! - machine manifest exists but can't load → DENY (fail-closed): an
//!   installed hook proves the guard was configured; "config rotted so
//!   everything is allowed" is the one wrong answer for a security tool.
//! - guard not configured or disabled → ALLOW (a leftover hook after an
//!   opt-out must not keep enforcing).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use serde_json::{json, Value};

use crate::cli::{GuardArgs, GuardCmd};
use crate::guard::{check_event, Decision, GuardContext, GuardEvent, Protocol};
use crate::manifest;
use crate::util::paths;

/// Default `[policy.filesystem] deny` seeded into the machine manifest on
/// first install (explicit and user-editable — never a hidden built-in).
/// Deliberately excludes template names like `.env.example`: patterns match
/// the bare file name too, so keep them tight.
const DEFAULT_DENY: &[&str] = &[
    ".env",
    ".env.local",
    ".env.*.local",
    ".env.production",
    ".env.development",
    "id_rsa",
    "id_ed25519",
    "*.pem",
];

/// Hooks stdin is hostile input: bound it (a tool-call payload is KBs).
const MAX_PAYLOAD: u64 = 4 * 1024 * 1024;

pub fn run(args: &GuardArgs) -> Result<()> {
    match &args.cmd {
        GuardCmd::Check { protocol } => check(protocol.as_deref()),
        GuardCmd::Test { command } => test(&command.join(" ")),
        GuardCmd::Install {} => install(),
        GuardCmd::Uninstall {} => uninstall(),
        GuardCmd::Status {} => status(),
    }
}

// ── check: the hook entrypoint ──────────────────────────────────────────────

fn check(protocol: Option<&str>) -> Result<()> {
    // Installed hooks always pass an explicit protocol. The fallback exists
    // only for hand-written/legacy hooks whose payload cannot be parsed.
    let requested_proto = protocol.and_then(Protocol::parse);
    let fallback_proto = requested_proto.unwrap_or(Protocol::Claude);

    // Check enablement before hostile-input parsing: a disabled/removed guard
    // must remain inert, while a configured guard must fail closed if its
    // input cannot be read within the hard cap.
    let guard_cfg = match manifest::machine_guard_health() {
        None => finish(fallback_proto, &Decision::Allow, None),
        Some(Err(e)) => {
            let deny = Decision::Deny {
                reason: format!("machine config unreadable — failing closed ({e:#})"),
            };
            finish(fallback_proto, &deny, None);
        }
        Some(Ok(cfg)) if !cfg.enabled() => finish(fallback_proto, &Decision::Allow, None),
        Some(Ok(cfg)) => cfg,
    };

    // Active guard checks use the same fail-closed machine-policy input as
    // apply and the gateway. A broken first run denies; a previously validated
    // policy may continue from its LKG. A disabled/removed guard remains inert
    // because enablement was resolved first.
    let machine_policy = match crate::machine_policy::load() {
        Ok(policy) => policy,
        Err(error) => {
            let deny = Decision::Deny {
                reason: format!("machine policy unavailable — failing closed ({error:#})"),
            };
            finish(fallback_proto, &deny, None);
        }
    };

    let raw = match read_payload(std::io::stdin()) {
        Ok(raw) => raw,
        Err(error) => {
            let deny = Decision::Deny {
                reason: format!("hook payload unreadable — failing closed ({error})"),
            };
            finish(fallback_proto, &deny, None);
        }
    };
    let payload: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => std::process::exit(0), // unknown shape → fail-open
    };
    let proto = requested_proto.unwrap_or_else(|| Protocol::detect(&payload));
    let Some((event, cwd)) = proto.parse_event(&payload) else {
        finish(proto, &Decision::Allow, None);
    };
    let cwd = cwd
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"));
    // Anchor the workspace at the project root, not the transient `cwd`. An
    // agent that has `cd`'d into a subdirectory still writes to files higher
    // up the same project (a repo-root `README.md`, say); scoping to `cwd`
    // alone would wrongly report those as "outside the workspace". This also
    // fixes the project-policy load just below — the `[policy]` deny globs
    // live in the root `.agentstack/` manifest, which a subdirectory `cwd`
    // would miss.
    let workspace = anchor_workspace(&cwd);

    // The project layer may ADD deny globs (union — it can never loosen the
    // machine's). A broken/hostile project manifest is simply skipped: the
    // machine layer still applies in full.
    let project_policy = project_policy_at(&workspace);

    let allow_roots = effective_allow_roots(&guard_cfg, &workspace);
    let ctx = GuardContext {
        workspace,
        home: dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
        tmp: tmp_dirs(),
        allow_roots,
        agentstack_home: paths::agentstack_home(),
        ruleset: agentstack_policy::compile(&machine_policy, &project_policy, &[]),
    };
    let decision = check_event(&ctx, &event);
    finish(proto, &decision, Some((&event, &ctx)))
}

/// The effective extra write roots for one workspace: the global
/// `[guard] allow_roots` plus every `[guard.project_roots]` entry whose key
/// path contains the anchored workspace. The scoped grants live in the
/// MACHINE manifest — a project can never widen its own write scope, and the
/// guard already denies shell writes to that manifest's directory. Pure, so
/// it is unit-testable.
fn effective_allow_roots(
    cfg: &agentstack_core::manifest::GuardConfig,
    workspace: &Path,
) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = cfg
        .allow_roots
        .iter()
        .map(|r| paths::expand_tilde(r))
        .collect();
    for (scope, extra) in &cfg.project_roots {
        if workspace.starts_with(paths::expand_tilde(scope)) {
            roots.extend(extra.iter().map(|r| paths::expand_tilde(r)));
        }
    }
    roots
}

/// Anchor the guard's workspace at the project root: the nearest ancestor of
/// `cwd` that holds a `.git` or `.agentstack` entry. Falls back to `cwd`
/// itself when no marker is found, preserving the previous behavior for a
/// loose directory that is its own workspace.
///
/// The filesystem probe is the only I/O here; the outward walk is factored
/// into [`nearest_ancestor`] so it stays unit-testable without touching disk.
fn anchor_workspace(cwd: &Path) -> PathBuf {
    nearest_ancestor(cwd, |dir| {
        dir.join(".git").exists() || dir.join(".agentstack").exists()
    })
}

/// The project policy layer for a guard decision: whatever manifest the
/// workspace carries — the preferred `.agentstack/agentstack.toml` layout or
/// the legacy root `agentstack.toml`. `load_from_dir` expects the MANIFEST
/// dir, not the project root, so the workspace must be resolved through
/// `resolve_manifest_dir` first; passing the workspace directly silently
/// ignored every `[policy.filesystem]` deny declared in the preferred layout.
/// A broken/hostile/absent manifest contributes nothing: the machine layer
/// still applies in full (union semantics — a project can only ADD denies).
fn project_policy_at(workspace: &Path) -> manifest::Policy {
    manifest::load_from_dir(&manifest::resolve_manifest_dir(workspace))
        .map(|l| l.manifest.policy)
        .unwrap_or_default()
}

/// Walk `start` and its ancestors outward, returning the first for which
/// `is_root` holds; if none do, return `start` unchanged. `is_root` is a
/// closure (not a hard-coded filesystem check) so tests can drive it with an
/// in-memory predicate.
fn nearest_ancestor(start: &Path, is_root: impl Fn(&Path) -> bool) -> PathBuf {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if is_root(d) {
            return d.to_path_buf();
        }
        dir = d.parent();
    }
    start.to_path_buf()
}

fn read_payload(reader: impl Read) -> std::io::Result<String> {
    let mut raw = String::new();
    reader.take(MAX_PAYLOAD + 1).read_to_string(&mut raw)?;
    if raw.len() as u64 > MAX_PAYLOAD {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("hook payload exceeds {MAX_PAYLOAD} bytes"),
        ));
    }
    Ok(raw)
}

/// Emit the protocol response, audit a denial, and exit with the dialect's
/// code. Never returns.
fn finish(
    proto: Protocol,
    decision: &Decision,
    audited: Option<(&GuardEvent, &GuardContext)>,
) -> ! {
    if let (Decision::Deny { reason }, Some((event, ctx))) = (decision, audited) {
        let subject = match event {
            GuardEvent::Bash { command } => format!("bash: {command}"),
            GuardEvent::FileRead { path } => format!("read: {path}"),
            GuardEvent::FileWrite { path } => format!("write: {path}"),
            GuardEvent::Other => "other".to_string(),
        };
        crate::calllog::record(&crate::calllog::CallRecord {
            ts: crate::calllog::now_epoch(),
            run: None,
            pid: std::process::id(),
            project: Some(ctx.workspace.display().to_string()),
            server: "host-guard".to_string(),
            tool: subject.chars().take(200).collect(),
            args_digest: crate::calllog::digest_args(&json!(subject)),
            outcome: crate::calllog::CallOutcome::Denied,
            detail: Some(reason.clone()),
            ms: 0,
        });
    }
    let (stdout, stderr, code) = proto.respond(decision);
    if let Some(s) = stdout {
        println!("{s}");
    }
    if let Some(s) = stderr {
        eprintln!("{s}");
    }
    std::process::exit(code);
}

fn tmp_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![std::env::temp_dir(), PathBuf::from("/tmp")];
    if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/private/tmp"));
        dirs.push(PathBuf::from("/private/var/folders"));
        dirs.push(PathBuf::from("/var/folders"));
    }
    dirs
}

// ── test: judge one command from argv ───────────────────────────────────────

fn test(command: &str) -> Result<()> {
    let guard_cfg = match manifest::machine_guard_health() {
        Some(Ok(cfg)) => cfg,
        Some(Err(e)) => anyhow::bail!("machine config unreadable: {e:#}"),
        None => Default::default(),
    };
    let machine_policy = crate::machine_policy::load()?;
    // Anchor exactly like the live hook does, so `guard test` reproduces the
    // hook's decision — including workspace-scoped [guard.project_roots].
    let workspace =
        anchor_workspace(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));
    let project_policy = project_policy_at(&workspace);
    let allow_roots = effective_allow_roots(&guard_cfg, &workspace);
    let ctx = GuardContext {
        workspace,
        home: dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
        tmp: tmp_dirs(),
        allow_roots,
        agentstack_home: paths::agentstack_home(),
        ruleset: agentstack_policy::compile(&machine_policy, &project_policy, &[]),
    };
    match check_event(
        &ctx,
        &GuardEvent::Bash {
            command: command.to_string(),
        },
    ) {
        Decision::Allow => {
            println!("{} {command}", "ALLOW".green().bold());
            Ok(())
        }
        Decision::Deny { reason } => {
            println!("{} {command}\n  {reason}", "DENY ".red().bold());
            std::process::exit(1);
        }
    }
}

// ── install / uninstall / status ────────────────────────────────────────────

/// The marker every entry/file we write carries, and the test `uninstall`
/// uses to find (only) our own entries in shared config files.
const MARKER: &str = "agentstack-guard";

/// How the hook is written into one CLI. `detect` is a directory whose
/// existence means the CLI is present — install touches nothing for absent
/// CLIs.
struct Target {
    id: &'static str,
    detect: &'static str,
    kind: Kind,
}

enum Kind {
    /// Merge our entries into a shared JSON hooks file: (path, json pointer
    /// of the hooks object, per-event entry list).
    SharedJson {
        path: &'static str,
        events: &'static [&'static str],
        entry: fn(&str) -> Value,
        /// Extra top-level fields the file wants (e.g. `"version": 1`).
        top_level: &'static [(&'static str, i64)],
    },
    /// A file wholly owned by the guard (safe to create/delete outright).
    OwnedFile {
        path: &'static str,
        contents: fn(&str) -> String,
    },
}

fn targets() -> Vec<Target> {
    vec![
        // VS Code agent mode reads the same user-scope Claude-format hooks,
        // so this one entry covers both (per VS Code's own hooks docs).
        Target {
            id: "claude-code (+ vscode agent mode)",
            detect: "~/.claude",
            kind: Kind::SharedJson {
                path: "~/.claude/settings.json",
                events: &["PreToolUse"],
                entry: claude_entry,
                top_level: &[],
            },
        },
        Target {
            id: "codex",
            detect: "~/.codex",
            kind: Kind::SharedJson {
                path: "~/.codex/hooks.json",
                events: &["PreToolUse"],
                entry: codex_entry,
                top_level: &[],
            },
        },
        Target {
            id: "gemini",
            detect: "~/.gemini",
            kind: Kind::SharedJson {
                path: "~/.gemini/settings.json",
                events: &["BeforeTool"],
                entry: gemini_entry,
                top_level: &[],
            },
        },
        Target {
            id: "antigravity",
            detect: "~/.gemini/antigravity-cli",
            kind: Kind::SharedJson {
                path: "~/.gemini/config/hooks.json",
                events: &["PreToolUse"],
                entry: antigravity_entry,
                top_level: &[],
            },
        },
        Target {
            id: "cursor",
            detect: "~/.cursor",
            kind: Kind::SharedJson {
                path: "~/.cursor/hooks.json",
                events: &["beforeShellExecution"],
                entry: cursor_entry,
                top_level: &[("version", 1)],
            },
        },
        Target {
            id: "windsurf",
            detect: "~/.codeium/windsurf",
            kind: Kind::SharedJson {
                path: "~/.codeium/windsurf/hooks.json",
                events: &["pre_run_command", "pre_write_code", "pre_read_code"],
                entry: windsurf_entry,
                top_level: &[],
            },
        },
        Target {
            id: "copilot-cli",
            detect: "~/.copilot",
            kind: Kind::OwnedFile {
                path: "~/.copilot/hooks/agentstack-guard.json",
                contents: copilot_file,
            },
        },
        Target {
            id: "opencode",
            // OpenCode uses XDG ~/.config even on macOS.
            detect: "~/.config/opencode",
            kind: Kind::OwnedFile {
                path: "~/.config/opencode/plugins/agentstack-guard.js",
                contents: opencode_plugin,
            },
        },
        Target {
            id: "pi",
            detect: "~/.pi/agent",
            kind: Kind::OwnedFile {
                path: "~/.pi/agent/extensions/agentstack-guard.ts",
                contents: pi_extension,
            },
        },
    ]
}

/// CLIs with no per-call hook surface at all — reported, never touched.
const NO_HOOK_SURFACE: &[(&str, &str)] = &[
    (
        "claude-desktop",
        "no PreToolUse-style hook exists on this surface",
    ),
    ("junie", "only a static action allowlist; no per-call hook"),
    (
        "kiro",
        "hooks nest inside per-agent config files; not wired yet",
    ),
];

fn exe() -> String {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "agentstack".to_string())
}

/// The machine-layer hook entries `apply` must render alongside a manifest's
/// own (global scope only): the guard hook, when `[guard]` is enabled. This
/// is what keeps a global-scope `apply` — which owns the whole hooks key —
/// from stripping the guard the user installed.
pub fn machine_hooks_for_apply() -> Vec<(String, crate::manifest::Hook)> {
    let enabled = matches!(
        manifest::machine_guard_health(),
        Some(Ok(cfg)) if cfg.enabled()
    );
    if !enabled {
        return Vec::new();
    }
    vec![(
        MARKER.to_string(),
        crate::manifest::Hook {
            event: "PreToolUse".to_string(),
            matcher: None,
            command: format!("{} guard check --protocol claude", exe()),
            args: vec![],
            timeout: Some(10),
            // Only the claude-shape settings.json target — every other CLI's
            // guard hook lives in a file apply never rewrites.
            targets: vec!["claude-code".to_string()],
        },
    )]
}

fn wrapper_path(name: &str) -> PathBuf {
    paths::agentstack_home().join("guard").join(name)
}

// Per-CLI hook entries. Each carries MARKER via the command path/args so
// uninstall can find its own work.

fn claude_entry(exe: &str) -> Value {
    json!({ "hooks": [{ "type": "command",
        "command": format!("{exe} guard check --protocol claude"), "timeout": 10 }] })
}

fn codex_entry(exe: &str) -> Value {
    json!({ "hooks": [{ "type": "command",
        "command": format!("{exe} guard check --protocol codex"), "timeout": 10 }] })
}

fn gemini_entry(exe: &str) -> Value {
    json!({ "hooks": [{ "name": MARKER, "type": "command",
        "command": format!("{exe} guard check --protocol gemini"), "timeout": 5000 }] })
}

fn antigravity_entry(exe: &str) -> Value {
    json!({ "hooks": [{ "type": "command",
        "command": format!("{exe} guard check --protocol antigravity") }] })
}

// Wrapper file names carry MARKER so `value_mentions_guard` recognizes an
// entry that references a wrapper rather than the binary itself.

fn cursor_entry(_exe: &str) -> Value {
    json!({ "command": wrapper_path("agentstack-guard-cursor.sh").display().to_string() })
}

fn windsurf_entry(_exe: &str) -> Value {
    json!({ "command": wrapper_path("agentstack-guard-windsurf.sh").display().to_string() })
}

fn copilot_file(_exe: &str) -> String {
    let wrapper = wrapper_path("agentstack-guard-copilot.sh")
        .display()
        .to_string();
    serde_json::to_string_pretty(&json!({
        "version": 1,
        "hooks": { "preToolUse": [{
            "type": "command", "bash": wrapper, "powershell": wrapper, "timeoutSec": 10
        }] }
    }))
    .expect("static json")
        + "\n"
}

fn opencode_plugin(exe: &str) -> String {
    format!(
        r#"// {MARKER}: generated by `agentstack guard install` — do not edit.
// Bridges OpenCode's tool.execute.before to `agentstack guard check`.
import {{ spawnSync }} from "node:child_process";

export const AgentstackGuard = async () => ({{
  "tool.execute.before": async (input, output) => {{
    const payload = JSON.stringify({{
      tool_name: input?.tool ?? "",
      tool_input: output?.args ?? {{}},
      cwd: process.cwd(),
    }});
    let decision = null;
    try {{
      const res = spawnSync({exe:?}, ["guard", "check", "--protocol", "claude"],
        {{ input: payload, encoding: "utf8", timeout: 10000 }});
      const out = JSON.parse(res.stdout || "{{}}");
      decision = out?.hookSpecificOutput ?? null;
    }} catch {{
      return; // guard unavailable → fail open, never wedge the harness
    }}
    if (decision && decision.permissionDecision === "deny") {{
      throw new Error(decision.permissionDecisionReason || "blocked by agentstack guard");
    }}
  }},
}});
"#
    )
}

fn pi_extension(exe: &str) -> String {
    format!(
        r#"// {MARKER}: generated by `agentstack guard install` — do not edit.
// Bridges Pi's tool_call event to `agentstack guard check`.
import {{ spawnSync }} from "node:child_process";

export default function (pi: any) {{
  pi.on("tool_call", async (event: any) => {{
    const payload = JSON.stringify({{
      tool_name: event?.toolName ?? "",
      tool_input: event?.input ?? {{}},
      cwd: process.cwd(),
    }});
    try {{
      const res = spawnSync({exe:?}, ["guard", "check", "--protocol", "claude"],
        {{ input: payload, encoding: "utf8", timeout: 10000 }});
      const out = JSON.parse(res.stdout || "{{}}");
      const d = out?.hookSpecificOutput;
      if (d && d.permissionDecision === "deny") {{
        return {{ block: true, reason: d.permissionDecisionReason || "blocked by agentstack guard" }};
      }}
    }} catch {{
      // guard unavailable → fail open
    }}
    return undefined;
  }});
}}
"#
    )
}

fn install() -> Result<()> {
    seed_machine_config()?;
    write_wrappers()?;
    let exe = exe();
    let mut wrote = 0usize;
    for t in targets() {
        if !paths::expand_tilde(t.detect).exists() {
            println!("  {} {} — not detected, skipped", "·".dimmed(), t.id);
            continue;
        }
        match apply_target(&t, &exe, true) {
            Ok(path) => {
                wrote += 1;
                println!("  {} {} → {}", "✓".green(), t.id, path.display());
            }
            Err(e) => println!("  {} {} — {e:#}", "✗".red(), t.id),
        }
    }
    for (id, why) in NO_HOOK_SURFACE {
        println!("  {} {id} — {why}", "○".dimmed());
    }
    println!(
        "\n{} guard wired into {wrote} CLI(s). Blocks: destructive commands, reads/writes of \
         [policy.filesystem] deny paths, writes outside the workspace/[guard] allow_roots.\n\
         This is cooperative (accident) protection — for hostile code use `agentstack run --sandbox`.",
        "✓".green().bold()
    );
    println!(
        "  config: {} ([guard] + [policy.filesystem] deny)",
        paths::agentstack_home().join("agentstack.toml").display()
    );
    Ok(())
}

fn uninstall() -> Result<()> {
    let exe = exe();
    for t in targets() {
        if !target_installed(&t) {
            continue;
        }
        match apply_target(&t, &exe, false) {
            Ok(path) => println!(
                "  {} {} — guard removed from {}",
                "✓".green(),
                t.id,
                path.display()
            ),
            Err(e) => println!("  {} {} — {e:#}", "✗".red(), t.id),
        }
    }
    let dir = paths::agentstack_home().join("guard");
    if dir.exists() {
        fs::remove_dir_all(&dir).ok();
    }
    set_guard_enabled(false)?;
    println!(
        "\n{} guard hooks removed; [guard] enabled = false.",
        "✓".green().bold()
    );
    Ok(())
}

fn status() -> Result<()> {
    let (cfg, guard_error) = match manifest::machine_guard_health() {
        Some(Ok(c)) => (Some(c), None),
        Some(Err(e)) => (None, Some(format!("{e:#}"))),
        None => (Some(Default::default()), None),
    };
    let inspected = crate::machine_policy::inspect();
    println!(
        "guard: {}",
        match (&cfg, &guard_error) {
            (_, Some(error)) =>
                format!("{} — machine config unreadable ({error})", "BLOCKED".red()),
            (Some(cfg), None) if cfg.enabled() => "enabled".green().to_string(),
            _ => "disabled (run `agentstack guard install`)"
                .yellow()
                .to_string(),
        }
    );
    match &inspected.status {
        crate::machine_policy::Status::Unconfigured => {
            println!("  machine policy: unconfigured");
        }
        crate::machine_policy::Status::Current { .. } => {
            println!("  machine policy: current");
        }
        crate::machine_policy::Status::LastKnownGood { source_error, .. } => {
            println!(
                "  machine policy: {} — source unreadable ({source_error})",
                "DEGRADED (last-known-good)".yellow()
            );
        }
        crate::machine_policy::Status::Blocked {
            source_error,
            snapshot_error,
        } => {
            println!(
                "  machine policy: {} — source: {source_error}; snapshot: {snapshot_error}",
                "BLOCKED".red()
            );
        }
    }
    if let Some(policy) = &inspected.policy {
        println!(
            "  deny globs ({}): {}",
            policy.filesystem.deny.len(),
            policy.filesystem.deny.join(", ")
        );
    }
    if let Some(cfg) = &cfg {
        println!("  allow_roots: {}", cfg.allow_roots.join(", "));
    } else {
        println!("  allow_roots: unavailable");
    }
    for t in targets() {
        let detected = paths::expand_tilde(t.detect).exists();
        let installed = detected && target_installed(&t);
        let mark = match (detected, installed) {
            (false, _) => "· not detected".dimmed().to_string(),
            (true, true) => "✓ hook installed".green().to_string(),
            (true, false) => "✗ detected, hook missing".yellow().to_string(),
        };
        println!("  {:<32} {mark}", t.id);
    }
    for (id, why) in NO_HOOK_SURFACE {
        println!("  {id:<32} {} ({why})", "○ no hook surface".dimmed());
    }
    Ok(())
}

// ── file surgery ────────────────────────────────────────────────────────────

/// Install (or remove, with `add = false`) the guard's entries for one
/// target. Returns the touched path.
fn apply_target(t: &Target, exe: &str, add: bool) -> Result<PathBuf> {
    match &t.kind {
        Kind::OwnedFile { path, contents } => {
            let path = paths::expand_tilde(path);
            if add {
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir)?;
                }
                crate::util::atomic::write(&path, &contents(exe))?;
            } else if path.exists() {
                fs::remove_file(&path)?;
            }
            Ok(path)
        }
        Kind::SharedJson {
            path,
            events,
            entry,
            top_level,
        } => {
            let path = paths::expand_tilde(path);
            let existing = match fs::read_to_string(&path) {
                Ok(t) => t,
                // Removing from a file that doesn't exist must not create it.
                Err(_) if !add => return Ok(path),
                Err(_) => String::new(),
            };
            let mut root: Value = if existing.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&existing)
                    .with_context(|| format!("{} is not valid JSON", path.display()))?
            };
            let obj = root
                .as_object_mut()
                .with_context(|| format!("{} is not a JSON object", path.display()))?;
            for (k, v) in *top_level {
                obj.entry(k.to_string()).or_insert(json!(v));
            }
            let hooks = obj
                .entry("hooks".to_string())
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .with_context(|| format!("`hooks` in {} is not an object", path.display()))?;
            for event in *events {
                let arr = hooks
                    .entry(event.to_string())
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .with_context(|| format!("hooks.{event} is not an array"))?;
                // Remove any previous guard entry (ours are recognizable by
                // the marker or the guard-check command in any string field).
                arr.retain(|e| !value_mentions_guard(e));
                if add {
                    arr.push(entry(exe));
                }
                if arr.is_empty() {
                    hooks.remove(*event);
                }
            }
            if hooks.is_empty() {
                obj.remove("hooks");
            }
            // After a removal, a file holding nothing but the scaffolding we
            // added (e.g. cursor's `"version": 1`) — or nothing at all — is
            // ours to delete rather than leave as an empty husk.
            if !add && obj.keys().all(|k| top_level.iter().any(|(t, _)| t == k)) {
                fs::remove_file(&path).ok();
                return Ok(path);
            }
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            crate::util::atomic::write(&path, &(serde_json::to_string_pretty(&root)? + "\n"))?;
            Ok(path)
        }
    }
}

fn target_installed(t: &Target) -> bool {
    match &t.kind {
        Kind::OwnedFile { path, .. } => paths::expand_tilde(path).exists(),
        Kind::SharedJson { path, .. } => fs::read_to_string(paths::expand_tilde(path))
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .map(|v| value_mentions_guard(&v))
            .unwrap_or(false),
    }
}

/// Deep-scan a JSON value for our marker or the guard-check invocation.
fn value_mentions_guard(v: &Value) -> bool {
    match v {
        Value::String(s) => s.contains(MARKER) || s.contains("guard check --protocol"),
        Value::Array(a) => a.iter().any(value_mentions_guard),
        Value::Object(o) => o.values().any(value_mentions_guard),
        _ => false,
    }
}

fn write_wrappers() -> Result<()> {
    let exe = exe();
    let dir = paths::agentstack_home().join("guard");
    fs::create_dir_all(&dir)?;
    for (name, protocol) in [
        ("agentstack-guard-cursor.sh", "cursor"),
        ("agentstack-guard-windsurf.sh", "windsurf"),
        ("agentstack-guard-copilot.sh", "copilot"),
    ] {
        let path = dir.join(name);
        let body =
            format!("#!/bin/sh\n# {MARKER}\nexec \"{exe}\" guard check --protocol {protocol}\n");
        crate::util::atomic::write(&path, &body)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}

// ── machine manifest seeding (toml_edit keeps user comments intact) ─────────

fn seed_machine_config() -> Result<()> {
    let path = paths::agentstack_home().join("agentstack.toml");
    fs::create_dir_all(paths::agentstack_home())?;
    let text = fs::read_to_string(&path).unwrap_or_else(|_| "version = 1\n".to_string());
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("{} is not valid TOML", path.display()))?;
    // Real `[guard]` / `[policy.filesystem]` tables (not inline) — this file
    // is hand-edited (allow_roots, deny), so it should read like TOML people
    // write.
    if !doc.contains_key("guard") {
        doc["guard"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    doc["guard"]["enabled"] = toml_edit::value(true);
    if doc["guard"].get("allow_roots").is_none() {
        doc["guard"]["allow_roots"] = toml_edit::value(toml_edit::Array::new());
    }
    // Seed the deny list only when the user has never written one — an
    // explicitly empty list is an opt-out we must respect.
    let has_deny = doc
        .get("policy")
        .and_then(|p| p.get("filesystem"))
        .and_then(|f| f.get("deny"))
        .is_some();
    if !has_deny {
        if !doc.contains_key("policy") {
            let mut t = toml_edit::Table::new();
            t.set_implicit(true); // print [policy.filesystem], not empty [policy]
            doc["policy"] = toml_edit::Item::Table(t);
        }
        if doc["policy"].get("filesystem").is_none() {
            doc["policy"]["filesystem"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let mut arr = toml_edit::Array::new();
        for d in DEFAULT_DENY {
            arr.push(*d);
        }
        doc["policy"]["filesystem"]["deny"] = toml_edit::value(arr);
    }
    crate::util::atomic::write(&path, &doc.to_string())?;
    Ok(())
}

fn set_guard_enabled(enabled: bool) -> Result<()> {
    let path = paths::agentstack_home().join("agentstack.toml");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(());
    };
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("{} is not valid TOML", path.display()))?;
    doc["guard"]["enabled"] = toml_edit::value(enabled);
    crate::util::atomic::write(&path, &doc.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `[guard.project_roots]` is a MACHINE-owned, workspace-scoped grant:
    /// the extra root applies inside the keyed workspace (and its subdirs,
    /// e.g. worktrees), and NOT anywhere else — a different project gets only
    /// the global allow_roots.
    #[test]
    fn project_roots_grant_only_inside_the_keyed_workspace() {
        let cfg = agentstack_core::manifest::GuardConfig {
            enabled: Some(true),
            allow_roots: vec!["/machines/shared".into()],
            project_roots: indexmap::IndexMap::from([(
                "/work/agentstack".to_string(),
                vec!["/home/me/agent-setup".to_string()],
            )]),
        };
        let inside = effective_allow_roots(&cfg, Path::new("/work/agentstack"));
        assert!(inside.contains(&PathBuf::from("/home/me/agent-setup")));
        let nested = effective_allow_roots(&cfg, Path::new("/work/agentstack/.claude/worktrees/x"));
        assert!(nested.contains(&PathBuf::from("/home/me/agent-setup")));
        let elsewhere = effective_allow_roots(&cfg, Path::new("/work/other-repo"));
        assert!(!elsewhere.contains(&PathBuf::from("/home/me/agent-setup")));
        assert!(
            elsewhere.contains(&PathBuf::from("/machines/shared")),
            "global allow_roots still apply everywhere"
        );
    }

    #[test]
    fn nearest_ancestor_walks_up_to_the_first_matching_root() {
        // Pure predicate, no disk: from a deep subdir the walk must return the
        // project root, not the starting directory.
        let start = Path::new("/work/repo/src/deep");
        let root = Path::new("/work/repo");
        assert_eq!(nearest_ancestor(start, |d| d == root), root);
    }

    #[test]
    fn nearest_ancestor_falls_back_to_start_when_nothing_matches() {
        let start = Path::new("/loose/dir");
        assert_eq!(nearest_ancestor(start, |_| false), start);
    }

    #[test]
    fn anchor_workspace_finds_the_repo_root_from_a_subdirectory() {
        // The regression this fixes: an agent that `cd`'d into a subdirectory
        // must still count the repo root (marked by `.git`) as its workspace,
        // so a write to a repo-root file is not "outside the workspace".
        let root = tempdir().join(format!("anchor-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let sub = root.join("docs");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(anchor_workspace(&sub), root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn project_policy_loads_from_the_preferred_agentstack_layout() {
        // The regression this pins: a [policy.filesystem] deny declared in
        // the PREFERRED `.agentstack/agentstack.toml` was silently ignored —
        // the workspace root (not the manifest dir) was handed straight to
        // the loader, so only legacy-root manifests were ever enforced.
        let root = tempdir().join(format!("guardpol-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".agentstack")).unwrap();
        std::fs::write(
            root.join(".agentstack/agentstack.toml"),
            "version = 1\n[policy.filesystem]\ndeny = [\"vault/**\"]\n",
        )
        .unwrap();
        assert_eq!(project_policy_at(&root).filesystem.deny, vec!["vault/**"]);

        // The legacy root layout keeps working (manifest dir IS the workspace).
        let legacy = tempdir().join(format!("guardpol-legacy-{}", std::process::id()));
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("agentstack.toml"),
            "version = 1\n[policy.filesystem]\ndeny = [\"secrets/**\"]\n",
        )
        .unwrap();
        assert_eq!(
            project_policy_at(&legacy).filesystem.deny,
            vec!["secrets/**"]
        );

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&legacy);
    }

    #[test]
    fn oversized_hook_payload_is_rejected_before_json_parsing() {
        let payload = vec![b' '; MAX_PAYLOAD as usize + 1];
        let error = read_payload(std::io::Cursor::new(payload)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    /// The surgical merge owns ONLY its own entries: foreign hooks in a
    /// shared file survive install + uninstall byte-for-byte.
    #[test]
    fn shared_json_surgery_preserves_foreign_entries() {
        let dir = tempdir();
        std::env::set_var("HOME_TEST_GUARD", dir.display().to_string());
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{"model":"opus","hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"my-own-hook"}]}]}}"#,
        )
        .unwrap();

        let t = Target {
            id: "test",
            detect: "~",
            kind: Kind::SharedJson {
                path: Box::leak(path.display().to_string().into_boxed_str()),
                events: &["PreToolUse"],
                entry: claude_entry,
                top_level: &[],
            },
        };
        // Install twice (idempotent), then verify both entries coexist.
        apply_target(&t, "/bin/agentstack", true).unwrap();
        apply_target(&t, "/bin/agentstack", true).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let arr = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2, "foreign + exactly one guard entry: {v}");
        assert_eq!(v["model"], "opus");
        assert!(value_mentions_guard(&v));

        // Uninstall removes only ours.
        apply_target(&t, "/bin/agentstack", false).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let arr = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert!(!value_mentions_guard(&v));
        assert_eq!(v["model"], "opus");
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("guard-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
