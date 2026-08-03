//! Git-hosted packs: any git repo (any host) with a `pack.toml` at its root —
//! or under `#<subdir>` — is an installable vendor pack, versioned by git tags.
//!
//! `agentstack add from git:github.com/acme/agent-pack@v1.2.0` clones the repo
//! at that tag (policy-gated *before* any fetch), parses `pack.toml`, scans the
//! skill/instruction content (hidden Unicode blocks, injection heuristics
//! warn — the same gate as `install`), and hands the normalized pack to the
//! same install path catalog packs use. Omitting `@<tag>` selects the newest
//! version-shaped tag. Deliberately out of scope for v1: semver ranges and
//! transitive pack dependencies.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use super::{Candidate, CandidateKind, Install, InstrRef, PackSpec, SkillRef};

/// A parsed `git:` pack reference: `git:<url>[@<tag>][#<subdir>]`.
/// Host-style URLs (`github.com/acme/pack`) get `https://`; explicit schemes
/// (`https://`, `file://`, `ssh://`, `git@`) pass through.
#[derive(Debug, Clone, PartialEq)]
pub struct GitPackRef {
    pub url: String,
    pub tag: Option<String>,
    pub subdir: Option<String>,
}

impl GitPackRef {
    /// Parse an `add from` / ledger id. `None` when `id` is not `git:`-shaped.
    pub fn parse(id: &str) -> Option<GitPackRef> {
        let rest = id.strip_prefix("git:")?;
        let (rest, subdir) = match rest.split_once('#') {
            Some((r, s)) if !s.is_empty() => (r, Some(s.to_string())),
            Some((r, _)) => (r, None),
            None => (rest, None),
        };
        // `@` may appear in `git@host:` — split on the last `@` only if what
        // follows looks like a tag (no `/` or `:`).
        let (url_part, tag) = match rest.rsplit_once('@') {
            Some((u, t)) if !t.is_empty() && !t.contains('/') && !t.contains(':') => {
                (u, Some(t.to_string()))
            }
            _ => (rest, None),
        };
        if url_part.is_empty() {
            return None;
        }
        let url = if url_part.contains("://") || url_part.starts_with("git@") {
            url_part.to_string()
        } else {
            format!("https://{url_part}")
        };
        Some(GitPackRef { url, tag, subdir })
    }

    /// The id written to the pack ledger so `upgrade` can re-resolve it:
    /// `git:<url>@<tag>[#subdir]`.
    pub fn ledger_id(&self, tag: &str) -> String {
        let mut s = format!("git:{}@{tag}", self.url);
        if let Some(sub) = &self.subdir {
            s.push('#');
            s.push_str(sub);
        }
        s
    }

    /// Source labels to check against `[policy] allowed_sources`, scheme-full
    /// and scheme-less (`git:https://github.com/x` and `git:github.com/x`), so
    /// the documented `git:github.com/acme/*` pattern matches either spelling.
    pub fn policy_sources(&self) -> Vec<String> {
        let mut v = vec![format!("git:{}", self.url)];
        if let Some(rest) = self.url.split_once("://").map(|(_, r)| r) {
            v.push(format!("git:{rest}"));
        }
        v
    }
}

/// `pack.toml` — the pack manifest a repo publishes.
///
/// `deny_unknown_fields`, and it is load-bearing (review finding 4). This file
/// is somebody else's bytes (invariant 7: all repository content is hostile
/// input), and serde's default is to DROP every key it does not know. That is
/// how a pack declaring hooks installed as though it had none — the bug
/// [`Self::hooks`] exists to fix. Naming two more keys fixed two spellings and
/// left the failure mode intact for every other: a plural `[[hooks]]`, a
/// future `[[workflow]]`, a typo. A publisher would ship a package believing a
/// capability was in it, a consumer would install one silently missing that
/// capability, and no surface anywhere would say so.
///
/// Refusing an unknown key by name is the fail-closed reading: it costs a
/// publisher one clear error and it costs a consumer nothing.
/// `LibraryInstructionToml` already takes this posture over the same class of
/// input, for the same reason.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackToml {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub server: Option<PackServerToml>,
    #[serde(default, rename = "skill")]
    pub skills: Vec<PackMemberToml>,
    #[serde(default, rename = "instruction")]
    pub instructions: Vec<PackMemberToml>,
    /// Hook members. **Parsed only so they can be REFUSED** — v1 packages carry
    /// servers, skills and instructions, and package-carried executable kinds
    /// are deferred by name (`docs/design/package-layer.md`). Before this
    /// field existed, serde silently dropped the array, so a pack declaring
    /// hooks installed as if it had none. Seeing them and refusing is the
    /// fail-closed reading of the same bytes.
    #[serde(default, rename = "hook")]
    pub hooks: Vec<PackMemberToml>,
    /// Extension members. Present for the same reason as [`Self::hooks`], and
    /// refused by the same gate.
    #[serde(default, rename = "extension")]
    pub extensions: Vec<PackMemberToml>,
    #[serde(default)]
    pub targets: Vec<String>,
}

