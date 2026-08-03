//! Invariant 8, applied to the two bounds that are deliberately PARTIAL.
//!
//! The promotion added two ceilings — an engine-owned buffer cap and a
//! run-total budget on the natives this crate installs — and either one is
//! easy to read as more than it is. A reader who sees "memory is bounded" and
//! "there is an instruction budget" will reasonably conclude the interpreter
//! is contained. It is not, in two specific ways:
//!
//!   1. the memory bound covers untrusted INGRESS, not INTENT — a trusted,
//!      reviewed, pinned script that allocates on purpose is bounded by
//!      nothing but the out-of-thread watchdog; and
//!   2. the native budget covers the natives THIS crate installs — work
//!      inside a single Boa built-in ticks no counter at any setting.
//!
//! Both facts must appear wherever the bound is claimed, and there are exactly
//! two such places: [`POSTURE_LABEL`], which `agentstack workflow report`
//! prints verbatim, and `docs/workflows.md`, which is what a reader actually
//! reads. This test fails if a future edit strengthens the claim in one place
//! without the residual travelling with it — which is precisely how a partial
//! bound turns into a false promise.
//!
//! It is a text lint, and it is honest about being one: it proves the SENTENCE
//! is present, never that the sentence is true. The behavioural halves live in
//! `interpreter_memory_is_bounded_at_every_untrusted_ingress` and
//! `a_total_native_call_budget_bounds_host_natives`.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // crates/workflow → crates → repo root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/workflow sits two levels below the repo root")
        .to_path_buf()
}

/// Markdown emphasis is presentation, not content: `**trusted, reviewed,
/// pinned** script` and `trusted, reviewed, pinned script` make the same claim
/// to a reader, and a lint that fails when someone bolds a word is a lint
/// people delete. Strip `*`/`_` and collapse whitespace (the doc hard-wraps,
/// the const does not) so both sources are compared on their words.
fn normalize(text: &str) -> String {
    text.replace(['*', '_'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The residual sentences, in the wording both sources share.
const RESIDUALS: &[(&str, &str)] = &[
    (
        "the memory bound covers untrusted ingress, not intent",
        "no JS heap cap",
    ),
    (
        "a reviewed script may still allocate on purpose",
        "trusted, reviewed, pinned script that allocates on purpose",
    ),
    (
        "Boa's own built-ins are outside the native budget",
        "ticks no counter at any setting",
    ),
];

#[test]
fn both_partial_bounds_are_stated_wherever_the_bound_is_claimed() {
    let label = normalize(agentstack_workflow::POSTURE_LABEL);
    let docs_path = repo_root().join("docs/workflows.md");
    let docs = normalize(
        &std::fs::read_to_string(&docs_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", docs_path.display())),
    );

    for (residual, phrase) in RESIDUALS {
        assert!(
            label.contains(phrase),
            "POSTURE_LABEL no longer states that {residual} (looked for {phrase:?}). The label \
             is printed verbatim by `agentstack workflow report`; a bound stated there without \
             its residual is a claim the enforcement cannot back."
        );
        assert!(
            docs.contains(phrase),
            "docs/workflows.md no longer states that {residual} (looked for {phrase:?}). The \
             posture label is not what a reader reads — the docs are, and the two must not \
             disagree about what is bounded."
        );
    }

    // Un-hiding the command tree changed discoverability, not enforcement. The
    // sharpest Honest limits claim is the one most likely to be softened by an
    // edit that wants the newly visible feature to sound finished.
    assert!(
        docs.contains("cooperative-guard only"),
        "docs/workflows.md dropped the 'a host-tier step is cooperative-guard only' limit. \
         Workflows became visible because six review findings closed; not one enforcement \
         boundary moved with them, and no copy may imply otherwise."
    );
}
