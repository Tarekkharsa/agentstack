//! Machine-local commitment key and argv commitment for the locked-run
//! `AuthorityGrant` (Phase 0A). **Not wired to any runtime path yet.**
//!
//! The `AuthorityGrant`'s canonical digest (increment 3b) binds the exact
//! invocation without recording raw argv (which may carry caller tokens or
//! passwords). It does so by committing argv under a machine-local HMAC-SHA256
//! key: an evidence reader who lacks the key cannot brute-force low-entropy
//! argv. This is dictionary-attack resistance for shipped/exfiltrated evidence,
//! NOT authenticity — the harness runs as the same user who can read the key, so
//! it is a cooperative local commitment, not tamper-proof attestation (see
//! `docs/design/locked-run-contract.md` §3.1). Portable, independently
//! verifiable authenticity would require asymmetric signing and durable key
//! management, which is out of scope here.
//!
//! Because the key is machine-stable, the same complete grant on one machine
//! digests to the same value across `--plan` and the live run (the "plan matches
//! run" property) — which also means two identical *complete* grants correlate
//! on that machine. The standalone argv commitment is likewise correlatable, so
//! it is opaque, redacted, non-serializable, and exposed only to the outer grant
//! digest — never recorded or displayed on its own.

use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::util::paths;

type HmacSha256 = Hmac<Sha256>;

/// Domain separator for the argv commitment (versioned).
const ARGV_COMMIT_DOMAIN: &[u8] = b"agentstack-argv-commit-v1\0";

/// A machine-local 32-byte HMAC key for argv commitments.
///
/// Sensitive: no `Debug`/`Serialize` derive — `Debug` is redacted and the key is
/// never serialized, so its bytes cannot leak through logs or evidence.
pub struct CommitmentKey([u8; 32]);

impl std::fmt::Debug for CommitmentKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CommitmentKey(<redacted>)")
    }
}

/// An opaque HMAC-SHA256 commitment over an argv sequence.
///
/// Correlatable: identical argv under the same machine key yields the same tag,
/// so it is NOT safe to record or display standalone. Redacted in `Debug`, never
/// serialized; its bytes will be exposed to the `AuthorityGrant` digest in
/// increment 3b (which adds the crate-internal accessor), never on their own.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ArgvCommitment([u8; 32]);

impl std::fmt::Debug for ArgvCommitment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ArgvCommitment(<redacted>)")
    }
}

/// `~/.agentstack/grant/commit-key` — the machine-local commitment key.
fn commit_key_path() -> PathBuf {
    paths::agentstack_home().join("grant").join("commit-key")
}

/// Load the commitment key for **read-only** use (grant construction, `--plan`).
///
/// Fail-closed: a missing, unreadable, malformed (not exactly 32 bytes), symlink,
/// or insecurely-permissioned key (including a symlinked or group/other-writable
/// parent directory) **blocks**. Never creates, heals, or replaces a key —
/// provisioning is a separate, explicitly-mutating operation
/// ([`provision_commitment_key`]).
pub fn load_commitment_key() -> Result<CommitmentKey> {
    let path = commit_key_path();

    // The parent must be a real directory (not a symlink) and not writable by
    // group/other: a 0600 key is still swappable through a permissive dir. (Unix.)
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        use std::os::unix::fs::PermissionsExt;
        let pmeta = std::fs::symlink_metadata(parent)
            .with_context(|| format!("commitment key directory {} is missing", parent.display()))?;
        if !pmeta.file_type().is_dir() {
            bail!(
                "commitment key directory {} is not a real directory (a symlink is refused) — refusing",
                parent.display()
            );
        }
        let pmode = pmeta.permissions().mode() & 0o777;
        if pmode & 0o022 != 0 {
            bail!(
                "commitment key directory {} is writable by group/other (mode {:o}) — refusing",
                parent.display(),
                pmode
            );
        }
    }

    // Portable symlink refusal: reject anything but a regular file before opening
    // (Unix additionally enforces this atomically via O_NOFOLLOW below).
    let lmeta = std::fs::symlink_metadata(&path).with_context(|| {
        format!(
            "commitment key {} is missing or unreadable — provision it first",
            path.display()
        )
    })?;
    if !lmeta.file_type().is_file() {
        bail!(
            "commitment key {} is not a regular file (a symlink or directory is refused) — refusing",
            path.display()
        );
    }

    // Open ONE handle without following a symlink, and validate that same handle
    // — no path-based re-read that could resolve to a different file.
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW); // a symlink makes open() fail; no unsafe
    }
    let file = opts.open(&path).with_context(|| {
        format!(
            "commitment key {} is missing, unreadable, or a symlink — provision it first",
            path.display()
        )
    })?;

    let meta = file
        .metadata()
        .with_context(|| format!("stat commitment key {}", path.display()))?;
    if !meta.file_type().is_file() {
        bail!(
            "commitment key {} is not a regular file — refusing",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            bail!(
                "commitment key {} has insecure permissions {:o} — must be readable only by its owner (0600)",
                path.display(),
                mode
            );
        }
    }

    // Bounded read: at most 33 bytes, then require exactly 32 — never allocates
    // for an arbitrarily large file, and a larger file is malformed.
    let mut buf = Vec::with_capacity(33);
    file.take(33)
        .read_to_end(&mut buf)
        .with_context(|| format!("reading commitment key {}", path.display()))?;
    if buf.len() != 32 {
        bail!(
            "commitment key {} is malformed: expected exactly 32 bytes, found {}{}",
            path.display(),
            buf.len(),
            if buf.len() > 32 { "+" } else { "" }
        );
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&buf);
    Ok(CommitmentKey(key))
}

