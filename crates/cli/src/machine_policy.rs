//! Fail-closed machine-policy loading with a last-known-good snapshot.
//!
//! Core parses the machine manifest and returns either exact policy bytes
//! identity or an error. This CLI-owned provider adds operational state: a
//! valid source refreshes a secret-free snapshot, a broken source reuses that
//! snapshot, and first-run corruption returns an error instead of silently
//! substituting an empty policy.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::manifest::Policy;

const SNAPSHOT_VERSION: u32 = 1;
const SNAPSHOT_FILE: &str = "machine-policy-lkg.json";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Unconfigured,
    Current {
        source_digest: String,
        snapshot_synced: bool,
        cache_error: Option<String>,
    },
    LastKnownGood {
        source_error: String,
        source_digest: String,
    },
    Blocked {
        source_error: String,
        snapshot_error: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Inspection {
    pub policy: Option<Policy>,
    pub status: Status,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    source_digest: String,
    policy: Policy,
}

/// Load policy for an enforcement path. Explicit absence is open; corruption
/// uses last-known-good or returns an error. Callers can therefore never obtain
/// `Policy::default()` as an accidental corruption fallback.
pub fn load() -> Result<Policy> {
    let inspected = inspect();
    match &inspected.status {
        Status::Unconfigured | Status::Current { .. } => {}
        Status::LastKnownGood {
            source_error,
            source_digest,
        } => eprintln!(
            "warning: machine policy is unreadable ({source_error}); using last-known-good policy from {source_digest}. Fix ~/.agentstack/agentstack.toml and run `agentstack doctor`"
        ),
        Status::Blocked {
            source_error,
            snapshot_error,
        } => anyhow::bail!(
            "machine policy is unreadable and no valid last-known-good policy is available; refusing to continue (source: {source_error}; snapshot: {snapshot_error}). Fix ~/.agentstack/agentstack.toml and run `agentstack doctor`"
        ),
    }
    if let Status::Current {
        cache_error: Some(error),
        ..
    } = &inspected.status
    {
        eprintln!(
            "warning: machine policy is valid but its last-known-good snapshot could not be refreshed ({error})"
        );
    }
    inspected
        .policy
        .ok_or_else(|| anyhow::anyhow!("machine policy unavailable"))
}

/// Inspect policy health without turning a blocked state into a command error.
/// Doctor and explain use this to remain diagnostic while enforcement paths
/// call [`load`].
pub fn inspect() -> Inspection {
    match crate::manifest::machine_policy_health() {
        None => Inspection {
            policy: Some(Policy::default()),
            status: Status::Unconfigured,
        },
        Some(Ok(source)) => {
            let snapshot = Snapshot {
                version: SNAPSHOT_VERSION,
                source_digest: source.source_digest.clone(),
                policy: source.policy.clone(),
            };
            let (snapshot_synced, cache_error) = match read_snapshot() {
                Ok(existing) if existing == snapshot => (true, None),
                _ => match write_snapshot(&snapshot) {
                    Ok(()) => (true, None),
                    Err(error) => (false, Some(format!("{error:#}"))),
                },
            };
            Inspection {
                policy: Some(source.policy),
                status: Status::Current {
                    source_digest: source.source_digest,
                    snapshot_synced,
                    cache_error,
                },
            }
        }
        Some(Err(error)) => {
            let source_error = format!("{error:#}");
            match read_snapshot() {
                Ok(snapshot) => Inspection {
                    policy: Some(snapshot.policy),
                    status: Status::LastKnownGood {
                        source_error,
                        source_digest: snapshot.source_digest,
                    },
                },
                Err(snapshot_error) => Inspection {
                    policy: None,
                    status: Status::Blocked {
                        source_error,
                        snapshot_error: format!("{snapshot_error:#}"),
                    },
                },
            }
        }
    }
}

fn snapshot_path() -> PathBuf {
    crate::util::paths::agentstack_home()
        .join("state")
        .join(SNAPSHOT_FILE)
}

fn read_snapshot() -> Result<Snapshot> {
    let path = snapshot_path();
    let text = crate::util::read_to_string_bounded(&path, crate::util::MAX_CONFIG_BYTES)
        .with_context(|| format!("reading {}", path.display()))?;
    let snapshot: Snapshot =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    crate::util::check_schema_version(
        snapshot.version,
        SNAPSHOT_VERSION,
        "machine-policy snapshot",
        &path,
    )?;
    if snapshot.source_digest.len() != 64
        || !snapshot
            .source_digest
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        anyhow::bail!("{} has an invalid source digest", path.display());
    }
    Ok(snapshot)
}

