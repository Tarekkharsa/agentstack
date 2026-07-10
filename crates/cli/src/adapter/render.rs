//! Generic renderer: turn a manifest [`Server`] into a target-shaped value tree
//! according to an [`AdapterDescriptor`]. The output is a `serde_json::Value`
//! object whose keys are already the target's field names; the format-specific
//! mergers ([`crate::render`]) then write it as JSON or TOML.

use serde_json::{Map, Value};

use super::descriptor::{AdapterDescriptor, SecretMode};
use crate::manifest::{Server, ServerType};
use crate::secret::{Lookup, Resolver};

/// A rendered server entry plus any secret references that could not be
/// resolved on this machine (surfaced by `doctor`/`apply`).
pub struct Rendered {
    /// The server body, keyed by the server name.
    pub value: Value,
    /// `${REF}`s no secret store has (genuinely not set).
    pub unresolved: Vec<String>,
    /// `${REF}`s a store errored on while reading — `(name, why)`. Not the
    /// same as unresolved: the secret may well be set, the read failed.
    pub failed: Vec<(String, String)>,
    /// Whether this adapter's config format can represent this server's
    /// transport at all. `false` means the descriptor maps no field for the
    /// server's defining attribute (e.g. an HTTP server for the stdio-only
    /// Claude Desktop config) — callers must skip it rather than write an empty
    /// `{}` entry into a real config file.
    pub representable: bool,
    /// Every `${REF}` actually resolved for this render, as `(ref-name, value)`.
    /// The real values are still in `value` (that's what gets written); this set
    /// lets the display layer redact them from the human-facing diff/apply
    /// preview so a resolved secret is never printed in cleartext.
    pub secrets: Vec<(String, String)>,
}

