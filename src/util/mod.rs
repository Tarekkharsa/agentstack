//! Small shared helpers.

pub mod atomic;
pub mod confirm;
pub mod diff;
pub mod fsx;
pub mod paths;

/// Guard a just-deserialized on-disk schema `version` against the newest
/// schema this build understands. Versions above `supported` come from a
/// future agentstack and must not be interpreted with today's semantics.
/// Versions in `1..=supported` pass — the range below `supported` is the seam
/// where per-format migrations hook in once a version 2 exists. `0` never
/// named a real schema and is rejected as malformed.
pub fn check_schema_version(
    version: u32,
    supported: u32,
    what: &str,
    path: &std::path::Path,
) -> anyhow::Result<()> {
    if version > supported {
        anyhow::bail!(
            "{}: {what} version {version} is newer than this agentstack build supports \
             (up to {supported}); upgrade agentstack",
            path.display()
        );
    }
    if version == 0 {
        anyhow::bail!(
            "{}: {what} version 0 is not valid (expected 1..={supported})",
            path.display()
        );
    }
    Ok(())
}

/// A process-wide lock for tests that mutate the global `AGENTSTACK_HOME` env
/// var, so they don't clobber each other under cargo's parallel test runner.
#[cfg(test)]
pub static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
