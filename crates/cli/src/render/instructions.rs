//! Compile a manifest's instruction fragments into each harness's instruction
//! file (CLAUDE.md / AGENTS.md), shared + harness-specific (PLAN §9c).

use std::fs;
use std::path::{Path, PathBuf};

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
pub fn plan_instructions(
    manifest: &Manifest,
    desc: &AdapterDescriptor,
    scope: Scope,
    project_dir: &Path,
) -> Option<InstrPlan> {
    let spec = desc.instructions.as_ref()?;
    let path = spec.path_for(scope, project_dir)?;

    let mut blocks: Vec<String> = Vec::new();
    let mut fragments: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();

    for (name, instr) in &manifest.instructions {
        // One predicate gates the compile (adapter match + personal fragments
        // stay out of a repo's project file) — see [`Instruction::compiles_at`].
        if !instr.compiles_at(&desc.id, scope) {
            continue;
        }
        let src = fragment_source(project_dir, &instr.path);
        match fs::read_to_string(&src) {
            Ok(text) => {
                blocks.push(text.trim_end_matches('\n').to_string());
                fragments.push(name.clone());
            }
            Err(_) => missing.push(name.clone()),
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
    })
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
