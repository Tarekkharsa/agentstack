//! Compile a manifest's instruction fragments into each harness's instruction
//! file (CLAUDE.md / AGENTS.md), shared + harness-specific (PLAN §9c).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::adapter::AdapterDescriptor;
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
        let src = resolve(project_dir, &instr.path);
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

/// Whether the instruction file at `path` currently carries agentstack's
/// managed region. This on-disk marker is the persistent record that we
/// compiled (and therefore gitignore) this file: `use`, which never compiles
/// instructions, reads it so its managed `.gitignore` block matches `apply`'s.
pub fn manages_file(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|t| t.contains(merge_md::START))
        .unwrap_or(false)
}

fn resolve(dir: &Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        dir.join(p)
    }
}
