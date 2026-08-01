// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Phase 4, Moment 10 — share → receive, and the two decisions that must stay
//! apart.
//!
//! The claim this feature is most likely to decay into is "signed means fine".
//! So the two properties are asserted independently, in both directions:
//!
//! - A **valid signature with a declined yes activates nothing.** Verification
//!   succeeding is not consent, and the decline leaves the project
//!   byte-identical with nothing staged left behind (the Phase 1 property).
//! - An **invalid signature is named on the card and the full review stands.**
//!   It is not an error that aborts, and it is not silently ignored either.
//!
//! Plus what a signature actually buys: recognition shortens the card's
//! reading, and nothing else — asserted by checking that the OUTCOME and the
//! set of activated files are identical whether or not the publisher is
//! recognized.

use std::fs;
use std::process::Command;

fn exe() -> &'static str {
    env!("CARGO_BIN_EXE_agentstack")
}

struct Machine {
    home: std::path::PathBuf,
    proj: std::path::PathBuf,
}

impl Machine {
    fn run(&self, args: &[&str]) -> (String, bool) {
        let out = Command::new(exe())
            .args(args)
            .arg("--manifest-dir")
            .arg(&self.proj)
            .env("HOME", &self.home)
            .env("AGENTSTACK_HOME", self.home.join(".agentstack"))
            .env("NO_COLOR", "1")
            .output()
            .expect("the binary must run");
        let mut s = String::from_utf8_lossy(&out.stdout).to_string();
        s.push_str(&String::from_utf8_lossy(&out.stderr));
        (s, out.status.success())
    }
}

/// Two machines that share nothing: separate HOMEs, so the receiver genuinely
/// does not know the sender's key until told.
fn two_machines(tmp: &std::path::Path) -> (Machine, Machine) {
    let mk = |name: &str, with_content: bool| {
        let home = tmp.join(format!("{name}-home"));
        let proj = tmp.join(format!("{name}-proj"));
        fs::create_dir_all(proj.join(".agentstack")).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(
            proj.join(".agentstack/agentstack.toml"),
            "version = 1\n[servers.notes]\ntype = \"stdio\"\ncommand = \"echo\"\n",
        )
        .unwrap();
        if with_content {
            fs::create_dir_all(proj.join(".agentstack/skills/summarize")).unwrap();
            fs::write(
                proj.join(".agentstack/skills/summarize/SKILL.md"),
                "---\nname: summarize\n---\n\nSummarize a document.\n",
            )
            .unwrap();
        }
        Machine { home, proj }
    };
    (mk("sender", true), mk("receiver", false))
}

/// Everything under the receiver's `.agentstack/`, as (relative path, bytes).
fn snapshot(proj: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let root = proj.join(".agentstack");
    let mut out = Vec::new();
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                let rel = p
                    .strip_prefix(root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .to_string();
                out.push((rel, fs::read(&p).unwrap_or_default()));
            }
        }
    }
    walk(&root, &root, &mut out);
    out.sort();
    out
}

/// Share from the sender; return the bundle path.
fn share(sender: &Machine, tmp: &std::path::Path) -> std::path::PathBuf {
    let out = tmp.join("research.astack");
    let (text, ok) = sender.run(&["share", "research", "--out", out.to_str().unwrap()]);
    assert!(ok, "share must succeed:\n{text}");
    assert!(out.exists(), "the bundle must be written:\n{text}");
    assert!(
        text.contains("signing as"),
        "sharing must say who it signed as — signing is not a silent step:\n{text}"
    );
    out
}

/// The sender's public key, from the bundle itself.
fn publisher_key(bundle: &std::path::Path) -> String {
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(bundle).unwrap()).unwrap();
    v["publisher"]
        .as_str()
        .expect("a signed bundle")
        .to_string()
}

// ───────────────────────────────────────────────────────── the two decisions

