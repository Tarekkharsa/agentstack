//! Small shared helpers.

pub mod atomic;
pub mod diff;
pub mod fsx;
pub mod paths;

/// A process-wide lock for tests that mutate the global `AGENTSTACK_HOME` env
/// var, so they don't clobber each other under cargo's parallel test runner.
#[cfg(test)]
pub static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
