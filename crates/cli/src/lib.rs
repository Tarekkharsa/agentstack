//! agentstack — one portable manifest, every agent CLI.
//!
//! Library surface so both the `agentstack` binary and the integration tests
//! can drive the same code. The module layout mirrors the data flow:
//! [`manifest`] (source of truth) → [`secret`] (resolve `${REF}`s) →
//! [`adapter`] (per-CLI descriptors + generic render) → [`render`]
//! (non-destructive merge into native config).

// Unsafe is denied crate-wide; the sole sanctioned exception is `sys`, which
// concentrates every libc / raw-fd / pre_exec call behind safe wrappers so the
// entire unsafe surface is one greppable file (CLAUDE.md rule 1). `deny`, not
// `forbid`, because `forbid` can't be locally downgraded by this `#[allow]`.
#![deny(unsafe_code)]

// TODO(phase-1): shim — migrate callers to agentstack_adapters:: and drop.
pub use agentstack_adapters as adapter;
// TODO(phase-1): shim — migrate callers to agentstack_recorder:: and drop.
pub use agentstack_recorder as calllog;
pub mod catalog;
pub mod cli;
pub mod codemode;
pub mod commands;
pub mod discover;
pub mod executable;
pub mod execution;
pub mod footprint;
pub mod gateway;
pub mod gateway_http;
pub mod gitx;
pub mod grant;
pub mod guard;
pub mod history;
pub mod intake;
pub mod library;
pub mod machine_policy;
// TODO(phase-1): re-export shims — migrate callers to agentstack_core:: paths
// and drop these, so the crate graph (not cli) is what exposes core types.
pub use agentstack_core::lock;
pub mod manifest;
pub mod mcp;
pub mod mcp_server;
pub mod provider;
pub mod proxy;
pub mod recognition;
pub mod regate;
pub mod render;
pub mod resolve;
pub mod runs;
pub mod scan;
pub use agentstack_core::scope;
pub mod seatbelt;
pub mod secret;
pub mod session;
pub mod snapshot;
pub mod state;
pub mod store;
#[allow(unsafe_code)]
pub(crate) mod sys;
pub mod text;
pub mod ui_contract;
pub mod update;
// The binary calls this before its first print; the module itself stays
// crate-private so the unsafe surface is reachable only through this fn.
pub use sys::reset_sigpipe;

/// `println!` that drops write errors instead of panicking on them.
///
/// Paired with [`sys::SigpipeIgnored`]: inside a write pass a reader hanging
/// up must cost the user some *output*, never a half-finished set of file
/// writes. Everywhere else keep using `println!` — a silent failure to print
/// is only acceptable where the alternative is worse.
#[macro_export]
macro_rules! outln {
    () => {{
        use std::io::Write;
        let _ = writeln!(std::io::stdout());
    }};
    ($($arg:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

/// `print!` counterpart of [`outln`] — same contract, no trailing newline.
#[macro_export]
macro_rules! out {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let _ = write!(std::io::stdout(), $($arg)*);
    }};
}
// TODO(phase-1): shim — migrate callers to agentstack_trust:: and drop.
pub use agentstack_trust as trust;
pub mod usage;
pub mod verify;
pub mod workflows;
pub use agentstack_core::util;