fn write_snapshot(snapshot: &Snapshot) -> Result<()> {
    let path = snapshot_path();
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    crate::util::restrict(parent, true);

    let mut bytes = serde_json::to_vec_pretty(snapshot)?;
    bytes.push(b'\n');
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(
        ".{SNAPSHOT_FILE}.{}.{}.tmp",
        std::process::id(),
        counter
    ));
    write_restricted(&temp, &bytes)?;
    if let Err(error) = fs::rename(&temp, &path) {
        let _ = fs::remove_file(&temp);
        return Err(error).with_context(|| format!("replacing {}", path.display()));
    }
    crate::util::restrict(&path, false);
    Ok(())
}

fn write_restricted(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("writing {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    fn with_home(test: impl FnOnce(&assert_fs::TempDir)) {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        test(&home);
        std::env::remove_var("AGENTSTACK_HOME");
    }

    fn write_manifest(home: &assert_fs::TempDir, text: &str) {
        home.child(crate::manifest::MANIFEST_FILE)
            .write_str(text)
            .unwrap();
    }

    #[test]
    fn absent_manifest_is_unconfigured_not_corrupt() {
        with_home(|_| {
            let inspected = inspect();
            assert_eq!(inspected.policy, Some(Policy::default()));
            assert_eq!(inspected.status, Status::Unconfigured);
            assert!(!snapshot_path().exists());
        });
    }

    #[test]
    fn valid_manifest_persists_snapshot_and_unchanged_digest_skips_refresh() {
        with_home(|home| {
            write_manifest(
                home,
                "version = 1\n[policy.tools]\n\"*\" = [\"!delete_*\"]\n",
            );
            let first = inspect();
            assert!(matches!(
                first.status,
                Status::Current {
                    snapshot_synced: true,
                    cache_error: None,
                    ..
                }
            ));
            let before = fs::read(snapshot_path()).unwrap();
            let writes_before = TEMP_COUNTER.load(Ordering::Relaxed);
            let second = inspect();
            assert!(matches!(
                second.status,
                Status::Current {
                    snapshot_synced: true,
                    cache_error: None,
                    ..
                }
            ));
            assert_eq!(fs::read(snapshot_path()).unwrap(), before);
            assert_eq!(TEMP_COUNTER.load(Ordering::Relaxed), writes_before);
        });
    }

    #[test]
    fn corrupt_manifest_uses_last_known_good_policy() {
        with_home(|home| {
            write_manifest(
                home,
                "version = 1\n[policy.tools]\n\"*\" = [\"!delete_*\"]\n",
            );
            load().unwrap();
            write_manifest(home, "not toml {{{");
            let inspected = inspect();
            assert!(matches!(inspected.status, Status::LastKnownGood { .. }));
            assert!(inspected
                .policy
                .unwrap()
                .tool_allowed("renamed", "delete_all")
                .is_err());
        });
    }

    #[test]
    fn first_run_corruption_is_blocked() {
        with_home(|home| {
            write_manifest(home, "not toml {{{");
            assert!(matches!(inspect().status, Status::Blocked { .. }));
            assert!(load()
                .unwrap_err()
                .to_string()
                .contains("refusing to continue"));
        });
    }

    #[test]
    fn corrupt_snapshot_is_blocked_when_source_is_corrupt() {
        with_home(|home| {
            write_manifest(home, "not toml {{{");
            let path = snapshot_path();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "not json").unwrap();
            let inspected = inspect();
            assert!(matches!(inspected.status, Status::Blocked { .. }));
        });
    }

    #[test]
    fn future_snapshot_version_is_blocked_when_source_is_corrupt() {
        with_home(|home| {
            write_manifest(home, "not toml {{{");
            let path = snapshot_path();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                path,
                r#"{"version":99,"source_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","policy":{}}"#,
            )
            .unwrap();
            let inspected = inspect();
            assert!(matches!(inspected.status, Status::Blocked { .. }));
        });
    }
}
