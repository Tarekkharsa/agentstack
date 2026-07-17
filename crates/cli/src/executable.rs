//! D3 local-executable pin derivation (locked-run contract §8).
//!
//! One classifier decides which of a stdio server's `command`/`args` strings
//! name repository-local executable files, so lock generation, strict locked
//! verification, and the trust/doctor gates all agree on the derived pin set —
//! never three slightly different heuristics.
//!
//! The rules, per the approved contract and rulings:
//! - repo-relative `command` and `args` entries that resolve to a regular file
//!   inside the project are pinned **automatically** (auto-detected — they are
//!   already declared in the manifest);
//! - `integrity_roots` are **declared** and always pinned; a declared root
//!   that cannot be digested is a hard error;
//! - a candidate that *exists* locally but is unverifiable — a symlink, a
//!   traversal component, a non-regular file — is a hard error, never silently
//!   left unpinned (that would be a bypass: declare a symlink, never re-gate);
//! - external `$PATH` binaries, absolute paths, `${REF}`-carrying values, and
//!   flag-like strings are not local content and are never pinned (§3.1:
//!   the harness/interpreter binary is explicitly unbound).

use std::fs;
use std::path::{Component, Path, PathBuf};

use agentstack_core::digest::{contained_file_digest, integrity_root_digest, resolve_contained};
use agentstack_core::lock::{ExecutableKind, LockedExecutable};
use agentstack_core::manifest::{Server, ServerType};
use anyhow::{Context, Result};

/// What one `command`/`args` string turned out to be.
///
/// (TS mental model: a discriminated union consumed with exhaustive `match`.)
#[derive(Debug, PartialEq, Eq)]
pub enum LocalExecutable {
    /// Not repository-local content: a `$PATH` binary, an absolute path, a
    /// flag, a `${REF}`, or a path with nothing on disk behind it.
    NotLocal,
    /// A contained regular file — pin it. Carries the normalized
    /// project-relative path (no `./` prefix), the lock key.
    File(String),
    /// Exists locally but can never be verified: symlink en route, traversal,
    /// or a non-regular file. Fail closed — an unpinnable local executable
    /// must block, not slip through unpinned.
    Rejected { path: String, reason: String },
}

/// Classify one candidate string against `anchor` (the directory the server
/// is spawned from — the project root, or its contained `cwd`).
pub fn classify_local_executable(
    project_dir: &Path,
    anchor: &Path,
    candidate: &str,
) -> LocalExecutable {
    if candidate.is_empty() || candidate.starts_with('-') || candidate.contains("${") {
        return LocalExecutable::NotLocal;
    }
    let rel = Path::new(candidate);
    if rel.is_absolute() {
        return LocalExecutable::NotLocal;
    }
    let mut resolved = anchor.to_path_buf();
    for component in rel.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => {
                resolved.push(part);
                match fs::symlink_metadata(&resolved) {
                    // Nothing on disk: an option value or free-form arg, not
                    // local content.
                    Err(_) => return LocalExecutable::NotLocal,
                    Ok(meta) if meta.file_type().is_symlink() => {
                        return LocalExecutable::Rejected {
                            path: candidate.to_string(),
                            reason: format!(
                                "passes through a symlink at {} — symlinks are never part of a pinned integrity surface",
                                resolved.display()
                            ),
                        }
                    }
                    Ok(_) => {}
                }
            }
            _ => return LocalExecutable::Rejected {
                path: candidate.to_string(),
                reason:
                    "contains a traversal component — only paths inside the project can be pinned"
                        .to_string(),
            },
        }
    }
    let Ok(meta) = fs::symlink_metadata(&resolved) else {
        return LocalExecutable::NotLocal;
    };
    if meta.is_file() {
        match project_relative(project_dir, &resolved) {
            Some(path) => LocalExecutable::File(path),
            // Unreachable while anchors are contained; fail closed anyway.
            None => LocalExecutable::Rejected {
                path: candidate.to_string(),
                reason: "resolves outside the project root".to_string(),
            },
        }
    } else if meta.is_dir() {
        // A directory isn't an executable file; declared roots are the way to
        // pin subtrees.
        LocalExecutable::NotLocal
    } else {
        LocalExecutable::Rejected {
            path: candidate.to_string(),
            reason: "is not a regular file — only regular files can be pinned".to_string(),
        }
    }
}