/// Same posture as [`PackToml`], and for the same reason: a dropped key here
/// is a server that contacts or runs something other than what its author
/// declared.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackServerToml {
    #[serde(rename = "type")]
    pub server_type: String,
    pub url: Option<String>,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Header names that need a secret (lifted to `${REF}`s, never values).
    #[serde(default)]
    pub secret_headers: Vec<String>,
    /// Env var names that need a secret.
    #[serde(default)]
    pub secret_env: Vec<String>,
}

impl PackServerToml {
    /// The transport-neutral [`Install`] this `[server]` table describes.
    ///
    /// Extracted from [`resolve`] rather than duplicated because the library
    /// package rail ([`crate::package`]) needs the identical reading: one
    /// `pack.toml` grammar must produce one server definition, or a package's
    /// server would digest differently depending on which rail installed it.
    pub fn to_install(&self) -> Result<Install> {
        Ok(match self.server_type.as_str() {
            "http" => Install::Http {
                url: self
                    .url
                    .clone()
                    .context("pack.toml [server] type=http needs url")?,
                secret_headers: self.secret_headers.clone(),
            },
            "stdio" => Install::Stdio {
                command: self
                    .command
                    .clone()
                    .context("pack.toml [server] type=stdio needs command")?,
                args: self.args.clone(),
                secret_env: self.secret_env.clone(),
            },
            other => anyhow::bail!("pack.toml [server] type '{other}' is not http|stdio"),
        })
    }
}

/// Same posture as [`PackToml`]: a member's unknown key is a member declaring
/// something this version does not honour, and silence about it is what
/// finding 4 is.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackMemberToml {
    pub name: String,
    /// Path relative to the pack root.
    pub path: String,
}

/// Name-contract gate for everything a remote `pack.toml` self-reports
/// (design §C.3). The pack's own name becomes `[packs.<name>]` and
/// `[servers.<name>]` manifest keys; member names become `[skills.<name>]`
/// keys and — for instructions — path components. All-or-nothing: one bad
/// name rejects the whole pack, matching the atomic-install semantics.
pub fn validate_pack_names(parsed: &PackToml) -> anyhow::Result<()> {
    use anyhow::Context;
    crate::text::validate_name(&parsed.name)
        .with_context(|| format!("pack.toml name '{}'", parsed.name.escape_debug()))?;
    // Hooks and extensions are name-gated too even though
    // [`refuse_executable_members`] rejects the package outright: the refusal
    // message quotes the member name, and quoting an ungated name would put
    // unvalidated remote text in the message. Gate first, then refuse.
    for m in parsed
        .skills
        .iter()
        .chain(parsed.instructions.iter())
        .chain(parsed.hooks.iter())
        .chain(parsed.extensions.iter())
    {
        crate::text::validate_name(&m.name)
            .with_context(|| format!("pack.toml member '{}'", m.name.escape_debug()))?;
    }
    Ok(())
}

/// The v1 scope fence: a package carrying **hooks or extensions is refused by
/// name**, whether it arrives from a git repo or from the central library.
///
/// This is not a "not yet" that a later patch relaxes quietly. Hooks and
/// extensions run commands at user permission, and `CLAUDE.md`'s standing
/// classification says the full consent ceremony always applies to them, with
/// no compressed-consent path ever covering them. A package reference IS a
/// compressed consent path — one name in a toolset standing for a set of
/// members — so the two cannot meet. The fence is a permanent property of the
/// v1 schema; lifting it means designing a ceremony, not deleting a check.
///
/// All-or-nothing, like [`validate_pack_names`]: one executable member refuses
/// the whole package, matching the atomic-install semantics.
pub fn refuse_executable_members(parsed: &PackToml) -> anyhow::Result<()> {
    let offending: Vec<String> = parsed
        .hooks
        .iter()
        .map(|m| format!("hook '{}'", m.name.escape_debug()))
        .chain(
            parsed
                .extensions
                .iter()
                .map(|m| format!("extension '{}'", m.name.escape_debug())),
        )
        .collect();
    anyhow::ensure!(
        offending.is_empty(),
        "package '{}' carries {} — package-carried executable kinds (hooks, extensions) are \
         not supported in v1: they run commands at your permission and always take the full \
         consent ceremony, which a package reference cannot compress. Declare them in this \
         project's manifest instead (`[hooks.<name>]` / `[extensions.<name>]`), where the \
         ceremony applies, and take the rest of the package as it is.",
        parsed.name.escape_debug(),
        offending.join(", ")
    );
    Ok(())
}

