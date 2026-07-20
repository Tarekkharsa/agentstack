//! The official MCP Registry (`registry.modelcontextprotocol.io`) as a provider
//! (PLAN §9h, D13). Read-only HTTP; degrades to empty results when offline so
//! the catalog still works.

use std::time::Duration;

use serde::Deserialize;

use super::{clean_name, Candidate, CandidateKind, Install, Provider};

const DEFAULT_BASE: &str = "https://registry.modelcontextprotocol.io";

pub struct RegistryProvider {
    base: String,
}

impl Default for RegistryProvider {
    fn default() -> Self {
        // Allow override for tests / private mirrors.
        let base = std::env::var("AGENTSTACK_REGISTRY_URL").unwrap_or_else(|_| DEFAULT_BASE.into());
        RegistryProvider { base }
    }
}

impl Provider for RegistryProvider {
    fn id(&self) -> &'static str {
        "registry"
    }
    fn search(&self, query: &str, limit: usize) -> Vec<Candidate> {
        self.try_search(query, limit).unwrap_or_default()
    }
}

impl RegistryProvider {
    fn try_search(&self, query: &str, limit: usize) -> Option<Vec<Candidate>> {
        if query.trim().is_empty() {
            return Some(Vec::new());
        }
        let url = format!(
            "{}/v0/servers?search={}&limit={}",
            self.base,
            urlencode(query),
            limit
        );
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()
            .ok()?;
        let resp = client.get(&url).send().ok()?;
        if !resp.status().is_success() {
            return Some(Vec::new());
        }
        let body: ApiResponse = resp.json().ok()?;
        Some(body.servers.into_iter().filter_map(to_candidate).collect())
    }
}

fn to_candidate(entry: ApiEntry) -> Option<Candidate> {
    let s = entry.server;
    let name = clean_name(&s.name);
    if name.is_empty() {
        return None;
    }
    // Live remote HTTP text headed for terminals and MCP results — sanitized
    // once at ingestion so every consumer inherits the clean value
    // (design §A.2 #3).
    let description = crate::text::sanitize_line(&if s.description.is_empty() {
        s.title.unwrap_or_default()
    } else {
        s.description
    });

    // Prefer a remote (HTTP) install; else derive a stdio package install.
    if let Some(remote) = s.remotes.into_iter().next() {
        let secret_headers = remote
            .headers
            .into_iter()
            .filter_map(|h| h.name)
            .collect::<Vec<_>>();
        return Some(Candidate {
            id: s.name,
            name,
            description,
            source: "registry",
            kind: CandidateKind::Server(Install::Http {
                url: remote.url,
                secret_headers,
            }),
        });
    }

    let pkg = s.packages.into_iter().next()?;
    let (command, mut args) = match pkg.registry_type.as_str() {
        "npm" => ("npx".to_string(), vec!["-y".to_string()]),
        "pypi" => ("uvx".to_string(), vec![]),
        _ => return None, // oci/nuget/etc. — skip auto-install for now
    };
    let ident = if pkg.version.is_empty() {
        pkg.identifier.clone()
    } else {
        format!("{}@{}", pkg.identifier, pkg.version)
    };
    if !ident.is_empty() {
        args.push(ident);
    }
    let secret_env = pkg
        .environment_variables
        .into_iter()
        .filter_map(|e| e.name)
        .collect::<Vec<_>>();
    Some(Candidate {
        id: s.name,
        name,
        description,
        source: "registry",
        kind: CandidateKind::Server(Install::Stdio {
            command,
            args,
            secret_env,
        }),
    })
}

fn urlencode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".to_string(),
            other => format!("%{:02X}", other as u32),
        })
        .collect()
}

#[derive(Deserialize)]
struct ApiResponse {
    #[serde(default)]
    servers: Vec<ApiEntry>,
}

#[derive(Deserialize)]
struct ApiEntry {
    server: ApiServer,
}

#[derive(Deserialize)]
struct ApiServer {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    remotes: Vec<ApiRemote>,
    #[serde(default)]
    packages: Vec<ApiPackage>,
}

#[derive(Deserialize)]
struct ApiRemote {
    url: String,
    #[serde(default)]
    headers: Vec<ApiInput>,
}

#[derive(Deserialize)]
struct ApiPackage {
    #[serde(default, alias = "registryType")]
    registry_type: String,
    #[serde(default)]
    identifier: String,
    #[serde(default)]
    version: String,
    #[serde(default, alias = "environmentVariables")]
    environment_variables: Vec<ApiInput>,
}

#[derive(Deserialize)]
struct ApiInput {
    #[serde(default)]
    name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remote_and_package_servers() {
        let json = r#"{ "servers": [
            { "server": { "name": "io.github.x/remote-srv", "description": "a remote",
                "remotes": [{ "type": "streamable-http", "url": "https://x/mcp",
                    "headers": [{ "name": "Authorization" }] }] } },
            { "server": { "name": "io.github.y/npm-srv", "description": "an npm one",
                "packages": [{ "registryType": "npm", "identifier": "@a/b", "version": "1.2.3",
                    "environmentVariables": [{ "name": "API_KEY" }] }] } }
        ] }"#;
        let body: ApiResponse = serde_json::from_str(json).unwrap();
        let cands: Vec<Candidate> = body.servers.into_iter().filter_map(to_candidate).collect();
        assert_eq!(cands.len(), 2);

        let remote = &cands[0];
        assert_eq!(remote.name, "remote-srv");
        match &remote.kind {
            CandidateKind::Server(Install::Http {
                url,
                secret_headers,
            }) => {
                assert_eq!(url, "https://x/mcp");
                assert_eq!(secret_headers, &vec!["Authorization".to_string()]);
            }
            _ => panic!("expected http"),
        }

        let npm = &cands[1];
        match &npm.kind {
            CandidateKind::Server(Install::Stdio {
                command,
                args,
                secret_env,
            }) => {
                assert_eq!(command, "npx");
                assert_eq!(args, &vec!["-y".to_string(), "@a/b@1.2.3".to_string()]);
                assert_eq!(secret_env, &vec!["API_KEY".to_string()]);
            }
            _ => panic!("expected stdio"),
        }
    }
}
