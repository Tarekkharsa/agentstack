//! Browse the Pi package marketplace (pi.dev/packages) from agentstack.
//!
//! Pi packages (extensions / skills / prompts / themes) are published to npm
//! with the `pi-package` keyword and installed via `pi install npm:<name>`.
//! There's no bespoke pi.dev API, so we query the public npm registry search —
//! the same data pi.dev surfaces.

use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;

const NPM_SEARCH: &str = "https://registry.npmjs.org/-/v1/search";

/// One Pi package from the marketplace.
#[derive(Debug, Clone)]
pub struct PiPackage {
    pub name: String,
    pub version: String,
    pub description: String,
    /// extension / skill / prompt / theme / package (inferred from keywords).
    pub kind: String,
    pub npm_url: String,
    pub repo_url: Option<String>,
    /// The command a user runs to install it.
    pub install: String,
}

#[derive(Deserialize)]
struct SearchResp {
    #[serde(default)]
    objects: Vec<Obj>,
}
#[derive(Deserialize)]
struct Obj {
    package: Pkg,
}
#[derive(Deserialize)]
struct Pkg {
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    links: Links,
}
#[derive(Deserialize, Default)]
struct Links {
    #[serde(default)]
    npm: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
}

/// Search the marketplace. Empty query lists popular `pi-package`s; otherwise it
/// narrows within that keyword. Returns at most `limit` results.
pub fn search(query: &str, limit: usize) -> Result<Vec<PiPackage>> {
    let text = if query.trim().is_empty() {
        "keywords:pi-package".to_string()
    } else {
        format!("keywords:pi-package {}", query.trim())
    };
    let url = format!("{NPM_SEARCH}?text={}&size={}", urlencode(&text), limit);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()?;
    let resp = client.get(&url).header("User-Agent", "agentstack").send()?;
    if !resp.status().is_success() {
        return Ok(Vec::new());
    }
    let body: SearchResp = resp.json()?;
    Ok(body
        .objects
        .into_iter()
        .map(|o| to_pkg(o.package))
        .collect())
}

fn to_pkg(p: Pkg) -> PiPackage {
    PiPackage {
        kind: infer_kind(&p.keywords),
        npm_url: p
            .links
            .npm
            .clone()
            .unwrap_or_else(|| format!("https://www.npmjs.com/package/{}", p.name)),
        repo_url: p.links.repository.or(p.links.homepage),
        install: format!("pi install npm:{}", p.name),
        name: p.name,
        version: p.version,
        description: p.description,
    }
}

fn infer_kind(keywords: &[String]) -> String {
    let has = |k: &str| keywords.iter().any(|x| x.eq_ignore_ascii_case(k));
    if has("pi-extension") || has("extension") {
        "extension"
    } else if has("pi-skill") || has("skill") {
        "skill"
    } else if has("pi-prompt") || has("prompt") {
        "prompt"
    } else if has("pi-theme") || has("theme") {
        "theme"
    } else {
        "package"
    }
    .to_string()
}

/// Minimal percent-encoding for a query string value.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_kind_from_keywords() {
        assert_eq!(
            infer_kind(&["pi-package".into(), "extension".into()]),
            "extension"
        );
        assert_eq!(infer_kind(&["pi-package".into(), "theme".into()]), "theme");
        assert_eq!(infer_kind(&["pi-package".into()]), "package");
    }

    #[test]
    fn urlencodes_spaces_and_colons() {
        assert_eq!(
            urlencode("keywords:pi-package web"),
            "keywords%3Api-package%20web"
        );
    }
}
