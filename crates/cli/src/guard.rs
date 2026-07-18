//! Host-mode destructive-command guard — the engine behind `agentstack guard`.
//!
//! Each agent CLI is wired (via `agentstack guard install`) to run
//! `agentstack guard check` as a pre-tool-use hook. The hook receives the
//! pending tool call on stdin, and this module decides allow/deny from the
//! machine's own config: `[policy.filesystem] deny` globs (never readable or
//! writable) and `[guard] allow_roots` (where writes are allowed beyond the
//! workspace). Denials are recorded to the audit log.
//!
//! Claim discipline (mirrors ENFORCEMENT.md): this is COOPERATIVE
//! enforcement — it protects against an agent's *accidents* in everyday host
//! use, because the harness chooses to consult the hook. A malicious agent
//! or a harness that ignores its own hook protocol bypasses it entirely; the
//! kernel-enforced story is `run --sandbox` / `--lockdown`. Never describe
//! this dimension as "enforced".
//!
//! The engine is pure (no I/O) with one exception: [`GuardContext`] carries
//! everything a decision needs, so the whole surface is unit-testable — but
//! `deny_glob_check` symlink-resolves paths to catch equivalent spellings
//! (macOS `/var` vs `/private/var`), I/O that can only ADD deny spellings
//! and degrades to the lexical check on fake test paths. Protocol
//! translation (each CLI's payload/response dialect) lives in
//! [`Protocol`]; the shell-command analysis is a conservative tokenizer,
//! not a full parser — bounded, allocation-light, and honest about its
//! limits (a `cd` in one segment does not re-anchor relative paths in the
//! next).

use std::path::{Component, Path, PathBuf};

use agentstack_policy::CompiledRuleset;
use serde_json::{json, Value};

/// What the pending tool call is, once a protocol has parsed its payload.
#[derive(Debug, Clone, PartialEq)]
pub enum GuardEvent {
    /// A shell command (the high-risk surface).
    Bash { command: String },
    /// A read-shaped file tool (Read / Glob / Grep …).
    FileRead { path: String },
    /// A write-shaped file tool (Write / Edit / NotebookEdit …).
    FileWrite { path: String },
    /// Anything else — allowed (the guard constrains files and shells, not
    /// e.g. web fetches; egress is the proxy's dimension).
    Other,
}

/// Everything a decision needs, resolved once by the caller.
pub struct GuardContext {
    /// The workspace the agent is working in (the hook payload's `cwd`).
    pub workspace: PathBuf,
    /// The user's home directory (deleting it, or `/`, is always refused).
    pub home: PathBuf,
    /// Temp directories writes are always allowed in.
    pub tmp: Vec<PathBuf>,
    /// `[guard] allow_roots` — extra write roots beyond the workspace.
    pub allow_roots: Vec<PathBuf>,
    /// `~/.agentstack` (or `$AGENTSTACK_HOME`) — the guard's own config and
    /// state: the machine manifest whose `[guard]` table configures this very
    /// check, the trust store, and the hook wrapper scripts. Shell writes
    /// here are always denied, even inside `allow_roots` — otherwise
    /// `allow_roots` could be edited to allowlist itself.
    pub agentstack_home: PathBuf,
    /// Compiled policy; only `[policy.filesystem] deny` is consulted here.
    pub ruleset: CompiledRuleset,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    Deny { reason: String },
}

impl Decision {
    fn deny(reason: impl Into<String>) -> Self {
        Decision::Deny {
            reason: reason.into(),
        }
    }
    pub fn is_deny(&self) -> bool {
        matches!(self, Decision::Deny { .. })
    }
}

// ── The decision engine ─────────────────────────────────────────────────────

/// The one entry point: decide a parsed event against the context.
pub fn check_event(ctx: &GuardContext, event: &GuardEvent) -> Decision {
    match event {
        GuardEvent::Other => Decision::Allow,
        GuardEvent::FileRead { path } => deny_glob_check(ctx, path),
        GuardEvent::FileWrite { path } => write_target_check(ctx, path),
        GuardEvent::Bash { command } => check_bash(ctx, command),
    }
}

/// `[policy.filesystem] deny` for one path, matched in every spelling we
/// know (absolute, workspace-relative, bare file name) — more spellings can
/// only make a blocklist stricter.
fn deny_glob_check(ctx: &GuardContext, path: &str) -> Decision {
    let abs = normalize(path, &ctx.workspace, &ctx.home);
    let mut spellings: Vec<String> = vec![abs.to_string_lossy().into_owned()];
    // The workspace-relative spelling, tried across the as-reported AND
    // symlink-resolved forms of both sides. A payload can name the same file
    // under two equivalent spellings (macOS: `cwd` as `/var/...`, `file_path`
    // as `/private/var/...`); a lexical-only strip fails then, and losing the
    // relative spelling silently fails OPEN for path-prefixed deny globs
    // like `vault/**` (#23). Resolving only ADDS spellings, so it cannot
    // weaken the blocklist.
    let workspaces = [
        Some(ctx.workspace.clone()),
        std::fs::canonicalize(&ctx.workspace).ok(),
    ];
    let targets = [Some(abs.clone()), resolve_existing_prefix(&abs)];
    for ws in workspaces.iter().flatten() {
        for target in targets.iter().flatten() {
            if let Ok(rel) = target.strip_prefix(ws) {
                let rel = rel.to_string_lossy().into_owned();
                if !spellings.contains(&rel) {
                    spellings.push(rel);
                }
            }
        }
    }
    if let Some(name) = abs.file_name() {
        spellings.push(name.to_string_lossy().into_owned());
    }
    let refs: Vec<&str> = spellings.iter().map(String::as_str).collect();
    match ctx.ruleset.fs_deny_decision(&refs) {
        Ok(()) => Decision::Allow,
        Err(rule) => Decision::deny(format!("{path}: {rule}")),
    }
}

/// Writes are confined to the workspace, `[guard] allow_roots`, and temp
/// dirs — deny-by-default everywhere else (that "everywhere else" includes
/// the rest of the home directory).
fn write_scope_check(ctx: &GuardContext, path: &str) -> Decision {
    let abs = normalize(path, &ctx.workspace, &ctx.home);
    if within(&abs, &ctx.workspace)
        || ctx.allow_roots.iter().any(|r| within(&abs, r))
        || ctx.tmp.iter().any(|r| within(&abs, r))
    {
        return Decision::Allow;
    }
    // Teach the exact fix inline (P3), the way the deny-glob denial names its
    // source: the precise TOML line to add, keyed on the denied path's PARENT
    // directory, and the file it goes in.
    let dir = abs.parent().unwrap_or(&abs);
    Decision::deny(format!(
        "write outside the workspace: {} (allowed: the workspace, [guard] allow_roots, temp dirs)\n  \
         to allow writes here, add to {} →\n    [guard]\n    allow_roots = [\"{}\"]",
        abs.display(),
        machine_manifest(ctx).display(),
        dir.display(),
    ))
}

