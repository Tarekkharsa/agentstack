//! The embedded starter catalog (registry v0): a curated list of well-known MCP
//! servers that `search` matches against and turns into `add` suggestions. This
//! grows into the git-index registry (PLAN §9d).

use serde::Deserialize;

const CATALOG_YAML: &str = include_str!("../catalog/catalog.yaml");

#[derive(Debug, Clone, Deserialize)]
pub struct CatalogEntry {
    pub name: String,
    pub kind: String,
    pub description: String,
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
