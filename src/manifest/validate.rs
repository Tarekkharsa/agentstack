//! Static manifest validation: profile references resolve, servers are
//! internally consistent for their transport.

use std::path::Path;

use super::model::{Manifest, ServerType};
use crate::library::Library;
use crate::resolve::{resolve_skill, ResolveError};
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueKind {
    UnknownServerRef,
    UnknownSkillRef,
    /// A skill ref names a known entry, but resolving its source failed (e.g. a
    /// library entry with a broken/missing source). Distinct from
    /// `UnknownSkillRef`, which means the name resolves nowhere at all.
    UnresolvableSkillRef,
    UnknownHookRef,
    MissingTransportFields,
    UnknownTargetServer,
    UnknownPluginTarget,
    InvalidPluginName,
}

impl IssueKind {
    /// Structural errors that would render broken/partial config — these block
    /// `--write`. (All current kinds are errors; kept as a method so future
    /// warning-only kinds can return `false`.)
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            IssueKind::UnknownServerRef
                | IssueKind::UnknownSkillRef
                | IssueKind::UnresolvableSkillRef
                | IssueKind::UnknownHookRef
                | IssueKind::MissingTransportFields
                | IssueKind::UnknownTargetServer
                | IssueKind::UnknownPluginTarget
                | IssueKind::InvalidPluginName
        )
    }
}

impl Issue {
    fn new(kind: IssueKind, message: impl Into<String>) -> Self {
        Issue {
            kind,
            message: message.into(),
        }
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

    // Server transport consistency.
    for (name, server) in &manifest.servers {
        match server.server_type {
            ServerType::Http => {
                if server.url.is_none() {
                    issues.push(Issue::new(
                        IssueKind::MissingTransportFields,
                        format!("server '{name}' is type=http but has no `url`"),
                    ));
                }
            }
            ServerType::Stdio => {
                if server.command.is_none() {
                    issues.push(Issue::new(
                        IssueKind::MissingTransportFields,
                        format!("server '{name}' is type=stdio but has no `command`"),
                    ));
                }
            }
        }
    }

    // Profile references.
    for (pname, profile) in &manifest.profiles {
        for sref in &profile.servers {
            if !manifest.servers.contains_key(sref) {
                issues.push(Issue::new(
                    IssueKind::UnknownServerRef,
                    format!("profile '{pname}' references unknown server '{sref}'"),
                ));
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
                    match resolve_skill(
                        manifest,
                        cx.manifest_dir,
                        cx.library,
                        cx.lib_home,
                        cx.store,
                        kref,
                    ) {
                        Ok(_) => {}
                        Err(ResolveError::Unresolved { .. }) => issues.push(Issue::new(
                            IssueKind::UnknownSkillRef,
                            format!("profile '{pname}' references unknown skill '{kref}'"),
                        )),
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

    for (plugin_name, plugin) in &manifest.plugins {
        if !is_native_plugin_id(plugin_name) {
            issues.push(Issue::new(
                IssueKind::InvalidPluginName,
                format!("plugin recipe '{plugin_name}' must use kebab-case native id characters"),
            ));
        }
        for sref in &plugin.servers {
            if !manifest.servers.contains_key(sref) {
                issues.push(Issue::new(
                    IssueKind::UnknownServerRef,
                    format!("plugin recipe '{plugin_name}' references unknown server '{sref}'"),
                ));
            }
        }
        for kref in &plugin.skills {
            if !manifest.skills.contains_key(kref) {
                issues.push(Issue::new(
                    IssueKind::UnknownSkillRef,
                    format!("plugin recipe '{plugin_name}' references unknown skill '{kref}'"),
                ));
            }
        }
        for href in &plugin.hooks {
            if !manifest.hooks.contains_key(href) {
                issues.push(Issue::new(
                    IssueKind::UnknownHookRef,
                    format!("plugin recipe '{plugin_name}' references unknown hook '{href}'"),
                ));
            }
        }
        if !targets.is_empty() {
            for target in &plugin.targets {
                if target != "*" && !targets.contains(target) {
                    issues.push(Issue::new(
                        IssueKind::UnknownPluginTarget,
                        format!(
                            "plugin recipe '{plugin_name}' references unknown target '{target}'"
                        ),
                    ));
                }
            }
        }
    }

    issues
}

fn is_native_plugin_id(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::LibrarySkill;
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
            checksum: None,
            version: None,
            provenance: Some("consolidated".into()),
        });
        lib
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

    #[test]
    fn parses_and_validates_plugin_recipe() {
        let m = parse(
            r#"
            version = 1
            [servers.play]
            type = "stdio"
            command = "play"
            [skills.play]
            path = "./skills/play"
            [hooks.notify]
            event = "Stop"
            command = "say done"
            [plugins.play]
            version = "1.0.0"
            description = "Play workflow"
            targets = ["codex", "claude-code"]
            servers = ["play"]
            skills = ["play"]
            hooks = ["notify"]
            "#,
        );
        assert!(validate_with_targets(&m, ["codex", "claude-code"]).is_empty());
    }

    #[test]
    fn flags_invalid_plugin_recipe_refs_and_targets() {
        let m = parse(
            r#"
            version = 1
            [plugins.Bad_Name]
            version = "1.0.0"
            description = "Bad"
            targets = ["ghost"]
            servers = ["missing-server"]
            skills = ["missing-skill"]
            hooks = ["missing-hook"]
            "#,
        );
        let issues = validate_with_targets(&m, ["codex"]);
        assert!(issues
            .iter()
            .any(|i| i.kind == IssueKind::InvalidPluginName));
        assert!(issues.iter().any(|i| i.kind == IssueKind::UnknownServerRef));
        assert!(issues.iter().any(|i| i.kind == IssueKind::UnknownSkillRef));
        assert!(issues.iter().any(|i| i.kind == IssueKind::UnknownHookRef));
        assert!(issues
            .iter()
            .any(|i| i.kind == IssueKind::UnknownPluginTarget));
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
}
