//! Native-extension render (D6): copy each declared `[extensions.*]` source
//! into its target harness's extension directory, fail-closed on trust + lock,
//! tracked by an ownership ledger so a re-render prunes exactly what agentstack
//! placed and never touches a user's own files or the host guard's reserved
//! artifacts.
//!
//! Extensions are the highest-risk capability agentstack manages — the code
//! runs INSIDE the harness process at full user permission, outside every
//! policy ceiling. agentstack pins and delivers the bytes; it never executes
//! or governs them (docs/design/extensions-capability.md). So this renderer
//! makes exactly two promises: the bytes are copies of a lock-pinned source,
//! and the project was trusted at render time. Both are checked before a single
//! byte is written.
//!
//! Copy, never symlink (unlike skills): the harness must load the bytes that
//! were pinned, not whatever a post-render source edit leaves behind — a live
//! edit reaching the harness between agentstack operations would defeat the
//! content binding.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use serde::{Deserialize, Serialize};

use crate::adapter::descriptor::AdapterDescriptor;
use crate::adapter::registry::Registry;
use crate::lock::Lock;
use crate::manifest::Manifest;
use crate::resolve::{extension_lock_status, ExtensionLockStatus};
use crate::scope::Scope;
use crate::verify::{extension_verdict, Verdict};

/// The host guard's reserved artifact prefix. Any filename starting with this
/// belongs to the guard (`agentstack-guard.ts`, `agentstack-guard.js`) and is
/// NEVER created, overwritten, or pruned by this renderer — even if a
/// hand-forged ledger entry claims one. A hard deny-list, checked at BOTH prune
/// and render. `pub` so read-only surfaces (doctor's rendered-artifact audit,
/// the dashboard) skip guard artifacts the same way this renderer does.
pub const GUARD_PREFIX: &str = "agentstack-guard";

/// Ownership ledger dropped inside each rendered extension directory. Hidden
/// (`.`-prefixed) so `discover_extensions` skips it — agentstack's own
/// bookkeeping must never surface as a discovered "extension".
const LEDGER_FILE: &str = ".agentstack-extensions.json";

/// The render ledger for ONE extension directory (which may be shared across
/// projects at global scope), so each entry records which project placed the
/// artifact — pruning only ever touches THIS project's own artifacts.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Ledger {
    version: u32,
    #[serde(default)]
    artifacts: BTreeMap<String, LedgerEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LedgerEntry {
    /// The manifest extension name (`[extensions.<name>]`).
    name: String,
    /// Absolute project base path that placed this artifact.
    project: String,
    /// The lock pin checksum the artifact was rendered from.
    checksum: String,
}

/// One artifact to render into a resolved extension directory.
struct Planned {
    /// The manifest extension name.
    ext_name: String,
    /// Target adapter id (for user-facing lines).
    target: String,
    /// Destination basename: the extension name for a directory source, or
    /// `<name><source extension>` for a single-file source.
    filename: String,
    /// The digest's exact anchor root (manifest dir for inline paths, the git
    /// checkout root, or the library body dir) — copying from the same pair
    /// the pin walked is what makes the delivered bytes the pinned bytes.
    anchor: PathBuf,
    /// The declared path under `anchor` (`ResolvedExtension::declared`).
    declared: String,
    /// Whether the source resolves to a directory (copy the tree) or a file.
    is_dir: bool,
}

