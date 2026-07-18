//! The local web dashboard (PLAN §9f): an embedded localhost server + a
//! self-contained UI, both baked into the single binary. Read-only — it
//! surfaces state and diffs; every mutation happens through the CLI.

pub mod server;
pub mod snapshot;

pub use server::serve;
