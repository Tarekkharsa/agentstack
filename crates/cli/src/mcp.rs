//! Minimal MCP (Model Context Protocol) handshakes for `doctor`'s two probes.
//!
//! [`handshake`] (`--live`) performs the Streamable-HTTP `initialize` →
//! `notifications/initialized` → `tools/list` sequence; [`probe_stdio`]
//! (`--probe`) performs the same sequence against a freshly spawned child
//! process. Both report server identity + tool count, or a classified error.
//! Just enough to prove a server actually comes up under the configuration the
//! manifest declares.
//!
//! This is the *diagnostic* MCP client. The gateway (`crate::gateway`) keeps
//! the single dispatch path that real agent traffic flows through; nothing
//! here proxies a call, holds a session open, or grants authority — a probe
//! starts a server, asks it who it is, and stops it again.

use std::time::{Duration, Instant};

use indexmap::IndexMap;

#[derive(Debug)]
pub struct Handshake {
    pub server_name: Option<String>,
    pub protocol: Option<String>,
    pub tool_count: Option<usize>,
}

#[derive(Debug)]
pub enum LiveError {
    /// 401/403 — credentials missing or rejected.
    Auth(u16),
    /// Could not connect / timed out / TLS error.
    Connect(String),
    /// Connected, but the response wasn't a usable MCP handshake.
    Protocol(String),
}

impl std::fmt::Display for LiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LiveError::Auth(code) => write!(f, "{code} unauthorized"),
            LiveError::Connect(e) => write!(f, "connection failed: {e}"),
            LiveError::Protocol(e) => write!(f, "protocol error: {e}"),
        }
    }
}

/// Run the handshake against an HTTP MCP server.
pub fn handshake(
    url: &str,
    headers: &IndexMap<String, String>,
    timeout: Duration,
) -> Result<Handshake, LiveError> {
    let headers: Vec<(String, String)> = headers
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let client = agentstack_mcp::HttpUpstreamClient::connect(url, &headers, timeout)
        .map_err(classify_live_error)?;
    let (server_name, protocol) = client.server_identity();
    let tool_count = client.list_tools().ok().map(|tools| tools.len());

    Ok(Handshake {
        server_name,
        protocol,
        tool_count,
    })
}

/// Turn one failed handshake into the question it answers for the user.
///
/// The auth code is read from the protocol adapter's typed refusal, never
/// scraped out of the message: an error chain carries the URL, so searching it
/// for "401"/"403" classified `http://127.0.0.1:4013/mcp` as an authentication
/// failure and sent the user to fix a credential that was fine.
fn classify_live_error(error: anyhow::Error) -> LiveError {
    match agentstack_mcp::auth_status(&error) {
        Some(code) => LiveError::Auth(code),
        None => LiveError::Connect(format!("{error:#}")),
    }
}

// ── stdio probe (`doctor --probe`) ──────────────────────────────────────────

/// What a successful stdio probe learned. `elapsed` is wall time from just
/// before `spawn` to the `initialize` reply — the number that answers "is this
/// server slow to start?", which is the other half of "does it start at all".
#[derive(Debug)]
pub struct StdioProbe {
    pub server_name: Option<String>,
    pub protocol: Option<String>,
    pub tool_count: Option<usize>,
    pub elapsed: Duration,
}

/// Why a stdio probe failed. Each variant answers a different user question,
/// which is the point of classifying rather than returning one string: a
/// command that isn't there is a `PATH`/install problem, a child that exits is
/// usually a bad argument or a rejected credential, and a child that never
/// answers is a hang.
#[derive(Debug)]
pub enum StdioProbeError {
    /// The command could not be started at all — not on `PATH`, not
    /// executable, or the working directory does not exist.
    Spawn(String),
    /// The child started but ended before completing the handshake.
    Exited { status: String, stderr: String },
    /// No `initialize` reply inside the deadline. The child was killed.
    Timeout { after: Duration, stderr: String },
    /// The child answered, but not with a usable MCP `initialize` result.
    Protocol { detail: String, stderr: String },
}

