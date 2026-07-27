//! Release-channel awareness: what version is published, and is this binary
//! older than it.
//!
//! Two consumers, one seam:
//!
//! - `agentstack self update` (`commands::self_update`) — the upgrade path.
//! - `agentstack doctor` — a once-a-day cached note that a newer release
//!   exists. Never blocks, never fails a run, silent when offline.
//!
//! Everything the network says is hostile input (invariant 7). A release tag
//! becomes part of a download URL, so it is validated against a strict charset
//! *and* parsed as a version before it is used for anything; response bodies
//! are read through a byte cap so a hostile/broken endpoint cannot stream us
//! out of memory; and the release title/body are deliberately never printed
//! (a link to the notes is enough, and markdown from a network response is not
//! terminal-safe text).
//!
//! The network is reached through the [`ReleaseSource`] trait rather than
//! inline `reqwest` calls — the same isolation `secret::Resolver` uses — so the
//! version-compare, cache, and install paths are all testable offline.

use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

use crate::util::paths::agentstack_home;

/// The repository releases are published from (matches `install.sh` and the
/// releases link in CHANGELOG.md).
pub const REPO: &str = "Tarekkharsa/agentstack";

/// GitHub's REST host — where "what is the latest release" is asked.
const API_HOST: &str = "https://api.github.com";
/// GitHub's web host — where release *assets* are served from.
const WEB_HOST: &str = "https://github.com";

/// Set (to anything non-empty) to stop AgentStack contacting the release
/// channel at all: no background check in `doctor`, and `self update` refuses
/// rather than reaching out. The cache is still read, so an already-known
/// answer stays available offline.
pub const NO_CHECK_ENV: &str = "AGENTSTACK_NO_UPDATE_CHECK";

/// Point the release channel somewhere else (tests, a private mirror). Same
/// escape hatch shape as `AGENTSTACK_REGISTRY_URL`. It does not weaken the
/// default channel: whoever can set this already controls the process
/// environment, and the checksum gate below still runs against whatever the
/// override serves.
pub const BASE_URL_ENV: &str = "AGENTSTACK_UPDATE_BASE_URL";

/// How long a background check result stays good for.
pub const CHECK_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Response caps. A release JSON body is a few KB and `checksums.txt` a few
/// hundred bytes; the asset is a compressed binary. Anything past these is a
/// broken or hostile endpoint, not a release.
const MAX_JSON: usize = 1 << 20; // 1 MiB
const MAX_TEXT: usize = 1 << 20; // 1 MiB
const MAX_ASSET: usize = 128 << 20; // 128 MiB

// ── versions ────────────────────────────────────────────────────────────────

/// A `MAJOR.MINOR.PATCH` release version. Derived `Ord` compares the fields in
/// declaration order, which is exactly precedence order — that is the whole
/// comparison rule, so there is no hand-written `cmp` to get wrong.
///
/// Pre-release/build suffixes are parsed but not ranked: a tag like
/// `v0.16.0-rc1` is treated as `0.16.0` and, being not *greater* than a
/// released `0.16.0`, never nags someone already on the final build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

impl Version {
    /// Parse `v0.15.0`, `0.15.0`, or `0.15.0-rc1`. Returns `None` for anything
    /// else — bounded first, so a megabyte of digits is rejected on length
    /// rather than parsed.
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        if s.is_empty() || s.len() > 64 {
            return None;
        }
        let s = s.strip_prefix('v').unwrap_or(s);
        // Drop a pre-release / build suffix; the numeric core is what ranks.
        let core = s.split(['-', '+']).next()?;
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Version {
            major,
            minor,
            patch,
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The version of the binary that is running.
pub fn current() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("our own crate version parses")
}

/// A published release: the tag exactly as the channel spelled it (already
/// validated safe for a URL) plus its parsed version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: Version,
}

impl Release {
    /// Accept a tag only if it is a short, path-component-safe token that also
    /// parses as a version. This is the gate that lets the tag be interpolated
    /// into a download URL: no `/`, no `..`, no scheme, no control bytes, no
    /// query separators.
    pub fn from_tag(tag: &str) -> Option<Release> {
        let tag = tag.trim();
        if tag.is_empty() || tag.len() > 64 {
            return None;
        }
        if !tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
        {
            return None;
        }
        // `.` is allowed above (versions need it) so rule out the traversal
        // spellings explicitly.
        if tag == "." || tag == ".." {
            return None;
        }
        Version::parse(tag).map(|version| Release {
            tag: tag.to_string(),
            version,
        })
    }

    /// The human-facing release-notes page for this tag.
    pub fn notes_url(&self) -> String {
        format!("{}/{REPO}/releases/tag/{}", web_base(), self.tag)
    }
}

