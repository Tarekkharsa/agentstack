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
    /// A write-shaped call whose targets are carried by the PAYLOAD rather
    /// than a path field — Codex's `apply_patch`, whose one argument is a
    /// patch envelope naming every file it adds, updates, deletes or moves.
    /// Every path takes the identical [`write_target_check`] a `FileWrite`
    /// takes, and the first refusal refuses the whole call: a patch applies
    /// as a unit, so allowing the confined half of it is not a decision the
    /// guard can make.
    ///
    /// An EMPTY list is a refusal by construction, not an oversight — see
    /// [`check_write_set`]. A known writer whose target the guard could not
    /// determine cannot be confined to the workspace, and the guard says so
    /// rather than allowing it.
    FileWrites { paths: Vec<String> },
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
        GuardEvent::FileRead { path } => deny_glob_check(ctx, Access::Read, path),
        GuardEvent::FileWrite { path } => write_target_check(ctx, path),
        GuardEvent::FileWrites { paths } => check_write_set(ctx, paths),
        GuardEvent::Bash { command } => check_bash(ctx, command),
    }
}

/// What a denial STOPPED — wording only, never the decision. One deny-glob
/// blocklist governs reads, writes and shell mentions alike, so every arm below
/// reaches the identical `fs_deny_decision`; they differ solely in what the
/// refusal says happened, because "nothing was written" is a false statement
/// about a read that was never going to write anything.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Access {
    /// A read-shaped file tool.
    Read,
    /// A write-shaped file tool, or a write reached through the shell.
    Write,
    /// A shell command that NAMES a refused path. The guard judges tokens, not
    /// intent, so it says what it actually knows: the command did not run.
    Command,
}

impl Access {
    /// (what the denial stopped, what therefore did not happen).
    fn phrasing(self) -> (&'static str, &'static str) {
        match self {
            Access::Read => ("a read of", "nothing was read"),
            Access::Write => ("a write to", "nothing was written"),
            Access::Command => ("a command naming", "the command did not run"),
        }
    }
}

/// `[policy.filesystem] deny` for one path, matched in every spelling we
/// know (absolute, workspace-relative, bare file name) — more spellings can
/// only make a blocklist stricter.
///
/// `access` reaches the message and nothing else: the path spellings, the
/// blocklist and the allow/deny answer are byte-identical for every value of
/// it. A path denied to a write is denied to a read, and the reverse.
fn deny_glob_check(ctx: &GuardContext, access: Access, path: &str) -> Decision {
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
    // The bare file name, but ONLY for a token that could be a file name.
    //
    // This spelling exists so `cat .env` is caught when the deny glob is
    // written `.env` rather than `**/.env`. It is also the one spelling that
    // can fire on text that names no file at all: `check_bash` judges every
    // token, and a quoted argument like a commit message is a single token, so
    // `git commit -m "update docs/.env.local handling"` normalized to a path
    // and offered its last component — `.env.local handling` — which a `.env*`
    // glob happily matched. The command was then refused for containing a
    // sentence.
    //
    // Whitespace is the discriminator, and it costs nothing: a real path
    // CAN contain a space, but such a path still gets its absolute and
    // workspace-relative spellings below, which are the ones that actually
    // identify a file. Only the loosest spelling is withheld, and only from
    // tokens that cannot be bare file names. Deliberately NOT done by
    // skipping `-m`/`--message` operands: a real path must never be able to
    // hide behind a flag.
    if !path.chars().any(char::is_whitespace) {
        if let Some(name) = abs.file_name() {
            spellings.push(name.to_string_lossy().into_owned());
        }
    }
    let refs: Vec<&str> = spellings.iter().map(String::as_str).collect();
    match ctx.ruleset.fs_deny_decision(&refs) {
        Ok(()) => Decision::Allow,
        // Phase 3 seatbelt: the deny-glob branch named the path and the rule
        // but stopped there — the write-scope branch below has taught its
        // exact fix for a while, and this one had no reason not to. The rule
        // text already names its source file, so the next step is where to go
        // and change it.
        Err(rule) => {
            let (stopped, outcome) = access.phrasing();
            Decision::deny(format!(
                "blocked: {stopped} {path} was refused — {rule}\n  \
                 {outcome} · edit that [policy.filesystem] rule if this path should be allowed"
            ))
        }
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
        "blocked: a write to {} was refused — outside the workspace (allowed: the workspace, [guard] allow_roots, temp dirs)\n  \
         nothing was written · to allow writes here, add to {} →\n    [guard]\n    allow_roots = [\"{}\"]",
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

/// Every operation that can modify or delete a path must pass the
/// machine/project deny globs, the `~/.agentstack` absolute deny, and the
/// writable-root boundary. Keeping this as one primitive prevents
/// spelling-specific command handlers from omitting part of the check.
fn write_target_check(ctx: &GuardContext, path: &str) -> Decision {
    if let d @ Decision::Deny { .. } = deny_glob_check(ctx, Access::Write, path) {
        return d;
    }
    if let d @ Decision::Deny { .. } = agentstack_home_check(ctx, path) {
        return d;
    }
    write_scope_check(ctx, path)
}

/// `~/.agentstack` is never writable, `[guard] allow_roots` notwithstanding.
///
/// That directory holds the machine manifest whose `[guard]` table configures
/// this very check (a write there could widen `allow_roots` or flip
/// `enabled = false`), the trust store that records what a human consented to,
/// and the hook wrapper scripts the CLIs execute. An allow_roots that covers
/// it — or a permissive `["/"]` — would let the guard be edited out of the way
/// through the guard.
///
/// Applies to file-tool writes (Write / Edit / `apply_patch`) as well as
/// shell writes. It did not, once: the exemption argued that a harness shows
/// those diffs to the user, so they are consented edits. That holds for
/// manifests in a workspace, and it is exactly wrong for the trust store —
/// "the user saw a diff scroll past" is not the consent ceremony, and the file
/// it forges is the record OF that ceremony. So the exemption survives for
/// everything outside this directory (that is [`write_scope_check`], which
/// still allows `allow_roots`), and stops at its edge.
fn agentstack_home_check(ctx: &GuardContext, path: &str) -> Decision {
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
                    "{} is inside {} — the guard's own config, the trust store, and the \
                     hook scripts; [guard] allow_roots cannot allowlist it (edit it \
                     directly, outside the agent)",
                    abs.display(),
                    ctx.agentstack_home.display()
                ));
            }
        }
    }
    Decision::Allow
}