/// The machine manifest whose `[guard]` / `[policy.filesystem]` tables drive
/// this check — named in denials so the user knows where the rule (and its
/// fix) lives.
fn machine_manifest(ctx: &GuardContext) -> PathBuf {
    ctx.agentstack_home.join("agentstack.toml")
}

/// Append the source citation to a built-in destructive-command denial (P11):
/// these patterns are hard-coded, but the deny/allow lists that govern the
/// rest live in the machine manifest — name it so the block is not a mystery.
fn cite_builtin(ctx: &GuardContext, decision: Decision) -> Decision {
    match decision {
        Decision::Deny { reason } => Decision::deny(format!(
            "{reason} (built-in rule; deny/allow lists: {})",
            machine_manifest(ctx).display()
        )),
        allow => allow,
    }
}

/// Every operation that can modify or delete a path must pass both the
/// machine/project deny globs and the writable-root boundary. Keeping this as
/// one primitive prevents spelling-specific command handlers from omitting
/// half of the check.
fn write_target_check(ctx: &GuardContext, path: &str) -> Decision {
    if let d @ Decision::Deny { .. } = deny_glob_check(ctx, path) {
        return d;
    }
    write_scope_check(ctx, path)
}

/// [`write_target_check`] for writes reached through the SHELL (in-place
/// edits, redirects, rm/mv/cp/tee …), with one addition: targets inside
/// `~/.agentstack` are always denied, `allow_roots` notwithstanding. That
/// directory holds the machine manifest whose `[guard]` table configures
/// this very check (a `sed -i` there could widen `allow_roots` or flip
/// `enabled = false`), the trust store, and the hook wrapper scripts the
/// CLIs execute. File-tool writes (Write/Edit) are exempt: those diffs are
/// shown to the user by the harness, and legitimately edit manifests.
fn shell_write_check(ctx: &GuardContext, path: &str) -> Decision {
    let abs = normalize(path, &ctx.workspace, &ctx.home);
    // Both sides in as-given AND symlink-resolved spellings — resolving can
    // only ADD ways to hit the deny, never ways to escape it.
    let targets = [Some(abs.clone()), resolve_existing_prefix(&abs)];
    let homes = [
        Some(ctx.agentstack_home.clone()),
        resolve_existing_prefix(&ctx.agentstack_home),
    ];
    for t in targets.iter().flatten() {
        for h in homes.iter().flatten() {
            if within(t, h) {
                return Decision::deny(format!(
                    "{} is inside {} — the guard's own config and state; \
                     [guard] allow_roots cannot allowlist it (edit it directly, \
                     outside the agent)",
                    abs.display(),
                    ctx.agentstack_home.display()
                ));
            }
        }
    }
    write_target_check(ctx, path)
}

/// Lexical normalization: make `path` absolute against `base`, expand `~`
/// and `$HOME`, resolve `.`/`..` without touching the filesystem (the
/// target may not exist yet, and a symlink-following canonicalize would
/// answer about the wrong moment anyway — the hook runs before the call).
fn normalize(path: &str, base: &Path, home: &Path) -> PathBuf {
    let expanded: PathBuf = if path == "~" || path == "$HOME" || path == "${HOME}" {
        home.to_path_buf()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else if let Some(rest) = path.strip_prefix("$HOME/") {
        home.join(rest)
    } else if let Some(rest) = path.strip_prefix("${HOME}/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    };
    let joined = if expanded.is_absolute() {
        expanded
    } else {
        base.join(expanded)
    };
    let mut out = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Symlink-resolve as much of `path` as exists on disk, re-attaching any
/// non-existent tail unchanged. `fs::canonicalize` alone errors on paths
/// that don't exist yet, which would lose the resolved spelling exactly
/// when a write target is being judged. Used only to add spellings to the
/// deny blocklist — never to decide an allow.
fn resolve_existing_prefix(path: &Path) -> Option<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(real) => Some(real),
        // Recursion is bounded by the component count; `/` always
        // canonicalizes, so the walk terminates before `parent()` runs out.
        Err(_) => {
            let parent = path.parent()?;
            let name = path.file_name()?;
            Some(resolve_existing_prefix(parent)?.join(name))
        }
    }
}

/// Component-wise prefix check (string prefixes would let `/tmp2` pass as
/// inside `/tmp`).
fn within(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root).is_ok()
}

// ── Shell-command analysis ──────────────────────────────────────────────────

/// Wrappers that just run another command: strip them and judge what they
/// run. (`xargs` is handled separately — its targets come from stdin, so
/// they are unknowable here.)
const WRAPPERS: &[&str] = &[
    "sudo", "env", "nohup", "time", "nice", "ionice", "command", "builtin", "exec", "stdbuf",
];

fn check_bash(ctx: &GuardContext, command: &str) -> Decision {
    for segment in split_segments(command) {
        let tokens = tokenize(&segment);
        if tokens.is_empty() {
            continue;
        }
        // Any token naming a deny-globbed path blocks the whole command —
        // this is what catches `cat .env`, `source .env`, `cp .env /tmp`.
        for tok in &tokens {
            if !tok.starts_with('-') {
                if let d @ Decision::Deny { .. } = deny_glob_check(ctx, tok) {
                    return d;
                }
            }
        }
        // Redirections write: `> file`, `>> file`, `2> file`, `>file`.
        for target in redirect_targets(&tokens) {
            if target.starts_with("/dev/") {
                if !matches!(target.as_str(), "/dev/null" | "/dev/stdout" | "/dev/stderr") {
                    return Decision::deny(format!("redirect into a device: > {target}"));
                }
                continue;
            }
            if let d @ Decision::Deny { .. } = shell_write_check(ctx, &target) {
                return d;
            }
        }
        let (program, rest, via_xargs) = strip_wrappers(tokens);
        let Some(program) = program else { continue };
        let d = match program.as_str() {
            "rm" => check_rm(ctx, &rest, via_xargs),
            "git" => cite_builtin(ctx, check_git(&rest)),
            "find" => check_find(ctx, &rest),
            "shred" => cite_builtin(
                ctx,
                Decision::deny("shred irrecoverably destroys file contents"),
            ),
            "dd" => cite_builtin(ctx, check_dd(&rest)),
            "diskutil" => cite_builtin(ctx, check_diskutil(&rest)),
            "chmod" | "chown" => check_chmod_chown(ctx, program.as_str(), &rest),
            // tee's non-flag operands are all write targets; truncate's too.
            "truncate" | "tee" => check_write_targets(ctx, program.as_str(), &rest),
            "sed" | "perl" => check_in_place_edit(ctx, program.as_str(), &rest),
            "mv" | "cp" => check_mv_cp(ctx, program.as_str(), &rest),
            // `install -d` creates every operand; otherwise it copies like cp.
            "install" => {
                if combined_flags(&rest).contains('d') {
                    check_write_targets(ctx, program.as_str(), &rest)
                } else {
                    check_mv_cp(ctx, program.as_str(), &rest)
                }
            }
            p if p.starts_with("mkfs") => cite_builtin(
                ctx,
                Decision::deny(format!(
                    "{p} formats a filesystem — never allowed via a hook"
                )),
            ),
            _ => Decision::Allow,
        };
        if d.is_deny() {
            return d;
        }
    }
    Decision::Allow
}