/// Derive the full pin set one server contributes to `agentstack.lock`:
/// auto-detected file pins for its stdio `command`/`args`, plus a root pin per
/// declared integrity root. An unverifiable local candidate or an undigestable
/// declared root is a hard error naming the server.
pub fn derive_executable_pins(
    project_dir: &Path,
    name: &str,
    server: &Server,
) -> Result<Vec<LockedExecutable>> {
    let mut pins = Vec::new();
    if server.server_type == ServerType::Stdio {
        if let Some(anchor) = server_anchor(project_dir, server)
            .with_context(|| format!("server '{name}': resolving its cwd"))?
        {
            for candidate in server.command.iter().chain(server.args.iter()) {
                match classify_local_executable(project_dir, &anchor, candidate) {
                    LocalExecutable::NotLocal => {}
                    LocalExecutable::File(path) => pins.push(LockedExecutable {
                        checksum: contained_file_digest(project_dir, &path)?,
                        path,
                        kind: ExecutableKind::File,
                    }),
                    LocalExecutable::Rejected { path, reason } => anyhow::bail!(
                        "server '{name}': local executable '{path}' cannot be pinned — {reason}"
                    ),
                }
            }
        }
    }
    for root in &server.integrity_roots {
        let checksum = integrity_root_digest(project_dir, root)
            .with_context(|| format!("server '{name}': pinning integrity root '{root}'"))?;
        pins.push(LockedExecutable {
            path: normalize_declared(root),
            kind: ExecutableKind::Root,
            checksum,
        });
    }
    Ok(pins)
}

/// The directory a stdio server's relative `command`/`args` resolve against:
/// the project root, or a contained relative `cwd`.
///
/// `Ok(None)` — an absolute or `${REF}` cwd: not repository content, not
/// statically classifiable; candidates stay unpinned and the trust preview
/// labels that surface honestly rather than guessing.
///
/// `Err` — a **relative** cwd that fails containment (symlink, traversal,
/// missing, or not a directory). A relative cwd IS repository content: a
/// symlinked cwd silently mapped to `None` would disable pinning for the whole
/// server — the exact declare-a-symlink bypass the reject-not-skip rule
/// exists to prevent, relocated one field over.
fn server_anchor(project_dir: &Path, server: &Server) -> Result<Option<PathBuf>> {
    match &server.cwd {
        None => Ok(Some(project_dir.to_path_buf())),
        Some(cwd) if cwd.contains("${") || Path::new(cwd).is_absolute() => Ok(None),
        Some(cwd) => {
            let anchor = resolve_contained(project_dir, cwd)
                .with_context(|| format!("resolving cwd '{cwd}'"))?;
            if !anchor.is_dir() {
                anyhow::bail!("cwd '{cwd}' is not a directory");
            }
            Ok(Some(anchor))
        }
    }
}

/// How one server's currently-derived executable surface compares to its
/// `agentstack.lock` pins. Mirrors the other `*LockStatus` families in
/// `crate::resolve` so the verify gates treat all input kinds identically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutableLockStatus {
    /// The current content digest matches the locked pin.
    Matches,
    /// The input is derivable but has no lock entry yet.
    MissingLockEntry,
    /// The current content digest differs from the locked pin.
    ChecksumDrift { locked: String, current: String },
    /// The surface could not be derived at all — a symlink, traversal,
    /// non-regular file, or broken declared root. Never proceed.
    ResolveFailed { error: String },
}

/// Compare every server's derived executable surface to the lock, one labeled
/// status per input (`executable 'path' (server 'name')` / `integrity root
/// 'path' (server 'name')`). Verification re-derives through the SAME
/// classifier that produced the pins, so the two can never disagree on what
/// should be pinned; a server whose surface fails to derive yields one
/// `ResolveFailed` entry naming it.
pub fn executable_lock_statuses(
    project_dir: &Path,
    servers: &[(String, Server)],
    lock: &agentstack_core::lock::Lock,
) -> Vec<(String, ExecutableLockStatus)> {
    executable_lock_statuses_and_pins(project_dir, servers, lock).0
}

/// [`executable_lock_statuses`] plus the derived pins themselves, keyed by
/// owning server. The locked run freezes EXACTLY these content identities into
/// the grant — the same derivation the verify gate judged, never a second one
/// between check and freeze.
#[allow(clippy::type_complexity)]
pub fn executable_lock_statuses_and_pins(
    project_dir: &Path,
    servers: &[(String, Server)],
    lock: &agentstack_core::lock::Lock,
) -> (
    Vec<(String, ExecutableLockStatus)>,
    Vec<(String, LockedExecutable)>,
) {
    let mut statuses = Vec::new();
    let mut derived = Vec::new();
    for (name, server) in servers {
        match derive_executable_pins(project_dir, name, server) {
            Err(e) => statuses.push((
                format!("server '{name}' local executables"),
                ExecutableLockStatus::ResolveFailed {
                    error: format!("{e:#}"),
                },
            )),
            Ok(pins) => {
                for pin in pins {
                    let label = match pin.kind {
                        ExecutableKind::File => {
                            format!("executable '{}' (server '{name}')", pin.path)
                        }
                        ExecutableKind::Root => {
                            format!("integrity root '{}' (server '{name}')", pin.path)
                        }
                    };
                    let status = match lock.get_executable(&pin.path, pin.kind) {
                        None => ExecutableLockStatus::MissingLockEntry,
                        Some(entry) if entry.checksum != pin.checksum => {
                            ExecutableLockStatus::ChecksumDrift {
                                locked: entry.checksum.hex().to_string(),
                                current: pin.checksum.hex().to_string(),
                            }
                        }
                        Some(_) => ExecutableLockStatus::Matches,
                    };
                    statuses.push((label, status));
                    derived.push((name.clone(), pin));
                }
            }
        }
    }
    (statuses, derived)
}

