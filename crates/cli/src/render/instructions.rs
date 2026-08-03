//! Compile a manifest's instruction fragments into each harness's instruction
//! file (CLAUDE.md / AGENTS.md), shared + harness-specific (PLAN §9c).

use std::fs;
use std::path::{Path, PathBuf};

use agentstack_core::lock::{LockedPackage, PackageMemberKind};
use anyhow::Result;

use crate::adapter::{AdapterDescriptor, Registry};
use crate::manifest::Manifest;
use crate::scope::Scope;
use crate::util::diff;

use super::merge_md;

/// The computed instruction-file change for one target.
pub struct InstrPlan {
    pub path: PathBuf,
    pub existing: String,
    pub proposed: String,
    /// Fragment names included for this target, in order.
    pub fragments: Vec<String>,
    /// Fragment names whose source file is missing.
    pub missing: Vec<String>,
    /// Fragments excluded by a standing re-gate answer, with the reason shown
    /// to the user. A blocked fragment is refused outright; a keep-pinned
    /// fragment whose approved bytes cannot be verified in the content store
    /// fails closed to the same exclusion rather than compiling the live file
    /// — which holds exactly the change the human declined.
    pub excluded: Vec<(String, String)>,
    /// Fragments compiled from a per-(CLI, model) **variant** rather than the
    /// base body: `(fragment, variant label, why that model)`. Empty is the
    /// ordinary case. Reported rather than derived so a surface names the same
    /// selection the compile actually made.
    pub selected: Vec<(String, String, String)>,
}

impl InstrPlan {
    pub fn changed(&self) -> bool {
        diff::differs(&self.existing, &self.proposed)
    }

    pub fn diff(&self) -> String {
        diff::render(&self.existing, &self.proposed)
    }

    pub fn write(&self) -> Result<()> {
        crate::util::atomic::write(&self.path, &self.proposed)
    }
}