/// Split a command line into independently judged segments on `;`, `&&`,
/// `||`, `|`, `&`, newlines, and command substitution boundaries — outside
/// quotes. Substitution contents become their own segments, so
/// `echo $(rm -rf /)` is judged as `rm -rf /`.
fn split_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
                current.push(c);
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    current.push(c);
                }
                ';' | '\n' | '|' | '&' | '`' => {
                    segments.push(std::mem::take(&mut current));
                    // Swallow the doubled operator char (&&, ||).
                    if (c == '&' || c == '|') && chars.peek() == Some(&c) {
                        chars.next();
                    }
                }
                '$' if chars.peek() == Some(&'(') => {
                    chars.next();
                    segments.push(std::mem::take(&mut current));
                }
                ')' => segments.push(std::mem::take(&mut current)),
                _ => current.push(c),
            },
        }
    }
    segments.push(current);
    segments.retain(|s| !s.trim().is_empty());
    segments
}

/// Whitespace tokenizer that honors (and strips) single/double quotes.
fn tokenize(segment: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in segment.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None => match c {
                '\'' | '"' => quote = Some(c),
                c if c.is_whitespace() => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                _ => current.push(c),
            },
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Skip env assignments and wrapper programs; returns the effective program
/// (basename), its args, and whether it runs under `xargs` (targets unknown).
// Takes `tokens` by value so the tail can be MOVED out (`split_off`) instead of
// cloned — this runs on every `;`/`&&`/`|`-separated segment of every guarded
// bash command, the hottest path in the guard.
fn strip_wrappers(mut tokens: Vec<String>) -> (Option<String>, Vec<String>, bool) {
    let mut i = 0;
    let mut via_xargs = false;
    while i < tokens.len() {
        let t = &tokens[i];
        let base = Path::new(t)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| t.clone());
        if t.contains('=') && !t.starts_with('-') && !t.starts_with('/') && i == 0 {
            // Leading VAR=value assignment.
            i += 1;
        } else if WRAPPERS.contains(&base.as_str()) {
            i += 1;
            // `env` and `sudo` may be followed by more assignments/flags.
            while i < tokens.len() && (tokens[i].contains('=') || tokens[i].starts_with('-')) {
                i += 1;
            }
        } else if base == "xargs" {
            via_xargs = true;
            i += 1;
            while i < tokens.len() && tokens[i].starts_with('-') {
                i += 1;
            }
        } else {
            // Move the tail out rather than cloning it — `tokens` is owned and
            // dropped right after this return.
            let rest = tokens.split_off(i + 1);
            return (Some(base), rest, via_xargs);
        }
    }
    (None, Vec::new(), via_xargs)
}

/// Extract write targets of `>`/`>>` redirections (with optional fd digits).
fn redirect_targets(tokens: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut expect_target = false;
    for t in tokens {
        if expect_target {
            out.push(t.clone());
            expect_target = false;
            continue;
        }
        let stripped = t.trim_start_matches(|c: char| c.is_ascii_digit());
        if stripped == ">" || stripped == ">>" {
            expect_target = true;
        } else if let Some(rest) = stripped.strip_prefix(">>") {
            if !rest.is_empty() && !rest.starts_with('&') {
                out.push(rest.to_string());
            }
        } else if let Some(rest) = stripped.strip_prefix('>') {
            if !rest.is_empty() && !rest.starts_with('&') && !rest.starts_with('(') {
                out.push(rest.to_string());
            }
        }
    }
    out
}

/// The targets of a command: everything not flag-shaped (after `--`,
/// everything).
fn targets_of(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut after_dashdash = false;
    for a in args {
        if a == "--" {
            after_dashdash = true;
        } else if after_dashdash || !a.starts_with('-') {
            out.push(a.clone());
        }
    }
    out
}

fn combined_flags(args: &[String]) -> String {
    args.iter()
        .take_while(|a| *a != "--")
        .filter(|a| a.starts_with('-') && !a.starts_with("--"))
        .flat_map(|a| a.chars().skip(1))
        .collect()
}

fn check_rm(ctx: &GuardContext, args: &[String], via_xargs: bool) -> Decision {
    let flags = combined_flags(args);
    let recursive =
        flags.contains('r') || flags.contains('R') || args.iter().any(|a| a == "--recursive");
    if recursive && via_xargs {
        return cite_builtin(
            ctx,
            Decision::deny(
                "recursive rm via xargs — targets come from stdin and cannot be checked",
            ),
        );
    }
    for t in targets_of(args) {
        let abs = normalize(&t, &ctx.workspace, &ctx.home);
        if abs == Path::new("/") || abs == ctx.home || abs == ctx.workspace {
            return cite_builtin(
                ctx,
                Decision::deny(format!(
                    "rm of {} — refusing to delete a root",
                    abs.display()
                )),
            );
        }
        // Deletion is a write: confined to the workspace + allow_roots + tmp.
        if let d @ Decision::Deny { .. } = shell_write_check(ctx, &t) {
            return d;
        }
    }
    Decision::Allow
}

fn check_git(args: &[String]) -> Decision {
    // Skip global flags (`-C <dir>`, `-c a=b`) to find the subcommand.
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-C" | "-c" => i += 2,
            a if a.starts_with('-') => i += 1,
            _ => break,
        }
    }
    let Some(sub) = args.get(i) else {
        return Decision::Allow;
    };
    let rest = &args[i + 1..];
    let flags = combined_flags(rest);
    match sub.as_str() {
        "reset" if rest.iter().any(|a| a == "--hard") => Decision::deny(
            "git reset --hard discards uncommitted work irrecoverably — stash or commit first",
        ),
        "clean" if flags.contains('f') || rest.iter().any(|a| a == "--force") => {
            Decision::deny("git clean -f deletes untracked files irrecoverably")
        }
        "checkout" if rest.iter().any(|a| a == ".") => {
            Decision::deny("git checkout . discards all working-tree changes — stash first")
        }
        "restore" if rest.iter().any(|a| a == ".") && !rest.iter().any(|a| a == "--staged") => {
            Decision::deny("git restore . discards all working-tree changes — stash first")
        }
        "push"
            if (flags.contains('f') || rest.iter().any(|a| a == "--force"))
                && !rest.iter().any(|a| a.starts_with("--force-with-lease")) =>
        {
            Decision::deny("git push --force without --force-with-lease can destroy remote history")
        }
        "stash" if rest.first().map(String::as_str) == Some("clear") => {
            Decision::deny("git stash clear drops every stash irrecoverably")
        }
        _ => Decision::Allow,
    }
}

