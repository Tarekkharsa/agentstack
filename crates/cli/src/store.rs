//! Content store: `~/.agentstack/store/` — where capability sources are fetched
//! and cached (PLAN §9d). Git sources are cloned/checked-out via the `git` CLI;
//! path sources pass through. A content digest gives the lockfile its integrity
//! field.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use agentstack_core::digest::Sha256Hex;

use crate::manifest::{Skill, SkillSource};
use crate::util::paths;

pub struct Store {
    root: PathBuf,
}

/// The most files an integrity-root pin will deposit. Matched to the re-gate
/// reader's own `MAX_TREE_FILES`: past this point that reader truncates the
/// walk, so a larger deposit could not be faithfully diffed even if it existed.
const MAX_DEPOSIT_FILES: usize = 500;

/// The most bytes an integrity-root pin will deposit, across the whole pinned
/// set. Generous for a real extension or workflow, and small enough that the
/// worst case a single pin can add to a never-evicted store stays bounded.
/// See [`Store::deposit_integrity_root`] for why a bigger source is skipped
/// rather than archived.
const MAX_DEPOSIT_BYTES: u64 = 8 * 1024 * 1024;

/// The on-disk path of one member of an integrity root. A single-file root
/// yields one EMPTY relative path (`integrity_root_files`' contract), and
/// joining `""` would append a trailing separator — so the root is read
/// directly in that case.
fn member_path(root: &Path, rel: &Path) -> PathBuf {
    if rel.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    }
}

/// Where that member lands inside the deposit. A directory root keeps its
/// relative path; a single-file root (empty relative path) is named by the
/// file, so the deposit is `content/<hex>/<file name>` — the same shape a
/// fragment deposit takes, which is what lets one re-gate reader serve both.
fn deposit_member_name(root: &Path, rel: &Path) -> PathBuf {
    if rel.as_os_str().is_empty() {
        PathBuf::from(root.file_name().unwrap_or(std::ffi::OsStr::new("body")))
    } else {
        rel.to_path_buf()
    }
}

/// The resolved local location of a skill's content.
#[derive(Debug)]
pub struct Resolved {
    pub path: PathBuf,
    /// Resolved git commit (git sources only).
    pub rev: Option<String>,
    pub checksum: String,
    /// Whether a network fetch happened this call.
    pub fetched: bool,
    pub source_kind: &'static str,
}

impl Store {
    pub fn default_store() -> Self {
        Store {
            root: paths::agentstack_home().join("store"),
        }
    }

