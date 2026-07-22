//! Static manifest validation: profile references resolve, servers are
//! internally consistent for their transport.

use std::path::Path;

use super::model::{Manifest, ServerType};
use crate::library::Library;
use crate::resolve::{
    resolve_server, resolve_skill, ResolveError, ResolveMode, ServerResolveError,
};
use crate::store::Store;

/// Context enabling library-aware skill-ref validation. Without it, a profile
/// skill ref must be defined inline (`[skills.*]`) to validate; with it, a ref
/// may also resolve from the central library. Callers that have not yet been
/// wired for the library pass no context and keep today's inline-only behavior.
pub struct ValidateCtx<'a> {
    pub manifest_dir: &'a Path,
    pub library: &'a Library,
    pub lib_home: &'a Path,
    pub store: &'a Store,
}

/// A single validation problem. Carries a stable kind for testing plus a
/// human-readable message for `doctor`/CLI output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub kind: IssueKind,
    pub message: String,
    /// A copy-pasteable repair command, where one is derivable from the issue
    /// alone. Printers append it in the `↳ fix` voice doctor established, and
    /// doctor's closing `start with:` line reads it for triage.
    pub fix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueKind {
    UnknownServerRef,
    /// A server ref names a known entry, but resolving its definition failed
    /// (e.g. a library entry whose `servers/<name>.toml` is missing/malformed).
    /// Distinct from `UnknownServerRef`, which means the name resolves nowhere.
    UnresolvableServerRef,
    UnknownSkillRef,
    /// A skill ref names a known entry, but resolving its source failed (e.g. a
    /// library entry with a broken/missing source). Distinct from
    /// `UnknownSkillRef`, which means the name resolves nowhere at all.
    UnresolvableSkillRef,
    UnknownHookRef,
    MissingTransportFields,
    UnknownTargetServer,
    /// A `[servers.X] targets` entry names an adapter id that isn't registered
    /// — the server would silently render nowhere the author expected.
    UnknownServerTarget,
    /// A `[servers.X.extra.<id>]` table names an adapter id that isn't
    /// registered — the extras would silently never render.
    UnknownExtraTarget,
    /// A `[servers.X] owner` names an adapter id that isn't registered — the
    /// refresh-from-disk would silently never happen and the stale manifest
    /// values would fan out (the exact downgrade `owner` exists to prevent).
    UnknownServerOwner,
    /// An `[instructions.X] targets` entry names an adapter id that isn't
    /// registered — the fragment would silently compile into no harness the
    /// author expected (the instruction analogue of `UnknownServerTarget`).
    UnknownInstructionTarget,
    /// A `[policy.egress]` pattern the grammar cannot interpret (bad bracket
    /// form or invalid `:port` suffix). At run time such a pattern fails the
    /// decision CLOSED, so this is caught here first — at authoring time.
    MalformedEgressPattern,
    /// An `[extensions.X] target` names no registered adapter (or the
    /// wildcard) — extension code is harness-specific, so it must name
    /// exactly one real adapter id.
    UnknownExtensionTarget,
    /// An `[extensions.X]` source that can't be pinned: a git source without
    /// the required `subpath`, or no source at all (and not resolvable from
    /// the central library). Blocked here so an unpinnable extension can
    /// never exist half-declared.
    InvalidExtensionSource,
    /// An extension name colliding with the host guard's reserved artifact
    /// names (`agentstack-guard*`) — the guard's files must never be
    /// squattable by repo content.
    ReservedExtensionName,
    /// An extension name that is not a plain path component (contains `/`, `\`,
    /// `..`, or is empty/absolute). The name becomes the rendered artifact's
    /// basename, so a name carrying a separator would escape the extension
    /// directory agentstack owns — rejected before it can ever be rendered.
    InvalidExtensionName,
    /// A workflow name that is not a plain path component (contains `/`, `\`,
    /// `..`, or is empty/absolute) — the same containment rule as
    /// `InvalidExtensionName`: the name is an artifact/run identity, never a
    /// path escape.
    InvalidWorkflowName,
    /// A `[workflows.X]` source that can't be pinned: a git source without the
    /// required `subpath`, or no source at all. Workflow sources are
    /// inline-only in W1 — a sourceless entry is NOT a library reference (the
    /// central-library workflow kind is W4), so it is always an error.
    InvalidWorkflowSource,
    /// A `[workflows.X] roles` entry names no `[profiles.*]` table. Roles are
    /// the workflow's whole authority-request surface; a role that resolves
    /// to no profile could never be admitted, so it fails at authoring time.
    UnknownWorkflowRole,
    /// `[workflows.X]` ceilings outside the sane envelope: more than 32 unique
    /// roles, `max_agents` outside 1..=1000, or `max_wall_seconds` outside
    /// 1..=604800 (one week). Zero is refused rather than read as "unlimited"
    /// — a zero ceiling is always a typo, and misreading it open would be a
    /// widening (rule 2).
    InvalidWorkflowBounds,
}

