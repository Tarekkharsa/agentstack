//! Recursive tree copy that takes the APFS fast path when — and only when —
//! the result is indistinguishable from the ordinary copy.
//!
//! [`copy_dir_all`] is a drop-in replacement for
//! [`agentstack_core::util::fsx::copy_dir_all`], and falls back to it for
//! anything it cannot clone. Prefer it at every non-following call site; it
//! never has a different outcome, only a faster one.
//!
//! # Why one syscall and not one per file
//!
//! `clonefile(2)` on a *directory* clones the whole hierarchy copy-on-write in
//! a single call. Measured on a 2000-file tree (512 bytes each, 20
//! directories, APFS, warm cache): the ordinary loop takes 415–1067 ms, the
//! eligibility scan below 6–9 ms, and the clone itself 15–20 ms — about 22 ms
//! against about 500 ms. Cloning the same tree file by file instead
//! (`/bin/cp -c -R`, which is what a per-file clone API would give us) measured
//! 470–580 ms: no better than the loop. The saving is in the syscall count,
//! not in the bytes, so the whole optimization is the one call on the
//! directory. A design that cloned and then walked the result to repair it
//! would hand back exactly what it saved.
//!
//! # Why a scan instead of a repair
//!
//! `clonefile` reproduces the source exactly. `fsx::copy_dir_all` deliberately
//! does not: it drops `.git`, and it hands symlinks to `fs::copy`, which reads
//! through them. So this module does not clone and then fix the difference —
//! it clones only when [`tree_is_eligible`] proves there is no difference to
//! fix, at the cost of one metadata-only pass that reads no file contents.
//!
//! The unsafe call itself is not here. It is [`crate::sys::clone_tree`], with
//! the rest of the crate's FFI.

use std::fs;
use std::path::Path;

use anyhow::Result;

/// Recursively copy `src` into `dst`, skipping `.git`, with the same result as
/// [`agentstack_core::util::fsx::copy_dir_all`] — including its errors, which
/// callers rely on: a symlink to a directory, or a broken one, still fails.
///
/// On macOS an eligible tree is one `clonefile(2)` call instead of a copy per
/// file. Anything else — an ineligible tree, a non-APFS volume, a cross-volume
/// copy, a destination that already exists, any other platform — runs the
/// ordinary loop. The fast path can only ever be skipped, never be the reason
/// a copy fails.
pub fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    if try_clone(src, dst) {
        return Ok(());
    }
    agentstack_core::util::fsx::copy_dir_all(src, dst)
}

/// Reproduce `copy_dir_all(src, dst)` with a single clone if that lands in
/// exactly the state the loop would have left. `true` means the destination
/// tree is complete and the caller is done.
fn try_clone(src: &Path, dst: &Path) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    // The clone insists its destination not exist, while the loop merges into
    // whatever is already there. Merging is left to the loop.
    if fs::symlink_metadata(dst).is_ok() {
        return false;
    }
    // A symlinked `src` is left to the loop, which reads through it.
    if !fs::symlink_metadata(src)
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        return false;
    }

    // Clone into a temporary sibling and rename it into place, rather than
    // cloning straight onto `dst`. The name is one we own, so a failed or
    // partial clone can be cleaned up with no chance of deleting a directory
    // that another writer put at `dst` in the meantime; the rename is then the
    // only thing that touches `dst`, and it is atomic. The store already
    // copies under this discipline for the same reason.
    let parent = match dst.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let tmp = parent.join(format!(
        ".agentstack-clone-{}-{}",
        std::process::id(),
        nanos()
    ));

    // `create_dir_all` on the temporary name does double duty: it creates
    // exactly the ancestor directories the loop's own `create_dir_all(dst)`
    // would have created, and it is the probe for the mode a new directory
    // gets — measured on the destination's own filesystem under this process's
    // umask, rather than by calling `umask(2)`, which is process-global and
    // would race other threads.
    if fs::create_dir_all(&tmp).is_err() {
        return false;
    }
    let new_dir_mode = match dir_mode(&tmp) {
        Some(mode) => mode,
        None => {
            let _ = fs::remove_dir(&tmp);
            return false;
        }
    };
    // The probe has to go again: the clone wants the name free.
    if fs::remove_dir(&tmp).is_err() {
        return false;
    }

    if !tree_is_eligible(src, new_dir_mode) {
        return false;
    }
    if !crate::sys::clone_tree(src, &tmp) {
        // Nothing is reported: the fast path must never be the reason a copy
        // fails, and the loop is about to run anyway.
        let _ = fs::remove_dir_all(&tmp);
        return false;
    }
    if fs::rename(&tmp, dst).is_err() {
        let _ = fs::remove_dir_all(&tmp);
        return false;
    }
    true
}