/// A git pack resolved to concrete content on disk.
pub struct ResolvedGitPack {
    pub candidate: Candidate,
    pub spec: PackSpec,
    /// The pack root inside the store clone (clone dir + subdir).
    pub root: PathBuf,
    /// The tag the content came from.
    pub tag: String,
    /// The commit that tag resolved to.
    pub commit: String,
    /// Ledger id: `git:<url>@<tag>[#subdir]`.
    pub source_id: String,
}

/// Newest version-shaped tag (`v1.2.3` / `1.2.3`-style), by numeric component
/// compare. `None` when no tag parses as a version.
pub fn latest_version_tag(tags: &[String]) -> Option<&String> {
    tags.iter()
        .filter_map(|t| version_key(t).map(|k| (k, t)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, t)| t)
}

/// Numeric components of a version-shaped tag; `None` for non-version tags.
pub fn version_key(tag: &str) -> Option<Vec<u64>> {
    let t = tag.strip_prefix('v').unwrap_or(tag);
    let parts: Vec<u64> = t
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    (!parts.is_empty()).then_some(parts)
}

/// Clone + checkout + parse + content-scan a git pack. Policy must already be
/// gated by the caller (never fetch a forbidden source). With no explicit tag
/// the newest version tag is selected — packs are versioned artifacts, so a
/// repo with no version tags is an error, not a floating install.
pub fn resolve(refr: &GitPackRef) -> Result<ResolvedGitPack> {
    let store = crate::store::Store::default_store();
    let tag = match &refr.tag {
        Some(t) => t.clone(),
        None => {
            let tags = crate::store::ls_remote_tags(&refr.url)
                .with_context(|| format!("listing tags of {}", refr.url))?;
            latest_version_tag(&tags)
                .cloned()
                .with_context(|| format!(
                    "{} has no version tags (v1.2.3-style); packs install from tags — pass an explicit git:<url>@<tag>",
                    refr.url
                ))?
        }
    };
    let (clone_dir, commit) = crate::store::checkout(&store, &refr.url, Some(&tag))
        .with_context(|| format!("fetching {} at {tag}", refr.url))?;
    let root = match &refr.subdir {
        Some(sub) => {
            let p = clone_dir.join(sub);
            // Containment: a hostile subdir must not escape the clone.
            anyhow::ensure!(
                p.canonicalize()
                    .map(|c| c.starts_with(&clone_dir))
                    .unwrap_or(false),
                "subdir '{sub}' escapes the repository"
            );
            p
        }
        None => clone_dir,
    };

    let pack_toml = root.join("pack.toml");
    let text = std::fs::read_to_string(&pack_toml)
        .with_context(|| format!("{} has no pack.toml at {}", refr.url, pack_toml.display()))?;
    let parsed: PackToml = toml::from_str(&text).context("parsing pack.toml")?;
    validate_pack_names(&parsed)?;
    refuse_executable_members(&parsed)?;

    // Content scan at fetch time — the same gate `install` applies to skills.
    // High severity (hidden Unicode) blocks; heuristics print as warnings.
    for m in parsed.skills.iter().chain(parsed.instructions.iter()) {
        let target = contained(&root, &m.path, &m.name)?;
        let findings = if target.is_dir() {
            crate::scan::scan_tree(&target)
        } else {
            crate::scan::scan_file(&target, &m.path)
        }
        .with_context(|| format!("scanning pack member '{}'", m.name))?;
        for f in &findings {
            if f.severity == crate::scan::Severity::High {
                anyhow::bail!(
                    "pack member '{}' failed the content scan: {} — nothing installed",
                    m.name,
                    f.describe()
                );
            }
            eprintln!("warning: pack member '{}': {}", m.name, f.describe());
        }
    }

    let server = match &parsed.server {
        None => None,
        Some(s) => Some(s.to_install()?),
    };
    let spec = PackSpec {
        server,
        skills: parsed
            .skills
            .iter()
            .map(|m| SkillRef {
                name: m.name.clone(),
                path: Some(m.path.clone()),
                git: None,
                rev: None,
            })
            .collect(),
        instructions: parsed
            .instructions
            .iter()
            .map(|m| InstrRef {
                name: m.name.clone(),
                path: m.path.clone(),
            })
            .collect(),
        targets: parsed.targets.clone(),
    };
    let source_id = refr.ledger_id(&tag);
    let candidate = Candidate {
        id: source_id.clone(),
        name: parsed.name.clone(),
        description: parsed.description.clone(),
        source: "git",
        kind: CandidateKind::Pack(spec.clone()),
    };
    Ok(ResolvedGitPack {
        candidate,
        spec,
        root,
        tag,
        commit,
        source_id,
    })
}

