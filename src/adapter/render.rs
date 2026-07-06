//! Generic renderer: turn a manifest [`Server`] into a target-shaped value tree
//! according to an [`AdapterDescriptor`]. The output is a `serde_json::Value`
//! object whose keys are already the target's field names; the format-specific
//! mergers ([`crate::render`]) then write it as JSON or TOML.

use serde_json::{Map, Value};

use super::descriptor::{AdapterDescriptor, SecretMode};
use crate::manifest::{Server, ServerType};
use crate::secret::Resolver;

/// A rendered server entry plus any secret references that could not be
/// resolved on this machine (surfaced by `doctor`/`apply`).
pub struct Rendered {
    /// The server body, keyed by the server name.
    pub value: Value,
    pub unresolved: Vec<String>,
    /// Whether this adapter's config format can represent this server's
    /// transport at all. `false` means the descriptor maps no field for the
    /// server's defining attribute (e.g. an HTTP server for the stdio-only
    /// Claude Desktop config) — callers must skip it rather than write an empty
    /// `{}` entry into a real config file.
    pub representable: bool,
}

/// Render one named server for one adapter.
pub fn render_server(
    desc: &AdapterDescriptor,
    server: &Server,
    resolver: &dyn Resolver,
) -> Rendered {
    let mut body: Map<String, Value> = Map::new();
    let mut unresolved: Vec<String> = Vec::new();

    // CLIs with no MCP support render nothing.
    let Some(mcp) = desc.mcp.as_ref() else {
        return Rendered {
            value: Value::Object(body),
            unresolved,
            representable: false,
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
    let mut sub = |s: &str| substitute(s, resolver, passthrough, &mut unresolved);

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
    if mcp.command_array {
        if let (Some(field), Some(cmd)) = (&mcp.fields.command, &server.command) {
            let mut arr = vec![Value::String(sub(cmd))];
            arr.extend(server.args.iter().map(|a| Value::String(sub(a))));
            body.insert(field.clone(), Value::Array(arr));
        }
    } else {
        if let (Some(field), Some(cmd)) = (&mcp.fields.command, &server.command) {
            body.insert(field.clone(), Value::String(sub(cmd)));
        }
        // 4. args
        if let Some(field) = &mcp.fields.args {
            if !server.args.is_empty() {
                let arr = server.args.iter().map(|a| Value::String(sub(a))).collect();
                body.insert(field.clone(), Value::Array(arr));
            }
        }
    }

    // 5. headers (nested object)
    if let Some(field) = &mcp.fields.headers {
        if !server.headers.is_empty() {
            let mut h = Map::new();
            for (k, v) in &server.headers {
                h.insert(k.clone(), Value::String(sub(v)));
            }
            body.insert(field.clone(), Value::Object(h));
        }
    }

    // 6. env (nested object)
    if let Some(field) = &mcp.fields.env {
        if !server.env.is_empty() {
            let mut e = Map::new();
            for (k, v) in &server.env {
                e.insert(k.clone(), Value::String(sub(v)));
            }
            body.insert(field.clone(), Value::Object(e));
        }
    }

    // 7. Per-target extras: native keys with no transport-neutral equivalent
    // (e.g. Codex `startup_timeout_sec`), passed through verbatim. Rendered
    // last so a deliberate extra can override a canonical field.
    if let Some(extra) = server.extra.get(&desc.id) {
        for (k, v) in extra {
            body.insert(
                k.clone(),
                substitute_value(v, resolver, passthrough, &mut unresolved),
            );
        }
    }

    // De-duplicate unresolved refs while keeping first-seen order.
    unresolved.dedup();

    Rendered {
        value: Value::Object(body),
        unresolved,
        representable,
    }
}

/// Substitute `${NAME}` refs in every string leaf of an extras value; numbers,
/// bools, and arrays pass through untouched.
fn substitute_value(
    v: &Value,
    resolver: &dyn Resolver,
    passthrough: bool,
    unresolved: &mut Vec<String>,
) -> Value {
    match v {
        Value::String(s) => Value::String(substitute(s, resolver, passthrough, unresolved)),
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|el| substitute_value(el, resolver, passthrough, unresolved))
                .collect(),
        ),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, el)| {
                    (
                        k.clone(),
                        substitute_value(el, resolver, passthrough, unresolved),
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Replace every `${NAME}` token in `s`. In passthrough mode the token is left
/// verbatim. Otherwise it is resolved; unresolved tokens are recorded and left
/// in place (never silently blanked). Spans that are not valid reference names
/// (shell syntax like `${VAR:-fallback}`) are left verbatim and not recorded.
pub(crate) fn substitute(
    s: &str,
    resolver: &dyn Resolver,
    passthrough: bool,
    unresolved: &mut Vec<String>,
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
                    match resolver.resolve(name) {
                        Some(val) => out.push_str(&val),
                        None => {
                            unresolved.push(name.to_string());
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
        let resolver = MapResolver::from([("B", "vb")]);
        let out = substitute("${A:-${B}}", &resolver, false, &mut unresolved);
        assert_eq!(out, "${A:-vb}");
        assert!(unresolved.is_empty());
    }

    #[test]
    fn substitute_is_utf8_safe() {
        let mut unresolved = Vec::new();
        let resolver = MapResolver::from([("TOK", "v")]);
        // Multibyte chars survive both outside refs and inside a skipped
        // shell-syntax span.
        let out = substitute(
            "héllo ${TOK} Ω=${GREETING:-héllo}",
            &resolver,
            false,
            &mut unresolved,
        );
        assert_eq!(out, "héllo v Ω=${GREETING:-héllo}");
        assert!(unresolved.is_empty());
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
