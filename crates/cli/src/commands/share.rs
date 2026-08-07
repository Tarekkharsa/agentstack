//! `agentstack share` / `agentstack receive` — sharing is signing, receiving
//! is reviewing.
//!
//! Phase 4, Moment 10. Outbound, a bundle is signed as part of sharing rather
//! than as a separate ceremony nobody performs. Inbound, it lands in the same
//! staged funnel everything else does: fetched, bounded, quarantined, carded,
//! and only then — if the human says yes — activated.
//!
//! # Why this is not `export`/`import`
//!
//! [`super::bundle`] already moves a setup between YOUR machines: it
//! age-encrypts the manifest, the lock, and optionally your resolved secret
//! *values*, for an audience of one who already trusts the source completely.
//! Sharing is the opposite problem. The audience is someone else, the artifact
//! must be readable rather than encrypted-to-a-passphrase, it must carry the
//! pinned content bytes (not just the manifest text that names them), and it
//! must carry a signature. Retrofitting one onto the other would conflate
//! "move my setup" with "publish for others to review", and the second one's
//! whole point is that the receiver does not trust the sender yet.
//!
//! # The two decisions stay separate
//!
//! A signature answers "did these bytes come from this key, unchanged". The
//! yes answers "do I want this". A valid signature from a recognized publisher
//! shortens the card's reading; there is no signature, from anybody, that
//! substitutes for the yes. The witnesses assert both directions of that
//! independently, because it is the single claim of this feature most likely
//! to decay into "signed means fine".
//!
//! # Every received byte is hostile input
//!
//! A bundle is a file from someone else (invariant 7). It is size-capped
//! before parsing, its entry count and per-entry size are capped, its names are
//! rejected unless they are plain, and nothing in it is ever interpolated into
//! a command. Quarantine is a directory of inert bytes: nothing there is on any
//! search path, in any agent's context, or reachable by any server.

use agentstack_core::paint::OwoColorize;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use std::path::Path;

use crate::cli::{PublisherArgs, PublisherCmd, ReceiveArgs, ShareArgs};
use crate::publisher::{self, Provenance};

/// Hard ceilings on anything parsed from a bundle. Bounded before trusted, and
/// bounded generously enough that a legitimate setup never meets them: the
/// numbers exist to stop a hostile file from exhausting memory, not to express
/// a product opinion about how big a share may be.
const MAX_BUNDLE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 2_000;
const MAX_ENTRY_BYTES: usize = 4 * 1024 * 1024;
const MAX_NAME_LEN: usize = 128;

/// One piece of pinned content travelling with the bundle.
///
/// `deny_unknown_fields` (nit): a bundle is hostile input, and an unknown key
/// is either a newer format we should refuse loudly or a field smuggled past
/// the signature — the signed message is built from the KNOWN fields, so any
/// extra key rides along uncovered. Rejecting at parse time closes both.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub name: String,
    /// `skill` / `instruction` — what kind of capability this is.
    pub kind: String,
    /// File path RELATIVE to the capability's own root. Validated on receive.
    pub path: String,
    /// UTF-8 content. Bundles carry text capabilities; a binary payload is
    /// refused rather than base64-smuggled, because a capability nobody can
    /// read is a capability nobody can review.
    pub body: String,
    /// SPDX identifier, when the source declared one. Attribution is carried
    /// mechanically rather than by promise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Where these bytes came from, verbatim from the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// NOTICE / LICENSE text carried WITH the content, because an SPDX tag
    /// alone does not satisfy an attribution obligation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

