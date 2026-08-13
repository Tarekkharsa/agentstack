//! `agentstack.lock` — pins each capability's resolved source for reproducible,
//! SHA-256 checksum-verified installs (PLAN §9d, D12). Lives next to the
//! manifest.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::digest::Sha256Hex;
use serde::{Deserialize, Serialize};

pub const LOCK_FILE: &str = "agentstack.lock";
/// Newest lockfile schema version this build reads and writes. Anything above
/// it was written by a future agentstack; [`Lock::load`] refuses it instead of
/// misinterpreting silently.
///
/// v2 added `[[instruction]]` pins. The bump matters because TOML parsing here
/// is permissive (no `deny_unknown_fields`): a v1 binary reading a lock with
/// instruction pins would silently drop them — and write them away on its next
/// save. The version guard turns that silent unpinning into a loud refusal.
///
/// Phase 4 added `license` and `origin` to [`LockedSkill`] for attribution
/// capture, and did NOT bump this — for the same reason the executable,
/// extension, and workflow pins did not. They are `#[serde(default)]` optional
/// fields, so every lock written before them parses unchanged; and an older
/// binary that rewrote them away would change the lock bytes, which flips the
/// trust digest and forces a re-review rather than losing them silently.
///
/// That last point cuts both ways and is worth stating plainly: lock bytes are
/// consent-digest material by design, so the first re-lock that records an
/// attribution field on an existing project WILL re-gate it. That is the
/// mechanism working, not a regression — the content the user consented to is
/// now described differently, and they get to see that. A witness asserts the
/// re-gate happens rather than trying to suppress it.
pub const SUPPORTED_LOCK_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Lock {
    pub version: u32,
    #[serde(default, rename = "skill")]
    pub skills: Vec<LockedSkill>,
    #[serde(default, rename = "server")]
    pub servers: Vec<LockedServer>,
    #[serde(default, rename = "instruction")]
    pub instructions: Vec<LockedInstruction>,
    /// D3 executable pins (locked-run contract §8). Additive `#[serde(default)]`
    /// fields at version 2, unlike the v1→v2 instruction-pin bump: a pre-D3 v2
    /// binary that rewrites these pins away changes the lock bytes, which flips
    /// the trust digest and forces re-review — and both the trust gate and
    /// strict locked verification block unpinned repo-relative executables, so
    /// silent unpinning cannot pass any gate downstream.
    #[serde(default, rename = "executable")]
    pub executables: Vec<LockedExecutable>,
    /// Native-extension pins (D6). Additive `#[serde(default)]` at version 2,
    /// on the same justification as the executable pins above: an older binary
    /// that rewrites these pins away changes the lock bytes, which flips the
    /// trust digest and forces re-review — and both the trust gate and strict
    /// locked verification block unpinned extensions, so silent unpinning
    /// cannot pass any gate downstream.
    #[serde(default, rename = "extension")]
    pub extensions: Vec<LockedExtension>,
    /// Workflow pins (D7 W1). Additive `#[serde(default)]` at version 2, on
    /// the same justification as the executable and extension pins above: an
    /// older binary that rewrites these pins away changes the lock bytes,
    /// which flips the trust digest and forces re-review — and both the trust
    /// gate and workflow admission block unpinned workflows, so silent
    /// unpinning cannot pass any gate downstream.
    #[serde(default, rename = "workflow")]
    pub workflows: Vec<LockedWorkflow>,
    /// Expanded package pins (W5, `docs/design/package-layer.md`). Additive
    /// `#[serde(default)]` at version 2, on the same justification as the
    /// executable, extension and workflow pins above: an older binary that
    /// rewrites these away changes the lock bytes, which flips the trust digest
    /// and forces re-review — and a toolset that selects a package refuses to
    /// activate without its expansion, so silent unpinning cannot pass any gate
    /// downstream.
    #[serde(default, rename = "package")]
    pub packages: Vec<LockedPackage>,
    /// Native-settings key pins (G18). Additive `#[serde(default)]` at version
    /// 2, on the same justification as the executable, extension, workflow and
    /// package pins above — with one difference worth stating: this list also
    /// carries `skip_serializing_if`, like [`LockedInstruction::variants`], so a
    /// project that declares no `[settings.*]` keeps a byte-identical lock and
    /// is never re-gated by the mere arrival of a new pin kind.
    ///
    /// Silent unpinning by an older binary is not a hole here for the same
    /// reason it is not one above: rewriting these away changes the lock bytes,
    /// which flips the trust digest and forces a review. What it does NOT do is
    /// block delivery — see [`LockedSetting`] for exactly what a settings pin
    /// claims and what it does not.
    #[serde(default, rename = "setting", skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<LockedSetting>,
    /// The declaration bytes this pin set was computed from — SHA-256 over
    /// `agentstack.toml` plus the `agentstack.local.toml` overlay, framed by
    /// [`manifest_stamp`].
    ///
    /// **Why the lock records it.** Every pin above answers "did this content
    /// move?"; none of them answers "was this lock computed from the manifest
    /// that is on disk now?". A hand-edited manifest that adds a server, a
    /// settings key, or an instruction leaves every existing pin matching, so
    /// the only way to notice the lock predates the edit used to be to run the
    /// resolver again — which the refusal path may not do. This field turns
    /// that question into a byte comparison ([`Lock::manifest_stale`]).
    ///
    /// **Absent means UNKNOWN, never fresh.** Every lock written before this
    /// field, and every write that was not a whole-manifest re-lock, carries
    /// no stamp — and a reader must then behave exactly as it did before the
    /// field existed. That is why [`Lock::save`] DROPS the stamp and only
    /// [`Lock::save_stamped`] records one: a partial write (an `add`, a
    /// profile-scoped `lock`, a `use --write` first pin) cannot honestly claim
    /// the pins cover the whole manifest, and a stale claim would report a
    /// healthy project as broken.
    ///
    /// Additive `#[serde(default)]` optional at version 2, on the precedent
    /// [`LockedSkill::license`] states: an older binary that rewrites it away
    /// changes the lock bytes, which flips the trust digest and forces a
    /// review rather than losing it silently. `skip_serializing_if` keeps an
    /// unstamped lock byte-identical, and the writer only records a stamp on a
    /// re-lock that was rewriting the file anyway — so an existing project is
    /// re-gated by the arrival of this field exactly once, folded into a
    /// re-review its own pin change had already earned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_digest: Option<Sha256Hex>,
}

/// Domain separator for the manifest stamp. Distinct from every content digest
/// so a stamp can never be mistaken for (or collide with) a pin.
const MANIFEST_STAMP_DOMAIN: &[u8] = b"agentstack-lock-manifest-stamp-v1\0";

/// The stamp a whole-manifest re-lock records for the project at `dir`:
/// SHA-256 over the declaration layers the pins are computed from — the
/// manifest, and the local overlay when one exists.
///
/// The overlay is included because it declares servers, and a server declared
/// there is pinned like any other; leaving it out would make an overlay edit
/// invisible to the staleness check. Each segment is framed presence-byte +
/// length + bytes, the same framing the trust digest uses, so an absent
/// overlay can never collide with a present empty one.
///
/// `None` when there is no readable manifest — nothing was declared, so there
/// is nothing to be stale against.
pub fn manifest_stamp(dir: &Path) -> Option<Sha256Hex> {
    use sha2::{Digest, Sha256};
    let manifest = fs::read(dir.join(crate::manifest::load::MANIFEST_FILE)).ok()?;
    let local = fs::read(dir.join(crate::manifest::load::LOCAL_FILE)).ok();
    let mut hasher = Sha256::new();
    hasher.update(MANIFEST_STAMP_DOMAIN);
    for segment in [Some(manifest.as_slice()), local.as_deref()] {
        match segment {
            Some(bytes) => {
                hasher.update([1u8]);
                hasher.update((bytes.len() as u64).to_le_bytes());
                hasher.update(bytes);
            }
            None => hasher.update([0u8]),
        }
    }
    Sha256Hex::parse(&format!("{:x}", hasher.finalize())).ok()
}

