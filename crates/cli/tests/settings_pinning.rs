// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! G18 WITNESS: `[settings.*]` values are pinned, and drift is reported.
//!
//! The gap this closes, stated plainly: a `[settings.claude-code]` value is
//! merged into the harness's own `settings.json`, and `permissions.defaultMode`
//! lives there — so a settings value is security-relevant. Before this there was
//! no `LockedSetting`, and nothing anywhere compared what sits in that file
//! against what a human approved. `doctor` reported destination and shape only.
//!
//! What the pin claims is a chain of two legs, and this file witnesses both
//! separately, because they have different fixes:
//!
//!   1. declaration ↔ pin — the declared value still digests to what
//!      `agentstack.lock` records (`agentstack lock --write`);
//!   2. declaration ↔ disk — re-merging that key into the live settings file
//!      would change nothing (`agentstack apply --write`).
//!
//! And two things it deliberately does NOT claim, each with its own test:
//! bytes of the harness's file that AgentStack never declared (an unrelated
//! user edit is not drift), and delivery (an unpinned settings key does not
//! refuse a render — see `LockedSetting`'s doc comment for why).
//!
//! Driven through the real binary, because the claim is about what the shipped
//! `lock` → `trust` → `apply` → `doctor` sequence records and reports.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// A repo-shaped project declaring two owned top-level keys in Claude Code's
/// settings file. `permissions` is the security-relevant one the gap was named
/// for; `model` is a plain scalar, so the witness covers both shapes.
const MANIFEST: &str = "version = 1\n\
    \n[targets]\ndefault = [\"claude-code\"]\n\
    \n[settings.claude-code]\n\
    model = \"opus\"\n\
    permissions = { defaultMode = \"plan\", allow = [\"Bash(git:*)\"] }\n";

struct Fixture {
    _tmp: assert_fs::TempDir,
    home: PathBuf,
    proj: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = assert_fs::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".agentstack")).unwrap();
        // A `.git` dir makes this the ordinary repo shape, so `apply` and
        // `doctor` both judge the PROJECT settings file.
        let proj = tmp.path().join("proj");
        fs::create_dir_all(proj.join(".git")).unwrap();
        fs::create_dir_all(proj.join(".agentstack")).unwrap();
        fs::write(proj.join(".agentstack/agentstack.toml"), MANIFEST).unwrap();
        Fixture {
            _tmp: tmp,
            home,
            proj,
        }
    }

    fn run(&self, args: &[&str]) -> (bool, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
            .args(args)
            .current_dir(&self.proj)
            .env("HOME", &self.home)
            .env("AGENTSTACK_HOME", self.home.join(".agentstack"))
            .output()
            .unwrap();
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        (out.status.success(), text)
    }

    /// The real ceremony: preview for the digest, then grant bound to it.
    fn trust(&self) {
        let (ok, preview) = self.run(&["trust", "--preview"]);
        assert!(ok, "trust --preview failed: {preview}");
        let surface: serde_json::Value = serde_json::from_str(&preview).unwrap();
        let digest = surface["surface_digest"].as_str().unwrap().to_string();
        let (ok, out) = self.run(&["trust", ".", "--yes", "--consented-digest", &digest]);
        assert!(ok, "trust failed: {out}");
    }

    fn doctor(&self) -> serde_json::Value {
        let (_, out) = self.run(&["doctor", "--json", "--all"]);
        serde_json::from_str(&out)
            .unwrap_or_else(|e| panic!("doctor --json is not JSON ({e}): {out}"))
    }

    fn lock_text(&self) -> String {
        fs::read_to_string(self.proj.join(".agentstack/agentstack.lock")).unwrap_or_default()
    }

    fn settings_path(&self) -> PathBuf {
        self.proj.join(".claude/settings.json")
    }

    fn settings_json(&self) -> serde_json::Value {
        let text = fs::read_to_string(self.settings_path()).unwrap_or_default();
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("settings.json unparseable: {e}"))
    }

    fn write_settings_json(&self, value: &serde_json::Value) {
        fs::write(
            self.settings_path(),
            format!("{}\n", serde_json::to_string_pretty(value).unwrap()),
        )
        .unwrap();
    }
}

