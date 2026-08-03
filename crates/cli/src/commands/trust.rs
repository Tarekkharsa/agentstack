//! `agentstack trust` — the human gate every activation path checks.
//!
//! `connect` registers one global gateway per harness; `mcp --auto-project`
//! then discovers whatever manifest the current repo carries. This command is
//! what stands between "cloned a repo" and "that repo's manifest spawns stdio
//! servers and receives secrets": trust is granted per project, pinned to the
//! manifest's content digest, and shown to the human as the list of things the
//! manifest would actually run.

use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::cli::TrustArgs;
use crate::manifest::ServerType;
use crate::trust::{self, PriorSurface, SurfaceItem, TrustState, TrustStore};

/// Threads the P14 re-trust diff through the consent review. In diff mode it
/// holds the last consented surface keyed by `(kind, name)`; [`mark`] returns
/// the two-char marker to print before each item's line — `"+ "` added,
/// `"~ "` changed, `"  "` unchanged — and remembers which prior items it saw so
/// [`removed`] can report the rest as `- removed`. In flat mode (`prior` is
/// `None`: first-ever trust, or an older entry with no snapshot) every marker is
/// the plain two-space indent, so the review reads exactly as it did before
/// P14. Either way it accumulates the CURRENT surface, which the caller then
/// persists so the *next* re-trust has something to diff against.
///
/// [`mark`]: ReviewDiff::mark
/// [`removed`]: ReviewDiff::removed
struct ReviewDiff {
    /// `(kind, name) -> identity` from the last consented surface, or `None` in
    /// flat mode.
    prior: Option<HashMap<(String, String), String>>,
    /// The prior surface in its recorded order, for a stable `removed` pass.
    prior_order: Vec<SurfaceItem>,
    seen: HashSet<(String, String)>,
    /// The surface being reviewed now — handed to `trust_with_snapshot`.
    current: Vec<SurfaceItem>,
    /// The marker each `current` item was rendered with, index-aligned.
    ///
    /// Kept beside the surface rather than inside `SurfaceItem` on purpose:
    /// `SurfaceItem` is a `trust`-crate serde record that a re-gate diffs
    /// against, and a marker is presentation — it describes *this* review, not
    /// the consented surface, so persisting it would put display state into the
    /// thing consent is bound to. The grouped detail body tallies these; it
    /// never recomputes what changed.
    marks: Vec<&'static str>,
}

impl ReviewDiff {
    fn new(prior: PriorSurface) -> Self {
        // Only a recorded prior turns on diff markers; NeverTrusted and
        // Untracked both render flat.
        let (map, order) = match prior {
            PriorSurface::Recorded(items) => {
                let map = items
                    .iter()
                    .map(|it| ((it.kind.clone(), it.name.clone()), it.identity.clone()))
                    .collect();
                (Some(map), items)
            }
            _ => (None, Vec::new()),
        };
        Self {
            prior: map,
            prior_order: order,
            seen: HashSet::new(),
            current: Vec::new(),
            marks: Vec::new(),
        }
    }

    fn diffing(&self) -> bool {
        self.prior.is_some()
    }

    /// Record a reviewed item and return its two-char line marker. Called
    /// exactly once per item, in render order.
    fn mark(&mut self, kind: &str, name: &str, identity: &str) -> &'static str {
        self.mark_pinned(kind, name, identity, None)
    }

    /// `mark`, additionally recording the content digest this item is pinned
    /// to. Only the kinds whose bytes live outside the manifest carry one —
    /// skills and instructions — because they are the kinds a re-gate has to
    /// diff. The pin never affects the marker: it is not part of the diff key,
    /// so a re-lock that changes only the pin is not a `~ changed` surface.
    fn mark_pinned(
        &mut self,
        kind: &str,
        name: &str,
        identity: &str,
        pin: Option<String>,
    ) -> &'static str {
        self.current.push(SurfaceItem {
            kind: kind.to_string(),
            name: name.to_string(),
            identity: identity.to_string(),
            pin,
        });
        let marker = match &self.prior {
            None => "  ",
            Some(prior) => {
                let key = (kind.to_string(), name.to_string());
                self.seen.insert(key.clone());
                match prior.get(&key) {
                    None => "+ ",
                    Some(prev) if prev != identity => "~ ",
                    Some(_) => "  ",
                }
            }
        };
        self.marks.push(marker);
        marker
    }

    /// Prior items no marker was requested for — removed since the last trust.
    /// Empty in flat mode (`prior_order` is empty there).
    fn removed(&self) -> Vec<&SurfaceItem> {
        self.prior_order
            .iter()
            .filter(|it| !self.seen.contains(&(it.kind.clone(), it.name.clone())))
            .collect()
    }

    /// Tally one capability kind's markers, for that group's header line.
    ///
    /// Purely a fold over the markers already handed to the per-item lines, so
    /// a group header can never claim a change the lines below it do not show.
    fn group_counts(&self, kind: &str) -> GroupCounts {
        let mut counts = GroupCounts::default();
        for (item, marker) in self.current.iter().zip(&self.marks) {
            if item.kind != kind {
                continue;
            }
            match *marker {
                "+ " => counts.added += 1,
                "~ " => counts.changed += 1,
                _ => counts.unchanged += 1,
            }
        }
        counts.removed = self.removed().iter().filter(|it| it.kind == kind).count();
        counts
    }
}

/// One capability group's change tally: how many of its items were added,
/// changed, left alone, or dropped since the last consent.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
struct GroupCounts {
    added: usize,
    changed: usize,
    unchanged: usize,
    removed: usize,
}

impl GroupCounts {
    fn total(&self) -> usize {
        self.added + self.changed + self.unchanged
    }

    /// The group's own marker, the same three words the per-item `change`
    /// markers use: `added` when the whole group is new, `changed` when
    /// anything moved, `unchanged` when nothing did.
    fn change(&self) -> &'static str {
        if self.changed == 0 && self.removed == 0 && self.added == 0 {
            "unchanged"
        } else if self.changed == 0 && self.removed == 0 && self.unchanged == 0 {
            "added"
        } else {
            "changed"
        }
    }

    /// The trailing summary for a terminal group header, or `None` when there
    /// is nothing to say — a first-ever trust (flat mode) has no prior surface
    /// to compare against, and an untouched group should not be decorated with
    /// a tally that only restates "nothing happened".
    fn header_suffix(&self, diffing: bool) -> Option<String> {
        if !diffing || self.change() == "unchanged" {
            return None;
        }
        let mut parts = Vec::new();
        if self.added > 0 {
            parts.push(format!("+{} added", self.added));
        }
        if self.changed > 0 {
            parts.push(format!("~{} changed", self.changed));
        }
        if self.unchanged > 0 {
            parts.push(format!("{} unchanged", self.unchanged));
        }
        if self.removed > 0 {
            parts.push(format!("-{} removed", self.removed));
        }
        Some(format!("   [{}]", parts.join(", ")))
    }
}

// ---- Identity strings, shared by both walks --------------------------------
//
// `grant_probed` records these through `diff.mark(...)`; `preview_value`
// recomputes them for the machine-readable card (`trust-card-diff-v1`). They
// must agree BYTE FOR BYTE: the preview decides "added / changed / unchanged"
// by comparing its own string against the one the grant persisted, so a second
// construction site would make every re-review lie the day one of the two is
// edited. Hence one small pure function per kind, with no display formatting of
// its own — the mark call sites stay exactly where they are (the `:review`
// structure check anchors on them).

/// What the review records for a server whose reference does not resolve. The
/// error text is display only: two different failures are the same fact here,
/// and folding the message into the identity would read as `~ changed` every
/// time the wording moved.
const UNRESOLVABLE_SERVER_IDENTITY: &str = "unresolvable";

/// Instruction fragments are keyed by name and have no finer identity to
/// record, so they only ever read as added or removed. Their PIN, recorded
/// separately, is what carries their bytes into a re-gate.
const INSTRUCTION_IDENTITY: &str = "";

/// A stdio server's identity is the command line it runs — the thing the trust
/// gate exists for. Not the pin or origin annotation: pin drift is a hard
/// blocker of its own.
fn server_stdio_identity(server: &crate::manifest::Server) -> String {
    format!(
        "{} {}",
        server.command.as_deref().unwrap_or("?"),
        server.args.join(" ")
    )
}

/// An http server's identity is the URL it contacts.
///
/// Borrowed from `server` rather than cloned so a caller prints exactly the
/// string it marks; the returned `&str` lives as long as the borrow of the
/// server it came from, which outlasts every use in both walks.
fn server_http_identity(server: &crate::manifest::Server) -> &str {
    server.url.as_deref().unwrap_or("?")
}

/// Secrets are ONE aggregate item whose identity is the whole referenced set
/// (sorted by `referenced_secrets`), so adding or dropping any reference flips
/// the line to `~ changed`.
fn secrets_identity(refs: &[String]) -> String {
    refs.join(", ")
}

/// A repository-local executable is identified by the path label the review
/// shows; byte drift is caught by its verdict, not by the diff.
fn executable_identity(label: &str) -> &str {
    label
}

/// An extension's identity is where it installs, so a retarget reads as
/// `~ changed`.
fn extension_identity(ext: &crate::manifest::Extension) -> &str {
    &ext.target
}

/// A workflow's identity is its sorted role set — the authority it requests —
/// so a roles widening reads as `~ changed` even with unchanged bytes.
fn workflow_identity(wf: &crate::manifest::Workflow) -> String {
    wf.roles_sorted_unique().join(", ")
}

/// A skill has no command or URL; its identity is where its body comes from,
/// so a source flip reads as `~ changed`. `None` means the declaration itself
/// names no locatable source — an empty inline block shadowing a library skill
/// (P19) — which both walks record as `?`. A resolver failure is NOT a `None`
/// input: identity is declared, never resolved. See [`declared_skill_origin`].
fn skill_identity(origin: Option<crate::resolve::SkillOrigin>) -> &'static str {
    match origin {
        Some(crate::resolve::SkillOrigin::Inline) => "inline",
        Some(crate::resolve::SkillOrigin::Library) => "library",
        None => "?",
    }
}

/// The " matching <m>" fragment both the hook's identity and its review line
/// carry — one construction so they can never describe different scopes.
fn hook_matcher_suffix(hook: &crate::manifest::Hook) -> String {
    match &hook.matcher {
        Some(mt) if !mt.is_empty() => format!(" matching {mt}"),
        _ => String::new(),
    }
}

/// The ", timeout <n>s" fragment; see [`hook_matcher_suffix`].
fn hook_timeout_suffix(hook: &crate::manifest::Hook) -> String {
    match hook.timeout {
        Some(t) => format!(", timeout {t}s"),
        None => String::new(),
    }
}

/// The command line a hook runs, args included.
fn hook_invocation(hook: &crate::manifest::Hook) -> String {
    let args = if hook.args.is_empty() {
        String::new()
    } else {
        format!(" {}", hook.args.join(" "))
    };
    format!("{}{args}", hook.command)
}

/// A hook's identity is the WHOLE invocation (event, matcher, command line,
/// timeout, targets): changing any of them must read as `~ changed` rather than
/// hide behind a stable name. `targets` stays raw here — two manifests that
/// differ only in wildcard-vs-explicit are different consents.
fn hook_identity(hook: &crate::manifest::Hook) -> String {
    format!(
        "{}{} runs {}{} → {}",
        hook.event,
        hook_matcher_suffix(hook),
        hook_invocation(hook),
        hook_timeout_suffix(hook),
        hook.targets.join(", ")
    )
}

/// A settings block's identity is its canonical (key-sorted) JSON, so any value
/// change reads as `~ changed` while a re-ordering of the same keys does not.
fn settings_identity(value: &serde_json::Value) -> String {
    canonical_json(value)
}

/// The requested policy is ONE aggregate item; any change to the requested set
/// flips it.
fn policy_identity(p: &crate::manifest::Policy) -> String {
    policy_requested_lines(p).join("\n")
}

/// The capability kinds the card groups by, with the plural label each group
/// header uses, in the order both walks render them.
///
/// One list, so the terminal's group order and the payload's `review.groups`
/// order cannot diverge: a panel that showed the same review in a different
/// order would be telling a second story about one consent moment. A kind
/// missing from this list is not dropped — `CardWalk::groups` appends any
/// unlisted kind it meets, so a ninth capability kind lands in the grouping
/// the day it starts marking, unlabelled rather than invisible.
const CARD_GROUP_ORDER: &[(&str, &str)] = &[
    ("server", "servers"),
    ("secrets", "secrets"),
    ("executable", "local executable content"),
    ("extension", "native extensions"),
    ("workflow", "workflows"),
    ("skill", "skills"),
    ("instruction", "instruction fragments"),
    ("hook", "hooks"),
    ("settings", "settings"),
    ("policy", "requested policy"),
];

/// The one closing question the card asks, carried in the payload so a panel
/// renders the same single yes the terminal does.
///
/// It is a constant, and there is exactly one of it, because "one card, one
/// yes" is the contract: grouping the detail body per capability must not
/// multiply the moments a human commits to something. The answer is given by
/// `agentstack trust --yes --consented-digest <surface_digest>` — bound to the
/// bytes this payload described, never to a group or an item.
const CARD_QUESTION: &str = "Trust this project — allow the capabilities above to activate here?";

/// What the preview says instead of a library server's live command line when
/// that definition no longer matches its lock pin. Shared by the `servers`
/// entry and the card item so the redaction cannot drift into two wordings.
const REDACTED_LIBRARY_SERVER: &str =
    "library definition does not match the lockfile pin — run `agentstack lock`, review the change, and re-run the preview";

/// One row of the structured consent card (`trust-card-diff-v1`) before it
/// becomes JSON.
struct CardItem<'a> {
    /// `kind` and `name` are borrowed for as long as the item is being built —
    /// `push` copies what it needs into JSON immediately, so nothing outlives
    /// the manifest or lockfile they came from.
    kind: &'a str,
    name: &'a str,
    /// The identity the GRANT walk records for this item — what the change
    /// marker is computed from. RAW, exactly like `ReviewDiff::mark`'s
    /// argument: two different hostile values must never collide after
    /// sanitizing.
    identity: String,
    /// What the payload may DISCLOSE, when that is not the identity. Set only
    /// where the preview redacts (a drifted library server): the marker is
    /// still computed from the live identity, because saying that something
    /// changed discloses nothing, while emitting the bytes the consent digest
    /// does not cover would.
    shown: Option<&'a str>,
    /// Command lines this item runs; hosts it contacts; secret references it
    /// may read. Empty for the kinds that do none of those.
    runs: Vec<String>,
    contacts: Vec<String>,
    may_read: Vec<String>,
    /// The pin the CURRENT lockfile records, for the kinds whose bytes live
    /// outside the manifest.
    pin: Option<String>,
    /// Whether this kind carries a pin-to-pin diff at all (skills and
    /// instructions do; everything else emits `null`).
    pinned_kind: bool,
}

