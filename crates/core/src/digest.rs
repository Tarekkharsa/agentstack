//! Content digests — the bytes-to-identity layer everything above builds on.
//!
//! Extracted verbatim from the cli crate (store/resolve): `sha256_hex` gives a
//! server definition its lockfile checksum; `dir_digest` gives a skill
//! directory its lockfile checksum. These digests are what `agentstack.lock`
//! pins and what use-time verification compares against, so their semantics
//! are security-relevant: any byte, path, or file-set change must change the
//! digest (property-tested below).
//!
//! The stat-fingerprint *cache* over `dir_digest` deliberately stays in the
//! cli crate — caching is a performance policy, not part of the digest's
//! definition, and core stays free of `~/.agentstack` knowledge.

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

const DIR_DIGEST_DOMAIN: &[u8] = b"agentstack-dir-digest-v2\0";
const MAX_DIRECTORY_DEPTH: usize = 64;

/// SHA-256 hex digest of a byte string.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// SHA-256 digest of a directory's contents (relative paths + file bytes,
/// sorted; `.git` excluded).
pub fn dir_digest(root: &Path) -> Result<String> {
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
    Ok(format!("{:x}", hasher.finalize()))
}

/// Collect every file under `dir` as a path relative to `root`, recursing into
/// subdirectories; `.git` is excluded. Shared by [`dir_digest`] and the cli's
/// stat-fingerprint cache so both walk the identical file set.
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
    fn dir_digest_stable_and_sensitive() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("a.txt").write_str("hello").unwrap();
        tmp.child("sub/b.txt").write_str("world").unwrap();
        let d1 = dir_digest(tmp.path()).unwrap();
        let d2 = dir_digest(tmp.path()).unwrap();
        assert_eq!(d1, d2);
        assert_eq!(d1.len(), 64);
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
