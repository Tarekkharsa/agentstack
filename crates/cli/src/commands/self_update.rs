//! `agentstack self update` — replace the running binary with the newest
//! published release.
//!
//! Until this existed there was no upgrade path at all: a binary installed by
//! `install.sh` or downloaded from the releases page stayed at whatever version
//! it was, and nothing ever said a newer one existed (review finding C4b).
//!
//! House shape, same as every other mutating command: **preview by default,
//! act on `--write`.** A preview downloads nothing and touches nothing.
//!
//! ## The security-load-bearing part
//!
//! The downloaded archive is verified against the `checksums.txt` published
//! with the release **before it is unpacked, executed, or moved anywhere**
//! (byte order in [`install_verified`]: hash, compare, bail — extraction is
//! after the comparison, not before). A mismatch aborts with both digests
//! printed and leaves the existing binary byte-for-byte untouched;
//! `refuses_a_corrupted_download_and_leaves_the_binary_intact` is the witness.
//!
//! What that check *is*, precisely (invariant 8 — claims match enforcement):
//! integrity of the transfer, not provenance of the release. Both the archive
//! and `checksums.txt` come from the same TLS-authenticated origin, so this
//! catches a truncated, corrupted, or mismatched-asset download — it is not a
//! signature, and it cannot tell you the release itself is genuine. The
//! provenance answer is the build attestation
//! (`gh attestation verify … --repo Tarekkharsa/agentstack`, RELEASING.md),
//! which this command names rather than claiming to have done. This is exactly
//! the guarantee `install.sh` gives, implemented the same way.
//!
//! ## What it deliberately cannot do
//!
//! Three situations get an explanation and a working next step instead of an
//! obscure failure: a Homebrew-managed binary (`brew upgrade`), a binary in a
//! directory this user cannot write (`sudo`), and a platform with no published
//! asset (the releases page). A source build is a fourth: replacing somebody's
//! `target/release/agentstack` with a download would be a surprise, so it is
//! refused and pointed at `cargo build --release`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use owo_colors::OwoColorize;
use sha2::{Digest, Sha256};

use crate::cli::SelfUpdateArgs;
use crate::update::{self, GitHub, Release, ReleaseSource};

/// The `checksums.txt` every release since v0.6.0 carries (see `install.sh`).
const CHECKSUMS: &str = "checksums.txt";

pub fn run(args: &SelfUpdateArgs) -> Result<()> {
    run_with(args, &GitHub::interactive())
}

/// [`run`] with the release channel injected, so tests never need a network.
fn run_with(args: &SelfUpdateArgs, source: &dyn ReleaseSource) -> Result<()> {
    if update::opted_out() {
        bail!(
            "{} is set, so AgentStack will not contact the release channel.\n  \
             Unset it to update, or download the release yourself: {}",
            update::NO_CHECK_ENV,
            releases_url()
        );
    }
    if let Some(base) = update::override_base() {
        // A non-default channel is worth one loud line: the checksum gate
        // still runs, but it only ever proves the archive matches the
        // checksums *that same source* served.
        println!(
            "{} using {}={} — not the official release channel",
            "⚠".yellow(),
            update::BASE_URL_ENV,
            base
        );
    }

    let current = update::current();
    let latest = source
        .latest()
        .context("could not reach the release channel")?;
    if latest.version <= current {
        println!(
            "{} agentstack {current} is the latest release.",
            "✓".green()
        );
        return Ok(());
    }

    // The running binary, fully resolved — a `self link` symlink must not be
    // what we replace; the real file behind it is.
    let exe = super::self_cmd::running_exe()?;
    println!(
        "{} {current} → {}",
        "Update available:".bold(),
        latest.version.to_string().bold()
    );
    println!("  release notes  {}", latest.notes_url());
    println!("  binary         {}", exe.display());

    // Every reason this cannot proceed is discovered BEFORE anything is
    // downloaded, so a preview is already the full answer.
    if let Some(blocker) = blocker(&exe) {
        bail!("{}", blocker.explain(&exe, &latest));
    }
    let target = target_triple().expect("blocker() rejects unsupported platforms");
    let asset = asset_name(target);
    println!("  asset          {asset}");

    if !args.write {
        println!(
            "\n{}",
            "Nothing has been changed yet. Re-run with --write to download, verify, and install it."
                .dimmed()
        );
        return Ok(());
    }

    println!("\n  downloading {CHECKSUMS} …");
    // No checksums, no install. The underlying cause (404 vs network) is
    // chained onto this by anyhow and printed with it.
    let checksums = source.asset(&latest.tag, CHECKSUMS).with_context(|| {
        format!(
            "could not fetch {CHECKSUMS} for {} — without it there is nothing to verify the \
             download against, so nothing is installed. Releases before v0.6.0 carry none; \
             install those by hand from {}",
            latest.tag,
            releases_url()
        )
    })?;
    let checksums = String::from_utf8_lossy(&checksums).into_owned();

    println!("  downloading {asset} …");
    let archive = source.asset(&latest.tag, &asset)?;

    install_verified(&archive, &asset, &checksums, target, &exe, &latest)?;

    println!(
        "\n{} updated {current} → {} at {}",
        "✓".green(),
        latest.version,
        exe.display()
    );
    println!(
        "  {}",
        format!(
            "Provenance is a separate question: gh attestation verify {asset} --repo {}",
            update::REPO
        )
        .dimmed()
    );
    Ok(())
}