/// Normalize a declared path to its lock key: `Normal` components joined with
/// `/` (drops any `./` prefix so `"./tools"` and `"tools"` share one entry).
/// The ONE normalizer for D3 lock keys — lock generation, verification, and
/// the grant's server-tie validation all key through it.
pub(crate) fn normalize_declared(declared: &str) -> String {
    Path::new(declared)
        .components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// `resolved` as a normalized project-relative string, or `None` if it
/// escaped the project (defensive; anchors are contained by construction).
fn project_relative(project_dir: &Path, resolved: &Path) -> Option<String> {
    let rel = resolved.strip_prefix(project_dir).ok()?;
    Some(normalize_declared(&rel.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    fn stdio(toml_src: &str) -> Server {
        toml::from_str(toml_src).unwrap()
    }

    #[test]
    fn classify_skips_external_and_flag_like_candidates() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("scripts/run.sh").write_str("echo").unwrap();

        for candidate in [
            "node",
            "python3",
            "--flag",
            "-v",
            "/usr/bin/env",
            "${HOME}/bin/tool",
            "scripts/missing.sh",
            "",
            "scripts", // a directory, not an executable file
        ] {
            assert_eq!(
                classify_local_executable(tmp.path(), tmp.path(), candidate),
                LocalExecutable::NotLocal,
                "{candidate:?}"
            );
        }
    }

    #[test]
    fn classify_pins_contained_files_and_normalizes_the_key() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("scripts/run.sh").write_str("echo").unwrap();

        for candidate in ["scripts/run.sh", "./scripts/run.sh"] {
            assert_eq!(
                classify_local_executable(tmp.path(), tmp.path(), candidate),
                LocalExecutable::File("scripts/run.sh".to_string()),
                "{candidate:?}"
            );
        }
    }

    #[test]
    fn classify_rejects_unverifiable_local_candidates() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("real.sh").write_str("echo").unwrap();

        assert!(matches!(
            classify_local_executable(tmp.path(), tmp.path(), "../outside.sh"),
            LocalExecutable::Rejected { .. }
        ));

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(tmp.child("real.sh").path(), tmp.child("link.sh").path())
                .unwrap();
            let verdict = classify_local_executable(tmp.path(), tmp.path(), "link.sh");
            let LocalExecutable::Rejected { reason, .. } = verdict else {
                panic!("symlink must be rejected, got {verdict:?}");
            };
            assert!(reason.contains("symlink"), "{reason}");
        }
    }

    #[test]
    fn classify_anchors_at_the_server_cwd() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("tools/run.sh").write_str("echo").unwrap();

        // Relative to the project root "run.sh" is nothing; anchored at the
        // server's cwd it is the payload, keyed by its project-relative path.
        assert_eq!(
            classify_local_executable(tmp.path(), tmp.path(), "run.sh"),
            LocalExecutable::NotLocal
        );
        assert_eq!(
            classify_local_executable(tmp.path(), &tmp.path().join("tools"), "run.sh"),
            LocalExecutable::File("tools/run.sh".to_string())
        );
    }

    #[test]
    fn derive_pins_command_args_and_declared_roots() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("scripts/entry.py").write_str("import x").unwrap();
        tmp.child("tools/lib.py").write_str("v1").unwrap();

        let server = stdio(
            r#"
            type = "stdio"
            command = "python"
            args = ["./scripts/entry.py", "--verbose"]
            integrity_roots = ["./tools"]
            "#,
        );
        let pins = derive_executable_pins(tmp.path(), "agent", &server).unwrap();
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[0].path, "scripts/entry.py");
        assert_eq!(pins[0].kind, ExecutableKind::File);
        assert_eq!(pins[1].path, "tools", "declared root key is normalized");
        assert_eq!(pins[1].kind, ExecutableKind::Root);

        // The pinned digests match the core routines (same inputs, same pins).
        assert_eq!(
            pins[0].checksum,
            contained_file_digest(tmp.path(), "scripts/entry.py").unwrap()
        );
        assert_eq!(
            pins[1].checksum,
            integrity_root_digest(tmp.path(), "tools").unwrap()
        );
    }

    #[test]
    fn derive_pins_cwd_anchored_payloads_under_their_project_relative_key() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("tools/run.sh").write_str("echo").unwrap();

        let server = stdio(
            r#"
            type = "stdio"
            command = "./run.sh"
            cwd = "tools"
            "#,
        );
        let pins = derive_executable_pins(tmp.path(), "agent", &server).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].path, "tools/run.sh");
    }

    #[cfg(unix)]
    #[test]
    fn derive_fails_on_unverifiable_candidates_and_broken_roots() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("real.sh").write_str("echo").unwrap();
        std::os::unix::fs::symlink(tmp.child("real.sh").path(), tmp.child("link.sh").path())
            .unwrap();

        // A symlink in the command slot is a hard error, not silently unpinned.
        let server = stdio("type = \"stdio\"\ncommand = \"./link.sh\"\n");
        let err = derive_executable_pins(tmp.path(), "agent", &server).unwrap_err();
        assert!(err.to_string().contains("server 'agent'"), "{err}");
        assert!(err.to_string().contains("symlink"), "{err}");

        // A declared root that can't be digested (missing) is a hard error.
        let server = stdio("type = \"stdio\"\ncommand = \"node\"\nintegrity_roots = [\"gone\"]\n");
        let err = derive_executable_pins(tmp.path(), "agent", &server).unwrap_err();
        assert!(err.to_string().contains("integrity root 'gone'"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn derive_fails_on_symlinked_cwd_but_skips_unclassifiable_anchors() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("scripts/run.sh").write_str("echo").unwrap();
        std::os::unix::fs::symlink(tmp.child("scripts").path(), tmp.child("alias").path()).unwrap();

        // A relative cwd is repository content: a symlink there is a hard
        // error, never a silent skip that disables pinning for the server.
        let server = stdio("type = \"stdio\"\ncommand = \"./run.sh\"\ncwd = \"alias\"\n");
        let err = derive_executable_pins(tmp.path(), "agent", &server).unwrap_err();
        assert!(format!("{err:#}").contains("cwd"), "{err:#}");

        // Absolute / ${REF} cwds are not repository content — candidates are
        // honestly unclassifiable, not errors (labeled by the trust preview).
        for cwd in ["/opt/tools", "${TOOLS_HOME}"] {
            let server = stdio(&format!(
                "type = \"stdio\"\ncommand = \"./run.sh\"\ncwd = \"{cwd}\"\n"
            ));
            assert!(derive_executable_pins(tmp.path(), "agent", &server)
                .unwrap()
                .is_empty());
        }
    }

    #[cfg(unix)]
    #[test]
    fn lock_statuses_cover_match_drift_missing_and_underivable() {
        use agentstack_core::lock::Lock;

        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("scripts/entry.py").write_str("v1").unwrap();
        let pinned =
            stdio("type = \"stdio\"\ncommand = \"python\"\nargs = [\"scripts/entry.py\"]\n");

        // Pin, then verify: Matches.
        let mut lock = Lock::default();
        for pin in derive_executable_pins(tmp.path(), "a", &pinned).unwrap() {
            lock.upsert_executable(pin);
        }
        let servers = vec![("a".to_string(), pinned)];
        let statuses = executable_lock_statuses(tmp.path(), &servers, &lock);
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].1, ExecutableLockStatus::Matches);

        // One-byte edit → ChecksumDrift (the D3 re-gate witness at the
        // verification layer).
        tmp.child("scripts/entry.py").write_str("v2").unwrap();
        let statuses = executable_lock_statuses(tmp.path(), &servers, &lock);
        assert!(
            matches!(statuses[0].1, ExecutableLockStatus::ChecksumDrift { .. }),
            "{statuses:?}"
        );
        assert!(statuses[0].0.contains("executable 'scripts/entry.py'"));

        // No lock entry → MissingLockEntry.
        let statuses = executable_lock_statuses(tmp.path(), &servers, &Lock::default());
        assert_eq!(statuses[0].1, ExecutableLockStatus::MissingLockEntry);

        // An underivable surface (symlinked command) → one ResolveFailed
        // entry naming the server.
        std::os::unix::fs::symlink(
            tmp.child("scripts/entry.py").path(),
            tmp.child("link.py").path(),
        )
        .unwrap();
        let hostile = vec![(
            "h".to_string(),
            stdio("type = \"stdio\"\ncommand = \"./link.py\"\n"),
        )];
        let statuses = executable_lock_statuses(tmp.path(), &hostile, &Lock::default());
        assert_eq!(statuses.len(), 1);
        assert!(
            matches!(&statuses[0].1, ExecutableLockStatus::ResolveFailed { error } if error.contains("symlink")),
            "{statuses:?}"
        );
    }

    #[test]
    fn derive_ignores_http_servers_without_roots() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let server = stdio("type = \"http\"\nurl = \"https://x/mcp\"\n");
        assert!(derive_executable_pins(tmp.path(), "api", &server)
            .unwrap()
            .is_empty());
    }
}