impl<'a> CardItem<'a> {
    fn new(kind: &'a str, name: &'a str, identity: impl Into<String>) -> Self {
        CardItem {
            kind,
            name,
            identity: identity.into(),
            shown: None,
            runs: Vec::new(),
            contacts: Vec::new(),
            may_read: Vec::new(),
            pin: None,
            pinned_kind: false,
        }
    }
}

/// Accumulates the preview's own read-only walk of the reviewed surface.
///
/// It is the mirror of [`ReviewDiff`], not a replacement for it: the grant walk
/// stays authoritative and stays where it is, and this recomputes the same
/// identities so a panel can render the same card. Everything it reads is a
/// pure read — the consent snapshot, the trust store, the content store, the
/// recognition index — because `trust --preview` may not write.
struct CardWalk {
    /// The surface the last consent recorded; empty when there is none, which
    /// is what makes every item read `added`.
    prior: Vec<SurfaceItem>,
    seen: HashSet<(String, String)>,
    items: Vec<serde_json::Value>,
    /// `None` when this machine has no readable recognition index. The
    /// per-item count is then `null` rather than a fabricated zero — the
    /// difference between "approved nowhere else" and "nothing to ask".
    index: Option<crate::recognition::Index>,
    project_key: String,
    store_root: PathBuf,
}

impl CardWalk {
    fn new(base: &Path, prior: Vec<SurfaceItem>) -> Self {
        CardWalk {
            prior,
            seen: HashSet::new(),
            items: Vec::new(),
            index: crate::recognition::Index::load_existing(),
            project_key: trust::key_for(base),
            store_root: crate::store::Store::default_store().root().to_path_buf(),
        }
    }

    /// Record one item, computing its marker exactly as
    /// [`ReviewDiff::mark_pinned`] does: same key, same raw comparison, same
    /// three answers. Sanitizing happens HERE rather than at the call sites so
    /// a new kind cannot forget it.
    fn push(&mut self, item: CardItem) {
        let prior = self
            .prior
            .iter()
            .find(|p| p.kind == item.kind && p.name == item.name);
        let change = match prior {
            None => "added",
            Some(p) if p.identity != item.identity => "changed",
            Some(_) => "unchanged",
        };
        let prior_pin = prior.and_then(|p| p.pin.clone());
        // Pin-to-pin, never pin-to-live: locating live bytes reaches git
        // worktree materialization, and this command must not write. See
        // `regate::diff_between_pins`.
        let diff = if item.pinned_kind {
            let computed = match (&prior_pin, &item.pin) {
                (Some(before), Some(after)) => {
                    crate::regate::diff_between_pins(&self.store_root, before, after)
                }
                _ => crate::regate::PinDiff::NoSnapshot,
            };
            crate::regate::pin_diff_json(&computed, crate::regate::DIFF_LINE_CAP)
        } else {
            serde_json::Value::Null
        };
        // Display information only. Nothing downstream may treat this as an
        // input to a decision — the per-project yes is unchanged by it.
        let recognized = match (&self.index, &item.pin) {
            (Some(index), Some(pin)) => index.others(pin, &self.project_key).into(),
            _ => serde_json::Value::Null,
        };
        let clean = |values: &[String]| -> Vec<String> {
            values
                .iter()
                .map(|v| crate::text::sanitize_line(v))
                .collect()
        };
        self.seen
            .insert((item.kind.to_string(), item.name.to_string()));
        self.items.push(serde_json::json!({
            "kind": item.kind,
            "name": crate::text::sanitize_line(item.name),
            "change": change,
            "identity": crate::text::sanitize_line(item.shown.unwrap_or(&item.identity)),
            "runs": clean(&item.runs),
            "contacts": clean(&item.contacts),
            "may_read": clean(&item.may_read),
            "pin": item.pin,
            "prior_pin": prior_pin,
            "recognized_other_projects": recognized,
            "diff": diff,
        }));
    }

    /// The detail body, grouped per capability (`trust-card-groups-v1`).
    ///
    /// This is **presentation over the flat list, not a second list**: a group
    /// carries INDICES into `review.items` / `review.removed` rather than
    /// copies of them. That is the structural reason grouping cannot become
    /// granularity — there is nowhere for a group to hold a fact of its own, no
    /// per-group question, and no per-group answer. A consumer that renders
    /// groups and a consumer that renders the flat list are reading the same
    /// bytes.
    ///
    /// `removed` is passed in rather than recomputed so the indices are into
    /// exactly the array the payload emits.
    fn groups(&self, removed: &[serde_json::Value]) -> Vec<serde_json::Value> {
        let kind_of = |v: &serde_json::Value| v["kind"].as_str().unwrap_or_default().to_string();
        // Listed kinds first, in the shared order; then anything unlisted, in
        // the order it was met, so a new kind is unlabelled rather than absent.
        let mut order: Vec<String> = CARD_GROUP_ORDER
            .iter()
            .map(|(k, _)| k.to_string())
            .collect();
        for value in self.items.iter().chain(removed) {
            let kind = kind_of(value);
            if !order.contains(&kind) {
                order.push(kind);
            }
        }
        let mut groups = Vec::new();
        for kind in order {
            let items: Vec<usize> = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, v)| kind_of(v) == kind)
                .map(|(ix, _)| ix)
                .collect();
            let gone: Vec<usize> = removed
                .iter()
                .enumerate()
                .filter(|(_, v)| kind_of(v) == kind)
                .map(|(ix, _)| ix)
                .collect();
            if items.is_empty() && gone.is_empty() {
                continue;
            }
            let mut counts = GroupCounts {
                removed: gone.len(),
                ..GroupCounts::default()
            };
            for ix in &items {
                match self.items[*ix]["change"].as_str() {
                    Some("added") => counts.added += 1,
                    Some("changed") => counts.changed += 1,
                    _ => counts.unchanged += 1,
                }
            }
            let label = CARD_GROUP_ORDER
                .iter()
                .find(|(k, _)| *k == kind)
                .map(|(_, label)| (*label).to_string())
                .unwrap_or_else(|| kind.clone());
            groups.push(serde_json::json!({
                "kind": kind,
                "label": label,
                "change": counts.change(),
                "counts": {
                    "added": counts.added,
                    "changed": counts.changed,
                    "unchanged": counts.unchanged,
                    "removed": counts.removed,
                    "total": counts.total(),
                },
                "items": items,
                "removed": gone,
            }));
        }
        groups
    }

    /// Prior items this walk never saw — removed since the last consent. The
    /// mirror of [`ReviewDiff::removed`].
    fn removed(&self) -> Vec<serde_json::Value> {
        self.prior
            .iter()
            .filter(|it| !self.seen.contains(&(it.kind.clone(), it.name.clone())))
            .map(|it| {
                serde_json::json!({
                    "kind": crate::text::sanitize_line(&it.kind),
                    "name": crate::text::sanitize_line(&it.name),
                    "identity": crate::text::sanitize_line(&it.identity),
                })
            })
            .collect()
    }
}

/// The skill origin named WITHOUT resolving anything: inline wins over the
/// central library, the same precedence activation applies.
///
/// This is the single authority for skill diff identity — the grant walk and
/// the read-only preview both call it, so they cannot diverge. Identity answers
/// "where does this body come from by declaration", never "did the resolver
/// succeed": whether the source could actually be reached is carried by the
/// verdict line (`offline — pin unverified`, `broken ref`, …) and by the pin,
/// never by the recorded identity. Deriving it from the resolver instead made a
/// freshly granted, uncached git-sourced library skill record `?` and then read
/// `changed` in the very next preview, forever offline.
fn declared_skill_origin(
    m: &crate::manifest::Manifest,
    library: &crate::library::Library,
    name: &str,
) -> Option<crate::resolve::SkillOrigin> {
    if let Some(skill) = m.skills.get(name) {
        // P19: an empty inline block shadowing a library skill is a resolve
        // ERROR, which the grant walk records as `?`. Mirror the refusal
        // rather than claiming an origin the authoritative walk will not name.
        if skill.path.is_none() && skill.git.is_none() && library.get(name).is_some() {
            return None;
        }
        return Some(crate::resolve::SkillOrigin::Inline);
    }
    library
        .get(name)
        .map(|_| crate::resolve::SkillOrigin::Library)
}

pub fn run(args: &TrustArgs) -> Result<()> {
    if args.list {
        return list();
    }
    let base = resolve_base(args.path.as_deref())?;
    if args.preview {
        return preview(&base);
    }
    if args.revoke {
        return revoke(&base);
    }
    grant(&base, args.yes, args.consented_digest.as_deref())
}