// ── verify, then install ────────────────────────────────────────────────────

/// Hash `archive`, compare it with the release's published digest, and only
/// then unpack and swap it into place.
///
/// The ordering here is the security property, so it is written to be read in
/// order: nothing below the comparison runs unless the digests match, and
/// nothing above it touches the filesystem. On mismatch the process ends with
/// `dest` exactly as it was.
fn install_verified(
    archive: &[u8],
    asset: &str,
    checksums: &str,
    target: &str,
    dest: &Path,
    release: &Release,
) -> Result<()> {
    let expected = checksum_for(checksums, asset)
        .with_context(|| format!("{CHECKSUMS} for {} has no entry for {asset}", release.tag))?;
    let actual = sha256_hex(archive);
    if actual != expected {
        // Loud, specific, and terminal: naming both digests is what lets
        // someone tell a flaky CDN from a tampered artifact. Deliberately
        // uncoloured — `main` sanitizes every error chain before printing it
        // (§A.2 #6), which strips escape sequences, so the emphasis has to be
        // in the words.
        bail!(
            "CHECKSUM MISMATCH for {asset}\n  \
             expected  {expected}\n  \
             actual    {actual}\n\
             The download does not match the checksum published with {}. It may be corrupted \
             or tampered with.\n\
             Nothing was installed — {} is untouched.",
            release.tag,
            dest.display()
        );
    }
    println!("  {} sha256 verified {}", "✓".green(), &actual[..16]);

    // Verified bytes only from here down.
    let staging = Staging::beside(dest)?;
    let archive_path = staging.dir.join(asset);
    std::fs::write(&archive_path, archive)
        .with_context(|| format!("writing {}", archive_path.display()))?;
    unpack(&archive_path, &staging.dir)?;

    // The release layout (release.yml "Package"): `agentstack-<target>/agentstack`.
    // Only this one path is taken out of the archive — whatever else it holds
    // (README, licences) is left in the staging dir and thrown away with it.
    let staged_bin = staging
        .dir
        .join(format!("agentstack-{target}"))
        .join("agentstack");
    if !staged_bin.is_file() {
        bail!(
            "{asset} does not contain agentstack-{target}/agentstack — \
             nothing was installed, {} is untouched",
            dest.display()
        );
    }
    make_executable(&staged_bin)?;

    // Atomic swap. `rename` within one directory is atomic, so an interrupted
    // update leaves either the old binary or the new one, never a half-written
    // file — and on unix replacing a *running* executable this way is safe
    // (the running process keeps its own inode alive). The staging dir lives
    // beside `dest` precisely so this is a same-filesystem rename.
    std::fs::rename(&staged_bin, dest).with_context(|| {
        format!(
            "installing the new binary at {} (is the directory writable?)",
            dest.display()
        )
    })?;
    Ok(())
}

