//! The **read half** of `lock --upgrade`: is a newer version of an installed
//! pack resolvable? Nothing here writes, fetches, or clones — it exists so
//! `status` can *offer* an update (design `docs/design/automatic-delivery.md`,
//! §Update model rule 2) without becoming a command that can hang.
//!
//! The write half lives in [`super::upgrade`], which resolves against the
//! remote for real. Keeping the two apart is deliberate: the offer must be
//! cheap and infallible, the upgrade must be accurate, and those are different
//! network postures.

use crate::manifest::Manifest;
use crate::provider::gitpack::{self, GitPackRef};
use crate::store::Store;

/// One installed pack with a newer version resolvable **from local state
/// alone**. Strings are display-sanitized at construction: a tag is repository
/// content, so it reaches a terminal or a JSON consumer only after passing
/// through `text::sanitize_line` (invariant 7).
pub(crate) struct PackUpdate {
    pub name: String,
    pub current: String,
    pub available: String,
}

/// Installed packs with a newer version available, newest-first by declaration
/// order. Empty is the common case and the fail-quiet case alike.
///
/// **Network posture: none.** This runs on the `status` path, which must feel
/// instant and must not fail, so it consults only state already on this
/// machine: the tag pinned in the `[packs.*]` ledger, and the tags git already
/// fetched into this machine's store clone. It never runs `ls-remote`, never
/// fetches, and never clones. Every unanswerable case — no local clone, an
/// unreadable clone, git not installed, a non-version tag, a `catalog:` source
/// (the embedded catalog carries one version per id, so there is no version
/// axis to compare) — reports *nothing* rather than guessing or blocking.
///
/// The honest limit that follows, which every surface rendering this must
/// carry: **an absent offer is not proof of currency.** A tag published since
/// the last local fetch is invisible here. Only `agentstack lock --upgrade
/// <pack>` asks the remote.
pub(crate) fn available_updates(manifest: &Manifest) -> Vec<PackUpdate> {
    let store = Store::default_store();
    let mut out = Vec::new();
    for (name, install) in &manifest.packs {
        let Some(source) = install.source.as_deref() else {
            continue;
        };
        // Only the git rail has a version axis we can read locally.
        let Some(git_ref) = GitPackRef::parse(source) else {
            continue;
        };
        let Some(current) = git_ref.tag.as_deref() else {
            continue;
        };
        let Some(newest) = newest_local_tag(&store, &git_ref.url) else {
            continue;
        };
        // Same comparison `upgrade` uses to refuse a downgrade, so the offer
        // and the command that takes it cannot disagree about "newer".
        let newer = match (gitpack::version_key(&newest), gitpack::version_key(current)) {
            (Some(n), Some(c)) => n > c,
            _ => false,
        };
        if newer {
            out.push(PackUpdate {
                name: crate::text::sanitize_line(name),
                current: crate::text::sanitize_line(current),
                available: crate::text::sanitize_line(&newest),
            });
        }
    }
    out
}

/// The one command that takes these updates, in the **shipped** spelling.
/// `upgrade` is a working name only; `lock --upgrade` is what exists, and a
/// status line that names a verb the binary does not have is worse than none.
/// One pack names it directly; several take `--all`, because there is no
/// single-pack command that covers them and inventing one in copy would be a
/// lie a user discovers at the prompt.
pub(crate) fn fix_command(updates: &[PackUpdate]) -> String {
    match updates {
        [only] => format!("agentstack lock --upgrade {}", only.name),
        _ => "agentstack lock --upgrade --all".to_string(),
    }
}

/// Newest version-shaped tag **already in the local clone**. `None` for every
/// reason the answer cannot be had cheaply — that is the point of the function.
fn newest_local_tag(store: &Store, url: &str) -> Option<String> {
    // No clone here means the pack was installed on another machine (or the
    // store was cleared): unknowable offline, so unknown.
    let (clone, _head) = store.local_git_clone(url)?;
    // `git tag --list` is a local ref read — no remote, no auth, no prompt —
    // and goes through `gitx` because that is the only module allowed to spawn
    // git. A spawn failure, a timeout, or a non-zero exit all read as "no
    // signal", never as an error the caller has to handle.
    let out = crate::gitx::run_raw(
        crate::gitx::Profile::Ingest,
        &["tag", "--list"],
        Some(&clone),
    )
    .ok()?;
    if !out.success {
        return None;
    }
    let tags: Vec<String> = out
        .stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    gitpack::latest_version_tag(&tags).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Manifest {
        toml::from_str(s).expect("manifest parses")
    }

    /// A `catalog:` pack has no version axis to compare against, and a pack
    /// whose clone is not on this machine cannot be checked at all. Both must
    /// stay silent — reporting "current" for either would be the exact
    /// false-currency claim the contract forbids.
    #[test]
    fn unknowable_sources_offer_nothing() {
        let m = parse(
            r#"
            version = 1

            [packs.from-catalog]
            version = "0.1.0"
            description = "catalog pack"
            source = "catalog:linear-pack"

            [packs.never-cloned]
            version = "v0.1.0"
            description = "git pack"
            source = "git:https://example.invalid/nope@v0.1.0"
            "#,
        );
        assert!(available_updates(&m).is_empty());
    }

    /// The `fix` string is the shipped surface, and it changes shape rather
    /// than naming a pack it cannot cover.
    #[test]
    fn fix_command_names_the_shipped_verb() {
        let one = vec![PackUpdate {
            name: "acme".into(),
            current: "v0.1.0".into(),
            available: "v0.2.0".into(),
        }];
        assert_eq!(fix_command(&one), "agentstack lock --upgrade acme");
        let two = vec![
            PackUpdate {
                name: "acme".into(),
                current: "v0.1.0".into(),
                available: "v0.2.0".into(),
            },
            PackUpdate {
                name: "beta".into(),
                current: "v1.0.0".into(),
                available: "v1.1.0".into(),
            },
        ];
        assert_eq!(fix_command(&two), "agentstack lock --upgrade --all");
    }
}