/// Whether every entry under `root` copies identically both ways. Each rule is
/// a place where the loop and a clone would otherwise disagree, and a single
/// disagreement anywhere disqualifies the whole tree, because the clone is
/// all-or-nothing.
///
/// * **`.git`** — the loop drops it at every level; a clone keeps it.
/// * **anything that is not a regular file or a real directory** — the loop
///   hands a symlink to `fs::copy`, which reads through it: a link to a file
///   lands as a regular file holding the target's bytes, and a link to a
///   directory or a broken link is an *error*. A clone reproduces the link
///   itself. The error cases matter as much as the success case —
///   `store::snapshot_content` documents that its callers reject symlinks
///   first — so any link, fifo, socket or device disqualifies the tree.
/// * **a file with more than one link** — the loop breaks the sharing into
///   independent files; a clone need not.
/// * **a file the owner cannot read** — `fs::copy` fails on it, a clone does
///   not. The mode bit is the cheap approximation: it misses a file owned by
///   somebody else and it misses ACL denials, both outside the trees this is
///   used on, where the bytes were just written by this user.
/// * **a directory whose mode is not `new_dir_mode`** — the loop creates
///   destination directories with `create_dir_all`, which applies the umask
///   and ignores the source's bits; a clone carries them over.
///
/// Regular *files* need no mode rule. `fs::copy` on macOS is `fcopyfile` with
/// `COPYFILE_ALL`, so it already carries mode, timestamps, ACLs and xattrs
/// across — the same as a clone.
///
/// This is a scan, so it races anything mutating the tree underneath it. So
/// does the loop, which stats each entry and then copies it.
fn tree_is_eligible(root: &Path, new_dir_mode: u32) -> bool {
    // The root's own mode counts: the loop would have created the destination
    // root with `create_dir_all`, not with the source's bits.
    if dir_mode(root) != Some(new_dir_mode) {
        return false;
    }
    // Iterative, not recursive: a pass that only reads metadata has no reason
    // to risk the stack on a deep tree.
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            return false;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                return false;
            };
            if entry.file_name() == ".git" {
                return false;
            }
            // `DirEntry::metadata` does not follow symlinks, so a link is seen
            // as a link rather than as whatever it points at.
            let Ok(md) = entry.metadata() else {
                return false;
            };
            if md.file_type().is_dir() {
                if permissions(&md) != new_dir_mode {
                    return false;
                }
                pending.push(entry.path());
            } else if md.file_type().is_file() {
                if links(&md) > 1 || permissions(&md) & 0o400 == 0 {
                    return false;
                }
            } else {
                return false;
            }
        }
    }
    true
}

/// Permission bits of `path`, or `None` if it is not a real directory.
fn dir_mode(path: &Path) -> Option<u32> {
    let md = fs::symlink_metadata(path).ok()?;
    md.is_dir().then(|| permissions(&md))
}

#[cfg(unix)]
fn permissions(md: &fs::Metadata) -> u32 {
    std::os::unix::fs::MetadataExt::mode(md) & 0o777
}

#[cfg(unix)]
fn links(md: &fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::nlink(md)
}

// The fast path is macOS-only, so these two exist purely to keep the module
// compiling elsewhere; `try_clone` has already returned by the time they would
// be reached.
#[cfg(not(unix))]
fn permissions(_md: &fs::Metadata) -> u32 {
    0
}

#[cfg(not(unix))]
fn links(_md: &fs::Metadata) -> u64 {
    1
}