impl Default for Lock {
    fn default() -> Self {
        Lock {
            version: SUPPORTED_LOCK_VERSION,
            skills: Vec::new(),
            servers: Vec::new(),
            instructions: Vec::new(),
            executables: Vec::new(),
            extensions: Vec::new(),
            workflows: Vec::new(),
            packages: Vec::new(),
            settings: Vec::new(),
            manifest_digest: None,
        }
    }
}

/// Which kind of capability one package member is. Serializes lowercase
/// (`"skill"` / `"server"` / `"instruction"`).
///
/// **Hooks and extensions are absent on purpose and must stay absent.** They
/// are executable kinds for which the full consent ceremony always applies, and
/// a package reference is by construction a compressed consent path — one name
/// standing for a set of members. Adding a variant here would be the schema
/// half of exactly the compression `CLAUDE.md`'s standing classification
/// forbids, so the intake gate refuses such a package by name instead
/// (`docs/design/package-layer.md` §"Deferred by name").
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum PackageMemberKind {
    Skill,
    Server,
    Instruction,
}

impl PackageMemberKind {
    /// The lane this kind is delivered through — a property of the KIND, not of
    /// a choice any caller makes, which is why it is derived here rather than
    /// stored. Skills and servers go through the gateway lease; instructions
    /// are written into a file, and no surface may say otherwise
    /// (`docs/design/automatic-delivery.md` §"Mixed-lane upgrades").
    pub fn lane(&self) -> &'static str {
        match self {
            PackageMemberKind::Skill | PackageMemberKind::Server => "dynamic",
            PackageMemberKind::Instruction => "rendered",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            PackageMemberKind::Skill => "skill",
            PackageMemberKind::Server => "server",
            PackageMemberKind::Instruction => "instruction",
        }
    }
}

/// Who decided one member of the effective set. Serializes as
/// `"package"` / `"project-override"`.
///
/// The whole point of the field: a compact package reference could otherwise
/// diverge silently, and the W5 acceptance criterion is that an override is
/// visible as an *effective member set* rather than a quiet divergence.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PackageMemberOrigin {
    /// The package published this member and the project took it as published.
    Package,
    /// The project replaced this member with one of its own declarations.
    ProjectOverride,
}

impl PackageMemberOrigin {
    pub fn as_str(&self) -> &'static str {
        match self {
            PackageMemberOrigin::Package => "package",
            PackageMemberOrigin::ProjectOverride => "project-override",
        }
    }
}

/// One member of a package's **effective** set, pinned.
///
/// The per-member digest is the reason a compact reference is safe: the
/// checksum comes from the same pinning acts every other kind uses
/// (`Store::pin` for a skill tree, `Store::pin_instruction` for a fragment, the
/// server-definition digest for a server), so a member's bytes moving changes
/// the lock bytes, which flips the trust digest and re-gates. There is
/// deliberately no fast path that accepts a package on its version alone.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedPackageMember {
    /// The member name, as the package publishes it. Stable across an override:
    /// replacing a member does not rename it, or `removed`/`replace` keys in
    /// one project would stop matching the package's own vocabulary.
    pub name: String,
    pub kind: PackageMemberKind,
    pub origin: PackageMemberOrigin,
    /// Content digest of this member's bytes.
    pub checksum: Sha256Hex,
    /// Where these bytes came from, for a human:
    /// `package:<name>@<version>#<path-in-package>`, or `project:<key>` for an
    /// overriding member. Not a second copy of the package coordinates — those
    /// are on [`LockedPackage`] — this says which artifact produced the digest.
    pub provenance: String,
}

/// One package a toolset selected, expanded to its exact members.
///
/// `docs/design/package-layer.md` §"The lock expansion". Everything the
/// contract names is here: the exact version and revision, the exact member
/// list, per-member digests, and per-member provenance — plus the two fields
/// that keep an override honest (`removed`, and each member's `origin`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedPackage {
    pub name: String,
    /// The exact package version this project is pinned to.
    pub version: String,
    /// Where the package was resolved from: `library:<name>` for a
    /// central-library body, or `git:<url>@<tag>[#subdir]` for a git-hosted one.
    pub source: String,
    /// The exact revision for a git-sourced package (`None` for a path body,
    /// which has no revision axis). Distinguishes "the same package, later"
    /// from "the same package".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// Which toolsets selected this package — sorted, de-duplicated. Without
    /// it, dropping one toolset's `packages` line would leave a lock entry that
    /// nothing in the manifest explains.
    #[serde(default)]
    pub toolsets: Vec<String>,
    /// Package member names this project dropped (`[package_overrides.<p>]
    /// remove`), sorted. Kept OUT of `members`: the member list is the
    /// effective set, and something removed is not a member. It is recorded
    /// here instead so a reader can still see that the project diverged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// The effective member set, sorted by name.
    #[serde(default, rename = "member")]
    pub members: Vec<LockedPackageMember>,
}

/// Where a pinned server's definition came from: declared inline in the
/// manifest, or referenced by name from the central library. Serializes
/// lowercase (`"inline"` / `"library"`), so the lockfile bytes — and thus the
/// trust digest over them — are byte-identical to the pre-enum string form.
/// (TS mental model: a `"inline" | "library"` union with an exhaustive match.)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServerSource {
    Inline,
    Library,
}

/// Where a pinned skill's body came from: a local path or a git source.
/// Serializes lowercase (`"path"` / `"git"`) — lockfile-byte-identical to the
/// pre-enum string form (see [`ServerSource`]).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SkillLockSource {
    Path,
    Git,
}

/// A pinned native harness extension (D6): its source tree's strict
/// integrity-root digest (symlink anywhere = hard error, `.git` included —
/// the executable-content digest family, never the lenient skill digest) plus
/// the one adapter it targets, so a review diff is self-describing.
///
/// The source-provenance fields (`source`/`path`/`git`/`rev`) mirror
/// [`LockedSkill`]: an inline project-local `path`, an inline or library `git`
/// checkout (with the resolved `rev`), or a `library` path body. They are
/// additive over the E1-era shape (name/target/checksum only) — `source`
/// carries a serde default so a pre-E3 lock entry still parses, and every
/// extension pinned before E3 was a project-local `path`, so `"path"` is the
/// honest default.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedExtension {
    pub name: String,
    pub target: String,
    /// `"path"` (project-local), `"git"` (a fetched checkout), or `"library"`
    /// (a central-library path body). Defaults to `"path"` for E1-era entries.
    #[serde(default = "locked_extension_source_default")]
    pub source: String,
    /// The declared path a `path`/`library` source pinned (provenance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The git URL a `git` source pinned (provenance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// The resolved git commit a `git` source was pinned at (rev-drift check).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    pub checksum: String,
}

/// E1-era `[[extension]]` entries carry no `source` key; every extension pinned
/// before E3 was a project-local `path`, so that is the honest default.
fn locked_extension_source_default() -> String {
    "path".to_string()
}