fn check_find(ctx: &GuardContext, args: &[String]) -> Decision {
    if !args.iter().any(|a| a == "-delete") {
        return Decision::Allow;
    }
    // find's roots come before the first expression token.
    let roots: Vec<&String> = args
        .iter()
        .take_while(|a| !a.starts_with('-') && *a != "(")
        .collect();
    if roots.is_empty() {
        return Decision::Allow; // implicit `.` — inside the workspace
    }
    for r in roots {
        if let d @ Decision::Deny { .. } = shell_write_check(ctx, r) {
            return d;
        }
        let abs = normalize(r, &ctx.workspace, &ctx.home);
        if abs == Path::new("/") || abs == ctx.home {
            return cite_builtin(
                ctx,
                Decision::deny(format!("find {} -delete — refusing a root", abs.display())),
            );
        }
    }
    Decision::Allow
}

fn check_dd(args: &[String]) -> Decision {
    for a in args {
        if let Some(of) = a.strip_prefix("of=") {
            if of.starts_with("/dev/") && of != "/dev/null" {
                return Decision::deny(format!("dd writing to a device: {a}"));
            }
        }
    }
    Decision::Allow
}

fn check_diskutil(args: &[String]) -> Decision {
    match args.first().map(String::as_str) {
        Some("eraseDisk") | Some("eraseVolume") | Some("partitionDisk") | Some("zeroDisk") => {
            Decision::deny("diskutil erase/partition destroys a volume")
        }
        _ => Decision::Allow,
    }
}

fn check_chmod_chown(ctx: &GuardContext, program: &str, args: &[String]) -> Decision {
    let recursive = combined_flags(args).contains('R') || args.iter().any(|a| a == "--recursive");
    if !recursive {
        return Decision::Allow;
    }
    for t in targets_of(args).iter().skip(1) {
        // skip(1): the mode/owner argument
        let abs = normalize(t, &ctx.workspace, &ctx.home);
        if abs == Path::new("/") || abs == ctx.home {
            return cite_builtin(
                ctx,
                Decision::deny(format!("{program} -R on {}", abs.display())),
            );
        }
        if let d @ Decision::Deny { .. } = shell_write_check(ctx, t) {
            return d;
        }
    }
    Decision::Allow
}

fn check_write_targets(ctx: &GuardContext, program: &str, args: &[String]) -> Decision {
    for t in targets_of(args) {
        // The sink devices a pipeline legitimately tees into.
        if matches!(t.as_str(), "/dev/null" | "/dev/stdout" | "/dev/stderr") {
            continue;
        }
        if let Decision::Deny { reason } = shell_write_check(ctx, &t) {
            return Decision::deny(format!("{program}: {reason}"));
        }
    }
    Decision::Allow
}

fn check_mv_cp(ctx: &GuardContext, program: &str, args: &[String]) -> Decision {
    let targets = targets_of(args);
    if targets.len() < 2 {
        return Decision::Allow;
    }
    // The destination is a write; for `mv`, sources are deletions too.
    let (sources, dest) = targets.split_at(targets.len() - 1);
    if let Decision::Deny { reason } = shell_write_check(ctx, &dest[0]) {
        return Decision::deny(format!("{program} destination: {reason}"));
    }
    for s in sources {
        if let d @ Decision::Deny { .. } = deny_glob_check(ctx, s) {
            return d;
        }
        if program == "mv" {
            if let Decision::Deny { reason } = shell_write_check(ctx, s) {
                return Decision::deny(format!("mv source (a deletion): {reason}"));
            }
        }
    }
    Decision::Allow
}

/// `sed -i` / `perl -i` rewrite their file operands in place — a write, not
/// a read. Conservative by design (this is cooperative accident protection,
/// per the `GuardConfig` doc): only operands statically identifiable as
/// paths (absolute or `~`/`$HOME`-anchored) are judged, and the script
/// operand is skipped, so an invocation we can't parse degrades to the
/// pre-existing allow rather than a false block.
fn check_in_place_edit(ctx: &GuardContext, program: &str, args: &[String]) -> Decision {
    if !has_in_place_flag(program, args) {
        return Decision::Allow;
    }
    // Flags whose separate VALUE is not a file operand. The script-carrying
    // ones among them also mean "every remaining operand is a file".
    let (value_flags, script_flags): (&[&str], &[&str]) = if program == "sed" {
        (
            &["-e", "-f", "--expression", "--file"],
            &["-e", "-f", "--expression", "--file"],
        )
    } else {
        (&["-e", "-E", "-I"], &["-e", "-E"])
    };
    let mut operands: Vec<&String> = Vec::new();
    let mut script_via_flag = false;
    let mut skip_value = false;
    let mut after_dashdash = false;
    for a in args {
        if skip_value {
            skip_value = false;
        } else if after_dashdash {
            operands.push(a);
        } else if a == "--" {
            after_dashdash = true;
        } else if a.starts_with('-') && a.len() > 1 {
            if value_flags.contains(&a.as_str()) {
                skip_value = true;
                script_via_flag |= script_flags.contains(&a.as_str());
            }
        } else {
            operands.push(a);
        }
    }
    let files = if script_via_flag {
        operands.as_slice()
    } else {
        // Without `-e`/`-f` the first operand is the script (sed) or the
        // program file (perl) — a read, not a write target.
        operands.get(1..).unwrap_or_default()
    };
    for f in files {
        if !is_explicit_path(f) {
            continue;
        }
        if let Decision::Deny { reason } = shell_write_check(ctx, f) {
            return Decision::deny(format!("{program} -i rewrites {f} in place: {reason}"));
        }
    }
    Decision::Allow
}

/// Does this sed/perl invocation edit in place? Clusters are scanned
/// left-to-right with each program's grammar: an `i` before any
/// value-taking letter means in-place (`-i`, `-i.bak`, `-pi`, `-Ei`); a
/// letter that consumes the rest of the token (or the next arg) stops the
/// scan, so `perl -ne'if…'` and `-Mstrict` never false-positive.
fn has_in_place_flag(program: &str, args: &[String]) -> bool {
    // (letters that never take a value, letters whose value follows)
    let (transparent, terminators) = if program == "sed" {
        ("nErsuzgpal", "ef")
    } else {
        ("pnlawcstuUWX", "eE")
    };
    for a in args.iter().take_while(|a| a.as_str() != "--") {
        if a == "--in-place" || a.starts_with("--in-place=") {
            return true;
        }
        let Some(cluster) = a.strip_prefix('-') else {
            continue;
        };
        if cluster.starts_with('-') {
            continue;
        }
        for c in cluster.chars() {
            if c == 'i' {
                return true;
            }
            if terminators.contains(c) || !transparent.contains(c) {
                break;
            }
        }
    }
    false
}

