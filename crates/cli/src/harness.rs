//! Harness identity for code that *special-cases* specific CLIs.
//!
//! Adapter ids are an open, data-driven set (one YAML descriptor per CLI), so
//! most of the codebase treats them as opaque strings — registry keys, target
//! lists, config paths. But a few places must branch on a *particular* CLI: the
//! native plugin story is different for Codex (`codex plugin …`) than for
//! Claude Code (`claude plugin … --scope local`), and generic for everything
//! else.
//!
//! [`Harness`] is the hybrid that fits: the two CLIs with bespoke handling are
//! named variants, and every other adapter id is [`Harness::Other`] carrying
//! its id. The payoff over `match id.as_str() { "codex" => …, "claude-code" =>
//! … }` is that the `match` is now *exhaustive* — teaching a third CLI a
//! native plugin flow becomes a compile error at every dispatch site, instead
//! of a silently-missed string literal. (For the TS-minded: this is a
//! discriminated union with an exhaustive switch, versus comparing magic
//! strings.)

/// The classification is TOTAL — every string is some `Harness` — so this is
/// an infallible inherent constructor, not `FromStr`. `FromStr` would imply
/// fallible parsing and hand callers a `Result<_, Infallible>` they'd have to
/// `.unwrap()` (which the workspace `unwrap_used` lint forbids anyway).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Harness {
    /// `claude-code` — `claude plugin …`, scoped installs.
    ClaudeCode,
    /// `codex` — `codex plugin …`, JSON output.
    Codex,
    /// Any other adapter id; carries the id so the generic path can still
    /// name the target where it needs to.
    Other(String),
}

impl Harness {
    /// Classify an adapter id. Total: an unrecognized id is `Other(id)`.
    pub fn from_id(id: &str) -> Harness {
        match id {
            "claude-code" => Harness::ClaudeCode,
            "codex" => Harness::Codex,
            other => Harness::Other(other.to_string()),
        }
    }

    /// The canonical adapter id — the inverse of [`from_id`](Harness::from_id).
    pub fn id(&self) -> &str {
        match self {
            Harness::ClaudeCode => "claude-code",
            Harness::Codex => "codex",
            Harness::Other(id) => id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_and_round_trips() {
        assert_eq!(Harness::from_id("codex"), Harness::Codex);
        assert_eq!(Harness::from_id("claude-code"), Harness::ClaudeCode);
        assert_eq!(
            Harness::from_id("gemini"),
            Harness::Other("gemini".to_string())
        );
        // id() is the inverse of from_id for every input.
        for id in ["codex", "claude-code", "gemini", "cursor"] {
            assert_eq!(Harness::from_id(id).id(), id);
        }
    }
}