/// Provision the machine-local commitment key: 32 CSPRNG bytes written with
/// checked, verified `0600` permissions. **Explicitly mutating** — never called
/// by grant construction or `--plan`.
///
/// - Refuses to replace an existing key: a present *valid* key is left untouched
///   (idempotent `Ok`); a present *invalid* key is a hard error the operator must
///   resolve manually — never an automatic overwrite.
/// - Atomic + race-safe: bytes go to a per-attempt **randomly-named** temp file
///   (thread-safe — no shared PID path to unlink), fsynced, then hard-linked into
///   place (the link fails if the key exists, so the final name only appears with
///   complete content and is never clobbered).
/// - Permissions are set and **verified** with checked, handle-based operations
///   (not the umask-narrowed `OpenOptions::mode` or error-swallowing `restrict`),
///   after confirming the directory is real (not a symlink whose target would be
///   chmod'd). The final key is validated by a read-only load before success.
/// - Uses a fallible CSPRNG (`getrandom`); a CSPRNG failure is a hard error, not
///   a fallback to weaker entropy.
pub fn provision_commitment_key() -> Result<()> {
    let path = commit_key_path();
    if path.exists() {
        load_commitment_key().map(|_| ()).with_context(|| {
            format!(
                "refusing to overwrite the existing commitment key at {} — resolve it manually",
                path.display()
            )
        })?;
        return Ok(());
    }
    let dir = path
        .parent()
        .with_context(|| format!("commitment key path {} has no parent", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // create_dir_all accepts a pre-existing directory SYMLINK; require a real
        // directory BEFORE any chmod so we never follow a symlink and mutate its
        // target's permissions.
        let dmeta =
            std::fs::symlink_metadata(dir).with_context(|| format!("stat {}", dir.display()))?;
        if !dmeta.file_type().is_dir() {
            bail!(
                "commitment key directory {} is not a real directory (a symlink is refused) — refusing to secure it",
                dir.display()
            );
        }
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("securing {} to 0700", dir.display()))?;
        let m = std::fs::symlink_metadata(dir)?.permissions().mode() & 0o777;
        if m & 0o077 != 0 {
            bail!(
                "commitment key directory {} could not be secured (mode {:o})",
                dir.display(),
                m
            );
        }
    }

    let mut key = [0u8; 32];
    getrandom::getrandom(&mut key).map_err(|e| {
        anyhow::anyhow!("CSPRNG failure while provisioning the commitment key: {e}")
    })?;

    // Per-attempt random suffix: thread-safe, so one provisioner can never unlink
    // another's in-flight temp file.
    let mut nonce = [0u8; 12];
    getrandom::getrandom(&mut nonce)
        .map_err(|e| anyhow::anyhow!("CSPRNG failure choosing a temp name: {e}"))?;
    let suffix: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let tmp = dir.join(format!("commit-key.tmp.{suffix}"));

    let write_tmp = || -> Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        // `OpenOptions::mode` is umask-narrowed — set + verify perms on the OPEN
        // handle (not a path-based call) before writing key bytes.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("securing {}", tmp.display()))?;
            let m = f.metadata()?.permissions().mode() & 0o777;
            if m & 0o077 != 0 {
                bail!(
                    "temp key {} could not be secured (mode {:o})",
                    tmp.display(),
                    m
                );
            }
        }
        f.write_all(&key)?;
        f.sync_all()?;
        Ok(())
    };
    if let Err(e) = write_tmp() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    // Hard-link into place: atomic (full content before the name appears) and
    // exclusive (fails if a key exists, so it is never clobbered). The final key
    // shares the temp's inode, so it inherits the verified 0600 perms; a
    // read-only load then validates it before success is reported.
    let link = std::fs::hard_link(&tmp, &path);
    let _ = std::fs::remove_file(&tmp);
    match link {
        Ok(()) => load_commitment_key().map(|_| ()).with_context(|| {
            format!(
                "provisioned commitment key at {} did not validate",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // A concurrent provisioner won — validate theirs, never overwrite.
            load_commitment_key().map(|_| ()).with_context(|| {
                format!(
                    "a commitment key already exists at {} — resolve it manually",
                    path.display()
                )
            })
        }
        Err(e) => Err(e)
            .with_context(|| format!("linking commitment key into place at {}", path.display())),
    }
}

/// HMAC-SHA256 commitment over an argv sequence, keyed by the machine-local
/// commitment key. **Order-preserving** (argv order is semantic — never sorted)
/// and unambiguous across element boundaries via length framing, so `["ab","c"]`
/// and `["a","bc"]` commit to different tags.
pub fn commit_argv(key: &CommitmentKey, argv: &[String]) -> ArgvCommitment {
    // `new_from_slice` accepts any key length; ours is always 32 bytes.
    let mut mac = HmacSha256::new_from_slice(&key.0).expect("HMAC accepts a 32-byte key");
    mac.update(ARGV_COMMIT_DOMAIN);
    mac.update(&(argv.len() as u64).to_le_bytes());
    for arg in argv {
        let b = arg.as_bytes();
        mac.update(&(b.len() as u64).to_le_bytes());
        mac.update(b);
    }
    let out = mac.finalize().into_bytes();
    let mut tag = [0u8; 32];
    tag.copy_from_slice(&out);
    ArgvCommitment(tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_home<T>(f: impl FnOnce() -> T) -> T {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let out = f();
        std::env::remove_var("AGENTSTACK_HOME");
        out
    }

    fn write_key(bytes: &[u8], _mode: u32) {
        let path = commit_key_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(_mode)).unwrap();
        }
    }

    /// RFC 4231 Test Case 1 — proves the `hmac`/`sha2` wiring.
    #[test]
    fn hmac_sha256_matches_rfc4231_vector() {
        let mut mac = HmacSha256::new_from_slice(&[0x0bu8; 20]).unwrap();
        mac.update(b"Hi There");
        let hex: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    /// Fixed tag from `commit_argv` itself — locks the v1 domain, argv count,
    /// per-element length framing, and ordering (not merely the `hmac` crate).
    #[test]
    fn commit_argv_has_a_stable_v1_tag() {
        let key = CommitmentKey([0x2au8; 32]);
        let argv = ["run", "--flag", "value"].map(String::from).to_vec();
        let hex: String = commit_argv(&key, &argv)
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            "c0014367ebb8954c0eb5e84e1629345e8a79df6d5032e80944ebe2fa3d4c7dc8"
        );
    }

    #[test]
    fn commit_argv_frames_elements_unambiguously() {
        let key = CommitmentKey([7u8; 32]);
        let a = commit_argv(&key, &["ab".to_string(), "c".to_string()]);
        let b = commit_argv(&key, &["a".to_string(), "bc".to_string()]);
        assert_ne!(a, b, "length framing must distinguish [ab,c] from [a,bc]");
    }

    #[test]
    fn commit_argv_binds_the_key() {
        let argv = vec!["--token".to_string(), "s3cr3t".to_string()];
        let a = commit_argv(&CommitmentKey([1u8; 32]), &argv);
        let b = commit_argv(&CommitmentKey([2u8; 32]), &argv);
        assert_ne!(a, b, "different keys must produce different commitments");
    }

    #[test]
    fn commit_argv_preserves_order() {
        let key = CommitmentKey([9u8; 32]);
        let a = commit_argv(&key, &["x".to_string(), "y".to_string()]);
        let b = commit_argv(&key, &["y".to_string(), "x".to_string()]);
        assert_ne!(
            a, b,
            "argv order is semantic and must change the commitment"
        );
    }

    #[test]
    fn sensitive_types_have_redacted_debug() {
        assert_eq!(
            format!("{:?}", CommitmentKey([0xABu8; 32])),
            "CommitmentKey(<redacted>)"
        );
        let commit = commit_argv(&CommitmentKey([3u8; 32]), &["x".to_string()]);
        let dbg = format!("{commit:?}");
        assert_eq!(dbg, "ArgvCommitment(<redacted>)");
        assert!(!dbg.contains("ab"), "no tag bytes in Debug: {dbg}");
    }

    #[test]
    fn load_missing_key_blocks_and_creates_nothing() {
        with_home(|| {
            let err = load_commitment_key().unwrap_err().to_string();
            assert!(err.contains("missing"), "{err}");
            assert!(
                !commit_key_path().exists(),
                "read-only load must not create a key"
            );
        });
    }

    #[test]
    fn load_malformed_key_blocks() {
        with_home(|| {
            write_key(&[0u8; 31], 0o600);
            let err = load_commitment_key().unwrap_err().to_string();
            assert!(err.contains("expected exactly 32 bytes"), "{err}");
        });
    }

    #[cfg(unix)]
    #[test]
    fn load_wrong_permission_key_blocks() {
        with_home(|| {
            write_key(&[0u8; 32], 0o644);
            let err = load_commitment_key().unwrap_err().to_string();
            assert!(err.contains("insecure permissions"), "{err}");
        });
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_a_symlinked_key() {
        with_home(|| {
            use std::os::unix::fs::PermissionsExt;
            let target = commit_key_path().parent().unwrap().join("real-key");
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, [0u8; 32]).unwrap();
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::os::unix::fs::symlink(&target, commit_key_path()).unwrap();
            let err = load_commitment_key().unwrap_err().to_string();
            assert!(err.contains("symlink"), "{err}");
        });
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_insecure_parent_directory() {
        with_home(|| {
            write_key(&[0u8; 32], 0o600);
            use std::os::unix::fs::PermissionsExt;
            let dir = commit_key_path().parent().unwrap().to_path_buf();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
            let err = load_commitment_key().unwrap_err().to_string();
            assert!(err.contains("writable by group/other"), "{err}");
        });
    }

    #[test]
    fn load_valid_key_succeeds_and_is_usable() {
        with_home(|| {
            write_key(&[5u8; 32], 0o600);
            let key = load_commitment_key().expect("valid 0600 32-byte key loads");
            let _ = commit_argv(&key, &["x".to_string()]);
        });
    }

    #[test]
    fn provision_creates_valid_key_and_is_idempotent() {
        with_home(|| {
            assert!(!commit_key_path().exists());
            provision_commitment_key().expect("provision succeeds");
            let first = std::fs::read(commit_key_path()).unwrap();
            assert_eq!(first.len(), 32);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(commit_key_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o600, "provisioned key must be 0600");
            }
            provision_commitment_key().expect("second provision is a no-op");
            let second = std::fs::read(commit_key_path()).unwrap();
            assert_eq!(first, second, "provision must never replace a valid key");
            assert!(load_commitment_key().is_ok());
        });
    }

    #[test]
    fn provision_refuses_to_replace_an_invalid_key() {
        with_home(|| {
            write_key(&[0u8; 10], 0o600); // present but invalid length
            let err = provision_commitment_key().unwrap_err().to_string();
            assert!(err.contains("refusing to overwrite"), "{err}");
            assert_eq!(
                std::fs::read(commit_key_path()).unwrap().len(),
                10,
                "an invalid key is never auto-replaced"
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn provision_refuses_symlinked_grant_directory_without_touching_target() {
        with_home(|| {
            use std::os::unix::fs::PermissionsExt;
            let grant = commit_key_path().parent().unwrap().to_path_buf();
            let home = grant.parent().unwrap().to_path_buf();
            let target = home.join("decoy");
            std::fs::create_dir_all(&target).unwrap();
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
            // `grant/` is a symlink to the decoy directory.
            std::os::unix::fs::symlink(&target, &grant).unwrap();

            let err = provision_commitment_key().unwrap_err().to_string();
            assert!(err.contains("not a real directory"), "{err}");
            let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "symlinked target dir perms must be untouched");
        });
    }

    #[test]
    fn concurrent_provision_is_race_safe() {
        with_home(|| {
            let results: Vec<Result<()>> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..8).map(|_| s.spawn(provision_commitment_key)).collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            for r in &results {
                assert!(r.is_ok(), "every concurrent provision must succeed: {r:?}");
            }
            assert!(
                load_commitment_key().is_ok(),
                "exactly one valid key results"
            );
            let dir = commit_key_path().parent().unwrap().to_path_buf();
            let leftovers: Vec<_> = std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
                .collect();
            assert!(
                leftovers.is_empty(),
                "no temp files should remain: {:?}",
                leftovers.iter().map(|e| e.file_name()).collect::<Vec<_>>()
            );
        });
    }
}