/// The share artifact. Plain JSON on purpose: a receiver should be able to read
/// what they are being asked to trust with the tools they already have.
///
/// `deny_unknown_fields` for the same reason as [`Entry`]: an unknown top-level
/// key is not covered by the signature (`signed_message` re-serializes only
/// the declared fields), so accepting one would let a sender attach signed-past
/// data. Refuse it at parse time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareBundle {
    pub version: u32,
    /// The manifest text.
    pub manifest: String,
    /// The lockfile text, when the project has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<String>,
    #[serde(default)]
    pub entries: Vec<Entry>,
    /// Publisher public key, hex. Absent on an unsigned bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// Detached signature over [`ShareBundle::signed_message`], hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl ShareBundle {
    /// The exact bytes a signature covers: everything EXCEPT the signature and
    /// the key that made it.
    ///
    /// Built from a canonical re-serialization rather than from the file's own
    /// bytes, so that reformatting the JSON does not invalidate a signature
    /// while changing any actual content still does. The two fields excluded
    /// are excluded because including them would be circular.
    pub fn signed_message(&self) -> Vec<u8> {
        let bare = ShareBundle {
            version: self.version,
            manifest: self.manifest.clone(),
            lock: self.lock.clone(),
            entries: self.entries.clone(),
            publisher: None,
            signature: None,
        };
        // `to_vec` on a struct with declared field order is deterministic here;
        // this is our own type, not arbitrary JSON, so there is no map-ordering
        // ambiguity to worry about.
        //
        // A serialization failure returns a SENTINEL, never `b""` (nit): the
        // empty message is a value an all-empty bundle could plausibly sign,
        // so an `unwrap_or_default()` here would make a failed re-serialization
        // verify against a signature over nothing. This byte string cannot be
        // any bundle's real signed content, so a failure fails closed.
        serde_json::to_vec(&bare).unwrap_or_else(|_| b"\0agentstack:unserializable-bundle".to_vec())
    }

    /// What a receiver can establish about where this came from.
    pub fn provenance(&self) -> Provenance {
        let key = self
            .publisher
            .as_deref()
            .and_then(agentstack_trust::sign::PublicKey::from_hex);
        let sig = self
            .signature
            .as_deref()
            .and_then(agentstack_trust::sign::Signature::from_hex);
        publisher::assess(&self.signed_message(), key.as_ref(), sig.as_ref())
    }
}

// ─────────────────────────────────────────────────────────────── share (out)

pub fn run_share(args: &ShareArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let base = super::project_base(manifest_dir)?;
    let dir = crate::manifest::resolve_manifest_dir(&base);

    let manifest = std::fs::read_to_string(dir.join(crate::manifest::load::MANIFEST_FILE))
        .context("reading the manifest to share")?;
    let lock = std::fs::read_to_string(dir.join("agentstack.lock")).ok();

    let entries = collect_entries(&dir)?;

    let mut bundle = ShareBundle {
        version: 1,
        manifest,
        lock,
        entries,
        publisher: None,
        signature: None,
    };

    // Signing is not a flag. "Sharing is signing" is the whole design: an
    // unsigned bundle is something you can still hand-craft, but it is not
    // something this command produces, because an opt-in signature is one
    // nobody remembers to opt into.
    let seed = publisher::signing_seed()?;
    let (key, sig) = agentstack_trust::sign::sign(&seed, &bundle.signed_message());
    bundle.publisher = Some(key.to_hex());
    bundle.signature = Some(sig.to_hex());

    let out = args
        .out
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from(format!("{}.astack", args.name)));
    let body = serde_json::to_string_pretty(&bundle)?;
    agentstack_core::util::atomic::write(&out, &body)
        .with_context(|| format!("writing {}", out.display()))?;

    crate::outln!(
        "{:<12}{} {}",
        "signing as".dimmed(),
        publisher::fingerprint(&key),
        "✓".green()
    );
    crate::outln!(
        "{:<12}{} {}",
        "bundle".dimmed(),
        out.display().to_string().bold(),
        format!(
            "(manifest + {} + {})",
            if bundle.lock.is_some() {
                "lock"
            } else {
                "no lock"
            },
            count(bundle.entries.len(), "pinned file")
        )
        .dimmed()
    );
    crate::outln!(
        "\n{}",
        "receivers will review before anything activates — that's the point".dimmed()
    );
    Ok(())
}