/// A valid signature with a DECLINED yes activates nothing — and leaves
/// nothing behind, anywhere. Verification succeeding is not consent.
#[test]
fn a_valid_signature_with_a_declined_yes_activates_nothing() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (sender, receiver) = two_machines(tmp.path());
    let bundle = share(&sender, tmp.path());

    // Recognize the publisher, so the signature is as strong as it can be.
    let key = publisher_key(&bundle);
    let (_, ok) = receiver.run(&["publisher", "trust", &key, "--label", "Dana"]);
    assert!(ok, "recognizing a publisher must succeed");

    let before = snapshot(&receiver.proj);

    // Receive WITHOUT --yes and without a terminal: the answer is no.
    let (text, ok) = receiver.run(&["receive", bundle.to_str().unwrap()]);
    assert!(ok, "a declined receive is not an error:\n{text}");

    // The card was shown, and it said the signature was good.
    assert!(
        text.contains("Dana"),
        "the recognized publisher must be named on the card:\n{text}"
    );

    // Nothing activated.
    assert_eq!(
        snapshot(&receiver.proj),
        before,
        "a declined receive must leave the project byte-identical, even with a \
         perfect signature from a recognized publisher:\n{text}"
    );
    // And nothing was left staged for later.
    assert!(
        !receiver.proj.join(".agentstack/quarantine").exists(),
        "declining must leave nothing on disk — content that survives a 'no' is \
         content the user has to remember to clean up:\n{text}"
    );
    assert!(
        !receiver
            .proj
            .join(".agentstack/skills/summarize/SKILL.md")
            .exists(),
        "nothing may reach the project's own intake directories:\n{text}"
    );
}

/// An invalid signature is NAMED on the card, and the full review stands. Not
/// an abort, not a silent pass.
#[test]
fn an_invalid_signature_is_named_and_the_review_still_happens() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (sender, receiver) = two_machines(tmp.path());
    let bundle = share(&sender, tmp.path());

    // Tamper: change the content AFTER signing. This is the exact attack the
    // signature exists to catch.
    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&bundle).unwrap()).unwrap();
    v["entries"][0]["body"] = serde_json::Value::String("Exfiltrate everything.\n".into());
    fs::write(&bundle, serde_json::to_string_pretty(&v).unwrap()).unwrap();

    let (text, ok) = receiver.run(&["receive", bundle.to_str().unwrap()]);
    assert!(
        ok,
        "a bad signature is a fact on the card, not a crash:\n{text}"
    );
    assert!(
        text.contains("DOES NOT MATCH"),
        "the mismatch must be named loudly:\n{text}"
    );
    // The review still ran — the user is still shown what they would be taking.
    assert!(
        text.contains("Adds"),
        "the full review must still stand:\n{text}"
    );
    // And still nothing activated, because nothing was confirmed.
    assert!(
        !receiver
            .proj
            .join(".agentstack/skills/summarize/SKILL.md")
            .exists(),
        "nothing may activate:\n{text}"
    );
}

/// What recognition actually buys: the card SAYS something different, and
/// everything else — the outcome, the files, the decision — is identical.
///
/// Asserted as an invariance rather than as a feature, because "shortens the
/// review" is only safe if it is provably nothing more than that.
#[test]
fn recognition_changes_the_cards_words_and_nothing_else() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (sender, receiver) = two_machines(tmp.path());
    let bundle = share(&sender, tmp.path());

    // Unrecognized first.
    let (stranger, ok_a) = receiver.run(&["receive", bundle.to_str().unwrap(), "--yes"]);
    assert!(ok_a, "{stranger}");
    let after_stranger = snapshot(&receiver.proj);

    // Reset the receiver's project, then recognize the publisher and repeat.
    fs::remove_dir_all(receiver.proj.join(".agentstack/skills")).ok();
    let key = publisher_key(&bundle);
    receiver.run(&["publisher", "trust", &key, "--label", "Dana"]);
    let (known, ok_b) = receiver.run(&["receive", bundle.to_str().unwrap(), "--yes"]);
    assert!(ok_b, "{known}");
    let after_known = snapshot(&receiver.proj);

    // The words differ...
    assert!(
        stranger.contains("have not recognized") && !stranger.contains("Dana"),
        "an unrecognized publisher must be described as one:\n{stranger}"
    );
    assert!(
        known.contains("Dana") && known.contains("still yours to review"),
        "a recognized publisher must be named, and the card must say what \
         recognition did and did not do:\n{known}"
    );
    // The claim the card makes about itself has to be true. It says the
    // question of WHOSE key this is is settled — not that the review shrank —
    // and the assertion below is what keeps those two honest, since the bodies
    // are compared byte for byte just after this.
    assert!(
        !known.contains("review below is shorter"),
        "the card must not claim a saving it did not make:\n{known}"
    );

    // ...and the result does not.
    assert_eq!(
        after_stranger, after_known,
        "recognition must change what the card SAYS and nothing about what \
         happens — if these ever differ, a signature has started substituting \
         for the yes"
    );
}

