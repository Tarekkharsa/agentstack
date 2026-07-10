//! The embedded starter catalog (registry v0): a curated list of well-known MCP
//! servers that `search` matches against and turns into `add` suggestions. This
//! grows into the git-index registry (PLAN §9d).

use include_dir::{include_dir, Dir};
use serde::Deserialize;

const CATALOG_YAML: &str = include_str!("../catalog/catalog.yaml");

/// The bundled catalog asset tree (skill dirs + instruction markdown), embedded
/// in the binary the same way adapter descriptors are (see
/// [`crate::adapter::registry`]). Packs extract members out of this at install.
static CATALOG_ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/catalog");

#[derive(Debug, Clone, Deserialize)]
pub struct CatalogEntry {
    pub name: String,
    pub kind: String,
    pub description: String,
    /// Human-friendly display name (e.g. `Linear`). Falls back to `name`.
    #[serde(default)]
    pub display: Option<String>,
    /// Vendor homepage / docs URL.
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Env var names this server needs (become `${NAME}` references).
    #[serde(default)]
    pub env: Vec<String>,
    /// Header names that need a secret (e.g. `Authorization`).
    #[serde(default)]
    pub headers: Vec<String>,
    /// Local asset path for a standalone `kind: skill` entry (relative to
    /// `catalog/`).
    #[serde(default)]
    pub path: Option<String>,
    /// Pack member: the vendor's MCP server (nested under `kind: pack`).
    #[serde(default)]
    pub server: Option<CatalogServer>,
    /// Pack members: bundled skill(s).
    #[serde(default)]
    pub skills: Vec<CatalogSkill>,
    /// Pack members: bundled instruction fragment(s).
    #[serde(default)]
    pub instructions: Vec<CatalogInstruction>,
    /// Pack: adapter ids the members target; `["*"]` (default) = all.
    #[serde(default)]
    pub targets: Vec<String>,
}

/// The nested `server:` block of a `kind: pack` entry.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogServer {
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub headers: Vec<String>,
}

/// A bundled skill member of a pack (or a standalone skill via the top-level
/// `path`/`git` fields). `path` is relative to `catalog/`.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogSkill {
    pub name: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub git: Option<String>,
    #[serde(default)]
    pub rev: Option<String>,
}

/// A bundled instruction member of a pack. `path` is an embedded asset path
/// relative to `catalog/`.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogInstruction {
    pub name: String,
    pub path: String,
}

/// Extract an embedded asset (a skill directory) from the catalog tree into
/// `dest` on disk, writing every file recursively. Used by `add_pack`.
pub fn extract_asset_dir(asset_path: &str, dest: &std::path::Path) -> anyhow::Result<()> {
    use anyhow::Context;
    let dir = CATALOG_ASSETS
        .get_dir(asset_path)
        .with_context(|| format!("bundled asset dir '{asset_path}' not found in the catalog"))?;
    write_dir_recursive(dir, asset_path, dest)
}

fn write_dir_recursive(dir: &Dir<'_>, strip: &str, dest: &std::path::Path) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::fs;
    for file in dir.files() {
        let rel = file
            .path()
            .strip_prefix(strip)
            .unwrap_or_else(|_| file.path());
        let out = dest.join(rel);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&out, file.contents()).with_context(|| format!("writing {}", out.display()))?;
    }
    for sub in dir.dirs() {
        write_dir_recursive(sub, strip, dest)?;
    }
    Ok(())
}

/// Read an embedded asset file (an instruction fragment) from the catalog tree.
pub fn read_asset_file(asset_path: &str) -> anyhow::Result<String> {
    use anyhow::Context;
    let file = CATALOG_ASSETS
        .get_file(asset_path)
        .with_context(|| format!("bundled asset file '{asset_path}' not found in the catalog"))?;
    file.contents_utf8()
        .map(str::to_string)
        .with_context(|| format!("bundled asset '{asset_path}' is not valid UTF-8"))
}