impl std::fmt::Display for StdioProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Every child-supplied string on this path is hostile: it reaches a
        // terminal, so it goes through the §A display sanitizer first. The
        // spawn error is ours (an OS message about our own command), but the
        // command name inside it came from the manifest, so it is sanitized
        // too rather than trusted for being wrapped in our text.
        match self {
            StdioProbeError::Spawn(e) => {
                write!(f, "did not start: {}", crate::text::sanitize_line(e))
            }
            StdioProbeError::Exited { status, stderr } => {
                write!(f, "exited before the handshake ({status}){}", tail(stderr))
            }
            StdioProbeError::Timeout { after, stderr } => write!(
                f,
                "no response {}s after starting — killed{}",
                after.as_secs(),
                tail(stderr)
            ),
            StdioProbeError::Protocol { detail, stderr } => write!(
                f,
                "started but did not speak MCP: {}{}",
                crate::text::sanitize_line(detail),
                tail(stderr)
            ),
        }
    }
}

/// Render captured stderr as a trailing ` — <text>` clause, or nothing when the
/// child said nothing. Sanitized and length-capped: a server that fails by
/// printing a stack trace must not repaint the user's terminal or fill the
/// report with it.
fn tail(stderr: &str) -> String {
    let line = crate::text::sanitize_line(stderr);
    if line.is_empty() {
        String::new()
    } else {
        format!(
            " — {}",
            crate::text::truncate_chars(&line, STDERR_DISPLAY_CHARS)
        )
    }
}

/// How much of the bounded capture is shown. The display limit is smaller than
/// the capture limit: the
/// line that explains a failure ("command not found", "missing API key") is
/// short and comes first — newlines became spaces, so the front of the string
/// is the front of the output — while the rest is a stack trace that would
/// bury the other servers' results.
const STDERR_DISPLAY_CHARS: usize = 160;

/// Start a stdio MCP server exactly as a harness would, speak `initialize`,
/// and stop it again.
///
/// `timeout` is a hard wall over the whole call — spawn, handshake, and the
/// best-effort `tools/list` share one deadline, so a probe can never take
/// longer than the caller budgeted no matter how the child behaves.
///
/// The child inherits this process's environment with `env` layered on top,
/// which is what a rendered config gives a harness. That makes the probe an
/// honest answer to "does this start *here*" — from a terminal-launched
/// agentstack, "here" has your shell's `PATH`, which a GUI-launched harness
/// may not.
pub fn probe_stdio(
    command: &str,
    args: &[String],
    env: &IndexMap<String, String>,
    cwd: &std::path::Path,
    timeout: Duration,
) -> Result<StdioProbe, StdioProbeError> {
    let started = Instant::now();
    let deadline = started + timeout;
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let client =
        agentstack_mcp::StdioUpstreamClient::connect(command, args, &env, cwd, timeout, timeout)
            .map_err(|error| match error.kind {
                agentstack_mcp::StdioUpstreamErrorKind::Spawn => {
                    StdioProbeError::Spawn(error.detail)
                }
                // The stream closed with no reply: for a child process that
                // means it is gone, which is a different user question ("what
                // did it reject?") than a hang or a bad dialect.
                agentstack_mcp::StdioUpstreamErrorKind::Exited => StdioProbeError::Exited {
                    status: crate::text::one_line(&error.detail, 200),
                    stderr: error.stderr,
                },
                agentstack_mcp::StdioUpstreamErrorKind::Timeout => StdioProbeError::Timeout {
                    after: timeout,
                    stderr: error.stderr,
                },
                agentstack_mcp::StdioUpstreamErrorKind::Protocol => StdioProbeError::Protocol {
                    detail: crate::text::one_line(&error.detail, 200),
                    stderr: error.stderr,
                },
            })?;
    let elapsed = started.elapsed();
    let (server_name, protocol) = client.server_identity();
    let server_name = server_name.map(|name| crate::text::sanitize_line(&name));
    let protocol = protocol.map(|version| crate::text::sanitize_line(&version));

    // Best effort, inside the same end-to-end deadline. Dropping the RMCP
    // client closes the transport and process wrapper, including descendants.
    let remaining = deadline.saturating_duration_since(Instant::now());
    let tool_count = (!remaining.is_zero())
        .then(|| client.list_tools_with_timeout(remaining).ok())
        .flatten()
        .map(|tools| tools.len());

    Ok(StdioProbe {
        server_name,
        protocol,
        tool_count,
        elapsed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A URL is part of every connection error, and ports are four digits.
    /// Classifying by substring turned `:4013` into "401 unauthorized" and sent
    /// the user to fix a credential that was never involved. The auth code now
    /// comes from the adapter's typed refusal or not at all.
    #[test]
    fn a_port_in_the_error_text_is_not_an_auth_failure() {
        let cases = [
            "contacting kibana: error sending request for url (http://127.0.0.1:4013/mcp)",
            "connection refused: http://localhost:4031/mcp",
            "dns error for https://example.com:14038/mcp",
        ];
        for text in cases {
            let classified = classify_live_error(anyhow::anyhow!("{text}"));
            assert!(
                matches!(classified, LiveError::Connect(_)),
                "{text} was classified as {classified}"
            );
        }
    }

    /// A child that starts and then gives up is its own answer: "exited before
    /// the handshake", not a hang and not a dialect problem. The stream closing
    /// with no reply is what proves it.
    #[test]
    fn a_child_that_dies_before_the_handshake_is_reported_as_exited() {
        let err = probe_stdio(
            "sh",
            &[
                "-c".to_string(),
                "echo 'FATAL: no API key' >&2; exit 3".to_string(),
            ],
            &IndexMap::new(),
            std::path::Path::new("."),
            Duration::from_secs(5),
        )
        .expect_err("a server that exits cannot complete a handshake");
        assert!(
            matches!(err, StdioProbeError::Exited { .. }),
            "expected an exit, got: {err}"
        );
        assert!(
            err.to_string().contains("FATAL: no API key"),
            "the child's own reason must reach the user: {err}"
        );
    }

    /// The modern probe gets the WHOLE startup budget on ONE child.
    ///
    /// It used to be capped at 100ms and wrapped around spawn + handshake, so
    /// any server that took longer than that to come up failed the probe, was
    /// killed, and was spawned a second time for the dated handshake — modern
    /// protocol unreachable in practice and every start-up side effect done
    /// twice. This server needs 300ms and then refuses `server/discover` the
    /// way the spec requires; one spawn must be enough.
    #[test]
    fn a_slow_legacy_server_is_spawned_once_and_negotiated_on_that_child() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let starts = tmp.path().join("starts");
        // Test-authored path, not repository content.
        let script = format!(
            r#"echo start >> {}
sleep 0.3
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *server/discover*) printf '{{"jsonrpc":"2.0","id":%s,"error":{{"code":-32601,"message":"unsupported"}}}}\n' "$id" ;;
    *'"method":"initialize"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-11-25","capabilities":{{}},"serverInfo":{{"name":"dated","version":"1"}}}}}}\n' "$id" ;;
    *tools/list*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[]}}}}\n' "$id" ;;
  esac
