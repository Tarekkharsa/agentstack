//! Content digests — the bytes-to-identity layer everything above builds on.
//!
//! Extracted verbatim from the cli crate (store/resolve): `sha256_hex` gives a
//! server definition its lockfile checksum; `dir_digest` gives a skill
//! directory its lockfile checksum. These digests are what `agentstack.lock`
//! pins and what use-time verification compares against, so their semantics
//! are security-relevant: any byte, path, or file-set change must change the
//! digest (property-tested below).
//!
//! There is no stat-fingerprint cache: `dir_digest` reads current bytes on
//! every call, and verification never consults a memoized digest (see
//! `docs/ARCHITECTURE.md`). Core stays free of `~/.agentstack` knowledge.

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

const DIR_DIGEST_DOMAIN: &[u8] = b"agentstack-dir-digest-v2\0";
/// Domain separator for D3 integrity-root digests. Distinct from
/// [`DIR_DIGEST_DOMAIN`] so a skill-directory digest can never stand in for an
/// integrity-root digest over the same tree (the two routines make different
/// promises about symlinks and `.git`).
const INTEGRITY_ROOT_DOMAIN: &[u8] = b"agentstack-integrity-root-v1\0";
const MAX_DIRECTORY_DEPTH: usize = 64;

/// A validated SHA-256 digest: exactly 64 lowercase hex chars, stored bare
/// (no `sha256:` prefix). Extracted from `cli::grant`, where it already guarded
/// the grant digests, so `trust`, `lock`, and `adapters` share ONE digest type
/// instead of each passing an unvalidated `String` around.
///
/// Wire form is the bare hex string, byte-identical to the `String` fields this
/// replaces — `agentstack.lock` bytes (and so the trust digest over them) do not
/// change. Deserialization goes through [`Sha256Hex::parse`], so a malformed
/// digest in a lockfile fails loudly at parse instead of silently mismatching
/// later (rule 7: bundle content is hostile input).
///
/// (TS mental model: a branded/opaque type — you can't hand a random string to
/// something expecting a digest without going through the validating parser.)
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Hex(String);

impl Sha256Hex {
    /// Validate a digest string. Accepts an optional `sha256:` prefix (the
    /// spelling `trust` stores) and normalizes to bare lowercase hex.
    pub fn parse(s: &str) -> Result<Self> {
        let h = s.strip_prefix("sha256:").unwrap_or(s);
        if h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()) {
            Ok(Sha256Hex(h.to_ascii_lowercase()))
        } else {
            anyhow::bail!("not a sha256 hex digest: {s:?}");
        }
    }

    /// The bare lowercase hex (no prefix).
    pub fn hex(&self) -> &str {
        &self.0
    }

    /// The digest OF some bytes, typed — the one-shot companion to
    /// [`sha256_hex`] for callers that want the validated type rather than a
    /// bare `String`. Cannot fail: a sha2 finalize is always 64 lowercase hex.
    pub fn of(bytes: &[u8]) -> Sha256Hex {
        Sha256Hex(sha256_hex(bytes))
    }
}

/// Renders bare hex — the stored form. Callers that need the prefixed
/// `sha256:<hex>` spelling add it themselves (see `trust`'s digest field and
/// `cli::grant`'s wrapper types).
impl std::fmt::Display for Sha256Hex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A digest is public, non-secret data, so `Debug` shows it — but via `Display`
/// so the two never drift.
impl std::fmt::Debug for Sha256Hex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