/// The project's own skill and instruction files, as text.
///
/// Deliberately only this project's `.agentstack/` content: a bundle carries
/// what the sender authored, not whatever their machine happened to have
/// resolved from elsewhere. Anything unreadable as UTF-8 is refused loudly
/// rather than dropped quietly — a bundle that silently lost a file would give
/// the receiver a card describing content that is not what they will get.
fn collect_entries(dir: &Path) -> Result<Vec<Entry>> {
    // Attribution travels when the sender's lock recorded it (F18): the
    // schema and carry-forward already exist, and the receive card renders
    // exactly these fields — hard-coding `None` here was one of the dead
    // segments of that wire.
    let lock = crate::lock::Lock::load(dir).unwrap_or_default();
    let mut out = Vec::new();
    for (kind, sub) in [("skill", "skills"), ("instruction", "instructions")] {
        let root = dir.join(sub);
        if !root.exists() {
            continue;
        }
        for path in walk(&root) {
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let body = std::fs::read_to_string(&path).with_context(|| {
                format!(
                    "{} is not UTF-8 text — a bundle carries reviewable content, so this \
                     cannot be shared as-is",
                    path.display()
                )
            })?;
            let name = rel.split('/').next().unwrap_or(&rel).to_string();
            let (license, origin) = lock
                .get(&name)
                .map(|e| (e.license.clone(), e.origin.clone()))
                .unwrap_or((None, None));
            out.push(Entry {
                name,
                kind: kind.to_string(),
                path: rel,
                body,
                license,
                origin,
                notice: None,
            });
        }
    }
    Ok(out)
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        // Do not follow symlinks out of the tree: a link pointing at
        // `~/.ssh/id_rsa` would otherwise be packaged into a file the sender
        // is about to hand to someone else.
        if e.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            continue;
        }
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

// ──────────────────────────────────────────────────────────── receive (in)

pub fn run_receive(args: &ReceiveArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let base = super::project_base(manifest_dir)?;
    let dir = crate::manifest::resolve_manifest_dir(&base);

    let bundle = read_bounded(&args.path)?;
    let provenance = bundle.provenance();

    // ── quarantine, BEFORE the card ──────────────────────────────────────
    // The bytes land somewhere inert first, so that what the card describes is
    // what is actually on disk rather than what was in memory a moment ago.
    // Nothing here is on a search path, in an agent's context, or reachable by
    // a server. Declining removes it entirely.
    let staged = crate::quarantine::stage(&dir, &bundle.entries)?;

    // ── the card ─────────────────────────────────────────────────────────
    crate::outln!("{}", "Review — a shared bundle".bold());
    crate::outln!("  {}", provenance.card_line());
    if provenance.shortens_the_card() {
        // What recognition actually saves is the work of establishing WHO sent
        // this — not any part of the review below, which is identical either
        // way. That identity is structural, not asserted: this line prints
        // ABOVE the `summary_lines` loop, and that loop reads the bundle alone,
        // never the publisher, so there is no branch for it to differ on.
        // The first draft of this line said "the review below is shorter",
        // which was false. The witness (`tests/share_round_trip.rs`) pins the
        // OUTCOME byte for byte — the receiving project's whole file tree after
        // a recognized and an unrecognized run — and probes this card only by
        // substring; the two bodies are never compared to each other.
        // Claiming a saving that did not happen, on the consent card, is the
        // precise failure this phase exists to remove.
        crate::outln!(
            "  {}",
            "you have already decided whose key this is, so that question is settled. \
             What it contains is still yours to review."
                .dimmed()
        );
    }
    crate::outln!();
    for line in summary_lines(&bundle) {
        crate::outln!("  {line}");
    }
    if let Some(attr) = attribution_line(&bundle) {
        crate::outln!("  {attr}");
    }
    crate::outln!(
        "\n  {}",
        format!("staged at {} · nothing is active", staged.display()).dimmed()
    );

    // ── the yes ──────────────────────────────────────────────────────────
    let decision = confirmed(args, &provenance);
    let accepted = match decision {
        Ok(v) => v,
        Err(e) => {
            // A refusal must leave nothing staged, exactly like a decline.
            crate::quarantine::discard(&staged)?;
            return Err(e);
        }
    };
    if !accepted {
        // Fetched-then-declined leaves nothing, anywhere. Same property Phase 1
        // witnessed for a declined drop: the project is byte-identical and the
        // staged bytes are gone.
        crate::quarantine::discard(&staged)?;
        crate::outln!(
            "\n{} nothing was added; the staged copy is gone.",
            "·".dimmed()
        );
        return Ok(());
    }

    let landed = crate::quarantine::adopt(&staged, &dir)?;
    crate::outln!(
        "\n{} {} into this project.",
        "✓".green(),
        count(landed, "file")
    );
    // Ends through the same seam everything else does: whatever is honestly
    // next, not a verdict minted here. `next_step` is the human sentence and
    // is always present; `next_action` is the machine field and is null when
    // the honest next step is not runnable verbatim.
    let report = super::doctor::collect(Some(&dir))?;
    if let Some(next) = report["next_step"].as_str() {
        crate::outln!("{} {}", "next:".bold(), next.bold());
    }
    Ok(())
}