/// A pinned governed workflow (D7 W1): its source tree's strict
/// integrity-root digest (the executable-content digest family, same as
/// [`LockedExtension`] — symlink anywhere = hard error, never the lenient
/// skill digest) plus the sorted role set the review bound it to.
///
/// The pin records `roles` the way an extension pin records `target`: the
/// human reviewed THIS script against THESE capability sets, so widening the
/// roles without re-locking is drift even when the bytes are unchanged.
/// `roles` is stored sorted and de-duplicated ([`Lock::upsert_workflow`]
/// canonicalizes) so declaration order never fakes or masks a change.
///
/// The source-provenance fields (`source`/`path`/`git`/`rev`) mirror
/// [`LockedExtension`]. The checksum is the typed [`Sha256Hex`] from day one —
/// a brand-new pin kind has no legacy entries to stay lenient for, so a
/// malformed checksum fails the parse loudly instead of riding to a silent
/// mismatch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedWorkflow {
    pub name: String,
    /// The reviewed role set — sorted, de-duplicated.
    #[serde(default)]
    pub roles: Vec<String>,
    /// `"path"` (project-local) or `"git"` (a fetched checkout).
    pub source: String,
    /// The declared path a `path` source pinned (provenance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The git URL a `git` source pinned (provenance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// The resolved git commit a `git` source was pinned at (rev-drift check).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    pub checksum: Sha256Hex,
    /// The declared blueprint path this workflow was authored from (F13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blueprint: Option<String>,
    /// SHA-256 of the approved blueprint's bytes. Pinned BESIDE `checksum` so
    /// the reviewed graph and the executable it produced are one consent: the
    /// lockfile feeds the trust digest, so editing either artifact re-gates the
    /// project. Absent for a workflow with no blueprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blueprint_checksum: Option<Sha256Hex>,
}

/// A pinned MCP server: the SHA-256 of its **definition** (a `${REF}`-only
/// server table — never resolved secret values, never a provider-specific render
/// shape), so a fresh checkout resolves the same server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedServer {
    pub name: String,
    pub source: ServerSource,
    pub checksum: Sha256Hex,
}

/// A pinned instruction fragment: the SHA-256 of the file's raw bytes.
/// Instructions are always local files declared by path in the manifest —
/// no source/git/rev fields apply. Machine-layer (user-layer) fragments are
/// never pinned; they aren't repo content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedInstruction {
    pub name: String,
    pub path: String,
    pub checksum: Sha256Hex,
    /// One pin per declared per-(CLI, model) variant body
    /// (`docs/design/instruction-variants.md` §"Every variant body is
    /// pinned"). A variant is content, and content is pinned — including a
    /// variant nothing currently selects, because consent is over content and
    /// not over what happened to be chosen today.
    ///
    /// Additive `#[serde(default)]` at version 2, on the same justification as
    /// the executable, extension, workflow and package pins above: a lock
    /// written before this parses unchanged, and an older binary that rewrote
    /// these away would change the lock bytes, which flips the trust digest and
    /// forces a review rather than losing the pins silently. An empty list
    /// serializes to nothing, so a project with no variants keeps a
    /// byte-identical lock.
    #[serde(default, rename = "variant", skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<LockedInstructionVariant>,
}

/// A pinned instruction **variant** body: its selector plus the SHA-256 of the
/// file's raw bytes, produced by the same `Store::pin_instruction` act as the
/// base fragment. No second digest path exists, and none may be added.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedInstructionVariant {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub path: String,
    pub checksum: Sha256Hex,
}

/// How a D3 executable pin's digest was computed — the two families are not
/// interchangeable (see `agentstack_core::digest`).
///
/// (TS mental model: a string-literal union `"file" | "root"` with exhaustive
/// `match` at every consumer.)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum ExecutableKind {
    /// One repository-relative file (a stdio `command` or interpreter-script
    /// `args` entry), pinned by `contained_file_digest` — raw file bytes.
    File,
    /// A declared integrity root (file or directory subtree), pinned by the
    /// symlink-rejecting, domain-separated `integrity_root_digest`.
    Root,
}

/// A pinned repository-local executable input (D3, contract §8): the
/// repo-relative path as declared in the manifest, which digest family pinned
/// it, and the content checksum. Identity is `(path, kind)` — the same path
/// may legitimately carry both a file pin (as an `args` entry) and a root pin
/// (as a declared root), and the two digests differ.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedExecutable {
    pub path: String,
    pub kind: ExecutableKind,
    pub checksum: Sha256Hex,
}

/// A pinned native-settings key (G18): one top-level key of one CLI's own
/// settings file, checksummed over the **declared** value.
///
/// **The grain is the key, not the block, because the key is the unit
/// AgentStack owns.** `crates/cli/src/render/settings.rs` says it in its own
/// header — "we own a *set of top-level keys*" — and `merge_top_level` acts one
/// key at a time: it replaces a key wholesale, prunes a key it used to own, and
/// leaves every other byte of the file alone. Pinning at that same grain buys
/// two things a single digest over the whole `[settings.<target>]` block could
/// not: a probe can name *which* key moved instead of saying "something in this
/// block changed", and the pin's coverage boundary is expressible at all — a
/// key AgentStack does not declare has no row here, so it can never be read as
/// drift. The cost is a few extra lines in the lockfile for values that are
/// small scalars and shallow tables; that is the cheap side of the trade.
///
/// **What the checksum covers: the value as DECLARED, `${REF}`s unresolved.**
/// The same rule [`LockedServer`] follows, and for the same two reasons. A
/// resolved value is machine-specific, so pinning it would make one committed
/// lockfile disagree with itself across two developers' machines; and a
/// resolved value can contain a secret, which must never reach a file that gets
/// committed. The consequence is stated rather than hidden: a pin match proves
/// the declaration is the reviewed declaration, and the *rendered* comparison
/// is a second, separate leg that `doctor` performs against the live file.
///
/// **What a settings pin does NOT claim.** It says nothing about bytes of the
/// harness's settings file that AgentStack did not declare — that is the point
/// of the per-key grain, and the reason a user's own unrelated edit is not
/// drift. It is also not a delivery gate: unlike an unpinned skill or
/// instruction, an unpinned or drifted settings key does not refuse a render.
/// Settings are inert configuration merged into a file the harness owns, and a
/// fail-closed refusal there would leave the user's own harness config
/// half-written for a class of change that trust review already re-gates
/// through the manifest bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedSetting {
    /// The adapter id whose native settings file this key is merged into
    /// (`claude-code`, `codex`, …). Recorded like [`LockedExtension::target`]
    /// so the pin is self-describing in a review diff.
    pub target: String,
    /// The top-level key AgentStack owns in that file (`permissions`, `env`, …).
    pub key: String,
    pub checksum: Sha256Hex,
}

impl LockedSetting {
    /// The checksum a pin for `value` must carry. The single formula: SHA-256
    /// over [`canonical_settings_json`]. Both the pinning side
    /// (`Store::pin_settings_key`, which additionally deposits these bytes) and
    /// the probing side (`doctor`) go through here, so the two can never drift
    /// into disagreeing about what a settings digest is.
    pub fn checksum_of(value: &serde_json::Value) -> Sha256Hex {
        Sha256Hex::of(canonical_settings_json(value).as_bytes())
    }
}