/// Build the instruction-file plan for one target in a scope, or `None` if the
/// adapter has no instruction file for that scope.
///
/// `packages` are this project's **pinned** package expansions (W5). Their
/// instruction members compile into the same managed region as the manifest's
/// own fragments — the rendered lane, always, and never through the gateway
/// (`docs/design/package-layer.md` §"Instruction-member semantics"). They are
/// passed in rather than read from the lock here so each call site states what
/// it means: `unrender` passes an empty slice because it is planning the whole
/// region away, and every rendering caller passes
/// [`crate::package::effective_members`]. A hidden lock read would have made
/// `unrender` silently keep prose it was asked to remove.
///
/// `sel` carries what variant selection needs — the linked library view and the
/// toolset the command was explicitly given
/// (`docs/design/instruction-variants.md`). It is built once per command
/// because the library is one read that must not be repeated per harness per
/// scope.
pub fn plan_instructions(
    manifest: &Manifest,
    desc: &AdapterDescriptor,
    scope: Scope,
    project_dir: &Path,
    packages: &[LockedPackage],
    sel: &crate::instructions::Selecting,
) -> Option<InstrPlan> {
    let spec = desc.instructions.as_ref()?;
    let path = spec.path_for(scope, project_dir)?;
    // The model for THIS harness, from a declaration we can point at — an
    // explicitly selected toolset's `model`, or the value we compile into the
    // CLI's own config. Never sniffed, never defaulted (§"How the model is
    // determined").
    let model = crate::instructions::model_for(manifest, &desc.id, sel.toolset());

    // Standing re-gate answers reshape the compile exactly as they reshape
    // skill delivery in `use` (F6): until this, a blocked or keep-pinned
    // instruction was a recorded decision the compiler never read — it kept
    // compiling the live file, which for keep-pinned is the very change the
    // human declined. Machine-layer fragments never carry decisions (they are
    // filtered out of the trust walk), so an empty decision list is the
    // common, zero-cost case.
    let base = crate::manifest::project_root_of(project_dir);
    let decisions = crate::trust::decisions_for(&base);
    let store = crate::store::Store::default_store();

    let mut blocks: Vec<String> = Vec::new();
    let mut fragments: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut excluded: Vec<(String, String)> = Vec::new();
    let mut selected: Vec<(String, String, String)> = Vec::new();

    for (name, instr) in &manifest.instructions {
        // One predicate gates the compile (adapter match + personal fragments
        // stay out of a repo's project file) — see [`Instruction::compiles_at`].
        if !instr.compiles_at(&desc.id, scope) {
            continue;
        }
        match decisions
            .iter()
            .find(|d| d.kind == "instruction" && d.name == *name)
            .map(|d| &d.answer)
        {
            Some(crate::trust::Decision::Blocked) => {
                excluded.push((
                    name.clone(),
                    "blocked by a standing decision — revisit with `agentstack trust`".into(),
                ));
                continue;
            }
            Some(crate::trust::Decision::KeepPinned { pin }) => {
                // Deliver the APPROVED bytes from the content store, verified
                // against the pin they were approved under — never the live
                // file. Missing or unverifiable approved bytes fail closed to
                // exclusion, the same posture keep-pinned skills take.
                match approved_fragment(&store, pin) {
                    Some(text) => {
                        blocks.push(text.trim_end_matches('\n').to_string());
                        fragments.push(name.clone());
                    }
                    None => excluded.push((
                        name.clone(),
                        "its approved copy is missing or failed verification — excluded until \
                         you review the live content with `agentstack trust`"
                            .into(),
                    )),
                }
                continue;
            }
            None => {}
        }
        // Which bytes this harness gets: most specific matching variant, else
        // the base body. A fragment whose bodies cannot be resolved at all (a
        // sourceless entry no linked source holds, or a library body that fails
        // its containment check) is reported missing rather than compiled from
        // a guess.
        let Ok(bodies) = crate::instructions::bodies(name, instr, project_dir, &sel.library) else {
            missing.push(name.clone());
            continue;
        };
        let chosen = bodies.choose(&desc.id, model.as_deref());
        match fs::read_to_string(&chosen.path) {
            Ok(text) => {
                blocks.push(text.trim_end_matches('\n').to_string());
                fragments.push(name.clone());
                if chosen.variant.is_some() {
                    // Name the variant beside the fragment, so a report reader
                    // can tell WHICH body reached this CLI. Never a silent
                    // substitution: the whole point of the feature is that the
                    // reader knows which paragraph they got.
                    selected.push((name.clone(), chosen.label(), model.clause()));
                }
            }
            Err(_) => missing.push(name.clone()),
        }
    }

    // W5 — package instruction members, after the project's own fragments so a
    // project's prose always has the last word in the region.
    //
    // The bytes come from the content store by digest, exactly like a
    // keep-pinned fragment, and never from the package body in the central
    // library: that is the reproducibility rule (`automatic-delivery.md`), and
    // it is why `lib sync` can move a package ahead without rewriting anybody's
    // CLAUDE.md. `approved_fragment` re-verifies the deposit against the pin,
    // so a tampered store fails closed to `missing` rather than compiling
    // unreviewed prose into the user's daily-driver instruction file.
    for pkg in packages {
        for member in &pkg.members {
            if member.kind != PackageMemberKind::Instruction {
                continue;
            }
            // `<package>:<member>` — a report reader has to be able to tell a
            // package's house rules from one this project wrote.
            let label = format!("{}:{}", pkg.name, member.name);
            match approved_fragment(&store, member.checksum.hex()) {
                Some(text) => {
                    blocks.push(text.trim_end_matches('\n').to_string());
                    fragments.push(label);
                }
                None => missing.push(label),
            }
        }
    }

    let content = blocks.join("\n\n");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let proposed = merge_md::merge_region(&existing, &content);

    Some(InstrPlan {
        path,
        existing,
        proposed,
        fragments,
        missing,
        excluded,
        selected,
    })
}