// ── the network seam ────────────────────────────────────────────────────────

/// Where release information comes from. Implemented by [`GitHub`] in
/// production and by stubs in tests, so nothing below this line needs a
/// network to be exercised.
pub trait ReleaseSource {
    /// The newest published release.
    fn latest(&self) -> Result<Release>;
    /// One file attached to a release, by asset file name.
    fn asset(&self, tag: &str, name: &str) -> Result<Vec<u8>>;
}

/// The real channel: GitHub Releases over HTTPS (rustls), with bounded bodies
/// and explicit timeouts.
pub struct GitHub {
    timeout: Duration,
}

impl GitHub {
    /// For a command the user is watching — worth waiting a few seconds for.
    pub fn interactive() -> GitHub {
        GitHub {
            timeout: Duration::from_secs(15),
        }
    }

    /// For the background check `doctor` runs. Deliberately impatient: a
    /// version note that makes `doctor` feel slow is worse than no note.
    pub fn background() -> GitHub {
        GitHub {
            timeout: Duration::from_secs(3),
        }
    }

    fn client(&self) -> Result<reqwest::blocking::Client> {
        reqwest::blocking::Client::builder()
            .timeout(self.timeout)
            .connect_timeout(std::cmp::min(self.timeout, Duration::from_secs(3)))
            // GitHub's API rejects requests without a User-Agent.
            .user_agent(concat!("agentstack/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building the HTTP client")
    }
}

/// Read at most `max` bytes of a response body. Going over the cap is an
/// error, not a truncation: a body we only half-read is not a body we should
/// try to interpret.
fn read_capped(resp: reqwest::blocking::Response, max: usize) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    // `take(max + 1)` so hitting exactly `max` is distinguishable from
    // overflowing it.
    resp.take(max as u64 + 1)
        .read_to_end(&mut buf)
        .context("reading the response body")?;
    if buf.len() > max {
        bail!("response larger than {max} bytes — refusing to read it");
    }
    Ok(buf)
}

impl ReleaseSource for GitHub {
    fn latest(&self) -> Result<Release> {
        if opted_out() {
            bail!("{NO_CHECK_ENV} is set — not contacting the release channel");
        }
        let url = format!("{}/repos/{REPO}/releases/latest", api_base());
        let resp = self
            .client()?
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .context("asking GitHub for the latest release")?;
        let status = resp.status();
        if !status.is_success() {
            // The status code only — a response body from a failing endpoint
            // is exactly the text not to echo into a terminal.
            bail!("the release channel answered HTTP {}", status.as_u16());
        }
        let body = read_capped(resp, MAX_JSON)?;
        #[derive(serde::Deserialize)]
        struct LatestRelease {
            tag_name: String,
        }
        let parsed: LatestRelease = serde_json::from_slice(&body)
            .context("the release channel returned unexpected JSON")?;
        Release::from_tag(&parsed.tag_name).with_context(|| {
            format!(
                "the release channel returned an unusable tag '{}'",
                crate::text::truncate_chars(&crate::text::sanitize_line(&parsed.tag_name), 32)
            )
        })
    }

    fn asset(&self, tag: &str, name: &str) -> Result<Vec<u8>> {
        if opted_out() {
            bail!("{NO_CHECK_ENV} is set — not contacting the release channel");
        }
        // Both components are ours or validated: `tag` passed
        // [`Release::from_tag`], `name` is built from a compile-time platform
        // table (see `commands::self_update::asset_name`).
        let url = format!("{}/{REPO}/releases/download/{tag}/{name}", web_base());
        let resp = self
            .client()?
            .get(&url)
            .send()
            .with_context(|| format!("downloading {name}"))?;
        let status = resp.status();
        if !status.is_success() {
            bail!(
                "{name} — the release channel answered HTTP {}",
                status.as_u16()
            );
        }
        let max = if name.ends_with(".txt") {
            MAX_TEXT
        } else {
            MAX_ASSET
        };
        read_capped(resp, max)
    }
}

/// True when the user has opted out of all release-channel traffic.
pub fn opted_out() -> bool {
    std::env::var_os(NO_CHECK_ENV).is_some_and(|v| !v.is_empty())
}

fn api_base() -> String {
    override_base().unwrap_or_else(|| API_HOST.to_string())
}

fn web_base() -> String {
    override_base().unwrap_or_else(|| WEB_HOST.to_string())
}

