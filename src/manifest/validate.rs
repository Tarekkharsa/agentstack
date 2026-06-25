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
    MissingTransportFields,
    UnknownTargetServer,
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
    let mut issues = Vec::new();

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

    issues
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
}