/// A per-call suffix, so two copies running at once cannot pick the same
/// temporary name. With the pid, that is unique enough for a name that exists
/// for the length of one clone.
fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;
    use std::path::PathBuf;

    /// Everything a copy is supposed to preserve, keyed by path relative to
    /// the root: each entry's kind, its permission bits, and (for a file) its
    /// bytes. Comparing two of these is how the fast path is held to "the same
    /// result the loop gives", instead of to a handful of spot assertions.
    #[cfg(unix)]
    fn describe(root: &Path) -> std::collections::BTreeMap<PathBuf, String> {
        let mut out = std::collections::BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                let md = entry.metadata().unwrap();
                let mode = permissions(&md);
                let kind = if md.file_type().is_symlink() {
                    format!("symlink {:?}", fs::read_link(&path).unwrap())
                } else if md.is_dir() {
                    pending.push(path.clone());
                    "dir".to_string()
                } else {
                    format!("file {:?}", fs::read(&path).unwrap())
                };
                out.insert(rel, format!("{kind} {mode:04o}"));
            }
        }
        out
    }

    fn eligible_src(tmp: &assert_fs::TempDir) -> PathBuf {
        let src = tmp.child("src");
        src.child("top.md").write_str("top\n").unwrap();
        src.child("a/b/deep.md").write_str("deep\n").unwrap();
        src.path().to_path_buf()
    }

    /// The headline claim: whatever path runs, the tree that lands is the tree
    /// the loop would have written — kind, mode and bytes, entry by entry.
    #[cfg(unix)]
    #[test]
    fn a_nested_tree_copies_to_exactly_what_the_loop_produces() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let src = tmp.child("src");
        src.child("top.md").write_str("top\n").unwrap();
        src.child("a/one.txt").write_str("one\n").unwrap();
        src.child("a/b/two.txt").write_str("two\n").unwrap();
        src.child("a/b/c/three.bin")
            .write_binary(&[0, 1, 2])
            .unwrap();
        src.child("empty").create_dir_all().unwrap();
        fs::set_permissions(
            src.child("a/one.txt").path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .unwrap();

        let fast = tmp.child("fast").path().to_path_buf();
        copy_dir_all(src.path(), &fast).unwrap();
        let slow = tmp.child("slow").path().to_path_buf();
        agentstack_core::util::fsx::copy_dir_all(src.path(), &slow).unwrap();

        assert_eq!(describe(&fast), describe(&slow));
        assert_eq!(
            fs::read_to_string(fast.join("a/b/two.txt")).unwrap(),
            "two\n"
        );
        assert!(fast.join("empty").is_dir());
    }

    /// A clone refuses a destination that exists; the loop merges into it, and
    /// that is the behaviour the wrapper has to keep.
    #[test]
    fn an_existing_destination_is_merged_into() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let src = tmp.child("src");
        src.child("new.md").write_str("new\n").unwrap();
        let dst = tmp.child("dst");
        dst.child("already.md").write_str("already\n").unwrap();
        dst.child("a/f.md").write_str("stale\n").unwrap();
        src.child("a/f.md").write_str("fresh\n").unwrap();

        copy_dir_all(src.path(), dst.path()).unwrap();

        assert_eq!(
            fs::read_to_string(dst.child("already.md").path()).unwrap(),
            "already\n"
        );
        assert_eq!(
            fs::read_to_string(dst.child("new.md").path()).unwrap(),
            "new\n"
        );
        assert_eq!(
            fs::read_to_string(dst.child("a/f.md").path()).unwrap(),
            "fresh\n"
        );
    }

    /// The non-following copy reads through a link to a file: the destination
    /// holds a regular file with the target's bytes, never a link. A clone
    /// would have reproduced the link, so this is a tree the fast path has to
    /// decline.
    #[cfg(unix)]
    #[test]
    fn a_link_to_a_file_still_lands_as_a_regular_file() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let target = tmp.child("target.md");
        target.write_str("target\n").unwrap();
        let src = tmp.child("src");
        src.child("plain.md").write_str("plain\n").unwrap();
        std::os::unix::fs::symlink(target.path(), src.child("link.md").path()).unwrap();

        let fast = tmp.child("fast").path().to_path_buf();
        copy_dir_all(src.path(), &fast).unwrap();
        let slow = tmp.child("slow").path().to_path_buf();
        agentstack_core::util::fsx::copy_dir_all(src.path(), &slow).unwrap();

        assert_eq!(describe(&fast), describe(&slow));
        assert!(!fs::symlink_metadata(fast.join("link.md"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(fast.join("link.md")).unwrap(),
            "target\n"
        );
    }

    /// A link to a directory is an error in the non-following copy and stays
    /// one: a clone would have turned it into a success.
    #[cfg(unix)]
    #[test]
    fn a_link_to_a_directory_is_still_an_error() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let real = tmp.child("real");
        real.child("f.md").write_str("f\n").unwrap();
        let src = tmp.child("src");
        src.create_dir_all().unwrap();
        std::os::unix::fs::symlink(real.path(), src.child("linked").path()).unwrap();

        assert!(copy_dir_all(src.path(), tmp.child("dst").path()).is_err());
    }

    /// A broken link is an error too — `fs::copy` cannot open the target.
    #[cfg(unix)]
    #[test]
    fn a_broken_link_is_still_an_error() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let src = tmp.child("src");
        src.create_dir_all().unwrap();
        std::os::unix::fs::symlink(tmp.child("missing").path(), src.child("dangling").path())
            .unwrap();

        assert!(copy_dir_all(src.path(), tmp.child("dst").path()).is_err());
    }

    /// A `.git` below the root, not just at the top, keeps being dropped.
    #[test]
    fn a_nested_git_dir_is_skipped_and_the_rest_still_copies() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let src = tmp.child("src");
        src.child("a/SKILL.md").write_str("# skill\n").unwrap();
        src.child("a/.git/HEAD").write_str("ref: x\n").unwrap();
        let dst = tmp.child("dst").path().to_path_buf();

        copy_dir_all(src.path(), &dst).unwrap();

        assert!(dst.join("a/SKILL.md").exists());
        assert!(!dst.join("a/.git").exists());
    }

    /// Missing destination parents are created, whichever path runs.
    #[test]
    fn missing_destination_parents_are_created() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let src = tmp.child("src");
        src.child("f.md").write_str("f\n").unwrap();
        let dst = tmp.child("deep/deeper/dst").path().to_path_buf();

        copy_dir_all(src.path(), &dst).unwrap();

        assert_eq!(fs::read_to_string(dst.join("f.md")).unwrap(), "f\n");
    }

    /// The temporary sibling the clone builds under must never survive the
    /// call, on either path.
    #[test]
    fn no_temporary_is_left_beside_the_destination() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let src = tmp.child("src");
        src.child("a/f.md").write_str("f\n").unwrap();
        let out = tmp.child("out");
        out.create_dir_all().unwrap();

        copy_dir_all(src.path(), out.child("dst").path()).unwrap();

        let siblings: Vec<_> = fs::read_dir(out.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(siblings, vec![std::ffi::OsString::from("dst")]);
    }

    /// The gate itself. Everything above proves `copy_dir_all` behaves; only
    /// these prove *which* path it took. Without them a fast path that
    /// silently never fires would pass the whole file.
    #[cfg(target_os = "macos")]
    mod fast_path {
        use super::*;

        #[test]
        fn an_ordinary_tree_is_cloned() {
            let tmp = assert_fs::TempDir::new().unwrap();
            let src = eligible_src(&tmp);
            let dst = tmp.child("dst").path().to_path_buf();

            assert!(
                try_clone(&src, &dst),
                "the clone declined an eligible tree — is this volume APFS?"
            );

            let slow = tmp.child("slow").path().to_path_buf();
            agentstack_core::util::fsx::copy_dir_all(&src, &slow).unwrap();
            assert_eq!(describe(&dst), describe(&slow));
        }

        #[test]
        fn an_existing_destination_is_declined() {
            let tmp = assert_fs::TempDir::new().unwrap();
            let src = eligible_src(&tmp);
            tmp.child("dst").create_dir_all().unwrap();

            assert!(!try_clone(&src, tmp.child("dst").path()));
        }

        #[test]
        fn a_symlink_anywhere_declines_the_whole_tree() {
            let tmp = assert_fs::TempDir::new().unwrap();
            let src = eligible_src(&tmp);
            std::os::unix::fs::symlink(src.join("top.md"), src.join("a/b/link.md")).unwrap();

            assert!(!try_clone(&src, tmp.child("dst").path()));
        }

        #[test]
        fn a_git_dir_anywhere_declines_the_whole_tree() {
            let tmp = assert_fs::TempDir::new().unwrap();
            let src = eligible_src(&tmp);
            tmp.child("src/a/.git/HEAD").write_str("ref: x\n").unwrap();

            assert!(!try_clone(&src, tmp.child("dst").path()));
        }

        #[test]
        fn a_symlinked_source_is_declined() {
            let tmp = assert_fs::TempDir::new().unwrap();
            let src = eligible_src(&tmp);
            let via_link = tmp.child("via-link").path().to_path_buf();
            std::os::unix::fs::symlink(&src, &via_link).unwrap();

            assert!(!try_clone(&via_link, tmp.child("dst").path()));
        }

        #[test]
        fn a_directory_with_an_unusual_mode_is_declined() {
            use std::os::unix::fs::PermissionsExt;
            let tmp = assert_fs::TempDir::new().unwrap();
            let src = eligible_src(&tmp);
            fs::set_permissions(src.join("a"), PermissionsExt::from_mode(0o700)).unwrap();

            assert!(!try_clone(&src, tmp.child("dst").path()));
        }

        #[test]
        fn a_file_the_owner_cannot_read_is_declined() {
            use std::os::unix::fs::PermissionsExt;
            let tmp = assert_fs::TempDir::new().unwrap();
            let src = eligible_src(&tmp);
            fs::set_permissions(src.join("top.md"), PermissionsExt::from_mode(0o200)).unwrap();

            assert!(!try_clone(&src, tmp.child("dst").path()));
            // …and the loop it falls back to still fails on that file, which is
            // the error a clone would have quietly replaced with success.
            assert!(copy_dir_all(&src, tmp.child("dst2").path()).is_err());
        }

        #[test]
        fn a_hard_linked_file_is_declined() {
            let tmp = assert_fs::TempDir::new().unwrap();
            let src = eligible_src(&tmp);
            fs::hard_link(src.join("top.md"), src.join("also-top.md")).unwrap();

            assert!(!try_clone(&src, tmp.child("dst").path()));
        }

        /// A declined clone must leave the destination side exactly as it found
        /// it: no half-built tree, no temporary sibling.
        #[test]
        fn a_declined_clone_leaves_nothing_behind() {
            let tmp = assert_fs::TempDir::new().unwrap();
            let src = eligible_src(&tmp);
            tmp.child("src/.git/HEAD").write_str("ref: x\n").unwrap();
            let out = tmp.child("out");
            out.create_dir_all().unwrap();

            assert!(!try_clone(&src, out.child("dst").path()));

            assert_eq!(fs::read_dir(out.path()).unwrap().count(), 0);
        }
    }

    /// A rough clone-against-loop measurement over ~2000 small files. Ignored
    /// by default: it is a stopwatch, not an assertion.
    /// `cargo test -p agentstack --lib fsclone -- --ignored --nocapture`
    #[test]
    #[ignore = "timing measurement, not an assertion"]
    fn timing_clone_against_loop() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let src = tmp.child("src");
        let body = "x".repeat(512);
        for d in 0..20 {
            for f in 0..100 {
                src.child(format!("d{d}/f{f}.txt"))
                    .write_str(&body)
                    .unwrap();
            }
        }

        // Warm the source into the page cache so both runs are compared on
        // equal footing.
        let warm = tmp.child("warm").path().to_path_buf();
        agentstack_core::util::fsx::copy_dir_all(src.path(), &warm).unwrap();

        let loop_dst = tmp.child("loop").path().to_path_buf();
        let t0 = std::time::Instant::now();
        agentstack_core::util::fsx::copy_dir_all(src.path(), &loop_dst).unwrap();
        let loop_time = t0.elapsed();

        let fast_dst = tmp.child("fast").path().to_path_buf();
        let t1 = std::time::Instant::now();
        copy_dir_all(src.path(), &fast_dst).unwrap();
        let fast_time = t1.elapsed();

        println!("2000 files: loop {loop_time:?}, fsclone {fast_time:?}");
    }
}