/// The `AGENTSTACK_UPDATE_BASE_URL` override, trimmed of a trailing slash.
pub fn override_base() -> Option<String> {
    let raw = std::env::var(BASE_URL_ENV).ok()?;
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

// ── the cached background check ─────────────────────────────────────────────

/// `~/.agentstack/update-check.json` — one small record, rewritten at most
/// once a day.
fn cache_path() -> PathBuf {
    agentstack_home().join("update-check.json")
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Cache {
    /// Unix seconds of the last *attempt* (successful or not).
    checked_at: u64,
    /// The newest tag seen, or `None` when the last attempt failed. Stored
    /// either way so a failed check backs off for the full TTL instead of
    /// re-dialling on every command.
    #[serde(default)]
    latest: Option<String>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn load_cache() -> Option<Cache> {
    let raw = std::fs::read_to_string(cache_path()).ok()?;
    // A malformed cache is treated as absent, never as an error.
    serde_json::from_str(&raw).ok()
}

fn store_cache(cache: &Cache) {
    // Entirely best-effort: a read-only home must not turn a version note
    // into a failure.
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(cache) {
        let _ = std::fs::write(path, json);
    }
}

/// True when the running binary is a cargo build output (somewhere under a
/// `target/debug` or `target/release` tree).
///
/// Such a binary is updated with `cargo build`, not by downloading a release —
/// it is the same situation `self update` refuses as `Blocker::SourceBuild` —
/// so nagging it about a published version would advertise an upgrade it
/// cannot take. Falling out of that rule, and the reason it is worth naming:
/// this crate's own test binaries live at `target/debug/deps/…`, so the whole
/// test suite is offline by construction rather than by remembering to set an
/// env var in each test.
fn running_from_build_tree() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let comps: Vec<_> = exe
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    comps
        .windows(2)
        .any(|w| w[0] == "target" && (w[1] == "debug" || w[1] == "release"))
}

/// The one-line note `doctor` shows when a newer release exists, or `None`.
///
/// At most one network call per [`CHECK_TTL`], bounded by [`GitHub::background`]'s
/// short timeout, and every failure path is silent.
pub fn advisory() -> Option<String> {
    if running_from_build_tree() {
        return None;
    }
    advisory_with(&GitHub::background(), CHECK_TTL)
}

/// [`advisory`] with the channel and TTL injected — the testable half.
pub fn advisory_with(source: &dyn ReleaseSource, ttl: Duration) -> Option<String> {
    let latest = latest_known(source, ttl)?;
    let current = current();
    if latest.version <= current {
        return None;
    }
    Some(format!(
        "AgentStack {latest} is available (you are on {current}) ↳ agentstack self update",
        latest = latest.version,
    ))
}

/// The newest release we know about: the cache when it is fresh, otherwise one
/// bounded lookup whose outcome (including failure) is cached.
fn latest_known(source: &dyn ReleaseSource, ttl: Duration) -> Option<Release> {
    let cached = load_cache();
    if let Some(c) = &cached {
        let age = now_secs().saturating_sub(c.checked_at);
        if age < ttl.as_secs() {
            return c.latest.as_deref().and_then(Release::from_tag);
        }
    }
    // Opting out means no traffic — but a previously cached answer is already
    // on disk and costs nothing, so it is still honoured.
    if opted_out() {
        return cached.and_then(|c| c.latest.as_deref().and_then(Release::from_tag));
    }
    let found = source.latest().ok();
    store_cache(&Cache {
        checked_at: now_secs(),
        latest: found.as_ref().map(|r| r.tag.clone()),
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A release channel that never touches the network and counts its calls.
    struct Stub {
        tag: &'static str,
        calls: Cell<usize>,
    }

    impl Stub {
        fn new(tag: &'static str) -> Stub {
            Stub {
                tag,
                calls: Cell::new(0),
            }
        }
    }

    impl ReleaseSource for Stub {
        fn latest(&self) -> Result<Release> {
            self.calls.set(self.calls.get() + 1);
            Release::from_tag(self.tag).context("stub tag")
        }
        fn asset(&self, _tag: &str, _name: &str) -> Result<Vec<u8>> {
            bail!("stub has no assets")
        }
    }

    #[test]
    fn versions_parse_and_rank() {
        assert_eq!(Version::parse("v0.15.0"), Version::parse("0.15.0"));
        assert!(Version::parse("0.16.0") > Version::parse("0.15.9"));
        assert!(Version::parse("1.0.0") > Version::parse("0.99.99"));
        assert!(Version::parse("0.15.1") > Version::parse("0.15.0"));
        // A pre-release ranks as its numeric core — never above the release.
        assert_eq!(Version::parse("0.16.0-rc1"), Version::parse("0.16.0"));
        for bad in [
            "",
            "latest",
            "0.15",
            "0.15.0.1",
            "v",
            "-1.0.0",
            "0.15.x",
            &"9".repeat(80),
        ] {
            assert!(Version::parse(bad).is_none(), "should reject {bad:?}");
        }
        assert_eq!(Version::parse("0.15.0").unwrap().to_string(), "0.15.0");
    }

    /// The tag becomes a URL path segment, so the accepting side is a security
    /// boundary: traversal, schemes, query separators, and control bytes are
    /// all refused before anything is fetched.
    #[test]
    fn release_tags_that_could_escape_a_url_are_refused() {
        assert_eq!(Release::from_tag("v0.16.0").unwrap().tag, "v0.16.0");
        for hostile in [
            "../../etc/passwd",
            "v0.16.0/../../x",
            "https://evil.example/x",
            "v0.16.0?x=1",
            "v0.16.0#frag",
            "v0.16.0 -rf",
            "v0.16.0\nSet-Cookie: x",
            "v0.16.0\u{1b}[2J",
            ".",
            "..",
            "",
            &"v0.16.0".repeat(20),
        ] {
            assert!(
                Release::from_tag(hostile).is_none(),
                "should refuse tag {hostile:?}"
            );
        }
    }

    #[test]
    fn advisory_fires_only_for_a_newer_release() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", tmp.path());
        std::env::remove_var(NO_CHECK_ENV);

        // Same version as this build: nothing to say.
        let same = format!("v{}", current());
        let same: &'static str = Box::leak(same.into_boxed_str());
        assert_eq!(advisory_with(&Stub::new(same), CHECK_TTL), None);

        // A newer one: one line, naming both versions and the command.
        std::fs::remove_file(cache_path()).unwrap();
        let note = advisory_with(&Stub::new("v99.0.0"), CHECK_TTL).expect("newer release notes");
        assert!(note.contains("99.0.0"), "{note}");
        assert!(note.contains(&current().to_string()), "{note}");
        assert!(note.contains("agentstack self update"), "{note}");

        std::env::remove_var("AGENTSTACK_HOME");
    }

    /// The check must cost at most one network call per TTL, and must not
    /// re-dial after a failure either — a failed attempt is cached too.
    #[test]
    fn check_hits_the_channel_at_most_once_per_ttl() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", tmp.path());
        std::env::remove_var(NO_CHECK_ENV);

        let stub = Stub::new("v99.0.0");
        assert!(advisory_with(&stub, CHECK_TTL).is_some());
        assert!(advisory_with(&stub, CHECK_TTL).is_some());
        assert!(advisory_with(&stub, CHECK_TTL).is_some());
        assert_eq!(stub.calls.get(), 1, "cache must absorb the repeat calls");

        // An expired TTL lets exactly one more call through.
        assert!(advisory_with(&stub, Duration::from_secs(0)).is_some());
        assert_eq!(stub.calls.get(), 2);

        std::env::remove_var("AGENTSTACK_HOME");
    }

    /// A channel that is down (offline, rate-limited) must be silent — and
    /// must not be retried until the TTL expires.
    #[test]
    fn an_unreachable_channel_is_silent_and_backs_off() {
        struct Down(Cell<usize>);
        impl ReleaseSource for Down {
            fn latest(&self) -> Result<Release> {
                self.0.set(self.0.get() + 1);
                bail!("network is unreachable")
            }
            fn asset(&self, _t: &str, _n: &str) -> Result<Vec<u8>> {
                bail!("no")
            }
        }
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", tmp.path());
        std::env::remove_var(NO_CHECK_ENV);

        let down = Down(Cell::new(0));
        assert_eq!(advisory_with(&down, CHECK_TTL), None);
        assert_eq!(advisory_with(&down, CHECK_TTL), None);
        assert_eq!(down.0.get(), 1, "a failed check backs off for the full TTL");

        std::env::remove_var("AGENTSTACK_HOME");
    }

    /// The test binary itself is the fixture: it lives under `target/debug/deps`,
    /// so the guard that keeps the whole suite (and any source build) off the
    /// network is proven by the suite it protects.
    #[test]
    fn a_build_tree_binary_never_dials_out() {
        assert!(
            running_from_build_tree(),
            "this test binary IS a cargo build output — the guard must see it"
        );
        // Therefore the production entry point is a no-op here, whatever the
        // cache says.
        assert_eq!(advisory(), None);
    }

    /// The opt-out is a traffic ban, not a display toggle: the channel is
    /// never asked.
    #[test]
    fn opt_out_makes_no_network_call() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", tmp.path());
        std::env::set_var(NO_CHECK_ENV, "1");

        let stub = Stub::new("v99.0.0");
        assert_eq!(advisory_with(&stub, CHECK_TTL), None);
        assert_eq!(stub.calls.get(), 0, "opted out means no traffic at all");
        assert!(opted_out());

        std::env::remove_var(NO_CHECK_ENV);
        std::env::remove_var("AGENTSTACK_HOME");
    }
}