/// Every target of a multi-target write (a patch envelope), through the SAME
/// [`write_target_check`] a single-path write takes — deny globs and the
/// writable-root boundary, no second spelling of either. The first refusal
/// refuses the whole call.
///
/// The empty case is the fail-closed one: the call was write-shaped (a name on
/// `WRITERS`, or an envelope) but named no target the guard could read. A
/// write whose target is unknown cannot be confined to the workspace, so it is
/// refused rather than allowed as `Other` — the pre-G25 outcome, which let
/// every Codex `apply_patch` through unjudged.
fn check_write_set(ctx: &GuardContext, paths: &[String]) -> Decision {
    if paths.is_empty() {
        return Decision::deny(format!(
            "blocked: a write was refused — the call is write-shaped but names no target \
             the guard could read (no file path, and no parseable '*** Begin Patch' envelope)\n  \
             nothing was written · a write whose target cannot be determined cannot be \
             confined to the workspace · rules: {}",
            machine_manifest(ctx).display()
        ));
    }
    for path in paths {
        if let d @ Decision::Deny { .. } = write_target_check(ctx, path) {
            return d;
        }
    }
    Decision::Allow
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

/// Whether `[guard] allow_roots` opens the whole filesystem — i.e. whether
/// [`write_scope_check`] can still refuse anything at all.
///
/// `/` is the spelling that does it, and it is worth naming rather than
/// inferring: a root that merely sits high (`/Users`) still leaves the rest of
/// the disk confined, so only a root that covers everything counts. Lives here,
/// beside the check it describes, because two reporting surfaces
/// (`doctor`'s machine-policy posture and `guard status`) must answer this
/// question the same way — a second spelling of it could disagree with the
/// first.
pub(crate) fn allow_roots_cover_everything(allow_roots: &[String]) -> bool {
    allow_roots.iter().any(|r| {
        let t = r.trim();
        // `!t.is_empty()` is load-bearing: an EMPTY entry also trims to "",
        // and reading a blank string as "the whole filesystem" would report a
        // misconfigured manifest as deliberately open.
        !t.is_empty() && matches!(t.trim_end_matches('/'), "" | "/**" | "/*")
    })
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

/// Interpreters that write any file from one `-c`/`-e` string argument.
///
/// The tokenizer sees that string as a single opaque token, so none of the
/// path analysis below can read the write out of it — `python3 -c
/// "open(...).write(...)"` reaches the filesystem with no target the guard
/// ever judged. That is tolerable almost everywhere (the guard is cooperative,
/// and an interpreter is a general-purpose tool), and NOT tolerable for one
/// directory: `~/.agentstack` holds the trust store, so a write there forges a
/// human's consent. See [`check_interpreter_consent_store`].
const INTERPRETERS: &[&str] = &["node", "nodejs", "ruby", "deno", "bun", "php"];

/// Shells whose `-c` argument is a whole command line of its own.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "ash"];

/// How deep a `sh -c "sh -c …"` nest is followed before the guard stops
/// unwrapping. Three is far past any real command and keeps the work bounded.
const MAX_SHELL_DEPTH: u8 = 3;

fn check_bash(ctx: &GuardContext, command: &str) -> Decision {
    check_bash_depth(ctx, command, 0)
}

fn check_bash_depth(ctx: &GuardContext, command: &str, depth: u8) -> Decision {
    for segment in split_segments(command) {
        let tokens = tokenize(&segment);
        if tokens.is_empty() {
            continue;
        }
        // Any token naming a deny-globbed path blocks the whole command —
        // this is what catches `cat .env`, `source .env`, `cp .env /tmp`.
        // A token is judged as a NAME, not as a read or a write: `cat .env`
        // and `echo x > .env` both land here, and only one of them would have
        // written anything, so the refusal claims neither.
        for tok in &tokens {
            if !tok.starts_with('-') {
                if let d @ Decision::Deny { .. } = deny_glob_check(ctx, Access::Command, tok) {
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
            if let d @ Decision::Deny { .. } = write_target_check(ctx, &target) {
                return d;
            }
        }
        let (program, rest, via_xargs) = strip_wrappers(tokens);
        let Some(program) = program else { continue };
        // The consent-store checks run BEFORE the per-program table, because
        // they judge programs the table also handles for other reasons
        // (`perl -i`), and because a refusal here is about consent, not about
        // destruction.
        if let d @ Decision::Deny { .. } = check_interpreter_consent_store(ctx, &program, &rest) {
            return d;
        }
        // `sh -c "<command>"`: the inner string is a command line the guard
        // would judge in full if it arrived on its own, and the tokenizer sees
        // it as ONE opaque token. Unwrap it and judge it, so a quoted shell is
        // not a way to say what the guard refuses to hear (bounded by
        // [`MAX_SHELL_DEPTH`]).
        if SHELLS.contains(&program.as_str()) && depth < MAX_SHELL_DEPTH {
            if let Some(script) = shell_script_arg(&rest) {
                if let d @ Decision::Deny { .. } = check_bash_depth(ctx, script, depth + 1) {
                    return d;
                }
            }
        }
        let d = match program.as_str() {
            "agentstack" => check_agentstack(&rest),
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
        if let d @ Decision::Deny { .. } = write_target_check(ctx, &t) {
            return d;
        }
    }
    Decision::Allow
}

/// The script argument of a shell invocation (`-c`, and combined spellings
/// like `-lc`/`-ec`), or `None` when the shell runs a file or a pipe instead.
fn shell_script_arg(args: &[String]) -> Option<&str> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        // Only short flag clusters carry `c`; `--posix` must not match on its
        // letters, and `--` ends the options.
        if a == "--" {
            return None;
        }
        if a.starts_with('-') && !a.starts_with("--") && a.contains('c') {
            return args.get(i + 1).map(String::as_str);
        }
        i += 1;
    }
    None
}

/// Whether this program runs an inline program text (see [`INTERPRETERS`]).
///
/// `perl` counts only with a `-e`-family flag: the guard already judges
/// `perl -i` as an in-place editor with real path operands, and a `perl`
/// invocation with no inline program is that, not this.
fn is_interpreter(program: &str, args: &[String]) -> bool {
    if program.starts_with("python") {
        return true;
    }
    if program == "perl" {
        return args
            .iter()
            .any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('e'));
    }
    INTERPRETERS.contains(&program)
}

/// One directory an interpreter may not name from an agent's shell.
///
/// Deliberately TEXTUAL and coarse: the argument to `-c`/`-e` is a program in
/// another language, and the guard neither parses it nor knows whether the
/// mention is a read or a write. Everywhere else that imprecision would be a
/// bad trade; here it is the right one, because `~/.agentstack` holds the
/// trust store — the record of what a human consented to — and a false
/// negative there forges a human's yes, while a false positive costs one
/// script that has to be run outside the agent.
///
/// It is not a boundary and does not claim to be: an interpreter can build the
/// same path out of pieces this never sees. It closes the spelling anyone
/// actually writes, and `docs/ENFORCEMENT.md` names what remains.
fn check_interpreter_consent_store(ctx: &GuardContext, program: &str, args: &[String]) -> Decision {
    if !is_interpreter(program, args) {
        return Decision::Allow;
    }
    let text = args.join(" ");
    let home = ctx.agentstack_home.to_string_lossy();
    let names_store = text.contains(".agentstack")
        // Every spelling of the env var at once — `$AGENTSTACK_HOME`,
        // `${AGENTSTACK_HOME}`, `os.environ["AGENTSTACK_HOME"]`.
        || text.contains("AGENTSTACK_HOME")
        || (!home.as_ref().is_empty() && text.contains(home.as_ref()));
    if !names_store {
        return Decision::Allow;
    }
    Decision::deny(format!(
        "blocked: a {program} program naming {} was refused — that directory holds the \
         trust store (what a human consented to) and the guard's own config\n  \
         nothing ran · an interpreter's inline program is opaque to this check, so it is \
         refused here rather than judged · edit those files directly, outside the agent",
        ctx.agentstack_home.display()
    ))
}

/// `agentstack` itself, run from an agent's shell.
///
/// Consent is the one thing this CLI must never take from the agent it is
/// governing: `trust`, the `yes` funnel, and the promptless `--yes` forms of
/// `init`/`apply` all end with a grant recorded as though a human had read the
/// review. A hooked agent shell is not that human, and the flag that says "I
/// read it" is one the agent can type as easily as any other. So those
/// invocations are refused at the hook, and everything else `agentstack`
/// does — status, preview, lock, render, undo — stays allowed.
///
/// The verb's READ-ONLY spellings are explicitly allowed, because refusing
/// them would cost an agent the ability to describe the situation without
/// buying anything: `--preview` prints the surface and its digest (and is what
/// an agent should run to hand a human something to review), and `--list`
/// prints which projects are trusted. Neither grants anything. `--revoke` is
/// NOT in that set: it writes the store, and "the agent may take trust away"
/// is a separate decision from this one.
///
/// Cooperative, like the rest of this module: the harness chooses to consult
/// the hook, and a harness that does not gets nothing from this. What it
/// removes is the everyday path — an agent with an ordinary tool loop typing
/// the grant itself.
fn check_agentstack(args: &[String]) -> Decision {
    let Some(verb) = agentstack_verb(args) else {
        return Decision::Allow;
    };
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let refuse = |what: &str| {
        Decision::deny(format!(
            "blocked: `agentstack {what}` grants consent — it was refused\n  \
             nothing was granted · consent is granted at your terminal, not from an agent \
             shell · the agent may prepare the review with `agentstack trust --preview`"
        ))
    };
    match verb {
        "trust" if !has("--preview") && !has("--list") => refuse("trust"),
        "yes" => refuse("yes"),
        "init" if has("--yes") => refuse("init --yes"),
        "apply" if has("--yes") => refuse("apply --yes"),
        _ => Decision::Allow,
    }
}

/// The subcommand of an `agentstack` command line: the first operand, past any
/// flags and past the display-only namespace, which `strip_namespace` removes
/// before clap parses.
///
/// **Both spellings of that namespace, and that is load-bearing.** `x` was the
/// original prefix and `more` is what the product teaches now; `x` is kept as a
/// permanent alias, so both reach the same verb. A guard that skipped only one
/// of them would leave `agentstack <other> yes` as a way for an agent shell to
/// grant consent — the exact thing this check exists to refuse. When a third
/// spelling is ever added, it belongs here on the same line.
fn agentstack_verb(args: &[String]) -> Option<&str> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        // The one global flag that takes a separate value — its operand is not
        // the verb. (`--manifest-dir=DIR` is one token and skips as a flag.)
        if a == "--manifest-dir" {
            i += 2;
            continue;
        }
        if a.starts_with('-') || a == "x" || a == "more" {
            i += 1;
            continue;
        }
        return Some(a);
    }
    None
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
        if let d @ Decision::Deny { .. } = write_target_check(ctx, r) {
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
        if let d @ Decision::Deny { .. } = write_target_check(ctx, t) {
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
        if let Decision::Deny { reason } = write_target_check(ctx, &t) {
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
    if let Decision::Deny { reason } = write_target_check(ctx, &dest[0]) {
        return Decision::deny(format!("{program} destination: {reason}"));
    }
    for s in sources {
        if let d @ Decision::Deny { .. } = deny_glob_check(ctx, Access::Command, s) {
            return d;
        }
        if program == "mv" {
            if let Decision::Deny { reason } = write_target_check(ctx, s) {
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
        if let Decision::Deny { reason } = write_target_check(ctx, f) {
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
                if let Some(command) = command_line_at(payload, "command") {
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
                if let Some(command) = command_line_at(&args, "CommandLine") {
                    return Some((GuardEvent::Bash { command }, cwd));
                }
                let tool = call.get("name").and_then(Value::as_str).unwrap_or_default();
                Some((classify_tool(tool, &args), cwd))
            }
            Protocol::Windsurf => {
                let info = payload.get("tool_info").cloned().unwrap_or(Value::Null);
                let cwd = info.get("cwd").and_then(Value::as_str).map(str::to_string);
                if let Some(command) = command_line_at(&info, "command_line") {
                    return Some((GuardEvent::Bash { command }, cwd));
                }
                let action = str_at(payload, "agent_action_name").unwrap_or_default();
                if let Some(path) = path_from_input(&info) {
                    // The action name is the fast path; the payload shape adds
                    // the edit actions that don't spell "write" (G6).
                    let write = action.contains("write") || payload_is_write(&info);
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

/// Map (tool name, tool input) to an event. Two signals decide, in order: the
/// known-writer NAME list (a fast path and a floor — every name on it is a
/// write, as it always was), then the payload SHAPE, which adds reach for
/// tools this build has never heard of. A path-bearing tool that matches
/// neither is still treated as a read (deny globs apply, workspace
/// confinement does not) — the safe default that can't wedge a harness.
///
/// The PATH-LESS call is where those two signals used to run out, and it is no
/// longer allowed to end in `Other`. A patch envelope carries its targets in
/// its text, and a known writer that names no target at all is refused — see
/// [`GuardEvent::FileWrites`].
fn classify_tool(tool: &str, input: &Value) -> GuardEvent {
    let path = path_from_input(input);
    // The text-editor dialect (`str_replace_based_edit_tool` and its clones)
    // puts an editor VERB in `command` alongside the path it edits. Judge it
    // by the verb: without this it reaches the shell branch below and gets
    // analysed as the harmless command `create`, so the file it names is
    // never write-checked. A real shell payload never carries a path field,
    // so the guard here is exact.
    if let (Some(path), Some(verb)) = (&path, input.get("command").and_then(Value::as_str)) {
        match normalize_key(verb).as_str() {
            "create" | "strreplace" | "insert" | "undoedit" | "append" | "write" => {
                return GuardEvent::FileWrite { path: path.clone() }
            }
            "view" | "read" => return GuardEvent::FileRead { path: path.clone() },
            _ => {}
        }
    }
    // Both branches below are for the PATH-LESS call only. A payload that
    // names its file needs neither: the path field is what the call is about,
    // and reading targets out of a patch that a `Write` is merely storing to
    // disk (`{"file_path": "fix.patch", "content": "*** Begin Patch…"}`) would
    // judge the wrong file.
    if path.is_none() {
        // A patch ENVELOPE is a write whatever the tool is called, and it
        // carries its targets in the text — Codex hands `apply_patch` to the
        // hook as `tool_input: {"command": "*** Begin Patch…"}`, which without
        // this branch reads as a shell command line and confines nothing (G25).
        if let Some(text) = patch_envelope(input) {
            return GuardEvent::FileWrites {
                paths: patch_targets(text),
            };
        }
        // A known writer that named no target is refused, not analysed as a
        // shell line and not allowed as `Other`: nothing on `WRITERS` is a
        // shell tool, so a `command` here is payload, not a command, and a
        // write the guard cannot locate is a write it cannot confine. The
        // empty set IS the refusal.
        if is_known_writer(tool) {
            return GuardEvent::FileWrites { paths: Vec::new() };
        }
    }
    if let Some(command) = command_line_at(input, "command") {
        // Shell-shaped input regardless of the tool's name (Bash,
        // run_shell_command, execute_bash, run_in_terminal …).
        return GuardEvent::Bash { command };
    }
    let Some(path) = path else {
        return GuardEvent::Other;
    };
    if is_known_writer(tool) || payload_is_write(input) {
        GuardEvent::FileWrite { path }
    } else {
        GuardEvent::FileRead { path }
    }
}

/// Tool names that are writes by NAME — the floor under the payload-shape
/// signal, unchanged in content since it lived inside [`classify_tool`]; it is
/// a module item now only because two places consult it.
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

fn is_known_writer(tool: &str) -> bool {
    WRITERS.iter().any(|w| tool.eq_ignore_ascii_case(w))
}

// ── The `apply_patch` envelope ──────────────────────────────────────────────
//
// Codex's `apply_patch` takes ONE argument: patch text in the envelope format
// below, with the target paths inside it. The hook payload is
// `{"tool_name": "apply_patch", "tool_input": {"command": "<patch text>"}}`,
// so no path key exists to read and the guard used to judge the patch as if it
// were a shell line. The format and these exact marker spellings (including
// the trailing space after each colon) are Codex's own parser constants:
// https://github.com/openai/codex/blob/main/codex-rs/apply-patch/src/parser.rs
// (`BEGIN_PATCH_MARKER`, `END_PATCH_MARKER`, `ADD_FILE_MARKER`,
// `DELETE_FILE_MARKER`, `UPDATE_FILE_MARKER`, `MOVE_TO_MARKER`), whose grammar
// requires the first line to be `*** Begin Patch` and the last to be
// `*** End Patch`. Nothing here is guessed.

const PATCH_BEGIN: &str = "*** Begin Patch";
const PATCH_END: &str = "*** End Patch";

/// The directives that NAME a file the patch will write. `*** Move to: ` is on
/// the list because a rename WRITES its destination as surely as an add does;
/// `*** Delete File: ` is a write in the only sense that matters here (the
/// path stops existing).
const PATCH_TARGET_MARKERS: &[&str] = &[
    "*** Add File: ",
    "*** Update File: ",
    "*** Delete File: ",
    "*** Move to: ",
];

/// Bounds. The hook payload is already capped upstream (`MAX_PAYLOAD`); these
/// keep one hostile string from dominating the check anyway. Exceeding the
/// line bound is treated as UNPARSEABLE (an empty target set, i.e. a refusal),
/// never as "no more targets" — truncating a patch would fail open on whatever
/// the tail names.
const MAX_PATCH_LINES: usize = 20_000;
const MAX_PATCH_TARGETS: usize = 500;

/// The patch envelope a payload carries, if any: a string value whose first
/// non-empty line is `*** Begin Patch` and whose last is `*** End Patch`.
///
/// Both ends are required, which is what keeps this branch off the shell path.
/// The begin marker alone would let a payload PREFIX a patch to a real command
/// (`"*** Begin Patch\n…\n*** End Patch\nrm -rf ~"`) and suppress the
/// destructive-command analysis; demanding the envelope be the whole string
/// leaves any such payload on the shell path exactly as before. Values are
/// scanned one level deep (the object's own string fields), because the key
/// differs by harness — Codex sends `command`, the API's function form sends
/// `input` — and neither spelling is worth guessing at.
fn patch_envelope(input: &Value) -> Option<&str> {
    fn envelope(v: &Value) -> Option<&str> {
        v.as_str().filter(|s| is_patch_envelope(s))
    }
    match input {
        Value::String(_) => envelope(input),
        Value::Object(obj) => obj.values().find_map(envelope),
        _ => None,
    }
}

fn is_patch_envelope(text: &str) -> bool {
    let mut lines = text.lines().map(str::trim_end).filter(|l| !l.is_empty());
    let first = lines.next();
    let last = lines.next_back().or(first);
    first == Some(PATCH_BEGIN) && last == Some(PATCH_END)
}

/// Every file path the envelope names. Hostile input is assumed: only lines
/// that START with a marker count (patch CONTENT lines are prefixed with
/// `+`/`-`/space, so a marker quoted inside the new text cannot pose as a
/// directive), duplicates collapse, and an empty result means "no target found"
/// — which [`check_write_set`] refuses.
///
/// Over-matching is the safe direction and is left deliberately possible: an
/// extra path can only ADD a write check, never remove one.
fn patch_targets(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if n >= MAX_PATCH_LINES {
            return Vec::new(); // unparseable → refuse, never a partial answer
        }
        let line = line.trim_end();
        let Some(path) = PATCH_TARGET_MARKERS
            .iter()
            .find_map(|m| line.strip_prefix(m))
            .map(str::trim)
            .filter(|p| !p.is_empty())
        else {
            continue;
        };
        if !out.iter().any(|p| p == path) {
            out.push(path.to_string());
        }
        if out.len() >= MAX_PATCH_TARGETS {
            break;
        }
    }
    out
}

/// Lowercase a payload key or verb and drop `_`/`-`, so one table covers the
/// snake, camel and Pascal spellings the harnesses use (`new_string`,
/// `newStr`, `CodeContent`).
fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|c| *c != '_' && *c != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

/// The shell command a payload carries under `key`, in either shape a harness
/// can send it: a command LINE (a string) or argv (an array of words). Both
/// must reach the same analysis — reading only the string form let an
/// argv-emitting harness skip the whole destructive-command check, because a
/// list payload matched nothing and fell through to `Other`, i.e. allowed
/// (G25).
///
/// Argv is joined into a line and handed to the SAME [`check_bash`] the string
/// form uses, rather than analysed as pre-split tokens: one command path to
/// review, and no vendor schema to be wrong about.
fn command_line_at(value: &Value, key: &str) -> Option<String> {
    match value.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Array(argv) => argv_line(argv),
        _ => None,
    }
}

/// Join argv into a command line. Any string in the array is a word of the
/// command — that is the whole assumption, and it holds for every harness that
/// spells argv as a list. A non-string element carries no word we could
/// reconstruct honestly, so it is dropped rather than guessed; a nested array
/// is flattened, with a depth bound so a pathological payload cannot recurse
/// us off the stack. No strings at all yields `None`: the pre-G25 outcome,
/// unchanged.
fn argv_line(argv: &[Value]) -> Option<String> {
    let mut words = Vec::new();
    collect_argv_words(argv, 0, &mut words);
    (!words.is_empty()).then(|| words.join(" "))
}

fn collect_argv_words(argv: &[Value], depth: usize, out: &mut Vec<String>) {
    if depth > 4 {
        return;
    }
    for item in argv {
        match item {
            Value::String(s) => out.push(shell_quote(s)),
            Value::Array(nested) => collect_argv_words(nested, depth + 1, out),
            _ => {}
        }
    }
}

/// Quote one argv word so the analysis reads it as exactly one token. An argv
/// word is LITERAL — a `;`, `|` or `>` inside one is data, not shell structure
/// — so an unquoted join would invent segments and redirections that the
/// harness never asked for (a commit message could deny its own commit). The
/// POSIX single-quote form is used, which [`split_segments`] and [`tokenize`]
/// both round-trip: they strip quotes without interpreting anything inside.
fn shell_quote(word: &str) -> String {
    let bare = |c: char| c.is_ascii_alphanumeric() || "_-./=:@+,%~".contains(c);
    if !word.is_empty() && word.chars().all(bare) {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', r#"'"'"'"#))
}

/// Does this payload SHAPE intend a write, whatever the tool is called?
///
/// The false-positive budget here is tight: calling a READ a write would deny
/// legitimate out-of-workspace reads and wedge the harness, which is exactly
/// what the read-path default protects against. Three things keep it tight.
/// First, a pre-tool-use hook sees the call's INPUT, never its result — so
/// content in the payload is content the caller is supplying, not a file it
/// is fetching. Second, only fields that carry that supplied content, or an
/// edit structure, or an explicit write mode count; a bare path, an
/// offset/limit window or a recursion flag does not. Third, the weakest
/// signal (a plain body field, which a search tool could plausibly use for
/// its needle) is vetoed whenever the payload also looks like a search.
fn payload_is_write(input: &Value) -> bool {
    let Some(obj) = input.as_object() else {
        return false;
    };
    let keys: Vec<String> = obj.keys().map(|k| normalize_key(k)).collect();
    let has = |name: &str| keys.iter().any(|k| k == name);

    // 1. An edit/patch structure. Unambiguous: nothing reads a file by
    //    handing over the old and new text, a diff, or a list of edits.
    const EDIT_SHAPE: &[&str] = &[
        "oldstring",
        "oldstr",
        "oldtext",
        "newstring",
        "newstr",
        "newtext",
        "newcontent",
        "replacement",
        "replacements",
        "patch",
        "diff",
        "edits",
        "insertline",
    ];
    if EDIT_SHAPE.iter().any(|k| has(k)) {
        return true;
    }
    // 1b. The same signal under a compound name. `replacements` was an exact
    //     match; a harness that calls the identical list `ReplacementChunks`
    //     (Antigravity's `replace_file_content`) or `replacement_chunks` is
    //     supplying replacement content just as plainly, and the guard need
    //     not know what a chunk contains to know that. Matching the
    //     `replacement*` FAMILY (and nothing else by prefix) keeps the
    //     false-positive budget: no read or search argument is named that,
    //     whereas prefixing a mode word like `edit` would swallow
    //     `edit_mode: "view"`.
    if keys.iter().any(|k| k.starts_with("replacement")) {
        return true;
    }

    // 2. An explicit write mode or intent flag.
    const WRITE_VERBS: &[&str] = &[
        "write",
        "append",
        "overwrite",
        "create",
        "edit",
        "insert",
        "strreplace",
        "modify",
        "patch",
    ];
    for (key, value) in obj {
        match normalize_key(key).as_str() {
            "mode" | "operation" | "action" | "editmode" => {
                if value
                    .as_str()
                    .is_some_and(|v| WRITE_VERBS.contains(&normalize_key(v).as_str()))
                {
                    return true;
                }
            }
            "append" | "overwrite" | "create" | "truncate" if value.as_bool() == Some(true) => {
                return true
            }
            _ => {}
        }
    }

    // 3. A body of content for the named file — the weakest signal, so a
    //    search-shaped payload (whose needle may also be called `text`) opts
    //    out rather than risking a denied read.
    const SEARCH_MARKERS: &[&str] = &["pattern", "query", "regex", "search", "outputmode"];
    if SEARCH_MARKERS.iter().any(|k| has(k)) {
        return false;
    }
    // `*content`/`*contents` covers `content`, `Contents`, `CodeContent`,
    // `file_content`; the bare-body names are listed rather than suffixed, so
    // a read tool's `context` argument can never be mistaken for `text`.
    const BODY_KEYS: &[&str] = &["text", "filetext", "sourcetext", "filecontent"];
    obj.iter().any(|(key, value)| {
        let key = normalize_key(key);
        value.is_string()
            && (key.ends_with("content")
                || key.ends_with("contents")
                || BODY_KEYS.contains(&key.as_str()))
    })
}

/// The path a file tool names, in the key spellings the harnesses use. Keys
/// are matched normalized, so one entry covers `file_path`, `filePath` and
/// `FilePath` — the Pascal-case dialects (Windsurf's `TargetFile`) reached no
/// entry at all before, and a payload with no path at all is `Other`, i.e.
/// allowed. Order is priority, not payload order.
fn path_from_input(input: &Value) -> Option<String> {
    let obj = input.as_object()?;
    for want in ["filepath", "path", "notebookpath", "targetfile"] {
        for (key, value) in obj {
            if normalize_key(key) == want {
                if let Some(p) = value.as_str() {
                    return Some(p.to_string());
                }
            }
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

    /// G2: free text that merely MENTIONS a denied name is not a file access.
    ///
    /// `check_bash` judges every token, and a quoted argument is ONE token, so
    /// a commit message was normalized to a path and offered its last
    /// component to the deny globs. `"update docs/.env.local handling"` ends in
    /// `.env.local handling`, which a prefix glob like `.env*` matches — and
    /// the whole commit was refused for containing a sentence.
    ///
    /// The fixture uses a prefix glob on purpose: with only exact names in the
    /// deny list the bug does not reproduce, and a test that cannot reproduce
    /// it cannot hold the fix.
    #[test]
    fn a_denied_name_inside_free_text_is_not_a_file_access() {
        let machine = Policy {
            filesystem: FsPolicy {
                read: vec![],
                write: vec![],
                deny: vec![".env*".into(), "id_rsa".into()],
            },
            ..Policy::default()
        };
        let c = GuardContext {
            ruleset: agentstack_policy::compile(&machine, &Policy::default(), &[]),
            ..ctx()
        };

        // (a) The false positive: a message ABOUT a denied file.
        for cmd in [
            r#"git commit -m "update docs/.env.local handling""#,
            r#"git commit -m "remove .env from the repo""#,
            r#"echo "see .env.local for the token""#,
        ] {
            assert!(
                !denied(check_event(&c, &bash(cmd))),
                "free text must not read as a file access: {cmd}"
            );
        }

        // (b) The thing it must never stop denying: a real access.
        for cmd in [
            "echo secret > .env.local",
            "cp /tmp/x .env",
            "cat .env.local",
            "cat .env",
            "cat sub/dir/.env.local",
            "source ./.env",
        ] {
            assert!(
                denied(check_event(&c, &bash(cmd))),
                "a real access to a denied file must still be refused: {cmd}"
            );
        }

        // (c) A flag operand gets no exemption: the fix keys on whitespace,
        //     never on the preceding flag, so a real path passed to `-m` is
        //     still judged.
        assert!(
            denied(check_event(&c, &bash("git commit -m .env.local"))),
            "a real path must not hide behind a flag"
        );
    }

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

    /// The message must match the EVENT. A blocked READ used to claim
    /// "a write to … was refused / nothing was written" — false twice over:
    /// it was a read, and nothing was ever going to be written.
    #[test]
    fn a_denial_says_what_the_event_actually_was() {
        let c = ctx();
        let reason = |ev: &GuardEvent| match check_event(&c, ev) {
            Decision::Deny { reason } => reason,
            Decision::Allow => panic!("{ev:?} must be denied"),
        };

        let read = reason(&GuardEvent::FileRead {
            path: ".env".into(),
        });
        assert!(read.contains("a read of .env was refused"), "{read}");
        assert!(read.contains("nothing was read"), "{read}");
        assert!(
            !read.contains("was written"),
            "a read writes nothing: {read}"
        );
        assert!(!read.contains("a write to"), "{read}");

        let write = reason(&GuardEvent::FileWrite {
            path: ".env".into(),
        });
        assert!(write.contains("a write to .env was refused"), "{write}");
        assert!(write.contains("nothing was written"), "{write}");

        // A shell token is judged as a NAME, not as a read or a write: `cat`
        // and `>` both land on the same token pass, and only one of them would
        // have written anything — so the refusal claims neither, and states
        // the one thing that is true of both.
        for cmd in ["cat .env", "echo x > .env", "source .env"] {
            let r = reason(&bash(cmd));
            assert!(
                r.contains("a command naming .env was refused"),
                "{cmd}: {r}"
            );
            assert!(r.contains("the command did not run"), "{cmd}: {r}");
            assert!(!r.contains("nothing was written"), "{cmd}: {r}");
        }

        // The write-SCOPE refusal is untouched: that one is always a write.
        let out = reason(&GuardEvent::FileWrite {
            path: "/etc/hosts".into(),
        });
        assert!(out.contains("a write to /etc/hosts was refused"), "{out}");
        assert!(out.contains("nothing was written"), "{out}");
    }

    /// The enforcement control for that wording change: [`Access`] reaches the
    /// message and nothing else. Every path denied to one access is denied to
    /// all three, and no path allowed before became denied — or the reverse.
    #[test]
    fn the_access_wording_never_moves_the_deny_decision() {
        let c = ctx();
        let each = [Access::Read, Access::Write, Access::Command];
        for path in [
            ".env",
            "sub/dir/.env",
            "/anywhere/else/.env",
            ".env.local",
            "/work/proj/.env",
            "/Users/me/.ssh/id_rsa",
        ] {
            for access in each {
                assert!(
                    deny_glob_check(&c, access, path).is_deny(),
                    "{path} must stay denied for {access:?}"
                );
            }
        }
        // The negative control: outside the blocklist, every access still
        // passes — no denial was added anywhere.
        for path in ["src/main.rs", "/etc/hosts", "README.md", ".env.example"] {
            for access in each {
                assert_eq!(
                    deny_glob_check(&c, access, path),
                    Decision::Allow,
                    "{path} must stay allowed for {access:?}"
                );
            }
        }
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

    /// G6: write confinement must not depend on knowing the tool's NAME. A
    /// payload that plainly carries content for the file it names is a write
    /// whatever the harness calls the tool, so an unfamiliar (or renamed)
    /// write tool no longer slips through as a read.
    #[test]
    fn payload_shaped_writes_are_confined_under_unknown_tool_names() {
        let c = ctx();
        let outside = "/Users/me/.zshrc";
        for input in [
            // whole-file content, in each spelling harnesses use
            json!({"file_path": outside, "content": "x"}),
            json!({"path": outside, "contents": "x"}),
            json!({"path": outside, "file_text": "x"}),
            json!({"path": outside, "text": "x"}),
            json!({"filePath": outside, "CodeContent": "x"}),
            json!({"TargetFile": outside, "CodeContent": "x"}),
            // str-replace / patch structures
            json!({"path": outside, "old_string": "a", "new_string": "b"}),
            json!({"path": outside, "oldStr": "a", "newStr": "b"}),
            json!({"path": outside, "patch": "@@ -1 +1 @@"}),
            json!({"path": outside, "edits": [{"a": 1}]}),
            // mode / intent flags
            json!({"path": outside, "mode": "append"}),
            json!({"path": outside, "append": true}),
        ] {
            let ev = classify_tool("conjure_file", &input);
            assert_eq!(
                ev,
                GuardEvent::FileWrite {
                    path: outside.into()
                },
                "{input} must classify as a write"
            );
            assert!(
                denied(check_event(&c, &ev)),
                "{input} outside the workspace should be denied"
            );
            // The same payload inside the workspace stays allowed.
            let mut inside = input.clone();
            for k in ["file_path", "path", "filePath", "TargetFile"] {
                if inside.get(k).is_some() {
                    inside[k] = json!("src/main.rs");
                }
            }
            assert_eq!(
                check_event(&c, &classify_tool("conjure_file", &inside)),
                Decision::Allow,
                "{inside} in-workspace"
            );
        }
    }

    /// The anti-wedge control, stated as a test: a READ under an unknown tool
    /// name must NOT become a write. Classifying a read as a write would deny
    /// legitimate reads outside the workspace and wedge the harness — the
    /// exact failure the read-path default exists to avoid.
    #[test]
    fn payload_shaped_reads_under_unknown_names_stay_reads() {
        let c = ctx();
        let outside = "/Users/me/.zshrc";
        for input in [
            json!({"file_path": outside}),
            json!({"file_path": outside, "offset": 0, "limit": 100}),
            json!({"path": outside, "pattern": "TODO"}),
            // a search whose needle happens to be called `text`
            json!({"path": outside, "query": "TODO", "text": "TODO"}),
            json!({"path": outside, "regex": "^a", "output_mode": "content"}),
            json!({"path": outside, "mode": "read"}),
            json!({"path": outside, "recursive": true}),
        ] {
            let ev = classify_tool("peer_at_file", &input);
            assert_eq!(
                ev,
                GuardEvent::FileRead {
                    path: outside.into()
                },
                "{input} must stay a read"
            );
            assert_eq!(check_event(&c, &ev), Decision::Allow, "{input} allowed");
        }
    }

    /// The name list stays a FLOOR: every `WRITERS` entry keeps classifying as
    /// a write on a bare path payload, exactly as before the payload signal.
    #[test]
    fn every_known_writer_name_still_classifies_as_a_write() {
        for tool in [
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
            "replace_string_in_file",
            "multi_replace_string_in_file",
            "apply_patch",
        ] {
            assert_eq!(
                classify_tool(tool, &json!({"file_path": "/Users/me/.zshrc"})),
                GuardEvent::FileWrite {
                    path: "/Users/me/.zshrc".into()
                },
                "{tool} must still be a write"
            );
        }
    }

    /// A Codex `apply_patch` hook payload, in the shape Codex actually sends
    /// it: `tool_input: {"command": "<patch text>"}`, the patch in the
    /// documented `*** Begin Patch` envelope. The fixture exercises every
    /// directive that names a file.
    fn apply_patch(patch: &str) -> Value {
        json!({ "command": patch })
    }

    const PATCH_IN_WORKSPACE: &str = "\
*** Begin Patch
*** Update File: src/main.rs
@@ fn main() {
-    println!(\"a\");
+    println!(\"b\");
*** Add File: docs/new.md
+hello
*** Delete File: old.txt
*** End Patch
";

    /// G25: the envelope's targets are read out of the patch TEXT and each one
    /// takes the ordinary write check. Inside the workspace the call is
    /// allowed, and the event names exactly the files the patch touches — the
    /// same paths the audit line will carry.
    #[test]
    fn apply_patch_envelope_targets_are_write_checked() {
        let c = ctx();
        let ev = classify_tool("apply_patch", &apply_patch(PATCH_IN_WORKSPACE));
        assert_eq!(
            ev,
            GuardEvent::FileWrites {
                paths: vec!["src/main.rs".into(), "docs/new.md".into(), "old.txt".into(),]
            }
        );
        assert_eq!(check_event(&c, &ev), Decision::Allow);
    }

    /// One target outside the workspace refuses the WHOLE patch, wherever in
    /// the envelope it sits and whichever directive names it — a rename's
    /// destination included, because a move writes where it lands.
    #[test]
    fn apply_patch_target_outside_the_workspace_refuses_the_call() {
        let c = ctx();
        for patch in [
            "*** Begin Patch\n*** Add File: /Users/me/.zshrc\n+evil\n*** End Patch",
            "*** Begin Patch\n*** Update File: ../../Users/me/.zshrc\n@@\n+evil\n*** End Patch",
            "*** Begin Patch\n*** Delete File: /Users/me/.ssh/config\n*** End Patch",
            // a good first target cannot buy the bad second one
            "*** Begin Patch\n*** Add File: src/ok.rs\n+ok\n\
             *** Update File: src/moved.rs\n*** Move to: /etc/cron.d/pwn\n*** End Patch",
        ] {
            let ev = classify_tool("apply_patch", &apply_patch(patch));
            assert!(
                denied(check_event(&c, &ev)),
                "an out-of-workspace patch target must be refused: {patch}"
            );
        }
    }

    /// A deny-globbed path is refused inside the workspace too — the patch
    /// path reaches the same `[policy.filesystem] deny` blocklist every other
    /// write reaches.
    #[test]
    fn apply_patch_target_on_a_deny_glob_refuses_the_call() {
        let c = ctx();
        for patch in [
            "*** Begin Patch\n*** Add File: .env\n+SECRET=1\n*** End Patch",
            "*** Begin Patch\n*** Update File: config/.env.local\n@@\n+X=1\n*** End Patch",
            "*** Begin Patch\n*** Delete File: keys/id_rsa\n*** End Patch",
        ] {
            let ev = classify_tool("apply_patch", &apply_patch(patch));
            // The classification is part of the claim: before G25 a denial
            // here was an accident of the shell tokenizer noticing the name,
            // not the write path judging the patch's target.
            assert!(
                matches!(ev, GuardEvent::FileWrites { .. }),
                "{patch} must classify as a patch write, got {ev:?}"
            );
            let d = check_event(&c, &ev);
            assert!(
                denied(d.clone()),
                "deny-globbed target must be refused: {patch}"
            );
            if let Decision::Deny { reason } = d {
                assert!(reason.contains("policy.filesystem"), "{reason}");
            }
        }
    }

    /// Fail closed: a call the guard KNOWS is a write, whose target it cannot
    /// determine, is refused. Before G25 each of these was allowed outright —
    /// `apply_patch` carries no path key, so the call fell through unjudged.
    #[test]
    fn a_write_whose_target_cannot_be_determined_is_refused() {
        let c = ctx();
        for input in [
            // a well-formed envelope that names no file
            json!({"command": "*** Begin Patch\n*** End Patch"}),
            // a truncated envelope (no end marker) — unparseable, not shell
            json!({"command": "*** Begin Patch\n*** Update File: src/main.rs"}),
            // patch text under a key the guard does not read, and not an
            // envelope either
            json!({"input": "some patch-ish text"}),
            json!({}),
        ] {
            let ev = classify_tool("apply_patch", &input);
            let d = check_event(&c, &ev);
            assert!(denied(d.clone()), "{input} must be refused");
            if let Decision::Deny { reason } = d {
                assert!(
                    reason.contains("names no target"),
                    "the refusal must say the target is unknown: {reason}"
                );
            }
        }
        // The same fail-closed rule covers every name on the writer floor.
        for tool in WRITERS {
            assert!(
                denied(check_event(&c, &classify_tool(tool, &json!({})))),
                "{tool} with no target must be refused"
            );
        }
    }

    /// A patch envelope is judged by SHAPE, so the same call is confined under
    /// the API's `input` key as under Codex's `command`.
    #[test]
    fn a_patch_envelope_is_confined_under_any_key_or_tool_name() {
        let c = ctx();
        let patch = "*** Begin Patch\n*** Add File: /Users/me/.zshrc\n+evil\n*** End Patch";
        for input in [json!({"input": patch}), json!({"patch_text": patch})] {
            assert!(
                denied(check_event(&c, &classify_tool("conjure_patch", &input))),
                "{input} must be confined"
            );
        }
    }

    /// The negative control: nothing outside the writer floor and the envelope
    /// shape changes. Reads stay reads, shells stay shells, path-less
    /// non-writers stay `Other` (allowed), and a shell line that merely
    /// CONTAINS an envelope is still analysed as a command — the envelope must
    /// be the whole argument, or a hostile payload could prefix a patch to a
    /// destructive command and suppress the command analysis.
    #[test]
    fn non_writer_tools_are_unchanged_by_the_envelope_branch() {
        let c = ctx();
        assert_eq!(
            classify_tool("Read", &json!({"file_path": "/etc/hosts"})),
            GuardEvent::FileRead {
                path: "/etc/hosts".into()
            }
        );
        assert_eq!(
            classify_tool("WebFetch", &json!({"url": "https://x"})),
            GuardEvent::Other
        );
        assert_eq!(
            classify_tool("Bash", &json!({"command": "ls -la"})),
            bash("ls -la")
        );
        // A call that NAMES its file is judged by that file, even when the
        // content it stores happens to be a patch: saving `fix.patch` writes
        // `fix.patch`, not whatever the patch inside it mentions.
        let stored = "*** Begin Patch\n*** Add File: /etc/cron.d/pwn\n+evil\n*** End Patch";
        let ev = classify_tool(
            "Write",
            &json!({"file_path": "fix.patch", "content": stored}),
        );
        assert_eq!(
            ev,
            GuardEvent::FileWrite {
                path: "fix.patch".into()
            }
        );
        assert_eq!(check_event(&c, &ev), Decision::Allow);

        let smuggled = "*** Begin Patch\n*** Add File: src/ok.rs\n+ok\n*** End Patch\nrm -rf ~";
        assert_eq!(
            classify_tool("Bash", &json!({"command": smuggled})),
            bash(smuggled),
            "an envelope must not swallow the command around it"
        );
        assert!(denied(check_event(
            &c,
            &classify_tool("Bash", &json!({"command": smuggled}))
        )));
    }

    /// The text-editor dialect (`command` is an editor VERB, not a shell
    /// line). Before G6 these short-circuited to the Bash branch and were
    /// judged as the harmless command `create`/`str_replace`, so workspace
    /// confinement never ran on the file they name.
    #[test]
    fn text_editor_verbs_are_classified_by_verb_not_as_shell() {
        let outside = "/Users/me/.zshrc";
        for verb in ["create", "str_replace", "insert", "undo_edit"] {
            assert_eq!(
                classify_tool(
                    "str_replace_editor",
                    &json!({"command": verb, "path": outside, "file_text": "x"})
                ),
                GuardEvent::FileWrite {
                    path: outside.into()
                },
                "{verb} must be a write"
            );
        }
        assert_eq!(
            classify_tool(
                "str_replace_editor",
                &json!({"command": "view", "path": outside})
            ),
            GuardEvent::FileRead {
                path: outside.into()
            }
        );
        // A real shell payload is untouched: no path field, so it still
        // routes to the command analysis.
        assert_eq!(
            classify_tool("Bash", &json!({"command": "rm -rf /"})),
            bash("rm -rf /")
        );
    }

    /// G25: a harness that emits argv as an ARRAY must reach the same
    /// destructive-command analysis as the string form. Before this, the
    /// `as_str()` read returned `None`, the call fell through to `Other`, and
    /// `rm -rf` outside the workspace was allowed outright.
    #[test]
    fn argv_array_commands_are_judged_like_the_string_form() {
        let c = ctx();
        // Outside the workspace: denied, exactly as the joined string is.
        let outside = json!({"command": ["rm", "-rf", "/Users/me/Desktop"]});
        let ev = classify_tool("shell", &outside);
        assert_eq!(ev, bash("rm -rf /Users/me/Desktop"), "argv must join");
        assert!(
            denied(check_event(&c, &ev)),
            "an argv-array rm outside the workspace must be denied"
        );
        // The same array inside the workspace stays allowed.
        let inside = json!({"command": ["rm", "-rf", "/work/proj/build"]});
        assert_eq!(
            check_event(&c, &classify_tool("shell", &inside)),
            Decision::Allow
        );
        // The rest of the destructive-command surface, reached the same way.
        for argv in [
            json!(["git", "reset", "--hard"]),
            json!(["bash", "-lc", "cat", "/work/proj/.env"]), // a deny glob
            json!(["sudo", "rm", "-rf", "/Users/me"]),
        ] {
            let ev = classify_tool("shell", &json!({ "command": argv }));
            assert!(denied(check_event(&c, &ev)), "{argv} must be denied");
        }
        // A string command is unchanged — same event, same decision.
        assert_eq!(
            classify_tool("Bash", &json!({"command": "rm -rf /Users/me/Desktop"})),
            bash("rm -rf /Users/me/Desktop")
        );
    }

    /// Argv words are LITERAL: joining must not manufacture shell structure a
    /// harness never asked for, or an innocent argument would deny its own
    /// command (the wedge this fix must not introduce).
    #[test]
    fn argv_words_are_quoted_so_data_never_becomes_shell_structure() {
        let c = ctx();
        for argv in [
            json!(["git", "commit", "-m", "fix; rm -rf /Users/me"]),
            json!(["git", "commit", "-m", "wip && rm -rf ~"]),
            json!(["echo", "> /Users/me/.zshrc"]),
            json!(["echo", "it's fine"]),
        ] {
            let ev = classify_tool("shell", &json!({ "command": argv }));
            assert_eq!(check_event(&c, &ev), Decision::Allow, "{argv} is data");
        }
        // …and the real thing is still caught when it IS the command.
        let ev = classify_tool("shell", &json!({"command": ["git", "reset", "--hard"]}));
        assert!(denied(check_event(&c, &ev)));
    }

    /// A malformed argv must degrade, never panic and never wedge: the words
    /// we can read are still judged, and an array with nothing readable in it
    /// behaves exactly as it did before (allowed).
    #[test]
    fn malformed_argv_degrades_without_panicking() {
        let c = ctx();
        // A nested array is flattened; a non-string element is dropped.
        let ev = classify_tool(
            "shell",
            &json!({"command": ["rm", ["-rf"], 7, {"x": 1}, null, "/Users/me/Desktop"]}),
        );
        assert!(denied(check_event(&c, &ev)), "readable words still judged");
        // Nothing readable at all: the pre-G25 outcome, unchanged.
        for input in [
            json!({"command": []}),
            json!({"command": [1, 2, 3]}),
            json!({"command": {"argv": ["rm", "-rf", "/"]}}),
        ] {
            assert_eq!(
                classify_tool("mystery_tool", &input),
                GuardEvent::Other,
                "{input} must not be invented into a command"
            );
        }
    }

    /// The array shape must be handled in every dialect that reads a command
    /// straight off the payload, not just the tool-input one.
    #[test]
    fn every_dialect_reads_argv_arrays_as_commands() {
        let want = bash("rm -rf /Users/me/Desktop");
        let argv = json!(["rm", "-rf", "/Users/me/Desktop"]);
        let cases = [
            (
                Protocol::Claude,
                json!({"tool_name": "shell", "tool_input": {"command": argv}, "cwd": "/w"}),
            ),
            (
                Protocol::Codex,
                json!({"turn_id": "t1", "tool_name": "shell",
                       "tool_input": {"command": argv}, "cwd": "/w"}),
            ),
            (
                Protocol::Copilot,
                json!({"toolName": "bash", "toolArgs": {"command": argv}, "cwd": "/w"}),
            ),
            (Protocol::Cursor, json!({"command": argv, "cwd": "/w"})),
            (
                Protocol::Antigravity,
                json!({"toolCall": {"name": "run_command",
                       "args": {"CommandLine": argv, "Cwd": "/w"}}}),
            ),
            (
                Protocol::Windsurf,
                json!({"agent_action_name": "pre_run_command",
                       "tool_info": {"command_line": argv, "cwd": "/w"}}),
            ),
        ];
        for (protocol, payload) in cases {
            let (ev, cwd) = protocol.parse_event(&payload).unwrap();
            assert_eq!(ev, want, "{protocol:?} must read argv as a command");
            assert_eq!(cwd.as_deref(), Some("/w"), "{protocol:?} cwd");
        }
    }

    /// An array of edit chunks is supplied content, so the file it names is a
    /// write — whatever the harness calls the tool or the field. This is the
    /// `replace_file_content` hole: a `ReplacementChunks` array matched
    /// neither the name list nor the body-key rule (which wants a string).
    #[test]
    fn replacement_chunk_arrays_are_writes() {
        let c = ctx();
        let outside = "/Users/me/.zshrc";
        for input in [
            json!({"TargetFile": outside, "ReplacementChunks": [{"TargetContent": "a"}]}),
            json!({"path": outside, "replacement_chunks": [{"old": "a", "new": "b"}]}),
        ] {
            let ev = classify_tool("replace_file_content", &input);
            assert_eq!(
                ev,
                GuardEvent::FileWrite {
                    path: outside.into()
                },
                "{input} must classify as a write"
            );
            assert!(
                denied(check_event(&c, &ev)),
                "{input} outside the workspace"
            );
        }
        // Inside the workspace it stays allowed — reach added, not denial.
        assert_eq!(
            check_event(
                &c,
                &classify_tool(
                    "replace_file_content",
                    &json!({"TargetFile": "src/main.rs", "ReplacementChunks": [{"a": 1}]})
                )
            ),
            Decision::Allow
        );
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

    /// The guard's own config/state dir is never writable — by a shell OR by a
    /// file tool — even when `allow_roots` covers it. Otherwise a write could
    /// widen allow_roots (or flip `enabled = false`) and then write anywhere,
    /// or rewrite `trust.json` and forge a consent nobody gave.
    #[test]
    fn guard_own_config_is_never_writable() {
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
        // H3: a file tool reaches the identical deny. It did not before — the
        // exemption argued that a harness shows Write/Edit diffs to the user,
        // which is true of a manifest in a workspace and worthless for the
        // file that RECORDS a consent ceremony.
        for path in [
            "/Users/me/.agentstack/agentstack.toml",
            "/Users/me/.agentstack/trust.json",
            "~/.agentstack/trust.json",
        ] {
            assert!(
                denied(check_event(
                    &c,
                    &GuardEvent::FileWrite { path: path.into() }
                )),
                "FileWrite {path} should be denied"
            );
            assert!(
                denied(check_event(
                    &c,
                    &GuardEvent::FileWrites {
                        paths: vec![path.into()]
                    }
                )),
                "FileWrites {path} should be denied"
            );
        }
        // The special case is write-only and bounded to that one directory:
        // reads pass, and other home writes pass (allow_roots covers home
        // here) exactly as before.
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
                    path: "/Users/me/notes.txt".into()
                }
            ),
            Decision::Allow
        );
        assert_eq!(
            check_event(
                &c,
                &GuardEvent::FileRead {
                    path: "/Users/me/.agentstack/trust.json".into()
                }
            ),
            Decision::Allow
        );
    }

    // ── consent (H1–H2) ─────────────────────────────────────────────────

    /// RED TEAM — an agent's own shell cannot type the grant.
    ///
    /// `--yes --consented <digest>` is the fully-formed non-interactive grant:
    /// it satisfies every check inside the CLI, because inside the CLI it is
    /// indistinguishable from a human who previewed and pasted the digest
    /// back. The hook is where that difference still exists, so the refusal
    /// lives here. Reverting the `agentstack` arm allows every line below.
    #[test]
    fn an_agent_shell_cannot_grant_consent() {
        let c = ctx();
        for cmd in [
            "agentstack trust .",
            "agentstack trust . --yes --consented deadbeef",
            "agentstack trust --yes --consented deadbeef",
            "agentstack yes",
            "agentstack yes --yes",
            "agentstack init --yes",
            "agentstack apply --write --yes",
            // Spelled with a path, under a wrapper, behind the display-only
            // namespace in BOTH its spellings, after a global flag with a
            // value, inside a pipeline, and inside a quoted `sh -c` — the
            // effective program is the same one every way. `x` is a permanent
            // alias of `more`, so a guard that refused only the new spelling
            // would leave the old one open.
            "/usr/local/bin/agentstack trust . --yes --consented d",
            "sudo agentstack trust .",
            "agentstack more yes",
            "agentstack x yes",
            "agentstack --manifest-dir /work/proj trust .",
            "echo hi && agentstack trust . --yes --consented d",
            "sh -c 'agentstack trust . --yes --consented d'",
            "bash -lc \"agentstack yes --yes\"",
        ] {
            assert!(
                denied(check_event(&c, &bash(cmd))),
                "{cmd} should be denied"
            );
        }
        // Everything else the CLI does stays allowed — including the read half
        // of the trust verb, which is exactly what an agent SHOULD run to hand
        // a human something to review.
        for cmd in [
            "agentstack trust --preview",
            "agentstack trust --preview --json",
            // Read-only, and it grants nothing: an agent may still say which
            // projects are trusted.
            "agentstack trust --list",
            "agentstack status",
            "agentstack lock --write",
            "agentstack apply --write",
            "agentstack init --plan",
            "agentstack use dev --write",
            "sh -c 'agentstack trust --preview'",
        ] {
            assert_eq!(check_event(&c, &bash(cmd)), Decision::Allow, "{cmd}");
        }
    }

    /// RED TEAM — an interpreter is not a way to reach the trust store.
    ///
    /// The inline program is one opaque token to every path check in this
    /// module, so the mention of the directory is the whole signal. Coarse on
    /// purpose: this is the one directory where a false negative forges a
    /// human's consent.
    #[test]
    fn interpreters_may_not_name_the_consent_store() {
        let c = ctx();
        for cmd in [
            r#"python3 -c "open('/Users/me/.agentstack/trust.json','w').write('{}')""#,
            r#"python -c "import os,json; json.dump({}, open(os.environ['AGENTSTACK_HOME']+'/trust.json','w'))""#,
            r#"python3.12 -c "print(open('~/.agentstack/trust.json').read())""#,
            r#"node -e "require('fs').writeFileSync(process.env.HOME+'/.agentstack/trust.json','{}')""#,
            r#"ruby -e "File.write('/Users/me/.agentstack/trust.json','{}')""#,
            r#"deno eval "Deno.writeTextFileSync('$AGENTSTACK_HOME/trust.json','{}')""#,
            r#"bun -e "Bun.write('/Users/me/.agentstack/trust.json','{}')""#,
            r#"php -r "file_put_contents('/Users/me/.agentstack/trust.json','{}');""#,
            r#"perl -e "open(F,'>','/Users/me/.agentstack/trust.json')""#,
            // Under a wrapper, and inside a quoted shell.
            r#"env python3 -c "open('/Users/me/.agentstack/trust.json','w')""#,
            r#"sh -c "python3 -c \"open('/Users/me/.agentstack/trust.json','w')\"""#,
        ] {
            assert!(
                denied(check_event(&c, &bash(cmd))),
                "{cmd} should be denied"
            );
        }
        // Ordinary interpreter work is untouched — the check is one directory
        // wide, and `perl` with no inline program is still just the in-place
        // editor the table already judged.
        for cmd in [
            r#"python3 -c "open('/work/proj/out.txt','w').write('hi')""#,
            "python3 script.py --out /work/proj/build",
            r#"node -e "console.log(1+1)""#,
            "perl -i -pe 's/a/b/' /work/proj/notes.txt",
        ] {
            assert_eq!(check_event(&c, &bash(cmd)), Decision::Allow, "{cmd}");
        }
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