/// The canonical JSON form of a settings value — object keys sorted
/// recursively, no insignificant whitespace. This exact byte string is what a
/// [`LockedSetting`] checksum covers and what the content store deposits.
///
/// Object key order is not meaning: two manifests that type the same keys in a
/// different order declare the same settings, and a pin that flipped on
/// re-ordering would push people through a re-review that says nothing. Array
/// order IS meaning — element order in an allow/deny list is precedence — so
/// arrays keep theirs. Same rule the consent card's `settings_identity` already
/// applies (`crates/cli/src/commands/trust.rs`); this is the digest-bearing
/// twin of it, in valid JSON so the deposited bytes are readable content rather
/// than an internal display string.
pub fn canonical_settings_json(value: &serde_json::Value) -> String {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            let inner: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        // `Value::String(..).to_string()` is the JSON string
                        // escape, so a key containing `"` cannot forge a
                        // separator and collide two distinct values onto one
                        // digest.
                        "{}:{}",
                        Value::String((*k).to_string()),
                        canonical_settings_json(&map[*k])
                    )
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(canonical_settings_json).collect();
            format!("[{}]", inner.join(","))
        }
        other => other.to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedSkill {
    pub name: String,
    pub source: SkillLockSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    pub checksum: Sha256Hex,
    /// SPDX identifier declared by the source this content came from, when it
    /// declared one (Phase 4, governed intake).
    ///
    /// **The convention these two fields follow, stated here because this is
    /// where the next person adding a field will be looking:** additive
    /// `#[serde(default)]` optionals keep [`SUPPORTED_LOCK_VERSION`] as it is,
    /// and the compatibility guarantee is carried by property tests
    /// (`crates/cli/tests/attribution_schema.rs`) rather than by the version
    /// number. A version bump is for a SHAPE change — a field whose meaning or
    /// type moves, or one an older binary would misread rather than ignore.
    /// So: another addition should follow this precedent and leave the version
    /// alone; a shape change cannot, and must bump.
    ///
    /// Recorded in the lock rather than only in the library because the lock is
    /// what travels with the project and what the trust digest covers: an
    /// attribution obligation that lived only on the importing machine would be
    /// lost by the first person to clone the repo. `None` means the source said
    /// nothing — deliberately distinct from a source that declared no licence,
    /// because "unknown" and "unlicensed" are different facts and only one of
    /// them is a reason to stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Where these bytes came from, verbatim from the intake source (a URL or a
    /// registry id). Not a second copy of `git`/`path`: those say how to fetch
    /// it again, this says who it belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

impl Lock {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(LOCK_FILE)
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let path = Self::path(dir);
        // Bounded: a cloned repo's lockfile is hostile input (rule 7).
        match crate::util::read_to_string_bounded(&path, crate::util::MAX_CONFIG_BYTES) {
            Ok(text) => Self::parse(&text, &path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Lock::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Parse lockfile TEXT a caller already captured — the seam that lets a
    /// consent snapshot holder interpret the exact lock bytes it digested,
    /// instead of a second disk read that could see different content.
    /// `origin` names the file in errors. Same version handling as [`load`].
    pub fn parse(text: &str, origin: &Path) -> Result<Self> {
        let lock: Lock =
            toml::from_str(text).with_context(|| format!("parsing {}", origin.display()))?;
        crate::util::check_schema_version(
            lock.version,
            SUPPORTED_LOCK_VERSION,
            "lockfile",
            origin,
        )?;
        // Normalize the in-memory version so struct equality (and the
        // callers' "no-op if unchanged" save checks) stays honest: an
        // untouched older lock is never rewritten just to bump its
        // version, but any genuine write upgrades the file.
        let mut lock = lock;
        lock.version = SUPPORTED_LOCK_VERSION;
        Ok(lock)
    }

    /// Write the lock, DROPPING any manifest stamp.
    ///
    /// The ordinary save, and the one every partial write takes. A write that
    /// re-pinned one capability (`add`), one toolset (`lock --profile`), or one
    /// activation's subset (`use --write`) knows nothing about the rest of the
    /// manifest, so it must not leave behind a claim that the pins were
    /// computed from the manifest on disk. Dropping the stamp returns the file
    /// to "staleness unknown", which is exactly how every lock behaved before
    /// the field existed — the fail-safe direction.
    ///
    /// A no-op save is still a no-op: callers only write when the pins
    /// changed, so an untouched lock keeps its stamp.
    pub fn save(&self, dir: &Path) -> Result<()> {
        if self.manifest_digest.is_none() {
            return self.write(dir);
        }
        let mut unstamped = self.clone();
        unstamped.manifest_digest = None;
        unstamped.write(dir)
    }

    /// Write the lock, recording the manifest stamp for `dir`.
    ///
    /// Only a WHOLE-manifest re-lock may call this — the pin pass that walked
    /// every declared kind and every toolset. See [`Lock::manifest_digest`]
    /// for why a partial write must take [`Lock::save`] instead.
    pub fn save_stamped(&self, dir: &Path) -> Result<()> {
        let mut stamped = self.clone();
        stamped.manifest_digest = manifest_stamp(dir);
        stamped.write(dir)
    }

    /// Write the lock, leaving any stamp exactly as it is.
    ///
    /// The narrow middle case between [`save`] and [`save_stamped`]: a write
    /// that re-pins the BYTES of items already in the lock without touching
    /// which declarations the lock covers — the re-gate card's accept path,
    /// which patches checksums for reviewed items and re-resolves nothing.
    /// The stamp answers "were these pins computed from this manifest?", and
    /// such a write moves neither side of that question, so dropping the stamp
    /// would throw away a true answer (and make the card's lockfile differ,
    /// byte for byte, from the `lock --write` + `trust` sequence it must stay
    /// at parity with).
    ///
    /// Not for a write that could ADD or REMOVE a pin: there the coverage
    /// claim really is untouched-but-now-wrong, and [`save`] is the honest
    /// choice.
    ///
    /// [`save`]: Lock::save
    /// [`save_stamped`]: Lock::save_stamped
    pub fn save_keeping_stamp(&self, dir: &Path) -> Result<()> {
        self.write(dir)
    }

    fn write(&self, dir: &Path) -> Result<()> {
        let path = Self::path(dir);
        let text = toml::to_string_pretty(self)?;
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }

    /// Does this lock's pin set predate the manifest at `dir`?
    ///
    /// `None` = unknown: no stamp was recorded (an older lock, or one whose
    /// last write was partial), or there is no readable manifest. A reader
    /// that gets `None` must behave exactly as it did before the stamp
    /// existed — the absence is not evidence of freshness.
    pub fn manifest_stale(&self, dir: &Path) -> Option<bool> {
        let recorded = self.manifest_digest.as_ref()?;
        let current = manifest_stamp(dir)?;
        Some(*recorded != current)
    }

    pub fn get(&self, name: &str) -> Option<&LockedSkill> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// Insert or replace a skill entry, keeping entries sorted by name.
    /// Insert or replace a skill pin, carrying attribution forward.
    ///
    /// Attribution is preserved HERE rather than at each call site, and that is
    /// the whole reason this function has a body worth reading. `license` and
    /// `origin` are known only at intake — the moment the content arrived from
    /// somewhere with a licence to honour. Every later re-lock rebuilds the
    /// entry from resolved state, which has no idea where the content came
    /// from, and would therefore hand us `None`. If that `None` won, an
    /// ordinary `agentstack install` would quietly erase an attribution
    /// obligation, which is exactly the "carried by promise" failure the
    /// attribution work exists to replace.
    ///
    /// So: an incoming `None` never overwrites a recorded value, while an
    /// incoming `Some` does. Re-importing from a source that now declares a
    /// different licence updates it; re-locking never launders it away.
    pub fn upsert(&mut self, mut entry: LockedSkill) {
        if let Some(existing) = self.skills.iter_mut().find(|s| s.name == entry.name) {
            if entry.license.is_none() {
                entry.license = existing.license.clone();
            }
            if entry.origin.is_none() {
                entry.origin = existing.origin.clone();
            }
            *existing = entry;
        } else {
            self.skills.push(entry);
        }
        self.skills.sort_by(|a, b| a.name.cmp(&b.name));
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.skills.len();
        self.skills.retain(|s| s.name != name);
        self.skills.len() != before
    }

    /// Drop locked skills whose name is no longer in `keep`.
    pub fn retain_names(&mut self, keep: &[String]) {
        self.skills.retain(|s| keep.contains(&s.name));
    }

    pub fn get_instruction(&self, name: &str) -> Option<&LockedInstruction> {
        self.instructions.iter().find(|i| i.name == name)
    }

    /// Insert or replace an instruction entry, keeping entries sorted by name.
    pub fn upsert_instruction(&mut self, entry: LockedInstruction) {
        if let Some(existing) = self.instructions.iter_mut().find(|i| i.name == entry.name) {
            *existing = entry;
        } else {
            self.instructions.push(entry);
        }
        self.instructions.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Drop pinned instructions whose name is no longer in `keep` — stale pins
    /// for removed fragments are pruned by `agentstack lock`.
    pub fn retain_instruction_names(&mut self, keep: &[String]) {
        self.instructions.retain(|i| keep.contains(&i.name));
    }

    pub fn get_server(&self, name: &str) -> Option<&LockedServer> {
        self.servers.iter().find(|s| s.name == name)
    }

    /// The pin for one owned settings key, if this lock records it. Identity is
    /// `(target, key)`: the same key name in two harnesses' settings files is
    /// two independent declarations that may legitimately hold different values.
    pub fn get_setting(&self, target: &str, key: &str) -> Option<&LockedSetting> {
        self.settings
            .iter()
            .find(|s| s.target == target && s.key == key)
    }

    /// Insert or replace a settings-key pin, keeping entries sorted by
    /// `(target, key)` so declaration order in the manifest never shows up as a
    /// lockfile diff.
    pub fn upsert_setting(&mut self, entry: LockedSetting) {
        if let Some(existing) = self
            .settings
            .iter_mut()
            .find(|s| s.target == entry.target && s.key == entry.key)
        {
            *existing = entry;
        } else {
            self.settings.push(entry);
        }
        self.settings
            .sort_by(|a, b| (&a.target, &a.key).cmp(&(&b.target, &b.key)));
    }

    /// Drop settings pins no longer in `keep` — a key removed from the manifest
    /// must not leave a pin behind, or `doctor` would keep measuring drift
    /// against a value nobody declares (the mirror of
    /// [`Lock::retain_instruction_names`]).
    pub fn retain_settings(&mut self, keep: &[(String, String)]) {
        self.settings
            .retain(|s| keep.iter().any(|(t, k)| *t == s.target && *k == s.key));
    }

    pub fn get_executable(&self, path: &str, kind: ExecutableKind) -> Option<&LockedExecutable> {
        self.executables
            .iter()
            .find(|e| e.path == path && e.kind == kind)
    }

    /// Insert or replace an executable pin, keeping entries sorted by
    /// `(path, kind)`.
    pub fn upsert_executable(&mut self, entry: LockedExecutable) {
        if let Some(existing) = self
            .executables
            .iter_mut()
            .find(|e| e.path == entry.path && e.kind == entry.kind)
        {
            *existing = entry;
        } else {
            self.executables.push(entry);
        }
        self.executables
            .sort_by(|a, b| (&a.path, a.kind).cmp(&(&b.path, b.kind)));
    }

    /// Drop executable pins no longer in `keep` — stale pins for paths a
    /// re-lock no longer derives from the manifest are pruned, so the lock
    /// never carries dead pins that mask a renamed payload.
    pub fn retain_executables(&mut self, keep: &[(String, ExecutableKind)]) {
        self.executables
            .retain(|e| keep.iter().any(|(p, k)| *p == e.path && *k == e.kind));
    }

    pub fn get_extension(&self, name: &str) -> Option<&LockedExtension> {
        self.extensions.iter().find(|e| e.name == name)
    }

    /// Insert or replace an extension pin, keeping entries sorted by name.
    pub fn upsert_extension(&mut self, entry: LockedExtension) {
        if let Some(existing) = self.extensions.iter_mut().find(|e| e.name == entry.name) {
            *existing = entry;
        } else {
            self.extensions.push(entry);
        }
        self.extensions.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Drop extension pins whose name is no longer declared — the same
    /// stale-pin pruning rule as instructions (`retain_instruction_names`).
    pub fn retain_extension_names(&mut self, keep: &[String]) {
        self.extensions.retain(|e| keep.contains(&e.name));
    }

    pub fn get_workflow(&self, name: &str) -> Option<&LockedWorkflow> {
        self.workflows.iter().find(|w| w.name == name)
    }

    /// Insert or replace a workflow pin, keeping entries sorted by name and
    /// canonicalizing `roles` to sorted-unique — the pin NEVER stores a role
    /// list in declaration order, so roles drift is a set comparison.
    pub fn upsert_workflow(&mut self, mut entry: LockedWorkflow) {
        entry.roles.sort();
        entry.roles.dedup();
        if let Some(existing) = self.workflows.iter_mut().find(|w| w.name == entry.name) {
            *existing = entry;
        } else {
            self.workflows.push(entry);
        }
        self.workflows.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Drop workflow pins whose name is no longer declared — the same
    /// stale-pin pruning rule as extensions (`retain_extension_names`).
    pub fn retain_workflow_names(&mut self, keep: &[String]) {
        self.workflows.retain(|w| keep.contains(&w.name));
    }

    pub fn get_package(&self, name: &str) -> Option<&LockedPackage> {
        self.packages.iter().find(|p| p.name == name)
    }

    /// Insert or replace a package pin, keeping entries sorted by name and
    /// canonicalizing the three lists it carries.
    ///
    /// Canonicalization here rather than at the call site for the same reason
    /// [`upsert_workflow`] canonicalizes `roles`: these lists feed the lock
    /// bytes, which feed the trust digest, so declaration or map-iteration
    /// order must never fake a change (or, worse, mask one).
    ///
    /// [`upsert_workflow`]: Lock::upsert_workflow
    pub fn upsert_package(&mut self, mut entry: LockedPackage) {
        entry.toolsets.sort();
        entry.toolsets.dedup();
        entry.removed.sort();
        entry.removed.dedup();
        entry.members.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(existing) = self.packages.iter_mut().find(|p| p.name == entry.name) {
            *existing = entry;
        } else {
            self.packages.push(entry);
        }
        self.packages.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Drop package pins whose name is no longer selected — the same stale-pin
    /// pruning rule as extensions and workflows. A package nobody selects must
    /// not leave an expansion behind: a stale member set is a set nothing
    /// re-verifies, and it would keep answering "which members?" long after the
    /// reference that justified it went away.
    pub fn retain_package_names(&mut self, keep: &[String]) {
        self.packages.retain(|p| keep.contains(&p.name));
    }

    /// Insert or replace a server entry, keeping entries sorted by name.
    pub fn upsert_server(&mut self, entry: LockedServer) {
        if let Some(existing) = self.servers.iter_mut().find(|s| s.name == entry.name) {
            *existing = entry;
        } else {
            self.servers.push(entry);
        }
        self.servers.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The typed `Sha256Hex` checksum must serialize as the SAME bare hex the
    /// `String` field emitted — the lockfile feeds the trust digest (rule 4),
    /// so one changed byte would re-gate every trusted project. Also pins the
    /// validate-on-read contract: a malformed checksum fails the parse loudly
    /// instead of riding along to a silent mismatch. NEVER weaken.
    #[test]
    fn locked_checksums_are_bare_hex_on_the_wire_and_validated_on_read() {
        let hex = "a".repeat(64);
        let mut lock = Lock::default();
        lock.skills.push(LockedSkill {
            name: "k".into(),
            source: SkillLockSource::Path,
            path: Some("./k".into()),
            git: None,
            rev: None,
            checksum: Sha256Hex::parse(&hex).unwrap(),
            license: None,
            origin: None,
        });
        let text = toml::to_string_pretty(&lock).unwrap();
        // Bare hex, no `sha256:` prefix, no struct wrapper — byte-identical to
        // the pre-newtype String field.
        assert!(text.contains(&format!("checksum = \"{hex}\"")), "{text}");
        // Round-trips through the real lockfile parser.
        let back: Lock = toml::from_str(&text).unwrap();
        assert_eq!(back.skills[0].checksum.hex(), hex);
        // A garbage checksum is refused at parse (it used to be accepted as an
        // opaque String and only fail later as a mismatch).
        let bad = text.replace(&hex, "not-a-digest");
        assert!(toml::from_str::<Lock>(&bad).is_err(), "{bad}");
    }

    /// The typed sources must serialize to exactly the lowercase strings the
    /// lockfile has always used — the lockfile feeds the trust digest (rule 4),
    /// so a byte change here would spuriously re-gate every project. NEVER
    /// weaken: this pins the wire form, not just that it round-trips.
    #[test]
    fn locked_sources_serialize_to_the_legacy_strings() {
        let mut lock = Lock::default();
        lock.servers.push(LockedServer {
            name: "s".into(),
            source: ServerSource::Library,
            checksum: Sha256Hex::of(b"sha256:aa"),
        });
        lock.skills.push(LockedSkill {
            name: "k".into(),
            source: SkillLockSource::Git,
            path: None,
            git: Some("u".into()),
            rev: None,
            checksum: Sha256Hex::of(b"sha256:bb"),
            license: None,
            origin: None,
        });
        let toml = toml::to_string_pretty(&lock).unwrap();
        assert!(toml.contains("source = \"library\""), "{toml}");
        assert!(toml.contains("source = \"git\""), "{toml}");
        // And each variant maps to its exact tag on the wire.
        for (v, s) in [
            (ServerSource::Inline, "\"inline\""),
            (ServerSource::Library, "\"library\""),
        ] {
            assert_eq!(serde_json::to_string(&v).unwrap(), s);
        }
        for (v, s) in [
            (SkillLockSource::Path, "\"path\""),
            (SkillLockSource::Git, "\"git\""),
        ] {
            assert_eq!(serde_json::to_string(&v).unwrap(), s);
        }
    }

    #[test]
    fn load_checks_the_lock_schema_version() {
        let dir = assert_fs::TempDir::new().unwrap();

        // No lockfile → empty default (unchanged).
        assert_eq!(Lock::load(dir.path()).unwrap(), Lock::default());

        // The current version loads.
        fs::write(
            Lock::path(dir.path()),
            format!("version = {SUPPORTED_LOCK_VERSION}\n"),
        )
        .unwrap();
        assert!(Lock::load(dir.path()).is_ok());

        // An older (v1, pre-instruction-pins) lock still loads, and its
        // in-memory version normalizes to the current one so a genuine write
        // upgrades the file.
        fs::write(Lock::path(dir.path()), "version = 1\n").unwrap();
        assert_eq!(
            Lock::load(dir.path()).unwrap().version,
            SUPPORTED_LOCK_VERSION
        );

        // A future version is refused, not silently misread.
        fs::write(Lock::path(dir.path()), "version = 99\n").unwrap();
        let err = Lock::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("lockfile version 99"), "{err}");
        assert!(err.contains("upgrade agentstack"), "{err}");

        // A lockfile with no version fails deserialization (required field).
        fs::write(Lock::path(dir.path()), "[[skill]]\n").unwrap();
        assert!(Lock::load(dir.path()).is_err());
    }

    /// The manifest stamp: absent means UNKNOWN (behaves exactly as a
    /// pre-stamp lock did), an ordinary `save` drops it, `save_stamped`
    /// records it, and an edit to either declaration layer moves it.
    #[test]
    fn manifest_stamp_is_optional_dropped_by_save_and_moves_with_the_manifest() {
        let dir = assert_fs::TempDir::new().unwrap();
        let manifest = dir.path().join(crate::manifest::load::MANIFEST_FILE);

        // No manifest at all: nothing to be stale against.
        assert_eq!(manifest_stamp(dir.path()), None);
        fs::write(&manifest, "version = 1\n").unwrap();
        let first = manifest_stamp(dir.path()).expect("a readable manifest stamps");

        // A lock with no stamp serializes to the same bytes it always did, and
        // answers "unknown", never "fresh".
        let lock = Lock::default();
        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(!text.contains("manifest_digest"), "{text}");
        assert_eq!(lock.manifest_stale(dir.path()), None);

        // `save_stamped` records it; `Lock::load` reads it back.
        lock.save_stamped(dir.path()).unwrap();
        let loaded = Lock::load(dir.path()).unwrap();
        assert_eq!(loaded.manifest_digest, Some(first.clone()));
        assert_eq!(loaded.manifest_stale(dir.path()), Some(false));

        // An edit to the manifest makes the recorded pin set stale — the
        // whole point: a byte comparison, no resolver run.
        fs::write(&manifest, "version = 1\n[servers.x]\ncommand = \"x\"\n").unwrap();
        assert_eq!(loaded.manifest_stale(dir.path()), Some(true));

        // The overlay counts too: it declares servers, so an overlay-only edit
        // must not slip past the stamp.
        let stamp_before_overlay = manifest_stamp(dir.path());
        fs::write(dir.path().join(crate::manifest::load::LOCAL_FILE), "").unwrap();
        assert_ne!(manifest_stamp(dir.path()), stamp_before_overlay);

        // And an ordinary `save` — the partial-write path — drops the stamp
        // back to unknown rather than leaving a claim it cannot support.
        loaded.save(dir.path()).unwrap();
        let reloaded = Lock::load(dir.path()).unwrap();
        assert_eq!(reloaded.manifest_digest, None);
        assert_eq!(reloaded.manifest_stale(dir.path()), None);
    }

    #[test]
    fn upsert_and_roundtrip() {
        let mut lock = Lock::default();
        lock.upsert(LockedSkill {
            name: "b".into(),
            source: SkillLockSource::Path,
            path: Some("./b".into()),
            git: None,
            rev: None,
            checksum: Sha256Hex::of(b"deadbeef"),
            license: None,
            origin: None,
        });
        lock.upsert(LockedSkill {
            name: "a".into(),
            source: SkillLockSource::Git,
            path: None,
            git: Some("https://x".into()),
            rev: Some("abc".into()),
            checksum: Sha256Hex::of(b"cafe"),
            license: None,
            origin: None,
        });
        // Sorted by name.
        assert_eq!(lock.skills[0].name, "a");
        let text = toml::to_string_pretty(&lock).unwrap();
        let parsed: Lock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.skills, lock.skills);
        assert!(text.contains("[[skill]]"));
    }

    #[test]
    fn server_upsert_sorts_and_roundtrips() {
        let mut lock = Lock::default();
        lock.upsert_server(LockedServer {
            name: "kibana".into(),
            source: ServerSource::Library,
            checksum: Sha256Hex::of(b"cafe"),
        });
        lock.upsert_server(LockedServer {
            name: "figma".into(),
            source: ServerSource::Inline,
            checksum: Sha256Hex::of(b"beef"),
        });
        assert_eq!(lock.servers[0].name, "figma", "sorted by name");
        assert_eq!(
            lock.get_server("kibana").unwrap().source,
            ServerSource::Library
        );

        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("[[server]]"));
        let parsed: Lock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.servers, lock.servers);
    }

    #[test]
    fn executable_upsert_sorts_roundtrips_and_retains() {
        let mut lock = Lock::default();
        lock.upsert_executable(LockedExecutable {
            path: "tools".into(),
            kind: ExecutableKind::Root,
            checksum: Sha256Hex::of(b"cafe"),
        });
        lock.upsert_executable(LockedExecutable {
            path: "scripts/run.sh".into(),
            kind: ExecutableKind::File,
            checksum: Sha256Hex::of(b"beef"),
        });
        assert_eq!(lock.executables[0].path, "scripts/run.sh", "sorted by path");

        // Identity is (path, kind): the same path carries both pin kinds.
        lock.upsert_executable(LockedExecutable {
            path: "tools".into(),
            kind: ExecutableKind::File,
            checksum: Sha256Hex::of(b"f00d"),
        });
        assert_eq!(lock.executables.len(), 3);
        assert_eq!(
            lock.get_executable("tools", ExecutableKind::File)
                .unwrap()
                .checksum,
            Sha256Hex::of(b"f00d")
        );
        assert_eq!(
            lock.get_executable("tools", ExecutableKind::Root)
                .unwrap()
                .checksum,
            Sha256Hex::of(b"cafe")
        );

        // Upsert replaces in place, keyed by both path and kind.
        lock.upsert_executable(LockedExecutable {
            path: "tools".into(),
            kind: ExecutableKind::Root,
            checksum: Sha256Hex::of(b"0000"),
        });
        assert_eq!(lock.executables.len(), 3);
        assert_eq!(
            lock.get_executable("tools", ExecutableKind::Root)
                .unwrap()
                .checksum,
            Sha256Hex::of(b"0000")
        );

        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("[[executable]]"));
        assert!(text.contains("kind = \"root\""));
        assert!(text.contains("kind = \"file\""));
        let parsed: Lock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.executables, lock.executables);

        // Prune to the derived set.
        lock.retain_executables(&[("tools".to_string(), ExecutableKind::Root)]);
        assert_eq!(lock.executables.len(), 1);
        assert!(lock.get_executable("tools", ExecutableKind::Root).is_some());
    }

    #[test]
    fn pre_d3_lock_without_executables_parses_to_empty() {
        // Ruling: additive #[serde(default)] fields, no version bump — an
        // existing v2 lock with no [[executable]] entries must load with an
        // empty pin set (the trust gate and strict verification decide what an
        // absent pin means; parsing never invents one).
        let parsed: Lock =
            toml::from_str(&format!("version = {SUPPORTED_LOCK_VERSION}\n")).unwrap();
        assert!(parsed.executables.is_empty());
    }

    #[test]
    fn extension_upsert_sorts_roundtrips_and_retains() {
        let mut lock = Lock::default();
        lock.upsert_extension(LockedExtension {
            name: "checkpoint".into(),
            target: "pi".into(),
            source: "path".into(),
            path: Some("./extensions/checkpoint".into()),
            git: None,
            rev: None,
            checksum: "cafe".into(),
        });
        lock.upsert_extension(LockedExtension {
            name: "audit-log".into(),
            target: "opencode".into(),
            source: "git".into(),
            path: None,
            git: Some("https://example.com/x.git".into()),
            rev: Some("abc123".into()),
            checksum: "beef".into(),
        });
        assert_eq!(lock.extensions[0].name, "audit-log", "sorted by name");

        // Upsert replaces in place (a re-lock after an edit updates the pin).
        lock.upsert_extension(LockedExtension {
            name: "checkpoint".into(),
            target: "pi".into(),
            source: "path".into(),
            path: Some("./extensions/checkpoint".into()),
            git: None,
            rev: None,
            checksum: "f00d".into(),
        });
        assert_eq!(lock.get_extension("checkpoint").unwrap().checksum, "f00d");

        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("[[extension]]"));
        let parsed: Lock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.extensions, lock.extensions);

        // Prune to the declared set; a v2 lock without [[extension]] parses
        // to an empty pin set (additive field, same rule as executables).
        lock.retain_extension_names(&["audit-log".to_string()]);
        assert!(lock.get_extension("checkpoint").is_none());
        assert!(lock.get_extension("audit-log").is_some());
        let parsed: Lock =
            toml::from_str(&format!("version = {SUPPORTED_LOCK_VERSION}\n")).unwrap();
        assert!(parsed.extensions.is_empty());
    }

    /// D7 W1 witness: workflow pins upsert sorted by name, canonicalize roles
    /// to sorted-unique regardless of declaration order, round-trip through
    /// the lockfile parser, and prune to the declared set. A v2 lock without
    /// `[[workflow]]` parses to an empty pin set (additive field, same rule
    /// as executables/extensions).
    #[test]
    fn workflow_upsert_sorts_roles_roundtrips_and_retains() {
        let mut lock = Lock::default();
        lock.upsert_workflow(LockedWorkflow {
            name: "nightly-review".into(),
            roles: vec!["reviewer".into(), "reader".into(), "reviewer".into()],
            source: "path".into(),
            path: Some("./workflows/nightly-review.js".into()),
            git: None,
            rev: None,
            checksum: Sha256Hex::of(b"cafe"),
            blueprint: None,
            blueprint_checksum: None,
        });
        lock.upsert_workflow(LockedWorkflow {
            name: "audit".into(),
            roles: vec!["reader".into()],
            source: "git".into(),
            path: None,
            git: Some("https://example.com/x.git".into()),
            rev: Some("abc123".into()),
            checksum: Sha256Hex::of(b"beef"),
            blueprint: None,
            blueprint_checksum: None,
        });
        assert_eq!(lock.workflows[0].name, "audit", "sorted by name");
        assert_eq!(
            lock.get_workflow("nightly-review").unwrap().roles,
            vec!["reader".to_string(), "reviewer".to_string()],
            "roles stored sorted-unique, never declaration order"
        );

        // Upsert replaces in place (a re-lock after an edit updates the pin).
        lock.upsert_workflow(LockedWorkflow {
            name: "nightly-review".into(),
            roles: vec!["reader".into()],
            source: "path".into(),
            path: Some("./workflows/nightly-review.js".into()),
            git: None,
            rev: None,
            checksum: Sha256Hex::of(b"f00d"),
            blueprint: None,
            blueprint_checksum: None,
        });
        assert_eq!(
            lock.get_workflow("nightly-review").unwrap().checksum,
            Sha256Hex::of(b"f00d")
        );

        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("[[workflow]]"));
        let parsed: Lock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.workflows, lock.workflows);

        // Prune to the declared set; an existing v2 lock without [[workflow]]
        // parses to an empty pin set.
        lock.retain_workflow_names(&["audit".to_string()]);
        assert!(lock.get_workflow("nightly-review").is_none());
        assert!(lock.get_workflow("audit").is_some());
        let parsed: Lock =
            toml::from_str(&format!("version = {SUPPORTED_LOCK_VERSION}\n")).unwrap();
        assert!(parsed.workflows.is_empty());
    }

    /// An E1-era `[[extension]]` entry — name/target/checksum only, no source
    /// provenance fields — must still parse: `source` defaults to `"path"`
    /// (every pre-E3 extension was a project-local path) and the optional
    /// path/git/rev stay absent.
    #[test]
    fn e1_era_extension_entry_without_source_fields_parses() {
        let parsed: Lock = toml::from_str(&format!(
            "version = {SUPPORTED_LOCK_VERSION}\n\
             [[extension]]\nname = \"checkpoint\"\ntarget = \"pi\"\nchecksum = \"cafe\"\n"
        ))
        .unwrap();
        let ext = parsed.get_extension("checkpoint").expect("pinned");
        assert_eq!(ext.source, "path", "absent source defaults to path");
        assert_eq!(ext.path, None);
        assert_eq!(ext.git, None);
        assert_eq!(ext.rev, None);
        assert_eq!(ext.checksum, "cafe");
    }

    /// W5: a package pin canonicalizes its three lists, round-trips through the
    /// lockfile parser with the exact wire spellings external readers gate on,
    /// and prunes to the selected set. An existing v2 lock without
    /// `[[package]]` parses to an empty pin set (additive field, same rule as
    /// executables/extensions/workflows).
    #[test]
    fn package_upsert_canonicalizes_roundtrips_and_retains() {
        let member = |name: &str, kind, origin| LockedPackageMember {
            name: name.into(),
            kind,
            origin,
            checksum: Sha256Hex::of(name.as_bytes()),
            provenance: format!("package:rust-backend@1.4.0#{name}"),
        };
        let mut lock = Lock::default();
        lock.upsert_package(LockedPackage {
            name: "rust-backend".into(),
            version: "1.4.0".into(),
            source: "library:rust-backend".into(),
            rev: None,
            toolsets: vec!["ops".into(), "backend".into(), "ops".into()],
            removed: vec!["legacy".into(), "legacy".into()],
            members: vec![
                member(
                    "sql-review",
                    PackageMemberKind::Skill,
                    PackageMemberOrigin::Package,
                ),
                member(
                    "house-rules",
                    PackageMemberKind::Instruction,
                    PackageMemberOrigin::ProjectOverride,
                ),
            ],
        });
        let pinned = lock.get_package("rust-backend").expect("pinned");
        assert_eq!(pinned.toolsets, vec!["backend", "ops"], "sorted-unique");
        assert_eq!(pinned.removed, vec!["legacy"], "sorted-unique");
        assert_eq!(
            pinned
                .members
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>(),
            vec!["house-rules", "sql-review"],
            "members sorted by name"
        );

        // The wire spellings are the contract an external reader gates on.
        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("[[package]]"), "{text}");
        assert!(text.contains("[[package.member]]"), "{text}");
        assert!(text.contains("kind = \"instruction\""), "{text}");
        assert!(text.contains("origin = \"project-override\""), "{text}");
        assert!(text.contains("origin = \"package\""), "{text}");
        let parsed: Lock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.packages, lock.packages);

        // Lane is derived from the kind, never stored — an instruction can
        // never be described as going live via the gateway.
        assert_eq!(PackageMemberKind::Instruction.lane(), "rendered");
        assert_eq!(PackageMemberKind::Skill.lane(), "dynamic");
        assert_eq!(PackageMemberKind::Server.lane(), "dynamic");

        lock.retain_package_names(&[]);
        assert!(lock.get_package("rust-backend").is_none());
        let parsed: Lock =
            toml::from_str(&format!("version = {SUPPORTED_LOCK_VERSION}\n")).unwrap();
        assert!(parsed.packages.is_empty());
    }

    #[test]
    fn instruction_upsert_sorts_roundtrips_and_retains() {
        let mut lock = Lock::default();
        lock.upsert_instruction(LockedInstruction {
            name: "style".into(),
            path: "./instructions/style.md".into(),
            checksum: Sha256Hex::of(b"cafe"),
            variants: Vec::new(),
        });
        lock.upsert_instruction(LockedInstruction {
            name: "house".into(),
            path: "./instructions/house.md".into(),
            checksum: Sha256Hex::of(b"beef"),
            variants: vec![LockedInstructionVariant {
                cli: Some("claude-code".into()),
                model: Some("opus".into()),
                path: "./instructions/house.opus.md".into(),
                checksum: Sha256Hex::of(b"0p05"),
            }],
        });
        assert_eq!(lock.instructions[0].name, "house", "sorted by name");

        // Upsert replaces in place.
        lock.upsert_instruction(LockedInstruction {
            name: "house".into(),
            path: "./instructions/house.md".into(),
            checksum: Sha256Hex::of(b"f00d"),
            variants: Vec::new(),
        });
        assert_eq!(
            lock.get_instruction("house").unwrap().checksum,
            Sha256Hex::of(b"f00d")
        );

        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("[[instruction]]"));
        let parsed: Lock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.instructions, lock.instructions);

        // Prune to the declared set.
        lock.retain_instruction_names(&["style".to_string()]);
        assert!(lock.get_instruction("house").is_none());
        assert!(lock.get_instruction("style").is_some());
    }

    /// The settings digest keys on MEANING, not on how the value was typed:
    /// re-ordering object keys is the same consent, changing a value is not.
    /// Without this a cosmetic re-order would re-gate every trusted project and
    /// train the re-review into a reflex.
    #[test]
    fn settings_checksum_ignores_key_order_but_not_values() {
        use serde_json::json;
        let a = json!({"permissions": {"defaultMode": "plan", "allow": ["Bash"]}});
        let b = json!({"permissions": {"allow": ["Bash"], "defaultMode": "plan"}});
        assert_eq!(
            LockedSetting::checksum_of(&a),
            LockedSetting::checksum_of(&b)
        );

        let changed = json!({"permissions": {"allow": ["Bash"], "defaultMode": "acceptEdits"}});
        assert_ne!(
            LockedSetting::checksum_of(&a),
            LockedSetting::checksum_of(&changed),
            "a changed permission mode must move the pin"
        );

        // Array order IS meaning (precedence in an allow/deny list).
        let one = json!(["a", "b"]);
        let other = json!(["b", "a"]);
        assert_ne!(
            LockedSetting::checksum_of(&one),
            LockedSetting::checksum_of(&other)
        );

        // The canonical form is real JSON, so the deposited bytes are readable
        // content rather than an internal display string.
        let canonical = canonical_settings_json(&a);
        let _: serde_json::Value = serde_json::from_str(&canonical).unwrap();

        // A key containing the separator characters cannot forge a collision.
        let sneaky = json!({"a\":1,\"b": 0});
        let plain = json!({"a": 1, "b": 0});
        assert_ne!(
            LockedSetting::checksum_of(&sneaky),
            LockedSetting::checksum_of(&plain)
        );
    }

    /// `[[setting]]` round-trips, sorts by `(target, key)`, upserts in place,
    /// prunes to the declared set — and, crucially, serializes to NOTHING when
    /// empty, so a project that declares no settings keeps a byte-identical
    /// lockfile and is not re-gated by the arrival of a new pin kind.
    #[test]
    fn setting_pins_round_trip_and_stay_absent_when_empty() {
        let mut lock = Lock::default();
        assert!(
            !toml::to_string_pretty(&lock).unwrap().contains("setting"),
            "an empty settings pin list must not appear in the lockfile"
        );

        lock.upsert_setting(LockedSetting {
            target: "codex".into(),
            key: "sandbox".into(),
            checksum: Sha256Hex::of(b"5b0x"),
        });
        lock.upsert_setting(LockedSetting {
            target: "claude-code".into(),
            key: "permissions".into(),
            checksum: Sha256Hex::of(b"p3rm"),
        });
        assert_eq!(lock.settings[0].target, "claude-code", "sorted by target");

        // Same key name under a different target is a different pin.
        lock.upsert_setting(LockedSetting {
            target: "codex".into(),
            key: "permissions".into(),
            checksum: Sha256Hex::of(b"0th3r"),
        });
        assert_eq!(lock.settings.len(), 3);

        // Upsert replaces in place.
        lock.upsert_setting(LockedSetting {
            target: "claude-code".into(),
            key: "permissions".into(),
            checksum: Sha256Hex::of(b"n3w"),
        });
        assert_eq!(
            lock.get_setting("claude-code", "permissions")
                .unwrap()
                .checksum,
            Sha256Hex::of(b"n3w")
        );

        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("[[setting]]"));
        let parsed: Lock = toml::from_str(&text).unwrap();
        assert_eq!(parsed.settings, lock.settings);

        // A lock written before settings pins existed parses with none, and is
        // not an error — the backward-compatibility guarantee, pinned.
        let old: Lock = toml::from_str("version = 2\n").unwrap();
        assert!(old.settings.is_empty());

        // Prune to the declared set.
        lock.retain_settings(&[("codex".to_string(), "sandbox".to_string())]);
        assert_eq!(lock.settings.len(), 1);
        assert!(lock.get_setting("codex", "sandbox").is_some());
        assert!(lock.get_setting("claude-code", "permissions").is_none());
    }
}
