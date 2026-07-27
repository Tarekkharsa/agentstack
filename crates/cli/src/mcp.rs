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
use serde_json::{json, Value};

const PROTOCOL_VERSION: &str = "2025-06-18";

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
    /// Other non-success HTTP status.
    Http(u16),
    /// Could not connect / timed out / TLS error.
    Connect(String),
    /// Connected, but the response wasn't a usable MCP handshake.
    Protocol(String),
}

impl std::fmt::Display for LiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LiveError::Auth(code) => write!(f, "{code} unauthorized"),
            LiveError::Http(code) => write!(f, "HTTP {code}"),
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
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| LiveError::Connect(e.to_string()))?;

    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "agentstack", "version": env!("CARGO_PKG_VERSION") }
        }
    });

    let resp = post(&client, url, headers, None, &init)?;
    let status = resp.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(LiveError::Auth(status.as_u16()));
    }
    if !status.is_success() {
        return Err(LiveError::Http(status.as_u16()));
    }
    let session = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = resp
        .text()
        .map_err(|e| LiveError::Protocol(e.to_string()))?;
    let result = extract_result(&body)
        .ok_or_else(|| LiveError::Protocol("no result in initialize response".into()))?;

    let server_name = result
        .get("serverInfo")
        .and_then(|s| s.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let protocol = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .map(str::to_string);

    // Best-effort: complete the handshake and count tools. Failures here don't
    // invalidate a successful initialize.
    let tool_count = count_tools(&client, url, headers, session.as_deref());

    Ok(Handshake {
        server_name,
        protocol,
        tool_count,
    })
}

fn count_tools(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &IndexMap<String, String>,
    session: Option<&str>,
) -> Option<usize> {
    let initialized = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    let _ = post(client, url, headers, session, &initialized);

    let list = json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}});
    let resp = post(client, url, headers, session, &list).ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.text().ok()?;
    let result = extract_result(&body)?;
    result
        .get("tools")
        .and_then(Value::as_array)
        .map(|t| t.len())
}

fn post(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: &IndexMap<String, String>,
    session: Option<&str>,
    body: &Value,
) -> Result<reqwest::blocking::Response, LiveError> {
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    for (k, v) in headers {
        req = req.header(k, v);
    }
    if let Some(s) = session {
        req = req.header("Mcp-Session-Id", s);
    }
    req.json(body)
        .send()
        .map_err(|e| LiveError::Connect(e.to_string()))
}

/// Parse a JSON-RPC `result` from a body that may be plain JSON or an SSE
/// stream (`data: {...}` lines).
fn extract_result(body: &str) -> Option<Value> {
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') {
        let v: Value = serde_json::from_str(trimmed).ok()?;
        return v.get("result").cloned();
    }
    // SSE: find the first data line carrying a result.
    for line in body.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            if let Ok(v) = serde_json::from_str::<Value>(data.trim()) {
                if let Some(r) = v.get("result") {
                    return Some(r.clone());
                }
            }
        }
    }
    None
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

/// How much of a probed child's stderr is KEPT, in bytes. Unbounded hostile
/// output must not grow host memory; this is enough for the one line that
/// usually explains the failure ("command not found", "missing API key").
/// Note "kept", not "read" — see the reader in [`probe_stdio`].
const STDERR_CAP: usize = 4096;

/// How much of that capture is shown. Much smaller than [`STDERR_CAP`]: the
/// line that explains a failure ("command not found", "missing API key") is
/// short and comes first — newlines became spaces, so the front of the string
/// is the front of the output — while the rest is a stack trace that would
/// bury the other servers' results.
const STDERR_DISPLAY_CHARS: usize = 160;

/// How much of a probed child's stdout is read, in bytes. The probe wants two
/// small replies; only a `tools/list` from a very large server needs more than
/// a few KiB. Past this the reader stops for real (unlike stderr above, which
/// keeps draining): the memory bound has to cover an unbounded single line,
/// and `lines()` cannot be capped per line without reading it first. The
/// SIGPIPE that closing the pipe hands the child is acceptable here in a way
/// it is not on stderr — a server that has written two megabytes to its
/// PROTOCOL channel without answering `initialize` is already failing, and the
/// probe is about to kill it anyway.
const STDOUT_CAP: u64 = 2 * 1024 * 1024;

/// Bound on queued stdout frames. The probe consumes at most two replies;
/// anything beyond this is a server talking to nobody, and a full queue parks
/// the reader thread rather than buffering without limit.
const STDOUT_QUEUE_CAP: usize = 64;