/// Render one named server for one adapter.
pub fn render_server(
    desc: &AdapterDescriptor,
    server: &Server,
    resolver: &dyn Resolver,
) -> Rendered {
    let mut body: Map<String, Value> = Map::new();
    let mut unresolved: Vec<String> = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    let mut secrets: Vec<(String, String)> = Vec::new();

    // CLIs with no MCP support render nothing.
    let Some(mcp) = desc.mcp.as_ref() else {
        return Rendered {
            value: Value::Object(body),
            unresolved,
            failed,
            representable: false,
            secrets,
        };
    };

    // Can this adapter's config format express this server's transport? The
    // defining field is `url` for HTTP and `command` for stdio; if the descriptor
    // maps neither, the entry would render empty and must be skipped by callers.
    let representable = match server.server_type {
        ServerType::Http => mcp.fields.url.is_some(),
        ServerType::Stdio => mcp.fields.command.is_some(),
    };

    let passthrough = mcp.secret_mode == SecretMode::Passthrough;
    let mut sub = |s: &str| {
        substitute_with(
            s,
            resolver,
            passthrough,
            &mut unresolved,
            &mut failed,
            &mut secrets,
        )
    };

    // 1. Transport tag (e.g. Claude's "type": "http").
    if let Some(t) = &mcp.transport {
        let tag = match server.server_type {
            ServerType::Http => Some(t.http_value.clone()),
            ServerType::Stdio => t.stdio_value.clone(),
        };
        if let Some(tag) = tag {
            body.insert(t.key.clone(), Value::String(tag));
        }
    }

    // 2. url
    if let (Some(field), Some(url)) = (&mcp.fields.url, &server.url) {
        body.insert(field.clone(), Value::String(sub(url)));
    }

    // 3. command (+ args). Some CLIs (e.g. OpenCode) want a single combined
    // array under `command`; others want a command string + separate args array.
    // The resolved (post-substitution) command/args are kept aside so step 5b
    // can rebuild them into a shell wrapper if this adapter can't express `cwd`.
    let mut resolved_command: Option<String> = None;
    let mut resolved_args: Vec<String> = Vec::new();
    if mcp.command_array {
        if let (Some(field), Some(cmd)) = (&mcp.fields.command, &server.command) {
            let cmd_s = sub(cmd);
            let args_s: Vec<String> = server.args.iter().map(|a| sub(a)).collect();
            let mut arr = vec![Value::String(cmd_s.clone())];
            arr.extend(args_s.iter().cloned().map(Value::String));
            body.insert(field.clone(), Value::Array(arr));
            resolved_command = Some(cmd_s);
            resolved_args = args_s;
        }
    } else {
        if let (Some(field), Some(cmd)) = (&mcp.fields.command, &server.command) {
            let cmd_s = sub(cmd);
            body.insert(field.clone(), Value::String(cmd_s.clone()));
            resolved_command = Some(cmd_s);
        }
        // 4. args
        if let Some(field) = &mcp.fields.args {
            if !server.args.is_empty() {
                let args_s: Vec<String> = server.args.iter().map(|a| sub(a)).collect();
                let arr = args_s.iter().cloned().map(Value::String).collect();
                body.insert(field.clone(), Value::Array(arr));
                resolved_args = args_s;
            }
        }
    }

    // 5. cwd (working directory). Only meaningful for stdio servers; rendered
    // to the adapter's native key where one exists.
    let mut cwd_rendered_natively = false;
    if server.server_type == ServerType::Stdio {
        if let (Some(field), Some(cwd)) = (&mcp.fields.cwd, &server.cwd) {
            body.insert(field.clone(), Value::String(sub(cwd)));
            cwd_rendered_natively = true;
        }
    }

    // 5b. Auto-wrap: this adapter has no native `cwd` key, but the server
    // needs one. Rather than silently dropping it, rewrite command/args into
    // a POSIX shell invocation that `cd`s there first — `sh -c "cd <dir> &&
    // exec <cmd> <args...>"` — so the working directory is still honored.
    // Requires an actual resolved command to wrap around; if the manifest
    // somehow lacks one, there is nothing to wrap and cwd is simply dropped.
    if server.server_type == ServerType::Stdio
        && !cwd_rendered_natively
        && mcp.fields.cwd.is_none()
        && server.cwd.is_some()
    {
        if let Some(cmd) = &resolved_command {
            let cwd_s = sub(server.cwd.as_deref().unwrap_or_default());
            let mut shell_cmd = format!(
                "cd {} && exec {}",
                posix_shell_quote(&cwd_s),
                posix_shell_quote(cmd)
            );
            for a in &resolved_args {
                shell_cmd.push(' ');
                shell_cmd.push_str(&posix_shell_quote(a));
            }
            if mcp.command_array {
                if let Some(field) = &mcp.fields.command {
                    body.insert(
                        field.clone(),
                        Value::Array(vec![
                            Value::String("sh".to_string()),
                            Value::String("-c".to_string()),
                            Value::String(shell_cmd),
                        ]),
                    );
                }
            } else {
                if let Some(field) = &mcp.fields.command {
                    body.insert(field.clone(), Value::String("sh".to_string()));
                }
                if let Some(field) = &mcp.fields.args {
                    body.insert(
                        field.clone(),
                        Value::Array(vec![
                            Value::String("-c".to_string()),
                            Value::String(shell_cmd),
                        ]),
                    );
                }
            }
        }
    }

    // 6. headers (nested object)
    if let Some(field) = &mcp.fields.headers {
        if !server.headers.is_empty() {
            let mut h = Map::new();
            for (k, v) in &server.headers {
                h.insert(k.clone(), Value::String(sub(v)));
            }
            body.insert(field.clone(), Value::Object(h));
        }
    }

    // 7. env (nested object)
    if let Some(field) = &mcp.fields.env {
        if !server.env.is_empty() {
            let mut e = Map::new();
            for (k, v) in &server.env {
                e.insert(k.clone(), Value::String(sub(v)));
            }
            body.insert(field.clone(), Value::Object(e));
        }
    }

    // 8. Per-target extras: native keys with no transport-neutral equivalent
    // (e.g. Codex `startup_timeout_sec`), passed through verbatim. Rendered
    // last so a deliberate extra can override a canonical field.
    if let Some(extra) = server.extra.get(&desc.id) {
        for (k, v) in extra {
            body.insert(
                k.clone(),
                substitute_value(
                    v,
                    resolver,
                    passthrough,
                    &mut unresolved,
                    &mut failed,
                    &mut secrets,
                ),
            );
        }
    }

    // De-duplicate refs while keeping first-seen order.
    unresolved.dedup();
    failed.dedup_by(|a, b| a.0 == b.0);
    secrets.dedup();

    Rendered {
        value: Value::Object(body),
        unresolved,
        failed,
        representable,
        secrets,
    }
}