/// A token statically identifiable as a filesystem path: absolute or
/// home-anchored. Relative operands stay un-judged here (they may be a
/// script, a suffix, a flag value …) — the per-token deny-glob pass and the
/// workspace anchor already cover the common relative spellings.
fn is_explicit_path(arg: &str) -> bool {
    arg.starts_with('/')
        || arg == "~"
        || arg.starts_with("~/")
        || arg == "$HOME"
        || arg == "${HOME}"
        || arg.starts_with("$HOME/")
        || arg.starts_with("${HOME}/")
}

// ── Protocols: each CLI's payload / response dialect ────────────────────────

/// The hook dialect `guard check --protocol <x>` speaks. `Claude` covers
/// Claude Code AND VS Code agent mode (same envelope); OpenCode and Pi are
/// bridged to `Claude` by the generated plugin/extension files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Claude,
    Codex,
    Gemini,
    Cursor,
    Copilot,
    Antigravity,
    Windsurf,
}

impl Protocol {
    pub fn parse(name: &str) -> Option<Protocol> {
        Some(match name {
            "claude" => Protocol::Claude,
            "codex" => Protocol::Codex,
            "gemini" => Protocol::Gemini,
            "cursor" => Protocol::Cursor,
            "copilot" => Protocol::Copilot,
            "antigravity" => Protocol::Antigravity,
            "windsurf" => Protocol::Windsurf,
            _ => return None,
        })
    }

    /// Payload-shape sniffing for hooks installed without `--protocol`
    /// (or by hand). Discriminators per the per-CLI wire formats.
    pub fn detect(payload: &Value) -> Protocol {
        if payload.get("toolCall").is_some() {
            Protocol::Antigravity
        } else if payload.get("tool_info").is_some() || payload.get("agent_action_name").is_some() {
            Protocol::Windsurf
        } else if payload.get("toolArgs").is_some() {
            Protocol::Copilot
        } else if payload.get("command").is_some() && payload.get("tool_name").is_none() {
            Protocol::Cursor
        } else if payload
            .get("turn_id")
            .and_then(Value::as_str)
            .is_some_and(|t| !t.is_empty())
        {
            Protocol::Codex
        } else {
            Protocol::Claude
        }
    }

