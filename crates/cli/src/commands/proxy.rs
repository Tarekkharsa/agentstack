//! `agentstack proxy` — the runtime wire lens.
//!
//! `proxy start` stands up a loopback proxy in front of the Anthropic API and
//! relays every request VERBATIM (observe only — nothing is injected, the
//! tools/system block is never touched, so the prompt-prefix cache stays warm).
//! As requests flow, it accounts what the `tools` block costs in input tokens
//! per turn and stashes best-effort per-capability numbers plus which tools the
//! model actually called.
//!
//! `proxy report` aggregates that telemetry into a ranked, per-capability view:
//! tokens/turn, calls, and a loaded-vs-called hint — the ground-truth companion
//! to the static estimate in `agentstack report usage`.

use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::{ProxyCmd, ProxyReportArgs, ProxyStartArgs};
use crate::footprint::fmt_tokens;
use crate::proxy::{self, ProxyConfig, Report};

pub fn run(cmd: &ProxyCmd, _manifest_dir: Option<&Path>) -> Result<()> {
    match cmd {
        ProxyCmd::Start(args) => start(args),
        ProxyCmd::Report(args) => report(args),
    }
}

fn start(args: &ProxyStartArgs) -> Result<()> {
    let config = ProxyConfig {
        port: args.port,
        upstream: args.upstream.clone(),
    };
    proxy::serve(config)
}

fn report(args: &ProxyReportArgs) -> Result<()> {
    let records = proxy::read_all();
    let report = proxy::aggregate(&records);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    print_human(&report);
    Ok(())
}

fn print_human(report: &Report) {
    if report.requests == 0 {
        println!(
            "No wire activity observed yet. Start the proxy with `agentstack proxy start`, point"
        );
        println!(
            "your harness at {}, then run `agentstack proxy report`.",
            "ANTHROPIC_BASE_URL=http://127.0.0.1:8787".bold()
        );
        return;
    }

    // Headline: the per-turn weight (max seen in a single request) across all
    // observed requests.
    println!(
        "{} tools, ~{} tokens/turn observed across {} request{}",
        report.total_tools.to_string().bold(),
        fmt_tokens(report.total_est_tokens).bold(),
        report.requests,
        if report.requests == 1 { "" } else { "s" },
    );
    println!();

    println!(
        "{:<24} {:>6}  {:>18}  {:>7}  {}",
        "capability".bold(),
        "tools".bold(),
        "avg tokens/turn".bold(),
        "calls".bold(),
        "hint".bold(),
    );
    for cap in &report.capabilities {
        let hint = match cap.hint.as_str() {
            "drop / lazy" => cap.hint.yellow().to_string(),
            "keep" => cap.hint.green().to_string(),
            _ => cap.hint.dimmed().to_string(),
        };
        println!(
            "{:<24} {:>6}  {:>18}  {:>7}  {}",
            cap.capability,
            cap.tools,
            fmt_tokens(cap.avg_est_tokens),
            cap.calls,
            hint,
        );
    }

    println!();
    println!(
        "{}",
        "loaded vs called — a capability whose tools cost the most tokens/turn but were never"
            .dimmed()
    );
    println!(
        "{}",
        "called this window is the first candidate to drop or make lazy.".dimmed()
    );
}
