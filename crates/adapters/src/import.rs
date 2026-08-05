//! Reverse of [`render`](super::render): read an existing CLI config (already
//! parsed to a JSON-shaped value tree) and recover manifest [`Server`]s, using
//! the same adapter descriptor that drives rendering. Values are recovered
//! verbatim; secret-lifting is a separate policy step in `init`.

use indexmap::IndexMap;
use serde_json::Value;

use super::descriptor::AdapterDescriptor;
use agentstack_core::manifest::{Server, ServerType};

/// Extract the settings worth importing from a CLI's parsed settings file:
/// every top-level key that has at least one catalog field. Whole top-level
/// values are taken (so e.g. `permissions` keeps its `allow`/`deny` alongside
/// the catalogued `defaultMode`) — this matches the top-level ownership model so
/// re-applying never drops sibling keys.
pub fn extract_settings(desc: &AdapterDescriptor, root: &Value) -> serde_json::Map<String, Value> {
    let Some(spec) = desc.settings.as_ref() else {
        return Default::default();
    };
    let catalog: std::collections::HashSet<&str> = spec
        .fields
        .iter()
        .map(|f| f.key.split('.').next().unwrap_or(&f.key))
        .collect();
    root.as_object()
        .map(|o| {
            o.iter()
                .filter(|(k, _)| catalog.contains(k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// An entry in a CLI's MCP section that could not be imported, with a
/// plain-language reason. Import never deletes anything — the entry stays in
/// the CLI's own config — so a skip is information the user deserves, not an
/// error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedImport {
    pub name: String,
    pub reason: &'static str,
}

/// Extract `(name, Server)` pairs from a target config's value tree, in file
/// order. Entries that don't look like MCP servers are skipped.
pub fn extract_servers(desc: &AdapterDescriptor, root: &Value) -> Vec<(String, Server)> {
    extract_servers_with_skips(desc, root).0
}

/// [`extract_servers`], but also reporting every entry the import had to skip
/// and why — so `init` can explain a lossy import in plain language instead of
/// silently dropping entries.
pub fn extract_servers_with_skips(
    desc: &AdapterDescriptor,
    root: &Value,
) -> (Vec<(String, Server)>, Vec<SkippedImport>) {
    let Some(mcp) = desc.mcp.as_ref() else {
        return (Vec::new(), Vec::new());
    };
    let Some(section) = navigate(root, &mcp.location).and_then(Value::as_object) else {
        return (Vec::new(), Vec::new());
    };

    let mut out = Vec::new();
    let mut skipped = Vec::new();
    for (name, body) in section {
        let Some(obj) = body.as_object() else {
            skipped.push(SkippedImport {
                name: name.clone(),
                reason: "the entry is not a table of fields, so it does not look like an \
                         MCP server definition",
            });
            continue;
        };

        let get_str = |field: &Option<String>| -> Option<String> {
            field
                .as_ref()
                .and_then(|f| obj.get(f))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let get_map = |field: &Option<String>| -> IndexMap<String, String> {
            field
                .as_ref()
                .and_then(|f| obj.get(f))
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default()
        };

        let url = get_str(&mcp.fields.url);
        let command = get_str(&mcp.fields.command);
        let args = mcp
            .fields
            .args
            .as_ref()
            .and_then(|f| obj.get(f))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let cwd = get_str(&mcp.fields.cwd);
        let headers = get_map(&mcp.fields.headers);
        let env = get_map(&mcp.fields.env);

        let server_type = infer_type(mcp, obj, &url, &command);

        // Skip entries that are neither http nor stdio shaped.
        if url.is_none() && command.is_none() {
            skipped.push(SkippedImport {
                name: name.clone(),
                reason: "it has neither a url nor a command, so agentstack cannot tell \
                         how to launch or reach it",
            });
            continue;
        }

        // Keys the descriptor maps (plus the transport tag) are canonical;
        // anything else is a hand-tuned native key (e.g. Codex
        // `startup_timeout_sec`) that must round-trip rather than be dropped
        // on the next apply — keep it under `extra.<adapter id>`.
        let known: Vec<&str> = [
            mcp.fields.url.as_deref(),
            mcp.fields.command.as_deref(),
            mcp.fields.args.as_deref(),
            mcp.fields.cwd.as_deref(),
            mcp.fields.headers.as_deref(),
            mcp.fields.env.as_deref(),
            mcp.transport.as_ref().map(|t| t.key.as_str()),
        ]
        .into_iter()
        .flatten()
        .collect();
        let unknown: IndexMap<String, Value> = obj
            .iter()
            .filter(|(k, _)| !known.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut extra = IndexMap::new();
        if !unknown.is_empty() {
            extra.insert(desc.id.clone(), unknown);
        }

        out.push((
            name.clone(),
            Server {
                server_type,
                url,
                command,
                args,
                cwd,
                // Native configs have no integrity-root concept; declaring
                // roots is a manifest-side trust decision, never imported.
                integrity_roots: Vec::new(),
                targets: agentstack_core::manifest::model::all_targets(),
                owner: None,
                headers,
                env,
                extra,
            },
        ));
    }
    (out, skipped)
}

/// An imported server whose executable path lies inside another application's
/// bundle — the application that installed it owns the entry and rewrites it
/// on its own schedule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolManaged {
    /// The bundle's name without its `.app` suffix, exactly as it appears on
    /// the path (`ChatGPT`). Display only: it is a directory name, never a
    /// verified publisher identity.
    pub application: String,
    /// The path the rule matched, verbatim — the evidence behind the reading,
    /// so a user can check it rather than take the classification on faith.
    pub evidence: String,
}

/// Bounds on the wrapper parse below. Every value here comes from another
/// tool's config file, which is hostile input: the parse costs a fixed amount
/// of work, and anything past a bound is simply not classified — which, given
/// the bias stated in [`tool_managed`], means it imports.
const MAX_PATH_BYTES: usize = 4096;
const MAX_SCRIPT_BYTES: usize = 4096;
const MAX_SCRIPT_WORDS: usize = 256;

/// Classify an imported server as TOOL-MANAGED — installed, owned and updated
/// by another desktop application rather than chosen by this user — from its
/// command line alone.
///
/// # The rule
///
/// One of these paths must lie inside an application bundle:
///
/// - the `command` itself; or
/// - for a POSIX shell wrapper (`sh`/`bash`/`zsh`/`dash`/`ksh` invoked with
///   `-c`), the *command words* of the script it carries — the script's first
///   word, the word after each `&&`, `||`, `;` or `|`, and the word after
///   `exec`.
///
/// A path lies inside a bundle when one of its components ends in `.app`
/// (case-insensitively, and is more than that suffix), that component is not
/// the last one, and either the next component is `Contents` or the path
/// starts with `/Applications/` or `~/Applications/` (literal or expanded).
///
/// # Limits — read these before trusting the answer
///
/// **This is a heuristic over path TEXT, not a claim of provenance.** Nothing
/// here executes, resolves, stats, follows or verifies anything: no `PATH`
/// lookup, no symlink resolution, no variable expansion, no code signature. It
/// does not know who published a binary, who signed it, or who will update it.
/// It knows that a string looks like a path into a `.app` bundle.
///
/// So it misses, by construction: a bundle reached through `$VAR`, through a
/// symlink, through a bare name found on `PATH`, or through a `cwd` plus a
/// relative `command` (`cwd` is not consulted). It reads only the *program*
/// being launched, so `node /Applications/Foo.app/…/server.js` reads as the
/// user's — the bundle path there is data handed to `node`. A wrapper written
/// for a shell this does not know, or past the bounds above, reads as the
/// user's too.
///
/// The bias is deliberate and one-directional: **an uncertain answer is the
/// user's server.** A wrong "tool-managed" silently loses something someone
/// chose; a wrong "user's" is only the behaviour that shipped before this
/// existed.
pub fn tool_managed(server: &Server) -> Option<ToolManaged> {
    let command = server.command.as_deref()?;
    if let Some(found) = in_app_bundle(command) {
        return Some(found);
    }
    shell_wrapper_programs(command, &server.args)
        .iter()
        .find_map(|word| in_app_bundle(word))
}

/// The bundle test itself. Returns the owning bundle's name and the matched
/// path when `path` points inside an application bundle.
fn in_app_bundle(path: &str) -> Option<ToolManaged> {
    if path.len() > MAX_PATH_BYTES {
        return None;
    }
    let under_applications = under_applications_dir(path);
    let parts: Vec<&str> = path.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        let Some(stem) = bundle_stem(part) else {
            continue;
        };
        // A `.app` component must be a DIRECTORY on the way to the executable.
        // A file literally named `Foo.app` IS the program, not a bundle around
        // one, and `/home/x/my.app.js` never reaches here at all — its last
        // component ends in `.js`.
        if i + 1 >= parts.len() {
            continue;
        }
        if parts[i + 1] == "Contents" || under_applications {
            return Some(ToolManaged {
                application: stem.to_string(),
                evidence: path.to_string(),
            });
        }
    }
    None
}

/// `Foo.app` → `Foo`. Case-insensitive because the macOS filesystem is, and
/// `.app` alone is a hidden file, not a bundle.
fn bundle_stem(component: &str) -> Option<&str> {
    let cut = component.len().checked_sub(4)?;
    // `.app` is ASCII, but `component` need not be: never slice mid-character.
    if !component.is_char_boundary(cut) || !component[cut..].eq_ignore_ascii_case(".app") {
        return None;
    }
    let stem = &component[..cut];
    (!stem.is_empty()).then_some(stem)
}

/// Whether `path` starts in one of the two directories macOS installs
/// applications into. `~` is honoured both unexpanded (as configs often write
/// it) and expanded against `HOME`.
fn under_applications_dir(path: &str) -> bool {
    const SUFFIX: &str = "/Applications/";
    if path.starts_with(SUFFIX) || path.starts_with("~/Applications/") {
        return true;
    }
    std::env::var("HOME").is_ok_and(|home| {
        let home = home.trim_end_matches('/');
        !home.is_empty()
            && path
                .strip_prefix(home)
                .is_some_and(|rest| rest.starts_with(SUFFIX))
    })
}

/// For a `sh -c "…"` wrapper, the words the script would run as PROGRAMS.
/// Empty for anything that is not such a wrapper.
fn shell_wrapper_programs(command: &str, args: &[String]) -> Vec<String> {
    let base = command.rsplit('/').next().unwrap_or(command);
    if !matches!(base, "sh" | "bash" | "zsh" | "dash" | "ksh") {
        return Vec::new();
    }
    // The script is the argument straight after the first bare `-c`.
    let Some(script) = args
        .iter()
        .position(|a| a == "-c")
        .and_then(|i| args.get(i + 1))
    else {
        return Vec::new();
    };
    if script.len() > MAX_SCRIPT_BYTES {
        return Vec::new();
    }
    program_words(&split_script(script))
}

/// One word of a `-c` script, and whether it is a command separator.
struct ScriptWord {
    text: String,
    separator: bool,
}

/// Split a `-c` script the way a shell SEPARATES words — whitespace, single
/// quotes, double quotes, backslash escapes — and nothing else.
///
/// This is deliberately not a shell. There is no variable expansion, no
/// command substitution, no globbing, no `eval`, and nothing is run: an
/// unexpanded `$VAR` stays the literal text `$VAR`, which then simply fails to
/// match a bundle path. `&&`, `||`, `;` and `|` come back as separator words
/// so the caller can tell where one command ends and the next begins; a
/// quoted `';'` does not, because it is an argument.
fn split_script(script: &str) -> Vec<ScriptWord> {
    fn flush(cur: &mut String, quoted: &mut bool, out: &mut Vec<ScriptWord>) {
        if !cur.is_empty() || *quoted {
            out.push(ScriptWord {
                text: std::mem::take(cur),
                separator: false,
            });
        }
        *quoted = false;
    }

    let mut out: Vec<ScriptWord> = Vec::new();
    let mut cur = String::new();
    // Tracks "this word carried quotes", so an empty `''` still becomes a word.
    let mut quoted = false;
    let mut chars = script.chars().peekable();
    while let Some(c) = chars.next() {
        if out.len() >= MAX_SCRIPT_WORDS {
            return out;
        }
        match c {
            '\'' => {
                quoted = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '"' => {
                quoted = true;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' => {
                            if let Some(escaped) = chars.next() {
                                cur.push(escaped);
                            }
                        }
                        _ => cur.push(q),
                    }
                }
            }
            '\\' => {
                if let Some(escaped) = chars.next() {
                    cur.push(escaped);
                }
            }
            c if c.is_whitespace() => flush(&mut cur, &mut quoted, &mut out),
            '&' | '|' | ';' => {
                flush(&mut cur, &mut quoted, &mut out);
                let mut op = String::from(c);
                while chars.peek() == Some(&c) {
                    op.push(c);
                    chars.next();
                }
                out.push(ScriptWord {
                    text: op,
                    separator: true,
                });
            }
            _ => cur.push(c),
        }
    }
    flush(&mut cur, &mut quoted, &mut out);
    out.truncate(MAX_SCRIPT_WORDS);
    out
}

/// The words a shell would run as programs: the first, every word after a
/// separator, and the word after `exec`. Arguments are skipped on purpose —
/// `echo /Applications/Foo.app/Contents/x` must not read as launching it.
fn program_words(words: &[ScriptWord]) -> Vec<String> {
    let mut out = Vec::new();
    let mut expect_program = true;
    for word in words {
        if word.separator {
            expect_program = true;
            continue;
        }
        if expect_program {
            out.push(word.text.clone());
            // `exec PROGRAM` still leads to a program word. Only `exec` is
            // honoured: every extra prefix (`env`, `nice`, …) widens the rule
            // in the direction that loses a user's server.
            expect_program = word.text == "exec";
        }
    }
    out
}

/// Determine transport: prefer an explicit tag (Claude's `type`), else infer
/// from which fields are present.
fn infer_type(
    mcp: &super::descriptor::McpSpec,
    obj: &serde_json::Map<String, Value>,
    url: &Option<String>,
    command: &Option<String>,
) -> ServerType {
    if let Some(t) = &mcp.transport {
        if let Some(tag) = obj.get(&t.key).and_then(Value::as_str) {
            if tag == t.http_value {
                return ServerType::Http;
            }
            if t.stdio_value.as_deref() == Some(tag) {
                return ServerType::Stdio;
            }
        }
    }
    if url.is_some() && command.is_none() {
        ServerType::Http
    } else {
        ServerType::Stdio
    }
}

/// Navigate a dotted `location` path (single segment in practice).
fn navigate<'a>(root: &'a Value, location: &str) -> Option<&'a Value> {
    let mut cur = root;
    for seg in location.split('.') {
        cur = cur.as_object()?.get(seg)?;
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Registry;
    use serde_json::json;

    #[test]
    fn extracts_claude_http_and_stdio() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("claude-code").unwrap();
        let root = json!({
            "mcpServers": {
                "kibana": { "type": "http", "url": "https://k", "headers": { "Authorization": "Bearer x" } },
                "tldraw": { "type": "stdio", "command": "node", "args": ["a.js"], "env": { "K": "v" } }
            }
        });
        let servers = extract_servers(desc, &root);
        assert_eq!(servers.len(), 2);
        let kibana = &servers.iter().find(|(n, _)| n == "kibana").unwrap().1;
        assert_eq!(kibana.server_type, ServerType::Http);
        assert_eq!(kibana.url.as_deref(), Some("https://k"));
        assert_eq!(kibana.headers["Authorization"], "Bearer x");
        let tldraw = &servers.iter().find(|(n, _)| n == "tldraw").unwrap().1;
        assert_eq!(tldraw.server_type, ServerType::Stdio);
        assert_eq!(tldraw.args, vec!["a.js".to_string()]);
    }

    #[test]
    fn extract_settings_takes_catalog_keys_whole_and_skips_unknown() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("claude-code").unwrap();
        let file = json!({
            "$schema": "https://x",            // not in catalog → skip
            "model": "opusplan",               // catalog → keep
            "hooks": { "PreToolUse": [] },      // not in catalog → skip
            "permissions": {                    // catalog (permissions.*) → keep WHOLE object
                "defaultMode": "auto",
                "allow": ["Bash(git:*)"],
                "deny": ["Read(./.env)"]
            }
        });
        let out = extract_settings(desc, &file);
        assert!(out.contains_key("model"));
        assert!(out.contains_key("permissions"));
        assert!(!out.contains_key("$schema"));
        assert!(!out.contains_key("hooks"));
        // The whole permissions object comes along (so apply won't drop siblings).
        let perms = out["permissions"].as_object().unwrap();
        assert!(perms.contains_key("defaultMode"));
        assert!(perms.contains_key("allow"));
        assert!(perms.contains_key("deny"));
    }

    #[test]
    fn cwd_round_trips_and_is_not_lifted_into_extras() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("codex").unwrap();
        let root = json!({
            "mcp_servers": {
                "tldraw": {
                    "command": "node",
                    "args": ["dist/index.js"],
                    "cwd": "/srv/tldraw"
                }
            }
        });
        let servers = extract_servers(desc, &root);
        let tldraw = &servers.iter().find(|(n, _)| n == "tldraw").unwrap().1;
        assert_eq!(tldraw.cwd.as_deref(), Some("/srv/tldraw"));
        // `cwd` is a mapped field, not a hand-tuned native key: it must not be
        // duplicated into extras.
        assert!(tldraw.extra.is_empty(), "cwd should not become an extra");
    }

    #[test]
    fn unknown_keys_are_kept_as_per_target_extras() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("codex").unwrap();
        let root = json!({
            "mcp_servers": {
                "miro": {
                    "command": "npx",
                    "args": ["-y", "@mirohq/mcp-server"],
                    "startup_timeout_sec": 20
                },
                "figma": { "url": "https://mcp.figma.com/mcp" }
            }
        });
        let servers = extract_servers(desc, &root);
        let miro = &servers.iter().find(|(n, _)| n == "miro").unwrap().1;
        assert_eq!(miro.extra["codex"]["startup_timeout_sec"], json!(20));
        assert_eq!(miro.extra["codex"].len(), 1, "mapped keys stay canonical");
        let figma = &servers.iter().find(|(n, _)| n == "figma").unwrap().1;
        assert!(figma.extra.is_empty(), "no extras → no extra table");
    }

    #[test]
    fn transport_tag_is_not_lifted_into_extras() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("claude-code").unwrap();
        let root = json!({
            "mcpServers": {
                "k": { "type": "http", "url": "https://k", "custom_key": true }
            }
        });
        let servers = extract_servers(desc, &root);
        let k = &servers[0].1;
        assert_eq!(k.extra["claude-code"]["custom_key"], json!(true));
        assert!(!k.extra["claude-code"].contains_key("type"));
    }

    // Stage 1.2: a lossy import is explained, never silent — entries that
    // don't look like MCP servers come back as named skips with a
    // plain-language reason, while importable siblings still import.
    #[test]
    fn unimportable_entries_are_reported_with_reasons_not_dropped_silently() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("claude-code").unwrap();
        let root = json!({
            "mcpServers": {
                "good": { "command": "npx", "args": ["x"] },
                "no-transport": { "note": "neither url nor command" },
                "not-a-table": "just a string"
            }
        });
        let (servers, skipped) = extract_servers_with_skips(desc, &root);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].0, "good");
        assert_eq!(skipped.len(), 2);
        let no_transport = skipped
            .iter()
            .find(|s| s.name == "no-transport")
            .expect("shapeless entry reported");
        assert!(no_transport.reason.contains("neither a url nor a command"));
        let not_a_table = skipped
            .iter()
            .find(|s| s.name == "not-a-table")
            .expect("non-object entry reported");
        assert!(not_a_table.reason.contains("not a table"));
        // The wrapper keeps its original shape for existing callers.
        assert_eq!(extract_servers(desc, &root).len(), 1);
    }

    /// Build a stdio server from a command line, the way an imported entry
    /// arrives.
    fn stdio(command: &str, args: &[&str]) -> Server {
        serde_json::from_value(json!({
            "type": "stdio",
            "command": command,
            "args": args,
        }))
        .expect("valid server literal")
    }

    /// The two entries that motivated the classifier, copied from real global
    /// configs on a machine with the ChatGPT and Codex desktop apps installed.
    /// Both are owned by the application that installed them.
    #[test]
    fn the_real_desktop_app_servers_read_as_tool_managed() {
        let node_repl = stdio(
            "/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node_repl",
            &[],
        );
        let found = tool_managed(&node_repl).expect("a plain bundle path is tool-managed");
        assert_eq!(found.application, "ChatGPT");
        assert!(found.evidence.contains("ChatGPT.app"), "{found:?}");

        // The wrapper case: the executable is inside a quoted `-c` script, and
        // the bundle sits behind a RELATIVE path. Nothing is executed to find
        // it — the script is split, never run.
        let computer_use = stdio(
            "sh",
            &[
                "-c",
                "cd '.' && exec './Codex Computer Use.app/Contents/SharedSupport/\
                 SkyComputerUseClient.app/Contents/MacOS/SkyComputerUseClient' 'mcp'",
            ],
        );
        let found = tool_managed(&computer_use).expect("a wrapped bundle path is tool-managed");
        // The OUTERMOST bundle on the path is the owner.
        assert_eq!(found.application, "Codex Computer Use");
    }

    /// The bias: anything the rule is not sure about is the USER'S server. A
    /// wrong "tool-managed" loses a server someone chose, so every one of
    /// these must import.
    #[test]
    fn ordinary_commands_read_as_the_users_own() {
        for server in [
            stdio("npx", &["-y", "some-server"]),
            stdio("/usr/local/bin/foo", &[]),
            stdio("node", &["./scripts/local.js"]),
            // ".app" as a SUBSTRING of a file name is not a bundle.
            stdio("/home/x/my.app.js", &[]),
            stdio("node", &["/home/x/my.app.js"]),
            // A bundle path handed to another program as DATA: `node` is what
            // runs, so this is the user's choice of interpreter.
            stdio("node", &["/Applications/Foo.app/Contents/Resources/s.js"]),
            // A bundle path echoed inside a wrapper is an argument, not a
            // program word.
            stdio(
                "sh",
                &["-c", "echo '/Applications/Foo.app/Contents/MacOS/Foo'"],
            ),
            // A shell wrapper with no `-c` script to read.
            stdio("sh", &["/Applications/Foo.app/Contents/MacOS/run.sh"]),
        ] {
            assert_eq!(
                tool_managed(&server),
                None,
                "must import: {:?} {:?}",
                server.command,
                server.args
            );
        }
    }

    /// An http server has no executable to place, and a bare `.app` component
    /// or a trailing one is not a bundle a program lives inside.
    #[test]
    fn shapes_with_no_bundle_executable_read_as_the_users_own() {
        let http: Server = serde_json::from_value(json!({
            "type": "http",
            "url": "https://mcp.example.com/mcp",
        }))
        .expect("valid server literal");
        assert_eq!(tool_managed(&http), None);
        // `Foo.app` as the LAST component is the program itself, not a bundle.
        assert_eq!(tool_managed(&stdio("/opt/tools/Foo.app", &[])), None);
        // `.app` with an empty stem is a hidden file.
        assert_eq!(tool_managed(&stdio("/opt/.app/Contents/x", &[])), None);
    }

    /// `/Applications` and `~/Applications` are bundle roots on their own, so a
    /// layout that does not use `Contents` still reads as tool-managed.
    #[test]
    fn the_applications_directories_are_bundle_roots() {
        let found = tool_managed(&stdio("/Applications/Weird.app/bin/server", &[]))
            .expect("under /Applications");
        assert_eq!(found.application, "Weird");
        let found = tool_managed(&stdio("~/Applications/Weird.app/bin/server", &[]))
            .expect("under ~/Applications");
        assert_eq!(found.application, "Weird");
    }

    /// Hostile input, bounded: a pathological `-c` script costs a fixed amount
    /// of work and, past the bound, is not classified at all — which imports
    /// it, the safe direction.
    #[test]
    fn an_oversized_wrapper_script_is_not_classified() {
        let script = format!(
            "exec '/Applications/Foo.app/Contents/MacOS/Foo'{}",
            " x".repeat(MAX_SCRIPT_BYTES)
        );
        assert_eq!(tool_managed(&stdio("sh", &["-c", &script])), None);
        // An unterminated quote must terminate the parse, not loop it.
        assert_eq!(
            tool_managed(&stdio(
                "sh",
                &["-c", "exec '/Applications/Foo.app/Contents/MacOS/Foo"]
            ))
            .map(|f| f.application),
            Some("Foo".to_string())
        );
    }

    #[test]
    fn extracts_codex_renamed_headers() {
        let reg = Registry::load().unwrap();
        let desc = reg.get("codex").unwrap();
        let root = json!({
            "mcp_servers": {
                "kibana_mcp": { "url": "https://k", "http_headers": { "Authorization": "Bearer x" } }
            }
        });
        let servers = extract_servers(desc, &root);
        assert_eq!(servers.len(), 1);
        let (name, s) = &servers[0];
        assert_eq!(name, "kibana_mcp");
        assert_eq!(s.server_type, ServerType::Http);
        assert_eq!(s.headers["Authorization"], "Bearer x");
    }
}