done"#,
            starts.display()
        );

        let probe = probe_stdio(
            "sh",
            &["-c".to_string(), script],
            &IndexMap::new(),
            tmp.path(),
            Duration::from_secs(10),
        )
        .expect("a compliant dated server must be reachable");
        assert_eq!(probe.server_name.as_deref(), Some("dated"));
        assert_eq!(probe.protocol.as_deref(), Some("2025-11-25"));
        let starts = std::fs::read_to_string(&starts).unwrap_or_default();
        assert_eq!(
            starts.lines().count(),
            1,
            "the server was spawned more than once: {starts:?}"
        );
    }

    /// A paginating upstream must be read to the END. `tools/list` returns one
    /// page plus a `nextCursor`, and the client that stops at page one silently
    /// hides every tool after it — the surface looks small rather than broken,
    /// which is the worst way to lose a capability.
    #[test]
    fn every_page_of_a_paginating_upstream_is_read() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let script = r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *server/discover*) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"unsupported"}}\n' "$id" ;;
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-11-25","capabilities":{},"serverInfo":{"name":"paged","version":"1"}}}\n' "$id" ;;
    *cursor*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"second","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
    *tools/list*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"first","inputSchema":{"type":"object"}}],"nextCursor":"page-2"}}\n' "$id" ;;
  esac
