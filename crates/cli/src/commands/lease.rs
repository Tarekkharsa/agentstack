//! `agentstack lease status` — the authoritative read of the runtime lease
//! registry (W4, `docs/design/automatic-delivery.md` §"Lease lifecycle").
//!
//! Every other surface (`doctor`, a panel on `lease-status-v1`) reads leases
//! through this one command, so there is a single place where a record becomes
//! a liveness claim — [`crate::lease_registry::liveness`] — and no surface can
//! invent a second, friendlier answer.

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::lease_registry::{self, Liveness};

/// The honest scope note, printed and emitted verbatim. A lease is owned by one
/// MCP process; nothing survives that process, and no record here is a promise
/// that anything is still running (see `liveness`).
const SCOPE_NOTE: &str = "A lease is process-scoped: it disappears with the process that owns it. \
     Liveness is derived at read time from the recorded PID and that process's start time — a \
     record is never read as truth on its own.";

pub fn run(json: bool) -> Result<()> {
    let leases = lease_registry::open_leases();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::ui_contract::envelope(body(&leases)))?
        );
        return Ok(());
    }
    print_screen(&leases);
    Ok(())
}

fn body(leases: &[(lease_registry::LeaseRecord, Liveness)]) -> serde_json::Value {
    serde_json::json!({
        "leases": leases
            .iter()
            .map(|(record, state)| serde_json::json!({
                "instance": crate::text::sanitize_line(&record.instance),
                // Manifest-sourced strings are bounded and stripped on the way
                // out like every other repository-derived value (invariant 7).
                "project": crate::text::sanitize_line(&record.project),
                "toolset": crate::text::sanitize_line(&record.toolset),
                "pid": record.pid,
                "started_unix": record.started_unix,
                "liveness": state.as_str(),
                "why": state.why(),
            }))
            .collect::<Vec<_>>(),
        "note": SCOPE_NOTE,
    })
}

fn print_screen(leases: &[(lease_registry::LeaseRecord, Liveness)]) {
    if leases.is_empty() {
        println!("  {}  no lease records on this machine", "Leases".bold());
        println!("  {}", SCOPE_NOTE.dimmed());
        return;
    }
    println!(
        "  {}  {}",
        "Leases".bold(),
        super::count(leases.len(), "lease record")
    );
    for (record, state) in leases {
        let marker = match state {
            Liveness::Live => "✓".green().to_string(),
            Liveness::Stale => "·".dimmed().to_string(),
            Liveness::Unknown => "?".yellow().to_string(),
        };
        println!(
            "  {marker} {:<10} {:<8} pid {} · {} · instance {}",
            crate::text::sanitize_line(&record.toolset),
            state.as_str(),
            record.pid,
            crate::text::sanitize_line(&record.project),
            crate::text::sanitize_line(&record.instance),
        );
        if *state != Liveness::Live {
            println!("      {}", state.why().dimmed());
        }
    }
    println!("  {}", SCOPE_NOTE.dimmed());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_json_body_never_claims_liveness_a_record_cannot_support() {
        let record = lease_registry::LeaseRecord {
            instance: "abc".into(),
            project: "/tmp/proj".into(),
            toolset: "backend".into(),
            pid: 2_000_000_000,
            start_token: Some("whenever".into()),
            started_unix: 42,
        };
        let out = body(&[(record, Liveness::Stale)]);
        assert_eq!(out["leases"][0]["liveness"], "stale");
        assert_eq!(out["leases"][0]["toolset"], "backend");
        assert!(out["note"].as_str().unwrap().contains("process-scoped"));
    }
}