/// Read and parse a bundle with every bound applied before anything is trusted.
fn read_bounded(path: &Path) -> Result<ShareBundle> {
    let meta =
        std::fs::metadata(path).with_context(|| format!("cannot read {}", path.display()))?;
    if meta.len() > MAX_BUNDLE_BYTES {
        bail!(
            "{} is {} bytes — larger than the {MAX_BUNDLE_BYTES}-byte limit for a bundle. \
             Nothing was parsed.",
            path.display(),
            meta.len()
        );
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("{} is not UTF-8 text", path.display()))?;
    let bundle: ShareBundle = serde_json::from_str(&raw)
        .with_context(|| format!("{} is not a valid agentstack bundle", path.display()))?;

    if bundle.version != 1 {
        bail!(
            "bundle format version {} is not supported by this binary (expected 1)",
            bundle.version
        );
    }
    if bundle.entries.len() > MAX_ENTRIES {
        bail!(
            "bundle declares {} entries, over the {MAX_ENTRIES} limit. Nothing was staged.",
            bundle.entries.len()
        );
    }
    for e in &bundle.entries {
        if e.body.len() > MAX_ENTRY_BYTES {
            bail!(
                "'{}' is over the {MAX_ENTRY_BYTES}-byte per-file limit",
                crate::text::sanitize_line(&e.name)
            );
        }
        if e.name.len() > MAX_NAME_LEN || e.path.len() > MAX_NAME_LEN * 4 {
            bail!("a bundle entry has an unreasonably long name or path");
        }
        // Attribution fields render on the card and travel into records, so
        // they are bounded like every other field (F14). `kind` is bounded by
        // being an allow-list (F1): it becomes a path segment in quarantine,
        // which is the same hole `check_relative` closes for `path`.
        if e.license.as_deref().is_some_and(|l| l.len() > MAX_NAME_LEN)
            || e.origin
                .as_deref()
                .is_some_and(|o| o.len() > MAX_NAME_LEN * 4)
            || e.notice
                .as_deref()
                .is_some_and(|n| n.len() > MAX_ENTRY_BYTES)
        {
            bail!(
                "'{}' carries an unreasonably long license/origin/notice field",
                crate::text::sanitize_line(&e.name)
            );
        }
        crate::quarantine::check_kind(&e.kind)?;
        crate::quarantine::check_relative(&e.path)?;
    }
    Ok(bundle)
}

/// Two to five plain lines, the same shape the trust card uses.
fn summary_lines(bundle: &ShareBundle) -> Vec<String> {
    let mut out = Vec::new();
    let skills = bundle.entries.iter().filter(|e| e.kind == "skill").count();
    let instr = bundle
        .entries
        .iter()
        .filter(|e| e.kind == "instruction")
        .count();
    if skills > 0 {
        out.push(format!("Adds {}", count(skills, "skill file")));
    }
    if instr > 0 {
        out.push(format!("Adds {}", count(instr, "instruction file")));
    }
    // F22: only the skill/instruction ENTRIES survive the round trip —
    // `adopt` moves those two subtrees and nothing else. The manifest and
    // lockfile are parsed (for attribution and the signature) but never
    // delivered: servers, toolsets, policy, and hooks in a bundle's manifest
    // do not cross. Advertising "review and adopt what you want from it" over
    // a manifest the funnel then drops was the card promising a door that
    // isn't there. So the card describes what actually lands, and names the
    // manifest only as context for the signature, not as something to adopt.
    if skills == 0 && instr == 0 {
        out.push("No skill or instruction files — nothing would be added".to_string());
    }
    out.push(
        "The bundle's manifest and lockfile are used to verify these files, not merged — \
         servers, toolsets, and policy do not cross over"
            .to_string(),
    );
    out
}

