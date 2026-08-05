//! Compile a manifest's instruction fragments into each harness's instruction
//! file (CLAUDE.md / AGENTS.md), shared + harness-specific (PLAN §9c).

use std::fs;
use std::path::{Path, PathBuf};

use agentstack_core::lock::{LockedPackage, PackageMemberKind};
use anyhow::Result;

use crate::adapter::{AdapterDescriptor, Registry};
use crate::manifest::Manifest;
use crate::render::PriorTrust;
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
    /// Set when the project's trust state forbids compiling the project-content
    /// fragments this plan carries (see the gate in [`trust_refusal`]). The plan
    /// is still built so the caller can SHOW what is being withheld;
    /// [`InstrPlan::write`] refuses, so a plan in this state can never reach
    /// disk.
    pub refusal: Option<String>,
}

impl InstrPlan {
    pub fn changed(&self) -> bool {
        diff::differs(&self.existing, &self.proposed)
    }

    pub fn diff(&self) -> String {
        diff::render(&self.existing, &self.proposed)
    }

    /// The write choke point. The trust refusal is enforced HERE, not only at
    /// the call sites, so a caller that forgets to read `refusal` still cannot
    /// put an untrusted project's prose into the managed region a harness reads
    /// straight into a model's context. A refused plan writes nothing at all,
    /// which leaves an existing region exactly as the human last approved it —
    /// withholding unreviewed prose, never deleting reviewed prose.
    pub fn write(&self) -> Result<()> {
        if let Some(why) = &self.refusal {
            anyhow::bail!("{why}");
        }
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
///
/// `prior` is the trust state as of the calling command's START —
/// [`PriorTrust::STRICT`] unless the caller captured one before writing its own
/// manifest/lock bytes (`lock` and `upgrade` do; see [`trust_refusal`]).
// Seven arguments, and the seventh is the trust answer this plan is judged
// against. Passing it explicitly is the point: `PriorTrust::STRICT` is the
// `Default`, so a call site that forgets costs a refusal it will print, never a
// delivery it should not have made.
#[allow(clippy::too_many_arguments)]
pub fn plan_instructions(
    manifest: &Manifest,
    desc: &AdapterDescriptor,
    scope: Scope,
    project_dir: &Path,
    packages: &[LockedPackage],
    sel: &crate::instructions::Selecting,
    prior: PriorTrust,
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

    // The gate is judged BEFORE anything is compiled, over exactly the
    // fragments that are the PROJECT's content — the manifest's own (never the
    // machine layer's) plus every package instruction member. Naming them here,
    // rather than counting whatever the loop below happened to compile, keeps
    // the verdict independent of standing decisions and missing sources: a
    // fragment excluded for another reason must not also relax the gate.
    let gated: Vec<String> = manifest
        .instructions
        .iter()
        .filter(|(_, i)| !i.from_user_layer && i.compiles_at(&desc.id, scope))
        .map(|(name, _)| name.clone())
        .chain(packages.iter().flat_map(|pkg| {
            pkg.members
                .iter()
                .filter(|m| m.kind == PackageMemberKind::Instruction)
                .map(|m| format!("{}:{}", pkg.name, m.name))
        }))
        .collect();
    let refusal = trust_refusal(&gated, project_dir, prior);

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
        refusal,
    })
}

