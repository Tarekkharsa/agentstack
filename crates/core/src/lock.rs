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
        }
    }
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
}

impl Lock {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(LOCK_FILE)
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let path = Self::path(dir);
        // Bounded: a cloned repo's lockfile is hostile input (rule 7).
        match crate::util::read_to_string_bounded(&path, crate::util::MAX_CONFIG_BYTES) {
            Ok(text) => {
                let lock: Lock =
                    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
                crate::util::check_schema_version(
                    lock.version,
                    SUPPORTED_LOCK_VERSION,
                    "lockfile",
                    &path,
                )?;
                // Normalize the in-memory version so struct equality (and the
                // callers' "no-op if unchanged" save checks) stays honest: an
                // untouched older lock is never rewritten just to bump its
                // version, but any genuine write upgrades the file.
                let mut lock = lock;
                lock.version = SUPPORTED_LOCK_VERSION;
                Ok(lock)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Lock::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        let path = Self::path(dir);
        let text = toml::to_string_pretty(self)?;
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }

    pub fn get(&self, name: &str) -> Option<&LockedSkill> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// Insert or replace a skill entry, keeping entries sorted by name.
    pub fn upsert(&mut self, entry: LockedSkill) {
        if let Some(existing) = self.skills.iter_mut().find(|s| s.name == entry.name) {
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
        });
        lock.upsert(LockedSkill {
            name: "a".into(),
            source: SkillLockSource::Git,
            path: None,
            git: Some("https://x".into()),
            rev: Some("abc".into()),
            checksum: Sha256Hex::of(b"cafe"),
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
        });
        lock.upsert_workflow(LockedWorkflow {
            name: "audit".into(),
            roles: vec!["reader".into()],
            source: "git".into(),
            path: None,
            git: Some("https://example.com/x.git".into()),
            rev: Some("abc123".into()),
            checksum: Sha256Hex::of(b"beef"),
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

    #[test]
    fn instruction_upsert_sorts_roundtrips_and_retains() {
        let mut lock = Lock::default();
        lock.upsert_instruction(LockedInstruction {
            name: "style".into(),
            path: "./instructions/style.md".into(),
            checksum: Sha256Hex::of(b"cafe"),
        });
        lock.upsert_instruction(LockedInstruction {
            name: "house".into(),
            path: "./instructions/house.md".into(),
            checksum: Sha256Hex::of(b"beef"),
        });
        assert_eq!(lock.instructions[0].name, "house", "sorted by name");

        // Upsert replaces in place.
        lock.upsert_instruction(LockedInstruction {
            name: "house".into(),
            path: "./instructions/house.md".into(),
            checksum: Sha256Hex::of(b"f00d"),
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
}