/// Render every declared native extension for `scope`, returning the
/// project-root-relative, directory-level `.gitignore` entries the caller
/// should fold into its managed block (empty unless a project-scope artifact
/// landed under the project root).
///
/// `write` mirrors the caller's write decision: `false` prints the plan and
/// touches nothing; `true` runs the fail-closed gate (trust, then lock) and
/// materializes.
pub fn render(
    manifest: &Manifest,
    registry: &Registry,
    scope: Scope,
    manifest_dir: &Path,
    write: bool,
) -> Result<Vec<String>> {
    let base = crate::manifest::project_root_of(manifest_dir);
    let base_id = crate::trust::key_for(&base);

    // Group planned artifacts by their resolved destination directory: several
    // extensions can target one adapter (one shared dir + ledger + prune set).
    let mut by_dir: BTreeMap<PathBuf, Vec<Planned>> = BTreeMap::new();
    let mut unsupported: Vec<(String, String)> = Vec::new();
    let mut broken: Vec<(String, String)> = Vec::new();

    // Planning resolves each source the same way the pin did (inline path,
    // inline git, or central library — NoFetch: render never touches the
    // network; an un-cached git source fails closed with an install hint).
    let library = crate::library::Library::load_default_or_warn();
    let lib_home = crate::util::paths::lib_home();
    let store = crate::store::Store::default_store();

    for (name, ext) in &manifest.extensions {
        let Some(desc) = registry.get(&ext.target) else {
            // Validation already rejects unknown targets; if one slips through,
            // surface it rather than silently dropping the extension.
            unsupported.push((name.clone(), ext.target.clone()));
            continue;
        };
        let Some(ext_dir) = resolve_ext_dir(desc, scope, manifest_dir, name) else {
            unsupported.push((name.clone(), ext.target.clone()));
            continue;
        };
        let resolved = match crate::resolve::resolve_extension_entry(
            name,
            ext,
            manifest_dir,
            &library,
            &lib_home,
            &store,
            crate::resolve::ResolveMode::NoFetch,
        ) {
            Ok(r) => r,
            Err(e) => {
                // Unresolvable is not the same as unrenderable: the source
                // exists in the manifest/library but can't be delivered as
                // pinned right now (offline checkout, broken body). Fail
                // closed in write mode, honest line in dry-run.
                broken.push((name.clone(), format!("{e:#}")));
                continue;
            }
        };
        let (is_dir, filename) = plan_artifact_name(&resolved.anchor, &resolved.declared, name);
        by_dir.entry(ext_dir).or_default().push(Planned {
            ext_name: name.clone(),
            target: ext.target.clone(),
            filename,
            anchor: resolved.anchor,
            declared: resolved.declared,
            is_dir,
        });
    }

    // Prune-candidate directories: every ExtensionsSpec adapter's dir for THIS
    // scope, so a fully-removed extension's orphan is still reachable when the
    // manifest declares nothing for that adapter. Non-fallback resolution — a
    // project-scope prune only ever touches project directories, never an
    // unrelated adapter's global dir.
    for desc in registry.iter() {
        if desc.extensions.is_none() {
            continue;
        }
        if let Some(dir) = desc.extensions_dir_for(scope, manifest_dir) {
            by_dir.entry(dir).or_default();
        }
    }

    // Decide whether there is anything at all to say/do. A candidate dir with
    // no planned artifacts and no this-project ledger entries is a pure no-op:
    // never create or touch it (mirrors the skills prune-only rule).
    let mut prunes: BTreeMap<&PathBuf, Vec<String>> = BTreeMap::new();
    let mut has_prunes = false;
    for (dir, planned) in &by_dir {
        let p = prunable(dir, &base_id, planned)?;
        has_prunes |= !p.is_empty();
        prunes.insert(dir, p);
    }
    let has_renders = by_dir.values().any(|v| !v.is_empty());
    if !has_renders && !has_prunes && unsupported.is_empty() && broken.is_empty() {
        return Ok(Vec::new());
    }

    println!("\n{}", "Native extensions".bold());
    for (name, target) in &unsupported {
        // Loud, never silent (mirrors the instruction silent-drop warnings):
        // the harness has no extension directory agentstack can render into.
        println!(
            "  {} extension '{name}' targets '{target}', which agentstack cannot render extensions into — not delivered",
            "⚠".yellow()
        );
        println!(
            "  {} remove the extension or target a CLI with an extensions directory",
            "↳".cyan()
        );
    }
    for (name, why) in &broken {
        println!(
            "  {} extension '{name}' cannot be delivered as pinned: {why}",
            "✗".red()
        );
    }

    if !write {
        for (dir, planned) in &by_dir {
            for p in planned {
                println!(
                    "  {} extension '{}' → {} ({})",
                    "→".cyan(),
                    p.ext_name,
                    p.target,
                    dir.join(&p.filename).display()
                );
            }
            for fname in &prunes[dir] {
                println!(
                    "  {} would prune extension artifact '{fname}'",
                    "−".yellow()
                );
            }
        }
        return Ok(Vec::new());
    }

    // --- fail-closed gate, in the exact required order, before any RENDER
    // (adding executable bytes) — pruning our own artifacts is the safe,
    // inert direction and proceeds even for an untrusted/unpinned project.

    // A declared extension that can't be resolved to its pinned bytes right
    // now (offline git checkout, broken library body) fails the whole write:
    // rendering the rest would silently deliver a partial surface.
    if !broken.is_empty() {
        let lines = broken
            .iter()
            .map(|(name, why)| format!("  extension '{name}'  {why}"))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(
            "refusing to render native extensions: {} extension(s) cannot be resolved to their pinned content —\n{lines}",
            broken.len(),
        );
    }

    let mut checksums: BTreeMap<String, String> = BTreeMap::new();
    if has_renders {
        // (a) Trust: an untrusted or drifted project renders ZERO extension
        //     bytes (rule 3, "untrusted means inert").
        match crate::trust::check(&base) {
            crate::trust::TrustState::Trusted => {}
            state => anyhow::bail!(
                "refusing to render native extensions: project at {} is {} — review and \
                 `agentstack trust .` before rendering executable extension code",
                base.display(),
                match state {
                    crate::trust::TrustState::Untrusted => "not trusted",
                    crate::trust::TrustState::Changed => "changed since it was trusted",
                    crate::trust::TrustState::Trusted => unreachable!(),
                }
            ),
        }

        // (b) Lock: every extension to render must be pinned AND matching. A
        //     missing pin blocks too — executable content is never
        //     first-pinned at render. The status resolves through the same
        //     seams planning used (NoFetch: render never fetches), so the gate
        //     judges exactly the content the copy below will deliver.
        let lock = Lock::load(manifest_dir)?;
        let mut blocked: Vec<(String, String)> = Vec::new();
        for planned in by_dir.values() {
            for p in planned {
                let ext = &manifest.extensions[&p.ext_name];
                match extension_lock_status(
                    &p.ext_name,
                    ext,
                    manifest_dir,
                    &library,
                    &lib_home,
                    &store,
                    &lock,
                    crate::resolve::ResolveMode::NoFetch,
                )
                .status
                {
                    ExtensionLockStatus::Matches => {
                        // Gate passed → the pin exists; record it for the ledger.
                        if let Some(entry) = lock.get_extension(&p.ext_name) {
                            checksums.insert(p.ext_name.clone(), entry.checksum.clone());
                        }
                    }
                    ExtensionLockStatus::MissingLockEntry => blocked.push((
                        format!("extension '{}'", p.ext_name),
                        "not pinned in agentstack.lock — run `agentstack lock` (executable content is never first-pinned at render)".to_string(),
                    )),
                    other => {
                        if let Verdict::Block(why) = extension_verdict(&other) {
                            blocked.push((format!("extension '{}'", p.ext_name), why));
                        }
                    }
                }
            }
        }
        if !blocked.is_empty() {
            let width = blocked.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
            let lines = blocked
                .iter()
                .map(|(name, why)| format!("  {name:width$}  {why}"))
                .collect::<Vec<_>>()
                .join("\n");
            anyhow::bail!(
                "refusing to render native extensions: {} extension(s) failed lock verification —\n{}\nReview the changes, then run `agentstack lock` to accept them.",
                blocked.len(),
                lines
            );
        }
    }

    // --- materialize (gates passed) ---
    let mut ignore_entries: Vec<String> = Vec::new();
    for (dir, planned) in &by_dir {
        // Skip a candidate dir that has nothing to render and nothing to prune:
        // don't create or touch it.
        if planned.is_empty() && prunes[dir].is_empty() {
            continue;
        }
        materialize_dir(dir, &base_id, planned, &checksums)?;
        // Project-scope artifacts under the project root are machine-local
        // generated files — keep them out of git, directory-level like skills.
        if scope == Scope::Project {
            if let Ok(rel) = dir.strip_prefix(&base) {
                ignore_entries.push(format!("/{}/", rel.display()));
            }
        }
    }
    Ok(ignore_entries)
}