/// The approved bytes a keep-pinned instruction compiles from: the content
/// store's deposit for `pin`, accepted only if it still hashes to the pin.
/// `None` is the fail-closed answer for a missing, tampered, or malformed
/// deposit — the caller excludes the fragment rather than falling back to the
/// live file.
fn approved_fragment(store: &crate::store::Store, pin: &str) -> Option<String> {
    let hex = pin.rsplit(':').next().unwrap_or(pin);
    let dest = store.root().join("content").join(hex);
    // Shape check by hand rather than through the two-family
    // `store::verified_content`: an instruction pin is exactly "one regular
    // file whose raw bytes hash to the pin", and checking precisely that
    // leaves no room for a same-hex object of the other family to slip
    // through as a fragment.
    if !dest
        .symlink_metadata()
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false)
        || crate::scan::reject_symlinks(&dest).is_err()
    {
        return None;
    }
    let entries: Vec<_> = fs::read_dir(&dest).ok()?.flatten().collect();
    let [only] = entries.as_slice() else {
        return None;
    };
    if !only.file_type().map(|t| t.is_file()).unwrap_or(false) {
        return None;
    }
    let bytes = fs::read(only.path()).ok()?;
    if agentstack_core::digest::sha256_hex(&bytes) != hex {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Resolved targets that CANNOT receive instructions (no adapter instruction
/// file) yet have at least one fragment applying to them — so the fragment
/// silently reaches nowhere on those CLIs. Returned in `target_ids` order, by
/// id. Drives the aggregate warning `instructions` prints so a skills-less/
/// instructions-less target isn't a silent drop. Only 6 of 13 adapters have an
/// instruction file (see `desc.instructions`).
pub fn unreachable_instruction_targets(
    manifest: &Manifest,
    registry: &Registry,
    target_ids: &[String],
) -> Vec<String> {
    target_ids
        .iter()
        .filter(|id| {
            registry
                .get(id)
                .is_some_and(|desc| desc.instructions.is_none())
                && manifest.instructions.values().any(|i| i.applies_to(id))
        })
        .cloned()
        .collect()
}

/// `(fragment name, target id)` pairs where a fragment EXPLICITLY names (not via
/// `"*"`) a registered adapter that has no instruction file — the author asked
/// for a CLI that cannot receive it. Shared by the `instructions` command and
/// `doctor` so both flag the same fragments. Deterministic (manifest fragment
/// order, then declared target order).
pub fn explicit_incapable_instruction_targets(
    manifest: &Manifest,
    registry: &Registry,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (name, instr) in &manifest.instructions {
        for target in &instr.targets {
            if target == "*" {
                continue;
            }
            if registry
                .get(target)
                .is_some_and(|desc| desc.instructions.is_none())
            {
                out.push((name.clone(), target.clone()));
            }
        }
    }
    out
}

/// Whether the instruction file at `path` currently carries agentstack's
/// managed region. This on-disk marker is the persistent record that we
/// compiled (and therefore gitignore) this file: `use`, which never compiles
/// instructions, reads it so its managed `.gitignore` block matches `apply`'s.
pub fn manages_file(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|t| t.contains(merge_md::START))
        .unwrap_or(false)
}

/// Anchor an instruction fragment's declared path: absolute passes through,
/// relative joins the manifest dir. The single rule shared by the compiler
/// above and lock-pin verification (`resolve::instruction_lock_status`) — both
/// must read the same bytes or a pin could verify one file and compile another.
pub fn fragment_source(dir: &Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        dir.join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Manifest {
        toml::from_str(s).unwrap()
    }

    // Cursor and Gemini CLI are registered but have no instruction file; Claude
    // Code and Codex do. The shipped registry backs both assertions.
    #[test]
    fn flags_unreachable_and_explicit_incapable_instruction_targets() {
        let registry = Registry::load().unwrap();
        let m = parse(
            r#"
            version = 1
            [instructions.shared]
            path = "./a.md"
            [instructions.cursoronly]
            path = "./b.md"
            targets = ["cursor"]
            "#,
        );

        // Aggregate: a `"*"` fragment applies to cursor + gemini, neither of
        // which can receive it. A capable target (claude-code) never appears.
        let targets: Vec<String> = ["claude-code", "codex", "cursor", "gemini"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let unreachable = unreachable_instruction_targets(&m, &registry, &targets);
        assert!(unreachable.contains(&"cursor".to_string()));
        assert!(unreachable.contains(&"gemini".to_string()));
        assert!(!unreachable.contains(&"claude-code".to_string()));

        // Per-fragment: only the fragment EXPLICITLY naming an incapable CLI is
        // reported — the `"*"` fragment is not (it targets no one by name).
        let explicit = explicit_incapable_instruction_targets(&m, &registry);
        assert_eq!(
            explicit,
            vec![("cursoronly".to_string(), "cursor".to_string())]
        );
    }
}