impl CatalogEntry {
    /// Whether this entry matches a free-text query (name/description/tags).
    pub fn matches(&self, query: &str) -> bool {
        let q = query.to_ascii_lowercase();
        self.name.to_ascii_lowercase().contains(&q)
            || self.description.to_ascii_lowercase().contains(&q)
            || self
                .tags
                .iter()
                .any(|t| t.to_ascii_lowercase().contains(&q))
    }

    /// A copy-pasteable `agentstack add` command for this entry.
    pub fn add_command(&self) -> String {
        let mut parts = vec![format!("agentstack add server {}", self.name)];
        match self.transport.as_deref() {
            Some("http") => {
                if let Some(url) = &self.url {
                    parts.push(format!("--url {url}"));
                }
                for h in &self.headers {
                    let refname =
                        format!("{}_TOKEN", self.name.to_ascii_uppercase().replace('-', "_"));
                    parts.push(format!("--header '{h}=Bearer ${{{refname}}}'"));
                }
            }
            _ => {
                parts.push("--type stdio".into());
                if let Some(cmd) = &self.command {
                    parts.push(format!("--command {cmd}"));
                }
                for a in &self.args {
                    parts.push(format!("--arg {a}"));
                }
                for e in &self.env {
                    parts.push(format!("--env '{e}=${{{e}}}'"));
                }
            }
        }
        parts.join(" ")
    }
}

/// Parse the embedded catalog.
pub fn entries() -> Vec<CatalogEntry> {
    serde_yaml::from_str(CATALOG_YAML).expect("embedded catalog.yaml is valid")
}

/// Catalog entries matching `query` (all entries if the query is empty).
pub fn search(query: &str) -> Vec<CatalogEntry> {
    entries()
        .into_iter()
        .filter(|e| query.is_empty() || e.matches(query))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parses_and_is_nonempty() {
        let all = entries();
        assert!(all.len() >= 5);
        assert!(all.iter().any(|e| e.name == "github"));
    }

    #[test]
    fn search_matches_name_and_tags() {
        assert!(search("github").iter().any(|e| e.name == "github"));
        assert!(search("sql").iter().any(|e| e.name == "postgres")); // via tag
        assert!(search("zzz-nope").is_empty());
    }

    #[test]
    fn parses_pack_and_standalone_skill_entries() {
        let all = entries();
        let pack = all.iter().find(|e| e.name == "linear-pack").unwrap();
        assert_eq!(pack.kind, "pack");
        assert_eq!(pack.display.as_deref(), Some("Linear"));
        assert_eq!(pack.skills[0].name, "linear_breakdown");
        assert_eq!(pack.instructions[0].name, "linear_rules");
        let server = pack.server.as_ref().unwrap();
        assert_eq!(server.transport.as_deref(), Some("http"));
        assert_eq!(server.headers, vec!["Authorization".to_string()]);

        let skill = all.iter().find(|e| e.name == "pr-triage").unwrap();
        assert_eq!(skill.kind, "skill");
        assert_eq!(skill.path.as_deref(), Some("skills/pr-triage"));
    }

    #[test]
    fn extracts_embedded_skill_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("breakdown");
        extract_asset_dir("skills/linear/breakdown", &dest).unwrap();
        assert!(dest.join("SKILL.md").is_file());
    }

    #[test]
    fn reads_embedded_instruction_file() {
        let text = read_asset_file("instructions/linear/rules.md").unwrap();
        assert!(!text.trim().is_empty());
    }

    #[test]
    fn add_command_shapes_per_transport() {
        let http = search("linear").into_iter().next().unwrap();
        assert!(http
            .add_command()
            .contains("--url https://mcp.linear.app/mcp"));
        let stdio = search("github").into_iter().next().unwrap();
        assert!(stdio.add_command().contains("--type stdio"));
        assert!(stdio.add_command().contains("GITHUB_TOKEN"));
    }
}
