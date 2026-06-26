//! Static manifest validation: profile references resolve, servers are
//! internally consistent for their transport.

use super::model::{Manifest, ServerType};

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
            if !manifest.skills.contains_key(kref) {
                issues.push(Issue::new(
                    IssueKind::UnknownSkillRef,
                    format!("profile '{pname}' references unknown skill '{kref}'"),
                ));
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

    fn parse(s: &str) -> Manifest {
        toml::from_str(s).unwrap()
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
}