impl IssueKind {
    /// Structural errors that would render broken/partial config — these block
    /// `--write`. (All current kinds are errors; kept as a method so future
    /// warning-only kinds can return `false`.)
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            IssueKind::UnknownServerRef
                | IssueKind::UnresolvableServerRef
                | IssueKind::UnknownSkillRef
                | IssueKind::UnresolvableSkillRef
                | IssueKind::UnknownHookRef
                | IssueKind::MissingTransportFields
                | IssueKind::UnknownTargetServer
                | IssueKind::UnknownServerTarget
                | IssueKind::UnknownExtraTarget
                | IssueKind::UnknownServerOwner
                | IssueKind::UnknownInstructionTarget
                | IssueKind::MalformedEgressPattern
                | IssueKind::UnknownExtensionTarget
                | IssueKind::InvalidExtensionSource
                | IssueKind::ReservedExtensionName
                | IssueKind::InvalidExtensionName
                | IssueKind::InvalidWorkflowName
                | IssueKind::InvalidWorkflowSource
                | IssueKind::UnknownWorkflowRole
                | IssueKind::InvalidWorkflowBounds
        )
    }
}

/// `[workflows.X]` bounds envelope (D7 W1): the maxima requests are validated
/// against. Public so the admission choke point (`crate::workflows`) and
/// doctor state the same numbers.
pub const MAX_WORKFLOW_ROLES: usize = 32;
pub const MAX_WORKFLOW_AGENTS: u32 = 1000;
pub const MAX_WORKFLOW_WALL_SECONDS: u64 = 604_800; // one week

/// An extension or workflow name is used verbatim as an artifact basename /
/// run identity, so it must be a single plain path component — no separator,
/// no `..`, not empty or absolute. Mirrors the render sink's
/// `is_safe_artifact_key` so validation and the renderer agree on exactly
/// which names are containable.
fn is_safe_component_name(name: &str) -> bool {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return false;
    }
    let mut comps = Path::new(name).components();
    matches!(
        (comps.next(), comps.next()),
        (Some(std::path::Component::Normal(c)), None) if c == std::ffi::OsStr::new(name)
    )
}

impl Issue {
    fn new(kind: IssueKind, message: impl Into<String>) -> Self {
        Issue {
            kind,
            message: message.into(),
            fix: None,
        }
    }

    fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
}

/// Validate a manifest, returning every issue found (does not short-circuit).
pub fn validate(manifest: &Manifest) -> Vec<Issue> {
    validate_with_targets(manifest, std::iter::empty::<&str>())
}

/// Validate a manifest with a known adapter id set. Passing no target ids keeps
/// validation independent of the local adapter registry and skips target-id
/// checks.
pub fn validate_with_targets<'a>(
    manifest: &Manifest,
    targets: impl IntoIterator<Item = &'a str>,
) -> Vec<Issue> {
    run(manifest, targets, None)
}

/// Validate with library-aware skill resolution: a profile skill ref validates
/// if it is defined inline **or** resolves from the central library.
pub fn validate_with_context<'a>(
    manifest: &Manifest,
    targets: impl IntoIterator<Item = &'a str>,
    ctx: &ValidateCtx,
) -> Vec<Issue> {
    run(manifest, targets, Some(ctx))
}