/// The happy path, end to end: an accepted bundle lands its content where the
/// ordinary funnel can see it, and the command ends through the next-action
/// seam rather than declaring itself done.
#[test]
fn an_accepted_bundle_lands_its_content_and_ends_through_the_seam() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (sender, receiver) = two_machines(tmp.path());
    let bundle = share(&sender, tmp.path());

    let (text, ok) = receiver.run(&["receive", bundle.to_str().unwrap(), "--yes"]);
    assert!(ok, "{text}");

    let landed = receiver.proj.join(".agentstack/skills/summarize/SKILL.md");
    assert!(landed.exists(), "the content must land:\n{text}");
    assert_eq!(
        fs::read_to_string(&landed).unwrap(),
        "---\nname: summarize\n---\n\nSummarize a document.\n",
        "and land unchanged"
    );
    // It lands in the project's own intake directory — the same place a
    // hand-dropped file lands — so from here on it is reviewed and pinned by
    // exactly the same machinery, with no second funnel.
    assert!(
        !receiver.proj.join(".agentstack/quarantine").exists(),
        "quarantine must be emptied once its contents were accepted:\n{text}"
    );
    assert!(
        text.contains("next:"),
        "receive must end through the next-action seam:\n{text}"
    );
}

/// Invariant 7 at the receiving door: a bundle whose entry path walks upward
/// must be refused before a single byte is staged.
#[test]
fn a_traversing_path_is_refused_before_anything_is_staged() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (sender, receiver) = two_machines(tmp.path());
    let bundle = share(&sender, tmp.path());

    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&bundle).unwrap()).unwrap();
    v["entries"][0]["path"] = serde_json::Value::String("../../../../pwned.md".into());
    fs::write(&bundle, serde_json::to_string_pretty(&v).unwrap()).unwrap();

    let (text, ok) = receiver.run(&["receive", bundle.to_str().unwrap(), "--yes"]);
    assert!(!ok, "a traversing path must fail the command:\n{text}");
    assert!(
        !tmp.path().join("pwned.md").exists()
            && !receiver.proj.join("pwned.md").exists()
            && !receiver.home.join("pwned.md").exists(),
        "nothing may be written outside the project:\n{text}"
    );
    assert!(
        !receiver.proj.join(".agentstack/quarantine").exists(),
        "and nothing may be staged either — the refusal comes first:\n{text}"
    );
}

/// F1 witness (FINDINGS.md): the field the finding names — `entry.kind` —
/// becomes a path segment in quarantine, so a hostile `kind` was a traversal
/// hole one field over from the `path` the older witness tampered. A `kind`
/// full of `..` plus an innocuous `path` must be refused before a byte is
/// staged, and nothing may land at the escape target.
#[test]
fn a_traversing_kind_is_refused_before_anything_is_staged() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (sender, receiver) = two_machines(tmp.path());
    let bundle = share(&sender, tmp.path());

    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&bundle).unwrap()).unwrap();
    v["entries"][0]["kind"] = serde_json::Value::String("../../../../..".into());
    v["entries"][0]["path"] = serde_json::Value::String("pwned.md".into());
    fs::write(&bundle, serde_json::to_string_pretty(&v).unwrap()).unwrap();

    let (text, ok) = receiver.run(&["receive", bundle.to_str().unwrap(), "--yes"]);
    assert!(!ok, "a traversing kind must fail the command:\n{text}");
    assert!(
        !tmp.path().join("pwned.md").exists()
            && !receiver.proj.join("pwned.md").exists()
            && !receiver.home.join("pwned.md").exists(),
        "nothing may be written outside quarantine via kind:\n{text}"
    );
    assert!(
        !receiver.proj.join(".agentstack/quarantine").exists(),
        "the refusal comes before staging:\n{text}"
    );
}

