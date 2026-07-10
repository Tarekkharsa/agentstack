//! Small shared helpers.

pub mod atomic;
pub mod confirm;
pub mod diff;
pub mod fsx;
pub mod paths;

/// 32 bytes from the OS entropy pool, with a time/pid-mixed hash fallback
/// where /dev/urandom is unavailable. Shared by every credential-ish secret
/// agentstack mints locally (call-log digest key, code-mode endpoint token).
pub fn random_bytes() -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            let mut buf = vec![0u8; 32];
            if f.read_exact(&mut buf).is_ok() {
                return buf;
            }
        }
    }
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            .to_le_bytes(),
    );
    h.update(std::process::id().to_le_bytes());
    h.finalize().to_vec()
}

/// Best-effort permission tightening for files/dirs holding local secrets
/// (0600 files, 0700 dirs on unix; no-op elsewhere). Applied after creation
/// too, so pre-existing artifacts from before a hardening change get fixed.
pub fn restrict(path: &std::path::Path, dir: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if dir { 0o700 } else { 0o600 };
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (path, dir);
}

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
/// Compiled unconditionally (not `#[cfg(test)]`) because `cfg(test)` does not
/// propagate across crates — the cli crate's tests take this lock too. A
/// never-contended `Mutex<()>` static is free in release builds.
pub static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