impl serde::Serialize for Sha256Hex {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        // Bare hex: byte-identical to the String field this replaces.
        s.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for Sha256Hex {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        // Validate on the way in — a lockfile is hostile input.
        let s = String::deserialize(d)?;
        Sha256Hex::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// SHA-256 hex digest of a byte string.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// SHA-256 digest of a directory's contents (relative paths + file bytes,
/// sorted; `.git` excluded).
pub fn dir_digest(root: &Path) -> Result<Sha256Hex> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    hasher.update(DIR_DIGEST_DOMAIN);
    for rel in &files {
        let path_bytes = normalized_relative_path_bytes(rel);
        hasher.update((path_bytes.len() as u64).to_le_bytes());
        hasher.update(&path_bytes);
        let bytes = fs::read(root.join(rel))
            .with_context(|| format!("reading {}", root.join(rel).display()))?;
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    // Same-module tuple construction (like `Sha256Hex::of`), not `parse`: a
    // sha2 finalize is always exactly 64 lowercase hex chars, so there is no
    // fallible case to `unwrap`/`expect` around (both are workspace-denied
    // outside tests).
    Ok(Sha256Hex(format!("{:x}", hasher.finalize())))
}

/// Collect every file under `dir` as a path relative to `root`, recursing into
/// subdirectories; `.git` is excluded.
pub fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    collect_files_at_depth(root, dir, out, 0)
}

fn collect_files_at_depth(
    root: &Path,
    dir: &Path,
    out: &mut Vec<PathBuf>,
    depth: usize,
) -> Result<()> {
    if depth > MAX_DIRECTORY_DEPTH {
        anyhow::bail!(
            "directory nesting exceeds the maximum depth of {MAX_DIRECTORY_DEPTH} under {}",
            root.display()
        );
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type()?;
        // SAFETY of trust: links may escape the hostile bundle and can target
        // unbounded devices, so they are never part of a directory digest.
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_files_at_depth(root, &path, out, depth + 1)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
}

/// Resolve a manifest-declared repository-relative path to a real location
/// inside `project_root`, defensively (contract §8 canonical-path rule):
///
/// - absolute paths, `..` traversal, and Windows drive prefixes are hard errors;
/// - a symlink **anywhere** on the path — intermediate directory or final
///   component — is a hard error (D3 ruling: reject, never resolve), because a
///   link target can change without changing any pinned byte;
/// - the path must exist (can't pin what can't be read);
/// - the project root itself is rejected: a root-wide pin would include
///   `agentstack.lock`, so every re-lock would immediately re-drift it.
///
/// A leading `./` is accepted — manifests conventionally write
/// `command = "./scripts/foo.sh"`.
pub fn resolve_contained(project_root: &Path, declared: &str) -> Result<PathBuf> {
    if declared.is_empty() {
        anyhow::bail!("integrity path is empty");
    }
    let rel = Path::new(declared);
    if rel.is_absolute() {
        anyhow::bail!(
            "integrity path '{declared}' is absolute — only repository-relative paths can be pinned"
        );
    }
    let mut resolved = project_root.to_path_buf();
    for component in rel.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => {
                resolved.push(part);
                let meta = fs::symlink_metadata(&resolved)
                    .with_context(|| format!("reading {}", resolved.display()))?;
                if meta.file_type().is_symlink() {
                    anyhow::bail!(
                        "integrity path '{declared}' passes through a symlink at {} — \
                         symlinks are never part of a pinned integrity surface",
                        resolved.display()
                    );
                }
            }
            _ => anyhow::bail!(
                "integrity path '{declared}' contains a traversal or non-relative component — \
                 only paths inside the project can be pinned"
            ),
        }
    }
    if resolved == project_root {
        anyhow::bail!(
            "integrity path '{declared}' resolves to the project root itself — \
             declare a subdirectory or file"
        );
    }
    Ok(resolved)
}

/// SHA-256 of one repository-relative file's current bytes — the D3 pin for a
/// repo-relative stdio `command` or interpreter-script `args` entry. Same
/// digest shape as instruction pins (raw file bytes), same defensive path
/// rules as [`resolve_contained`]; a directory or other non-regular file is an
/// error.
pub fn contained_file_digest(project_root: &Path, declared: &str) -> Result<Sha256Hex> {
    let path = resolve_contained(project_root, declared)?;
    let meta =
        fs::symlink_metadata(&path).with_context(|| format!("reading {}", path.display()))?;
    if !meta.is_file() {
        anyhow::bail!(
            "integrity path '{declared}' is not a regular file — \
             a command/args pin must name one executable file"
        );
    }
    let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    Ok(Sha256Hex::of(&bytes))
}