/// Resolve `rel` under `root`, refusing paths that escape it (`..`, absolute).
pub fn contained(root: &Path, rel: &str, member: &str) -> Result<PathBuf> {
    let p = PathBuf::from(rel);
    anyhow::ensure!(
        !p.is_absolute()
            && !p
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
        "pack member '{member}' path '{rel}' must be relative and stay inside the pack"
    );
    let full = root.join(p);
    anyhow::ensure!(
        full.exists(),
        "pack member '{member}' path '{rel}' does not exist in the pack"
    );
    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C.1 regression (design §C.1): a pack whose instruction member name
    /// traverses — previously interpolated into `instructions/{name}.md`
    /// unchecked — is rejected at the parse gate, before any extraction. Same
    /// gate covers a hostile pack-level name.
    #[test]
    fn hostile_pack_names_rejected_at_parse() {
        let traversing: PackToml = toml::from_str(
            r#"
            name = "ok-pack"
            [[instruction]]
            name = "../../outside"
            path = "docs/i.md"
            "#,
        )
        .unwrap();
        let err = validate_pack_names(&traversing).unwrap_err();
        assert!(err.to_string().contains("../../outside"), "{err:#}");

        let bad_pack_name: PackToml = toml::from_str(
            r#"
            name = "Bad Pack"
            [[skill]]
            name = "fine"
            path = "skills/fine"
            "#,
        )
        .unwrap();
        assert!(validate_pack_names(&bad_pack_name).is_err());

        let clean: PackToml = toml::from_str(
            r#"
            name = "ok-pack"
            [[skill]]
            name = "sql-review"
            path = "skills/sql-review"
            "#,
        )
        .unwrap();
        assert!(validate_pack_names(&clean).is_ok());
    }

    #[test]
    fn parses_git_refs() {
        let r = GitPackRef::parse("git:github.com/acme/pack@v1.2.0").unwrap();
        assert_eq!(r.url, "https://github.com/acme/pack");
        assert_eq!(r.tag.as_deref(), Some("v1.2.0"));
        assert_eq!(r.subdir, None);

        let r = GitPackRef::parse("git:file:///tmp/repo@v1#packs/a").unwrap();
        assert_eq!(r.url, "file:///tmp/repo");
        assert_eq!(r.tag.as_deref(), Some("v1"));
        assert_eq!(r.subdir.as_deref(), Some("packs/a"));

        // No tag; scheme passthrough; not-git ids rejected.
        let r = GitPackRef::parse("git:https://gitlab.com/x/y").unwrap();
        assert_eq!(r.url, "https://gitlab.com/x/y");
        assert_eq!(r.tag, None);
        assert!(GitPackRef::parse("linear-pack").is_none());

        // git@ ssh form keeps its `@`.
        let r = GitPackRef::parse("git:git@github.com:acme/pack.git@v2").unwrap();
        assert_eq!(r.url, "git@github.com:acme/pack.git");
        assert_eq!(r.tag.as_deref(), Some("v2"));
    }

    #[test]
    fn ledger_id_round_trips() {
        let r = GitPackRef::parse("git:github.com/acme/pack@v1#sub/dir").unwrap();
        let id = r.ledger_id("v1");
        assert_eq!(id, "git:https://github.com/acme/pack@v1#sub/dir");
        let back = GitPackRef::parse(&id).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn latest_tag_by_version() {
        let tags: Vec<String> = ["v1.2.0", "v1.10.0", "v1.9.9", "nightly", "0.1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(latest_version_tag(&tags).unwrap(), "v1.10.0");
        assert!(latest_version_tag(&["snapshot".to_string()]).is_none());
    }

    #[test]
    fn policy_sources_cover_both_spellings() {
        let r = GitPackRef::parse("git:github.com/acme/pack").unwrap();
        let v = r.policy_sources();
        assert!(v.contains(&"git:https://github.com/acme/pack".to_string()));
        assert!(v.contains(&"git:github.com/acme/pack".to_string()));
    }
}