/// What [`verify_rendered`] found for one harness's declared extensions.
pub struct RenderedVerification {
    /// Extensions whose rendered copy was located and matched its lock pin.
    pub verified: Vec<String>,
    /// Extensions targeting this harness that rendered NO artifact — honestly
    /// absent (nothing delivered = nothing to verify), never an error.
    pub absent: Vec<String>,
}

/// Verify that every rendered extension copy destined for `harness` still
/// matches its `agentstack.lock` pin (E2b, design doc §6).
///
/// The locked-verify gate proves the SOURCE bytes still match the pin; this
/// proves the COPY that was delivered into the harness's extension directory
/// does too. The two are distinct surfaces: a rendered artifact can be
/// tampered after render while its source is left untouched, so a source-only
/// check would let doctored bytes reach the harness unreviewed. Copies are
/// what the harness actually loads, so copies are what must match.
///
/// Honest absence is not an error: a harness with nothing rendered (E2 never
/// ran, or this project rendered nothing for this adapter) has nothing to
/// verify — the extension is reported `absent`, not blocked. Only a *present*
/// artifact whose bytes drifted from the pin refuses, naming the extension.
///
/// The `harness` positional IS an adapter id (the locked flow resolves it via
/// `registry.get`), and an extension names its target adapter by that same id
/// (`ext.target`), so the extensions destined for this launch are exactly
/// those whose `target == harness`.
pub fn verify_rendered(
    manifest: &Manifest,
    registry: &Registry,
    harness: &str,
    scope: Scope,
    manifest_dir: &Path,
    lock: &Lock,
) -> Result<RenderedVerification> {
    let base = crate::manifest::project_root_of(manifest_dir);
    let base_id = crate::trust::key_for(&base);
    let mut verified: Vec<String> = Vec::new();
    let mut absent: Vec<String> = Vec::new();

    for (name, ext) in &manifest.extensions {
        // Only the extensions this harness's adapter would load.
        if ext.target != harness {
            continue;
        }
        // No adapter / no extension dir for this scope → nothing could have
        // been rendered, so nothing to verify (honestly absent, not an error).
        let Some(desc) = registry.get(&ext.target) else {
            absent.push(name.clone());
            continue;
        };
        let Some(ext_dir) = ext_dir_resolved(desc, scope, manifest_dir) else {
            absent.push(name.clone());
            continue;
        };

        // Locate THIS project's rendered artifact for THIS extension through
        // the ownership ledger — the same record the renderer wrote, so the
        // two never disagree on which file belongs to which extension.
        let ledger = load_ledger(&ext_dir.join(LEDGER_FILE))?;
        let Some((filename, _entry)) = ledger.artifacts.iter().find(|(fname, e)| {
            e.name == *name && e.project == base_id && !fname.starts_with(GUARD_PREFIX)
        }) else {
            absent.push(name.clone());
            continue;
        };

        let artifact = ext_dir.join(filename);
        if !artifact.exists() {
            // The ledger records a delivered artifact, but its bytes are gone:
            // the harness would load nothing where reviewed code was pinned.
            // A vanished delivery is tampering, not honest absence.
            anyhow::bail!(
                "extension '{name}' (rendered copy): the ledger records a delivered artifact at \
                 {} but it is missing — re-run `agentstack apply` to re-render it",
                artifact.display()
            );
        }

        // Recompute the copy's content digest with the SAME strict
        // integrity-root walk the pin used, anchored at the extension dir so
        // only the artifact basename is walked. The sibling ledger lives at
        // the directory root, never inside a `<name>/` tree or beside a
        // single `<name>.<ext>` file, so it is naturally excluded.
        let current = agentstack_core::digest::integrity_root_digest(&ext_dir, filename)
            .with_context(|| {
                format!(
                    "digesting rendered extension '{name}' at {}",
                    artifact.display()
                )
            })?;
        // Compare to the authoritative lock pin (not the ledger's own record):
        // a re-lock that was never re-rendered leaves a stale copy that must
        // refuse, exactly like a byte-tampered one.
        let pin = lock
            .get_extension(name)
            .map(|e| e.checksum.as_str())
            .unwrap_or_default();
        if current.hex() != pin {
            anyhow::bail!(
                "extension '{name}' (rendered copy) at {} drifted from agentstack.lock \
                 (locked {}, rendered {}) — the delivered bytes no longer match the reviewed \
                 pin; re-run `agentstack apply` to re-render, or review the source and \
                 `agentstack lock`",
                artifact.display(),
                short12(pin),
                short12(current.hex()),
            );
        }
        verified.push(name.clone());
    }

    Ok(RenderedVerification { verified, absent })
}

