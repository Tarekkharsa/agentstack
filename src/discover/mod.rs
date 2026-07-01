//! Discovery helpers for `init`: merge imported servers across CLIs and lift
//! inline secret literals into `${REF}`s so the resulting manifest is
//! commit-safe.

use indexmap::IndexMap;

use crate::manifest::Server;

/// A secret value lifted out of a config, to be stored under `reference`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lifted {
    pub reference: String,
    pub value: String,
    /// Where the plaintext value was found, e.g. `server 'github' (env GITHUB_TOKEN)`.
    pub origin: String,
}

/// Merge `incoming` servers (from one target) into `acc`. First definition of a
/// name wins; a later, structurally-different definition is reported as a
/// conflict (name returned) and dropped.
pub fn merge_servers(
    acc: &mut IndexMap<String, Server>,
    incoming: Vec<(String, Server)>,
) -> Vec<String> {
    let mut conflicts = Vec::new();
    for (name, server) in incoming {
        match acc.get(&name) {
            Some(existing) if existing != &server => conflicts.push(name),
            Some(_) => {}
            None => {
                acc.insert(name, server);
            }
        }
    }
    conflicts
}

/// Replace inline secret literals in `servers` with `${REF}` references,
/// returning the values to store. Idempotent: values already in `${...}` form
/// are left alone.
pub fn lift_secrets(servers: &mut IndexMap<String, Server>) -> Vec<Lifted> {
    let mut lifted: Vec<Lifted> = Vec::new();

    for (name, server) in servers.iter_mut() {
        // Headers: lift auth-ish values, preserving any scheme prefix
        // ("Bearer "/"Basic ").
        for (key, val) in server.headers.iter_mut() {
            if contains_ref(val) || !header_is_secret(key, val) {
                continue;
            }
            let (prefix, secret) = split_scheme(val);
            if secret.is_empty() {
                continue;
            }
            let origin = format!("server '{name}' (header {key})");
            let reference = unique_ref(
                &format!("{}_TOKEN", sanitize(name)),
                secret,
                origin,
                &mut lifted,
            );
            *val = format!("{prefix}${{{reference}}}");
        }

        // Env: lift secret-ish values. The env key is already a good ref name
        // (e.g. GITHUB_TOKEN).
        for (key, val) in server.env.iter_mut() {
            if contains_ref(val) || !env_is_secret(key, val) {
                continue;
            }
            let origin = format!("server '{name}' (env {key})");
            let reference = unique_ref(key, val, origin, &mut lifted);
            *val = format!("${{{reference}}}");
        }
    }

    lifted
}

fn contains_ref(s: &str) -> bool {
    s.contains("${")
}

/// Pick a reference name that doesn't collide with a different value already
/// lifted. Records the (reference, value, origin) triple.
fn unique_ref(base: &str, value: &str, origin: String, lifted: &mut Vec<Lifted>) -> String {
    // Reuse an existing reference if it holds the same value.
    if let Some(l) = lifted.iter().find(|l| l.value == value) {
        return l.reference.clone();
    }
    let mut candidate = base.to_string();
    let mut n = 2;
    while lifted.iter().any(|l| l.reference == candidate) {
        candidate = format!("{base}_{n}");
        n += 1;
    }
    lifted.push(Lifted {
        reference: candidate.clone(),
        value: value.to_string(),
        origin,
    });
    candidate
}

/// Split a "Bearer xyz" / "Basic xyz" value into its scheme prefix (incl.
/// trailing space) and the secret. No scheme → ("", whole value).
fn split_scheme(val: &str) -> (String, &str) {
    for scheme in ["Bearer ", "Basic ", "Token ", "token "] {
        if let Some(rest) = val.strip_prefix(scheme) {
            return (scheme.to_string(), rest);
        }
    }
    (String::new(), val)
}

fn header_is_secret(key: &str, val: &str) -> bool {
    let k = key.to_ascii_lowercase();
    let auth_key = k == "authorization"
        || k.contains("api-key")
        || k.contains("api_key")
        || k.contains("apikey")
        || k.contains("token")
        || k.contains("secret");
    let (_, secret) = split_scheme(val);
    auth_key && secret.len() >= 6
}

fn env_is_secret(key: &str, val: &str) -> bool {
    let k = key.to_ascii_uppercase();
    let secret_key = [
        "TOKEN",
        "SECRET",
        "KEY",
        "PASSWORD",
        "PASS",
        "PAT",
        "CREDENTIAL",
    ]
    .iter()
    .any(|kw| k.contains(kw));
    // Avoid lifting obvious non-secrets (paths, urls, short values).
    let looks_value = val.len() >= 6 && !val.starts_with('/') && !val.contains("://");
    secret_key && looks_value
}

/// Turn a server name into an uppercase, identifier-safe ref base.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http_server(headers: &[(&str, &str)]) -> Server {
        let mut s: Server = toml::from_str("type = \"http\"\nurl = \"https://x\"").unwrap();
        for (k, v) in headers {
            s.headers.insert(k.to_string(), v.to_string());
        }
        s
    }

    #[test]
    fn lifts_bearer_header_preserving_scheme() {
        let mut servers = IndexMap::new();
        servers.insert(
            "kibana_mcp".to_string(),
            http_server(&[("Authorization", "Bearer test-token-local")]),
        );
        let lifted = lift_secrets(&mut servers);
        assert_eq!(lifted.len(), 1);
        assert_eq!(lifted[0].reference, "KIBANA_MCP_TOKEN");
        assert_eq!(lifted[0].value, "test-token-local");
        assert_eq!(
            servers["kibana_mcp"].headers["Authorization"],
            "Bearer ${KIBANA_MCP_TOKEN}"
        );
    }

    #[test]
    fn lifts_env_using_key_as_ref() {
        let mut s: Server = toml::from_str("type = \"stdio\"\ncommand = \"npx\"").unwrap();
        s.env
            .insert("GITHUB_TOKEN".into(), "ghp_secretvalue".into());
        s.env.insert("PORT".into(), "8080".into());
        let mut servers = IndexMap::new();
        servers.insert("github".to_string(), s);
        let lifted = lift_secrets(&mut servers);
        assert_eq!(
            lifted,
            vec![Lifted {
                reference: "GITHUB_TOKEN".into(),
                value: "ghp_secretvalue".into(),
                origin: "server 'github' (env GITHUB_TOKEN)".into(),
            }]
        );
        assert_eq!(servers["github"].env["GITHUB_TOKEN"], "${GITHUB_TOKEN}");
        assert_eq!(servers["github"].env["PORT"], "8080"); // untouched
    }

    #[test]
    fn does_not_relift_existing_reference() {
        let mut servers = IndexMap::new();
        servers.insert(
            "k".to_string(),
            http_server(&[("Authorization", "Bearer ${KIBANA_TOKEN}")]),
        );
        assert!(lift_secrets(&mut servers).is_empty());
    }

    #[test]
    fn merge_detects_conflicts() {
        let mut acc = IndexMap::new();
        let conflicts = merge_servers(
            &mut acc,
            vec![(
                "k".to_string(),
                http_server(&[("Authorization", "Bearer a")]),
            )],
        );
        assert!(conflicts.is_empty());
        // Same name, different content → conflict, original kept.
        let conflicts = merge_servers(
            &mut acc,
            vec![(
                "k".to_string(),
                http_server(&[("Authorization", "Bearer DIFFERENT")]),
            )],
        );
        assert_eq!(conflicts, vec!["k".to_string()]);
        assert_eq!(acc["k"].headers["Authorization"], "Bearer a");
    }
}