/// A probed child process, bounded by construction: whatever happens to the
/// probe — success, timeout, protocol error, an early `?`, a panic — `Drop`
/// runs the same shutdown ladder, so nothing this module spawns outlives the
/// call. (The sibling implementation is `gateway::StdioChild`, which keeps a
/// child alive across calls and so cannot be shared with a one-shot probe.)
struct ProbeChild {
    proc: std::process::Child,
    /// `Some` until shutdown; closing it is EOF on the child's stdin, the
    /// polite MCP shutdown signal.
    stdin: Option<std::process::ChildStdin>,
    reaped: bool,
    /// How the child ended, recorded wherever it is collected. Kept because
    /// "exit status: 3" tells the user far more about a server that gave up
    /// than "it stopped" does — and the status is gone once the child is
    /// reaped, so it has to be captured at that moment.
    status: Option<std::process::ExitStatus>,
}

impl ProbeChild {
    /// Poll `try_wait` for up to `dur`; true once the child has exited (and,
    /// because `try_wait` reaps, has been collected).
    fn wait_for_exit(&mut self, dur: Duration) -> bool {
        let deadline = Instant::now() + dur;
        loop {
            if let Ok(Some(s)) = self.proc.try_wait() {
                self.status = Some(s);
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// How the child ended, for the report. `ExitStatus`'s own Display is
    /// already the right phrasing on both platforms ("exit status: 3",
    /// "signal: 9 (SIGKILL)"); the fallback covers the case where the child
    /// was never collected, which `terminate` makes unreachable in practice.
    fn exit_label(&self) -> String {
        match self.status {
            Some(s) => s.to_string(),
            None => "exit status unknown".to_string(),
        }
    }

    /// Stop the child and everything it spawned, then reap it. Idempotent, so
    /// the failure paths can call it explicitly (they need the child dead
    /// before they read its stderr to EOF) and `Drop` can call it again.
    ///
    /// Escalation ladder: stdin EOF → SIGTERM to the process group → SIGKILL
    /// to the group. The child is its own group leader (see `probe_stdio`), so
    /// a server that spawned helpers takes them with it.
    fn terminate(&mut self) {
        if self.reaped {
            return;
        }
        self.reaped = true;
        drop(self.stdin.take());
        if self.wait_for_exit(Duration::from_millis(200)) {
            return;
        }
        #[cfg(unix)]
        {
            let pgid = self.proc.id() as i32;
            let _ = crate::sys::signal_group(pgid, crate::sys::Signal::Term);
            if self.wait_for_exit(Duration::from_millis(300)) {
                return;
            }
            let _ = crate::sys::signal_group(pgid, crate::sys::Signal::Kill);
        }
        #[cfg(not(unix))]
        {
            let _ = self.proc.kill();
        }
        if let Ok(s) = self.proc.wait() {
            self.status = Some(s);
        }
    }
}

impl Drop for ProbeChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

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
    use std::io::{BufRead, Read, Write};

    let started = Instant::now();
    let deadline = started + timeout;

    let mut cmd = std::process::Command::new(command);
    cmd.args(args)
        .envs(env.iter().map(|(k, v)| (k.clone(), v.clone())))
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        // Captured, not inherited: a probe reports what the child said, and a
        // failing server's stderr is usually the whole diagnosis. Inheriting
        // would also let hostile bytes reach the terminal unsanitized.
        .stderr(std::process::Stdio::piped());
    // Own process group, so `terminate` can tree-kill the child and anything
    // it spawns.
    crate::sys::spawn_in_new_process_group(&mut cmd);

    let mut proc = match cmd.spawn() {
        Ok(p) => p,
        Err(e) => return Err(StdioProbeError::Spawn(e.to_string())),
    };
    let stdin = proc.stdin.take().expect("piped stdin");
    let stdout = proc.stdout.take().expect("piped stdout");
    let stderr = proc.stderr.take().expect("piped stderr");
    let mut child = ProbeChild {
        proc,
        stdin: Some(stdin),
        reaped: false,
        status: None,
    };

    // Both pipes get their own reader thread: reading one to completion while
    // the other fills would deadlock the child on a full pipe buffer.
    let (errtx, errrx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        // Keep the first STDERR_CAP bytes, but keep DRAINING everything after
        // them. Simply stopping the read would close the pipe and hand a
        // chatty-but-healthy server a SIGPIPE partway through startup — the
        // probe would kill the thing it is measuring and then report it as
        // broken. Servers log to stderr; that must never be fatal here.
        let mut stderr = stderr;
        let mut kept: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(n) = stderr.read(&mut chunk) {
            if n == 0 {
                break;
            }
            let room = STDERR_CAP.saturating_sub(kept.len());
            if room > 0 {
                kept.extend_from_slice(&chunk[..n.min(room)]);
            }
        }
        let _ = errtx.send(String::from_utf8_lossy(&kept).into_owned());
    });
    let (outtx, outrx) = std::sync::mpsc::sync_channel::<Value>(STDOUT_QUEUE_CAP);
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout.take(STDOUT_CAP)).lines() {
            let Ok(line) = line else { break };
            // Non-JSON stdout is noise, not a protocol frame — servers do log
            // there. Only JSON-RPC frames go through.
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if outtx.send(v).is_err() {
                    break;
                }
            }
        }
    });

    // Collect the child's stderr once it is dead, so the reader thread has
    // seen EOF and the capture is complete. Bounded either way: the recv
    // cannot outlast a process we just killed by more than the grace below.
    let drain_stderr = |child: &mut ProbeChild| -> String {
        child.terminate();
        errrx
            .recv_timeout(Duration::from_millis(500))
            .unwrap_or_default()
    };

    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "agentstack", "version": env!("CARGO_PKG_VERSION") }
        }
    });
    {
        let pipe = child.stdin.as_mut().expect("stdin open until shutdown");
        if let Err(e) = writeln!(pipe, "{init}").and_then(|()| pipe.flush()) {
            // A write that fails this early means the child is already gone —
            // a bad interpreter, an immediate exit. Report what it said.
            let stderr = drain_stderr(&mut child);
            return Err(StdioProbeError::Exited {
                status: e.to_string(),
                stderr,
            });
        }
    }

    let result = match await_reply(&outrx, &json!(1), deadline) {
        Ok(msg) => msg,
        Err(Wait::Timeout) => {
            let stderr = drain_stderr(&mut child);
            return Err(StdioProbeError::Timeout {
                after: timeout,
                stderr,
            });
        }
        Err(Wait::Closed) => {
            // stdout closed: the child exited, or stopped writing frames.
            // `drain_stderr` terminates it first, which is also what records
            // the exit status this reports.
            let stderr = drain_stderr(&mut child);
            return Err(StdioProbeError::Exited {
                status: child.exit_label(),
                stderr,
            });
        }
    };
    let elapsed = started.elapsed();

    if let Some(err) = result.get("error") {
        let stderr = drain_stderr(&mut child);
        return Err(StdioProbeError::Protocol {
            detail: crate::text::one_line(&err.to_string(), 200),
            stderr,
        });
    }
    let Some(res) = result.get("result") else {
        let stderr = drain_stderr(&mut child);
        return Err(StdioProbeError::Protocol {
            detail: "no result in initialize response".to_string(),
            stderr,
        });
    };

    let server_name = res
        .get("serverInfo")
        .and_then(|s| s.get("name"))
        .and_then(Value::as_str)
        // Upstream metadata is remote text on its way to a terminal.
        .map(crate::text::sanitize_line);
    let protocol = res
        .get("protocolVersion")
        .and_then(Value::as_str)
        .map(crate::text::sanitize_line);

    // Best-effort, inside the same deadline: completing the handshake and
    // counting tools proves the server is usable, but a server that came up
    // and then dawdles here is still a server that came up.
    let tool_count = stdio_tool_count(&mut child, &outrx, deadline);

    // Success path still stops the child: a probe leaves nothing running.
    child.terminate();

    Ok(StdioProbe {
        server_name,
        protocol,
        tool_count,
        elapsed,
    })
}