/// Every line of the named `doctor` section, as `(level, msg)`.
fn section(report: &serde_json::Value, title: &str) -> Vec<(String, String)> {
    report["sections"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["title"] == title)
        .flat_map(|s| s["lines"].as_array().unwrap())
        .map(|l| {
            (
                l["level"].as_str().unwrap_or_default().to_string(),
                l["msg"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn settings_lines(fx: &Fixture) -> Vec<(String, String)> {
    section(&fx.doctor(), "Settings")
}

fn joined(lines: &[(String, String)]) -> String {
    lines
        .iter()
        .map(|(l, m)| format!("[{l}] {m}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// THE WITNESS. Registered in `tools/check-structure.py`'s WITNESS_REGISTRY as
/// the settings kind's drift/pin test; renaming it fails the structural lint,
/// which is the point.
///
/// One test, one story, walked end to end: pin → approve → deliver → tamper on
/// each leg in turn. Splitting it would mean four fixtures re-walking the same
/// four commands to assert one line each, and the ORDER is load-bearing — "on
/// disk no longer holds the declared value" is only a meaningful finding after
/// a write that put the declared value there.
#[test]
fn settings_keys_are_pinned_and_disk_drift_is_reported() {
    let fx = Fixture::new();

    // ── Before any lock: honest about being unpinned, never an error. This is
    // also the backward-compatibility state — a lockfile written before G18
    // carries no `[[setting]]` rows and looks exactly like this.
    let lines = settings_lines(&fx);
    assert!(
        lines.iter().any(|(lvl, m)| lvl == "warn"
            && m.contains("not pinned in agentstack.lock")
            && m.contains("agentstack lock --write")),
        "an unpinned settings block must say so and name the fix:\n{}",
        joined(&lines)
    );
    assert!(
        !lines.iter().any(|(lvl, _)| lvl == "error"),
        "being unpinned is not an error — a project mid-upgrade must not fail:\n{}",
        joined(&lines)
    );

    // ── `lock --write` pins each OWNED KEY separately, not the block.
    let (ok, out) = fx.run(&["lock", "--write"]);
    assert!(ok, "lock failed:\n{out}");
    let lock = fx.lock_text();
    assert!(
        lock.contains("[[setting]]"),
        "no settings pins in the lock:\n{lock}"
    );
    for key in ["model", "permissions"] {
        assert!(
            lock.contains(&format!("key = \"{key}\"")),
            "`{key}` is declared but not pinned — the grain is the key:\n{lock}"
        );
    }
    assert!(
        lock.contains("target = \"claude-code\""),
        "the pin must name the harness whose file it lands in:\n{lock}"
    );
    // The pin deposits the bytes it hashed, so a later re-gate can show WHICH
    // lines of the value moved rather than only that it changed.
    let checksums: Vec<String> = lock
        .split("[[setting]]")
        .skip(1)
        .filter_map(|b| {
            b.lines()
                .find_map(|l| Some(l.trim().strip_prefix("checksum")?.trim_start()))
                .and_then(|r| r.strip_prefix('='))
                .map(|r| r.trim().trim_matches('"').to_string())
        })
        .collect();
    assert_eq!(checksums.len(), 2, "expected one checksum per key:\n{lock}");
    for hex in &checksums {
        assert!(
            fx.home.join(".agentstack/store/content").join(hex).is_dir(),
            "settings pin {hex} deposited no bytes — a re-gate would have \
             nothing to diff against"
        );
    }

    // ── Approve, deliver, and only then is the strong sentence said.
    fx.trust();
    let (ok, out) = fx.run(&["apply", "--write"]);
    assert!(ok, "apply failed:\n{out}");
    assert_eq!(fx.settings_json()["model"], "opus");
    assert_eq!(fx.settings_json()["permissions"]["defaultMode"], "plan");

    let lines = settings_lines(&fx);
    assert!(
        lines
            .iter()
            .any(|(lvl, m)| lvl == "ok" && m.contains("match agentstack.lock")),
        "a pinned, applied, unmodified project must read clean:\n{}",
        joined(&lines)
    );

    // ── LEG 2, the gap's headline case. Hand-edit the delivered
    // `permissions.defaultMode` in the harness's own file. Nothing in the
    // manifest or the lock moved, so trust still holds — and before G18 the
    // only thing said about this was "2 keys not yet in <path>", which reads as
    // a pending apply rather than as a value that was changed underneath you.
    let mut tampered = fx.settings_json();
    tampered["permissions"]["defaultMode"] = serde_json::json!("bypassPermissions");
    fx.write_settings_json(&tampered);

    let lines = settings_lines(&fx);
    assert!(
        lines.iter().any(|(lvl, m)| lvl == "warn"
            && m.contains("drifted from the declared value")
            && m.contains("permissions")
            && m.contains("agentstack apply --write")),
        "an edited owned key must be reported as drift, by name:\n{}",
        joined(&lines)
    );
    assert!(
        !lines
            .iter()
            .any(|(_, m)| m.contains("drifted from the declared value") && m.contains("model")),
        "an untouched key must not be swept into the drift line:\n{}",
        joined(&lines)
    );

    // The named fix really fixes it — no reproduced loop.
    let (ok, out) = fx.run(&["apply", "--write"]);
    assert!(ok, "apply failed:\n{out}");
    assert_eq!(fx.settings_json()["permissions"]["defaultMode"], "plan");
    let lines = settings_lines(&fx);
    assert!(
        lines
            .iter()
            .any(|(lvl, m)| lvl == "ok" && m.contains("match agentstack.lock")),
        "the named fix did not clear the finding:\n{}",
        joined(&lines)
    );

    // ── WHAT THE PIN DOES NOT CLAIM. A key AgentStack never declared is the
    // user's own; editing it is not drift. Without the per-key grain this would
    // be indistinguishable from a tampered value.
    let mut theirs = fx.settings_json();
    theirs["statusLine"] = serde_json::json!({"type": "command", "command": "date"});
    fx.write_settings_json(&theirs);
    let lines = settings_lines(&fx);
    assert!(
        lines
            .iter()
            .any(|(lvl, m)| lvl == "ok" && m.contains("match agentstack.lock")),
        "an unrelated key in the harness's own file was read as drift:\n{}",
        joined(&lines)
    );

    // ── LEG 1. Edit the DECLARED value. The manifest bytes re-gate trust (that
    // was always true), and now the pin says the lock is stale as well, naming
    // the command that re-records it.
    fs::write(
        fx.proj.join(".agentstack/agentstack.toml"),
        MANIFEST.replace("defaultMode = \"plan\"", "defaultMode = \"acceptEdits\""),
    )
    .unwrap();

    let lines = settings_lines(&fx);
    assert!(
        lines.iter().any(|(lvl, m)| lvl == "warn"
            && m.contains("moved since the lock was written")
            && m.contains("permissions")
            && m.contains("agentstack lock --write")),
        "an edited declaration must be reported against its pin:\n{}",
        joined(&lines)
    );

    // The edit re-gates consent — the same review as any other manifest edit.
    let (_, out) = fx.run(&["trust", "--check"]);
    assert!(
        !out.contains("\"trusted\""),
        "editing a settings value did not re-gate trust: {out}"
    );

    // Re-locking moves the pin (new consent), and re-approving closes the loop.
    let (ok, out) = fx.run(&["lock", "--write"]);
    assert!(ok, "re-lock failed:\n{out}");
    assert_ne!(
        fx.lock_text(),
        lock,
        "a changed settings value left the lockfile untouched — the pin is inert"
    );
    fx.trust();
    let (ok, out) = fx.run(&["apply", "--write"]);
    assert!(ok, "apply failed:\n{out}");
    assert_eq!(
        fx.settings_json()["permissions"]["defaultMode"],
        "acceptEdits"
    );
    let lines = settings_lines(&fx);
    assert!(
        lines
            .iter()
            .any(|(lvl, m)| lvl == "ok" && m.contains("match agentstack.lock")),
        "the full chain did not close:\n{}",
        joined(&lines)
    );

    // ── PRUNE. Dropping a key from the manifest must drop its pin, or `doctor`
    // would keep measuring against a value nobody declares.
    fs::write(
        fx.proj.join(".agentstack/agentstack.toml"),
        MANIFEST
            .replace("defaultMode = \"plan\"", "defaultMode = \"acceptEdits\"")
            .replace("model = \"opus\"\n", ""),
    )
    .unwrap();
    let (ok, out) = fx.run(&["lock", "--write"]);
    assert!(ok, "re-lock failed:\n{out}");
    assert!(
        !fx.lock_text().contains("key = \"model\""),
        "an undeclared key kept its pin:\n{}",
        fx.lock_text()
    );
}

/// BACKWARD COMPATIBILITY, on its own so it cannot be lost in the story above:
/// an `agentstack.lock` written before settings pins existed must keep working
/// — degrade to the pre-G18 behaviour, never fail the project — and the next
/// `lock --write` backfills the pins with no migration step.
#[test]
fn a_lock_without_settings_pins_still_works_and_is_backfilled() {
    let fx = Fixture::new();
    let (ok, out) = fx.run(&["lock", "--write"]);
    assert!(ok, "lock failed:\n{out}");

    // Strip every `[[setting]]` block — exactly the shape of a pre-G18 lock.
    let stripped: String = {
        let text = fx.lock_text();
        let mut out = String::new();
        let mut skipping = false;
        for line in text.lines() {
            if line.trim_start().starts_with("[[") {
                skipping = line.trim_start().starts_with("[[setting]]");
            }
            if !skipping {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    };
    assert!(!stripped.contains("[[setting]]"));
    fs::write(fx.proj.join(".agentstack/agentstack.lock"), &stripped).unwrap();

    // Every surface still answers, and none of them fails.
    fx.trust();
    let (ok, out) = fx.run(&["apply", "--write"]);
    assert!(ok, "apply failed over an unpinned settings block:\n{out}");
    assert_eq!(fx.settings_json()["model"], "opus");

    let lines = settings_lines(&fx);
    assert!(
        !lines.iter().any(|(lvl, _)| lvl == "error"),
        "an older lock was treated as a fault rather than as unpinned:\n{}",
        joined(&lines)
    );
    assert!(
        lines
            .iter()
            .any(|(lvl, m)| lvl == "warn" && m.contains("not pinned in agentstack.lock")),
        "the unpinned state must be stated, not silently tolerated:\n{}",
        joined(&lines)
    );

    // A later `lock --write` backfills — no migration command exists or is
    // needed.
    let (ok, out) = fx.run(&["lock", "--write"]);
    assert!(ok, "backfill lock failed:\n{out}");
    assert!(
        fx.lock_text().contains("[[setting]]"),
        "the re-lock did not backfill the missing pins:\n{}",
        fx.lock_text()
    );
    fx.trust();
    let lines = settings_lines(&fx);
    assert!(
        lines
            .iter()
            .any(|(lvl, m)| lvl == "ok" && m.contains("match agentstack.lock")),
        "after the backfill the chain should hold:\n{}",
        joined(&lines)
    );
}