/// Read-only: emit the runtime surface a human would consent to, as JSON,
/// granting nothing. This is the summary an external UI (the t3code trust
/// dialog) shows before the user consents; the AUTHORITATIVE line-by-line
/// review and the consent gate stay in `grant_gated`, and the grant itself
/// (`trust --yes`) still self-gates on an unpinned surface — so this preview
/// deliberately shows the surface + category counts, not a re-derived blocker
/// verdict. Nothing here writes or fetches.
fn preview(base: &Path) -> Result<()> {
    let out = preview_value(base)?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// Build the same read-only, enveloped trust preview emitted by
/// `trust --preview`.
pub fn preview_value(base: &Path) -> Result<serde_json::Value> {
    let dir = crate::manifest::resolve_manifest_dir(base);
    // §7.2: ONE immutable read of the consent surface. The parsed display and
    // the digest below both derive from this snapshot, so an edit landing
    // mid-preview can never pair one file state's display with another's
    // digest — whatever the interleaving (including A→B→A), display and
    // digest describe the same bytes.
    let Some(snapshot) = trust::ConsentSnapshot::read(base) else {
        // No readable manifest: surface the same friendly first-contact error
        // the disk load path gives.
        crate::manifest::load_from_dir(&dir)?;
        anyhow::bail!("manifest disappeared while previewing {}", base.display());
    };
    let loaded = load_snapshot_manifest(&snapshot, &dir)?;
    let m = &loaded.manifest;
    // The lock pins are part of the consented surface: parse them from the
    // SAME snapshot bytes the digest covers, never a second disk read.
    let lock = lock_from_snapshot(&snapshot, &dir)?;

    // State is judged against the SNAPSHOT digest, not a fresh disk read, so
    // the state chip describes the same bytes as the display and the digest.
    let surface_digest = snapshot.digest();
    let state = match trust::check_digest(base, Some(&surface_digest)) {
        trust::TrustState::Trusted => "trusted",
        trust::TrustState::Changed => "drifted",
        trust::TrustState::Untrusted => "untrusted",
    };
    let prior = trust::prior_surface(base);
    let re_trust = !matches!(prior, trust::PriorSurface::NeverTrusted);
    // `trust-card-diff-v1`: the same walk the review card renders, recomputed
    // read-only. A prior surface with no items (never trusted, or an older
    // entry that recorded none) leaves `prior_recorded` false and every item
    // reading `added` — the honest answer when there is nothing to compare to.
    let prior_items: Vec<SurfaceItem> = match prior {
        trust::PriorSurface::Recorded(items) => items,
        _ => Vec::new(),
    };
    let prior_recorded = !prior_items.is_empty();
    let mut card = CardWalk::new(base, prior_items);

    // The gateway's actual runtime surface — library refs resolve exactly as
    // they will at gateway time. Display strings are sanitized (hostile input).
    let library = crate::library::Library::load_default_or_warn();
    let lib_home = crate::util::paths::lib_home();
    let effective_servers = crate::resolve::effective_runtime_servers(m, &library, &lib_home, None);
    let mut server_blockers: Vec<serde_json::Value> = Vec::new();
    let mut servers: Vec<serde_json::Value> = Vec::new();
    for (name, resolved) in &effective_servers {
        match resolved {
            Ok(r) => {
                // A library-backed definition resolves from the LIVE central
                // library, but the digest binds only the lock pin. Displaying
                // a definition that doesn't match the pin would show the
                // consenting human content the digest does not cover (an
                // external UI would then bind consent to bytes nobody is
                // granting) — so an unpinned or drifted library server renders
                // as unverified instead of leaking the live definition into
                // the surface.
                let pinned_ok = match r.origin {
                    crate::resolve::ServerOrigin::Inline => true,
                    crate::resolve::ServerOrigin::Library => lock
                        .get_server(name)
                        .is_some_and(|entry| entry.checksum.hex() == r.checksum),
                };
                // Computed BEFORE the redaction check, and kept out of the
                // payload when redacting: the card still needs to say whether
                // this changed, and "something changed" discloses nothing.
                let identity = match r.server.server_type {
                    crate::manifest::ServerType::Stdio => server_stdio_identity(&r.server),
                    crate::manifest::ServerType::Http => {
                        server_http_identity(&r.server).to_string()
                    }
                };
                if !pinned_ok {
                    server_blockers.push(serde_json::json!({
                        "name": crate::text::sanitize_line(name),
                        "reason": "library definition does not match the lockfile pin",
                        "fix": "agentstack lock",
                    }));
                    servers.push(serde_json::json!({
                        "name": crate::text::sanitize_line(name),
                        "kind": "unverified",
                        "target": REDACTED_LIBRARY_SERVER,
                    }));
                    // No `runs` / `contacts` either: those ARE the redacted
                    // bytes, one field further down.
                    let mut item = CardItem::new("server", name, identity);
                    item.shown = Some(REDACTED_LIBRARY_SERVER);
                    card.push(item);
                    continue;
                }
                let (kind, target) = match r.server.server_type {
                    crate::manifest::ServerType::Stdio => ("stdio", identity.trim().to_string()),
                    crate::manifest::ServerType::Http => {
                        ("http", r.server.url.clone().unwrap_or_default())
                    }
                };
                servers.push(serde_json::json!({
                    "name": crate::text::sanitize_line(name),
                    "kind": kind,
                    "target": crate::text::sanitize_line(&target),
                }));
                let mut item = CardItem::new("server", name, identity);
                match r.server.server_type {
                    crate::manifest::ServerType::Stdio => item.runs.push(target),
                    crate::manifest::ServerType::Http => item.contacts.push(target),
                }
                card.push(item);
            }
            Err(e) => {
                server_blockers.push(serde_json::json!({
                    "name": crate::text::sanitize_line(name),
                    "reason": crate::text::sanitize_line(&e.to_string()),
                    "fix": "edit-manifest",
                }));
                servers.push(serde_json::json!({
                    "name": crate::text::sanitize_line(name),
                    "kind": "unresolvable",
                    "target": crate::text::sanitize_line(&e.to_string()),
                }));
                card.push(CardItem::new("server", name, UNRESOLVABLE_SERVER_IDENTITY));
            }
        }
    }

    // The trust grant also verifies repository-local executable content. Carry
    // server-specific failures in the machine preview so an external consent
    // screen can disable a grant that is known to fail and point at the exact
    // declaration. This is read-only and uses the same resolver/verdict path as
    // the authoritative grant review below.
    let executable_servers: Vec<(String, crate::manifest::Server)> = effective_servers
        .iter()
        .filter_map(|(name, resolved)| {
            resolved
                .as_ref()
                .ok()
                .map(|resolved| (name.clone(), resolved.server.clone()))
        })
        .collect();
    // Card order mirrors the grant walk's (servers, secrets, executables, …),
    // so a panel rendering `review.items` in order shows the same sequence the
    // terminal does.
    let secrets: Vec<String> = m.referenced_secrets();
    if !secrets.is_empty() {
        let mut item = CardItem::new("secrets", "", secrets_identity(&secrets));
        item.may_read = secrets.clone();
        card.push(item);
    }

    let exec_statuses =
        crate::executable::executable_lock_statuses(&dir, &executable_servers, &lock);
    for (label, _) in &exec_statuses {
        card.push(CardItem::new(
            "executable",
            label,
            executable_identity(label),
        ));
    }
    for (name, status) in exec_statuses {
        match crate::verify::executable_verdict(&status) {
            crate::verify::Verdict::Ok => {}
            crate::verify::Verdict::Unpinned => {
                server_blockers.push(serde_json::json!({
                    "name": crate::text::sanitize_line(&name),
                    "reason": "local executable content is not pinned yet",
                    "fix": "agentstack lock",
                }));
            }
            crate::verify::Verdict::Block(reason) => {
                server_blockers.push(serde_json::json!({
                    "name": crate::text::sanitize_line(&name),
                    "reason": crate::text::sanitize_line(&reason),
                    "fix": "edit-manifest",
                }));
            }
        }
    }

    // The COMPLETE reviewed surface, by name — not just counts. What an
    // external consent screen renders must be the same item list the
    // interactive review prints; a preview that collapsed workflows or
    // extensions into a number would let a user consent to code they never
    // saw named. All names arrive from repo content — hostile input — so
    // display copies are sanitized.
    let skills: Vec<String> = review_skill_names(m)
        .iter()
        .map(|n| crate::text::sanitize_line(n))
        .collect();
    let workflows: Vec<serde_json::Value> = m
        .workflows
        .iter()
        .map(|(name, w)| {
            serde_json::json!({
                "name": crate::text::sanitize_line(name),
                "roles": w.roles.iter().map(|r| crate::text::sanitize_line(r)).collect::<Vec<_>>(),
            })
        })
        .collect();
    let extensions: Vec<serde_json::Value> = m
        .extensions
        .iter()
        .map(|(name, e)| {
            serde_json::json!({
                "name": crate::text::sanitize_line(name),
                "target": crate::text::sanitize_line(&e.target),
            })
        })
        .collect();
    let instructions: Vec<String> = m
        .instructions
        .iter()
        .filter(|(_, i)| !i.from_user_layer)
        .map(|(name, _)| crate::text::sanitize_line(name))
        .collect();

    // §7.2: `surface_digest` (computed above, from the same snapshot the
    // display was parsed from) is exactly what a later grant must present as
    // `--consented-digest` — so "the surface shown" and "the bytes granted"
    // can never diverge without the digest flipping.
    // `trust-review-card-v1`: the kinds the terminal card discloses that this
    // read-only preview previously did not. All three are computed from the
    // manifest alone — no resolver, no store, no network, no worktree
    // materialization — which is exactly why they can be added here without
    // giving a read-only command the walk's disk-writing behaviour.
    //
    // Hooks matter most: they are an EXECUTABLE kind, and until now they were
    // absent from the machine-readable surface entirely, so a panel built on
    // this JSON would have shown a project's executable surface as smaller
    // than it is.
    let hooks: Vec<serde_json::Value> = m
        .hooks
        .iter()
        .map(|(name, h)| {
            serde_json::json!({
                "name": crate::text::sanitize_line(name),
                "event": crate::text::sanitize_line(&h.event),
                "matcher": h.matcher.as_deref().map(crate::text::sanitize_line),
                "runs": crate::text::sanitize_line(&hook_invocation(h)),
                "targets": h.targets.iter().map(|t| crate::text::sanitize_line(t)).collect::<Vec<_>>(),
                "executable": true,
            })
        })
        .collect();
    let settings: Vec<serde_json::Value> = m
        .settings
        .iter()
        .map(|(adapter, value)| {
            let mut keys: Vec<String> = value
                .as_object()
                .map(|o| o.keys().map(|k| crate::text::sanitize_line(k)).collect())
                .unwrap_or_default();
            keys.sort();
            serde_json::json!({
                "adapter": crate::text::sanitize_line(adapter),
                "sets": keys,
            })
        })
        .collect();
    let policy: Vec<String> = policy_requested_lines(&m.policy)
        .iter()
        .map(|l| crate::text::sanitize_line(l))
        .collect();

    // The rest of the card, in the grant walk's order. Everything here is a
    // manifest or lockfile read: no resolver, no store worktree, no network —
    // which is what keeps `trust --preview` read-only while still recomputing
    // the identities the grant persists.
    for (name, ext) in &m.extensions {
        card.push(CardItem::new("extension", name, extension_identity(ext)));
    }
    for (name, wf) in &m.workflows {
        card.push(CardItem::new("workflow", name, workflow_identity(wf)));
    }
    for name in review_skill_names(m) {
        let mut item = CardItem::new(
            "skill",
            &name,
            skill_identity(declared_skill_origin(m, &library, &name)),
        );
        item.pin = lock.get(&name).map(|e| e.checksum.hex().to_string());
        item.pinned_kind = true;
        card.push(item);
    }
    for (name, _) in m.instructions.iter().filter(|(_, i)| !i.from_user_layer) {
        let mut item = CardItem::new("instruction", name, INSTRUCTION_IDENTITY);
        item.pin = lock
            .get_instruction(name)
            .map(|e| e.checksum.hex().to_string());
        item.pinned_kind = true;
        card.push(item);
    }
    for (name, hook) in &m.hooks {
        let mut item = CardItem::new("hook", name, hook_identity(hook));
        // A hook runs a command at the user's permission, so it belongs in
        // `runs` beside the stdio servers — the same reasoning the terminal
        // card's executable count applies.
        item.runs.push(hook_invocation(hook));
        card.push(item);
    }
    for (adapter, value) in &m.settings {
        card.push(CardItem::new("settings", adapter, settings_identity(value)));
    }
    if !policy_requested_lines(&m.policy).is_empty() {
        card.push(CardItem::new("policy", "", policy_identity(&m.policy)));
    }

    // Computed once, in this order: the groups index into the very arrays the
    // payload emits, so they cannot point at a different list.
    let card_removed = card.removed();
    let card_groups = card.groups(&card_removed);

    // §7.2: `surface_digest` (computed above, from the same snapshot the
    // display was parsed from) is exactly what a later grant must present as
    // `--consented-digest` — so "the surface shown" and "the bytes granted"
    // can never diverge without the digest flipping.
    let out = serde_json::json!({
        "path": base.display().to_string(),
        "state": state,
        "re_trust": re_trust,
        "surface_digest": surface_digest,
        "servers": servers,
        "server_blockers": server_blockers,
        "secrets": secrets,
        "skills": skills,
        "workflows": workflows,
        "extensions": extensions,
        "instructions": instructions,
        "hooks": hooks,
        "settings": settings,
        "policy_requested": policy,
        "machine_policy_ceiling": crate::util::paths::agentstack_home()
            .join("agentstack.toml")
            .display()
            .to_string(),
        "counts": {
            "skills": skills.len(),
            "workflows": workflows.len(),
            "extensions": extensions.len(),
            "instructions": instructions.len(),
            "hooks": hooks.len(),
            "settings": settings.len(),
        },
        // `trust-card-diff-v1`. Always present from this binary on: the inner
        // fields degrade (a missing snapshot, a missing index, a project that
        // was never trusted), the key does not disappear, so a panel gates on
        // the feature name once instead of sniffing per project.
        "review": {
            "re_review": re_trust,
            "prior_recorded": prior_recorded,
            "items": card.items,
            "removed": card_removed,
            // `trust-card-groups-v1`. The SAME items, grouped per capability
            // by index, plus the one closing question. Additive: `items` and
            // `removed` keep their `trust-card-diff-v1` shape and order, so a
            // consumer that predates this reads exactly what it read before.
            "groups": card_groups,
            "question": CARD_QUESTION,
        },
    });
    Ok(crate::ui_contract::envelope(out))
}

/// Resolve the project base to act on: walk up from the given path (or cwd) so
/// `agentstack trust` works from a subdirectory too.
fn resolve_base(path: Option<&Path>) -> Result<PathBuf> {
    let start = match path {
        Some(p) => p
            .canonicalize()
            .with_context(|| format!("no such directory: {}", p.display()))?,
        None => std::env::current_dir()?,
    };
    crate::manifest::discover_project_base(&start).with_context(|| {
        format!(
            "no agentstack manifest at or above {} — run `agentstack init` first",
            start.display()
        )
    })
}

/// Parse the manifest layers out of a [`trust::ConsentSnapshot`]'s captured
/// bytes — the only way the review may load them, so what the human reads and
/// what the digest identifies are always the same bytes.
fn load_snapshot_manifest(
    snapshot: &trust::ConsentSnapshot,
    dir: &Path,
) -> Result<crate::manifest::LoadedManifest> {
    let manifest_text = std::str::from_utf8(&snapshot.manifest).with_context(|| {
        format!(
            "{} is not valid UTF-8",
            dir.join("agentstack.toml").display()
        )
    })?;
    let local_text = snapshot
        .local
        .as_deref()
        .map(std::str::from_utf8)
        .transpose()
        .with_context(|| {
            format!(
                "{} is not valid UTF-8",
                dir.join("agentstack.local.toml").display()
            )
        })?;
    crate::manifest::load_from_contents(dir, manifest_text, local_text)
}

/// Parse the lockfile from the same snapshot (absent → empty default lock),
/// mirroring [`load_snapshot_manifest`]: the pins the review verifies against
/// are exactly the pin bytes the consent digest covers.
fn lock_from_snapshot(snapshot: &trust::ConsentSnapshot, dir: &Path) -> Result<crate::lock::Lock> {
    let path = crate::lock::Lock::path(dir);
    match snapshot.lock.as_deref() {
        None => Ok(crate::lock::Lock::default()),
        Some(bytes) => {
            let text = std::str::from_utf8(bytes)
                .with_context(|| format!("{} is not valid UTF-8", path.display()))?;
            crate::lock::Lock::parse(text, &path)
        }
    }
}

fn grant(base: &Path, yes: bool, consented: Option<&str>) -> Result<()> {
    grant_gated(base, yes, consented, std::io::stdin().is_terminal(), None)
}

/// Extra review lines and the one question, supplied by the funnel. Held by
/// reference through the single grant path — it never becomes a second one.
pub(crate) struct ConsentCard {
    pub lines: Vec<String>,
    pub question: String,
    /// The answer, when it is supplied instead of read from stdin — the same
    /// kind of injected probe as `grant_gated`'s `interactive`, and for the
    /// same reason: a consent gate whose refusal path cannot be exercised in a
    /// test is a consent gate whose refusal path is unverified. Production
    /// always passes `None` and prompts; only the funnel's test seam sets it.
    pub answer: Option<bool>,
}

/// Review-and-grant with the funnel's card folded into the same screen. The
/// only entry point besides `trust` itself, and it reaches the identical
/// [`grant_gated`] — same surface, same digest, same recorded event.
pub(crate) fn grant_with_card(
    base: &Path,
    yes: bool,
    interactive: bool,
    card: &ConsentCard,
) -> Result<()> {
    grant_gated(base, yes, None, interactive, Some(card))
}

/// The grant path with the TTY probe injected, so the non-interactive consent
/// gate is testable without a real terminal. `interactive` is whether stdin is
/// a TTY; production passes `std::io::stdin().is_terminal()`.
///
/// Typing `agentstack trust` at a terminal IS the consent (direnv-allow style),
/// so an interactive session is unchanged. When stdin is NOT a terminal — a
/// pipe, a here-string, or an agent driving the shell — the command refuses
/// unless `--yes` explicitly acknowledges the review AND `--consented-digest`
/// binds that acknowledgement to the exact previewed bytes (§7.2): `--yes`
/// alone would let any RPC caller grant without anyone having seen the
/// surface, which is precisely the UI-enforcement gap this closes.
///
/// Honesty about the probe (independent review, 2026-07-23): `isatty(stdin)`
/// proves stdin is a terminal DEVICE, not that a human is attending it — a
/// process that allocates a PTY (`script`, `expect`, Python's `pty`) reads as
/// interactive. That is accepted, not overlooked: the trust store is a plain
/// file under the user's own account, so any same-user process able to stage
/// a PTY could equally write `trust.json` directly. The gate's enforceable
/// job is narrower and holds — headless callers (RPC servers, plain shell
/// pipes) cannot grant without presenting the reviewed digest — and the real
/// boundary against a hostile same-user process is the OS user account, as
/// `docs/ENFORCEMENT.md` states.
///
/// The entire review below renders from ONE [`trust::ConsentSnapshot`], and
/// the no-digest grant records that snapshot's digest — never a re-read — so
/// bytes swapped in mid-review are not blessed: the store then holds the
/// reviewed digest, the project reads `Changed`, and use sites fail closed.
/// `card` extends this one consent screen for the Phase 1 funnel instead of
/// giving the funnel a screen of its own: extra review lines are printed with
/// the surface (so the combined preview shows everything the separate steps
/// show, and never less), and the funnel's single confirmation is asked HERE,
/// after the whole review and before any grant. There is exactly one place a
/// human says yes to a project, whichever verb brought them to it.
fn grant_gated(
    base: &Path,
    yes: bool,
    consented: Option<&str>,
    interactive: bool,
    card: Option<&ConsentCard>,
) -> Result<()> {
    grant_probed(base, yes, consented, interactive, card, None, None)
}

/// The `trust` path (no funnel card) with re-gate answers injected — the entry
/// integration tests drive. Kept card-free so [`ConsentCard`], which belongs to
/// the funnel, stays crate-private.
pub fn grant_with_answers(
    base: &Path,
    yes: bool,
    consented: Option<&str>,
    interactive: bool,
    probe: Option<&ReGateProbe>,
) -> Result<()> {
    grant_probed(base, yes, consented, interactive, None, probe, None)
}

/// [`grant_with_answers`] with a hook fired AFTER the review is fully staged
/// (the displayed digest captured, the diff rendered, the answers collected)
/// and BEFORE the commit re-reads the bytes to pin them. It exists to witness
/// the F5 TOCTOU end to end: a test swaps the live content inside the hook —
/// the real human-scale window an adversarial writer would use — and asserts
/// the commit refuses to pin bytes different from those displayed. It is the
/// same injection philosophy as [`ReGateProbe`] and the `interactive` probe:
/// a consent path whose time-of-check/time-of-use window cannot be driven in a
/// test is a window whose guard is unwitnessed. Production never passes a hook.
pub fn grant_with_swap_between_review_and_commit(
    base: &Path,
    interactive: bool,
    probe: Option<&ReGateProbe>,
    on_reviewed: &dyn Fn(),
) -> Result<()> {
    grant_probed(
        base,
        false,
        None,
        interactive,
        None,
        probe,
        Some(on_reviewed),
    )
}

/// [`grant_gated`] with the re-gate answers injectable. See [`ReGateProbe`].
/// `on_reviewed`, when present, fires once between the staged review and the
/// commit — a test-only seam for the F5 swap witness; production passes `None`.
pub(crate) fn grant_probed(
    base: &Path,
    yes: bool,
    consented: Option<&str>,
    interactive: bool,
    card: Option<&ConsentCard>,
    probe: Option<&ReGateProbe>,
    on_reviewed: Option<&dyn Fn()>,
) -> Result<()> {
    let dir = crate::manifest::resolve_manifest_dir(base);
    let Some(snapshot) = trust::ConsentSnapshot::read(base) else {
        // No readable manifest: surface the same friendly first-contact error
        // the disk load path gives.
        crate::manifest::load_from_dir(&dir)?;
        anyhow::bail!("manifest disappeared while reviewing {}", base.display());
    };
    let loaded = load_snapshot_manifest(&snapshot, &dir)?;
    let m = &loaded.manifest;
    let surface_digest = snapshot.digest();

    // Name the whole consequence, not one consumer. The gateway used to be the
    // only thing this gate fed, but `session start` (and every other activation
    // path) refuses on an untrusted project too — describing it as gateway-only
    // made that refusal read as a bug.
    println!(
        "Reviewing {} — approving this lets its capabilities activate.\n",
        base.display().to_string().bold()
    );

    // P14: when this project was trusted before, mark the review against the
    // surface it last consented to — so a `git pull`'s new `evil` server reads
    // as `+ added` instead of hiding in a flat re-list. First-ever trust (and
    // an older entry that recorded no snapshot) stays the flat full review.
    let prior = trust::prior_surface(base);
    let untracked = matches!(prior, PriorSurface::Untracked);
    // Kept alongside the diff machinery: the re-gate card reads each item's
    // recorded PIN from here, which is what lets it diff against the bytes the
    // human approved rather than against the lock that drifted.
    let prior_items: Vec<SurfaceItem> = match &prior {
        PriorSurface::Recorded(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut diff = ReviewDiff::new(prior);
    if diff.diffing() {
        println!(
            "Re-trust — marking what changed since you last trusted this ({} added, {} changed, {} removed):\n",
            "+".green(),
            "~".yellow(),
            "-".red()
        );
    } else if untracked {
        println!(
            "Re-trust — no reviewed-surface snapshot was recorded last time, so this is a full re-review, not a diff.\n"
        );
    }

    // Preview the gateway's actual runtime surface, not just the inline
    // `[servers.*]` tables: library name refs resolve here exactly like they
    // will at gateway time, so the human reviews everything auto-mode may run.
    let library = crate::library::Library::load_default_or_warn();
    let lib_home = crate::util::paths::lib_home();
    // A broken lockfile must fail the trust review loudly: its pins are part
    // of what the human is consenting to, and the gateway will refuse
    // library-backed servers under an unreadable lock anyway. Parsed from the
    // snapshot bytes, so the pins reviewed are the pins the digest covers.
    let lock = lock_from_snapshot(&snapshot, &dir)?;
    let servers = crate::resolve::effective_runtime_servers(m, &library, &lib_home, None);
    // Phase 2: the review is COMPOSED into `body` first and rendered after, so
    // the glanceable card can lead with what this project runs, contacts, and
    // may read — facts that are only known once the whole surface has been
    // walked. Nothing is dropped in the process: every line that printed before
    // is pushed here and printed below, which is what makes "the card never
    // discloses less than the old preview" a structural property of the
    // rendering order rather than a claim about the copy.
    let mut body: Vec<String> = Vec::new();
    macro_rules! say {
        ($($t:tt)*) => { body.push(format!($($t)*)) };
    }
    // "One card, one yes": the detail body is GROUPED per capability, so a
    // reviewer reads one kind at a time instead of a flat run of lines. Each
    // group's header remembers its index here because the group's change tally
    // is not knowable until every item under it has been marked — it is
    // appended after the walk, never computed a second time.
    let mut group_headers: Vec<(&'static str, usize)> = Vec::new();
    macro_rules! group {
        ($kind:literal, $($t:tt)*) => {{
            group_headers.push(($kind, body.len()));
            body.push(format!($($t)*));
        }};
    }
    say!("This project declares — review what auto-mode may run/contact:");
    if servers.is_empty() {
        say!("  (no servers)");
    } else {
        group!("server", "  servers (spawned or contacted over MCP):");
    }
    // Trusting pins the lock bytes into the trust digest, so trusting over a
    // drifted or unpinned surface would bless pins that don't match content
    // (or bless no pin at all). Everything that must be lock-verified at use
    // time therefore has to be pinned and matching BEFORE trust is granted:
    // `agentstack lock` is a prerequisite of `agentstack trust`.
    let mut blockers: Vec<(String, String)> = Vec::new();
    // Re-gate questions the walk stages but does not ask; see PendingAnswer.
    let mut pending: Vec<PendingAnswer> = Vec::new();
    for (name, resolved) in &servers {
        // This review is the consent screen for content that may be hostile —
        // display copies are sanitized; diff identities and lookups stay RAW
        // (two different hostile values must never collide after cleaning).
        let disp = crate::text::sanitize_line(name);
        let r = match resolved {
            Ok(r) => r,
            Err(e) => {
                let mk = diff.mark("server", name, UNRESOLVABLE_SERVER_IDENTITY);
                say!(
                    "{mk}{} {disp}: unresolvable ({})",
                    "✗".red(),
                    crate::text::sanitize_line(&e.to_string())
                );
                blockers.push((name.clone(), format!("broken server ref — {e}")));
                continue;
            }
        };
        let origin = match r.origin {
            crate::resolve::ServerOrigin::Inline => String::new(),
            crate::resolve::ServerOrigin::Library => match lock.get_server(name) {
                Some(entry) if entry.checksum.hex() == r.checksum => {
                    "   [library, pinned]".to_string()
                }
                Some(_) => {
                    blockers.push((
                        name.clone(),
                        "library server definition DRIFTED from lock".to_string(),
                    ));
                    format!("   [library, {}]", "DRIFTED from lock".red())
                }
                None => {
                    blockers.push((
                        name.clone(),
                        "library server unpinned — run `agentstack lock`".to_string(),
                    ));
                    format!("   [library, {}]", "unpinned".red())
                }
            },
        };
        match r.server.server_type {
            // A stdio server is arbitrary local code execution — the thing the
            // trust gate exists for. Call it out explicitly. The diff identity
            // is the command line (what actually runs), not the pin/origin
            // annotation — pin drift is already a hard blocker below.
            ServerType::Stdio => {
                let command_line = server_stdio_identity(&r.server);
                let mk = diff.mark("server", name, &command_line);
                say!(
                    "{mk}{} {disp}: runs `{}`{origin}",
                    "▶".yellow(),
                    crate::text::sanitize_line(&command_line)
                );
            }
            ServerType::Http => {
                let url = server_http_identity(&r.server);
                let mk = diff.mark("server", name, url);
                say!(
                    "{mk}{} {disp}: contacts {}{origin}",
                    "→".cyan(),
                    crate::text::sanitize_line(url)
                );
            }
        }
    }
    let refs = m.referenced_secrets();
    if !refs.is_empty() {
        // Secrets are one aggregate line; its identity is the (sorted, from
        // `referenced_secrets`) set, so adding or dropping any ref flips the
        // whole line to `~ changed`.
        let joined = secrets_identity(&refs);
        let mk = diff.mark("secrets", "", &joined);
        say!(
            "{mk}secrets referenced: {}",
            crate::text::sanitize_line(&joined)
        );
    }

    // D3 (contract §8): the repository-local executable surface, pinned by
    // current bytes. Ruling: an unpinned repo-relative executable BLOCKS
    // trust — the lock is a prerequisite of trust, so an unpinned declared
    // executable means the lock is incomplete, and trusting would bless
    // ungoverned local code. What stays honestly unbound (the interpreter/
    // harness binary itself, imports outside a declared root) is labeled.
    let exec_servers: Vec<(String, crate::manifest::Server)> = servers
        .iter()
        .filter_map(|(n, r)| r.as_ref().ok().map(|r| (n.clone(), r.server.clone())))
        .collect();
    let exec_statuses = crate::executable::executable_lock_statuses(&dir, &exec_servers, &lock);
    if !exec_statuses.is_empty() {
        group!(
            "executable",
            "  local executable content (pinned by current bytes):"
        );
        for (label, status) in &exec_statuses {
            let disp = crate::text::sanitize_line(label);
            // An executable is identified by its path (the label the review
            // shows); byte drift is caught by the verdict below, not the diff.
            let mk = diff.mark("executable", label, executable_identity(label));
            match crate::verify::executable_verdict(status) {
                crate::verify::Verdict::Ok => say!("{mk}· {disp}   [pinned]"),
                crate::verify::Verdict::Unpinned => {
                    say!("{mk}{} {disp}   [{}]", "✗".red(), "unpinned".red());
                    blockers.push((
                        label.clone(),
                        "local executable unpinned — run `agentstack lock`".to_string(),
                    ));
                }
                crate::verify::Verdict::Block(why) => {
                    say!("{mk}{} {disp}   [{}]", "✗".red(), why.red());
                    blockers.push((label.clone(), why));
                }
            }
        }
        say!(
            "  (unbound, by design: interpreter/harness binaries from $PATH, and imports outside a declared integrity root)"
        );
    }

    // Native extensions (D6): executable add-on code delivered into a
    // harness's own extension directory. It runs INSIDE the harness process,
    // outside the policy ceiling — the pin is the only governance there is,
    // so unpinned AND drifted both block, like the D3 executable surface.
    if !m.extensions.is_empty() {
        group!(
            "extension",
            "  native extensions (EXECUTABLE — run inside the harness process; agentstack pins the bytes but cannot govern them at runtime):"
        );
        let store = crate::store::Store::default_store();
        for (name, ext) in &m.extensions {
            use crate::resolve::{ExtensionLockStatus, ExtensionOrigin};
            let disp = crate::text::sanitize_line(name);
            let dest = format!("→ {}", crate::text::sanitize_line(&ext.target));
            // The extension's identity for the diff is its target (where it
            // installs); a retarget shows as `~ changed`.
            let mk = diff.mark("extension", name, extension_identity(ext));
            // Read-only review: never fetch a git source here. An un-cached git
            // extension surfaces as offline, exactly like a skill.
            let report = crate::resolve::extension_lock_status(
                name,
                ext,
                &dir,
                &library,
                &lib_home,
                &store,
                &lock,
                crate::resolve::ResolveMode::NoFetch,
            );
            let origin_word = match report.origin {
                Some(ExtensionOrigin::Inline) => "inline",
                Some(ExtensionOrigin::Library) => "library",
                None => "?",
            };
            match report.status {
                ExtensionLockStatus::Matches => {
                    say!(
                        "{mk}{} {disp} {dest}   [{origin_word}, pinned]",
                        "▶".yellow()
                    );
                }
                ExtensionLockStatus::MissingLockEntry => {
                    say!(
                        "{mk}{} {disp} {dest}   [{origin_word}, {}]",
                        "✗".red(),
                        "unpinned".red()
                    );
                    blockers.push((
                        name.clone(),
                        "extension unpinned — run `agentstack lock`".to_string(),
                    ));
                }
                ExtensionLockStatus::ChecksumDrift { .. }
                | ExtensionLockStatus::RevDrift { .. } => {
                    say!(
                        "{mk}{} {disp} {dest}   [{origin_word}, {}]",
                        "✗".red(),
                        "DRIFTED from lock".red()
                    );
                    blockers.push((
                        name.clone(),
                        "extension content drifted from lock".to_string(),
                    ));
                }
                ExtensionLockStatus::TargetDrift { locked, .. } => {
                    say!(
                        "{mk}{} {disp} {dest}   [{origin_word}, {}]",
                        "✗".red(),
                        format!(
                            "RETARGETED since locked (was '{}')",
                            crate::text::sanitize_line(&locked)
                        )
                        .red()
                    );
                    blockers.push((
                        name.clone(),
                        "extension target changed since locked — run `agentstack lock`".to_string(),
                    ));
                }
                // Reproducibility can't be checked offline; not a blocker —
                // same posture as skills' un-cached git sources.
                ExtensionLockStatus::NotAvailableOffline { .. } => say!(
                    "{mk}{} {disp} {dest}   [{origin_word}, {}]",
                    "▶".yellow(),
                    "offline — pin unverified".yellow()
                ),
                ExtensionLockStatus::ResolveFailed { error } => {
                    say!("{mk}{} {disp} {dest}: {}", "✗".red(), error.red());
                    blockers.push((name.clone(), error));
                }
            }
        }
    }

    // Governed workflows (D7 W1): orchestration code agentstack ITSELF will
    // execute, spawning agent runs under the declared role profiles. Stronger
    // than skills (context, not code), different in kind from extensions (a
    // harness runs those, ungoverned; agentstack runs this, gated and
    // sandboxed — which is precisely why the gate stands in front of it).
    // Unpinned, drifted, roles-drifted, and unresolvable all block, like the
    // extension surface; the diff identity is the sorted role set, so a roles
    // widening reads as `~ changed` even with unchanged bytes.
    if !m.workflows.is_empty() {
        group!(
            "workflow",
            "  workflows (ORCHESTRATION CODE — spawns agent runs under the declared roles; agentstack executes this, gated and sandboxed):"
        );
        let store = crate::store::Store::default_store();
        for (name, wf) in &m.workflows {
            use crate::resolve::WorkflowLockStatus;
            let disp = crate::text::sanitize_line(name);
            let roles = wf.roles_sorted_unique();
            let roles_joined = workflow_identity(wf);
            let dest = format!(
                "→ roles: {}",
                if roles.is_empty() {
                    "(none — spawns nothing)".to_string()
                } else {
                    crate::text::sanitize_line(&roles_joined)
                }
            );
            let mk = diff.mark("workflow", name, &roles_joined);
            // Read-only review: never fetch a git source here. An un-cached
            // git workflow surfaces as offline, exactly like a skill.
            let status = crate::resolve::workflow_lock_status(
                name,
                wf,
                &dir,
                &store,
                &lock,
                crate::resolve::ResolveMode::NoFetch,
            );
            match status {
                WorkflowLockStatus::Matches => {
                    say!("{mk}{} {disp} {dest}   [pinned]", "▶".yellow());
                }
                WorkflowLockStatus::MissingLockEntry => {
                    say!("{mk}{} {disp} {dest}   [{}]", "✗".red(), "unpinned".red());
                    blockers.push((
                        name.clone(),
                        "workflow unpinned — run `agentstack lock`".to_string(),
                    ));
                }
                WorkflowLockStatus::ChecksumDrift { .. } | WorkflowLockStatus::RevDrift { .. } => {
                    say!(
                        "{mk}{} {disp} {dest}   [{}]",
                        "✗".red(),
                        "DRIFTED from lock".red()
                    );
                    blockers.push((
                        name.clone(),
                        "workflow content drifted from lock".to_string(),
                    ));
                }
                WorkflowLockStatus::RolesDrift { locked, .. } => {
                    say!(
                        "{mk}{} {disp} {dest}   [{}]",
                        "✗".red(),
                        format!(
                            "ROLES CHANGED since locked (was: {})",
                            crate::text::sanitize_line(&locked.join(", "))
                        )
                        .red()
                    );
                    blockers.push((
                        name.clone(),
                        "workflow roles changed since locked — run `agentstack lock`".to_string(),
                    ));
                }
                // Reproducibility can't be checked offline; not a blocker —
                // same posture as skills' and extensions' un-cached git sources.
                WorkflowLockStatus::NotAvailableOffline { .. } => say!(
                    "{mk}{} {disp} {dest}   [{}]",
                    "▶".yellow(),
                    "offline — pin unverified".yellow()
                ),
                WorkflowLockStatus::ResolveFailed { error } => {
                    say!(
                        "{mk}{} {disp} {dest}: {}",
                        "✗".red(),
                        crate::text::sanitize_line(&error).red()
                    );
                    blockers.push((name.clone(), error));
                }
            }
            // F13: when this script was authored from an approved blueprint,
            // show the SHAPE that was approved right here — this gate is the
            // one that authorizes execution, and until now it showed only
            // bytes while the graph the user actually reviewed lived in a chat
            // message. A reviewer should not have to remember a picture.
            if let Some(declared) = &wf.blueprint {
                for line in blueprint_review_lines(&dir, name, declared, &lock, &mut blockers) {
                    say!("      {line}");
                }
            }
        }
    }

    // Skills, reviewed like servers: name + origin + pin status. Their bodies
    // are exactly the bytes the trust digest does NOT cover, so the pin is
    // the only thing binding what the human reviews to what gets served.
    let skill_names = review_skill_names(m);
    if !skill_names.is_empty() {
        group!("skill", "  skills loadable over MCP:");
        let store = crate::store::Store::default_store();
        for name in &skill_names {
            let disp = crate::text::sanitize_line(name);
            let report = crate::resolve::skill_lock_status(
                name,
                m,
                &dir,
                &library,
                &lib_home,
                &store,
                &lock,
                crate::resolve::ResolveMode::NoFetch,
            );
            use crate::resolve::{SkillLockStatus, SkillOrigin};
            // Identity is DECLARED, not resolved — same function the preview
            // uses, so the two walks cannot disagree. `report.status` still
            // carries whether the source was reachable; recording the resolver's
            // failure as identity `?` made every grant read `changed` next
            // preview for a skill that is merely uncached.
            let origin_word = skill_identity(declared_skill_origin(m, &library, name));
            // A skill has no command/url; its diff identity is where its body
            // comes from (inline vs library), so a source flip shows `~ changed`.
            // The PIN — the lock checksum of the bytes being consented to — is
            // recorded alongside, not folded into the identity: it is what a
            // later re-gate needs to find the approved bytes in the content
            // store and render a real diff instead of "digest mismatch".
            let mk = diff.mark_pinned(
                "skill",
                name,
                origin_word,
                lock.get(name).map(|e| e.checksum.hex().to_string()),
            );
            match &report.status {
                SkillLockStatus::Matches => {
                    say!("{mk}· {disp}   [{origin_word}, pinned]");
                }
                SkillLockStatus::ChecksumDrift { .. } | SkillLockStatus::RevDrift { .. } => {
                    // Phase 2: say WHAT changed, not just that something did.
                    // The approved bytes are looked up by the pin the last
                    // consent recorded, so this compares against what the human
                    // actually said yes to — not against the current lock,
                    // which is what drifted in the first place.
                    let pin = prior_pin_for(&prior_items, "skill", name)
                        .or_else(|| lock.get(name).map(|e| e.checksum.hex().to_string()));
                    let live = live_skill_dir(name, m, &library, &dir, &lib_home, &store);
                    // F5: hash the live tree NOW, before the diff below reads
                    // it — this digest is what `accept` is allowed to pin. A
                    // change that lands after this line makes the commit
                    // point refuse rather than pin un-displayed bytes.
                    let displayed = live
                        .as_ref()
                        .and_then(|l| crate::store::dir_digest(l).ok())
                        .map(|d| d.hex().to_string());
                    let pin_diff = match (&pin, &live) {
                        (Some(pin), Some(live)) => {
                            crate::regate::diff_against_pin(store.root(), pin, live)
                        }
                        _ => crate::regate::PinDiff::NoSnapshot,
                    };
                    let headline = crate::regate::headline(&pin_diff)
                        .unwrap_or_else(|| "changed since you approved it".to_string());
                    say!(
                        "{mk}{} {disp}   [{origin_word}, {}]",
                        "✗".red(),
                        headline.red()
                    );
                    for line in crate::regate::render_lines(&pin_diff, crate::regate::DIFF_LINE_CAP)
                    {
                        say!("  {line}");
                    }
                    blockers.push((name.clone(), "skill content drifted from lock".to_string()));
                    // STAGE the question — do not ask it here. The walk has not
                    // printed anything yet (Slice A composes into `body` and
                    // renders afterwards), so prompting at this point would ask
                    // the human to judge a change before showing them the diff
                    // that is, at this instant, still sitting unrendered in
                    // `body`. The answer loop runs after the render.
                    if !matches!(pin_diff, crate::regate::PinDiff::NoSnapshot) {
                        pending.push(PendingAnswer {
                            kind: "skill",
                            name: name.clone(),
                            // Which blocker this answer clears. Keyed by INDEX,
                            // not by name: `blockers` is `(name, why)` with no
                            // kind, so a skill and an instruction sharing a
                            // name would clear each other's.
                            blocker_ix: blockers.len() - 1,
                            approved_pin: pin.clone(),
                            live: live.clone(),
                            displayed: displayed.clone(),
                            headline: headline.clone(),
                        });
                    }
                }
                SkillLockStatus::MissingLockEntry => match report.origin {
                    // An inline skill's bytes live in the repo under review —
                    // unpinned means trusting would leave them ungoverned.
                    Some(SkillOrigin::Inline) => {
                        say!("{mk}{} {disp}   [inline, {}]", "✗".red(), "unpinned".red());
                        blockers.push((
                            name.clone(),
                            "inline skill unpinned — run `agentstack lock`".to_string(),
                        ));
                    }
                    // A library skill's bytes are the user's own curated,
                    // scan-gated content — worth pinning, not worth blocking.
                    _ => say!(
                        "{mk}· {disp}   [{origin_word}, {}]",
                        "unpinned — run `agentstack lock`".yellow()
                    ),
                },
                // Reproducibility can't be checked offline; not a blocker.
                SkillLockStatus::NotAvailableOffline { .. } => say!(
                    "{mk}· {disp}   [{origin_word}, {}]",
                    "offline — pin unverified".yellow()
                ),
                SkillLockStatus::ResolveFailed { error } => {
                    say!(
                        "{mk}{} {disp}: broken ref ({})",
                        "✗".red(),
                        crate::text::sanitize_line(error)
                    );
                    blockers.push((name.clone(), format!("broken ref — {error}")));
                }
            }
        }
    }

    // Instruction fragments, same review: they compile into CLAUDE.md /
    // AGENTS.md — straight into agent context — and their bytes are repo
    // content the trust digest doesn't cover. The pin is what binds them.
    // (grant loads the project manifest only, so machine-layer fragments
    // can't appear here; the filter guards the invariant regardless.)
    let instructions: Vec<_> = m
        .instructions
        .iter()
        .filter(|(_, i)| !i.from_user_layer)
        .collect();
    if !instructions.is_empty() {
        group!(
            "instruction",
            "  instruction fragments (compile into CLAUDE.md / AGENTS.md):"
        );
        for (name, instr) in instructions {
            let disp = crate::text::sanitize_line(name);
            use crate::resolve::InstructionLockStatus;
            // Instructions are keyed by name; there is no finer identity to
            // show, so they only ever read as added or removed. The pin is
            // what makes a re-gate able to show which lines of the fragment
            // moved — the identity alone could never carry that.
            let mk = diff.mark_pinned(
                "instruction",
                name,
                INSTRUCTION_IDENTITY,
                lock.get_instruction(name)
                    .map(|e| e.checksum.hex().to_string()),
            );
            match crate::resolve::instruction_lock_status_with(name, instr, &dir, &lock, &library) {
                InstructionLockStatus::Matches => say!("{mk}· {disp}   [pinned]"),
                InstructionLockStatus::ChecksumDrift { .. } => {
                    // Instructions now deposit their bytes at pin time
                    // (`Store::pin_instruction`), so this shows the changed
                    // LINES of the fragment — the same treatment skills get,
                    // and the reason the always-degraded fallback is gone.
                    let pin = prior_pin_for(&prior_items, "instruction", name).or_else(|| {
                        lock.get_instruction(name)
                            .map(|e| e.checksum.hex().to_string())
                    });
                    let live = crate::instructions::base_source(name, instr, &dir, &library)
                        .unwrap_or_else(|| dir.join(name));
                    // F5, instruction flavor: one read, hashed before the
                    // diff renders — the digest `accept` is allowed to pin.
                    let displayed = std::fs::read(&live)
                        .ok()
                        .map(|b| agentstack_core::digest::sha256_hex(&b));
                    // Same path-derived singleton the skills walk uses; that
                    // binding is scoped to its own block.
                    let store = crate::store::Store::default_store();
                    let pin_diff = match &pin {
                        Some(pin) => crate::regate::diff_against_pin(store.root(), pin, &live),
                        None => crate::regate::PinDiff::NoSnapshot,
                    };
                    let headline = crate::regate::headline(&pin_diff)
                        .unwrap_or_else(|| "changed since you approved it".to_string());
                    say!("{mk}{} {disp}   [{}]", "✗".red(), headline.red());
                    for line in crate::regate::render_lines(&pin_diff, crate::regate::DIFF_LINE_CAP)
                    {
                        say!("  {line}");
                    }
                    blockers.push((
                        name.clone(),
                        "instruction content drifted from lock".to_string(),
                    ));
                    if !matches!(pin_diff, crate::regate::PinDiff::NoSnapshot) {
                        pending.push(PendingAnswer {
                            kind: "instruction",
                            name: name.clone(),
                            blocker_ix: blockers.len() - 1,
                            approved_pin: pin.clone(),
                            live: Some(live),
                            displayed,
                            headline: headline.clone(),
                        });
                    }
                }
                InstructionLockStatus::MissingLockEntry => {
                    say!("{mk}{} {disp}   [{}]", "✗".red(), "unpinned".red());
                    blockers.push((
                        name.clone(),
                        "instruction unpinned — run `agentstack lock`".to_string(),
                    ));
                }
                InstructionLockStatus::ResolveFailed { error } => {
                    say!(
                        "{mk}{} {disp}: broken ref ({})",
                        "✗".red(),
                        crate::text::sanitize_line(&error)
                    );
                    blockers.push((name.clone(), format!("broken ref — {error}")));
                }
            }
        }
    }

    // Hooks: an EXECUTABLE kind. Declaring or editing one re-gates trust
    // (the manifest bytes move, so the digest moves) — but until Phase 2 this
    // screen said nothing about them, so the human was re-asked without being
    // shown what they were re-approving. That is a consent surprise on the one
    // kind that runs commands in or around the harness at user permission, and
    // strategy v2 gives hooks the full ceremony with no compressed path. The
    // diff identity is the whole invocation (event, matcher, command line,
    // timeout, targets): changing ANY of them must read as `~ changed`, never
    // hide behind a stable name.
    if !m.hooks.is_empty() {
        group!(
            "hook",
            "  hooks (EXECUTABLE — agentstack compiles these into each harness's native config; the harness runs them at your permission, and agentstack does not govern them at runtime):"
        );
        for (name, hook) in &m.hooks {
            let disp = crate::text::sanitize_line(name);
            let invocation = hook_invocation(hook);
            let matcher = hook_matcher_suffix(hook);
            let timeout = hook_timeout_suffix(hook);
            // `targets` defaults to the wildcard `["*"]`, which is manifest
            // syntax, not something a consent screen may make the reader
            // decode — a bare `[*]` is the widest possible scope rendered as
            // the least alarming glyph. Say what it means. The diff identity
            // keeps the RAW targets: two manifests that differ only in
            // wildcard-vs-explicit must still read as `~ changed`.
            let targets_disp = if hook.targets.iter().any(|t| t == "*") {
                "every hook-capable CLI".to_string()
            } else {
                hook.targets.join(", ")
            };
            let identity = hook_identity(hook);
            let mk = diff.mark("hook", name, &identity);
            say!(
                "{mk}{} {disp}: on {} runs `{}`{}   [in {}]",
                "▶".yellow(),
                crate::text::sanitize_line(&format!("{}{matcher}", hook.event)),
                crate::text::sanitize_line(&invocation),
                crate::text::sanitize_line(&timeout),
                crate::text::sanitize_line(&targets_disp)
            );
        }
    }

    // Settings: inert per-CLI native config (permissions, feature flags) merged
    // into each harness's settings file. Not executable, but a settings value
    // can widen what a harness will do without asking — `ENFORCEMENT.md` says
    // editing one re-gates consent, so the review has to show it. The identity
    // is the canonical JSON of the whole per-adapter object, so any value
    // change reads as `~ changed`.
    if !m.settings.is_empty() {
        group!(
            "settings",
            "  settings (merged into each CLI's own config file):"
        );
        for (adapter, value) in &m.settings {
            let disp = crate::text::sanitize_line(adapter);
            // Canonical, key-sorted rendering: two objects that differ only in
            // key order are the same consent, and must not read as changed.
            let identity = settings_identity(value);
            let keys = match value.as_object() {
                Some(o) => {
                    let mut k: Vec<&str> = o.keys().map(|s| s.as_str()).collect();
                    k.sort_unstable();
                    k.join(", ")
                }
                None => identity.clone(),
            };
            let mk = diff.mark("settings", adapter, &identity);
            say!("{mk}· {disp}: sets {}", crate::text::sanitize_line(&keys));
        }
    }

    // Requested policy, shown at the trust boundary (ARCHITECTURE: "review
    // shows … policy changes"). Display-only: a bundle's policy can only
    // narrow — the machine layer caps everything at runtime regardless — so
    // there is nothing here to block on, but the human should see what the
    // repo asks for before blessing it.
    review_policy(&m.policy, &mut diff, &mut body);

    // P14: anything the last consented surface carried that is gone now. Printed
    // as part of the review (before the blocker bail) so the human sees the full
    // diff. A scoped block ends the borrow of `diff` before its `current` moves.
    {
        let removed = diff.removed();
        if !removed.is_empty() {
            say!("  no longer present (was trusted before):");
            for it in removed {
                let label = if it.name.is_empty() {
                    it.kind.clone()
                } else {
                    format!("{} {}", it.kind, crate::text::sanitize_line(&it.name))
                };
                let detail = if it.identity.is_empty() {
                    String::new()
                } else {
                    format!("  ({})", crate::text::sanitize_line(&it.identity))
                };
                say!("{} {label}{detail}", "-".red());
            }
        }
    }

    // Every item is marked by now, so each group's header can carry its own
    // change tally. Folded from the SAME markers the per-item lines printed —
    // a header can never claim a change the lines below it do not show.
    //
    // Two kinds deliberately have no header: `secrets` and `policy` are single
    // aggregate lines whose own `+` / `~` marker already IS the group's change
    // marker, and a tally beside it would only restate it.
    for (kind, ix) in &group_headers {
        if let Some(suffix) = diff.group_counts(kind).header_suffix(diff.diffing()) {
            body[*ix].push_str(&suffix);
        }
    }

    // ---- The card ----------------------------------------------------------
    // Two to five plain lines, answering the only questions the moment asks:
    // what runs, what it reaches, what it may read, and whether the bytes are
    // the ones being reviewed. Computed from the surface just walked, so it can
    // never describe a different set than the detail below it. Then the detail,
    // unabridged — the card summarizes the review, it does not replace it.
    for line in card_summary_lines(&diff.current, blockers.len()) {
        println!("{line}");
    }
    // Recognition shortens the card's BODY and nothing else: it adds one line
    // saying how much of this has already been reviewed on this machine, and
    // changes no outcome, no gate, and no recorded event. An absent or corrupt
    // index simply produces no line.
    {
        let index = crate::recognition::Index::load();
        let key = trust::key_for(base);
        let known = crate::recognition::recognized_count(&index, &key, &diff.current);
        let elsewhere = crate::recognition::other_projects(&index, &key, &diff.current);
        if let Some(line) = crate::recognition::line(known, diff.current.len(), elsewhere) {
            println!("{line}");
        }
    }
    println!();
    for line in &body {
        println!("{line}");
    }

    // ---- The answer loop -----------------------------------------------
    // The only place a re-gate question is asked, and it asks NOTHING that
    // acts: every answer is recorded in memory and applied at the commit
    // point below. This is the staging contract
    // (`docs/design/consent-card.md`, "Answers stage; the single final yes
    // commits"), and this is the one function to audit for effects leaking
    // early — there are none between here and the commit.
    //
    // Placement is forced: after the render (so the human has seen the diff
    // they are judging) and before the blocker bail (so an answer can still
    // clear its blocker). `consented.is_none()` is load-bearing beyond the
    // usual non-interactive check — `--consented-digest` does NOT require
    // `--yes`, so a TTY caller passing only a digest is `interactive && !yes`
    // yet bound to a digest that accepting would invalidate.
    let mut answers: Vec<(usize, Answer)> = Vec::new();
    if let Some(probe) = probe {
        for (ix, p) in pending.iter().enumerate() {
            if let Some((_, a)) = probe.answers.iter().find(|(n, _)| *n == p.name) {
                answers.push((ix, *a));
            }
        }
    } else if interactive && !yes && consented.is_none() && !pending.is_empty() {
        println!(
            "\n{}",
            "This content changed since you approved it. For each item:".bold()
        );
        for (ix, p) in pending.iter().enumerate() {
            let disp = crate::text::sanitize_line(&p.name);
            let picked = crate::util::confirm::choose(
                &format!("\n  {} {disp} — {}", p.kind, p.headline),
                &[
                    ("a", "accept the change"),
                    ("k", "keep the approved version"),
                    ("b", "block this item"),
                ],
            )?;
            // `None` is not a fourth answer: it means nothing was decided, so
            // this item's blocker stays and the review refuses exactly as it
            // does today. Silence never resolves a consent question.
            match picked.as_deref() {
                Some("a") => answers.push((ix, Answer::Accept)),
                Some("k") => answers.push((ix, Answer::KeepPinned)),
                Some("b") => answers.push((ix, Answer::Block)),
                _ => {}
            }
        }
    }
    // Answers only ever REMOVE blockers; nothing here can add surface.
    {
        let cleared: Vec<usize> = answers
            .iter()
            .map(|(ix, _)| pending[*ix].blocker_ix)
            .collect();
        let mut keep = 0usize;
        blockers.retain(|_| {
            let this = keep;
            keep += 1;
            !cleared.contains(&this)
        });
    }

    if !blockers.is_empty() {
        // Names and reasons carry manifest/resolver text — hostile input, so
        // the summary sanitizes exactly like the per-line review above.
        let blockers: Vec<(String, String)> = blockers
            .iter()
            .map(|(name, why)| {
                (
                    crate::text::sanitize_line(name),
                    crate::text::sanitize_line(why),
                )
            })
            .collect();
        let width = blockers.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        let lines: Vec<String> = blockers
            .iter()
            .map(|(name, why)| format!("  {name:width$}  {why}"))
            .collect();
        let next = if blockers
            .iter()
            .all(|(_, why)| why.contains("agentstack lock"))
        {
            "Run `agentstack lock`, review the result, then `agentstack trust` again."
        } else {
            "Fix or remove the blocked declarations above. Then run `agentstack lock` for \
             anything marked unpinned and review again."
        };
        anyhow::bail!(
            "cannot trust {}: its loadable surface isn't fully pinned — {} {} locking or review:\n{}\n{}",
            base.display(),
            super::count(blockers.len(), "item"),
            if blockers.len() == 1 { "needs" } else { "need" },
            lines.join("\n"),
            next
        );
    }

    // The funnel's own review lines join the surface above, inside the same
    // screen, before the same gate — presentation is combined, the disclosure
    // is additive.
    if let Some(card) = card {
        for line in &card.lines {
            println!("{line}");
        }
    }

    // Consent gate: the review above is now fully printed. Trust is granted by
    // a human who read it — typing the command at a terminal IS that consent.
    // When stdin is not a terminal (a pipe, a here-string, an agent driving the
    // shell), there is no interactive consent, so refuse unless `--yes` was
    // passed to acknowledge the review explicitly. This runs BEFORE anything is
    // pinned or written, so a refusal leaves the trust store untouched — an
    // agent with shell access cannot self-trust a repo to defeat the
    // untrusted-means-inert gate.
    if !interactive && !yes {
        anyhow::bail!(
            "refusing to trust: stdin is not a terminal — review the declarations above and re-run interactively, or acknowledge non-interactively with --yes --consented-digest <surface_digest from `agentstack trust --preview`>"
        );
    }
    // §7.2: a non-interactive `--yes` must also present the digest of the
    // surface that was reviewed. Without it, "the user saw the review" would
    // be the caller's claim, not a checked fact.
    if !interactive && consented.is_none() {
        anyhow::bail!(
            "refusing to trust: --yes requires --consented-digest — run `agentstack trust --preview`, review the surface, and pass its `surface_digest` back"
        );
    }

    // The funnel asks its single question here — after the complete review,
    // before anything is granted or rendered. A refusal leaves the trust store
    // untouched, exactly like every other refusal on this path.
    if let Some(card) = card {
        if interactive && !yes {
            let said_yes = match card.answer {
                Some(answer) => answer,
                None => super::panel_edit::confirm(&card.question)?,
            };
            if !said_yes {
                anyhow::bail!("cancelled — nothing was granted or activated");
            }
        }
    }

    // `agentstack trust` has no closing confirmation — typing the command at a
    // terminal IS the consent, and that already happened, BEFORE the answers
    // above were given. Without this, N per-item answers would commit with no
    // further yes: exactly the many-moments shape the staging contract exists
    // to prevent. So a re-gate that collected answers asks once, here, at the
    // same point the funnel's card asks. A clean review, or a re-gate the human
    // answered nothing on, prompts nothing and is unchanged.
    if card.is_none() && !answers.is_empty() {
        let summary = answers
            .iter()
            .map(|(ix, a)| {
                format!(
                    "{} {}",
                    match a {
                        Answer::Accept => "accept",
                        Answer::KeepPinned => "keep approved version of",
                        Answer::Block => "block",
                    },
                    crate::text::sanitize_line(&pending[*ix].name)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let said_yes = match probe {
            Some(probe) => probe.confirm,
            None => crate::util::confirm::confirm(&format!("\nApply: {summary}?"))?,
        };
        if !said_yes {
            anyhow::bail!("cancelled — nothing was granted or changed");
        }
    }

    // Test-only F5 seam: the review is fully staged (every `displayed` digest
    // captured, the diff rendered, the answers collected) and nothing has
    // committed yet. A swap performed here is exactly the time-of-check /
    // time-of-use window an adversarial writer would use. Production passes
    // `None`; the witness overwrites the live content and asserts the commit
    // below refuses to pin it.
    if let Some(hook) = on_reviewed {
        hook();
    }

    // ---- The commit point ----------------------------------------------
    // Everything the answers imply happens from here down, in one place, after
    // every gate. Order matters and is witnessed:
    //
    //   1. accepted items re-pin (deposit → patch lock → write → recompute the
    //      digest), because accept CHANGES the bytes the consent digest covers;
    //   2. the grant records that digest;
    //   3. standing decisions are written LAST — `set_decision` is a no-op
    //      without a trust entry (it must never be a second grant
    //      constructor), so on a first-ever trust an answer written before the
    //      grant would be silently dropped.
    let mut effective_digest = surface_digest.clone();
    let accepted: Vec<&PendingAnswer> = answers
        .iter()
        .filter(|(_, a)| *a == Answer::Accept)
        .map(|(ix, _)| &pending[*ix])
        .collect();
    if !accepted.is_empty() {
        // Patch the lock parsed FROM THE SNAPSHOT — never a manifest-wide
        // re-lock. `agentstack lock` re-pins every kind, which would fold
        // un-consented pin moves — including the very items answered
        // keep-pinned or block — into the digest this grant is about to bless.
        let mut patched = lock_from_snapshot(&snapshot, &dir)?;
        // The review's `store` binding is scoped to the skills block; this is
        // the same path-derived singleton, not a second store.
        let store = crate::store::Store::default_store();
        for p in &accepted {
            let Some(live) = &p.live else { continue };
            // Each kind pins through its own act (`pin` for skill trees,
            // `pin_instruction` for fragment files — see `Store` for why the
            // two digest families never collapse into one function), and both
            // pass the F5 gate: the digest being pinned must be the digest
            // captured when the diff was displayed. This runs before
            // `patched.save` and before the grant, so a refusal leaves the
            // lock, the trust store, and the decisions exactly as they were.
            let checksum = match p.kind {
                "instruction" => {
                    // One read inside `pin_instruction`: the bytes deposited
                    // ARE the bytes hashed, so the returned digest names
                    // exactly what would be pinned.
                    let pinned = store.pin_instruction(live)?;
                    refuse_undisplayed(&p.name, p.displayed.as_deref(), pinned.hex())?;
                    // Instructions patch their own lock table. Routing this
                    // through `patched.skills` was F6: `accept` on an
                    // instruction re-gate errored out after consent (a file
                    // fed to `dir_digest`), and could never have recorded the
                    // answer anywhere the compiler reads.
                    if let Some(entry) = patched.instructions.iter_mut().find(|i| i.name == p.name)
                    {
                        entry.checksum = pinned.clone();
                    }
                    pinned.hex().to_string()
                }
                _ => {
                    let checksum = crate::store::dir_digest(live)?.hex().to_string();
                    refuse_undisplayed(&p.name, p.displayed.as_deref(), &checksum)?;
                    // Through `Store::pin`, so the newly approved bytes land in
                    // the content store and the NEXT re-gate can still show a
                    // diff.
                    let pinned = store.pin(&crate::store::Resolved {
                        path: live.clone(),
                        rev: None,
                        checksum: checksum.clone(),
                        fetched: false,
                        source_kind: "path",
                    })?;
                    if let Some(entry) = patched.skills.iter_mut().find(|s| s.name == p.name) {
                        entry.checksum = pinned;
                    }
                    checksum
                }
            };
            // The recorded surface must carry the pin the human just approved,
            // or the next re-gate would diff against the superseded one.
            if let Some(item) = diff
                .current
                .iter_mut()
                .find(|i| i.kind == p.kind && i.name == p.name)
            {
                item.pin = Some(checksum);
            }
        }
        patched.save(&dir)?;
        // Recompute, never re-read (§7.2). The manifest and local bytes are the
        // ones this review rendered from; only the lock moved, and these are
        // the exact bytes we just serialized — so a concurrent edit cannot
        // sneak into the digest this grant records. Same precedent `repin`
        // documents: computed from written content, never from a disk re-read.
        let lock_bytes = std::fs::read(crate::lock::Lock::path(&dir)).ok();
        effective_digest = trust::ConsentSnapshot {
            manifest: snapshot.manifest.clone(),
            local: snapshot.local.clone(),
            lock: lock_bytes,
        }
        .digest();
    }

    // Store the reviewed surface alongside the pin so the NEXT re-trust can
    // diff against it (P14). Display metadata only — it does not enter the
    // trust digest, so recording it never re-gates the project. When a
    // consented digest was presented (any mode), the grant is bound to it:
    // the trust crate refuses at the store-write point unless it still
    // matches the bytes on disk. Without one, the grant records the digest
    // of the SNAPSHOT this review rendered — never a fresh disk read — so a
    // mid-review byte swap leaves the project `Changed`, not blessed.
    let recorded_surface = diff.current;
    // Cloned before the surface moves into the grant; recognition needs its
    // pins and the grant consumes the vector.
    let recorded_for_recognition = recorded_surface.clone();
    let digest = match consented {
        Some(consented) => trust::trust_with_consent(base, recorded_surface, consented)?,
        None => {
            trust::trust_reviewed(base, effective_digest.clone(), recorded_surface)?;
            effective_digest
        }
    };

    // Recognition, after the grant: this project has now approved these exact
    // digests, which is what lets the NEXT project's card be shorter. Records
    // digests and project keys only — never content — and cannot fail the
    // grant, because a convenience must never be able to.
    crate::recognition::record(
        &trust::key_for(base),
        crate::recognition::digests_of(&recorded_for_recognition),
    );

    // Standing answers, last — see the ordering note above.
    for (ix, answer) in &answers {
        let p = &pending[*ix];
        let decision = match answer {
            // Accepting clears any prior standing answer for this item: the new
            // bytes ARE the approved ones now, so there is nothing to keep or
            // refuse.
            Answer::Accept => None,
            Answer::KeepPinned => p
                .approved_pin
                .clone()
                .map(|pin| trust::Decision::KeepPinned { pin }),
            Answer::Block => Some(trust::Decision::Blocked),
        };
        trust::set_decision(base, p.kind, &p.name, decision)?;
    }
    println!(
        "\n{} trusted at {digest}.\nEditing the manifest or lockfile invalidates this — re-run `agentstack trust` after reviewing changes.\nPinned skill/server content that drifts is blocked at use time until re-locked.\nWithdraw anytime with `agentstack trust --revoke`.",
        "✓".green()
    );
    Ok(())
}

/// A re-gate question the review walk STAGED but did not ask.
///
/// The separation is the whole staging contract in one type: the walk records
/// what could be asked, the render happens, and only then is anything asked —
/// and even then, nothing is acted on until the commit point. Answering happens
/// in one place, so there is one place to audit for "did an effect leak early".
struct PendingAnswer {
    /// `"skill"` / `"instruction"` — the surface kind, needed because
    /// `blockers` does not carry one.
    kind: &'static str,
    name: String,
    /// Index into `blockers`; the answer clears exactly this entry.
    blocker_ix: usize,
    /// The pin whose bytes the human previously approved — what `keep pinned`
    /// keeps, and what the shown diff was taken against.
    approved_pin: Option<String>,
    /// The live content directory (skill) or fragment file (instruction),
    /// re-pinned on `accept`.
    live: Option<PathBuf>,
    /// The digest of the live bytes AT THE MOMENT this question was staged —
    /// captured before the diff renders, so it names the content the human is
    /// about to judge. The commit point refuses to pin anything else (F5):
    /// between the render and the closing confirmation there is a human-scale
    /// window in which the live content can change, and without this field
    /// `accept` would hash whatever is on disk *then* — granting bytes nobody
    /// displayed. `None` (the bytes could not be hashed at staging time)
    /// makes accept refuse, which is the fail-closed direction.
    displayed: Option<String>,
    headline: String,
}

/// The F5 refusal, factored out so the binding is witnessable on its own:
/// `accept` commits the displayed digest or nothing. `fresh` is the digest of
/// the bytes the commit point is about to pin; anything other than an exact
/// match with what was staged at display time refuses — including a staging
/// failure (`displayed == None`), because "I could not hash what you looked
/// at" is not a license to pin something else.
fn refuse_undisplayed(name: &str, displayed: Option<&str>, fresh: &str) -> Result<()> {
    if displayed == Some(fresh) {
        return Ok(());
    }
    anyhow::bail!(
        "'{}' changed while you were reviewing — the content on disk no longer matches \
         the diff you were shown. Nothing was granted or changed; re-run `agentstack trust` \
         to review the current bytes.",
        crate::text::sanitize_line(name)
    )
}

/// What the human said about one staged question. Distinct from
/// [`trust::Decision`] because `Accept` exists here (it is a thing to do at the
/// commit point) but leaves no standing state to store afterwards.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    Accept,
    KeepPinned,
    Block,
}

/// Test seam for the re-gate answer loop, injected exactly like `grant_gated`'s
/// `interactive` probe and `ConsentCard::answer`, and for the same stated
/// reason: a consent path whose answers cannot be driven in a test is a consent
/// path whose staging contract is unverified. Production always passes `None`
/// and prompts.
pub struct ReGateProbe {
    /// Answers by item name; items absent from this list are left undecided,
    /// which must keep their blocker exactly as it is today.
    pub answers: Vec<(String, Answer)>,
    /// What the closing confirmation returns.
    pub confirm: bool,
}

/// The pin a PREVIOUS consent recorded for an item, if any.
///
/// Deliberately preferred over the current lock entry when rendering a re-gate
/// diff: the lock is what drifted, so diffing against it would answer "what
/// changed since the machine last re-pinned" when the reviewer asked "what
/// changed since *I* said yes". Entries recorded before pins existed return
/// `None` and degrade to the honest no-snapshot message.
fn prior_pin_for(prior: &[SurfaceItem], kind: &str, name: &str) -> Option<String> {
    prior
        .iter()
        .find(|i| i.kind == kind && i.name == name)
        .and_then(|i| i.pin.clone())
}

/// The directory a skill's bytes live in right now, without network access or
/// content digesting — read-only, through the same seams activation uses.
/// `None` when the source cannot be located locally (an un-cached git skill),
/// which the caller renders as "no snapshot" rather than guessing.
fn live_skill_dir(
    name: &str,
    m: &crate::manifest::Manifest,
    library: &crate::library::Library,
    // The project's manifest dir — an inline skill's `path` is relative to it.
    dir: &Path,
    lib_home: &Path,
    store: &crate::store::Store,
) -> Option<PathBuf> {
    // An inline declaration always wins over a same-named library skill, which
    // is the same precedence activation applies.
    if let Some(skill) = m.skills.get(name) {
        return store
            .resolve_path_only(skill, dir, None)
            .ok()
            .flatten()
            .map(|r| r.path);
    }
    library.get(name)?.body_dir(lib_home)
}

/// The card: two to five plain lines summarizing the surface that was just
/// walked. Public and pure so a test asserts on exactly what the human sees —
/// the same reason [`policy_requested_lines`] is public.
///
/// It reads the reviewed surface (`ReviewDiff::current`) rather than the
/// manifest, which is deliberate: the card and the detail below it are then
/// provably the same set of items, and any kind added to the review later shows
/// up here without a second place to remember to update. `blocked` is the count
/// of items that failed their pin check, so the pin line is honest on the path
/// where the review ends in a refusal.
///
/// Every value interpolated here is already-sanitized display text or a machine
/// count; hostile names reach this function only through `SurfaceItem.name`,
/// which is sanitized at the point of use below.
pub fn card_summary_lines(items: &[SurfaceItem], blocked: usize) -> Vec<String> {
    // A server's identity is its command line or its URL; that is the only
    // place the two are distinguishable once the surface is flattened.
    let is_url = |s: &str| s.starts_with("http://") || s.starts_with("https://");
    let named = |kind: &str, items: &[SurfaceItem]| -> Vec<String> {
        items
            .iter()
            .filter(|i| i.kind == kind)
            .map(|i| crate::text::sanitize_line(&i.name))
            .collect()
    };

    let mut runs: Vec<String> = items
        .iter()
        .filter(|i| i.kind == "server" && !is_url(&i.identity))
        .map(|i| crate::text::sanitize_line(&i.name))
        .collect();
    // Hooks and extensions are executable too — a card that counted only
    // servers as "runs" would undercount the executable surface, which is the
    // one number a reviewer must not be misled about.
    runs.extend(named("hook", items));
    runs.extend(named("extension", items));

    let contacts: Vec<String> = items
        .iter()
        .filter(|i| i.kind == "server" && is_url(&i.identity))
        .map(|i| {
            let host = i
                .identity
                .split("://")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .unwrap_or(&i.identity);
            crate::text::sanitize_line(host)
        })
        .collect();

    let secrets: Vec<String> = items
        .iter()
        .filter(|i| i.kind == "secrets")
        .flat_map(|i| i.identity.split(", ").map(crate::text::sanitize_line))
        .filter(|s| !s.is_empty())
        .collect();

    let context = named("skill", items).len() + named("instruction", items).len();

    let mut lines = vec!["This project will:".to_string()];
    if !runs.is_empty() {
        lines.push(format!(
            "  run {} on your machine — {}",
            super::count(runs.len(), "command"),
            preview_names(&runs)
        ));
    }
    if !contacts.is_empty() {
        lines.push(format!(
            "  contact {} — {}",
            super::count(contacts.len(), "host"),
            preview_names(&contacts)
        ));
    }
    if !secrets.is_empty() {
        lines.push(format!(
            "  be able to read {} — {}",
            super::count(secrets.len(), "secret"),
            preview_names(&secrets)
        ));
    }
    if context > 0 {
        lines.push(format!(
            "  add {} to every agent's context",
            super::count(context, "file")
        ));
    }
    // A project that declares nothing still gets a card, and it should say so
    // plainly rather than presenting an empty list as if it were a surface.
    if lines.len() == 1 {
        lines.push("  nothing — this project declares no capabilities yet".to_string());
    }
    lines.push(if blocked == 0 {
        "  …using exactly the content shown below, pinned to these bytes.".to_string()
    } else {
        // `count` pluralizes by appending `s`, so the irregular verb branches
        // here, exactly as its doc comment directs.
        format!(
            "  …but {} {} not pinned to reviewed bytes — details below.",
            super::count(blocked, "item"),
            if blocked == 1 { "is" } else { "are" }
        )
    });
    lines
}

/// Name the first few items and count the rest, so a project with forty skills
/// still produces a card a human reads in one glance.
fn preview_names(names: &[String]) -> String {
    const SHOWN: usize = 3;
    if names.len() <= SHOWN {
        return names.join(", ");
    }
    format!(
        "{}, and {} more",
        names[..SHOWN].join(", "),
        names.len() - SHOWN
    )
}

/// Render a settings value with object keys sorted, recursively, so the review
/// diff keys on *meaning* and not on serialization order. Two manifests that
/// declare the same settings with their keys typed in a different order are the
/// same consent; without this they would read as `~ changed` and train the user
/// to wave through a diff that says nothing. Arrays keep their order — element
/// order in a settings list is meaningful (precedence), unlike key order.
fn canonical_json(value: &serde_json::Value) -> String {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            let inner: Vec<String> = keys
                .iter()
                .map(|k| format!("{}:{}", k, canonical_json(&map[*k])))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        other => other.to_string(),
    }
}

/// The review lines for a workflow's approved blueprint (F13): the shape the
/// user signed off as a graph, shown at the gate that actually authorizes the
/// script. Pushes a blocker when the blueprint is unpinned or drifted, because
/// "this is the graph you approved" must be a claim the lockfile can back.
///
/// Blueprint bytes are HOSTILE INPUT — they arrive from a model's output
/// stream through a repo file. Everything here is bounded and fails to a
/// stated "unreadable" rather than a panic or a partial render; a blueprint
/// that will not parse is shown as such, never silently skipped, or its
/// absence would read as "no graph was approved".
fn blueprint_review_lines(
    dir: &Path,
    name: &str,
    declared: &str,
    lock: &crate::lock::Lock,
    blockers: &mut Vec<(String, String)>,
) -> Vec<String> {
    // Anchored at the manifest dir, matching how `[workflows.*].path` and the
    // lock's blueprint pin resolve — see `lock::pin_blueprint`.
    let shown = crate::text::sanitize_line(declared);

    let actual = match agentstack_core::digest::contained_file_digest(dir, declared) {
        Ok(d) => d,
        Err(e) => {
            blockers.push((
                name.to_string(),
                format!("approved blueprint '{declared}' is unreadable: {e}"),
            ));
            return vec![format!(
                "{} approved blueprint {shown} — {}",
                "✗".red(),
                "UNREADABLE".red()
            )];
        }
    };
    match lock.workflows.iter().find(|w| w.name == name) {
        Some(l) if l.blueprint_checksum.as_ref() == Some(&actual) => {}
        Some(_) => {
            blockers.push((
                name.to_string(),
                "approved blueprint drifted from lock — re-review and run `agentstack lock`"
                    .to_string(),
            ));
            return vec![format!(
                "{} approved blueprint {shown} — {}",
                "✗".red(),
                "DRIFTED from lock".red()
            )];
        }
        None => {
            blockers.push((
                name.to_string(),
                "approved blueprint unpinned — run `agentstack lock`".to_string(),
            ));
            return vec![format!(
                "{} approved blueprint {shown} — {}",
                "✗".red(),
                "unpinned".red()
            )];
        }
    }

    let mut out = vec![format!(
        "{} approved blueprint {shown}   [pinned]",
        "◆".cyan()
    )];
    match std::fs::read_to_string(dir.join(declared))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    {
        Some(v) => {
            let pattern = v
                .get("pattern")
                .and_then(|p| p.as_str())
                .unwrap_or("custom");
            let goal = v.get("goal").and_then(|g| g.as_str()).unwrap_or("");
            out.push(format!(
                "    pattern: {}{}",
                crate::text::sanitize_line(pattern),
                if goal.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", crate::text::sanitize_line(goal))
                }
            ));
            // Cap the node list: a blueprint is bounded at authoring time, but
            // a repo file is not, and a review surface that can be flooded is
            // a review surface that gets skipped.
            if let Some(nodes) = v.get("nodes").and_then(|n| n.as_array()) {
                for node in nodes.iter().take(16) {
                    let f = |k: &str| {
                        node.get(k)
                            .and_then(|x| x.as_str())
                            .map(crate::text::sanitize_line)
                            .unwrap_or_else(|| "?".into())
                    };
                    let fanout = node
                        .get("fanout")
                        .and_then(|x| x.as_str())
                        .map(|s| format!(" ×{}", crate::text::sanitize_line(s)))
                        .unwrap_or_default();
                    out.push(format!(
                        "    · {} — role {} · {}/{}{fanout}",
                        f("phase"),
                        f("role"),
                        f("model"),
                        f("effort")
                    ));
                }
                if nodes.len() > 16 {
                    out.push(format!(
                        "    · … {}",
                        super::count(nodes.len() - 16, "more node")
                    ));
                }
            }
        }
        None => out.push(format!(
            "    {}",
            "(blueprint is pinned but not readable as JSON — review the file itself)".yellow()
        )),
    }
    // Rule 8: say exactly what the pin does and does not buy. The graph and
    // the script are one consent; nothing here proves the script implements
    // the graph.
    out.push(format!(
        "    {}",
        "the graph and the script are pinned together; agentstack does not verify the script implements the graph"
            .dimmed()
    ));
    out
}

/// Print what the project's `[policy]` requests, per dimension. Bundles can
/// only narrow, so this is review signal, not a gate. Filesystem scopes are
/// labelled honestly: the write scope decides the sandbox workspace mount
/// (ro unless covered); read scopes are informational, and host mode
/// enforces neither.
fn review_policy(p: &crate::manifest::Policy, diff: &mut ReviewDiff, body: &mut Vec<String>) {
    let lines = policy_requested_lines(p);
    if !lines.is_empty() {
        // One aggregate item: any change to the requested set flips the header
        // line to `~ changed`.
        let mk = diff.mark("policy", "", &policy_identity(p));
        body.push(format!(
            "{mk}policy requested by this project (can only narrow the machine layer):"
        ));
        for line in &lines {
            body.push(line.clone());
        }
    }
    // P15: ALWAYS name the machine policy ceiling file — even for a policy-free
    // repo — so a user consenting learns a machine layer exists and where it
    // lives. Constant machine fact, so no diff marker; honors AGENTSTACK_HOME.
    let ceiling = crate::util::paths::agentstack_home().join("agentstack.toml");
    body.push(format!(
        "  machine policy ceiling: {} — the repo can only narrow it, never loosen it",
        ceiling.display()
    ));
}

/// The requested-policy lines the trust review prints, as a pure builder —
/// public so the regression test asserts on exactly what the human sees.
pub fn policy_requested_lines(p: &crate::manifest::Policy) -> Vec<String> {
    let mut lines = Vec::new();
    let dims: [(&str, &indexmap::IndexMap<String, Vec<String>>); 3] = [
        ("tools", &p.tools),
        ("egress", &p.egress),
        ("secrets", &p.secrets),
    ];
    for (label, map) in dims {
        for (server, rules) in map {
            // Server names and rule strings are manifest content — hostile
            // input; sanitize like every other review line.
            lines.push(format!(
                "  · {label:<7} {}: {}",
                crate::text::sanitize_line(server),
                crate::text::sanitize_line(&rules.join(", "))
            ));
        }
    }
    if !p.filesystem.read.is_empty() {
        lines.push(format!(
            "  · filesystem read {} (informational — the sandbox mounts one whole workspace)",
            crate::text::sanitize_line(&p.filesystem.read.join(", "))
        ));
    }
    if !p.filesystem.write.is_empty() {
        lines.push(format!(
            "  · filesystem write {} (sandbox mode mounts the workspace read-only unless this covers it; advisory in host mode)",
            crate::text::sanitize_line(&p.filesystem.write.join(", "))
        ));
    }
    if !p.filesystem.deny.is_empty() {
        lines.push(format!(
            "  · filesystem deny {} (blocklist — UNIONS with the machine layer; enforced by the host guard)",
            p.filesystem.deny.join(", ")
        ));
    }
    lines
}

/// The skill names a trust review covers: the manifest's inline `[skills.*]`
/// plus every profile-referenced name (which may resolve to the central
/// library), deduped in first-seen order. The `"*"` wildcard expands to inline
/// skills only — the same rule as activation — so it adds nothing new here.
pub(crate) fn review_skill_names(m: &crate::manifest::Manifest) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let push = |n: &str, names: &mut Vec<String>| {
        if n != "*" && !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
    };
    for n in m.skills.keys() {
        push(n, &mut names);
    }
    for p in m.profiles.values() {
        for n in &p.skills {
            push(n, &mut names);
        }
    }
    names
}

fn revoke(base: &Path) -> Result<()> {
    // Recognition is derived from consent, so it does not outlive it: this
    // project stops corroborating any other project's card the moment its own
    // trust is withdrawn.
    crate::recognition::forget(&trust::key_for(base));
    if trust::revoke(base)? {
        println!(
            "{} trust revoked for {} — auto-mode is control-plane only there now.",
            "✓".green(),
            base.display()
        );
    } else {
        println!("{} was not trusted; nothing to revoke.", base.display());
    }
    Ok(())
}

fn list() -> Result<()> {
    let store = TrustStore::load();
    if store.trusted.is_empty() {
        println!("No trusted projects. Grant one with `agentstack trust <dir>`.");
        return Ok(());
    }
    for (path, entry) in &store.trusted {
        let state = trust::check(Path::new(path));
        let (mark, note) = match state {
            TrustState::Trusted => ("✓".green().to_string(), "current".to_string()),
            TrustState::Changed => (
                "⚠".yellow().to_string(),
                "manifest or lockfile changed since trusted — re-run `agentstack trust` there"
                    .to_string(),
            ),
            // An entry exists, so Untrusted can't come back here; kept for
            // completeness.
            TrustState::Untrusted => ("⚠".yellow().to_string(), "stale entry".to_string()),
        };
        println!("  {mark} {path} · {} · {note}", entry.digest);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    // F5 WITNESS (FINDINGS.md): accept commits the displayed digest or
    // nothing. The tamper here is the field that actually moves — the live
    // bytes AFTER the diff was rendered, inside the human-scale window before
    // the closing yes. `refuse_undisplayed` is the one gate every accepted
    // item passes at the commit point, for both pin families.
    #[test]
    fn accept_refuses_bytes_that_were_not_displayed() {
        // The reviewed bytes: displayed == fresh → pin proceeds.
        assert!(refuse_undisplayed("alpha", Some("abc123"), "abc123").is_ok());

        // Swapped after display: fresh digest differs → refuse, and say the
        // content moved rather than granting it.
        let err = refuse_undisplayed("alpha", Some("abc123"), "d0d0d0")
            .unwrap_err()
            .to_string();
        assert!(err.contains("changed while you were reviewing"), "{err}");
        assert!(err.contains("Nothing was granted"), "{err}");
        assert!(err.contains("agentstack trust"), "{err}");

        // Never displayed at all (the walk could not hash it): also refuse —
        // "I couldn't hash what you looked at" is not a license to pin
        // whatever is on disk now.
        assert!(refuse_undisplayed("alpha", None, "abc123").is_err());
    }

    // CONSENT WITNESS (Phase 2, the card): the summary must count the whole
    // EXECUTABLE surface, not just servers. Hooks and extensions run commands
    // too, and a card that said "run 1 command" while three things execute is
    // precisely the consent surprise the phase gate counts.
    #[test]
    fn card_counts_hooks_and_extensions_as_things_that_run() {
        let items = vec![
            item("server", "fs", "node fs.js"),
            item("hook", "pre-commit", "PreToolUse runs ./check.sh → *"),
            item("extension", "pi-ext", "./ext.ts"),
        ];
        let lines = card_summary_lines(&items, 0);
        let runs = lines
            .iter()
            .find(|l| l.contains("run "))
            .expect("runs line");
        assert!(runs.contains("3 commands"), "{runs}");
        assert!(runs.contains("fs") && runs.contains("pre-commit") && runs.contains("pi-ext"));
    }

    // An HTTP server is a thing CONTACTED, not a thing run — the two must never
    // be conflated, in either direction.
    #[test]
    fn card_separates_contacted_hosts_from_run_commands() {
        let items = vec![
            item("server", "remote", "https://api.example.com/mcp"),
            item("server", "local", "node local.js"),
        ];
        let lines = card_summary_lines(&items, 0);
        let runs = lines
            .iter()
            .find(|l| l.contains("run "))
            .expect("runs line");
        let contacts = lines
            .iter()
            .find(|l| l.contains("contact "))
            .expect("contacts line");
        assert!(
            runs.contains("1 command") && runs.contains("local"),
            "{runs}"
        );
        // The host, not the whole URL — a path is noise at card altitude.
        assert!(
            contacts.contains("1 host") && contacts.contains("api.example.com"),
            "{contacts}"
        );
        assert!(!contacts.contains("/mcp"), "{contacts}");
    }

    // The card is a summary, and a summary that lies about pinning is worse
    // than no summary. When items failed their pin check the card says so
    // instead of claiming the reviewed bytes are the ones that will be used.
    #[test]
    fn card_pin_line_is_honest_when_items_are_unpinned() {
        let items = vec![item("skill", "greet", "library")];
        let clean = card_summary_lines(&items, 0);
        assert!(
            clean.last().unwrap().contains("pinned to these bytes"),
            "{:?}",
            clean.last()
        );
        let dirty = card_summary_lines(&items, 2);
        let last = dirty.last().unwrap();
        assert!(last.contains("2 items are not pinned"), "{last}");
        let one = card_summary_lines(&items, 1);
        assert!(one.last().unwrap().contains("1 item is not pinned"));
    }

    // Two to five lines: a forty-skill project must still be glanceable, so
    // names elide rather than wrapping the terminal.
    #[test]
    fn card_stays_glanceable_and_elides_long_lists() {
        let items: Vec<SurfaceItem> = (0..40)
            .map(|i| item("server", &format!("srv{i}"), "node x.js"))
            .collect();
        let lines = card_summary_lines(&items, 0);
        assert!(
            lines.len() <= 5,
            "card grew to {} lines: {lines:?}",
            lines.len()
        );
        let runs = lines.iter().find(|l| l.contains("run ")).unwrap();
        assert!(
            runs.contains("40 commands") && runs.contains("and 37 more"),
            "{runs}"
        );
    }

    // A project that declares nothing gets a card that says nothing — never an
    // empty list rendered as though it were a surface.
    #[test]
    fn card_names_the_empty_surface_plainly() {
        let lines = card_summary_lines(&[], 0);
        assert!(lines.iter().any(|l| l.contains("declares no capabilities")));
        assert!(lines.len() <= 5);
    }

    // Settings identity keys on MEANING, not serialization order: re-typing the
    // same object with keys in a different order is the same consent, and must
    // not read as `~ changed` — training a user to wave through diffs that say
    // nothing is how a real change gets waved through too.
    #[test]
    fn settings_identity_ignores_key_order_but_not_values() {
        let a: serde_json::Value = serde_json::json!({"b": 1, "a": {"y": 2, "x": [3, 4]}});
        let b: serde_json::Value = serde_json::json!({"a": {"x": [3, 4], "y": 2}, "b": 1});
        assert_eq!(canonical_json(&a), canonical_json(&b));
        let changed: serde_json::Value = serde_json::json!({"b": 2, "a": {"y": 2, "x": [3, 4]}});
        assert_ne!(canonical_json(&a), canonical_json(&changed));
        // Array ORDER is meaningful (precedence) and must still register.
        let reordered: serde_json::Value = serde_json::json!({"b": 1, "a": {"y": 2, "x": [4, 3]}});
        assert_ne!(canonical_json(&a), canonical_json(&reordered));
    }

    // SECURITY WITNESS (trust granting): the non-interactive consent gate. An
    // agent with shell access must NOT be able to self-trust a repo when stdin
    // is not a terminal — doing so would defeat the untrusted-means-inert gate.
    // Since §7.2, `--yes` alone is not enough either: the acknowledgement must
    // carry the previewed surface digest, or a headless caller could grant a
    // surface nobody reviewed. Tests run without a TTY, so `interactive: false`
    // is the real refusal path; `grant_gated` takes the probe as a parameter so
    // both branches are driven directly. NEVER delete or weaken this test.
    #[test]
    fn non_tty_grant_refuses_without_yes_and_consented_digest() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());

        // A minimal, blocker-free project: one inline HTTP server needs no lock
        // pin, so the review reaches the consent gate with nothing to block on.
        let proj = assert_fs::TempDir::new().unwrap();
        proj.child(".agentstack/agentstack.toml")
            .write_str("version = 1\n[servers.x]\ntype = \"http\"\nurl = \"https://x/mcp\"\n")
            .unwrap();

        // (a) Non-TTY, no --yes: refuse, and the trust store keeps no grant.
        assert!(grant_gated(proj.path(), false, None, false, None).is_err());
        assert_eq!(trust::check(proj.path()), TrustState::Untrusted);

        // (b) Non-TTY with --yes but NO consented digest: still refuses —
        // the §7.2 binding, not just the acknowledgement, is required.
        let err = grant_gated(proj.path(), true, None, false, None).unwrap_err();
        assert!(format!("{err:#}").contains("--consented-digest"));
        assert_eq!(trust::check(proj.path()), TrustState::Untrusted);

        // (c) --yes with a WRONG digest: refuses (the trust-crate witness
        // covers the store staying clean; here we prove the CLI wiring).
        assert!(grant_gated(proj.path(), true, Some("sha256:beef"), false, None).is_err());
        assert_eq!(trust::check(proj.path()), TrustState::Untrusted);

        // (d) --yes with the previewed digest: grants.
        let previewed = trust::digest_for(proj.path()).unwrap();
        grant_gated(proj.path(), true, Some(&previewed), false, None).unwrap();
        assert_eq!(trust::check(proj.path()), TrustState::Trusted);

        std::env::remove_var("AGENTSTACK_HOME");
    }

    fn item(kind: &str, name: &str, identity: &str) -> SurfaceItem {
        SurfaceItem {
            kind: kind.to_string(),
            name: name.to_string(),
            identity: identity.to_string(),
            // The card summary reads kinds and identities, never pins — a pin
            // is for the re-gate diff, not for what the card counts.
            pin: None,
        }
    }

    // P14: the re-trust diff marks each item against the last consented
    // surface. This is the machine-checked form of the feature: same item →
    // plain, new item → added, same key but new identity → changed, and a prior
    // item never re-marked → removed. It also proves flat mode (no prior) marks
    // nothing, so first-trust and older-entry reviews look unchanged.
    #[test]
    fn mark_classifies_added_changed_unchanged_and_removed() {
        // The `git pull` scenario: last time we consented to a safe server and
        // a library skill; now a new `evil` server appears, the safe server's
        // command changed, the skill is unchanged, and an old server is gone.
        let prior = vec![
            item("server", "safe", "node safe.js"),
            item("server", "gone", "node gone.js"),
            item("skill", "greet", "library"),
        ];
        let mut diff = ReviewDiff::new(PriorSurface::Recorded(prior));
        assert!(diff.diffing());

        // Same key + same identity → unchanged (plain two-space indent).
        assert_eq!(diff.mark("skill", "greet", "library"), "  ");
        // Same key + different identity → changed.
        assert_eq!(diff.mark("server", "safe", "node safe.js --new"), "~ ");
        // New key → added — this is the surfaced `evil` server.
        assert_eq!(diff.mark("server", "evil", "sh -c pwn"), "+ ");

        // "gone" was in the prior surface but never re-marked → removed.
        let removed = diff.removed();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].name, "gone");

        // The accumulated current surface is exactly what would be persisted,
        // in render order.
        assert_eq!(
            diff.current,
            vec![
                item("skill", "greet", "library"),
                item("server", "safe", "node safe.js --new"),
                item("server", "evil", "sh -c pwn"),
            ]
        );
    }

    #[test]
    fn flat_mode_marks_nothing_and_has_no_removals() {
        // First-ever trust (and an older entry with no snapshot) both render
        // flat: every marker is the plain indent, nothing reads as removed, yet
        // the surface is still accumulated for the next re-trust to diff.
        for prior in [PriorSurface::NeverTrusted, PriorSurface::Untracked] {
            let mut diff = ReviewDiff::new(prior);
            assert!(!diff.diffing());
            assert_eq!(diff.mark("server", "anything", "whatever"), "  ");
            assert!(diff.removed().is_empty());
            assert_eq!(diff.current, vec![item("server", "anything", "whatever")]);
        }
    }

    // `trust-card-diff-v1`: the preview's marker and the review's marker are
    // two implementations of ONE rule, so drive both over the same prior
    // surface and assert they answer identically — including which prior items
    // read as removed. The integration witness proves the two walks build the
    // same identity STRINGS; this proves they classify them the same way.
    //
    // `CardWalk` is built by hand rather than through `new`, which would read
    // this machine's recognition index and make a unit test depend on global
    // state it does not care about.
    #[test]
    fn the_previews_change_marker_agrees_with_the_reviews_marker() {
        let prior = vec![
            item("server", "safe", "node safe.js"),
            item("server", "gone", "node gone.js"),
            item("skill", "greet", "library"),
        ];
        let now = [
            ("skill", "greet", "library"),
            ("server", "safe", "node safe.js --new"),
            ("server", "evil", "sh -c pwn"),
        ];

        let mut review = ReviewDiff::new(PriorSurface::Recorded(prior.clone()));
        let review_marks: Vec<&str> = now
            .iter()
            .map(|(kind, name, identity)| review.mark(kind, name, identity))
            .collect();

        let mut card = CardWalk {
            prior: prior.clone(),
            seen: HashSet::new(),
            items: Vec::new(),
            index: None,
            project_key: "/project".to_string(),
            store_root: PathBuf::from("/nonexistent"),
        };
        for (kind, name, identity) in now {
            card.push(CardItem::new(kind, name, identity));
        }
        let card_marks: Vec<&str> = card
            .items
            .iter()
            .map(|i| i["change"].as_str().unwrap())
            .collect();

        assert_eq!(review_marks, ["  ", "~ ", "+ "]);
        assert_eq!(card_marks, ["unchanged", "changed", "added"]);
        assert_eq!(
            review.removed().iter().map(|i| &i.name).collect::<Vec<_>>(),
            ["gone"]
        );
        assert_eq!(
            card.removed()
                .iter()
                .map(|i| i["name"].as_str().unwrap().to_string())
                .collect::<Vec<_>>(),
            ["gone"]
        );
        // Nothing pinned, so nothing claims a diff or a recognition count.
        assert!(card.items.iter().all(|i| i["diff"].is_null()));
        assert!(card
            .items
            .iter()
            .all(|i| i["recognized_other_projects"].is_null()));
    }
}