/// Why [`await_reply`] gave up. Distinguished because they mean different
/// things to the user: a hang and an exit are different bugs.
enum Wait {
    Timeout,
    Closed,
}

/// Wait for the JSON-RPC response carrying `id`, until `deadline`. Server
/// notifications and stale replies are skipped, not fatal.
fn await_reply(
    rx: &std::sync::mpsc::Receiver<Value>,
    id: &Value,
    deadline: Instant,
) -> Result<Value, Wait> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(msg) => {
                if msg.get("id") == Some(id) && msg.get("method").is_none() {
                    return Ok(msg);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Err(Wait::Timeout),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(Wait::Closed),
        }
    }
}

/// `notifications/initialized` then `tools/list`, returning the tool count.
/// Every failure here is `None`, never an error: the handshake already
/// succeeded, and that is what the probe set out to prove.
fn stdio_tool_count(
    child: &mut ProbeChild,
    rx: &std::sync::mpsc::Receiver<Value>,
    deadline: Instant,
) -> Option<usize> {
    use std::io::Write;
    let pipe = child.stdin.as_mut()?;
    let ready = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    let list = json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}});
    writeln!(pipe, "{ready}")
        .and_then(|()| writeln!(pipe, "{list}"))
        .and_then(|()| pipe.flush())
        .ok()?;
    let msg = await_reply(rx, &json!(2), deadline).ok()?;
    msg.get("result")?
        .get("tools")
        .and_then(Value::as_array)
        .map(Vec::len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json_result() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"x","serverInfo":{"name":"kibana"}}}"#;
        let r = extract_result(body).unwrap();
        assert_eq!(r["serverInfo"]["name"], "kibana");
    }

    #[test]
    fn parses_sse_result() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"x\"}}\n\n";
        let r = extract_result(body).unwrap();
        assert_eq!(r["protocolVersion"], "x");
    }

    #[test]
    fn no_result_returns_none() {
        assert!(extract_result("{\"error\":{}}").is_none());
        assert!(extract_result("garbage").is_none());
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
echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","serverInfo":{"name":"chatty"}}}'"#;

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
