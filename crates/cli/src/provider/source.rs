//! The `add skill <source>` grammar — one parser for every way the skills
//! ecosystem spells "where a skill lives" (design:
//! `docs/design/add-skill-source-grammar.md` §1).
//!
//! Rules that are policy, not convenience:
//! - A local path must be SPELLED (`./x`, `../x`, absolute, `~/…`) — a bare
//!   `owner/repo` never probes the filesystem, so the same input means the
//!   same thing on every machine.
//! - Fail, never normalize: credential-bearing URLs are rejected (they would
//!   print in the preview and persist into a committable manifest), not
//!   silently stripped.
//! - `#ref` and `@skill` are human aliases; the canonical spellings are
//!   `--rev`/`--skill`, and an alias disagreeing with its flag is the
//!   caller's error to raise (the parser only reports what it saw).

use std::path::PathBuf;

use anyhow::{bail, Result};

/// Where the skill bytes come from, after parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    Local {
        path: PathBuf,
    },
    Git {
        /// Canonical, credential-free URL — what the manifest records.
        url: String,
        /// From `/tree/<ref>` or a `#ref` fragment; the caller merges it
        /// with `--rev` (disagreement = error there).
        ref_: Option<String>,
        /// From a `/tree/<ref>/<subpath>` URL; validated: no `..`, no
        /// absolute segments.
        subpath: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSource {
    pub source: SkillSource,
    /// From an `owner/repo@skill` alias (shorthand form only).
    pub skill_alias: Option<String>,
}

/// Parse one `add skill` source argument. Resolution order matters and is
/// part of the contract: local spellings first (whole input, `#` is a legal
/// path byte), then git forms on the fragment-stripped remainder.
pub fn parse_source(input: &str) -> Result<ParsedSource> {
    let input = input.trim();
    if input.is_empty() {
        bail!("empty source — pass owner/repo, a git URL, or ./local-dir");
    }

    // 1. Spelled local paths take the whole input verbatim.
    if is_spelled_path(input) {
        return Ok(ParsedSource {
            source: SkillSource::Local {
                path: crate::util::paths::expand_tilde(input),
            },
            skill_alias: None,
        });
    }

    // 2. `#ref` fragment — only meaningful on git-shaped inputs, which is
    //    everything from here on (local was handled above).
    let (base, frag_ref) = match input.split_once('#') {
        Some((b, f)) if !f.is_empty() => (b, Some(f.to_string())),
        Some((b, _)) => (b, None),
        None => (input, None),
    };

    // 3. GitHub / GitLab https URLs, incl. /tree/<ref>[/<subpath>].
    if let Some(parsed) = parse_https_host(base, frag_ref.clone())? {
        return Ok(parsed);
    }

    // 4. `owner/repo[@skill]` shorthand — always GitHub.
    if let Some(parsed) = parse_shorthand(base, frag_ref.clone())? {
        return Ok(parsed);
    }

    // 5. Generic git remotes: scp-style `git@host:path`, `ssh://`,
    //    `file://`, or any `*.git` URL.
    if is_generic_remote(base) {
        deny_credentials(base)?;
        crate::gitx::deny_weird_transport(base)?;
        return Ok(ParsedSource {
            source: SkillSource::Git {
                url: base.to_string(),
                ref_: frag_ref,
                subpath: None,
            },
            skill_alias: None,
        });
    }

    bail!(
        "unrecognized source '{}' — pass owner/repo, a github.com/gitlab.com URL \
         (optionally with /tree/<ref>/<subpath>), a git remote (git@…, ssh://, file://, *.git), \
         or a spelled local path (./dir, ../dir, /abs, ~/dir)",
        crate::text::sanitize_line(input)
    )
}

/// Local sources must be spelled; this is the ONLY place the filesystem
/// grammar is decided (a bare name is never probed).
fn is_spelled_path(s: &str) -> bool {
    s == "."
        || s == ".."
        || s == "~"
        || s.starts_with("./")
        || s.starts_with("../")
        || s.starts_with('/')
        || s.starts_with("~/")
}

/// `https://github.com/o/r[.git][/tree/<ref>[/<subpath>]]` and the GitLab
/// equivalents (`/-/tree/` marker, subgroup paths). Returns `Ok(None)` when
/// the input is not an https URL on a known host.
fn parse_https_host(base: &str, frag_ref: Option<String>) -> Result<Option<ParsedSource>> {
    let Some(rest) = base
        .strip_prefix("https://")
        .or_else(|| base.strip_prefix("http://"))
    else {
        return Ok(None);
    };
    if base.starts_with("http://") {
        // The gitx allowlist excludes cleartext http; refuse at parse with
        // the fix rather than failing later inside git.
        bail!("cleartext http:// sources are not accepted — use https://");
    }
    deny_credentials(base)?;

    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let path = path.trim_end_matches('/');
    match host {
        "github.com" => {
            // owner/repo[.git][/tree/<ref>[/<subpath>]]
            let (repo_path, tree) = match path.split_once("/tree/") {
                Some((r, t)) => (r, Some(t)),
                None => (path, None),
            };
            let repo_path = repo_path.trim_end_matches(".git");
            let mut segs = repo_path.split('/');
            let (Some(owner), Some(repo), None) = (segs.next(), segs.next(), segs.next()) else {
                bail!(
                    "'{}' is not owner/repo on github.com",
                    crate::text::sanitize_line(base)
                );
            };
            validate_component(owner)?;
            validate_component(repo)?;
            let (ref_, subpath) = split_tree(tree, frag_ref)?;
            Ok(Some(ParsedSource {
                source: SkillSource::Git {
                    url: format!("https://github.com/{owner}/{repo}"),
                    ref_,
                    subpath,
                },
                skill_alias: None,
            }))
        }
        "gitlab.com" => {
            // Subgroups allowed: group[/sub…]/repo, tree marker is `/-/tree/`.
            let (repo_path, tree) = match path.split_once("/-/tree/") {
                Some((r, t)) => (r, Some(t)),
                None => (path, None),
            };
            let repo_path = repo_path.trim_end_matches(".git");
            if repo_path.split('/').count() < 2 || repo_path.split('/').any(|s| s.is_empty()) {
                bail!(
                    "'{}' is not a gitlab.com repo path",
                    crate::text::sanitize_line(base)
                );
            }
            for seg in repo_path.split('/') {
                validate_component(seg)?;
            }
            let (ref_, subpath) = split_tree(tree, frag_ref)?;
            Ok(Some(ParsedSource {
                source: SkillSource::Git {
                    url: format!("https://gitlab.com/{repo_path}"),
                    ref_,
                    subpath,
                },
                skill_alias: None,
            }))
        }
        // Unknown https hosts are only accepted as generic `*.git` remotes
        // (step 5) — arbitrary "well-known" HTTPS documents are a deliberate
        // exclusion (design §1).
        _ => Ok(None),
    }
}

/// `<ref>[/<subpath>]` after a tree marker. The ref is a single path
/// segment (a branch name containing `/` needs the `#ref` spelling — the
/// URL form can't disambiguate it from the subpath, a known ecosystem
/// limitation). A fragment ref alongside a tree ref that disagrees is an
/// error, same rule as alias-vs-flag.
fn split_tree(
    tree: Option<&str>,
    frag_ref: Option<String>,
) -> Result<(Option<String>, Option<String>)> {
    let Some(tree) = tree else {
        return Ok((frag_ref, None));
    };
    let tree = tree.trim_matches('/');
    if tree.is_empty() {
        bail!("empty ref after /tree/");
    }
    let (r, sub) = match tree.split_once('/') {
        Some((r, s)) => (r.to_string(), Some(s.to_string())),
        None => (tree.to_string(), None),
    };
    if let Some(sub) = &sub {
        validate_subpath(sub)?;
    }
    match frag_ref {
        Some(f) if f != r => bail!(
            "ref given twice and they disagree: /tree/{} vs #{}",
            crate::text::sanitize_line(&r),
            crate::text::sanitize_line(&f)
        ),
        _ => {}
    }
    Ok((Some(r), sub))
}

/// `owner/repo[@skill]` — always GitHub, never a filesystem probe.
fn parse_shorthand(base: &str, frag_ref: Option<String>) -> Result<Option<ParsedSource>> {
    let (repo_part, alias) = match base.split_once('@') {
        Some((r, a)) if !a.is_empty() => (r, Some(a.to_string())),
        Some(_) => return Ok(None),
        None => (base, None),
    };
    let mut segs = repo_part.split('/');
    let (Some(owner), Some(repo), None) = (segs.next(), segs.next(), segs.next()) else {
        return Ok(None);
    };
    let component_ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
            && s.chars().any(|c| c.is_ascii_alphanumeric())
            && s != "."
            && s != ".."
    };
    if !component_ok(owner) || !component_ok(repo) {
        return Ok(None);
    }
    Ok(Some(ParsedSource {
        source: SkillSource::Git {
            url: format!("https://github.com/{owner}/{repo}"),
            ref_: frag_ref,
            subpath: None,
        },
        skill_alias: alias,
    }))
}