/// SHA-256 digest of a declared D3 integrity root (contract §8): a file or
/// directory subtree whose **every byte** is pinned, cache-free.
///
/// This is deliberately NOT [`dir_digest`]:
/// - a symlink anywhere inside the root is a **hard error**, never skipped —
///   an interpreter would happily follow a link the skip-symlinks digest never
///   covered (contract §8, round-3 correction 2; D3 ruling: reject all);
/// - `.git` is NOT excluded — a payload hidden under a nested `.git/` inside a
///   declared root would otherwise be present-but-unpinned, breaking the
///   "one-byte change anywhere in a declared root re-gates" guarantee;
/// - its own domain separator, so the two digest families never collide.
///
/// Layout: domain, then for each file sorted by relative path, the
/// length-framed normalized path bytes and length-framed content bytes. A root
/// that is a single file frames the empty relative path (a directory entry can
/// never have an empty name, so file roots and directory roots cannot
/// collide).
/// The resolved root and its sorted, symlink-rejecting file list — the SAME
/// strict walk [`integrity_root_digest`] pins, exposed so a copy-render can
/// deliver exactly the pinned byte set. Returns the resolved absolute root plus
/// each contained file's path relative to it; a single-file root yields one
/// empty relative path (read `root` directly). Rejecting symlinks here — not
/// following them — means a link that appeared after the digest check can never
/// smuggle foreign bytes into a rendered extension.
pub fn integrity_root_files(
    project_root: &Path,
    declared: &str,
) -> Result<(PathBuf, Vec<PathBuf>)> {
    let root = resolve_contained(project_root, declared)?;
    let meta =
        fs::symlink_metadata(&root).with_context(|| format!("reading {}", root.display()))?;

    let mut files: Vec<PathBuf> = Vec::new();
    if meta.is_dir() {
        collect_files_rejecting_symlinks(&root, &root, &mut files, 0)?;
    } else if meta.is_file() {
        files.push(PathBuf::new());
    } else {
        anyhow::bail!("integrity root '{declared}' is neither a regular file nor a directory");
    }
    files.sort();
    Ok((root, files))
}

pub fn integrity_root_digest(project_root: &Path, declared: &str) -> Result<Sha256Hex> {
    let (root, files) = integrity_root_files(project_root, declared)?;

    let mut hasher = Sha256::new();
    hasher.update(INTEGRITY_ROOT_DOMAIN);
    for rel in &files {
        let path_bytes = normalized_relative_path_bytes(rel);
        hasher.update((path_bytes.len() as u64).to_le_bytes());
        hasher.update(&path_bytes);
        // A file root frames the empty relative path; joining "" would append
        // a trailing separator, so the root path itself is read directly.
        let path = if rel.as_os_str().is_empty() {
            root.clone()
        } else {
            root.join(rel)
        };
        let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    // Same-module tuple construction, not `parse`: see `dir_digest`.
    Ok(Sha256Hex(format!("{:x}", hasher.finalize())))
}

/// [`collect_files_at_depth`]'s strict sibling for integrity roots: same
/// depth-bounded recursive walk, but a symlink is a hard error instead of a
/// skip, and `.git` is included (see [`integrity_root_digest`]).
fn collect_files_rejecting_symlinks(
    root: &Path,
    dir: &Path,
    out: &mut Vec<PathBuf>,
    depth: usize,
) -> Result<()> {
    if depth > MAX_DIRECTORY_DEPTH {
        anyhow::bail!(
            "directory nesting exceeds the maximum depth of {MAX_DIRECTORY_DEPTH} under {}",
            root.display()
        );
    }
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            anyhow::bail!(
                "integrity root {} contains a symlink at {} — \
                 symlinks are never part of a pinned integrity surface",
                root.display(),
                path.display()
            );
        }
        if file_type.is_dir() {
            collect_files_rejecting_symlinks(root, &path, out, depth + 1)?;
        } else if file_type.is_file() {
            // A file outside `root` is impossible here (every path descends
            // from it), but a silent drop would mean present-but-unpinned
            // bytes, so the impossible case still fails closed.
            let rel = path.strip_prefix(root).map_err(|_| {
                anyhow::anyhow!("integrity walk escaped its root at {}", path.display())
            })?;
            out.push(rel.to_path_buf());
        } else {
            // FIFOs, sockets, and device nodes: reading one can block forever
            // (a FIFO with no writer), turning the trust gate into a hang —
            // and none of them are pinnable content anyway.
            anyhow::bail!(
                "integrity root {} contains a non-regular file at {} — \
                 only regular files and directories can be pinned",
                root.display(),
                path.display()
            );
        }
    }
    Ok(())
}

