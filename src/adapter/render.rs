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
}

/// Render one named server for one adapter.
pub fn render_server(
    desc: &AdapterDescriptor,
    server: &Server,
    resolver: &dyn Resolver,
) -> Rendered {
    let mut body: Map<String, Value> = Map::new();
    let mut unresolved: Vec<String> = Vec::new();

    let passthrough = desc.mcp.secret_mode == SecretMode::Passthrough;
    let mut sub = |s: &str| substitute(s, resolver, passthrough, &mut unresolved);

    // 1. Transport tag (e.g. Claude's "type": "http").
    if let Some(t) = &desc.mcp.transport {
        let tag = match server.server_type {
            ServerType::Http => Some(t.http_value.clone()),
            ServerType::Stdio => t.stdio_value.clone(),
        };
        if let Some(tag) = tag {
            body.insert(t.key.clone(), Value::String(tag));
        }
    }

    // 2. url
    if let (Some(field), Some(url)) = (&desc.mcp.fields.url, &server.url) {
        body.insert(field.clone(), Value::String(sub(url)));
    }

    // 3. command
    if let (Some(field), Some(cmd)) = (&desc.mcp.fields.command, &server.command) {
        body.insert(field.clone(), Value::String(sub(cmd)));
    }

    // 4. args
    if let Some(field) = &desc.mcp.fields.args {
        if !server.args.is_empty() {
            let arr = server.args.iter().map(|a| Value::String(sub(a))).collect();
            body.insert(field.clone(), Value::Array(arr));
        }
    }

    // 5. headers (nested object)
    if let Some(field) = &desc.mcp.fields.headers {
        if !server.headers.is_empty() {
            let mut h = Map::new();
            for (k, v) in &server.headers {
                h.insert(k.clone(), Value::String(sub(v)));
            }
            body.insert(field.clone(), Value::Object(h));
        }
    }

    // 6. env (nested object)
    if let Some(field) = &desc.mcp.fields.env {
        if !server.env.is_empty() {
            let mut e = Map::new();
            for (k, v) in &server.env {
                e.insert(k.clone(), Value::String(sub(v)));
            }
            body.insert(field.clone(), Value::Object(e));
        }
    }

    // De-duplicate unresolved refs while keeping first-seen order.
    unresolved.dedup();

    Rendered {
        value: Value::Object(body),
        unresolved,
    }
}

/// Replace every `${NAME}` token in `s`. In passthrough mode the token is left
/// verbatim. Otherwise it is resolved; unresolved tokens are recorded and left
/// in place (never silently blanked).
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
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = s[i + 2..].find('}') {
                let name = &s[i + 2..i + 2 + end];
                match resolver.resolve(name) {
                    Some(val) => out.push_str(&val),
                    None => {
                        unresolved.push(name.to_string());
                        out.push_str(&s[i..i + 2 + end + 1]); // keep `${NAME}`
                    }
                }
                i = i + 2 + end + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
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