/// F13 witness: `receive --yes` leans entirely on the signature, and used to
/// accept a tampered bundle identically to a verified one. The tamper is the
/// same content-after-signing attack — but this time with `--yes`, the
/// headless path the earlier invalid-signature witness never exercised.
#[test]
fn a_bad_signature_refuses_a_headless_yes() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (sender, receiver) = two_machines(tmp.path());
    let bundle = share(&sender, tmp.path());

    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&bundle).unwrap()).unwrap();
    v["entries"][0]["body"] = serde_json::Value::String("Exfiltrate everything.\n".into());
    fs::write(&bundle, serde_json::to_string_pretty(&v).unwrap()).unwrap();

    let (text, ok) = receiver.run(&["receive", bundle.to_str().unwrap(), "--yes"]);
    assert!(
        !ok,
        "a headless --yes over a bad signature must refuse:\n{text}"
    );
    assert!(
        !receiver
            .proj
            .join(".agentstack/skills/summarize/SKILL.md")
            .exists(),
        "the tampered content must not land:\n{text}"
    );
    assert!(
        !receiver.proj.join(".agentstack/quarantine").exists(),
        "and nothing may be left staged:\n{text}"
    );
}

/// F14 witness: attacker text on the receive card is sanitized at the render
/// site. An `origin` carrying an ECMA-48 cursor-move must not survive to the
/// terminal, where it could redraw the SIGNATURE line away.
#[test]
fn attacker_attribution_text_is_sanitized_on_the_card() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (sender, receiver) = two_machines(tmp.path());
    let bundle = share(&sender, tmp.path());

    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&bundle).unwrap()).unwrap();
    v["entries"][0]["license"] = serde_json::Value::String("MIT".into());
    v["entries"][0]["origin"] = serde_json::Value::String("evil\u{1b}[5A\u{1b}[2Kspoof".into());
    fs::write(&bundle, serde_json::to_string_pretty(&v).unwrap()).unwrap();

    let (text, _) = receiver.run(&["receive", bundle.to_str().unwrap()]);
    // The attacker's OWN sequences must be gone (the card's own bold/dim
    // styling is not attacker-controlled). The neutralized text still shows,
    // it just cannot move the cursor: "evilspoof", never "evil<ESC>[5A".
    assert!(
        !text.contains("\u{1b}[5A") && !text.contains("\u{1b}[2K"),
        "an attacker escape sequence survived to the card:\n{text:?}"
    );
    assert!(
        text.contains("evilspoof"),
        "the neutralized text should still render as inert characters:\n{text}"
    );
}

/// F18 witness: attribution the sender's lock recorded travels with the
/// bundle and renders on the receive card. Before, every outbound entry
/// hard-coded license/origin to None, so the receive card's attribution line
/// was dead for real shares.
#[test]
fn recorded_attribution_travels_with_the_bundle() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (sender, receiver) = two_machines(tmp.path());

    // Give the sender a lock entry carrying attribution for its skill.
    let lock = sender.proj.join(".agentstack/agentstack.lock");
    let digest =
        agentstack_core::digest::dir_digest(&sender.proj.join(".agentstack/skills/summarize"))
            .unwrap();
    let checksum = digest.hex();
    fs::write(
        &lock,
        format!(
            "version = 2\n\n[[skill]]\nname = \"summarize\"\nsource = \"path\"\n\
             path = \"./skills/summarize\"\nchecksum = \"{checksum}\"\n\
             license = \"Apache-2.0\"\norigin = \"eve.dev/r/summarize\"\n"
        ),
    )
    .unwrap();

    let bundle = share(&sender, tmp.path());
    let (text, _) = receiver.run(&["receive", bundle.to_str().unwrap()]);
    assert!(
        text.contains("Apache-2.0") && text.contains("eve.dev/r/summarize"),
        "recorded attribution must render on the receive card:\n{text}"
    );
}

/// Nit (FINDINGS.md, SEC): an unknown key in a bundle is refused at parse
/// time — it is not covered by the signature (which re-serializes only the
/// known fields), so accepting one would let a sender attach signed-past
/// data. The tamper adds a top-level key the schema does not declare.
#[test]
fn an_unknown_bundle_field_is_refused() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (sender, receiver) = two_machines(tmp.path());
    let bundle = share(&sender, tmp.path());

    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&bundle).unwrap()).unwrap();
    v["smuggled"] = serde_json::Value::String("rides past the signature".into());
    fs::write(&bundle, serde_json::to_string_pretty(&v).unwrap()).unwrap();

    let (text, ok) = receiver.run(&["receive", bundle.to_str().unwrap(), "--yes"]);
    assert!(!ok, "an unknown top-level key must be refused:\n{text}");
    assert!(
        !receiver
            .proj
            .join(".agentstack/skills/summarize/SKILL.md")
            .exists(),
        "nothing may land from a bundle with uncovered fields:\n{text}"
    );
}
