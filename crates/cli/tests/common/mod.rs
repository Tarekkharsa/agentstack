// Shared test support. Compiled into each binary that says `mod common;`, so
// most binaries use only part of it.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

//! One fake MCP stdio server, instead of eight hand-maintained copies of it.
//!
//! ## Why this module exists
//!
//! Near-identical POSIX-sh fake servers lived in `lease_registry.rs`,
//! `gateway_stdio.rs` (three of them), `trust_at_dispatch.rs`,
//! `yes_on_lease_path.rs`, `red_team_shell_injection.rs`,
//! `red_team_untrusted_inertness.rs` and `package_layer.rs` (two). They differ
//! only in the server's name, what its `tools/list` advertises, and what it
//! answers a `tools/call` with. Everything else — the read loop, the id
//! extraction, the `initialize` reply and, critically, the `server/discover`
//! reply — was copy-paste.
//!
//! That cost correctness, not time. The `server/discover` arm is the fix from
//! commit `7b7c59f`: the gateway probes `server/discover` first and only falls
//! back to the dated handshake once the peer ANSWERS, so a fixture that stays
//! silent burns the whole 10s stdio start budget
//! (`crate::gateway::stdio_start_timeout`). `lease_registry` paid 11s for that
//! omission. The repair had to be applied by hand in eight places, and a ninth
//! copy written next month would silently reintroduce the stall.
//!
//! So the rule the fix generalizes to — *never let a fixture stay silent while
//! the product waits for an answer* — is now structural: every server built
//! here answers `server/discover` with `-32601` immediately, because there is
//! one template and it always has that arm. A new fixture cannot forget it.
//!
//! ## Using it
//!
//! ```ignore
//! mod common;
//! use common::{write_executable, StdioServer};
//!
//! let script = StdioServer::new("fix")
//!     .tools(r#"{"name":"echo","description":"Echo.","inputSchema":{"type":"object"}}"#)
//!     .on_call(r#"      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pong"}]}}\n' "$id""#)
//!     .script();
//! write_executable(&dir.join("echo-server"), &script);
//! ```

use std::path::Path;

/// The read loop every fake stdio server shares. `@…@` slots are filled by
/// [`StdioServer::script`].
///
/// The `server/discover` arm is not optional decoration — see the module doc.
const TEMPLATE: &str = r#"#!/bin/sh
@PROLOGUE@while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"server/discover"'*)
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"method not found"}}\n' "$id"
      ;;
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"@NAME@","version":"0"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[@TOOLS@]}}\n' "$id"
      ;;
@CALL@  esac
done
"#;

/// A fake MCP stdio server, built as a POSIX-sh script.
pub struct StdioServer {
    name: String,
    prologue: String,
    tools: String,
    call_arm: Option<String>,
}

impl StdioServer {
    /// A server reporting `name` in its `serverInfo`, advertising no tools and
    /// answering no calls.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            prologue: String::new(),
            tools: String::new(),
            call_arm: None,
        }
    }

    /// Shell lines to run once, before the read loop — recording `$$` to a
    /// pidfile, capturing `$1`, and so on. Must end without a trailing newline;
    /// one is added.
    pub fn prologue(mut self, shell: &str) -> Self {
        self.prologue = format!("{shell}\n");
        self
    }

    /// The body of the `tools` array in the `tools/list` reply: raw JSON
    /// objects, comma-separated, written for a single-quoted `printf` format
    /// (so no `%`, and inner quotes stay bare).
    pub fn tools(mut self, json: &str) -> Self {
        self.tools = json.to_string();
        self
    }

    /// One named tool with an empty object schema — the common case.
    pub fn tool(self, name: &str, description: &str) -> Self {
        let json = format!(
            r#"{{"name":"{name}","description":"{description}","inputSchema":{{"type":"object"}}}}"#
        );
        self.tools(&json)
    }

    /// Shell lines answering a `tools/call`, indented to sit inside the `case`.
    /// `$id` and `$line` are in scope.
    pub fn on_call(mut self, shell: &str) -> Self {
        self.call_arm = Some(shell.to_string());
        self
    }

    /// The finished script.
    pub fn script(&self) -> String {
        let call = match &self.call_arm {
            Some(body) => format!("    *'\"method\":\"tools/call\"'*)\n{body}\n      ;;\n"),
            None => String::new(),
        };
        TEMPLATE
            .replace("@PROLOGUE@", &self.prologue)
            .replace("@NAME@", &self.name)
            .replace("@TOOLS@", &self.tools)
            .replace("@CALL@", &call)
    }
}

/// Write `body` to `path` and make it executable.
pub fn write_executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