/// One artifact agentstack's ownership ledger records in a governed extension
/// directory — surfaced read-only to doctor (the rendered-artifact audit) and
/// the dashboard ("managed by agentstack" labelling). A directory may hold
/// artifacts from several projects (a shared global dir); each carries the pin
/// checksum it was rendered from.
pub struct ManagedArtifact {
    /// The on-disk basename in the extension directory (`<name>` for a directory
    /// source, `<name>.<ext>` for a single file). Matches what
    /// `discover_extensions` reports as an entry's `name`.
    pub filename: String,
    /// The manifest extension name (`[extensions.<name>]`) that placed it.
    pub name: String,
    /// The strict integrity-root digest the artifact was rendered from — empty
    /// only for a pre-checksum ledger entry, in which case the copy can't be
    /// verified against a pin.
    pub checksum: String,
}

/// The ledger-owned artifacts in `ext_dir`, across every project that renders
/// into it (a global dir is multi-project). Empty when there is no ledger. A
/// corrupt ledger is a hard error — the same fail-closed reading the renderer
/// uses, never a silent empty set that would make managed artifacts look
/// unmanaged. Read-only: never writes or touches a byte.
pub fn managed_artifacts(ext_dir: &Path) -> Result<Vec<ManagedArtifact>> {
    let ledger = load_ledger(&ext_dir.join(LEDGER_FILE))?;
    Ok(ledger
        .artifacts
        .into_iter()
        .map(|(filename, e)| ManagedArtifact {
            filename,
            name: e.name,
            checksum: e.checksum,
        })
        .collect())
}

/// First 12 hex chars of a digest (or the whole string when shorter) — enough
/// to identify, short enough to read, matching `verify::short`.
fn short12(digest: &str) -> &str {
    digest.get(..12).unwrap_or(digest)
}

/// The resolved extension directory for `desc` at `scope`, quiet twin of
/// [`resolve_ext_dir`]: same project→global fallback (so verification and
/// render agree on where an artifact lives) but no printed note. `None` when
/// the adapter declares no extension surface at all.
fn ext_dir_resolved(
    desc: &AdapterDescriptor,
    scope: Scope,
    manifest_dir: &Path,
) -> Option<PathBuf> {
    desc.extensions.as_ref()?;
    desc.extensions_dir_for(scope, manifest_dir)
        .or_else(|| desc.extensions_dir_for(Scope::Global, manifest_dir))
}