    /// Parse a payload into (event, cwd-if-given). `None` = a shape we
    /// don't recognize — the caller allows (fail-open for unknown shapes;
    /// blocking on every parse hiccup would wedge the harness).
    pub fn parse_event(&self, payload: &Value) -> Option<(GuardEvent, Option<String>)> {
        let str_at = |v: &Value, key: &str| v.get(key)?.as_str().map(str::to_string);
        match self {
            Protocol::Claude | Protocol::Codex | Protocol::Gemini => {
                let tool = str_at(payload, "tool_name")
                    .or_else(|| str_at(payload, "toolName"))
                    .unwrap_or_default();
                let input = payload
                    .get("tool_input")
                    .or_else(|| payload.get("toolInput"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let cwd = str_at(payload, "cwd");
                Some((classify_tool(&tool, &input), cwd))
            }
            Protocol::Copilot => {
                let tool = str_at(payload, "toolName").unwrap_or_default();
                let input = payload.get("toolArgs").cloned().unwrap_or(Value::Null);
                let cwd = str_at(payload, "cwd");
                Some((classify_tool(&tool, &input), cwd))
            }
            Protocol::Cursor => {
                let cwd = str_at(payload, "cwd");
                // `beforeShellExecution` carries `command`; `beforeReadFile`
                // carries a file path. Route each to its event; anything else
                // is `Other` (allowed). `beforeMCPExecution` has no file/shell
                // surface to judge, so it lands here as `Other` too.
                if let Some(command) = str_at(payload, "command") {
                    Some((GuardEvent::Bash { command }, cwd))
                } else if let Some(path) = path_from_input(payload) {
                    Some((GuardEvent::FileRead { path }, cwd))
                } else {
                    Some((GuardEvent::Other, cwd))
                }
            }
            Protocol::Antigravity => {
                let call = payload.get("toolCall")?;
                let args = call.get("args").cloned().unwrap_or(Value::Null);
                let cwd = args.get("Cwd").and_then(Value::as_str).map(str::to_string);
                if let Some(cmd) = args.get("CommandLine").and_then(Value::as_str) {
                    return Some((
                        GuardEvent::Bash {
                            command: cmd.to_string(),
                        },
                        cwd,
                    ));
                }
                let tool = call.get("name").and_then(Value::as_str).unwrap_or_default();
                Some((classify_tool(tool, &args), cwd))
            }
            Protocol::Windsurf => {
                let info = payload.get("tool_info").cloned().unwrap_or(Value::Null);
                let cwd = info.get("cwd").and_then(Value::as_str).map(str::to_string);
                if let Some(cmd) = info.get("command_line").and_then(Value::as_str) {
                    return Some((
                        GuardEvent::Bash {
                            command: cmd.to_string(),
                        },
                        cwd,
                    ));
                }
                let action = str_at(payload, "agent_action_name").unwrap_or_default();
                if let Some(path) = path_from_input(&info) {
                    let write = action.contains("write");
                    return Some((
                        if write {
                            GuardEvent::FileWrite { path }
                        } else {
                            GuardEvent::FileRead { path }
                        },
                        cwd,
                    ));
                }
                Some((GuardEvent::Other, cwd))
            }
        }
    }

    /// Render the decision in this dialect: (stdout, stderr, exit code).
    pub fn respond(&self, decision: &Decision) -> (Option<String>, Option<String>, i32) {
        let reason = match decision {
            Decision::Allow => None,
            Decision::Deny { reason } => Some(format!("agentstack guard blocked this: {reason}")),
        };
        match self {
            // Codex documents the SAME `hookSpecificOutput` decision envelope
            // Claude uses (stdout JSON, exit 0) as the preferred deny form, so
            // the two share this arm.
            Protocol::Claude | Protocol::Codex => match reason {
                None => (None, None, 0),
                Some(r) => (
                    Some(
                        json!({"hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "permissionDecision": "deny",
                            "permissionDecisionReason": r,
                        }})
                        .to_string(),
                    ),
                    None,
                    0,
                ),
            },
            // Windsurf has no stdout decision channel — deny is exit 2 + stderr,
            // the one form its hook runner reads as a block.
            Protocol::Windsurf => match reason {
                None => (None, None, 0),
                Some(r) => (None, Some(r), 2),
            },
            Protocol::Gemini => match reason {
                None => (None, None, 0),
                Some(r) => (
                    Some(json!({"decision": "deny", "reason": r, "systemMessage": r}).to_string()),
                    None,
                    0,
                ),
            },
            Protocol::Cursor => match reason {
                None => (Some(json!({"permission": "allow"}).to_string()), None, 0),
                // Cursor documents snake_case `user_message`/`agent_message`
                // only — no camelCase duplicates, no `continue` field.
                Some(r) => (
                    Some(
                        json!({
                            "permission": "deny",
                            "user_message": r, "agent_message": r,
                        })
                        .to_string(),
                    ),
                    None,
                    0,
                ),
            },
            Protocol::Copilot => match reason {
                None => (
                    Some(json!({"permissionDecision": "allow"}).to_string()),
                    None,
                    0,
                ),
                // Copilot documents only `permissionDecision` +
                // `permissionDecisionReason`; `continue`/`stopReason` are
                // off-schema.
                Some(r) => (
                    Some(
                        json!({
                            "permissionDecision": "deny",
                            "permissionDecisionReason": r,
                        })
                        .to_string(),
                    ),
                    None,
                    0,
                ),
            },
            // agy ignores exit codes — the JSON body is the only reliable
            // block signal, so always exit 0.
            Protocol::Antigravity => match reason {
                None => (None, None, 0),
                Some(r) => (
                    Some(json!({"decision": "block", "reason": r}).to_string()),
                    None,
                    0,
                ),
            },
        }
    }
}

/// Map (tool name, tool input) to an event. Tool names come from each
/// harness; unknown tools with a path are treated as reads (deny globs still
/// apply — the safe default that can't wedge legitimate tools).
fn classify_tool(tool: &str, input: &Value) -> GuardEvent {
    if let Some(cmd) = input.get("command").and_then(Value::as_str) {
        // Shell-shaped input regardless of the tool's name (Bash,
        // run_shell_command, execute_bash, run_in_terminal …).
        return GuardEvent::Bash {
            command: cmd.to_string(),
        };
    }
    let Some(path) = path_from_input(input) else {
        return GuardEvent::Other;
    };
    const WRITERS: &[&str] = &[
        "Write",
        "Edit",
        "MultiEdit",
        "NotebookEdit",
        "write_file",
        "replace",
        "edit_file",
        "fs_write",
        "create_file",
        "str_replace_editor",
        // VS Code agent mode's in-place edit tools — without these the edits
        // classify as reads, so workspace confinement never runs for them (the
        // deny globs still fire, but out-of-workspace writes would slip through).
        "replace_string_in_file",
        "multi_replace_string_in_file",
        "apply_patch",
    ];
    if WRITERS.iter().any(|w| tool.eq_ignore_ascii_case(w)) {
        GuardEvent::FileWrite { path }
    } else {
        GuardEvent::FileRead { path }
    }
}

fn path_from_input(input: &Value) -> Option<String> {
    for key in [
        "file_path",
        "filePath",
        "path",
        "notebook_path",
        "target_file",
    ] {
        if let Some(p) = input.get(key).and_then(Value::as_str) {
            return Some(p.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentstack_core::manifest::{FsPolicy, Policy};

    fn ctx() -> GuardContext {
        let machine = Policy {
            filesystem: FsPolicy {
                read: vec![],
                write: vec![],
                deny: vec![".env".into(), ".env.local".into(), "id_rsa".into()],
            },
            ..Policy::default()
        };
        GuardContext {
            workspace: PathBuf::from("/work/proj"),
            home: PathBuf::from("/Users/me"),
            tmp: vec![PathBuf::from("/tmp"), PathBuf::from("/private/tmp")],
            allow_roots: vec![PathBuf::from("/Users/me/Documents/GitHub")],
            agentstack_home: PathBuf::from("/Users/me/.agentstack"),
            ruleset: agentstack_policy::compile(&machine, &Policy::default(), &[]),
        }
    }

    fn bash(cmd: &str) -> GuardEvent {
        GuardEvent::Bash {
            command: cmd.into(),
        }
    }

    fn denied(d: Decision) -> bool {
        d.is_deny()
    }

    // ── file tools ──────────────────────────────────────────────────────

    #[test]
    fn env_files_are_unreadable_and_unwritable_anywhere() {
        let c = ctx();
        for ev in [
            GuardEvent::FileRead {
                path: ".env".into(),
            },
            GuardEvent::FileRead {
                path: "sub/dir/.env".into(),
            },
            GuardEvent::FileRead {
                path: "/anywhere/else/.env".into(),
            },
            GuardEvent::FileWrite {
                path: ".env".into(),
            },
            GuardEvent::FileRead {
                path: "/Users/me/.ssh/id_rsa".into(),
            },
        ] {
            assert!(denied(check_event(&c, &ev)), "{ev:?} should be denied");
        }
        // Non-secret files pass.
        assert_eq!(
            check_event(
                &c,
                &GuardEvent::FileRead {
                    path: "src/main.rs".into()
                }
            ),
            Decision::Allow
        );
    }

    /// #23 — a payload can name the same file under two equivalent
    /// spellings (macOS: `cwd` as `/var/...`, `file_path` as
    /// `/private/var/...`). A path-prefixed deny glob must hold no matter
    /// which spelling each field arrived in.
    #[cfg(unix)]
    #[test]
    fn deny_globs_match_across_equivalent_path_spellings() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real-proj");
        std::fs::create_dir_all(real.join("vault")).unwrap();
        std::fs::write(real.join("vault/token.txt"), "secret").unwrap();
        let link = tmp.path().join("link-proj");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let machine = Policy {
            filesystem: FsPolicy {
                read: vec![],
                write: vec![],
                deny: vec!["vault/**".into()],
            },
            ..Policy::default()
        };
        let mk = |workspace: &Path| GuardContext {
            workspace: workspace.to_path_buf(),
            home: PathBuf::from("/Users/me"),
            tmp: vec![],
            allow_roots: vec![],
            agentstack_home: PathBuf::from("/Users/me/.agentstack"),
            ruleset: agentstack_policy::compile(&machine, &Policy::default(), &[]),
        };
        let read = |p: PathBuf| GuardEvent::FileRead {
            path: p.to_string_lossy().into_owned(),
        };
        // Workspace in one spelling, target in the other — both directions —
        // plus the consistent-spelling case that already worked.
        assert!(denied(check_event(
            &mk(&link),
            &read(real.join("vault/token.txt"))
        )));
        assert!(denied(check_event(
            &mk(&real),
            &read(link.join("vault/token.txt"))
        )));
        assert!(denied(check_event(
            &mk(&real),
            &read(real.join("vault/token.txt"))
        )));
        // Files outside the deny glob still pass.
        assert_eq!(
            check_event(&mk(&real), &read(real.join("src/main.rs"))),
            Decision::Allow
        );
    }

    #[test]
    fn writes_are_confined_to_workspace_allow_roots_and_tmp() {
        let c = ctx();
        let allow = |p: &str| {
            assert_eq!(
                check_event(&c, &GuardEvent::FileWrite { path: p.into() }),
                Decision::Allow,
                "{p} should be writable"
            )
        };
        let deny = |p: &str| {
            assert!(
                denied(check_event(&c, &GuardEvent::FileWrite { path: p.into() })),
                "{p} should be blocked"
            )
        };
        allow("src/new.rs"); // relative → workspace
        allow("/work/proj/deep/file.txt");
        allow("/Users/me/Documents/GitHub/other/file.txt"); // allow_root
        allow("/tmp/scratch.txt");
        deny("/Users/me/.zshrc"); // home, outside roots
        deny("/etc/hosts");
        deny("../outside.txt"); // .. escapes the workspace
                                // Reads outside the workspace stay allowed (host mode can't confine
                                // reads without breaking the harness itself — sandbox mode does that).
        assert_eq!(
            check_event(
                &c,
                &GuardEvent::FileRead {
                    path: "/etc/hosts".into()
                }
            ),
            Decision::Allow
        );
    }

    /// P13.1: VS Code agent-mode's in-place edit tools must classify as
    /// WRITES, so workspace confinement runs for them — an edit outside the
    /// workspace is denied, not silently treated as a read.
    #[test]
    fn vscode_edit_tools_are_writes_and_confined_to_the_workspace() {
        let c = ctx();
        for tool in [
            "replace_string_in_file",
            "multi_replace_string_in_file",
            "apply_patch",
        ] {
            let outside = classify_tool(tool, &json!({"file_path": "/Users/me/.zshrc"}));
            assert_eq!(
                outside,
                GuardEvent::FileWrite {
                    path: "/Users/me/.zshrc".into()
                },
                "{tool} must classify as a write"
            );
            assert!(
                denied(check_event(&c, &outside)),
                "{tool} outside the workspace should be denied"
            );
            // Inside the workspace the same edit is allowed.
            let inside = classify_tool(tool, &json!({"file_path": "src/main.rs"}));
            assert_eq!(
                check_event(&c, &inside),
                Decision::Allow,
                "{tool} in-workspace"
            );
        }
    }

    // ── bash: rm ────────────────────────────────────────────────────────

    #[test]
    fn rm_outside_roots_or_of_roots_is_denied() {
        let c = ctx();
        for cmd in [
            "rm -rf /",
            "rm -rf ~",
            "rm -rf $HOME",
            "rm -rf /work/proj", // the workspace root itself
            "rm -rf /Users/me/Desktop",
            "rm ../sibling.txt",
            "sudo rm -rf /etc",
            "find / -name x | xargs rm -rf",
            "rm .env",
        ] {
            assert!(
                denied(check_event(&c, &bash(cmd))),
                "{cmd} should be denied"
            );
        }
        for cmd in [
            "rm -rf target", // inside the workspace
            "rm -rf ./build",
            "rm /tmp/scratch.txt",
            "rm -rf /Users/me/Documents/GitHub/old-project/dist", // allow_root
        ] {
            assert_eq!(check_event(&c, &bash(cmd)), Decision::Allow, "{cmd}");
        }
    }

    // ── bash: git ───────────────────────────────────────────────────────

    #[test]
    fn destructive_git_is_denied_and_safe_git_passes() {
        let c = ctx();
        for cmd in [
            "git reset --hard HEAD~3",
            "git clean -fdx",
            "git checkout .",
            "git checkout -- .",
            "git restore .",
            "git push --force origin main",
            "git push -f",
            "git stash clear",
            "git -C /work/proj reset --hard",
        ] {
            assert!(
                denied(check_event(&c, &bash(cmd))),
                "{cmd} should be denied"
            );
        }
        for cmd in [
            "git status",
            "git reset --soft HEAD~1",
            "git checkout -b feature",
            "git checkout main",
            "git restore --staged .",
            "git push --force-with-lease origin main",
            "git stash pop",
            "git clean -n",
        ] {
            assert_eq!(check_event(&c, &bash(cmd)), Decision::Allow, "{cmd}");
        }
    }

    // ── bash: misc destructive ──────────────────────────────────────────

    #[test]
    fn disk_and_misc_destroyers_are_denied() {
        let c = ctx();
        for cmd in [
            "dd if=/dev/zero of=/dev/disk2",
            "mkfs.ext4 /dev/sda1",
            "diskutil eraseDisk JHFS+ Blank /dev/disk2",
            "shred secrets.txt",
            "find /Users/me -name '*.log' -delete",
            "chmod -R 777 /",
            "chmod -R 777 id_rsa",
            "chown -R me id_rsa",
            "echo x > /Users/me/.zshrc",
            "echo x >.env",
            "cat secret > /dev/sda",
            "mv src/main.rs /Users/me/Desktop/",
            "cp data.txt /etc/",
            "echo KEY=1 >> .env",
            "cat .env",
            "source .env",
            "cp .env /tmp/exfil",
        ] {
            assert!(
                denied(check_event(&c, &bash(cmd))),
                "{cmd} should be denied"
            );
        }
        for cmd in [
            "dd if=in.img of=out.img",
            "find . -name '*.tmp' -delete",
            "chmod -R 755 ./scripts",
            "echo hi > notes.txt",
            "echo hi > /dev/null",
            "cargo build --release",
            "ls -la",
            "mv old.rs new.rs",
        ] {
            assert_eq!(check_event(&c, &bash(cmd)), Decision::Allow, "{cmd}");
        }
    }

    #[test]
    fn segments_and_substitutions_are_each_judged() {
        let c = ctx();
        for cmd in [
            "ls && rm -rf /",
            "true; git reset --hard",
            "echo $(cat .env)",
            "ls | xargs rm -rf",
            "echo `git clean -fd`",
        ] {
            assert!(
                denied(check_event(&c, &bash(cmd))),
                "{cmd} should be denied"
            );
        }
        // Quoted operators are not separators; a quoted ".env"-free command
        // survives its own strings.
        assert_eq!(
            check_event(&c, &bash("echo 'rm -rf / is a bad idea'")),
            Decision::Allow
        );
    }

    // ── bash: write-capable commands (in-place edits, tee, install) ─────

    #[test]
    fn in_place_edits_are_writes_to_their_file_arguments() {
        let c = ctx();
        for cmd in [
            "sed -i '' 's|a|b|' /Users/me/.zshrc", // the live-repro shape
            "sed -i.bak -e 's/a/b/' /etc/hosts",
            "sed --in-place 's/a/b/' /Users/me/.profile",
            "perl -pi -e 's/a/b/' /Users/me/.zshrc",
            "perl -i.orig fix.pl /Users/me/notes.txt", // script skipped, file judged
        ] {
            assert!(
                denied(check_event(&c, &bash(cmd))),
                "{cmd} should be denied"
            );
        }
        for cmd in [
            "sed -i '' 's|a|b|' src/config.toml", // relative → workspace (not static; fail-open)
            "sed -i '' 's|a|b|' /work/proj/Cargo.toml",
            "sed -i '' 's|a|b|' /Users/me/Documents/GitHub/x/README.md", // allow_root
            "sed 's|a|b|' /Users/me/.zshrc",                             // no -i: a read
            "sed -n '/error/p' /var/log/system.log",                     // a read
            "sed -i '' '/debug/d' notes.txt", // address-form script is not a path
            "perl -ne 'print if /x/' /var/log/foo.log", // 'i' in inline code ≠ -i
            "perl -Mstrict -e 'print' /Users/me/data.txt", // 'i' in -M value ≠ -i
        ] {
            assert_eq!(check_event(&c, &bash(cmd)), Decision::Allow, "{cmd}");
        }
    }

    #[test]
    fn tee_and_install_targets_are_writes() {
        let c = ctx();
        for cmd in [
            "cat data.txt | tee /Users/me/.zshrc",
            "echo x | tee -a /etc/profile",
            "install -m 755 tool.sh /usr/local/bin/tool",
            "install -d /Users/me/newdir",
        ] {
            assert!(
                denied(check_event(&c, &bash(cmd))),
                "{cmd} should be denied"
            );
        }
        for cmd in [
            "make 2>&1 | tee build.log",
            "cargo test | tee /tmp/out.txt",
            "echo x | tee /dev/stderr",
            "install tool.sh bin/tool",
        ] {
            assert_eq!(check_event(&c, &bash(cmd)), Decision::Allow, "{cmd}");
        }
    }

    /// The guard's own config/state dir is never shell-writable, even when
    /// `allow_roots` covers it — otherwise a shell write could widen
    /// allow_roots (or flip `enabled = false`) and then write anywhere.
    #[test]
    fn guard_own_config_is_never_shell_writable() {
        let mut c = ctx();
        c.allow_roots = vec![PathBuf::from("/Users/me")]; // home allowlisted!
        for cmd in [
            "sed -i '' 's|true|false|' /Users/me/.agentstack/agentstack.toml",
            "echo '[guard]' > /Users/me/.agentstack/agentstack.toml",
            "echo 'allow_roots = [\"/\"]' >> ~/.agentstack/agentstack.toml",
            "rm -rf /Users/me/.agentstack",
            "cp evil.toml /Users/me/.agentstack/agentstack.toml",
            "mv /Users/me/.agentstack/agentstack.toml /tmp/x",
            "tee ~/.agentstack/guard/agentstack-guard-cursor.sh", // hook wrappers too
        ] {
            assert!(
                denied(check_event(&c, &bash(cmd))),
                "{cmd} should be denied"
            );
        }
        // The special case is shell-only and write-only: reads pass, other
        // home writes pass (allow_roots covers home here), and file-tool
        // writes stay governed by the ordinary boundary — harnesses show
        // those diffs to the user, and agents legitimately edit manifests.
        assert_eq!(
            check_event(&c, &bash("cat /Users/me/.agentstack/agentstack.toml")),
            Decision::Allow
        );
        assert_eq!(
            check_event(&c, &bash("sed -i '' 's|a|b|' /Users/me/notes.txt")),
            Decision::Allow
        );
        assert_eq!(
            check_event(
                &c,
                &GuardEvent::FileWrite {
                    path: "/Users/me/.agentstack/agentstack.toml".into()
                }
            ),
            Decision::Allow
        );
    }

    // ── protocols ───────────────────────────────────────────────────────

    #[test]
    fn protocol_detection_and_parsing_cover_each_dialect() {
        let claude = json!({"tool_name": "Bash", "tool_input": {"command": "ls"}, "cwd": "/w"});
        assert_eq!(Protocol::detect(&claude), Protocol::Claude);
        let (ev, cwd) = Protocol::Claude.parse_event(&claude).unwrap();
        assert_eq!(
            ev,
            GuardEvent::Bash {
                command: "ls".into()
            }
        );
        assert_eq!(cwd.as_deref(), Some("/w"));

        let codex = json!({"tool_name": "shell", "tool_input": {"command": "ls"}, "turn_id": "t1"});
        assert_eq!(Protocol::detect(&codex), Protocol::Codex);

        let cursor = json!({"command": "rm -rf /", "cwd": "/w"});
        assert_eq!(Protocol::detect(&cursor), Protocol::Cursor);
        let (ev, _) = Protocol::Cursor.parse_event(&cursor).unwrap();
        assert!(matches!(ev, GuardEvent::Bash { .. }));

        let agy = json!({"toolCall": {"name": "run_command",
            "args": {"CommandLine": "ls", "Cwd": "/w"}}, "conversationId": "c"});
        assert_eq!(Protocol::detect(&agy), Protocol::Antigravity);
        let (ev, cwd) = Protocol::Antigravity.parse_event(&agy).unwrap();
        assert_eq!(
            ev,
            GuardEvent::Bash {
                command: "ls".into()
            }
        );
        assert_eq!(cwd.as_deref(), Some("/w"));

        let windsurf = json!({"agent_action_name": "pre_run_command",
            "tool_info": {"command_line": "ls", "cwd": "/w"}});
        assert_eq!(Protocol::detect(&windsurf), Protocol::Windsurf);

        let copilot = json!({"toolName": "bash", "toolArgs": {"command": "ls"}, "cwd": "/w"});
        assert_eq!(Protocol::detect(&copilot), Protocol::Copilot);

        let write = json!({"tool_name": "Write",
            "tool_input": {"file_path": "/x/.env", "content": ""}});
        let (ev, _) = Protocol::Claude.parse_event(&write).unwrap();
        assert_eq!(
            ev,
            GuardEvent::FileWrite {
                path: "/x/.env".into()
            }
        );
    }

    #[test]
    fn responses_match_each_harness_block_contract() {
        let deny = Decision::deny("nope");
        // Claude AND Codex: stdout `hookSpecificOutput` deny envelope, exit 0.
        for p in [Protocol::Claude, Protocol::Codex] {
            let (out, err, code) = p.respond(&deny);
            assert!(out.unwrap().contains("\"permissionDecision\":\"deny\""));
            assert_eq!((err, code), (None, 0));
        }
        // Windsurf: stderr + exit 2, stdout EMPTY (no stdout decision channel).
        let (out, err, code) = Protocol::Windsurf.respond(&deny);
        assert_eq!(out, None);
        assert!(err.unwrap().contains("nope"));
        assert_eq!(code, 2);
        // Gemini: flat decision JSON.
        let (out, _, code) = Protocol::Gemini.respond(&deny);
        assert!(out.unwrap().contains("\"decision\":\"deny\""));
        assert_eq!(code, 0);
        // Antigravity: block JSON, ALWAYS exit 0.
        let (out, _, code) = Protocol::Antigravity.respond(&deny);
        assert!(out.unwrap().contains("\"decision\":\"block\""));
        assert_eq!(code, 0);
        // Cursor and Copilot emit explicit allow bodies.
        let (out, _, _) = Protocol::Cursor.respond(&Decision::Allow);
        assert!(out.unwrap().contains("\"permission\":\"allow\""));
        let (out, _, _) = Protocol::Copilot.respond(&Decision::Allow);
        assert!(out.unwrap().contains("\"permissionDecision\":\"allow\""));
        // Allow on claude-family: silent success.
        assert_eq!(Protocol::Claude.respond(&Decision::Allow), (None, None, 0));
    }
}
