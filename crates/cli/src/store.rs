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
/// pins are a plain SHA-256 over one file's bytes, deposited as
/// `content/<hex>/<file name>`. A snapshot verifies if it matches its name
/// under EITHER family; symlinks anywhere disqualify under both. Used by the
/// re-gate diff reader (F4/F19): a diff rendered from a tampered snapshot
/// would present bytes the user never approved as "the approved version",
/// which corrupts the consent surface itself — the honest degrade is
/// `NoSnapshot`.
pub(crate) fn verified_content(dest: &Path, digest_hex: &str) -> bool {
    if verified_snapshot(dest, digest_hex) {
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
