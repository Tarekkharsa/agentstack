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

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

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
    for rel in &files {
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update([0]);
        let bytes = fs::read(root.join(rel))
            .with_context(|| format!("reading {}", root.join(rel).display()))?;
        hasher.update(bytes);
        hasher.update([0]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Collect every file under `dir` as a path relative to `root`, recursing into
/// subdirectories; `.git` is excluded. Shared by [`dir_digest`] and the cli's
/// stat-fingerprint cache so both walk the identical file set.
pub fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_files(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
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
