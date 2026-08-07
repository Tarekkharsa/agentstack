// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `agentstack search` with no query must LIST, offline.
//!
//! The gap this closes: `search`'s own `--help` said the query "lists all if
//! omitted", and the command printed a usage line instead — a surface
//! promising one thing and doing another. It mattered more than a wording slip
//! because the empty-project guidance rung offers bare `agentstack search`, so
//! a person told to "find a server or skill to add" was handed a usage line
//! and no list.
//!
//! Two properties are asserted, and the second is what makes the first
//! evidence rather than a hope:
//!
//! 1. Bare `search` lists what the two LOCAL sources hold, and says in the
//!    output which sources it consulted and which it left out.
//! 2. It makes **no request to the registry at all**. The proof is a real
//!    HTTP server bound on loopback that counts the connections it accepts and
//!    answers instantly, pointed at by `AGENTSTACK_REGISTRY_URL`. Bare
//!    `search` must leave that counter at zero — and the negative control in
//!    the same test runs `search <query>` against the same server and requires
//!    the counter to move, so a zero above cannot be a probe that never
//!    worked. A counter that can never rise proves nothing.
//!
//! The binary is spawned with a cleared environment and an isolated `HOME`:
//! the listing reads the developer's real central library otherwise, and every
//! assertion here would then depend on the machine it ran on.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Out {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

fn run(args: &[&str], home: &Path, proj: &Path, registry: &str) -> Out {
    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(proj)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("AGENTSTACK_REGISTRY_URL", registry)
        .env("PATH", "/usr/bin:/bin")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn agentstack");
    Out {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code(),
    }
}

/// A registry that answers "no servers" instantly and counts who asked.
///
/// Answering instantly rather than hanging is deliberate: a hang would prove
/// the same thing through a stopwatch, and a stopwatch assertion is a flake
/// waiting for a loaded CI box. A connection counter is the same evidence
/// without the timing.
fn counting_registry() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            counter.fetch_add(1, Ordering::SeqCst);
            // Read whatever the client sent before replying, so the client
            // sees a complete exchange rather than a reset connection.
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let body = br#"{"servers":[]}"#;
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = stream.write_all(body);
            let _ = stream.flush();
        }
    });
    (format!("http://{addr}"), hits)
}

/// Bare `search` lists from the local sources, names what it skipped, and
/// never touches the registry — with the negative control that a query does.
#[test]
fn a_bare_search_lists_offline_and_a_query_is_what_reaches_the_registry() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&proj).unwrap();
    let (registry, hits) = counting_registry();

    let listing = run(&["search"], &home, &proj, &registry);

    assert_eq!(
        listing.code,
        Some(0),
        "bare search must succeed: {listing:?}"
    );
    assert!(
        listing.stderr.is_empty(),
        "bare search is offered as guidance, so it must be quiet on stderr: {:?}",
        listing.stderr
    );

    // It LISTS: a source heading, capability rows, and the step that adds
    // one. (Which catalog entries land on the first page is the paging's
    // business; the complete list is asserted through `--json` below, which is
    // never paginated.)
    assert!(
        listing.stdout.contains("catalog"),
        "no query means list what the local sources hold:\n{}",
        listing.stdout
    );
    assert!(
        listing.stdout.contains("agentstack add from "),
        "a listed capability carries the step that adds it:\n{}",
        listing.stdout
    );
    assert!(
        listing.stdout.contains("you can add:"),
        "the headline counts what is listable:\n{}",
        listing.stdout
    );

    // It says which sources it consulted and which it left out. A listing that
    // silently dropped the registry would read as "this is the whole
    // ecosystem".
    for phrase in [
        "your central library and the built-in catalog",
        "no network",
        "MCP Registry",
        "agentstack search <query>",
    ] {
        assert!(
            listing.stdout.contains(phrase),
            "the listing must state {phrase:?} plainly:\n{}",
            listing.stdout
        );
    }

    // The defect itself: the usage line is gone, replaced by the list its own
    // `--help` promised.
    assert!(
        !listing.stdout.contains("Usage: agentstack search"),
        "a usage line is what this replaced:\n{}",
        listing.stdout
    );

    // Nothing was asked of the registry.
    let after_listing = hits.load(Ordering::SeqCst);
    assert_eq!(
        after_listing, 0,
        "a listing must not reach the network; the registry saw {after_listing} connection(s)"
    );

    // ── Negative control. The counter has to be able to move, or the zero
    // above is a probe that never worked and proves nothing.
    let searched = run(&["search", "github"], &home, &proj, &registry);
    assert_eq!(
        searched.code,
        Some(0),
        "a query search must succeed: {searched:?}"
    );
    assert!(
        hits.load(Ordering::SeqCst) > after_listing,
        "negative control failed: a QUERY must reach the registry, so the counter \
         must be able to rise — it stayed at {after_listing}"
    );

    // And the second half of the control: the listing's source note belongs to
    // the listing. Printed unconditionally it would tell a person who DID
    // search that the registry was skipped, which is the same class of lie in
    // the other direction.
    assert!(
        !searched.stdout.contains("no network"),
        "the listing's source note must not appear on a query search:\n{}",
        searched.stdout
    );
}

/// `--json` lists the same thing the screen lists, in the documented shape
/// (`json-reads-v1`: `query` + `results[]`), and only from local sources.
#[test]
fn the_json_form_of_a_bare_search_lists_too() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&proj).unwrap();
    let (registry, hits) = counting_registry();

    let out = run(&["search", "--json"], &home, &proj, &registry);
    assert_eq!(out.code, Some(0), "search --json must succeed: {out:?}");

    let v: serde_json::Value = serde_json::from_str(&out.stdout)
        .unwrap_or_else(|e| panic!("search --json is not JSON ({e}):\n{}", out.stdout));
    assert_eq!(
        v["query"], "",
        "the echoed query is the empty one asked for"
    );

    let results = v["results"].as_array().expect("results[] is an array");
    assert!(
        !results.is_empty(),
        "the JSON form lists what the screen lists:\n{}",
        out.stdout
    );
    // `github` ships in the embedded catalog, so a listing that consulted the
    // catalog at all carries it — and `--json` is unpaginated, so this is the
    // place that can say so without depending on the screen's page.
    assert!(
        results
            .iter()
            .any(|r| r["name"] == "github" && r["source"] == "catalog"),
        "the whole catalog is listed, `github` included:\n{}",
        out.stdout
    );

    // Every row came from a local source. A `registry` row here would mean the
    // listing made a network call after all.
    for r in results {
        let source = r["source"].as_str().unwrap_or_default();
        assert!(
            source == "library" || source == "catalog",
            "a listing is local-only, got source {source:?}:\n{}",
            out.stdout
        );
    }
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the JSON listing must not reach the network either"
    );
}

impl std::fmt::Debug for Out {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "exit {:?}\n--- stdout ---\n{}--- stderr ---\n{}",
            self.code, self.stdout, self.stderr
        )
    }
}