    pub fn with_root(root: PathBuf) -> Self {
        Store { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a skill to a local directory, fetching git sources as needed.
    /// `pinned_rev` (from the lockfile) wins over the manifest's rev for
    /// reproducibility.
    pub fn resolve(
        &self,
        skill: &Skill,
        manifest_dir: &Path,
        pinned_rev: Option<&str>,
    ) -> Result<Resolved> {
        self.resolve_inner(skill, manifest_dir, pinned_rev, false)
    }

    /// The update/relock posture: ignore the lock pin, honor the manifest
    /// rev, and REQUIRE the fetch — a rev-less git skill re-tracks the
    /// remote's default-branch head, and an unreachable upstream errors
    /// instead of silently reusing the cached clone (see `ensure_git`).
    pub fn resolve_refresh(&self, skill: &Skill, manifest_dir: &Path) -> Result<Resolved> {
        self.resolve_inner(skill, manifest_dir, None, true)
    }

    fn resolve_inner(
        &self,
        skill: &Skill,
        manifest_dir: &Path,
        pinned_rev: Option<&str>,
        refresh: bool,
    ) -> Result<Resolved> {
        match skill.source()? {
            SkillSource::Path(p) => {
                let path = resolve_path(manifest_dir, &p);
                let checksum = if path.exists() {
                    // Same self-contained rule as git content: a symlink is
                    // excluded from the digest, so its bytes are never pinned.
                    crate::scan::reject_symlinks(&path)?;
                    dir_digest(&path)?.hex().to_string()
                } else {
                    String::new()
                };
                Ok(Resolved {
                    path,
                    rev: None,
                    checksum,
                    fetched: false,
                    source_kind: "path",
                })
            }
            SkillSource::Git { url, rev, subpath } => {
                let want = pinned_rev.map(str::to_string).or(rev);
                let clone = self.git_dir(&url);
                let fetched = ensure_git(&url, want.as_deref(), &clone, refresh)?;
                // HEAD is read from the clone root (`.git` lives there); the
                // skill body — and thus the checksum — is the subpath dir.
                let resolved_rev = git_head(&clone)?;
                let content = git_content_dir(&clone, subpath.as_deref())?;
                if let Some(sub) = subpath.as_deref() {
                    if !content.exists() {
                        bail!(
                            "subpath '{sub}' does not exist in {url} at {} — \
                             check the path within the repo",
                            &resolved_rev[..resolved_rev.len().min(12)]
                        );
                    }
                }
                // Reject symlinks before pinning or delivering the content
                // (findings: the digest excludes links, so a link's bytes are
                // never pinned, and following one escapes the checkout).
                crate::scan::reject_symlinks(&content)?;
                let checksum = dir_digest(&content)?.hex().to_string();
                // Immutable per-commit snapshot: the mutable clone's working
                // tree churns as other revisions of the same URL check out,
                // but a materialized symlink must point at bytes that never
                // change under it. Content-addressed by the digest, so a
                // different commit is a different dir — never a clobber.
                let path = self.snapshot_content(&content, &checksum)?;
                Ok(Resolved {
                    path,
                    rev: Some(resolved_rev),
                    checksum,
                    fetched,
                    source_kind: "git",
                })
            }
        }
    }

    /// **The pinning act.** Turn a resolved skill's checksum into the typed pin
    /// a lockfile entry carries — and, for a path source, deposit the bytes
    /// that pin covers into the content-addressed store as part of the same
    /// call.
    ///
    /// This exists so "the approved bytes were captured" is a property of
    /// PINNING rather than of call-site discipline. A lock entry cannot be
    /// built without a `Sha256Hex` pin, and the only way to get one from a
    /// `Resolved` is through here, so every path-sourced entry any code path
    /// writes has a corresponding store object. The alternative — a helper the
    /// known call sites remember to call — is the shape that produced two real
    /// bugs already this phase: a kind disclosed nowhere, and a lint satisfied
    /// by test code.
    ///
    /// Git sources are already deposited during `resolve` (their bytes must be
    /// stable under a materialized symlink), so this is a no-op for them.
    ///
    /// **Best-effort by design.** A failed deposit NEVER fails the pin: the
    /// lock write proceeds, and the only consequence is that a future re-gate
    /// shows the honest "no snapshot recorded" message instead of a diff. A
    /// consent improvement must not become a new way for `lock` to fail.
    pub fn pin(&self, resolved: &Resolved) -> Result<Sha256Hex> {
        let pin = Sha256Hex::parse(&resolved.checksum)?;
        if resolved.source_kind == "path" && resolved.path.exists() {
            // Re-hash before depositing: `resolved.checksum` was computed
            // earlier and the live project directory may have moved since. A
            // deposit under the wrong digest name would make a future diff
            // card show bytes the user never approved — worse than showing no
            // diff at all. (Reads are re-verified too, in `verified_snapshot`,
            // so this is the first of two independent guards.)
            let still = dir_digest(&resolved.path)
                .map(|d| d.hex().to_string())
                .unwrap_or_default();
            if still == resolved.checksum {
                // Errors are deliberately swallowed — see the failure posture
                // above. Nothing here may block the pin.
                let _ = self.snapshot_content(&resolved.path, &resolved.checksum);
            }
        }
        Ok(pin)
    }

    /// **The pinning act, for an instruction fragment.** Sibling of [`pin`],
    /// same contract: the checksum a lock entry needs is obtainable only by
    /// depositing the bytes it covers.
    ///
    /// Why this is a second function rather than a branch inside [`pin`]: the
    /// two kinds genuinely pin different things. A skill's checksum is a TREE
    /// digest over its directory (`dir_digest`); an instruction's is a plain
    /// SHA-256 over the file's raw bytes. Both shapes are load-bearing in the
    /// lockfile, so collapsing them into one function would mean one of the
    /// two kinds silently changing its pin format. What they DO share — the
    /// deposit, the re-hash guard, and the copy-never-link rule — is shared,
    /// in [`deposit_file`] and [`snapshot_content`] respectively.
    ///
    /// The deposited layout is `content/<hex>/<file name>`: a directory even
    /// for one file, so the diff renderer walks both kinds identically instead
    /// of growing a second code path on the read side.
    ///
    /// Best-effort, exactly like [`pin`]: a failed deposit never fails the pin.
    ///
    /// [`pin`]: Store::pin
    /// [`deposit_file`]: Store::deposit_file
    pub fn pin_instruction(&self, src: &Path) -> Result<Sha256Hex> {
        let bytes = fs::read(src).with_context(|| format!("reading {}", src.display()))?;
        let pin = Sha256Hex::of(&bytes);
        // The bytes just read ARE the bytes hashed, so unlike the skill path
        // there is no window to re-hash against — `bytes` is the single read.
        let _ = self.deposit_file(src, &bytes, pin.hex());
        Ok(pin)
    }

    /// **The pinning act, for a server definition.** Third sibling of [`pin`]
    /// and [`pin_instruction`], and the one that was missing: a server member's
    /// checksum has always been a plain SHA-256 over the definition text, but
    /// nothing deposited the bytes that checksum covers — so nothing could
    /// later serve the definition without re-reading the mutable source it came
    /// from. Depositing here is what lets the gateway resolve a package-carried
    /// server from the lock and the store alone
    /// (`docs/design/pinned-serving-and-library-drift.md` §"The rendered lane":
    /// serving pinned bytes is a property of *reading*, and every reader
    /// follows it).
    ///
    /// No new digest path: the returned digest is `Sha256Hex::of(text)`, byte
    /// for byte the same value `resolve_server` computes for an inline table
    /// and `pin_package_member` computed before this function existed. Only the
    /// deposit is new.
    ///
    /// `name` names the deposited file for a human reading the store; it is not
    /// part of the address, so two members with identical definitions share one
    /// content directory under whichever name landed first. Readers take the
    /// single file they find rather than a name they expect
    /// ([`pinned_definition`]).
    ///
    /// Best-effort, exactly like its two siblings: a failed deposit never fails
    /// the pin. The cost of a missing deposit is a fail-closed refusal at serve
    /// time naming `agentstack lock`, never a wrong answer.
    ///
    /// [`pin`]: Store::pin
    /// [`pin_instruction`]: Store::pin_instruction
    /// [`pinned_definition`]: Store::pinned_definition
    pub fn pin_server_definition(&self, name: &str, text: &str) -> Sha256Hex {
        let pin = Sha256Hex::of(text.as_bytes());
        let _ = self.deposit_bytes(
            &std::ffi::OsString::from(format!("{name}.toml")),
            text.as_bytes(),
            pin.hex(),
        );
        pin
    }

    /// **The pinning act, for an integrity-root source** — a native extension
    /// or a governed workflow. Fourth sibling of [`pin`], [`pin_instruction`]
    /// and [`pin_server_definition`], and the pair that deposited nothing at
    /// all: their checksums were digested straight into the lockfile, so a
    /// re-gate on the two EXECUTABLE kinds could show that the bytes moved but
    /// never which lines moved — the human was asked to review a change with
    /// only one side of it on disk.
    ///
    /// Why this is a fourth function rather than a branch inside [`pin`]: the
    /// digest is a third family. Extensions and workflows pin the STRICT
    /// integrity-root digest (`.git` included, a symlink anywhere is a hard
    /// error, its own domain separator), never the lenient `dir_digest` a skill
    /// pins. Collapsing them would silently change one kind's pin format. What
    /// they share — the content-addressed address, the copy-never-link rule,
    /// the re-proof of the name after the write — is shared, in
    /// [`deposit_integrity_root`].
    ///
    /// `resolved_hex` is the digest the resolver already computed from
    /// `(anchor, declared)`; this returns exactly that value parsed, so no pin
    /// changes meaning. Only the deposit is new — and, exactly like [`pin`], it
    /// is BEST-EFFORT: a failed or refused deposit never fails the pin, and the
    /// only consequence is that a re-gate shows the honest "the bytes you
    /// approved were not recorded" line instead of a diff. That is also the
    /// backward-compatibility story: a lockfile written before this existed
    /// carries pins with no deposit, and those degrade to precisely today's
    /// behaviour rather than failing a project that upgraded mid-flight.
    ///
    /// [`pin`]: Store::pin
    /// [`pin_instruction`]: Store::pin_instruction
    /// [`pin_server_definition`]: Store::pin_server_definition
    /// [`deposit_integrity_root`]: Store::deposit_integrity_root
    pub fn pin_integrity_root(
        &self,
        anchor: &Path,
        declared: &str,
        resolved_hex: &str,
    ) -> Result<Sha256Hex> {
        let pin = Sha256Hex::parse(resolved_hex)?;
        // Errors are deliberately swallowed — see the failure posture above.
        // Nothing here may block the pin.
        let _ = self.deposit_integrity_root(anchor, declared, pin.hex());
        Ok(pin)
    }

    /// **The pinning act, for a workflow's approved blueprint.** The blueprint
    /// is a single JSON file digested by `contained_file_digest`, which is the
    /// SAME raw-SHA-256-over-the-bytes family instruction fragments pin — so
    /// this is [`pin_instruction`] with the containment rules in front of it,
    /// and it needs no new store machinery at all. The deposited layout is the
    /// instruction family's `content/<hex>/<file name>`, which
    /// [`verified_content`] and the re-gate reader already understand.
    ///
    /// The digest is computed by `contained_file_digest` and nothing else, so
    /// the refusals a declared blueprint can produce (escape, traversal, a
    /// symlink anywhere on the path, not a regular file) are byte-for-byte the
    /// ones this path produced before.
    ///
    /// The re-read is guarded the way [`pin`]'s is: the deposit happens only
    /// while the bytes still hash to the digest just taken. A file that moved
    /// between the two reads deposits nothing rather than landing foreign bytes
    /// under an approved name — a wrong diff is worse than no diff.
    ///
    /// Best-effort, like every sibling: a failed deposit never fails the pin.
    ///
    /// [`pin`]: Store::pin
    /// [`pin_instruction`]: Store::pin_instruction
    /// [`verified_content`]: verified_content
    pub fn pin_blueprint(&self, anchor: &Path, declared: &str) -> Result<Sha256Hex> {
        let pin = agentstack_core::digest::contained_file_digest(anchor, declared)?;
        if let Ok(path) = agentstack_core::digest::resolve_contained(anchor, declared) {
            if let Ok(bytes) = fs::read(&path) {
                if Sha256Hex::of(&bytes).hex() == pin.hex() {
                    let _ = self.deposit_file(&path, &bytes, pin.hex());
                }
            }
        }
        Ok(pin)
    }

    /// **The serving act, for a pinned single-file definition.** The text a
    /// pinned server definition must be READ from: the content-addressed
    /// deposit named by `digest_hex`, never the manifest, library or package
    /// body it was originally read out of.
    ///
    /// The address is re-proven on every read. The store is a writable
    /// directory, so a read that trusted the path name instead of the bytes
    /// would be trusting the filesystem rather than the digest — the same
    /// reasoning [`verified_snapshot`] exists for, applied to the one-file pin
    /// family. Anything that does not verify is an error, and callers turn that
    /// into a refusal naming the capability; there is deliberately no fallback
    /// to a live source, which is exactly the mutable thing the pin exists to
    /// stop reading.
    pub fn pinned_definition(&self, digest_hex: &str) -> Result<String> {
        let dir = self.root.join("content").join(digest_hex);
        // Rejects a symlink at the address (or anywhere under it) and errors
        // when nothing is there at all — both are "no verified deposit".
        crate::scan::reject_symlinks(&dir).with_context(|| {
            format!(
                "no verified copy of the pinned definition at {}",
                dir.display()
            )
        })?;
        let mut files: Vec<PathBuf> = fs::read_dir(&dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .map(|e| e.path())
            .collect();
        files.sort();
        let [only] = files.as_slice() else {
            bail!(
                "the stored copy at {} is not a single-file definition deposit",
                dir.display()
            );
        };
        let bytes = fs::read(only).with_context(|| format!("reading {}", only.display()))?;
        if Sha256Hex::of(&bytes).hex() != digest_hex {
            bail!(
                "the stored copy at {} is present but does not hash to {digest_hex}",
                dir.display()
            );
        }
        String::from_utf8(bytes)
            .with_context(|| format!("the pinned definition at {} is not UTF-8", dir.display()))
    }

    /// Place one file's bytes at `content/<hex>/<file name>`, write-once.
    ///
    /// The bytes are passed in rather than re-read, so what is deposited is
    /// exactly what was hashed — the file cannot change between the two.
    /// Crash-safe via temp-then-rename, and a copy rather than a link, for the
    /// same reason the skill snapshot is: the delivered/compared artifact must
    /// never track later edits to the project file.
    fn deposit_file(&self, src: &Path, bytes: &[u8], digest_hex: &str) -> Result<()> {
        let name = src
            .file_name()
            .map(std::ffi::OsStr::to_os_string)
            .unwrap_or_else(|| std::ffi::OsString::from("fragment"));
        self.deposit_bytes(&name, bytes, digest_hex)
    }

    /// [`deposit_file`] for bytes that were never a file: the file name is
    /// chosen by the caller instead of taken from a source path. Split out so
    /// the on-disk layout, the write-once rule and the crash-safe rename live
    /// in one place rather than being restated per pin family.
    ///
    /// [`deposit_file`]: Store::deposit_file
    fn deposit_bytes(&self, name: &std::ffi::OsStr, bytes: &[u8], digest_hex: &str) -> Result<()> {
        let content_root = self.root.join("content");
        let dest = content_root.join(digest_hex);
        if dest.join(name).is_file() {
            return Ok(()); // already deposited, content-addressed
        }
        fs::create_dir_all(&content_root)
            .with_context(|| format!("creating {}", content_root.display()))?;
        let tmp = content_root.join(format!(".tmp-{}", crate::runs::gen_id()));
        fs::create_dir_all(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        fs::write(tmp.join(name), bytes)?;
        if fs::rename(&tmp, &dest).is_err() {
            let _ = fs::remove_dir_all(&tmp);
            if !dest.join(name).is_file() {
                bail!("could not place the content snapshot at {}", dest.display());
            }
        }
        Ok(())
    }

    /// Place an integrity-root source's pinned byte set at `content/<hex>/`,
    /// write-once — the deposit behind [`pin_integrity_root`].
    ///
    /// **What is deposited, and why not everything.** The bytes themselves, in
    /// the store's existing shape, so the re-gate reader, `verified_content`
    /// and every other content-addressed reader work unchanged — but only while
    /// the pinned set stays under [`MAX_DEPOSIT_FILES`] / [`MAX_DEPOSIT_BYTES`].
    /// A skill or a fragment is a small body; an extension can be a whole tree,
    /// and the store is never evicted, so every re-lock of a large one would add
    /// another permanent full copy. Above the ceiling that copy buys the
    /// reviewer nothing: the re-gate reader already caps what it will read
    /// (500 files, 2 MiB per file) and the card caps what it will show
    /// (`regate::DIFF_LINE_CAP`), so an oversized source renders as "too large
    /// to show here" whether or not it was deposited. The ceiling is set to
    /// match that reader, and exceeding it degrades to the same honest
    /// no-snapshot message a never-captured pin gets. A canonical archive was
    /// the alternative considered and rejected: it saves no bytes and would add
    /// a second format and a second reader to a store whose whole contract is
    /// "a directory that re-hashes to its own name".
    ///
    /// The byte set comes from `integrity_root_files` — the SAME strict walk
    /// the digest took, so `.git` is included and a symlink is a hard error
    /// rather than a silent skip. `copy_dir_all` is deliberately NOT used here:
    /// it strips `.git` and dereferences links, either of which would deposit
    /// bytes that are not the pinned bytes.
    ///
    /// Layout: a directory root lands verbatim under its relative paths; a
    /// single-file root lands as `content/<hex>/<file name>`, the same
    /// one-file-in-a-directory shape [`deposit_file`] uses, so the re-gate's
    /// reader lines both sides up without a second code path.
    ///
    /// [`pin_integrity_root`]: Store::pin_integrity_root
    /// [`deposit_file`]: Store::deposit_file
    fn deposit_integrity_root(
        &self,
        anchor: &Path,
        declared: &str,
        digest_hex: &str,
    ) -> Result<()> {
        let content_root = self.root.join("content");
        let dest = content_root.join(digest_hex);
        if verified_content(&dest, digest_hex) {
            return Ok(()); // already deposited, content-addressed
        }
        let (root, files) = agentstack_core::digest::integrity_root_files(anchor, declared)?;
        if files.len() > MAX_DEPOSIT_FILES {
            bail!(
                "{} holds {} files — past the deposit ceiling of {MAX_DEPOSIT_FILES}",
                root.display(),
                files.len()
            );
        }
        // Measured from metadata, before a single byte is copied: the point of
        // the ceiling is to not WRITE (and keep forever) what no card can show.
        let mut total: u64 = 0;
        for rel in &files {
            let from = member_path(&root, rel);
            total = total.saturating_add(fs::symlink_metadata(&from)?.len());
            if total > MAX_DEPOSIT_BYTES {
                bail!(
                    "{} is larger than the deposit ceiling of {MAX_DEPOSIT_BYTES} bytes",
                    root.display()
                );
            }
        }
        fs::create_dir_all(&content_root)
            .with_context(|| format!("creating {}", content_root.display()))?;
        // A leftover under this name that failed verification is rebuilt.
        if dest.symlink_metadata().is_ok() {
            remove_any(&dest);
        }
        // Copy to a temp then rename, so a crash never leaves a partial tree
        // under a digest name (which would read as complete).
        let tmp = content_root.join(format!(".tmp-{}", crate::runs::gen_id()));
        fs::create_dir_all(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        let copy_all = || -> Result<()> {
            for rel in &files {
                let from = member_path(&root, rel);
                let to = tmp.join(deposit_member_name(&root, rel));
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("creating {}", parent.display()))?;
                }
                fs::copy(&from, &to)
                    .with_context(|| format!("copying {} → {}", from.display(), to.display()))?;
            }
            Ok(())
        };
        if let Err(e) = copy_all() {
            let _ = fs::remove_dir_all(&tmp);
            return Err(e);
        }
        if fs::rename(&tmp, &dest).is_err() {
            let _ = fs::remove_dir_all(&tmp);
            // A concurrent writer may have placed a VALID deposit; accept it
            // only if it verifies.
            if !verified_content(&dest, digest_hex) {
                bail!("could not place the content snapshot at {}", dest.display());
            }
            return Ok(());
        }
        // F4: re-prove the address at the moment the name is claimed. The
        // digest was computed from a read that happened BEFORE this copy — if
        // the source moved in between, the copy holds mixed bytes that would
        // now sit under an approved digest name and read as approved forever.
        if !verified_content(&dest, digest_hex) {
            remove_any(&dest);
            bail!(
                "content changed while it was being snapshotted — {} does not hash to {digest_hex}",
                root.display()
            );
        }
        Ok(())
    }

    /// Copy `src` (a resolved, symlink-free skill body) into the immutable
    /// content-addressed cache `store/content/<digest>` and return that path.
    /// Write-once: if the digest dir already exists it is reused as-is, so
    /// two resolves of the same commit share one immutable dir and a later
    /// resolve of a *different* commit lands in a different dir — the bytes a
    /// materialized symlink points at can never change under it. Callers
    /// reject symlinks first, so `copy_dir_all` is faithful here.
    pub fn snapshot_content(&self, src: &Path, digest_hex: &str) -> Result<PathBuf> {
        let content_root = self.root.join("content");
        let dest = content_root.join(digest_hex);
        // Trust an existing snapshot ONLY if it still hashes to its own name.
        // The dir is writable and could have been corrupted, truncated by an
        // interrupted run, or replaced with a symlink — a bare `exists()`
        // would then materialize tampered bytes under a trusted digest.
        if verified_snapshot(&dest, digest_hex) {
            return Ok(dest);
        }
        fs::create_dir_all(&content_root)
            .with_context(|| format!("creating {}", content_root.display()))?;
        // A leftover under this name that failed verification is rebuilt.
        if dest.symlink_metadata().is_ok() {
            remove_any(&dest);
        }
        // Copy to a temp then rename into place, so a crash never leaves a
        // partial dir under a digest name (which would read as complete).
        let tmp = content_root.join(format!(".tmp-{}", crate::runs::gen_id()));
        crate::util::fsx::copy_dir_all(src, &tmp)
            .with_context(|| format!("snapshotting {}", src.display()))?;
        if fs::rename(&tmp, &dest).is_err() {
            let _ = fs::remove_dir_all(&tmp);
            // A concurrent writer may have placed a VALID snapshot; accept it
            // only if it verifies. Otherwise the rename failed for a real
            // reason (permissions, cross-device) — surface it, never return a
            // path we didn't successfully write.
            if !verified_snapshot(&dest, digest_hex) {
                bail!("could not place the content snapshot at {}", dest.display());
            }
            return Ok(dest);
        }
        // F4: verify what actually landed. `digest_hex` was computed from a
        // read of `src` that happened BEFORE the copy above — if the source
        // tree changed in between, the copy holds mixed bytes that would now
        // sit under an approved digest name and read as approved forever.
        // Content-addressing is only real if the address is re-proven at the
        // moment the name is claimed.
        if !verified_snapshot(&dest, digest_hex) {
            remove_any(&dest);
            bail!(
                "content changed while it was being snapshotted — {} does not hash to {digest_hex}",
                src.display()
            );
        }
        Ok(dest)
    }

    /// **The serving act.** The directory a pinned capability's bytes must be
    /// READ from: the content-addressed snapshot named by `digest_hex` — never
    /// the mutable directory `verified_live` was resolved from. This is the
    /// reproducibility rule of `docs/design/automatic-delivery.md`: runtime
    /// resolves from the project lock and serves the pinned bytes from the
    /// store by digest, so a central library can move arbitrarily far ahead
    /// without changing what any project serves.
    ///
    /// `verified_live` is used for one thing only — REPAIRING a snapshot that
    /// is absent — and that is not a hole in the rule. Callers reach here only
    /// after proving those live bytes hash to `digest_hex`, so the deposit
    /// stores the pinned-and-reviewed bytes and nothing else; and
    /// [`snapshot_content`] re-proves the address at the moment it is claimed,
    /// so a source that moved in between fails the deposit instead of landing
    /// under an approved name. The repair exists because [`pin`]'s deposit is
    /// best-effort by design: a store that never received it, or that was
    /// pruned, must self-heal rather than refuse a load the user consented to.
    ///
    /// Present-but-unverifiable is deliberately NOT repaired. Something
    /// already occupies this address and does not hash to it — a tampered,
    /// truncated, or symlinked store — which is a signal, not a gap to fill,
    /// so it errors and the caller turns that into a refusal naming the
    /// capability.
    ///
    /// [`pin`]: Store::pin
    /// [`snapshot_content`]: Store::snapshot_content
    pub fn pinned_content(&self, digest_hex: &str, verified_live: &Path) -> Result<PathBuf> {
        let dest = self.root.join("content").join(digest_hex);
        // `symlink_metadata`, not `exists()`: a symlink at the address counts
        // as occupied and must refuse rather than be followed or replaced.
        if dest.symlink_metadata().is_ok() {
            if verified_snapshot(&dest, digest_hex) {
                return Ok(dest);
            }
            bail!(
                "the stored copy at {} is present but does not hash to {digest_hex}",
                dest.display()
            );
        }
        self.snapshot_content(verified_live, digest_hex)
    }

    /// Is the pinned snapshot for `digest_hex` ALREADY in the store and still
    /// hashing to its own name? A pure question, with no repair: the answer is
    /// what a caller needs when it holds no verified live copy to repair from
    /// (the library-moved-ahead path in `mcp_server`, where the live directory
    /// is exactly the thing that no longer matches the pin).
    ///
    /// Kept beside [`pinned_content`] rather than folded into it because the
    /// two differ in one load-bearing way: `pinned_content` may deposit, this
    /// only observes. A caller with nothing trustworthy to deposit must be
    /// able to say so in the type of the call it makes.
    ///
    /// [`pinned_content`]: Store::pinned_content
    pub fn has_pinned_content(&self, digest_hex: &str) -> bool {
        verified_snapshot(&self.root.join("content").join(digest_hex), digest_hex)
    }

    /// Resolve a skill to a local directory **without any network access**.
    /// Path sources resolve as usual. Git sources resolve to an immutable
    /// worktree for the pinned/declared commit *only if its clone already
    /// exists*; an un-cached git source yields `Ok(None)` so callers can report
    /// it as unavailable offline rather than fetching.
    pub fn resolve_local(
        &self,
        skill: &Skill,
        manifest_dir: &Path,
        pinned_rev: Option<&str>,
    ) -> Result<Option<Resolved>> {
        match skill.source()? {
            SkillSource::Path(p) => {
                let path = resolve_path(manifest_dir, &p);
                let checksum = if path.exists() {
                    crate::scan::reject_symlinks(&path)?;
                    dir_digest(&path)?.hex().to_string()
                } else {
                    String::new()
                };
                Ok(Some(Resolved {
                    path,
                    rev: None,
                    checksum,
                    fetched: false,
                    source_kind: "path",
                }))
            }
            SkillSource::Git { url, rev, subpath } => {
                // Read the INTENDED commit's immutable worktree, never the
                // shared clone's working tree — another resolve may have
                // checked out a different revision there, which would make
                // this skill falsely drift or load the wrong bytes.
                let want = pinned_rev.or(rev.as_deref());
                let Some((content, commit)) =
                    self.git_worktree_content(&url, want, subpath.as_deref())?
                else {
                    return Ok(None);
                };
                Ok(Some(Resolved {
                    checksum: dir_digest(&content)?.hex().to_string(),
                    path: content,
                    rev: Some(commit),
                    fetched: false,
                    source_kind: "git",
                }))
            }
        }
    }

    /// The intended commit's content, from an immutable per-commit detached
    /// worktree (`store/co/<url>/<commit>`) — no network, no shared working
    /// tree. Returns `(content_dir, commit)`, or `None` when the clone or the
    /// commit isn't available offline. The commit is resolved locally: a
    /// pinned rev wins; an unpinned skill uses the fetched default branch
    /// (`origin/HEAD`), which only moves on fetch — never when another
    /// revision is checked out.
    pub(crate) fn git_worktree_content(
        &self,
        url: &str,
        rev: Option<&str>,
        subpath: Option<&str>,
    ) -> Result<Option<(PathBuf, String)>> {
        let clone = self.git_dir(url);
        if !clone.exists() {
            return Ok(None);
        }
        // Prefer a fetched remote-tracking branch for a symbolic rev, then
        // fall back to the exact local tag/commit spelling. An authoritative
        // lock pin is a commit id, so the first probe harmlessly misses and
        // the second resolves it exactly.
        let specs = match rev {
            Some(r) => vec![format!("origin/{r}^{{commit}}"), format!("{r}^{{commit}}")],
            None => vec!["origin/HEAD^{commit}".to_string()],
        };
        let commit = specs.into_iter().find_map(|spec| {
            crate::gitx::run(
                crate::gitx::Profile::Ingest,
                &["rev-parse", "--verify", "--quiet", &spec],
                Some(&clone),
            )
            .ok()
            .filter(|commit| !commit.is_empty())
        });
        let Some(commit) = commit else {
            return Ok(None);
        };
        let Some(co) = self.ensure_worktree(&clone, &commit)? else {
            return Ok(None);
        };
        let content = git_content_dir(&co, subpath)?;
        if !content.exists() {
            return Ok(None);
        }
        // This is the common boundary for resolve_local, resolve_path_only,
        // and local_source_dir: none may expose a worktree containing links.
        crate::scan::reject_symlinks(&content)?;
        Ok(Some((content, commit)))
    }

    /// Ensure an immutable detached worktree of `clone` at `commit` under
    /// `store/co/<url>/<commit>`. Idempotent, no network (the commit's
    /// objects are already local). `None` if the commit can't be checked out
    /// offline. Different commits get different dirs, so a materialized
    /// symlink or an offline read is never churned by another revision.
    fn ensure_worktree(&self, clone: &Path, commit: &str) -> Result<Option<PathBuf>> {
        let co = self
            .root
            .join("co")
            .join(sanitize(&clone.to_string_lossy()))
            .join(commit);
        // A worktree carries a `.git` gitlink file; a complete one is reused.
        if co.join(".git").is_file() {
            return Ok(Some(co));
        }
        if let Some(parent) = co.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        // Clear any stale admin entry (a dir removed out from under git) and a
        // partial leftover, then add fresh.
        let _ = crate::gitx::run(
            crate::gitx::Profile::Ingest,
            &["worktree", "prune"],
            Some(clone),
        );
        if co.exists() {
            fs::remove_dir_all(&co).ok();
        }
        match crate::gitx::run(
            crate::gitx::Profile::Ingest,
            &[
                "worktree",
                "add",
                "--detach",
                "--force",
                &co.to_string_lossy(),
                commit,
            ],
            Some(clone),
        ) {
            Ok(_) => Ok(Some(co)),
            Err(_) => Ok(None), // commit not present offline
        }
    }

    /// Locate a skill's directory without network access **or content
    /// digesting** — for read-only callers that only need the path (reading
    /// `SKILL.md`, listing). `checksum` is left empty, so the result must never
    /// feed lock recording; digesting is what makes small ops pay a whole-
    /// library read+hash. Un-cached git sources yield `Ok(None)`, like
    /// [`Store::resolve_local`].
    pub fn resolve_path_only(
        &self,
        skill: &Skill,
        manifest_dir: &Path,
        pinned_rev: Option<&str>,
    ) -> Result<Option<Resolved>> {
        match skill.source()? {
            SkillSource::Path(p) => {
                let path = resolve_path(manifest_dir, &p);
                if path.exists() {
                    crate::scan::reject_symlinks(&path)?;
                }
                Ok(Some(Resolved {
                    path,
                    rev: None,
                    checksum: String::new(),
                    fetched: false,
                    source_kind: "path",
                }))
            }
            SkillSource::Git { url, rev, subpath } => {
                // Same immutable-worktree read as `resolve_local`, minus the
                // digest — display/listing callers only need the path.
                let want = pinned_rev.or(rev.as_deref());
                match self.git_worktree_content(&url, want, subpath.as_deref())? {
                    Some((content, _)) => Ok(Some(Resolved {
                        path: content,
                        rev: None,
                        checksum: String::new(),
                        fetched: false,
                        source_kind: "git",
                    })),
                    None => Ok(None),
                }
            }
        }
    }

    fn git_dir(&self, url: &str) -> PathBuf {
        self.root.join("git").join(sanitize(url))
    }

    /// Adopt a staged clone into this store's slot for `url` — only if the
    /// slot is empty, and **rename-only**: staging (see [`Stage`]) lives on
    /// this filesystem by construction, so the scanned bytes land verbatim
    /// (`.git` and symlinks included). There is deliberately no copy
    /// fallback — the shipped copy helpers strip `.git` and dereference
    /// symlinks, either of which corrupts a promoted clone (design §3 of
    /// normalized source grammar). `Ok(None)` = slot taken or rename
    /// refused; the caller falls back to a commit-pinned re-resolve.
    pub fn adopt_clone(&self, url: &str, staged_clone: &Path) -> Result<Option<PathBuf>> {
        let dest = self.git_dir(url);
        if dest.exists() {
            return Ok(None);
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        Ok(fs::rename(staged_clone, &dest).ok().map(|()| dest))
    }

    /// The cached clone **root** for `url` with no network access, plus its
    /// current HEAD — `None` when the clone does not exist yet (report it as
    /// unavailable offline). Unlike [`Store::resolve_local`], this returns the
    /// clone root (where `.git` lives), not the subpath content dir: git
    /// extensions digest the checkout root anchored at a `subpath` with the
    /// strict integrity-root digest, so they need the root, not the body dir.
    pub fn local_git_clone(&self, url: &str) -> Option<(PathBuf, Option<String>)> {
        let clone = self.git_dir(url);
        if !clone.exists() {
            return None;
        }
        Some((clone.clone(), git_head(&clone).ok()))
    }
}

/// Transient staging for previewing remote sources without touching the
/// persistent store. Lives under
/// the agentstack home — the store's own filesystem by construction, so
/// promotion is a rename, never a copy. Random id (never reused: a crashed
/// run's leftovers must not skip re-fetch/re-scan), 0700, best-effort
/// removal on drop — the `SandboxGateway` RAII pattern.
pub struct Stage {
    root: PathBuf,
}

impl Stage {
    pub fn create() -> Result<Self> {
        let root = paths::agentstack_home()
            .join("stage")
            .join(crate::runs::gen_id());
        if root.exists() {
            bail!(
                "staging path {} already exists — retry the command",
                root.display()
            );
        }
        fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
        crate::util::restrict(&root, true);
        Ok(Self { root })
    }

    /// A `Store` rooted at this staging area: clones land under
    /// `<stage>/git/<sanitized-url>` with zero writes anywhere persistent.
    pub fn store(&self) -> Store {
        Store::with_root(self.root.clone())
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Resolve a skill's local source dir for materialization, *without* fetching
/// (path → local; git → store dir if already installed, else `None`).
pub fn local_source_dir(
    store: &Store,
    skill: &Skill,
    manifest_dir: &Path,
    pinned_rev: Option<&str>,
) -> Option<PathBuf> {
    match skill.source().ok()? {
        SkillSource::Path(p) => {
            let path = resolve_path(manifest_dir, &p);
            (path.exists() && crate::scan::reject_symlinks(&path).is_ok()).then_some(path)
        }
        SkillSource::Git {
            url, rev, subpath, ..
        } => {
            // Immutable per-commit worktree, not the churnable clone tree.
            store
                .git_worktree_content(&url, pinned_rev.or(rev.as_deref()), subpath.as_deref())
                .ok()?
                .map(|(content, _)| content)
        }
    }
}

/// Resolve a git skill's content directory: the clone root, or a validated
/// subdirectory within it. The subpath must be a plain relative path — no
/// absolute prefix and no `..` component — so a crafted library entry can never
/// point the skill body outside its own clone.
fn git_content_dir(clone: &Path, subpath: Option<&str>) -> Result<PathBuf> {
    let Some(sub) = subpath.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(clone.to_path_buf());
    };
    let rel = Path::new(sub);
    let safe = rel
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    if !safe {
        bail!("git subpath '{sub}' must be a relative path inside the repo (no '..' or absolute path)");
    }
    let content = clone.join(rel);
    // A `Normal`-only subpath still escapes if a component is a *symlink* the
    // repo shipped (e.g. `skills/x` → `~/.ssh`; git checks out symlinks). When
    // the target exists, resolve it and require it stay inside the clone —
    // otherwise dir_digest/scan/copy would read and vendor files outside the repo.
    if let (Ok(real_content), Ok(real_clone)) =
        (fs::canonicalize(&content), fs::canonicalize(clone))
    {
        if !real_content.starts_with(&real_clone) {
            bail!("git subpath '{sub}' resolves outside the repo (symlinked escape) — refusing");
        }
    }
    Ok(content)
}

fn resolve_path(dir: &Path, p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else {
        dir.join(pb)
    }
}

/// Clone (or refresh) `url` into the store and check out `rev` when given.
/// Returns the checkout dir and the resolved HEAD commit. The public seam the
/// git-pack provider uses; skills keep going through [`Store::resolve`].
pub fn checkout(store: &Store, url: &str, rev: Option<&str>) -> Result<(PathBuf, String)> {
    let dest = store.git_dir(url);
    ensure_git(url, rev, &dest, false)?;
    let head = git_head(&dest)?;
    Ok((dest, head))
}

/// List `url`'s tags via `git ls-remote --tags`, peeled entries preferred,
/// without cloning. Network; callers gate on policy first.
pub fn ls_remote_tags(url: &str) -> Result<Vec<String>> {
    crate::gitx::deny_weird_transport(url)?;
    let out = run_git(&["ls-remote", "--tags", url], None)?;
    let mut tags: Vec<String> = out
        .lines()
        .filter_map(|l| l.split_once("refs/tags/").map(|(_, t)| t))
        .map(|t| t.trim_end_matches("^{}").to_string())
        .collect();
    tags.sort();
    tags.dedup();
    Ok(tags)
}

/// Ensure a git clone exists at `dest` and is checked out at `want_rev` (or
/// its default branch). `refresh` is the update/relock posture: fetching is
/// REQUIRED — an unreachable or deleted upstream must surface, detecting
/// that is what update exists for — and a rev-less skill re-tracks the
/// remote's current default-branch head. Without that, `lock --update` on a
/// rev-less git skill with a cached clone made no network call at all: a
/// silent no-op that could neither update nor notice a vanished upstream.
fn ensure_git(url: &str, want_rev: Option<&str>, dest: &Path, refresh: bool) -> Result<bool> {
    crate::gitx::deny_weird_transport(url)?;
    let fresh = !dest.exists();
    if fresh {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        run_git(&["clone", url, &dest.to_string_lossy()], None)
            .with_context(|| format!("cloning {url}"))?;
    }
    match want_rev {
        Some(rev) => {
            if refresh {
                run_git(&["fetch", "--all", "--tags"], Some(dest))
                    .with_context(|| format!("fetching {url}"))?;
                // A branch pin must adopt the FETCHED head: `checkout
                // <branch>` lands on the stale LOCAL branch, which fetch
                // never fast-forwards (only remote-tracking refs moved).
                // Try the remote-tracking ref first; tags and commit shas
                // have no origin/ form and fall back to the plain checkout.
                let remote_ref = format!("origin/{rev}");
                if run_git(&["checkout", "--detach", &remote_ref], Some(dest)).is_err() {
                    run_git(&["checkout", rev], Some(dest))
                        .with_context(|| format!("checking out {rev}"))?;
                }
            } else {
                // Best-effort fetch so a pinned rev that arrived later is
                // available; a resolve against a cached clone stays offline-
                // tolerant.
                let _ = run_git(&["fetch", "--all", "--tags"], Some(dest));
                run_git(&["checkout", rev], Some(dest))
                    .with_context(|| format!("checking out {rev}"))?;
            }
        }
        None if refresh && !fresh => {
            run_git(&["fetch", "origin", "HEAD", "--tags"], Some(dest))
                .with_context(|| format!("fetching {url}"))?;
            run_git(&["checkout", "--detach", "FETCH_HEAD"], Some(dest))
                .with_context(|| format!("checking out the latest revision of {url}"))?;
        }
        None => {}
    }
    Ok(fresh)
}

/// The clone-containment guard, exposed for callers that hold a checkout
/// root directly (the add/lib source-grammar paths): resolves `subpath`
/// inside `clone_root` and refuses a checked-out symlink that escapes it —
/// the same refusal `Store::resolve` applies, so a hostile repo can't get a
/// preview's digest or scan to read files outside the repo.
pub fn contained_content_dir(clone_root: &Path, subpath: Option<&str>) -> Result<PathBuf> {
    git_content_dir(clone_root, subpath)
}

/// Whether `dest` is a real directory (not a symlink) whose content digest
/// equals `digest_hex` — the only condition under which a cached snapshot is
/// trusted without rebuilding, and (F4) the only condition under which
/// keep-pinned delivery may serve it: the store directory is writable, so
/// "the approved bytes are what agents load" holds only if the read re-proves
/// it. `pub(crate)` for exactly those read-side callers; the write side stays
/// in this module.
pub(crate) fn verified_snapshot(dest: &Path, digest_hex: &str) -> bool {
    match dest.symlink_metadata() {
        Ok(m) if m.file_type().is_dir() => {
            crate::scan::reject_symlinks(dest).is_ok()
                && dir_digest(dest)
                    .map(|d| d.hex() == digest_hex)
                    .unwrap_or(false)
        }
        _ => false,
    }
}

/// [`verified_snapshot`] for readers that cannot know which pin family a hex
/// digest belongs to. Skill pins are tree digests (`dir_digest`); instruction
/// and blueprint pins are a plain SHA-256 over one file's bytes, deposited as
/// `content/<hex>/<file name>`; extension and workflow pins are the strict
/// integrity-root digest, deposited by
/// [`Store::deposit_integrity_root`]. A snapshot verifies if it matches its
/// name under ANY family; symlinks anywhere disqualify under all of them.
///
/// Accepting any family is safe and is not a widening: every family re-derives
/// the digest from the bytes actually on disk, so a pass means "these bytes
/// hash to this address" no matter which one answered. The families carry
/// distinct domain separators precisely so they cannot be confused for each
/// other. Used by the re-gate diff reader (F4/F19): a diff rendered from a
/// tampered snapshot would present bytes the user never approved as "the
/// approved version", which corrupts the consent surface itself — the honest
/// degrade is `NoSnapshot`.
pub(crate) fn verified_content(dest: &Path, digest_hex: &str) -> bool {
    if verified_snapshot(dest, digest_hex) {
        return true;
    }
    if verified_integrity_root(dest, digest_hex) {
        return true;
    }
    // Instruction family: exactly one regular file whose raw bytes hash to
    // the digest. (`verified_snapshot` above already rejected symlinks — but
    // it only ran its hash check on the tree family, so re-check the shape
    // here from scratch rather than assuming its partial pass.)
    match dest.symlink_metadata() {
        Ok(m) if m.file_type().is_dir() => {
            if crate::scan::reject_symlinks(dest).is_err() {
                return false;
            }
            let Ok(entries) = fs::read_dir(dest) else {
                return false;
            };
            let files: Vec<_> = entries.flatten().collect();
            let [only] = files.as_slice() else {
                return false;
            };
            if !only.file_type().map(|t| t.is_file()).unwrap_or(false) {
                return false;
            }
            fs::read(only.path())
                .map(|b| Sha256Hex::of(&b).hex() == digest_hex)
                .unwrap_or(false)
        }
        _ => false,
    }
}

/// [`verified_snapshot`] for the integrity-root family (extensions, governed
/// workflows). Two framings, because the family has two shapes and they are
/// deliberately non-colliding: a DIRECTORY root frames each member's relative
/// path, while a SINGLE-FILE root frames the empty relative path (see
/// `integrity_root_digest`). The deposit stores both shapes as a directory, so
/// this re-derives the digest under each framing and accepts either — a
/// one-file directory root and a single-file root look identical on disk but
/// hash differently, and only the bytes present are ever hashed.
///
/// `integrity_root_digest` is used rather than a local walk so there is exactly
/// one implementation of the framing, and so its refusals (a symlink anywhere,
/// a missing path) disqualify a deposit here for free.
fn verified_integrity_root(dest: &Path, digest_hex: &str) -> bool {
    use agentstack_core::digest::integrity_root_digest;
    let matches = |root: &Path, declared: &str| {
        integrity_root_digest(root, declared)
            .map(|d| d.hex() == digest_hex)
            .unwrap_or(false)
    };
    // Directory framing: the deposit dir walked from the content root.
    if let (Some(parent), Some(name)) = (dest.parent(), dest.file_name().and_then(|n| n.to_str())) {
        if matches(parent, name) {
            return true;
        }
    }
    // Single-file framing: the deposit holds exactly one regular file, which
    // IS the pinned root.
    let Ok(entries) = fs::read_dir(dest) else {
        return false;
    };
    let files: Vec<_> = entries.flatten().collect();
    let [only] = files.as_slice() else {
        return false;
    };
    if !only.file_type().map(|t| t.is_file()).unwrap_or(false) {
        return false;
    }
    match only.file_name().to_str() {
        Some(name) => matches(dest, name),
        None => false,
    }
}

/// Remove a path whether it's a dir, file, or symlink (best-effort).
fn remove_any(path: &Path) {
    if path.is_dir()
        && !path
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    {
        let _ = fs::remove_dir_all(path);
    } else {
        let _ = fs::remove_file(path);
    }
}

fn git_head(dest: &Path) -> Result<String> {
    run_git(&["rev-parse", "HEAD"], Some(dest))
}

/// All store git runs under the `Ingest` profile — this is remote content on
/// its way to the trust gate (design §B).
fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<String> {
    crate::gitx::run(crate::gitx::Profile::Ingest, args, cwd)
}

fn sanitize(url: &str) -> String {
    url.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Total size in bytes of a directory's files (`.git` excluded, like
/// [`dir_digest`]). Best-effort: unreadable entries count as zero.
pub fn dir_size(root: &Path) -> u64 {
    let mut files: Vec<PathBuf> = Vec::new();
    if collect_files(root, root, &mut files).is_err() {
        return 0;
    }
    files
        .iter()
        .filter_map(|rel| fs::metadata(root.join(rel)).ok())
        .map(|m| m.len())
        .sum()
}

// The digest itself (paths + bytes → sha256) lives in core with the lockfile
// types it feeds. Authoritative skill checksums call it directly — no
// stat-fingerprint cache sits on the verification path (see ARCHITECTURE.md).
// TODO(phase-1): shim — migrate callers to agentstack_core::digest and drop.
pub use agentstack_core::digest::{collect_files, dir_digest};

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    /// F4 WITNESS (FINDINGS.md): the address must be re-proven at the moment
    /// the name is claimed. `digest_hex` is computed from a read that happens
    /// BEFORE the copy — a source that changed in between (simulated here by
    /// passing a digest the bytes never hashed to) must not land under the
    /// approved name, and must not leave a mislabeled snapshot behind.
    #[test]
    fn snapshot_content_refuses_bytes_that_do_not_hash_to_the_name() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("src/SKILL.md").write_str("# real\n").unwrap();
        let store = Store::with_root(tmp.child("store").path().to_path_buf());
        let wrong = "a".repeat(64);

        let err = store.snapshot_content(&tmp.path().join("src"), &wrong);
        assert!(err.is_err(), "mislabeled bytes were snapshotted");
        assert!(
            !tmp.child("store")
                .path()
                .join("content")
                .join(&wrong)
                .exists(),
            "a failed snapshot left mislabeled bytes under the approved name"
        );

        // The honest digest still snapshots.
        let right = dir_digest(&tmp.path().join("src"))
            .unwrap()
            .hex()
            .to_string();
        let dest = store
            .snapshot_content(&tmp.path().join("src"), &right)
            .unwrap();
        assert!(verified_snapshot(&dest, &right));
    }

    /// F4 WITNESS: `verified_content` accepts both pin families only while
    /// the bytes still hash to the name, and rejects the tampered forms the
    /// bare `is_dir()` read used to serve: edited bytes, and a symlink body.
    #[test]
    fn verified_content_rejects_tampering_under_either_family() {
        let tmp = assert_fs::TempDir::new().unwrap();

        // Skill family: a tree under its dir_digest.
        tmp.child("tree/SKILL.md").write_str("# t\n").unwrap();
        let tree = tmp.path().join("tree");
        let tree_hex = dir_digest(&tree).unwrap().hex().to_string();
        assert!(verified_content(&tree, &tree_hex));

        // Instruction family: one file under its raw sha256.
        let frag_hex = Sha256Hex::of(b"be kind\n").hex().to_string();
        tmp.child(format!("frag-{frag_hex}/house.md"))
            .write_str("be kind\n")
            .unwrap();
        let frag = tmp.path().join(format!("frag-{frag_hex}"));
        assert!(verified_content(&frag, &frag_hex));

        // Tamper the tree: edited bytes no longer verify.
        tmp.child("tree/SKILL.md").write_str("# EVIL\n").unwrap();
        assert!(
            !verified_content(&tree, &tree_hex),
            "edited snapshot bytes still verified"
        );

        // Tamper the fragment: a symlink body never verifies, even when its
        // target's bytes would hash correctly — following it reads outside
        // the store.
        fs::remove_file(frag.join("house.md")).unwrap();
        tmp.child("outside.md").write_str("be kind\n").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("outside.md"), frag.join("house.md")).unwrap();
        assert!(
            !verified_content(&frag, &frag_hex),
            "a symlinked snapshot body verified"
        );
    }

    /// G21 WITNESS — an extension (a directory integrity root) deposits at pin
    /// time, and the re-gate can then produce the APPROVED side and show which
    /// lines moved. Before this, the extension checksum went straight into the
    /// lockfile with no store object, so a re-gate had a pin and nothing to
    /// diff it against.
    #[test]
    fn an_extension_deposits_at_pin_time_and_a_regate_can_diff_it() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("ext/index.ts")
            .write_str("export default function (pi) {} // v1\n")
            .unwrap();
        tmp.child("ext/package.json")
            .write_str("{\"name\":\"checkpoint\"}\n")
            .unwrap();
        let store = Store::with_root(tmp.child("store").path().to_path_buf());

        // The digest the resolver computes — the strict integrity-root family.
        let hex = agentstack_core::digest::integrity_root_digest(tmp.path(), "ext")
            .unwrap()
            .hex()
            .to_string();
        let pin = store.pin_integrity_root(tmp.path(), "ext", &hex).unwrap();
        assert_eq!(pin.hex(), hex, "the pin value must not change");

        // The approved bytes are on disk, under exactly the lockfile's key,
        // and they verify against their own name.
        let dest = store.root().join("content").join(&hex);
        assert_eq!(
            fs::read_to_string(dest.join("index.ts")).unwrap(),
            "export default function (pi) {} // v1\n"
        );
        assert!(dest.join("package.json").is_file(), "every member deposits");
        assert!(
            verified_content(&dest, &hex),
            "an integrity-root deposit must verify under its own family"
        );

        // Drift the live source; the re-gate now has both sides.
        tmp.child("ext/index.ts")
            .write_str("export default function (pi) {} // v2\n")
            .unwrap();
        assert_eq!(
            fs::read_to_string(dest.join("index.ts")).unwrap(),
            "export default function (pi) {} // v1\n",
            "the deposit tracked a later edit — it must be a copy, never a link"
        );
        let d = crate::regate::diff_against_pin(store.root(), &hex, &tmp.path().join("ext"));
        let rendered = crate::regate::render_lines(&d, crate::regate::DIFF_LINE_CAP).join("\n");
        assert!(rendered.contains("index.ts"), "{rendered}");
        assert!(
            rendered.contains("- export default function (pi) {} // v1"),
            "{rendered}"
        );
        assert!(
            rendered.contains("+ export default function (pi) {} // v2"),
            "{rendered}"
        );
    }

    /// G21 WITNESS — a workflow is usually ONE script, which is a single-file
    /// integrity root. Its deposit takes the `content/<hex>/<file name>` shape
    /// so the re-gate reader lines the stored directory up against the live
    /// FILE without a second code path, and the two framings of the
    /// integrity-root digest stay distinguishable.
    #[test]
    fn a_single_file_workflow_deposits_and_a_regate_can_diff_it() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("wf/pipeline.js")
            .write_str("exports.run = () => 1;\n")
            .unwrap();
        let store = Store::with_root(tmp.child("store").path().to_path_buf());

        let hex = agentstack_core::digest::integrity_root_digest(tmp.path(), "wf/pipeline.js")
            .unwrap()
            .hex()
            .to_string();
        let pin = store
            .pin_integrity_root(tmp.path(), "wf/pipeline.js", &hex)
            .unwrap();
        assert_eq!(pin.hex(), hex);

        let dest = store.root().join("content").join(&hex);
        assert_eq!(
            fs::read_to_string(dest.join("pipeline.js")).unwrap(),
            "exports.run = () => 1;\n",
            "a single-file root deposits under its own file name"
        );
        assert!(verified_content(&dest, &hex));

        tmp.child("wf/pipeline.js")
            .write_str("exports.run = () => 2;\n")
            .unwrap();
        let d =
            crate::regate::diff_against_pin(store.root(), &hex, &tmp.path().join("wf/pipeline.js"));
        let rendered = crate::regate::render_lines(&d, crate::regate::DIFF_LINE_CAP).join("\n");
        assert!(rendered.contains("pipeline.js"), "{rendered}");
        assert!(rendered.contains("- exports.run = () => 1;"), "{rendered}");
        assert!(rendered.contains("+ exports.run = () => 2;"), "{rendered}");
    }

    /// G21 WITNESS — a workflow's approved blueprint is the raw-SHA-256 family,
    /// identical to an instruction fragment, so it reuses that deposit shape
    /// exactly. The digest must stay byte-for-byte what `contained_file_digest`
    /// produced before this deposited anything.
    #[test]
    fn a_blueprint_deposits_without_changing_its_digest() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let before = "{\"pattern\":\"fan-out\",\"goal\":\"review\"}\n";
        tmp.child("wf/p.blueprint.json").write_str(before).unwrap();
        let store = Store::with_root(tmp.child("store").path().to_path_buf());

        let expected =
            agentstack_core::digest::contained_file_digest(tmp.path(), "wf/p.blueprint.json")
                .unwrap();
        let pin = store
            .pin_blueprint(tmp.path(), "wf/p.blueprint.json")
            .unwrap();
        assert_eq!(pin, expected, "the blueprint pin value must not change");

        let dest = store.root().join("content").join(pin.hex());
        assert_eq!(
            fs::read_to_string(dest.join("p.blueprint.json")).unwrap(),
            before
        );
        assert!(verified_content(&dest, pin.hex()));

        tmp.child("wf/p.blueprint.json")
            .write_str("{\"pattern\":\"fan-out\",\"goal\":\"ship\"}\n")
            .unwrap();
        let d = crate::regate::diff_against_pin(
            store.root(),
            pin.hex(),
            &tmp.path().join("wf/p.blueprint.json"),
        );
        let rendered = crate::regate::render_lines(&d, crate::regate::DIFF_LINE_CAP).join("\n");
        assert!(rendered.contains("review"), "{rendered}");
        assert!(rendered.contains("ship"), "{rendered}");

        // The containment rules in front of the digest are unchanged: an
        // escaping or symlinked blueprint is still a hard refusal, never a
        // silent deposit of foreign bytes.
        assert!(store.pin_blueprint(tmp.path(), "../outside.json").is_err());
    }