/// Resolve a target adapter's extension directory for `scope`, falling back to
/// the global dir (with a printed note) when a project-scope render is asked of
/// an adapter that has no project directory. `None` means the adapter declares
/// no extension surface at all — the caller warns and drops.
fn resolve_ext_dir(
    desc: &AdapterDescriptor,
    scope: Scope,
    manifest_dir: &Path,
    ext_name: &str,
) -> Option<PathBuf> {
    // No `extensions:` block → the CLI has no extension directory to render into.
    desc.extensions.as_ref()?;
    if let Some(dir) = desc.extensions_dir_for(scope, manifest_dir) {
        return Some(dir);
    }
    // Project requested, but this adapter exposes only a global dir. A global
    // ExtensionsSpec always resolves, so this fallback never returns None.
    let global = desc.extensions_dir_for(Scope::Global, manifest_dir)?;
    println!(
        "  {} extension '{ext_name}': {} has no project extensions directory — rendering into the user dir {}",
        "·".dimmed(),
        desc.id,
        global.display()
    );
    Some(global)
}

/// Decide a source's destination basename: the extension name for a directory,
/// or `<name><source extension>` for a single file. Resolves defensively; a
/// source the strict digest would reject is treated as a file named after the
/// extension — the write-path lock gate blocks it regardless, so this only
/// affects the dry-run preview line.
fn plan_artifact_name(anchor: &Path, declared: &str, name: &str) -> (bool, String) {
    match agentstack_core::digest::resolve_contained(anchor, declared) {
        Ok(resolved) => {
            let is_dir = resolved.is_dir();
            if is_dir {
                (true, name.to_string())
            } else {
                let filename = match resolved.extension().and_then(|e| e.to_str()) {
                    Some(ext) => format!("{name}.{ext}"),
                    None => name.to_string(),
                };
                (false, filename)
            }
        }
        Err(_) => (false, name.to_string()),
    }
}

/// The this-project ledger artifacts no longer in `planned`'s render set —
/// candidates for pruning. Never includes a guard-reserved name.
fn prunable(ext_dir: &Path, base_id: &str, planned: &[Planned]) -> Result<Vec<String>> {
    let ledger = load_ledger(&ext_dir.join(LEDGER_FILE))?;
    let render_set: BTreeSet<&str> = planned.iter().map(|p| p.filename.as_str()).collect();
    Ok(ledger
        .artifacts
        .iter()
        .filter(|(fname, e)| {
            e.project == base_id
                && !render_set.contains(fname.as_str())
                && !fname.starts_with(GUARD_PREFIX)
        })
        .map(|(f, _)| f.clone())
        .collect())
}

/// Prune this project's stale artifacts, then render the current set into
/// `ext_dir`, updating the ledger. Fails closed on any collision with a
/// non-ledger file and refuses to touch a guard-reserved name.
fn materialize_dir(
    ext_dir: &Path,
    base_id: &str,
    planned: &[Planned],
    checksums: &BTreeMap<String, String>,
) -> Result<()> {
    fs::create_dir_all(ext_dir).with_context(|| format!("creating {}", ext_dir.display()))?;
    let ledger_path = ext_dir.join(LEDGER_FILE);
    let mut ledger = load_ledger(&ledger_path)?;

    let render_set: BTreeSet<&str> = planned.iter().map(|p| p.filename.as_str()).collect();

    // Prune: only THIS project's ledger artifacts that dropped out of the set,
    // and never a guard-reserved name even if a forged ledger entry claims one.
    let stale: Vec<String> = ledger
        .artifacts
        .iter()
        .filter(|(fname, e)| {
            e.project == base_id
                && !render_set.contains(fname.as_str())
                && !fname.starts_with(GUARD_PREFIX)
        })
        .map(|(f, _)| f.clone())
        .collect();
    for fname in stale {
        remove_artifact(&ext_dir.join(&fname))?;
        ledger.artifacts.remove(&fname);
        println!("  {} pruned extension artifact '{fname}'", "−".yellow());
    }

    for p in planned {
        // Containment: the destination basename derives from the extension name
        // (`[extensions.<name>]`), which validation rejects when it is not a
        // plain component — but trust binds consent to bytes, not safety, so the
        // sink refuses independently. A name like `../extensions/x` would both
        // escape this directory AND slip past the guard deny-list below (it does
        // not literally start with the prefix), so this check must come first.
        if !is_safe_artifact_key(&p.filename) {
            anyhow::bail!(
                "refusing to render extension '{}': artifact name '{}' is not a plain basename — an extension name must not contain path separators or `..`",
                p.ext_name,
                p.filename
            );
        }
        // Hard deny-list: never author over the guard's reserved names.
        if p.filename.starts_with(GUARD_PREFIX) {
            anyhow::bail!(
                "refusing to render extension '{}': artifact name '{}' collides with the host guard's reserved `{GUARD_PREFIX}*` names",
                p.ext_name,
                p.filename
            );
        }
        let dest = ext_dir.join(&p.filename);
        // Only overwrite an artifact THIS project already owns; never clobber a
        // hand-installed file or one another project rendered.
        let owned = ledger
            .artifacts
            .get(&p.filename)
            .is_some_and(|e| e.project == base_id);
        if dest.exists() && !owned {
            anyhow::bail!(
                "refusing to render extension '{}': {} already exists and is not managed by agentstack for this project — remove it or rename the extension",
                p.ext_name,
                dest.display()
            );
        }
        if owned {
            remove_artifact(&dest)?; // idempotent re-render
        }
        copy_artifact(&p.anchor, &p.declared, p.is_dir, &dest)
            .with_context(|| format!("rendering extension '{}'", p.ext_name))?;
        let checksum = checksums.get(&p.ext_name).cloned().unwrap_or_default();
        ledger.artifacts.insert(
            p.filename.clone(),
            LedgerEntry {
                name: p.ext_name.clone(),
                project: base_id.to_string(),
                checksum,
            },
        );
        println!(
            "  {} extension '{}' → {}",
            "✓".green(),
            p.ext_name,
            dest.display()
        );
    }

    save_ledger(&ledger_path, &ledger)
}

