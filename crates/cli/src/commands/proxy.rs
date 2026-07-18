//! `agentstack proxy` — the runtime wire relay.
//!
//! The bare `proxy` command stands up a loopback proxy in front of the Anthropic
//! API and relays every request VERBATIM (observe only — nothing is injected,
//! the tools/system block is never touched, so the prompt-prefix cache stays
//! warm). As requests flow, it accounts what the `tools` block costs in input
//! tokens per turn and stashes best-effort per-capability numbers plus which
//! tools the model actually called.
//!
//! The ranked, per-capability view over that telemetry lives under
//! `agentstack report wire` (see `commands::report::wire`) — the ground-truth
//! companion to the static estimate in `agentstack report usage`.

use std::path::Path;

use anyhow::Result;

use crate::cli::ProxyStartArgs;
use crate::proxy::{self, ProxyConfig};

pub fn run(args: &ProxyStartArgs, _manifest_dir: Option<&Path>) -> Result<()> {
    let config = ProxyConfig {
        port: args.port,
        upstream: args.upstream.clone(),
    };
    proxy::serve(config)
}
