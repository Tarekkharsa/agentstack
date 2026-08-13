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
/// conflict (name returned) and dropped. Per-target `extra` keys are unioned
/// rather than compared — each CLI contributes its own extras (Codex's
/// `startup_timeout_sec` must not make the Codex copy "conflict" with the
/// otherwise-identical Claude copy).
pub fn merge_servers(
    acc: &mut IndexMap<String, Server>,
    incoming: Vec<(String, Server)>,
) -> Vec<String> {
    let mut conflicts = Vec::new();
    for (name, server) in incoming {
        match acc.get_mut(&name) {
            Some(existing) if !same_ignoring_extra(existing, &server) => conflicts.push(name),
            Some(existing) => {
                for (target, fields) in server.extra {
                    existing.extra.entry(target).or_insert(fields);
                }
            }
            None => {
                acc.insert(name, server);
            }
        }
    }
    conflicts
}

/// Structural equality over the transport-neutral fields (everything but
/// `extra`).
fn same_ignoring_extra(a: &Server, b: &Server) -> bool {
    a.server_type == b.server_type
        && a.url == b.url
        && a.command == b.command
        && a.args == b.args
        && a.headers == b.headers
        && a.env == b.env
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

/// One CLI's native config found on disk, with the servers it declares that the
/// manifest does not.
///
/// This is the reading behind "you already have a setup here". Before it, three
/// surfaces asked the question three ways: `adopt` looked at the right files,
/// while `status` and `init` asked `detected()` — a machine-scope answer — and
/// so reported "none detected on this machine" over a `.mcp.json` sitting in
/// the working directory. One reading, three callers.
#[derive(Debug, Clone)]
pub struct NativeConfig {
    pub id: String,
    pub display: String,
    pub scope: crate::scope::Scope,
    pub path: std::path::PathBuf,
    /// Server names the file declares. Empty means the file exists but has no
    /// servers we can read — still worth naming, never worth an alarm.
    pub servers: Vec<String>,
    /// The subset of `servers` absent from the manifest passed to
    /// [`native_configs`] (all of `servers` when there is no manifest yet).
    pub unimported: Vec<String>,
}

/// Every native config present under `dir` (and, when `include_global`, on this
/// machine) with the servers it declares.
///
/// Unreadable or unparseable files are skipped rather than failing the caller:
/// this feeds orientation and diagnosis, and a broken third-party config is a
/// thing to survive, not to die on. `doctor`'s own parse checks still report
/// it. Repository content is hostile input (invariant 7) — nothing here is
/// executed or interpolated, only counted and named.
/// The set of server names a manifest covers: its own inline `[servers.*]`
/// keys **plus** every name its toolsets reference, whether that name is
/// satisfied inline or by a linked library source.
///
/// Referencing a library server by name has always been a way to declare it,
/// and library-first `init` made it the common one — so a coverage check that
/// only looked at inline keys would tell a perfectly managed project that its
/// own servers are "not in this manifest".
pub fn declared_server_names(
    manifest: &crate::manifest::Manifest,
) -> std::collections::HashSet<String> {
    let mut names: std::collections::HashSet<String> = manifest.servers.keys().cloned().collect();
    for name in crate::resolve::runtime_server_names(manifest, None) {
        names.insert(crate::sources::capability_name(&name).to_string());
    }
    names
}

pub fn native_configs(
    registry: &crate::adapter::Registry,
    dir: &std::path::Path,
    manifest_servers: &IndexMap<String, Server>,
    include_global: bool,
) -> Vec<NativeConfig> {
    let declared: std::collections::HashSet<String> = manifest_servers.keys().cloned().collect();
    native_configs_with(registry, dir, &declared, include_global)
}

/// [`native_configs`] against an explicit set of covered names — what a caller
/// that resolved name references (see [`declared_server_names`]) passes.
pub fn native_configs_with(
    registry: &crate::adapter::Registry,
    dir: &std::path::Path,
    manifest_servers: &std::collections::HashSet<String>,
    include_global: bool,
) -> Vec<NativeConfig> {
    use crate::scope::Scope;
    let mut out = Vec::new();
    let scopes: &[Scope] = if include_global {
        &[Scope::Project, Scope::Global]
    } else {
        &[Scope::Project]
    };
    for desc in registry.iter() {
        for scope in scopes {
            let Some((path, _)) = desc.config_for(*scope, dir) else {
                continue;
            };
            if !path.exists() {
                continue;
            }
            let Ok(Some(value)) = desc.read_config_value_for(*scope, dir) else {
                continue;
            };
            let extracted = crate::adapter::extract_servers(desc, &value);
            let servers: Vec<String> = extracted.iter().map(|(name, _)| name.clone()).collect();
            // Our OWN bridge registration is never something to import. It is
            // the control plane `gateway connect` writes into each harness's
            // global config, and asking the user to adopt it would ask the
            // project to serve the gateway.
            //
            // `abandoned_render_is_named.rs` already holds this for the
            // foreign-FILE detector; this is the second reading of the same
            // disk — the per-server count — and it had no such exclusion, so
            // the machine `up` had just bootstrapped reported the bridge as
            // "1 server configured here, not in this setup". Same recognizer
            // as `connect`, so a renamed registration is caught by its argv
            // shape too.
            let unimported: Vec<String> = extracted
                .iter()
                .filter(|(name, server)| {
                    !manifest_servers.contains(name)
                        && !crate::commands::connect::is_bridge_entry(name, server)
                })
                .map(|(name, _)| name.clone())
                .collect();
            out.push(NativeConfig {
                id: desc.id.clone(),
                display: desc.display.clone(),
                scope: *scope,
                path,
                servers,
                unimported,
            });
        }
    }
    out
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
    fn merge_unions_extras_instead_of_conflicting() {
        // The same server imported from Claude (no extras) and Codex (with
        // startup_timeout_sec) is one definition, not a conflict.
        let plain: Server = toml::from_str("type = \"stdio\"\ncommand = \"npx\"").unwrap();
        let with_extra: Server = toml::from_str(
            "type = \"stdio\"\ncommand = \"npx\"\n[extra.codex]\nstartup_timeout_sec = 20",
        )
        .unwrap();

        let mut acc = IndexMap::new();
        assert!(merge_servers(&mut acc, vec![("miro".into(), plain)]).is_empty());
        assert!(merge_servers(&mut acc, vec![("miro".into(), with_extra)]).is_empty());
        assert_eq!(
            acc["miro"].extra["codex"]["startup_timeout_sec"],
            serde_json::json!(20)
        );
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