/// The trust gate for instruction delivery — the same rule `render::extensions`
/// states as "untrusted means inert" (invariant 3), and `render::hooks`,
/// `render::apply` and `render::skills` apply to the other three kinds, applied
/// to the one that is not executable at all.
///
/// That is the whole reason it belongs here rather than being excused. An
/// instruction fragment is prose, and the managed region of `CLAUDE.md` /
/// `AGENTS.md` is read by the harness itself, at its own startup, straight into
/// a model's context — no agentstack process is in the path and there is
/// nothing to intercept at run time (`docs/ENFORCEMENT.md`, Instructions,
/// runtime). Compilation is therefore the delivery moment, and it is exactly
/// the injection channel the consent ceremony exists to gate: a repository that
/// nobody reviewed does not get to write the model's standing orders.
///
/// The LOCK gate stays exactly as it was: the pre-compile drift check in
/// `commands::apply` lets an unpinned fragment through because recording that
/// first pin IS the consenting act. This is a second, orthogonal question —
/// "did a human review this project?" — not a stricter version of the first.
///
/// Returns the refusal message, or `None` when there is nothing to refuse.
/// Deliberately NOT gated:
///   * an empty `gated` set — a prune (an empty manifest, `unrender`,
///     `uninstall`) plans the region AWAY, which is the inert direction and
///     must keep working for an untrusted project, exactly as hook, server and
///     extension pruning does;
///   * machine-layer fragments (`Instruction::from_user_layer`) — merged in
///     from `$AGENTSTACK_HOME/agentstack.toml`, they are the USER's house
///     rules, never the project's content, and they are deliberately not
///     pinned into any project's consent digest (`docs/ENFORCEMENT.md`, Layer
///     scope). They are filtered out of the trust walk for that reason, so a
///     review of this repository has nothing to say about them and cannot be
///     allowed to withhold them. They ride along either way: the caller passes
///     the same merged table both layers live in, and forgetting the
///     `from_user_layer` filter here would take the user's own notes hostage
///     to a repo they happened to be standing in;
///   * the machine manifest itself — `$AGENTSTACK_HOME/agentstack.toml` is that
///     same personal layer. It is deliberately undiscoverable as a project
///     (`manifest::discover_project_base`), so `trust` can never reach it: its
///     base resolves to `$HOME`, which no code path can put in the trust store.
///     Gating it would make machine-level instructions permanently uncompilable
///     rather than merely pending a review no command can perform. One spelling
///     of the rule across the capability kinds — keep this in step with
///     `render::hooks::trust_refusal`, `render::apply::trust_refusal` and
///     `render::skills::trust_refusal`.
///
/// Package instruction members ARE gated, and that is a decision rather than an
/// oversight. A package's prose arrives WITH THE REPOSITORY — its pin lives in
/// the project's `agentstack.lock`, which is part of the consent surface — and
/// it compiles into the very same managed region, indistinguishable to the
/// model from a fragment the project wrote by hand (W5). "The project's
/// content" is about where the bytes came from and whose review covers them,
/// not about who typed them; a hostile repo that moved its instructions behind
/// a package would otherwise walk straight through this gate.
///
/// `prior` is the last exemption and the only conditional one: a command that
/// WROTE the lock or manifest bytes this project is now `Changed` by is judged
/// against the state that held before it wrote, because a command cannot be
/// allowed to refuse itself. `lock` (which pins package members and then
/// renders them) and `upgrade` (which rewrites the manifest and re-pins, then
/// re-renders the region) are the two callers that need it. See [`PriorTrust`]
/// for why that authorizes nothing new, and why nothing is re-pinned
/// afterwards.
fn trust_refusal(gated: &[String], project_dir: &Path, prior: PriorTrust) -> Option<String> {
    if gated.is_empty() {
        return None;
    }
    if crate::util::paths::is_machine_home(project_dir) {
        return None;
    }
    let base = crate::manifest::project_root_of(project_dir);
    let why = prior.refusal_reason(&base)?;
    let names = gated
        .iter()
        .map(|n| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(", ");
    // "render", not "compile": `verify::ensure_instructions_compilable` already
    // owns "refusing to compile instructions for <target>" for the LOCK gate,
    // and two gates that answer different questions must not open with the same
    // sentence. This wording also matches its siblings, "refusing to render
    // hooks" and "refusing to render MCP servers".
    Some(format!(
        "refusing to render instructions: project at {} {why} — review and \
         `agentstack trust .` before putting its words into the managed region \
         a harness reads straight into an agent's context ({names})",
        base.display()
    ))
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