/// The digest `checksums.txt` publishes for `asset`, if any.
///
/// `sha256sum` format: `<64 hex>  <name>`. Parsed defensively — the file comes
/// off the network, so it is line-bounded, the digest must be exactly 64
/// lowercase hex characters, and the name must match exactly (no prefix or
/// suffix matching, which is how you accidentally accept
/// `agentstack-x86_64-apple-darwin.tar.gz` for the aarch64 asset).
fn checksum_for(checksums: &str, asset: &str) -> Option<String> {
    for line in checksums.lines().take(1000) {
        let mut parts = line.split_whitespace();
        let (Some(digest), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        if parts.next().is_some() || name != asset {
            continue;
        }
        if digest.len() == 64
            && digest
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Some(digest.to_string());
        }
    }
    None
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A scratch directory beside the destination, removed however this function
/// returns. Beside — not in `/tmp` — so the final [`std::fs::rename`] stays on
/// one filesystem and therefore stays atomic.
struct Staging {
    dir: PathBuf,
}

impl Staging {
    fn beside(dest: &Path) -> Result<Staging> {
        let parent = dest
            .parent()
            .context("the running binary has no parent directory")?;
        // Not a security boundary (the parent dir is one we can already
        // write): just a name unlikely to collide with a concurrent update.
        let dir = parent.join(format!(
            ".agentstack-update-{}-{}",
            std::process::id(),
            update::current()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir)
            .with_context(|| format!("creating a staging directory at {}", dir.display()))?;
        Ok(Staging { dir })
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Unpack a verified `.tar.gz` into `into`.
///
/// Spawned as a fixed argv (`tar -xzf <archive>`) with the working directory
/// set to the staging dir — no shell, so nothing here can be word-split or
/// interpolated (invariant 7). `tar` is the same tool `install.sh` already
/// requires, which is why unpacking costs no new dependency.
fn unpack(archive: &Path, into: &Path) -> Result<()> {
    let status = Command::new("tar")
        .current_dir(into)
        .arg("-xzf")
        .arg(archive)
        .status()
        .context("running `tar` to unpack the release (tar must be on PATH)")?;
    if !status.success() {
        bail!("`tar` failed to unpack {}", archive.display());
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("making {} executable", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

// ── what this command cannot fix, explained ─────────────────────────────────

/// A reason `self update` must hand off to something else.
#[derive(Debug, PartialEq)]
enum Blocker {
    /// Installed by Homebrew — replacing the file would put the formula and
    /// the Cellar out of sync.
    Homebrew,
    /// A `cargo build` output. Downloading over it would be a surprise.
    SourceBuild,
    /// The directory holding the binary is not writable by this user.
    NotWritable,
    /// No `.tar.gz` published for this OS/arch.
    NoAsset,
}

impl Blocker {
    fn explain(&self, exe: &Path, latest: &Release) -> String {
        match self {
            Blocker::Homebrew => format!(
                "this binary is managed by Homebrew ({}).\n  \
                 Update it with:  brew upgrade agentstack\n  \
                 Replacing the file directly would leave the formula out of sync.",
                exe.display()
            ),
            Blocker::SourceBuild => format!(
                "this is a source build ({}), not an installed release.\n  \
                 Update it with:  git pull && cargo build --release\n  \
                 Then re-run `agentstack self link` to re-point your PATH entry.",
                exe.display()
            ),
            Blocker::NotWritable => format!(
                "{} is not writable by you.\n  \
                 Re-run with elevated rights:  sudo agentstack self update --write\n  \
                 Or install {} somewhere you own — see {}",
                exe.parent().unwrap_or(exe).display(),
                latest.tag,
                releases_url()
            ),
            Blocker::NoAsset => format!(
                "no release asset is published for this platform ({}/{}).\n  \
                 Download {} yourself from {}",
                std::env::consts::OS,
                std::env::consts::ARCH,
                latest.tag,
                releases_url()
            ),
        }
    }
}

/// Why this binary cannot be replaced in place, if it cannot be. Cheap,
/// filesystem-only checks — no network, so a preview reports them too.
fn blocker(exe: &Path) -> Option<Blocker> {
    if target_triple().is_none() {
        return Some(Blocker::NoAsset);
    }
    if is_homebrew(exe) {
        return Some(Blocker::Homebrew);
    }
    if is_source_build(exe) {
        return Some(Blocker::SourceBuild);
    }
    // The swap is a rename inside the parent directory, so that directory —
    // not the file — is what must be writable.
    let parent = exe.parent()?;
    if !crate::sys::dir_writable(parent) {
        return Some(Blocker::NotWritable);
    }
    None
}

/// Homebrew keeps the real file under a `Cellar` (or `linuxbrew` equivalent)
/// directory and symlinks it onto PATH; `exe` is already canonicalized, so the
/// Cellar component is visible here.
fn is_homebrew(exe: &Path) -> bool {
    exe.components()
        .any(|c| c.as_os_str() == "Cellar" || c.as_os_str() == "linuxbrew")
}

/// `…/target/release/agentstack` or `…/target/debug/agentstack` — a cargo
/// build output, which the `self link` workflow points PATH at.
fn is_source_build(exe: &Path) -> bool {
    let Some(profile) = exe.parent() else {
        return false;
    };
    let name = profile.file_name().and_then(|n| n.to_str());
    if !matches!(name, Some("release" | "debug")) {
        return false;
    }
    profile.parent().and_then(|p| p.file_name()) == Some(std::ffi::OsStr::new("target"))
}

/// The published target triple for this platform, or `None` when releases
/// carry no `.tar.gz` for it. Compile-time table — deliberately not derived
/// from anything the network said. Mirrors `install.sh`'s detection and the
/// build matrix in `.github/workflows/release.yml`; Windows is absent because
/// its asset is a `.zip`, which this command does not unpack.
fn target_triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        _ => None,
    }
}

/// The asset file name for a target — the name `checksums.txt` keys on.
fn asset_name(target: &str) -> String {
    format!("agentstack-{target}.tar.gz")
}

fn releases_url() -> String {
    format!("https://github.com/{}/releases", update::REPO)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    /// Build the archive layout a real release publishes:
    /// `agentstack-<target>/agentstack`, gzipped tar.
    fn fixture_archive(dir: &Path, target: &str, body: &str) -> Vec<u8> {
        let inner = dir.join(format!("agentstack-{target}"));
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("agentstack"), body).unwrap();
        let archive = dir.join("asset.tar.gz");
        let status = Command::new("tar")
            .current_dir(dir)
            .arg("-czf")
            .arg(&archive)
            .arg(format!("agentstack-{target}"))
            .status()
            .unwrap();
        assert!(status.success(), "fixture tar failed");
        let bytes = std::fs::read(&archive).unwrap();
        std::fs::remove_dir_all(&inner).unwrap();
        std::fs::remove_file(&archive).unwrap();
        bytes
    }

    fn release() -> Release {
        Release::from_tag("v99.0.0").unwrap()
    }

    /// SECURITY WITNESS: an archive whose bytes do not match the published
    /// checksum is refused, and the binary already installed survives
    /// byte-for-byte. Nothing is unpacked and nothing is executed.
    #[cfg(unix)]
    #[test]
    fn refuses_a_corrupted_download_and_leaves_the_binary_intact() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let target = "x86_64-unknown-linux-gnu";
        let asset = asset_name(target);

        let good = fixture_archive(tmp.path(), target, "#!new-binary");
        let honest = sha256_hex(&good);
        let checksums = format!("{honest}  {asset}\n");

        // The binary that is already installed.
        let dest = tmp.child("bin/agentstack");
        dest.write_str("#!the-binary-i-already-have").unwrap();

        // One flipped byte in the middle of the payload.
        let mut corrupted = good.clone();
        let mid = corrupted.len() / 2;
        corrupted[mid] ^= 0xff;
        assert_ne!(sha256_hex(&corrupted), honest);

        let err = install_verified(
            &corrupted,
            &asset,
            &checksums,
            target,
            dest.path(),
            &release(),
        )
        .expect_err("a corrupted download must be refused");
        let msg = err.to_string();
        assert!(msg.contains("CHECKSUM MISMATCH"), "{msg}");
        assert!(msg.contains("untouched"), "{msg}");
        assert_eq!(
            std::fs::read_to_string(dest.path()).unwrap(),
            "#!the-binary-i-already-have",
            "the existing binary must survive a refused update"
        );
        // No staging litter left beside it either.
        let leftovers: Vec<_> = std::fs::read_dir(tmp.path().join("bin"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(leftovers.len(), 1, "only the binary should remain");

        // The same bytes, honestly published, DO install — otherwise the test
        // above would pass on a function that refuses everything.
        install_verified(&good, &asset, &checksums, target, dest.path(), &release()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dest.path()).unwrap(),
            "#!new-binary"
        );
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dest.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "the installed binary must be runnable");
    }

    /// A release whose `checksums.txt` has no line for our asset is refused
    /// too — "no published digest" is never "install it anyway".
    #[cfg(unix)]
    #[test]
    fn a_missing_checksum_entry_is_refused() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let target = "x86_64-unknown-linux-gnu";
        let archive = fixture_archive(tmp.path(), target, "#!new");
        let dest = tmp.child("bin/agentstack");
        dest.write_str("#!old").unwrap();

        let other = format!(
            "{}  agentstack-aarch64-apple-darwin.tar.gz\n",
            sha256_hex(&archive)
        );
        let err = install_verified(
            &archive,
            &asset_name(target),
            &other,
            target,
            dest.path(),
            &release(),
        )
        .expect_err("a checksums.txt without our asset must be refused");
        assert!(err.to_string().contains("no entry for"), "{err}");
        assert_eq!(std::fs::read_to_string(dest.path()).unwrap(), "#!old");
    }

    /// `checksums.txt` is network text: hostile and malformed lines must not
    /// produce a digest, and the asset name must match exactly.
    #[test]
    fn checksum_parsing_is_defensive() {
        let good = "a".repeat(64);
        let file = format!(
            "# a comment\n\
             not-a-digest  agentstack-x86_64-unknown-linux-gnu.tar.gz\n\
             {good}  agentstack-x86_64-unknown-linux-gnu.tar.gz\n\
             {good}  agentstack-aarch64-apple-darwin.tar.gz\n"
        );
        assert_eq!(
            checksum_for(&file, "agentstack-x86_64-unknown-linux-gnu.tar.gz").as_deref(),
            Some(good.as_str())
        );
        // Substring matches must not count.
        assert_eq!(
            checksum_for(&file, "agentstack-x86_64-unknown-linux-gnu.tar"),
            None
        );
        assert_eq!(checksum_for(&file, ""), None);
        // Uppercase hex, wrong length, and trailing junk are all rejected.
        for bad in [
            &format!("{}  x.tar.gz\n", "A".repeat(64)),
            &format!("{}  x.tar.gz\n", "a".repeat(63)),
            &format!("{good}  x.tar.gz extra\n"),
            "",
        ] {
            assert_eq!(checksum_for(bad, "x.tar.gz"), None, "should reject {bad:?}");
        }
    }

    #[test]
    fn blockers_identify_homebrew_and_source_builds() {
        assert!(is_homebrew(Path::new(
            "/opt/homebrew/Cellar/agentstack/0.15.0/bin/agentstack"
        )));
        assert!(is_homebrew(Path::new(
            "/home/linuxbrew/.linuxbrew/Cellar/agentstack/0.15.0/bin/agentstack"
        )));
        assert!(!is_homebrew(Path::new("/usr/local/bin/agentstack")));

        assert!(is_source_build(Path::new(
            "/w/agentstack/target/release/agentstack"
        )));
        assert!(is_source_build(Path::new(
            "/w/agentstack/target/debug/agentstack"
        )));
        assert!(!is_source_build(Path::new("/usr/local/bin/agentstack")));
        assert!(!is_source_build(Path::new("/opt/release/agentstack")));
    }

    /// Each blocker explains itself with a command the user can actually run.
    #[test]
    fn every_blocker_names_a_next_step() {
        let exe = Path::new("/opt/homebrew/Cellar/agentstack/0.15.0/bin/agentstack");
        let r = release();
        assert!(Blocker::Homebrew
            .explain(exe, &r)
            .contains("brew upgrade agentstack"));
        assert!(Blocker::SourceBuild
            .explain(exe, &r)
            .contains("cargo build --release"));
        assert!(Blocker::NotWritable
            .explain(exe, &r)
            .contains("sudo agentstack self update"));
        assert!(Blocker::NoAsset.explain(exe, &r).contains("/releases"));
    }

    /// A binary sitting in a directory this user cannot write is the `sudo`
    /// case, and it is detected before anything is downloaded.
    #[cfg(unix)]
    #[test]
    fn an_unwritable_directory_blocks_before_any_download() {
        use std::os::unix::fs::PermissionsExt;
        if nix_running_as_root() {
            return; // root can write anywhere; the check is vacuous.
        }
        let tmp = assert_fs::TempDir::new().unwrap();
        let dir = tmp.child("readonly");
        std::fs::create_dir(dir.path()).unwrap();
        let exe = dir.path().join("agentstack");
        std::fs::write(&exe, "#!binary").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        assert_eq!(blocker(&exe), Some(Blocker::NotWritable));

        // Restore write bits so the temp dir can be cleaned up.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    fn nix_running_as_root() -> bool {
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
            .unwrap_or(false)
    }

    /// A run on the newest release changes nothing and says so — even with
    /// `--write`, and without ever asking for an asset.
    #[test]
    fn an_up_to_date_binary_downloads_nothing() {
        struct Latest(String);
        impl ReleaseSource for Latest {
            fn latest(&self) -> Result<Release> {
                Release::from_tag(&self.0).context("tag")
            }
            fn asset(&self, _t: &str, _n: &str) -> Result<Vec<u8>> {
                panic!("an up-to-date binary must not download anything")
            }
        }
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(update::NO_CHECK_ENV);
        std::env::remove_var(update::BASE_URL_ENV);
        let source = Latest(format!("v{}", update::current()));
        run_with(&SelfUpdateArgs { write: true }, &source).unwrap();
    }
}