/// Copy the lock-pinned source tree (or single file) into `dest`, byte for
/// byte, reusing the strict symlink-rejecting integrity-root walk so no link
/// that appeared after the digest check can smuggle foreign bytes in.
/// `anchor` is the digest's own root (manifest dir, git checkout root, or the
/// library body dir) — the same pair the pin walked.
fn copy_artifact(anchor: &Path, declared: &str, is_dir: bool, dest: &Path) -> Result<()> {
    let (root, files) = agentstack_core::digest::integrity_root_files(anchor, declared)?;
    if is_dir {
        fs::create_dir_all(dest).with_context(|| format!("creating {}", dest.display()))?;
        for rel in &files {
            let from = root.join(rel);
            let to = dest.join(rel);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            fs::copy(&from, &to)
                .with_context(|| format!("copying {} → {}", from.display(), to.display()))?;
        }
    } else {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::copy(&root, dest)
            .with_context(|| format!("copying {} → {}", root.display(), dest.display()))?;
    }
    Ok(())
}

/// Remove one rendered artifact — a directory tree or a single file. Only ever
/// called on paths agentstack authored (ledger-owned or a same-project
/// re-render); the guard deny-list is enforced by the callers.
fn remove_artifact(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            fs::remove_dir_all(path).with_context(|| format!("removing {}", path.display()))
        }
        Ok(_) => fs::remove_file(path).with_context(|| format!("removing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// A ledger key (and any rendered artifact basename) must be a single, plain
/// path component — the basename agentstack itself wrote, never a path. The
/// ledger lives inside the repo-controlled extension directory and is outside
/// the trust digest (`digest_for` hashes only manifest + local + lock), so a
/// forged key like `../../etc/x` would, once joined onto the extension dir and
/// handed to `remove_artifact`, delete outside the directory agentstack owns.
/// Rejects empty, `.`/`..`, absolute, and any key carrying a `/` or `\`
/// separator, so nothing but an in-directory basename can reach a filesystem
/// join.
fn is_safe_artifact_key(key: &str) -> bool {
    if key.is_empty() || key.contains('/') || key.contains('\\') {
        return false;
    }
    let mut comps = Path::new(key).components();
    matches!(
        (comps.next(), comps.next()),
        (Some(std::path::Component::Normal(c)), None) if c == std::ffi::OsStr::new(key)
    )
}

/// Load a directory's ownership ledger. Bounded read (hostile input); a corrupt
/// ledger is a hard error, never a silent reset — a reset would strand the
/// artifacts it tracked as unmanaged. A key that is not a safe basename is
/// tampering (agentstack only ever writes plain basenames): refuse loudly
/// rather than let a traversal key reach the prune sink's `remove_artifact`.
fn load_ledger(path: &Path) -> Result<Ledger> {
    match crate::util::read_to_string_bounded(path, crate::util::MAX_CONFIG_BYTES) {
        Ok(text) => {
            let ledger: Ledger = serde_json::from_str(&text)
                .with_context(|| format!("parsing extension ledger {}", path.display()))?;
            for key in ledger.artifacts.keys() {
                if !is_safe_artifact_key(key) {
                    anyhow::bail!(
                        "extension ledger {} contains an unsafe artifact key {key:?} — a ledger \
                         key must be a plain basename, not a path; refusing to act on a tampered \
                         ledger",
                        path.display(),
                    );
                }
            }
            Ok(ledger)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Ledger {
            version: 1,
            artifacts: BTreeMap::new(),
        }),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn save_ledger(path: &Path, ledger: &Ledger) -> Result<()> {
    // An emptied ledger still persists (as `{}` artifacts): a future render
    // reads it rather than re-discovering strays.
    let text = serde_json::to_string_pretty(ledger)?;
    crate::util::atomic::write(path, &text).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    /// Isolate the global trust store (AGENTSTACK_HOME) under a temp home and
    /// run `f` with a fresh project dir. Serialized with the shared env lock.
    fn with_env(f: impl FnOnce(&assert_fs::TempDir, &assert_fs::TempDir)) {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        let proj = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        f(&home, &proj);
        std::env::remove_var("AGENTSTACK_HOME");
    }

    fn registry() -> Registry {
        Registry::load().unwrap()
    }

    /// The manifest TOML for a single pi extension `checkpoint`.
    const CHECKPOINT_TOML: &str = r#"version = 1
[extensions.checkpoint]
path = "./extensions/checkpoint"
target = "pi"
"#;

    /// Write an extension source dir + the `agentstack.toml` declaring it
    /// (on disk so `trust::trust` can pin its digest) against pi — whose
    /// project scope has a real `.pi/extensions` dir. Returns the manifest.
    fn project_with_extension(proj: &assert_fs::TempDir, body: &str) -> Manifest {
        proj.child("extensions/checkpoint/index.ts")
            .write_str(body)
            .unwrap();
        proj.child("agentstack.toml")
            .write_str(CHECKPOINT_TOML)
            .unwrap();
        toml::from_str(CHECKPOINT_TOML).unwrap()
    }

    /// Pin every declared (inline `path`) extension. All render tests use path
    /// extensions, so the library/store seams go unused, but they must still be
    /// passed through the resolver.
    fn pin_extensions(proj: &assert_fs::TempDir, manifest: &Manifest) {
        crate::commands::lock::record_extension_pins(
            proj.path(),
            manifest,
            &crate::library::Library::default(),
            &crate::util::paths::lib_home(),
            &crate::store::Store::default_store(),
        )
        .unwrap();
    }

    /// Pin every extension in `manifest` and trust the project — the clean
    /// state a render should accept.
    fn lock_and_trust(proj: &assert_fs::TempDir, manifest: &Manifest) {
        pin_extensions(proj, manifest);
        crate::trust::trust(proj.path()).unwrap();
    }

    fn ext_dir(proj: &assert_fs::TempDir) -> PathBuf {
        proj.path().join(".pi/extensions")
    }

    #[test]
    fn untrusted_project_renders_no_extension_bytes() {
        with_env(|_home, proj| {
            let manifest = project_with_extension(proj, "export default (pi) => {}\n");
            // Pinned, but NOT trusted.
            pin_extensions(proj, &manifest);

            let err = render(&manifest, &registry(), Scope::Project, proj.path(), true)
                .unwrap_err()
                .to_string();
            assert!(err.contains("not trusted"), "{err}");

            // Zero bytes reached the extension directory.
            let dir = ext_dir(proj);
            let empty = !dir.exists() || fs::read_dir(&dir).unwrap().next().is_none();
            assert!(empty, "untrusted render must leave the extension dir empty");
        });
    }

    #[test]
    fn drifted_or_unpinned_extension_blocks_render() {
        with_env(|_home, proj| {
            // `checkpoint` is pinned then drifts; `late` is declared but never
            // pinned. Both must be named and nothing written.
            proj.child("extensions/checkpoint/index.ts")
                .write_str("export default (pi) => {} // v1\n")
                .unwrap();
            proj.child("extensions/late/index.ts")
                .write_str("export default (pi) => {}\n")
                .unwrap();

            // Pin only `checkpoint`, and trust, before `late` is declared.
            let only_checkpoint: Manifest = toml::from_str(CHECKPOINT_TOML).unwrap();
            proj.child("agentstack.toml")
                .write_str(CHECKPOINT_TOML)
                .unwrap();
            pin_extensions(proj, &only_checkpoint);
            crate::trust::trust(proj.path()).unwrap();

            // Now drift checkpoint's bytes and declare the unpinned `late`.
            proj.child("extensions/checkpoint/index.ts")
                .write_str("export default (pi) => {} // v2\n")
                .unwrap();
            let manifest: Manifest = toml::from_str(
                r#"
                version = 1
                [extensions.checkpoint]
                path = "./extensions/checkpoint"
                target = "pi"
                [extensions.late]
                path = "./extensions/late"
                target = "pi"
                "#,
            )
            .unwrap();

            let err = render(&manifest, &registry(), Scope::Project, proj.path(), true)
                .unwrap_err()
                .to_string();
            assert!(err.contains("extension 'checkpoint'"), "{err}");
            assert!(err.contains("extension 'late'"), "{err}");
            assert!(err.contains("`agentstack lock`"), "{err}");

            // Nothing was written.
            let dir = ext_dir(proj);
            let empty = !dir.exists() || fs::read_dir(&dir).unwrap().next().is_none();
            assert!(empty, "a blocked render writes nothing");
        });
    }

    // The render path must deliver every source kind the pin machinery
    // accepts — a library-origin extension that pins, trusts, and verifies but
    // never lands in the harness dir would be a silent no-op wearing a green
    // checkmark.
    #[test]
    fn library_origin_extension_renders_and_verifies() {
        with_env(|_home, proj| {
            // A library body under <AGENTSTACK_HOME>/lib/extensions/, indexed.
            let lib_home = crate::util::paths::lib_home();
            fs::create_dir_all(lib_home.join("extensions/checkpoint")).unwrap();
            fs::write(
                lib_home.join("extensions/checkpoint/index.ts"),
                "export default (pi) => {} // lib\n",
            )
            .unwrap();
            let mut library = crate::library::Library::default();
            library.upsert_extension(crate::library::LibraryExtension {
                name: "checkpoint".into(),
                source: "path".into(),
                target: "pi".into(),
                path: Some("checkpoint".into()),
                git: None,
                rev: None,
                subpath: None,
                checksum: None,
                description: None,
                version: None,
                provenance: None,
            });
            library.save(&lib_home).unwrap();

            // Sourceless manifest entry → resolves from the central library.
            let toml_text = "version = 1\n[extensions.checkpoint]\ntarget = \"pi\"\n";
            proj.child("agentstack.toml").write_str(toml_text).unwrap();
            let manifest: Manifest = toml::from_str(toml_text).unwrap();
            crate::commands::lock::record_extension_pins(
                proj.path(),
                &manifest,
                &library,
                &lib_home,
                &crate::store::Store::default_store(),
            )
            .unwrap();
            crate::trust::trust(proj.path()).unwrap();

            render(&manifest, &registry(), Scope::Project, proj.path(), true).unwrap();
            let artifact = ext_dir(proj).join("checkpoint/index.ts");
            assert!(
                artifact.exists(),
                "library-origin extension must be delivered"
            );

            // The delivered copy digests to the lock pin.
            let lock = Lock::load(proj.path()).unwrap();
            let v = verify_rendered(
                &manifest,
                &registry(),
                "pi",
                Scope::Project,
                proj.path(),
                &lock,
            )
            .unwrap();
            assert_eq!(v.verified, vec!["checkpoint".to_string()]);
        });
    }

    #[test]
    fn forged_ledger_traversal_key_is_refused_and_deletes_nothing() {
        with_env(|_home, proj| {
            let manifest = project_with_extension(proj, "export default (pi) => {}\n");
            lock_and_trust(proj, &manifest);
            // Render once so a real ledger exists in the extension directory.
            render(&manifest, &registry(), Scope::Project, proj.path(), true).unwrap();

            // A sentinel OUTSIDE the extension directory that a `../`-escaping
            // ledger key resolves onto: `.pi/extensions/../../keepme.txt`.
            let sentinel = proj.path().join("keepme.txt");
            fs::write(&sentinel, b"do not delete\n").unwrap();

            // Forge the ledger with a traversal key owned by THIS project — the
            // ledger lives inside the repo-controlled dir and is outside the
            // trust digest, so this is the attacker's real primitive.
            let base_id = crate::trust::key_for(&crate::manifest::project_root_of(proj.path()));
            let forged = format!(
                r#"{{"version":1,"artifacts":{{"../../keepme.txt":{{"name":"evil","project":{base_id:?},"checksum":""}}}}}}"#
            );
            fs::write(ext_dir(proj).join(LEDGER_FILE), forged).unwrap();

            // Loading the tampered ledger must refuse before any prune runs, so
            // remove_artifact is never reached and the sentinel survives — even
            // on the write path an untrusted/pure-prune render would take.
            let err = render(&manifest, &registry(), Scope::Project, proj.path(), true)
                .unwrap_err()
                .to_string();
            assert!(err.contains("unsafe artifact key"), "{err}");
            assert!(
                sentinel.exists(),
                "a traversal ledger key must not delete outside the extension dir"
            );
        });
    }

    #[test]
    fn prune_removes_only_ledger_owned_artifacts() {
        with_env(|_home, proj| {
            let manifest = project_with_extension(proj, "export default (pi) => {}\n");
            lock_and_trust(proj, &manifest);

            // Round 1: render checkpoint.
            render(&manifest, &registry(), Scope::Project, proj.path(), true).unwrap();
            let dir = ext_dir(proj);
            assert!(dir.join("checkpoint/index.ts").exists());

            // Plant a hand-placed stranger file and a fake guard artifact.
            fs::write(dir.join("stranger.js"), b"// mine\n").unwrap();
            fs::write(dir.join("agentstack-guard.ts"), b"// guard\n").unwrap();

            // Round 2: manifest no longer declares checkpoint → its artifact is
            // pruned (pure-prune path needs neither trust nor a re-lock), but
            // the stranger and the guard file survive untouched.
            let empty_manifest: Manifest = toml::from_str("version = 1\n").unwrap();
            render(
                &empty_manifest,
                &registry(),
                Scope::Project,
                proj.path(),
                true,
            )
            .unwrap();

            assert!(
                !dir.join("checkpoint").exists(),
                "ledger-owned artifact pruned"
            );
            assert!(dir.join("stranger.js").exists(), "stranger file untouched");
            assert!(
                dir.join("agentstack-guard.ts").exists(),
                "guard artifact never pruned"
            );
        });
    }
}