done"#;

        let probe = probe_stdio(
            "sh",
            &["-c".to_string(), script.to_string()],
            &IndexMap::new(),
            tmp.path(),
            Duration::from_secs(10),
        )
        .expect("a paginating server is a healthy server");
        assert_eq!(
            probe.tool_count,
            Some(2),
            "the second page was dropped — nextCursor was not followed"
        );
    }

    /// The `--probe` bound, end to end: a server that starts and then says
    /// nothing must not hold doctor open, and must not survive it. The
    /// grandchild is the point — a real `npx` server is a launcher that
    /// spawns the actual process, so killing only the command we spawned
    /// would leave the server itself running forever.
    #[test]
    fn a_silent_server_is_killed_with_its_whole_process_group() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let leader = tmp.path().join("leader.pid");
        let grandchild = tmp.path().join("grandchild.pid");
        // Test-authored paths, not repository content — the invariant against
        // interpolating into a shell is about hostile input, and a temp dir we
        // just created is neither hostile nor attacker-influenced.
        let script = format!(
            "sleep 30 & echo $! > {}; echo $$ > {}; wait",
            grandchild.display(),
            leader.display()
        );

        let started = Instant::now();
        let err = probe_stdio(
            "sh",
            &["-c".to_string(), script],
            &IndexMap::new(),
            tmp.path(),
            Duration::from_millis(600),
        )
        .expect_err("a server that never answers cannot succeed");
        let elapsed = started.elapsed();

        assert!(
            matches!(err, StdioProbeError::Timeout { .. }),
            "expected a timeout, got: {err}"
        );
        // The deadline is a wall, not a suggestion: 600ms budget against a
        // child that wanted 30s. The slack covers the shutdown ladder.
        assert!(
            elapsed < Duration::from_secs(3),
            "probe ran {elapsed:?} — the hard timeout did not bound it"
        );

        let read_pid = |p: &std::path::Path| -> i32 {
            std::fs::read_to_string(p)
                .unwrap_or_default()
                .trim()
                .parse()
                .unwrap_or(0)
        };
        let (lead, grand) = (read_pid(&leader), read_pid(&grandchild));
        assert!(lead > 0 && grand > 0, "the child never recorded its pids");
        // The grandchild is reparented on death, so its reaper is init, not
        // us — poll briefly rather than assuming the collection already
        // happened. The leader we reap ourselves, so it is gone on return.
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline
            && (crate::sys::pid_alive(lead) || crate::sys::pid_alive(grand))
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!crate::sys::pid_alive(lead), "the probed command survived");
        assert!(
            !crate::sys::pid_alive(grand),
            "the probed command's own child survived — the group kill missed it"
        );
    }

    /// Regression: the stderr capture keeps only a prefix, but it has to keep
    /// READING past it. An earlier version stopped at the cap, which closed the
    /// pipe and handed a chatty-but-perfectly-healthy server a SIGPIPE partway
    /// through startup — the probe killed the thing it was measuring and then
    /// reported it as broken. Servers log to stderr; that must never be fatal.
    #[test]
    fn a_server_that_floods_stderr_still_probes_clean() {
        // ~120KB of boot logging, thirty times the keep-cap, then a normal
        // `initialize` reply once the probe writes its request.
        let script = r#"i=0
while [ $i -lt 3000 ]; do echo 'boot: initializing subsystem ..............' >&2; i=$((i+1)); done
read x
echo '{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"chatty","version":"1"}}}'"#;

        let probe = probe_stdio(
            "sh",
            &["-c".to_string(), script.to_string()],
            &IndexMap::new(),
            std::path::Path::new("."),
            Duration::from_secs(10),
        )
        .expect("a server that merely logs a lot is a healthy server");
        assert_eq!(probe.server_name.as_deref(), Some("chatty"));
    }

    /// A failing server's stderr reaches a terminal, so it is hostile output.
    /// Escape sequences are stripped and the length is capped before it is
    /// ever printed.
    #[test]
    fn child_stderr_is_stripped_of_escapes_and_capped() {
        let err = StdioProbeError::Exited {
            status: "exit status: 3".to_string(),
            stderr: format!("\x1b[2J\x1b]0;pwned\x07FATAL: boom\n{}", "A".repeat(50_000)),
        };
        let shown = err.to_string();
        assert!(shown.contains("FATAL: boom"), "{shown}");
        assert!(
            !shown.contains('\x1b') && !shown.contains('\x07'),
            "an escape sequence reached the terminal: {shown:?}"
        );
        assert!(
            shown.chars().count() < 250,
            "50KB of child stderr was not capped: {} chars",
            shown.chars().count()
        );
    }
}