fn is_generic_remote(s: &str) -> bool {
    // scp-style: user@host:path (the conventional bare-user form).
    let scp_style = s.split_once('@').is_some_and(|(user, rest)| {
        !user.is_empty() && !user.contains([':', '/']) && rest.contains(':')
    });
    scp_style || s.starts_with("ssh://") || s.starts_with("file://") || s.ends_with(".git")
}

/// Reject password-bearing userinfo everywhere, and ANY userinfo on https —
/// tokens ride the user slot too, and https auth belongs in a credential
/// helper, not a manifest. Bare-user ssh/scp forms (`git@host`) stay valid:
/// user-without-password is transport addressing, not a credential.
fn deny_credentials(url: &str) -> Result<()> {
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    // Userinfo lives in the authority — before the first '/'; an '@' later
    // in the path is just a path byte.
    let authority = after_scheme.split('/').next().unwrap_or(after_scheme);
    let Some((userinfo, _)) = authority.split_once('@') else {
        return Ok(());
    };
    let https = url.starts_with("https://") || url.starts_with("http://");
    if https || userinfo.contains(':') {
        bail!(
            "credentials in git URLs are not accepted — use a git credential helper \
             and pass the URL without userinfo"
        );
    }
    Ok(())
}

/// A `/tree/` or `--subpath` value: relative, normal components only.
pub fn validate_subpath(sub: &str) -> Result<()> {
    use std::path::Component;
    let p = std::path::Path::new(sub);
    if p.components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        && !sub.is_empty()
    {
        Ok(())
    } else {
        bail!(
            "subpath '{}' must be a relative path with no '..'",
            crate::text::sanitize_line(sub)
        )
    }
}