fn run<'a>(
    manifest: &Manifest,
    targets: impl IntoIterator<Item = &'a str>,
    ctx: Option<&ValidateCtx>,
) -> Vec<Issue> {
    let mut issues = Vec::new();
    let targets: std::collections::BTreeSet<String> =
        targets.into_iter().map(str::to_string).collect();

    // [policy.egress] pattern grammar: a malformed pattern fails every
    // decision that consults it closed at run time — reject it here so the
    // fix happens at authoring time, with the exact pattern named.
    for (server, patterns) in &manifest.policy.egress {
        for pattern in patterns {
            if agentstack_core::manifest::egress_pattern_is_malformed(pattern) {
                issues.push(Issue::new(
                    IssueKind::MalformedEgressPattern,
                    format!(
                        "[policy.egress] {server} pattern \"{pattern}\" is malformed \
                         (expected `host`, `host:port`, `host:*`, or a bracketed IPv6 form)"
                    ),
                ));
            }
        }
    }

    // Server transport consistency.
    for (name, server) in &manifest.servers {
        match server.server_type {
            ServerType::Http => {
                if server.url.is_none() {
                    issues.push(
                        Issue::new(
                            IssueKind::MissingTransportFields,
                            format!("server '{name}' is type=http but has no `url`"),
                        )
                        .with_fix(format!("agentstack set server {name} --url <URL> --write")),
                    );
                }
            }
            ServerType::Stdio => {
                if server.command.is_none() {
                    issues.push(
                        Issue::new(
                            IssueKind::MissingTransportFields,
                            format!("server '{name}' is type=stdio but has no `command`"),
                        )
                        .with_fix(format!(
                            "agentstack set server {name} --command \"<CMD>\" --write"
                        )),
                    );
                }
            }
        }
        // Extras keyed by an unregistered adapter id would silently never
        // render — a typo like `extra.codx` must not eat the keys it guards.
        if !targets.is_empty() {
            for target in server.extra.keys() {
                if !targets.contains(target) {
                    issues.push(Issue::new(
                        IssueKind::UnknownExtraTarget,
                        format!("server '{name}' has `extra.{target}` but no adapter '{target}' is registered"),
                    ));
                }
            }
            // Same for the fan-out scoping itself: a typo'd id in `targets`
            // would silently render the server nowhere the author expected.
            // (An explicit empty list is deliberate — recipe-owned servers.)
            for target in &server.targets {
                if target != "*" && !targets.contains(target) {
                    issues.push(Issue::new(
                        IssueKind::UnknownServerTarget,
                        format!("server '{name}' references unknown target '{target}'"),
                    ));
                }
            }
            // An owner that resolves to no adapter means the refresh-from-disk
            // silently never runs — stale values would fan out again.
            if let Some(owner) = &server.owner {
                if !targets.contains(owner) {
                    issues.push(Issue::new(
                        IssueKind::UnknownServerOwner,
                        format!("server '{name}' has `owner = \"{owner}\"` but no adapter '{owner}' is registered"),
                    ));
                }
            }
        }
    }

    // Profile references.
    for (pname, profile) in &manifest.profiles {
        for sref in &profile.servers {
            // Inline definitions validate directly; only non-inline names consult
            // the central library, and only when ctx is given.
            if manifest.servers.contains_key(sref) {
                continue;
            }
            match ctx {
                Some(cx) => match resolve_server(manifest, cx.library, cx.lib_home, sref) {
                    Ok(_) => {}
                    Err(ServerResolveError::Unresolved { .. }) => issues.push(Issue::new(
                        IssueKind::UnknownServerRef,
                        format!("profile '{pname}' references unknown server '{sref}'"),
                    )),
                    Err(ServerResolveError::Source(e)) => issues.push(Issue::new(
                        IssueKind::UnresolvableServerRef,
                        format!("profile '{pname}' server '{sref}' failed to resolve: {e}"),
                    )),
                },
                None => issues.push(Issue::new(
                    IssueKind::UnknownServerRef,
                    format!("profile '{pname}' references unknown server '{sref}'"),
                )),
            }
        }
        for kref in &profile.skills {
            if kref == "*" {
                continue;
            }
            // Inline definitions validate without touching the store; only
            // non-inline names consult the library (and only when ctx is given).
            if manifest.skills.contains_key(kref) {
                continue;
            }
            match ctx {
                Some(cx) => {
                    // Validation is offline: a name that resolves to a known
                    // source is valid even if a git body isn't cached yet.
                    match resolve_skill(
                        manifest,
                        cx.manifest_dir,
                        cx.library,
                        cx.lib_home,
                        cx.store,
                        kref,
                        ResolveMode::NoFetch,
                    ) {
                        Ok(_) | Err(ResolveError::NotAvailableOffline { .. }) => {}
                        Err(ResolveError::Unresolved { .. }) => issues.push(Issue::new(
                            IssueKind::UnknownSkillRef,
                            format!("profile '{pname}' references unknown skill '{kref}'"),
                        )),
                        // Unreachable in practice — an inline `[skills.<name>]`
                        // block short-circuits above at `contains_key`, so the
                        // resolver only sees non-inline names here — but the P19
                        // variant carries a real fix, so surface it rather than
                        // panic if the path ever changes.
                        Err(e @ ResolveError::InlineNoSourceShadowsLibrary { .. }) => {
                            issues.push(Issue::new(
                                IssueKind::UnresolvableSkillRef,
                                format!("profile '{pname}' skill '{kref}': {e}"),
                            ))
                        }
                        Err(ResolveError::Source(e)) => issues.push(Issue::new(
                            IssueKind::UnresolvableSkillRef,
                            format!("profile '{pname}' skill '{kref}' failed to resolve: {e}"),
                        )),
                    }
                }
                None => issues.push(Issue::new(
                    IssueKind::UnknownSkillRef,
                    format!("profile '{pname}' references unknown skill '{kref}'"),
                )),
            }
        }
    }

    // Instruction fragment targets: a typo'd adapter id in `[instructions.X]
    // targets` would silently compile the fragment into no harness — the same
    // check `[servers]` gets. Machine-layer fragments merge in
    // (from_user_layer): their ids come from the same registry, so validate
    // them too, but say so in the wording — a bad id there is the machine
    // manifest's, not this project's. (`"*"` and no target set stay valid.)
    if !targets.is_empty() {
        for (name, instr) in &manifest.instructions {
            for target in &instr.targets {
                if target != "*" && !targets.contains(target) {
                    let msg = if instr.from_user_layer {
                        format!("machine-layer instruction '{name}' references unknown target '{target}'")
                    } else {
                        format!("instruction '{name}' references unknown target '{target}'")
                    };
                    issues.push(Issue::new(IssueKind::UnknownInstructionTarget, msg));
                }
            }
        }
    }

    // Native extensions (D6/E3): every declaration must be pinnable and
    // deliverable exactly as reviewed — no wildcard target, no source the
    // strict digest can't cover, no squatting on the guard's artifact names.
    // Sources: an inline `path`, an inline `git` (which must name a `subpath`,
    // since a checkout's `.git` can't be reproducibly pinned), or a sourceless
    // entry that resolves from the central library (inline-first, like skills).
    for (name, ext) in &manifest.extensions {
        // The name becomes the rendered artifact's on-disk basename, so it must
        // be a single plain path component: no separator or `..` may smuggle the
        // copy outside the extension directory agentstack owns.
        if !is_safe_component_name(name) {
            issues.push(Issue::new(
                IssueKind::InvalidExtensionName,
                format!("extension '{name}' is not a valid name — an extension name must be a plain path component (no `/`, `\\`, or `..`)"),
            ));
        }
        if name.starts_with("agentstack-guard") {
            issues.push(Issue::new(
                IssueKind::ReservedExtensionName,
                format!("extension '{name}' uses a reserved name — `agentstack-guard*` belongs to the host guard"),
            ));
        }
        if ext.git.is_some() {
            // Git sources are supported (E3), but a git extension is always
            // digested at a subpath — the checkout's `.git` cannot be part of a
            // reproducible pin, so an in-repo directory must be named.
            let has_subpath = ext.subpath.as_deref().is_some_and(|s| !s.trim().is_empty());
            if !has_subpath {
                issues.push(Issue::new(
                    IssueKind::InvalidExtensionSource,
                    format!("extension '{name}' has a `git` source but no `subpath` — point `subpath` at the extension's directory within the repo"),
                ));
            }
        } else if ext.path.is_none() {
            // Sourceless: valid only as a central-library reference (and only
            // when the library is available to consult). Without a library
            // context, this is the inline-only view — treat it as unpinnable,
            // exactly like a profile skill ref that resolves nowhere offline.
            let in_library = ctx
                .map(|cx| cx.library.get_extension(name).is_some())
                .unwrap_or(false);
            if !in_library {
                issues.push(Issue::new(
                    IssueKind::InvalidExtensionSource,
                    format!("extension '{name}' has no `path` or `git` source and is not in the central library"),
                ));
            }
        }
        if ext.target == "*" {
            issues.push(Issue::new(
                IssueKind::UnknownExtensionTarget,
                format!("extension '{name}' must target exactly one adapter — extension code is harness-specific, `\"*\"` cannot apply"),
            ));
        } else if !targets.is_empty() && !targets.contains(&ext.target) {
            issues.push(Issue::new(
                IssueKind::UnknownExtensionTarget,
                format!(
                    "extension '{name}' references unknown target '{}'",
                    ext.target
                ),
            ));
        }
    }

    // Governed workflows (D7 W1): every declaration must be pinnable and its
    // authority request must be resolvable and bounded. All errors — a
    // half-declared workflow must never exist, and the admission choke point
    // (`crate::workflows::normalized_workflows`) refuses on any of these too.
    for (name, wf) in &manifest.workflows {
        // The name is the workflow's run identity (and a future artifact
        // basename) — same containment rule as extensions.
        if !is_safe_component_name(name) {
            issues.push(Issue::new(
                IssueKind::InvalidWorkflowName,
                format!("workflow '{name}' is not a valid name — a workflow name must be a plain path component (no `/`, `\\`, or `..`)"),
            ));
        }
        if wf.git.is_some() {
            // Same rule as git extensions: the checkout's `.git` cannot be
            // part of a reproducible pin, so a subpath must be named.
            let has_subpath = wf.subpath.as_deref().is_some_and(|s| !s.trim().is_empty());
            if !has_subpath {
                issues.push(Issue::new(
                    IssueKind::InvalidWorkflowSource,
                    format!("workflow '{name}' has a `git` source but no `subpath` — point `subpath` at the workflow's directory within the repo"),
                ));
            }
        } else if wf.path.is_none() {
            // Inline-only in W1: no central-library fallback exists for
            // workflows yet, so sourceless is unconditionally unpinnable.
            issues.push(Issue::new(
                IssueKind::InvalidWorkflowSource,
                format!("workflow '{name}' has no `path` or `git` source — workflow sources are inline-only (a central-library workflow kind is not implemented yet)"),
            ));
        }
        // Roles are the authority-request surface: each must name a declared
        // profile — a role resolving to no capability set can never be
        // admitted, so it fails here, at authoring time.
        for role in &wf.roles {
            if !manifest.profiles.contains_key(role) {
                issues.push(Issue::new(
                    IssueKind::UnknownWorkflowRole,
                    format!("workflow '{name}' role '{role}' names no `[profiles.{role}]` — every role must be a declared profile"),
                ));
            }
        }
        if wf.roles_sorted_unique().len() > MAX_WORKFLOW_ROLES {
            issues.push(Issue::new(
                IssueKind::InvalidWorkflowBounds,
                format!("workflow '{name}' declares more than {MAX_WORKFLOW_ROLES} unique roles"),
            ));
        }
        // Zero is refused rather than read as "unlimited": a zero ceiling is
        // always a typo, and misreading it open would widen (rule 2).
        if let Some(n) = wf.max_agents {
            if n == 0 || n > MAX_WORKFLOW_AGENTS {
                issues.push(Issue::new(
                    IssueKind::InvalidWorkflowBounds,
                    format!(
                        "workflow '{name}' max_agents = {n} is outside 1..={MAX_WORKFLOW_AGENTS}"
                    ),
                ));
            }
        }
        if let Some(s) = wf.max_wall_seconds {
            if s == 0 || s > MAX_WORKFLOW_WALL_SECONDS {
                issues.push(Issue::new(
                    IssueKind::InvalidWorkflowBounds,
                    format!("workflow '{name}' max_wall_seconds = {s} is outside 1..={MAX_WORKFLOW_WALL_SECONDS}"),
                ));
            }
        }
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{LibraryServer, LibrarySkill};
    use assert_fs::prelude::*;

    fn parse(s: &str) -> Manifest {
        toml::from_str(s).unwrap()
    }

    /// A library home with one path-source skill body on disk plus its index
    /// entry.
    fn library_with_skill(lib_home: &assert_fs::TempDir, name: &str) -> Library {
        lib_home
            .child(format!("skills/{name}/SKILL.md"))
            .write_str("# body\n")
            .unwrap();
        let mut lib = Library::default();
        lib.upsert(LibrarySkill {
            name: name.into(),
            source: "path".into(),
            path: Some(name.into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: Some("consolidated".into()),
        });
        lib
    }

    /// A malformed `[policy.egress]` pattern is rejected at authoring time —
    /// the same pattern would fail every runtime decision closed, so the
    /// validator names it before a run ever hits the denial.
    #[test]
    fn flags_malformed_egress_patterns() {
        let m = parse(
            r#"
            version = 1
            [policy.egress]
            api = ["api.example.com:443", "!evil.example:443junk"]
            "#,
        );
        let issues = validate(&m);
        let issue = issues
            .iter()
            .find(|i| i.kind == IssueKind::MalformedEgressPattern)
            .expect("malformed pattern must be flagged");
        assert!(issue.kind.is_error());
        assert!(issue.message.contains("!evil.example:443junk"), "{issue:?}");
        // Well-formed patterns raise nothing.
        let m = parse(
            r#"
            version = 1
            [policy.egress]
            api = ["api.example.com:443", "!evil.example", "*.corp.example:*"]
            "#,
        );
        assert!(validate(&m)
            .iter()
            .all(|i| i.kind != IssueKind::MalformedEgressPattern));
    }

    #[test]
    fn flags_unknown_profile_refs() {
        let m = parse(
            r#"
            version = 1
            [servers.kibana]
            type = "http"
            url = "https://x"
            [profiles.p]
            servers = ["kibana", "ghost"]
            skills = ["nope"]
            "#,
        );
        let issues = validate(&m);
        assert!(issues.iter().any(|i| i.kind == IssueKind::UnknownServerRef));
        assert!(issues.iter().any(|i| i.kind == IssueKind::UnknownSkillRef));
    }

    #[test]
    fn flags_missing_transport_fields() {
        let m = parse(
            r#"
            version = 1
            [servers.bad]
            type = "http"
            "#,
        );
        let issues = validate(&m);
        assert_eq!(issues[0].kind, IssueKind::MissingTransportFields);
    }

    #[test]
    fn flags_extras_for_unknown_adapter_id() {
        let m = parse(
            r#"
            version = 1
            [servers.miro]
            type = "stdio"
            command = "npx"
            [servers.miro.extra.codx]
            startup_timeout_sec = 20
            "#,
        );
        // With a known target set, the typo'd adapter id is flagged…
        let issues = validate_with_targets(&m, ["codex", "claude-code"]);
        assert!(issues
            .iter()
            .any(|i| i.kind == IssueKind::UnknownExtraTarget && i.message.contains("codx")));
        // …and a correct id validates clean.
        let m = parse(
            r#"
            version = 1
            [servers.miro]
            type = "stdio"
            command = "npx"
            [servers.miro.extra.codex]
            startup_timeout_sec = 20
            "#,
        );
        assert!(validate_with_targets(&m, ["codex", "claude-code"]).is_empty());
        // Without a target set, the check is skipped (registry-independent).
        assert!(validate(&m).is_empty());
    }

    #[test]
    fn flags_unknown_server_target_but_allows_wildcard_and_empty() {
        let m = parse(
            r#"
            version = 1
            [servers.typo]
            type = "http"
            url = "https://x"
            targets = ["codx"]
            [servers.scoped]
            type = "http"
            url = "https://x"
            targets = ["codex"]
            [servers.recipe-owned]
            type = "http"
            url = "https://x"
            targets = []
            [servers.wildcard]
            type = "http"
            url = "https://x"
            targets = ["*"]
            "#,
        );
        // With a known target set, only the typo'd id is flagged — the
        // wildcard, a registered id, and the deliberate empty list are fine.
        let issues = validate_with_targets(&m, ["codex", "claude-code"]);
        assert_eq!(
            issues
                .iter()
                .filter(|i| i.kind == IssueKind::UnknownServerTarget)
                .count(),
            1
        );
        assert!(issues
            .iter()
            .any(|i| i.kind == IssueKind::UnknownServerTarget
                && i.message.contains("typo")
                && i.message.contains("codx")));
        // Without a target set, the check is skipped (registry-independent).
        assert!(validate(&m).is_empty());
    }

    #[test]
    fn flags_unknown_instruction_target_but_allows_wildcard_and_default() {
        let m = parse(
            r#"
            version = 1
            [instructions.typo]
            path = "./instructions/typo.md"
            targets = ["claude-kode"]
            [instructions.scoped]
            path = "./instructions/scoped.md"
            targets = ["codex"]
            [instructions.wildcard]
            path = "./instructions/wildcard.md"
            targets = ["*"]
            [instructions.defaulted]
            path = "./instructions/defaulted.md"
            "#,
        );
        // With a known target set, only the typo'd id is flagged — the wildcard,
        // a registered id, and the implicit `["*"]` default are all fine.
        let issues = validate_with_targets(&m, ["codex", "claude-code"]);
        assert_eq!(
            issues
                .iter()
                .filter(|i| i.kind == IssueKind::UnknownInstructionTarget)
                .count(),
            1
        );
        assert!(issues
            .iter()
            .any(|i| i.kind == IssueKind::UnknownInstructionTarget
                && i.message.contains("typo")
                && i.message.contains("claude-kode")));
        // Without a target set, the check is skipped (registry-independent).
        assert!(validate(&m).is_empty());
    }

    #[test]
    fn flags_unknown_server_owner() {
        let m = parse(
            r#"
            version = 1
            [servers.owned]
            type = "stdio"
            command = "node"
            owner = "codex"
            [servers.typo]
            type = "stdio"
            command = "node"
            owner = "codx"
            "#,
        );
        // A registered owner id is fine; a typo'd one would silently disable
        // the refresh-from-disk and let stale values fan out again.
        let issues = validate_with_targets(&m, ["codex", "claude-code"]);
        assert_eq!(
            issues
                .iter()
                .filter(|i| i.kind == IssueKind::UnknownServerOwner)
                .count(),
            1
        );
        assert!(issues
            .iter()
            .any(|i| i.kind == IssueKind::UnknownServerOwner
                && i.message.contains("typo")
                && i.message.contains("codx")));
        // Without a target set, the check is skipped (registry-independent).
        assert!(validate(&m).is_empty());
    }

    #[test]
    fn clean_manifest_has_no_issues() {
        let m = parse(
            r#"
            version = 1
            [servers.kibana]
            type = "http"
            url = "https://x"
            [profiles.p]
            servers = ["kibana"]
            skills = ["*"]
            "#,
        );
        assert!(validate(&m).is_empty());
    }

    // A profile that references a skill only present in the central library.
    const PROFILE_REFS_LIBRARY: &str = r#"
        version = 1
        [profiles.p]
        skills = ["sql-review"]
    "#;

    #[test]
    fn library_skill_ref_validates_without_inline() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = library_with_skill(&lib_home, "sql-review");
        let ctx = ValidateCtx {
            manifest_dir: proj.path(),
            library: &library,
            lib_home: lib_home.path(),
            store: &store,
        };

        let m = parse(PROFILE_REFS_LIBRARY);
        // Without context, the library-only ref is unknown (today's behavior).
        assert!(validate(&m)
            .iter()
            .any(|i| i.kind == IssueKind::UnknownSkillRef));
        // With context, it resolves and validation is clean.
        assert!(validate_with_context(&m, std::iter::empty::<&str>(), &ctx).is_empty());
    }

    #[test]
    fn unresolved_skill_ref_still_fails_with_context() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = Library::default(); // empty — "sql-review" is nowhere
        let ctx = ValidateCtx {
            manifest_dir: proj.path(),
            library: &library,
            lib_home: lib_home.path(),
            store: &store,
        };

        let m = parse(PROFILE_REFS_LIBRARY);
        let issues = validate_with_context(&m, std::iter::empty::<&str>(), &ctx);
        assert!(issues.iter().any(|i| i.kind == IssueKind::UnknownSkillRef));
    }

    #[test]
    fn inline_skill_ref_still_validates_with_context() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        // Empty library: the ref must validate purely via the inline definition.
        let library = Library::default();
        let ctx = ValidateCtx {
            manifest_dir: proj.path(),
            library: &library,
            lib_home: lib_home.path(),
            store: &store,
        };

        let m = parse(
            r#"
            version = 1
            [skills.play]
            path = "./skills/play"
            [profiles.p]
            skills = ["play"]
            "#,
        );
        assert!(validate_with_context(&m, std::iter::empty::<&str>(), &ctx).is_empty());
    }

    #[test]
    fn wildcard_still_validates_with_context() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = Library::default();
        let ctx = ValidateCtx {
            manifest_dir: proj.path(),
            library: &library,
            lib_home: lib_home.path(),
            store: &store,
        };

        let m = parse(
            r#"
            version = 1
            [profiles.p]
            skills = ["*"]
            "#,
        );
        assert!(validate_with_context(&m, std::iter::empty::<&str>(), &ctx).is_empty());
    }

    #[test]
    fn broken_library_source_produces_useful_issue() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        // Library entry present by name but with neither `path` nor `git` — its
        // source cannot be resolved.
        let mut library = Library::default();
        library.upsert(LibrarySkill {
            name: "sql-review".into(),
            source: "path".into(),
            path: None,
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: None,
        });
        let ctx = ValidateCtx {
            manifest_dir: proj.path(),
            library: &library,
            lib_home: lib_home.path(),
            store: &store,
        };

        let m = parse(PROFILE_REFS_LIBRARY);
        let issues = validate_with_context(&m, std::iter::empty::<&str>(), &ctx);
        let issue = issues
            .iter()
            .find(|i| i.kind == IssueKind::UnresolvableSkillRef)
            .expect("expected an UnresolvableSkillRef issue");
        // The message names the skill and carries the resolver's reason.
        assert!(issue.message.contains("sql-review"));
        assert!(issue.message.contains("failed to resolve"));
    }

    // ---------- profile server refs against the central library (Phase 1b) ----------

    // A profile referencing a server only present in the central library.
    const PROFILE_REFS_SERVER: &str = r#"
        version = 1
        [profiles.p]
        servers = ["kibana"]
    "#;

    /// A library home with one server definition file plus its index entry.
    fn library_with_server(lib_home: &assert_fs::TempDir, name: &str) -> Library {
        lib_home
            .child(format!("servers/{name}.toml"))
            .write_str("type = \"http\"\nurl = \"https://central/mcp\"\n")
            .unwrap();
        let mut lib = Library::default();
        lib.upsert_server(LibraryServer {
            name: name.into(),
            checksum: None,
            version: None,
            provenance: Some("consolidated:codex".into()),
        });
        lib
    }

    #[test]
    fn library_server_ref_validates_without_inline() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = library_with_server(&lib_home, "kibana");
        let ctx = ValidateCtx {
            manifest_dir: proj.path(),
            library: &library,
            lib_home: lib_home.path(),
            store: &store,
        };

        let m = parse(PROFILE_REFS_SERVER);
        // Without context, the library-only ref is unknown (today's behavior).
        assert!(validate(&m)
            .iter()
            .any(|i| i.kind == IssueKind::UnknownServerRef));
        // With context, it resolves and validation is clean.
        assert!(validate_with_context(&m, std::iter::empty::<&str>(), &ctx).is_empty());
    }

    #[test]
    fn unresolved_server_ref_still_fails_with_context() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = Library::default(); // empty — "kibana" is nowhere
        let ctx = ValidateCtx {
            manifest_dir: proj.path(),
            library: &library,
            lib_home: lib_home.path(),
            store: &store,
        };

        let m = parse(PROFILE_REFS_SERVER);
        assert!(validate_with_context(&m, std::iter::empty::<&str>(), &ctx)
            .iter()
            .any(|i| i.kind == IssueKind::UnknownServerRef));
    }

    #[test]
    fn inline_server_ref_still_validates_with_context() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        // Empty library: the ref must validate purely via the inline definition.
        let library = Library::default();
        let ctx = ValidateCtx {
            manifest_dir: proj.path(),
            library: &library,
            lib_home: lib_home.path(),
            store: &store,
        };

        let m = parse(
            r#"
            version = 1
            [servers.kibana]
            type = "http"
            url = "https://x"
            [profiles.p]
            servers = ["kibana"]
            "#,
        );
        assert!(validate_with_context(&m, std::iter::empty::<&str>(), &ctx).is_empty());
    }

    #[test]
    fn broken_library_server_definition_produces_useful_issue() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        // Indexed by name, but its `servers/kibana.toml` file is missing → the
        // definition cannot be resolved.
        let mut library = Library::default();
        library.upsert_server(LibraryServer {
            name: "kibana".into(),
            checksum: None,
            version: None,
            provenance: None,
        });
        let ctx = ValidateCtx {
            manifest_dir: proj.path(),
            library: &library,
            lib_home: lib_home.path(),
            store: &store,
        };

        let m = parse(PROFILE_REFS_SERVER);
        let issues = validate_with_context(&m, std::iter::empty::<&str>(), &ctx);
        let issue = issues
            .iter()
            .find(|i| i.kind == IssueKind::UnresolvableServerRef)
            .expect("expected an UnresolvableServerRef issue");
        assert!(issue.message.contains("kibana"));
        assert!(issue.message.contains("failed to resolve"));
    }

    #[test]
    fn extension_declarations_validate_source_target_and_reserved_names() {
        let m = parse(
            r#"
            version = 1
            [extensions.ok]
            path = "./extensions/ok"
            target = "pi"
            [extensions.from-git]
            git = "https://example.com/x.git"
            target = "pi"
            [extensions.sourceless]
            target = "pi"
            [extensions.everywhere]
            path = "./extensions/everywhere"
            target = "*"
            [extensions.typo]
            path = "./extensions/typo"
            target = "poi"
            [extensions.agentstack-guard-evil]
            path = "./extensions/evil"
            target = "pi"
            "#,
        );
        let issues = validate_with_targets(&m, ["pi", "opencode"]);
        let count = |kind: IssueKind| issues.iter().filter(|i| i.kind == kind).count();
        // git and missing sources both block; the valid entry raises nothing.
        assert_eq!(count(IssueKind::InvalidExtensionSource), 2);
        // "*" and a typo'd id both fail the one-real-adapter rule.
        assert_eq!(count(IssueKind::UnknownExtensionTarget), 2);
        assert_eq!(count(IssueKind::ReservedExtensionName), 1);
        assert!(!issues.iter().any(|i| i.message.contains("'ok'")));

        // Registry-independent mode still catches the wildcard: it is wrong by
        // definition, not by registry lookup.
        let wild = parse("version = 1\n[extensions.e]\npath = \"./x\"\ntarget = \"*\"\n");
        assert!(validate(&wild)
            .iter()
            .any(|i| i.kind == IssueKind::UnknownExtensionTarget));
    }

    #[test]
    fn extension_name_with_path_separators_is_rejected() {
        // The name becomes the rendered artifact's basename; a traversal name
        // would escape the extension directory agentstack owns. Rejected at
        // validation, before it can ever reach the renderer.
        let m = parse("version = 1\n[extensions.\"../evil\"]\npath = \"./x\"\ntarget = \"pi\"\n");
        assert!(validate_with_targets(&m, ["pi"])
            .iter()
            .any(|i| i.kind == IssueKind::InvalidExtensionName));

        // A plain, containable name raises no name issue.
        let ok = parse("version = 1\n[extensions.checkpoint]\npath = \"./x\"\ntarget = \"pi\"\n");
        assert!(!validate_with_targets(&ok, ["pi"])
            .iter()
            .any(|i| i.kind == IssueKind::InvalidExtensionName));
    }

    #[test]
    fn git_extension_needs_a_subpath_but_is_otherwise_valid() {
        // A git source with a subpath is supported (E3) and validates clean.
        let ok = parse(
            "version = 1\n[extensions.e]\ngit = \"https://x/repo.git\"\nsubpath = \"ext\"\ntarget = \"pi\"\n",
        );
        assert!(validate_with_targets(&ok, ["pi"]).is_empty());

        // A git source WITHOUT a subpath is unpinnable (the checkout's `.git`
        // can't be part of a reproducible pin) → InvalidExtensionSource.
        let no_sub =
            parse("version = 1\n[extensions.e]\ngit = \"https://x/repo.git\"\ntarget = \"pi\"\n");
        assert!(validate_with_targets(&no_sub, ["pi"])
            .iter()
            .any(|i| i.kind == IssueKind::InvalidExtensionSource));
    }

    #[test]
    fn sourceless_extension_is_valid_only_as_a_library_ref() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let mut library = Library::default();
        library.upsert_extension(crate::library::LibraryExtension {
            name: "checkpoint".into(),
            source: "path".into(),
            target: "pi".into(),
            path: Some("checkpoint".into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            description: None,
            version: None,
            provenance: None,
        });
        let ctx = ValidateCtx {
            manifest_dir: proj.path(),
            library: &library,
            lib_home: lib_home.path(),
            store: &store,
        };
        let m = parse("version = 1\n[extensions.checkpoint]\ntarget = \"pi\"\n");

        // Without context (inline-only view), a sourceless entry is unpinnable.
        assert!(validate(&m)
            .iter()
            .any(|i| i.kind == IssueKind::InvalidExtensionSource));
        // With the library available, it resolves as a library ref → clean.
        assert!(validate_with_context(&m, ["pi"], &ctx).is_empty());
        // A sourceless entry NOT in the library still fails, even with context.
        let missing = parse("version = 1\n[extensions.ghost]\ntarget = \"pi\"\n");
        assert!(validate_with_context(&missing, ["pi"], &ctx)
            .iter()
            .any(|i| i.kind == IssueKind::InvalidExtensionSource));
    }

    /// D7 W1 witness (roles half): a role naming no `[profiles.*]` is refused
    /// as `UnknownWorkflowRole`; the other workflow findings each fire on
    /// their own trigger, and every one is an error.
    #[test]
    fn workflow_declarations_validate_name_source_roles_and_bounds() {
        let m = parse(
            r#"
            version = 1
            [profiles.reader]
            [workflows.ok]
            path = "./workflows/ok.js"
            roles = ["reader"]
            [workflows.ghost-role]
            path = "./workflows/g.js"
            roles = ["reader", "synthesizer"]
            [workflows.from-git]
            git = "https://x/repo.git"
            [workflows.sourceless]
            roles = []
            [workflows.zero]
            path = "./workflows/z.js"
            max_agents = 0
            max_wall_seconds = 700000
            [workflows."../evil"]
            path = "./workflows/e.js"
            "#,
        );
        let issues = validate(&m);
        let count = |kind: IssueKind| issues.iter().filter(|i| i.kind == kind).count();
        assert_eq!(count(IssueKind::UnknownWorkflowRole), 1);
        assert!(
            issues
                .iter()
                .any(|i| i.kind == IssueKind::UnknownWorkflowRole
                    && i.message.contains("synthesizer"))
        );
        // git-without-subpath AND sourceless are both source errors.
        assert_eq!(count(IssueKind::InvalidWorkflowSource), 2);
        // max_agents = 0 and an over-week wall clock are both bounds errors.
        assert_eq!(count(IssueKind::InvalidWorkflowBounds), 2);
        assert_eq!(count(IssueKind::InvalidWorkflowName), 1);
        assert!(issues.iter().all(|i| i.kind.is_error()));

        // A role list above the unique cap is refused; duplicates don't count.
        let mut toml = String::from("version = 1\n[workflows.big]\npath = \"./w\"\nroles = [");
        let mut profiles = String::new();
        for i in 0..33 {
            toml.push_str(&format!("\"r{i}\","));
            profiles.push_str(&format!("[profiles.r{i}]\n"));
        }
        toml.push_str("]\n");
        toml.push_str(&profiles);
        let issues = validate(&parse(&toml));
        assert!(issues
            .iter()
            .any(|i| i.kind == IssueKind::InvalidWorkflowBounds
                && i.message.contains("unique roles")));
    }
}