/// Attribution, surfaced on the card rather than buried in a file.
fn attribution_line(bundle: &ShareBundle) -> Option<String> {
    let mut seen: Vec<String> = Vec::new();
    for e in &bundle.entries {
        if let (Some(lic), Some(origin)) = (&e.license, &e.origin) {
            // Attacker-supplied text on the consent card: sanitized at the
            // render site with the same ECMA-48 state machine the trust and
            // eve cards use (F14) — an `origin` full of cursor moves could
            // otherwise redraw the SIGNATURE DOES NOT MATCH line away.
            let s = format!(
                "{}, from {}",
                crate::text::sanitize_line(lic),
                crate::text::sanitize_line(origin)
            );
            if !seen.contains(&s) {
                seen.push(s);
            }
        }
    }
    if seen.is_empty() {
        return None;
    }
    Some(format!("Licensed: {}", seen.join(" · ")))
}

fn confirmed(args: &ReceiveArgs, provenance: &Provenance) -> Result<bool> {
    if args.yes {
        // F13: `--yes` acknowledges a review nobody is present to perform, so
        // it leans entirely on the signature — and it used to lean on nothing:
        // a tampered or unsigned bundle was accepted identically to a verified
        // one. Headless acceptance now requires a signature that VERIFIES.
        // Interactively, a human who has read the loud card may still decide;
        // headlessly there is no one to have read it.
        if !provenance.verifies() {
            bail!(
                "refusing --yes: {} — a headless accept leans entirely on the                  signature, and this one does not hold. Review it interactively instead.",
                provenance.card_line()
            );
        }
        return Ok(true);
    }
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        // Not a terminal: the answer is no. A non-interactive run that treated
        // silence as consent would be exactly the compressed-consent path the
        // strategy forbids for content from someone else.
        crate::outln!(
            "\n{} not a terminal — nothing was added. Re-run with {} to accept.",
            "·".dimmed(),
            "--yes".bold()
        );
        return Ok(false);
    }
    super::panel_edit::confirm("Add this to the project?")
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

// ────────────────────────────────────────────────────────────── publisher

pub fn run_publisher(args: &PublisherArgs) -> Result<()> {
    match &args.cmd {
        None | Some(PublisherCmd::Show {}) => {
            match publisher::public_key() {
                Some(key) => {
                    crate::outln!("{:<12}{}", "you".dimmed(), publisher::fingerprint(&key));
                    crate::outln!("{:<12}{}", "".dimmed(), key.to_hex().dimmed());
                    crate::outln!(
                        "{:<12}{}",
                        "".dimmed(),
                        "share this line so others can recognize your bundles".dimmed()
                    );
                }
                None => crate::outln!(
                    "{}",
                    "no publishing key yet — one is created the first time you run \
                     `agentstack share`"
                        .dimmed()
                ),
            }
            let known = publisher::known();
            crate::outln!();
            if known.is_empty() {
                crate::outln!(
                    "{}",
                    "you recognize no publishers yet — a signed bundle will still be \
                     reviewable, just not shortened"
                        .dimmed()
                );
            } else {
                crate::outln!("{}", "you recognize".dimmed());
                for k in known {
                    let short = agentstack_trust::sign::PublicKey::from_hex(&k.key)
                        .map(|p| publisher::fingerprint(&p))
                        .unwrap_or_else(|| "(unreadable key)".into());
                    crate::outln!("  {:<20}{}", k.label.bold(), short.dimmed());
                }
            }
            Ok(())
        }
        Some(PublisherCmd::Trust { key, label }) => {
            let parsed = agentstack_trust::sign::PublicKey::from_hex(key).ok_or_else(|| {
                anyhow::anyhow!(
                    "that is not a valid public key (expected 64 hex characters). \
                     Nothing was recorded."
                )
            })?;
            publisher::remember(&parsed, label)?;
            crate::outln!(
                "{} recognizing {} as {}",
                "✓".green(),
                publisher::fingerprint(&parsed),
                label.bold()
            );
            crate::outln!(
                "  {}",
                "their bundles will now say so on the review card. It still opens.".dimmed()
            );
            Ok(())
        }
    }
}