    /// G21 WITNESS, the size decision — a source past the deposit ceiling is
    /// NOT copied, the pin still succeeds, and the re-gate degrades to the same
    /// honest no-snapshot answer a never-captured pin gets. Storing bytes the
    /// card could never show is permanent cost for no review value.
    #[test]
    fn an_oversized_integrity_root_skips_the_deposit_without_failing_the_pin() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let big = vec![b'x'; (MAX_DEPOSIT_BYTES + 1) as usize];
        tmp.child("ext").create_dir_all().unwrap();
        fs::write(tmp.path().join("ext/blob.bin"), &big).unwrap();
        let store = Store::with_root(tmp.child("store").path().to_path_buf());

        let hex = agentstack_core::digest::integrity_root_digest(tmp.path(), "ext")
            .unwrap()
            .hex()
            .to_string();
        // The pin is never blocked by a refused deposit.
        let pin = store.pin_integrity_root(tmp.path(), "ext", &hex).unwrap();
        assert_eq!(pin.hex(), hex);
        assert!(
            !store.root().join("content").join(&hex).exists(),
            "an oversized source was deposited anyway"
        );
        // And the degrade is the honest one, not a fabricated diff.
        assert_eq!(
            crate::regate::diff_against_pin(store.root(), &hex, &tmp.path().join("ext")),
            crate::regate::PinDiff::NoSnapshot
        );
    }

    /// G21 WITNESS, backward compatibility — a pin written before deposits
    /// existed has no store object, and that must read as today's behaviour
    /// (the honest no-snapshot message) rather than an error. Nothing about the
    /// lockfile format changed, so this is the whole compatibility surface.
    #[test]
    fn an_older_pin_with_no_deposit_degrades_instead_of_failing() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("ext/index.ts").write_str("// v1\n").unwrap();
        let store = Store::with_root(tmp.child("store").path().to_path_buf());
        let hex = agentstack_core::digest::integrity_root_digest(tmp.path(), "ext")
            .unwrap()
            .hex()
            .to_string();
        // No pin call at all: exactly an entry an older `lock` wrote.
        assert_eq!(
            crate::regate::diff_against_pin(store.root(), &hex, &tmp.path().join("ext")),
            crate::regate::PinDiff::NoSnapshot
        );
        // And a later re-pin backfills it — nothing has to be migrated.
        store.pin_integrity_root(tmp.path(), "ext", &hex).unwrap();
        assert!(verified_content(
            &store.root().join("content").join(&hex),
            &hex
        ));
    }

    /// A tampered integrity-root deposit is never presented as the approved
    /// bytes — the same F4 rule the other two families hold, extended to the
    /// family that had no verification at all.
    #[test]
    fn a_tampered_integrity_root_deposit_does_not_verify() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("ext/index.ts")
            .write_str("// approved\n")
            .unwrap();
        let store = Store::with_root(tmp.child("store").path().to_path_buf());
        let hex = agentstack_core::digest::integrity_root_digest(tmp.path(), "ext")
            .unwrap()
            .hex()
            .to_string();
        store.pin_integrity_root(tmp.path(), "ext", &hex).unwrap();
        let dest = store.root().join("content").join(&hex);
        assert!(verified_content(&dest, &hex));

        fs::write(dest.join("index.ts"), "// EVIL\n").unwrap();
        assert!(
            !verified_content(&dest, &hex),
            "edited deposit bytes still verified as approved"
        );
        assert_eq!(
            crate::regate::diff_against_pin(store.root(), &hex, &tmp.path().join("ext")),
            crate::regate::PinDiff::NoSnapshot
        );
    }

    #[test]
    fn resolves_path_source() {
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("skills/x/SKILL.md").write_str("# x\n").unwrap();
        let store = Store::with_root(tmp.child("store").path().to_path_buf());
        let skill: Skill = toml::from_str("path = \"./skills/x\"").unwrap();
        let r = store.resolve(&skill, tmp.path(), None).unwrap();
        assert_eq!(r.source_kind, "path");
        assert!(r.path.join("SKILL.md").exists());
        assert!(!r.checksum.is_empty());
    }

    /// Sandbox `AGENTSTACK_HOME` under `TEST_ENV_LOCK` so this regression is
    /// safe when run against the previously cached implementation or if a cache
    /// is reintroduced.
    fn with_home<T>(f: impl FnOnce(&assert_fs::TempDir) -> T) -> T {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let out = f(&home);
        std::env::remove_var("AGENTSTACK_HOME");
        out
    }

    /// Pin a file's mtime to an exact time so two directory states can be
    /// made stat-identical in the aggregate.
    fn set_mtime(path: &Path, t: std::time::SystemTime) {
        fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    /// Regression — contract §3 step 4 / ruling 3 and ARCHITECTURE.md:119.
    /// Drives the real authoritative verification seam (`skill_lock_status` —
    /// the chain trust-grant, `use --write`, and the MCP loader all share): a
    /// same-size, mtime-restored in-place edit — the "same-stat" change a
    /// reintroduced stat-fingerprint cache would miss — is still caught as lock
    /// drift. AGENTSTACK_HOME is sandboxed under TEST_ENV_LOCK so running this
    /// against the old (cached) code cannot touch the developer's real cache.
    #[test]
    fn same_stat_skill_edit_is_detected_by_skill_lock_status() {
        with_home(|_home| {
            let tmp = assert_fs::TempDir::new().unwrap();
            let skill_md = tmp.child("skills/x/SKILL.md");
            skill_md.write_str("# x\ncontent-AAAA\n").unwrap(); // 17 bytes
            let skill_dir = tmp.path().join("skills/x");

            // Pin an old mtime so a hypothetical settle-window cache would deem
            // the dir cache-eligible; the fix must not depend on mtime at all.
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
            set_mtime(skill_md.path(), t);

            // Ground-truth pin from the raw, cache-free digest.
            let pin = dir_digest(&skill_dir).unwrap();
            let lock = crate::lock::Lock {
                version: crate::lock::SUPPORTED_LOCK_VERSION,
                extensions: Vec::new(),
                packages: Vec::new(),
                skills: vec![crate::lock::LockedSkill {
                    name: "x".into(),
                    source: crate::lock::SkillLockSource::Path,
                    path: Some("./skills/x".into()),
                    git: None,
                    rev: None,
                    checksum: pin,
                    license: None,
                    origin: None,
                }],
                servers: Vec::new(),
                instructions: Vec::new(),
                executables: Vec::new(),
                workflows: Vec::new(),
            };

            let manifest: crate::manifest::Manifest =
                toml::from_str("version = 1\n[skills.x]\npath = \"./skills/x\"\n").unwrap();
            let library = crate::library::Library::default();
            let lib_home = tmp.child("lib").path().to_path_buf();
            let store = Store::with_root(tmp.child("store").path().to_path_buf());

            // Same `store` instance across both calls, so any in-memory cache on
            // the seam is primed by the first call and would be hit by the second.
            let status_of = || {
                crate::resolve::skill_lock_status(
                    "x",
                    &manifest,
                    tmp.path(),
                    &library,
                    &lib_home,
                    &store,
                    &lock,
                    crate::resolve::ResolveMode::NoFetch,
                )
                .status
            };

            // First pass: clean, and primes any cache that sits on the seam.
            assert_eq!(
                status_of(),
                crate::resolve::SkillLockStatus::Matches,
                "freshly pinned content verifies"
            );

            // Same-size, same-mtime in-place edit — only the bytes differ.
            skill_md.write_str("# x\ncontent-BBBB\n").unwrap(); // also 17 bytes
            set_mtime(skill_md.path(), t);

            // Second pass: drift despite an identical stat fingerprint.
            let status = status_of();
            assert!(
                matches!(
                    status,
                    crate::resolve::SkillLockStatus::ChecksumDrift { .. }
                ),
                "same-stat content change must be lock drift, got {status:?}"
            );
            assert!(
                matches!(
                    crate::verify::skill_verdict(&status),
                    crate::verify::Verdict::Block(_)
                ),
                "drift must fail closed (Block)"
            );
        });
    }

    #[test]
    fn resolves_git_source_from_local_repo() {
        // Build a local git repo and resolve it via a file:// URL — exercises the
        // real git path without network.
        let tmp = assert_fs::TempDir::new().unwrap();
        let repo = tmp.child("repo");
        repo.create_dir_all().unwrap();
        let git = |args: &[&str]| {
            super::run_git(args, Some(repo.path())).unwrap();
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@e.st"]);
        git(&["config", "user.name", "t"]);
        repo.child("SKILL.md").write_str("# from git\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);

        let store = Store::with_root(tmp.child("store").path().to_path_buf());
        let url = format!("file://{}", repo.path().display());
        let skill: Skill = toml::from_str(&format!("git = \"{url}\"")).unwrap();
        let r = store.resolve(&skill, tmp.path(), None).unwrap();
        assert_eq!(r.source_kind, "git");
        assert!(r.rev.is_some());
        assert!(r.path.join("SKILL.md").exists());
    }

    /// A git source with a subpath resolves to the subdir; a `..`/symlink escape
    /// is refused (the supply-chain boundary the subpath feature must hold).
    #[test]
    fn git_subpath_resolves_subdir_and_rejects_escapes() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let repo = tmp.child("repo");
        repo.create_dir_all().unwrap();
        let git = |args: &[&str]| {
            super::run_git(args, Some(repo.path())).unwrap();
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@e.st"]);
        git(&["config", "user.name", "t"]);
        repo.child("skills/improve/SKILL.md")
            .write_str("# improve\n")
            .unwrap();
        // A symlink that points outside the repo.
        std::os::unix::fs::symlink("/etc", repo.path().join("evil")).unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "init"]);

        let store = Store::with_root(tmp.child("store").path().to_path_buf());
        let url = format!("file://{}", repo.path().display());

        // Good subpath → an immutable content-addressed snapshot of the
        // subdir (not the churning clone), carrying its SKILL.md.
        let ok: Skill =
            toml::from_str(&format!("git = \"{url}\"\nsubpath = \"skills/improve\"")).unwrap();
        let r = store.resolve(&ok, tmp.path(), None).unwrap();
        assert!(
            r.path
                .starts_with(tmp.child("store").path().join("content")),
            "resolved path must be the immutable snapshot: {}",
            r.path.display()
        );
        assert!(r.path.join("SKILL.md").exists());
        // The snapshot dir is named by the content digest.
        assert_eq!(r.path.file_name().unwrap().to_string_lossy(), r.checksum);

        // `..` component → rejected before any read.
        let dots: Skill = toml::from_str(&format!("git = \"{url}\"\nsubpath = \"../x\"")).unwrap();
        assert!(store.resolve(&dots, tmp.path(), None).is_err());

        // Symlink escape → rejected.
        let evil: Skill = toml::from_str(&format!("git = \"{url}\"\nsubpath = \"evil\"")).unwrap();
        let err = store.resolve(&evil, tmp.path(), None).unwrap_err();
        assert!(
            err.to_string().contains("outside the repo"),
            "symlink escape must be refused: {err}"
        );
    }

    /// A digest does not include symlink entries, so re-hashing alone cannot
    /// detect a link added to an otherwise valid cached snapshot. Verification
    /// must reject the link and rebuild from the trusted, symlink-free source.
    #[cfg(unix)]
    #[test]
    fn snapshot_rebuilds_when_nested_symlink_is_added() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let src = tmp.child("src");
        src.child("nested/SKILL.md").write_str("# safe\n").unwrap();
        let digest = dir_digest(src.path()).unwrap().hex().to_string();
        let store = Store::with_root(tmp.child("store").path().to_path_buf());

        let snapshot = store.snapshot_content(src.path(), &digest).unwrap();
        std::os::unix::fs::symlink("SKILL.md", snapshot.join("nested/leak")).unwrap();
        assert!(
            !verified_snapshot(&snapshot, &digest),
            "a symlink must invalidate a snapshot even when its file digest is unchanged"
        );

        let rebuilt = store.snapshot_content(src.path(), &digest).unwrap();
        assert_eq!(rebuilt, snapshot);
        assert!(rebuilt.join("nested/leak").symlink_metadata().is_err());
        assert!(verified_snapshot(&rebuilt, &digest));
    }

    /// Every no-network path reader shares the same symlink refusal as fetch
    /// resolution; otherwise doctor, MCP load, or dry-run use could expose
    /// bytes that the ingest gate would reject.
    #[cfg(unix)]
    #[test]
    fn offline_path_readers_reject_symlinked_content() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let skill_dir = tmp.child("skills/x");
        skill_dir.child("SKILL.md").write_str("# safe\n").unwrap();
        std::os::unix::fs::symlink("SKILL.md", skill_dir.path().join("leak")).unwrap();
        let store = Store::with_root(tmp.child("store").path().to_path_buf());
        let skill: Skill = toml::from_str("path = \"./skills/x\"").unwrap();

        assert!(store.resolve_local(&skill, tmp.path(), None).is_err());
        assert!(store.resolve_path_only(&skill, tmp.path(), None).is_err());
        assert!(local_source_dir(&store, &skill, tmp.path(), None).is_none());

        // The source directory itself being a link is equally forbidden; a
        // recursive walk alone would otherwise follow it before seeing a link.
        let safe_dir = tmp.child("skills/safe");
        safe_dir.child("SKILL.md").write_str("# safe\n").unwrap();
        std::os::unix::fs::symlink("safe", tmp.path().join("skills/root-link")).unwrap();
        let root_link: Skill = toml::from_str("path = \"./skills/root-link\"").unwrap();
        assert!(store.resolve_local(&root_link, tmp.path(), None).is_err());
        assert!(store
            .resolve_path_only(&root_link, tmp.path(), None)
            .is_err());
        assert!(local_source_dir(&store, &root_link, tmp.path(), None).is_none());
    }
}