/// Owner/repo path segments in URLs: same shape as the shorthand rule —
/// contains an alphanumeric, never `.`/`..`. Defense in depth behind the
/// spelled-path check that already runs first.
fn validate_component(seg: &str) -> Result<()> {
    let ok = !seg.is_empty()
        && seg != "."
        && seg != ".."
        && seg.chars().any(|c| c.is_ascii_alphanumeric());
    if ok {
        Ok(())
    } else {
        bail!(
            "invalid repo path segment '{}'",
            crate::text::sanitize_line(seg)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(input: &str) -> (String, Option<String>, Option<String>, Option<String>) {
        match parse_source(input).unwrap() {
            ParsedSource {
                source: SkillSource::Git { url, ref_, subpath },
                skill_alias,
            } => (url, ref_, subpath, skill_alias),
            other => panic!("expected git source for {input:?}, got {other:?}"),
        }
    }

    #[test]
    fn grammar_table() {
        // Shorthand — always GitHub, never a filesystem probe.
        assert_eq!(
            git("anthropics/skills").0,
            "https://github.com/anthropics/skills"
        );
        // Shorthand + alias + fragment ref.
        let (url, ref_, _, alias) = git("vercel-labs/agent-skills@pdf#main");
        assert_eq!(url, "https://github.com/vercel-labs/agent-skills");
        assert_eq!(ref_.as_deref(), Some("main"));
        assert_eq!(alias.as_deref(), Some("pdf"));
        // Full URL, .git suffix stripped.
        assert_eq!(
            git("https://github.com/o/r.git").0,
            "https://github.com/o/r"
        );
        // Tree URL with ref and subpath.
        let (url, ref_, sub, _) = git("https://github.com/o/r/tree/main/skills/pdf");
        assert_eq!(url, "https://github.com/o/r");
        assert_eq!(ref_.as_deref(), Some("main"));
        assert_eq!(sub.as_deref(), Some("skills/pdf"));
        // GitLab subgroup + tree marker.
        let (url, ref_, sub, _) = git("https://gitlab.com/grp/sub/repo/-/tree/v2/sk/a");
        assert_eq!(url, "https://gitlab.com/grp/sub/repo");
        assert_eq!(ref_.as_deref(), Some("v2"));
        assert_eq!(sub.as_deref(), Some("sk/a"));
        // Generic remotes.
        assert_eq!(git("git@github.com:o/r.git").0, "git@github.com:o/r.git");
        assert_eq!(git("ssh://git@host/o/r").0, "ssh://git@host/o/r");
        assert_eq!(git("file:///tmp/repo").0, "file:///tmp/repo");
        assert_eq!(
            git("https://host.example/o/r.git").0,
            "https://host.example/o/r.git"
        );

        // Local — must be spelled.
        for p in ["./skill", "../skill", "/abs/skill", "~/skill", ".", ".."] {
            assert!(
                matches!(parse_source(p).unwrap().source, SkillSource::Local { .. }),
                "{p:?} should be local"
            );
        }
    }

    #[test]
    fn grammar_rejects() {
        // Bare name, deep shorthand, dot components.
        for bad in [
            "skills",
            "o/r/extra/deep@x", // not shorthand (3 segs), not a URL → unrecognized
            "../x@y",           // spelled path wins; '@' never parsed — this IS local
        ] {
            let r = parse_source(bad);
            if bad == "../x@y" {
                assert!(matches!(r.unwrap().source, SkillSource::Local { .. }));
            } else {
                assert!(r.is_err(), "{bad:?} should be rejected");
            }
        }
        // '.' / '..' can never reach the shorthand branch.
        assert!(parse_source("../x").is_ok_and(|p| matches!(p.source, SkillSource::Local { .. })));
        // Credentials.
        assert!(parse_source("https://user:tok@github.com/o/r").is_err());
        assert!(parse_source("https://token@github.com/o/r").is_err());
        assert!(parse_source("ssh://user:pw@host/o/r.git").is_err());
        // Bare-user ssh stays fine (checked in grammar_table too).
        assert!(parse_source("git@github.com:o/r.git").is_ok());
        // Cleartext http.
        assert!(parse_source("http://github.com/o/r").is_err());
        // Traversing subpath in a tree URL.
        assert!(parse_source("https://github.com/o/r/tree/main/../etc").is_err());
        // Disagreeing tree-ref and fragment-ref.
        assert!(parse_source("https://github.com/o/r/tree/main#dev").is_err());
        // Agreeing ones are fine.
        assert!(parse_source("https://github.com/o/r/tree/main#main").is_ok());
        // Exotic transports.
        assert!(parse_source("ext::sh -c whoami.git").is_err());
    }
}