/// POSIX single-quote a string for safe use inside a `sh -c "..."` argument:
/// wrap it in single quotes, escaping any embedded `'` as `'\''` (close the
/// quote, emit an escaped literal quote, reopen the quote). Every other
/// character — spaces, `$`, `&&`, etc. — is inert inside single quotes, so no
/// further escaping is needed.
fn posix_shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Substitute `${NAME}` refs in every string leaf of an extras value; numbers,
/// bools, and arrays pass through untouched.
fn substitute_value(
    v: &Value,
    resolver: &dyn Resolver,
    passthrough: bool,
    unresolved: &mut Vec<String>,
    failed: &mut Vec<(String, String)>,
    secrets: &mut Vec<(String, String)>,
) -> Value {
    match v {
        Value::String(s) => Value::String(substitute_with(
            s,
            resolver,
            passthrough,
            unresolved,
            failed,
            secrets,
        )),
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|el| substitute_value(el, resolver, passthrough, unresolved, failed, secrets))
                .collect(),
        ),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, el)| {
                    (
                        k.clone(),
                        substitute_value(el, resolver, passthrough, unresolved, failed, secrets),
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// [`substitute_with`] for callers with a single issue channel (hooks,
/// settings, gateway): read failures fold into `unresolved` with the failure
/// message attached, so they still block writes and read honestly.
pub(crate) fn substitute(
    s: &str,
    resolver: &dyn Resolver,
    passthrough: bool,
    unresolved: &mut Vec<String>,
    secrets: &mut Vec<(String, String)>,
) -> String {
    let mut failed = Vec::new();
    let out = substitute_with(s, resolver, passthrough, unresolved, &mut failed, secrets);
    unresolved.extend(
        failed
            .into_iter()
            .map(|(name, why)| format!("{name} — {why}")),
    );
    out
}

/// Replace every `${NAME}` token in `s`. In passthrough mode the token is left
/// verbatim. Otherwise it is resolved; tokens that don't resolve are recorded
/// and left in place (never silently blanked) — misses in `unresolved`, store
/// read errors in `failed`. Spans that are not valid reference names (shell
/// syntax like `${VAR:-fallback}`) are left verbatim and not recorded.
pub(crate) fn substitute_with(
    s: &str,
    resolver: &dyn Resolver,
    passthrough: bool,
    unresolved: &mut Vec<String>,
    failed: &mut Vec<(String, String)>,
    secrets: &mut Vec<(String, String)>,
) -> String {
    if passthrough || !s.contains("${") {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        if let Some(inner) = rest.strip_prefix("${") {
            if let Some(end) = inner.find('}') {
                let name = &inner[..end];
                if crate::secret::is_ref_name(name) {
                    match resolver.lookup(name) {
                        Lookup::Found(val) => {
                            secrets.push((name.to_string(), val.clone()));
                            out.push_str(&val);
                        }
                        Lookup::Missing => {
                            unresolved.push(name.to_string());
                            out.push_str(&rest[..2 + end + 1]); // keep `${NAME}`
                        }
                        Lookup::Failed(why) => {
                            failed.push((name.to_string(), why));
                            out.push_str(&rest[..2 + end + 1]); // keep `${NAME}`
                        }
                    }
                    i += 2 + end + 1;
                    continue;
                }
            }
            // Shell syntax, not a reference — emit `${` and keep scanning the
            // interior (so `${A:-${B}}` still resolves `B`).
            out.push_str("${");
            i += 2;
            continue;
        }
        let c = rest.chars().next().expect("i is on a char boundary");
        out.push(c);
        i += c.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::Registry;
    use crate::secret::MapResolver;

    fn server(toml_str: &str) -> Server {
        toml::from_str(toml_str).unwrap()
    }

    #[test]
    fn claude_http_gets_type_tag_and_resolves_secret() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("claude-code").unwrap();
        let s = server(
            r#"
            type = "http"
            url = "https://x/mcp"
            headers = { Authorization = "Bearer ${TOK}" }
            "#,
        );
        let resolver = MapResolver::from([("TOK", "secret123")]);
        let r = render_server(desc, &s, &resolver);
        assert_eq!(r.value["type"], "http");
        assert_eq!(r.value["headers"]["Authorization"], "Bearer secret123");
        assert!(r.unresolved.is_empty());
        // The resolved substitution is surfaced so the display layer can redact
        // it — the real value stays in `value` (that's what gets written).
        assert_eq!(
            r.secrets,
            vec![("TOK".to_string(), "secret123".to_string())]
        );
    }

    #[test]
    fn codex_renames_headers_field() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("codex").unwrap();
        let s = server(
            r#"
            type = "http"
            url = "https://x/mcp"
            headers = { Authorization = "Bearer ${TOK}" }
            "#,
        );
        let resolver = MapResolver::from([("TOK", "v")]);
        let r = render_server(desc, &s, &resolver);
        // Codex has no transport tag and renames headers -> http_headers.
        assert!(r.value.get("type").is_none());
        assert!(r.value.get("http_headers").is_some());
    }

    #[test]
    fn opencode_combines_command_and_args_into_one_array() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("opencode").unwrap();
        let s = server(
            r#"
            type = "stdio"
            command = "npx"
            args = ["-y", "some-mcp"]
            env = { TOKEN = "${TOK}" }
            "#,
        );
        let resolver = MapResolver::from([("TOK", "v")]);
        let r = render_server(desc, &s, &resolver);
        // command_array: command+args collapse into a single "command" array,
        // there is no separate "args" key, and env renders under "environment".
        assert_eq!(r.value["type"], "local");
        assert_eq!(
            r.value["command"],
            serde_json::json!(["npx", "-y", "some-mcp"])
        );
        assert!(r.value.get("args").is_none());
        assert_eq!(r.value["environment"]["TOKEN"], "v");
    }

    #[test]
    fn claude_desktop_cannot_represent_http_server() {
        // The stdio-only Claude Desktop config maps no url field: an http server
        // is not representable (callers must skip it, not write an empty entry),
        // while a stdio server renders fine.
        let reg = Registry::load().unwrap();
        let desc = reg.get("claude-desktop").unwrap();
        let resolver = MapResolver::default();

        let http = server("type = \"http\"\nurl = \"https://x/mcp\"\n");
        let r = render_server(desc, &http, &resolver);
        assert!(!r.representable, "http server is unrepresentable here");

        let stdio = server("type = \"stdio\"\ncommand = \"npx\"\n");
        let r = render_server(desc, &stdio, &resolver);
        assert!(r.representable, "stdio server is representable");
    }

    #[test]
    fn extras_render_only_for_their_adapter_and_substitute_strings() {
        let reg = Registry::load().unwrap();
        let s = server(
            r#"
            type = "stdio"
            command = "npx"
            args = ["-y", "some-mcp"]

            [extra.codex]
            startup_timeout_sec = 20
            note = "token ${TOK}"

            [extra.claude-code]
            timeout = 5
            "#,
        );
        let resolver = MapResolver::from([("TOK", "v")]);

        let codex = render_server(reg.get("codex").unwrap(), &s, &resolver);
        assert_eq!(codex.value["startup_timeout_sec"], 20);
        assert_eq!(codex.value["note"], "token v");
        assert!(codex.value.get("timeout").is_none(), "not codex's extra");

        let claude = render_server(reg.get("claude-code").unwrap(), &s, &resolver);
        assert_eq!(claude.value["timeout"], 5);
        assert!(claude.value.get("startup_timeout_sec").is_none());
    }

    #[test]
    fn cwd_renders_to_native_key_for_supporting_adapter_and_wraps_otherwise() {
        let reg = Registry::load().unwrap();
        let s = server(
            r#"
            type = "stdio"
            command = "node"
            args = ["dist/index.js"]
            cwd = "/srv/tldraw"
            "#,
        );
        let resolver = MapResolver::default();

        // Codex maps `cwd` → native `cwd`; command/args are untouched.
        let codex = render_server(reg.get("codex").unwrap(), &s, &resolver);
        assert_eq!(codex.value["cwd"], "/srv/tldraw");
        assert_eq!(codex.value["command"], "node");

        // Claude Code's config format has no working-directory key: instead of
        // dropping cwd, the command is auto-wrapped in a shell that `cd`s there
        // first.
        let claude = render_server(reg.get("claude-code").unwrap(), &s, &resolver);
        assert!(claude.value.get("cwd").is_none());
        assert_eq!(claude.value["command"], "sh");
        assert_eq!(
            claude.value["args"],
            serde_json::json!(["-c", "cd '/srv/tldraw' && exec 'node' 'dist/index.js'"])
        );
    }

    #[test]
    fn cwd_without_wrap_leaves_command_and_args_unchanged() {
        // A stdio server with no cwd at all must never be wrapped, regardless
        // of whether the target adapter can express cwd natively.
        let reg = Registry::load().unwrap();
        let s = server(
            r#"
            type = "stdio"
            command = "node"
            args = ["dist/index.js"]
            "#,
        );
        let resolver = MapResolver::default();

        let claude = render_server(reg.get("claude-code").unwrap(), &s, &resolver);
        assert_eq!(claude.value["command"], "node");
        assert_eq!(claude.value["args"], serde_json::json!(["dist/index.js"]));
        assert!(claude.value.get("cwd").is_none());
    }

    #[test]
    fn wrapped_cwd_server_preserves_env_and_transport_tag() {
        let reg = Registry::load().unwrap();
        let s = server(
            r#"
            type = "stdio"
            command = "/abs/node"
            args = ["/abs/x.js"]
            cwd = "/srv dir"
            env = { K = "v" }
            "#,
        );
        let resolver = MapResolver::default();
        let r = render_server(reg.get("claude-code").unwrap(), &s, &resolver);

        assert_eq!(r.value["type"], "stdio");
        assert_eq!(r.value["command"], "sh");
        assert_eq!(
            r.value["args"],
            serde_json::json!(["-c", "cd '/srv dir' && exec '/abs/node' '/abs/x.js'"])
        );
        assert_eq!(r.value["env"]["K"], "v");
        assert!(r.value.get("cwd").is_none());
    }

    #[test]
    fn posix_shell_quote_escapes_correctly() {
        assert_eq!(posix_shell_quote("plain"), "'plain'");
        assert_eq!(posix_shell_quote("/srv dir"), "'/srv dir'");
        assert_eq!(posix_shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(posix_shell_quote("$HOME && rm -rf"), "'$HOME && rm -rf'");
    }

    #[test]
    fn cwd_substitutes_refs_and_is_omitted_for_http() {
        let reg = Registry::load().unwrap();
        let resolver = MapResolver::from([("HOME_DIR", "/home/me")]);

        let stdio = server("type = \"stdio\"\ncommand = \"node\"\ncwd = \"${HOME_DIR}/server\"\n");
        let r = render_server(reg.get("codex").unwrap(), &stdio, &resolver);
        assert_eq!(r.value["cwd"], "/home/me/server");

        // cwd is meaningless for a remote transport and must never render.
        let http = server("type = \"http\"\nurl = \"https://x\"\ncwd = \"/nope\"\n");
        let r = render_server(reg.get("codex").unwrap(), &http, &resolver);
        assert!(r.value.get("cwd").is_none());
    }

    #[test]
    fn shell_fallback_syntax_is_not_a_secret_ref() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("codex").unwrap();
        let s = server(
            r#"
            type = "stdio"
            command = "zsh"
            args = ["-lc", "MIRO_TOKEN=${MIRO_ACCESS_TOKEN:-$MIRO_OAUTH_TOKEN} exec server"]
            "#,
        );
        let r = render_server(desc, &s, &MapResolver::default());
        // The shell expression is left verbatim and NOT reported unresolved.
        assert_eq!(
            r.value["args"][1],
            "MIRO_TOKEN=${MIRO_ACCESS_TOKEN:-$MIRO_OAUTH_TOKEN} exec server"
        );
        assert!(r.unresolved.is_empty(), "{:?}", r.unresolved);
    }

    #[test]
    fn nested_ref_inside_shell_fallback_still_resolves() {
        let mut unresolved = Vec::new();
        let mut secrets = Vec::new();
        let resolver = MapResolver::from([("B", "vb")]);
        let out = substitute(
            "${A:-${B}}",
            &resolver,
            false,
            &mut unresolved,
            &mut secrets,
        );
        assert_eq!(out, "${A:-vb}");
        assert!(unresolved.is_empty());
        assert_eq!(secrets, vec![("B".to_string(), "vb".to_string())]);
    }

    #[test]
    fn substitute_is_utf8_safe() {
        let mut unresolved = Vec::new();
        let mut secrets = Vec::new();
        let resolver = MapResolver::from([("TOK", "v")]);
        // Multibyte chars survive both outside refs and inside a skipped
        // shell-syntax span.
        let out = substitute(
            "héllo ${TOK} Ω=${GREETING:-héllo}",
            &resolver,
            false,
            &mut unresolved,
            &mut secrets,
        );
        assert_eq!(out, "héllo v Ω=${GREETING:-héllo}");
        assert!(unresolved.is_empty());
    }

    #[test]
    fn failed_store_read_is_failed_not_unresolved() {
        struct FailingResolver;
        impl Resolver for FailingResolver {
            fn resolve(&self, name: &str) -> Option<String> {
                self.lookup(name).found()
            }
            fn lookup(&self, _name: &str) -> Lookup {
                Lookup::Failed("keychain read failed: timeout".into())
            }
        }

        let reg = Registry::load().unwrap();
        let desc = reg.get("claude-code").unwrap();
        let s = server(
            r#"
            type = "http"
            url = "https://x"
            headers = { Authorization = "Bearer ${TOK}" }
            "#,
        );
        let r = render_server(desc, &s, &FailingResolver);
        assert!(r.unresolved.is_empty(), "a read error is not a miss");
        assert_eq!(
            r.failed,
            vec![(
                "TOK".to_string(),
                "keychain read failed: timeout".to_string()
            )]
        );
        // The token stays in place, same as an unresolved ref.
        assert_eq!(r.value["headers"]["Authorization"], "Bearer ${TOK}");

        // The single-channel wrapper folds the failure into `unresolved`
        // with the message attached (hooks/settings/gateway path).
        let mut unresolved = Vec::new();
        let mut secrets = Vec::new();
        let out = substitute(
            "${TOK}",
            &FailingResolver,
            false,
            &mut unresolved,
            &mut secrets,
        );
        assert_eq!(out, "${TOK}");
        assert_eq!(unresolved, vec!["TOK — keychain read failed: timeout"]);
        assert!(secrets.is_empty(), "a failed read records no secret value");
    }

    #[test]
    fn unresolved_secret_is_recorded_and_left_in_place() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("claude-code").unwrap();
        let s = server(
            r#"
            type = "http"
            url = "https://x"
            headers = { Authorization = "Bearer ${MISSING}" }
            "#,
        );
        let resolver = MapResolver::default();
        let r = render_server(desc, &s, &resolver);
        assert_eq!(r.unresolved, vec!["MISSING".to_string()]);
        assert_eq!(r.value["headers"]["Authorization"], "Bearer ${MISSING}");
    }
}