fn normalized_relative_path_bytes(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (index, component) in path.components().enumerate() {
        if index != 0 {
            bytes.push(b'/');
        }
        append_os_str_bytes(&mut bytes, component.as_os_str());
    }
    bytes
}

#[cfg(unix)]
fn append_os_str_bytes(out: &mut Vec<u8>, value: &std::ffi::OsStr) {
    out.extend_from_slice(value.as_bytes());
}

#[cfg(not(unix))]
fn append_os_str_bytes(out: &mut Vec<u8>, value: &std::ffi::OsStr) {
    out.extend_from_slice(value.to_string_lossy().as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    #[test]
    fn sha256_hex_known_vector() {
        // NIST test vector: sha256("abc").
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256hex_parse_normalizes_case_to_one_value() {
        // Hex case is spelling, not value: normalization maps both spellings
        // to the SAME 256-bit digest, so it can never widen the accepted
        // preimage set. Trust is separately unaffected — `digest_for` hashes
        // raw lock BYTES, so a respelled lockfile still re-gates (witnessed by
        // trust's any_single_byte_flip_in_any_pinned_file_regates).
        let lower = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let upper = lower.to_ascii_uppercase();
        let a = Sha256Hex::parse(lower).unwrap();
        let b = Sha256Hex::parse(&upper).unwrap();
        assert_eq!(a, b);
        assert_eq!(b.hex(), lower, "stored form is canonical lowercase");
        let prefixed = Sha256Hex::parse(&format!("sha256:{upper}")).unwrap();
        assert_eq!(prefixed.hex(), lower);
    }

    #[test]
    fn dir_digest_stable_and_sensitive() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("a.txt").write_str("hello").unwrap();
        tmp.child("sub/b.txt").write_str("world").unwrap();
        let d1 = dir_digest(tmp.path()).unwrap();
        let d2 = dir_digest(tmp.path()).unwrap();
        assert_eq!(d1, d2);
        assert_eq!(d1.hex().len(), 64);
        tmp.child("a.txt").write_str("changed").unwrap();
        assert_ne!(d1, dir_digest(tmp.path()).unwrap());
    }

    #[test]
    fn dir_digest_excludes_git_dir() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("a.txt").write_str("hello").unwrap();
        let d1 = dir_digest(tmp.path()).unwrap();
        tmp.child(".git/HEAD").write_str("ref: main\n").unwrap();
        assert_eq!(d1, dir_digest(tmp.path()).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn dir_digest_skips_symlinks_without_following_them() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let bundle = tmp.child("bundle");
        bundle.child("file.txt").write_str("inside").unwrap();
        let outside = tmp.child("outside.txt");
        outside.write_str("foreign bytes").unwrap();

        let without_link = dir_digest(bundle.path()).unwrap();
        std::os::unix::fs::symlink(outside.path(), bundle.child("link").path()).unwrap();

        assert_eq!(without_link, dir_digest(bundle.path()).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn dir_digest_skips_broken_symlinks() {
        let tmp = assert_fs::TempDir::new().unwrap();
        std::os::unix::fs::symlink(tmp.child("missing").path(), tmp.child("broken").path())
            .unwrap();

        assert!(dir_digest(tmp.path()).is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dir_digest_distinguishes_non_utf8_paths() {
        use std::ffi::OsStr;

        let first = assert_fs::TempDir::new().unwrap();
        let second = assert_fs::TempDir::new().unwrap();
        fs::write(first.path().join(OsStr::from_bytes(b"name-\x80")), b"same").unwrap();
        fs::write(second.path().join(OsStr::from_bytes(b"name-\x81")), b"same").unwrap();

        assert_ne!(
            dir_digest(first.path()).unwrap(),
            dir_digest(second.path()).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn normalized_paths_preserve_distinct_non_utf8_bytes() {
        use std::ffi::OsStr;

        let first = Path::new(OsStr::from_bytes(b"name-\x80"));
        let second = Path::new(OsStr::from_bytes(b"name-\x81"));

        assert_ne!(
            normalized_relative_path_bytes(first),
            normalized_relative_path_bytes(second)
        );
    }

    #[test]
    fn dir_digest_rejects_excessive_directory_depth() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let mut dir = tmp.path().to_path_buf();
        for _ in 0..=MAX_DIRECTORY_DEPTH {
            dir.push("nested");
            fs::create_dir(&dir).unwrap();
        }
        fs::write(dir.join("file.txt"), b"deep").unwrap();

        assert!(dir_digest(tmp.path()).is_err());
    }

    mod integrity_roots {
        use super::*;

        #[test]
        fn resolve_contained_rejects_hostile_paths() {
            let tmp = assert_fs::TempDir::new().unwrap();
            tmp.child("tools/agent.py").write_str("print()").unwrap();

            // Accepted: plain relative and ./-prefixed forms of a real path.
            assert!(resolve_contained(tmp.path(), "tools/agent.py").is_ok());
            assert!(resolve_contained(tmp.path(), "./tools/agent.py").is_ok());

            for (declared, why) in [
                ("", "empty"),
                ("/etc/passwd", "absolute"),
                ("../outside.sh", "traversal"),
                ("tools/../../outside.sh", "traversal"),
                (".", "project root itself"),
                ("./", "project root itself"),
                ("tools/missing.py", "missing"),
            ] {
                assert!(
                    resolve_contained(tmp.path(), declared).is_err(),
                    "{declared:?} must be rejected ({why})"
                );
            }
        }

        #[cfg(unix)]
        #[test]
        fn resolve_contained_rejects_symlinks_at_every_position() {
            let tmp = assert_fs::TempDir::new().unwrap();
            tmp.child("real/agent.py").write_str("print()").unwrap();
            tmp.child("outside.sh").write_str("echo").unwrap();

            // Final component is a symlink — even to a contained target.
            std::os::unix::fs::symlink(
                tmp.child("real/agent.py").path(),
                tmp.child("link.py").path(),
            )
            .unwrap();
            let err = resolve_contained(tmp.path(), "link.py").unwrap_err();
            assert!(err.to_string().contains("symlink"), "{err}");

            // Intermediate directory is a symlink.
            std::os::unix::fs::symlink(tmp.child("real").path(), tmp.child("alias").path())
                .unwrap();
            assert!(resolve_contained(tmp.path(), "alias/agent.py").is_err());

            // Symlink escaping the project root.
            std::os::unix::fs::symlink(
                tmp.child("outside.sh").path(),
                tmp.child("real/esc").path(),
            )
            .unwrap();
            assert!(resolve_contained(tmp.path(), "real/esc").is_err());
        }

        #[test]
        fn contained_file_digest_pins_current_bytes() {
            let tmp = assert_fs::TempDir::new().unwrap();
            tmp.child("scripts/run.sh").write_str("echo one").unwrap();

            let d1 = contained_file_digest(tmp.path(), "scripts/run.sh").unwrap();
            assert_eq!(
                d1.hex(),
                sha256_hex(b"echo one"),
                "raw file bytes, like instruction pins"
            );

            // The one-byte re-gate witness for a pinned entry file.
            tmp.child("scripts/run.sh").write_str("echo two").unwrap();
            assert_ne!(
                d1,
                contained_file_digest(tmp.path(), "scripts/run.sh").unwrap()
            );

            // A directory is not a file pin.
            assert!(contained_file_digest(tmp.path(), "scripts").is_err());
        }

        #[test]
        fn integrity_root_digest_stable_and_sensitive_anywhere_in_the_root() {
            let tmp = assert_fs::TempDir::new().unwrap();
            tmp.child("tools/agent.py")
                .write_str("import payload")
                .unwrap();
            tmp.child("tools/deep/payload.py").write_str("v1").unwrap();

            let d1 = integrity_root_digest(tmp.path(), "tools").unwrap();
            assert_eq!(d1, integrity_root_digest(tmp.path(), "tools").unwrap());
            assert_eq!(d1, integrity_root_digest(tmp.path(), "./tools").unwrap());
            assert_eq!(d1.hex().len(), 64);

            // One byte in a transitive import — not the entry file — re-gates.
            tmp.child("tools/deep/payload.py").write_str("v2").unwrap();
            let d2 = integrity_root_digest(tmp.path(), "tools").unwrap();
            assert_ne!(d1, d2);

            // Adding a new file anywhere re-gates too.
            tmp.child("tools/new.py").write_str("x").unwrap();
            assert_ne!(d2, integrity_root_digest(tmp.path(), "tools").unwrap());
        }

        #[test]
        fn integrity_root_digest_supports_single_file_roots() {
            let tmp = assert_fs::TempDir::new().unwrap();
            tmp.child("run.sh").write_str("echo").unwrap();
            tmp.child("dir/run.sh").write_str("echo").unwrap();

            let file_root = integrity_root_digest(tmp.path(), "run.sh").unwrap();
            let dir_root = integrity_root_digest(tmp.path(), "dir").unwrap();
            // Same bytes, but a file root and a one-file directory root must
            // not collide (empty vs named framed path).
            assert_ne!(file_root, dir_root);
        }

        #[test]
        fn integrity_root_digest_diverges_from_dir_digest_on_git_dirs() {
            let tmp = assert_fs::TempDir::new().unwrap();
            tmp.child("tools/agent.py").write_str("x").unwrap();

            let before = integrity_root_digest(tmp.path(), "tools").unwrap();
            // dir_digest ignores .git; the integrity root must NOT — bytes
            // under a nested .git are still reachable by an interpreter.
            tmp.child("tools/.git/hooks/payload.sh")
                .write_str("evil")
                .unwrap();
            assert_ne!(before, integrity_root_digest(tmp.path(), "tools").unwrap());

            // And its domain separation: same tree, different digest family.
            assert_ne!(
                integrity_root_digest(tmp.path(), "tools").unwrap(),
                dir_digest(&tmp.path().join("tools")).unwrap()
            );
        }

        #[cfg(unix)]
        #[test]
        fn integrity_root_digest_rejects_symlinks_dir_digest_skips() {
            let tmp = assert_fs::TempDir::new().unwrap();
            tmp.child("tools/agent.py").write_str("x").unwrap();
            tmp.child("tools/inner.py").write_str("y").unwrap();

            // A symlink to a CONTAINED sibling: dir_digest silently skips it;
            // the integrity root rejects it (ruling: reject all).
            std::os::unix::fs::symlink(
                tmp.child("tools/inner.py").path(),
                tmp.child("tools/link.py").path(),
            )
            .unwrap();
            assert!(dir_digest(&tmp.path().join("tools")).is_ok());
            let err = integrity_root_digest(tmp.path(), "tools").unwrap_err();
            assert!(err.to_string().contains("symlink"), "{err}");
        }

        #[cfg(unix)]
        #[test]
        fn integrity_root_digest_rejects_non_regular_files() {
            // A FIFO with no writer would block fs::read forever — the gate
            // must fail closed on any non-regular file, never hang. A unix
            // socket is the same file-type class, creatable with std alone.
            let tmp = assert_fs::TempDir::new().unwrap();
            tmp.child("tools/agent.py").write_str("x").unwrap();
            let _listener =
                std::os::unix::net::UnixListener::bind(tmp.path().join("tools/sock")).unwrap();

            let err = integrity_root_digest(tmp.path(), "tools").unwrap_err();
            assert!(err.to_string().contains("non-regular file"), "{err}");

            // The same special file declared directly is rejected too.
            assert!(contained_file_digest(tmp.path(), "tools/sock").is_err());
        }

        #[test]
        fn integrity_root_digest_rejects_excessive_depth() {
            let tmp = assert_fs::TempDir::new().unwrap();
            let mut dir = tmp.path().join("root");
            for _ in 0..=MAX_DIRECTORY_DEPTH {
                dir.push("nested");
            }
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("file.txt"), b"deep").unwrap();

            assert!(integrity_root_digest(tmp.path(), "root").is_err());
        }
    }

    /// The content-pinning invariant, one layer below the trust-store proptest
    /// (`any_single_byte_flip_in_any_pinned_file_regates` in the trust crate):
    /// for a skill directory, ANY change — a single flipped byte in any file,
    /// a file added, removed, or renamed — must change `dir_digest`, because
    /// the lockfile pin (and therefore use-time verification and, through the
    /// lock bytes, the trust digest itself) is only as strong as this digest.
    ///
    /// NEVER delete or weaken this test.
    mod content_pinning_invariant {
        use super::*;
        use proptest::prelude::*;

        /// A small random file tree: 1..=4 files, each with a relative path of
        /// 1..=2 components and 1..=64 arbitrary bytes of content. File names
        /// carry a `.f` suffix directory names never do, so a generated file
        /// can't collide with another path's directory prefix.
        fn file_tree() -> impl Strategy<Value = Vec<(String, Vec<u8>)>> {
            let relpath = "([a-z]{1,8}/)?[a-z]{1,8}\\.f";
            prop::collection::btree_map(relpath, prop::collection::vec(any::<u8>(), 1..=64), 1..=4)
                .prop_map(|m| m.into_iter().collect())
        }

        fn write_tree(root: &Path, files: &[(String, Vec<u8>)]) {
            for (rel, bytes) in files {
                let path = root.join(rel);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, bytes).unwrap();
            }
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(64))]

            #[test]
            fn any_single_byte_flip_changes_the_digest(
                files in file_tree(),
                file_pick: prop::sample::Index,
                byte_pick: prop::sample::Index,
            ) {
                let tmp = tempfile::tempdir().unwrap();
                write_tree(tmp.path(), &files);
                let before = dir_digest(tmp.path()).unwrap();

                let (rel, bytes) = &files[file_pick.index(files.len())];
                let mut flipped = bytes.clone();
                let i = byte_pick.index(flipped.len());
                flipped[i] ^= 0xff;
                fs::write(tmp.path().join(rel), &flipped).unwrap();

                prop_assert_ne!(before, dir_digest(tmp.path()).unwrap());
            }

            /// The same invariant for D3 integrity roots (contract §8): a
            /// single flipped byte ANYWHERE in a declared root — entry file or
            /// transitive import — must change `integrity_root_digest`,
            /// because strict locked verification (and, through the lock
            /// bytes, the trust digest) is only as strong as this digest.
            ///
            /// NEVER delete or weaken this test.
            #[test]
            fn any_single_byte_flip_changes_the_integrity_root_digest(
                files in file_tree(),
                file_pick: prop::sample::Index,
                byte_pick: prop::sample::Index,
            ) {
                let tmp = tempfile::tempdir().unwrap();
                let root = tmp.path().join("declared-root");
                fs::create_dir(&root).unwrap();
                write_tree(&root, &files);
                let before = integrity_root_digest(tmp.path(), "declared-root").unwrap();

                let (rel, bytes) = &files[file_pick.index(files.len())];
                let mut flipped = bytes.clone();
                let i = byte_pick.index(flipped.len());
                flipped[i] ^= 0xff;
                fs::write(root.join(rel), &flipped).unwrap();

                prop_assert_ne!(
                    before,
                    integrity_root_digest(tmp.path(), "declared-root").unwrap()
                );
            }

            #[test]
            fn adding_removing_or_renaming_a_file_changes_the_digest(
                files in file_tree(),
                file_pick: prop::sample::Index,
            ) {
                let tmp = tempfile::tempdir().unwrap();
                write_tree(tmp.path(), &files);
                let before = dir_digest(tmp.path()).unwrap();
                let (rel, bytes) = &files[file_pick.index(files.len())];

                // Rename: same bytes under a new path must change the digest.
                let renamed = tmp.path().join(format!("{rel}.renamed"));
                fs::rename(tmp.path().join(rel), &renamed).unwrap();
                let after_rename = dir_digest(tmp.path()).unwrap();
                prop_assert_ne!(&before, &after_rename);

                // Remove: dropping the file entirely must change it again.
                fs::remove_file(&renamed).unwrap();
                let after_remove = dir_digest(tmp.path()).unwrap();
                prop_assert_ne!(&before, &after_remove);

                // Add it back at a fresh path: differs from every prior state.
                fs::write(tmp.path().join("added.new"), bytes).unwrap();
                let after_add = dir_digest(tmp.path()).unwrap();
                prop_assert_ne!(&before, &after_add);
                prop_assert_ne!(&after_remove, &after_add);
            }
        }
    }
}
