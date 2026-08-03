//! Instruction variants and the per-harness honesty matrix
//! (`docs/design/instruction-variants.md`).
//!
//! Three questions live here, and nowhere else:
//!
//! 1. **Which bytes?** A fragment can carry per-`(cli, model)` variants; the
//!    most specific selector that matches wins, and the base body is the
//!    least specific match. The precedence function itself is
//!    [`agentstack_core::manifest::select_variant`] — pure, in core, shared by
//!    the manifest's variants and a library body's variants, which are the
//!    same grammar in two homes.
//! 2. **Which model?** Only from a declaration AgentStack can point at: an
//!    explicitly selected toolset's `model`, or the `[settings.<cli>] model`
//!    value AgentStack itself compiles into that CLI's config. Anything else
//!    is [`ModelSource::Unknown`], which is a first-class answer and never a
//!    guess.
//! 3. **Which channel, and how well is it known?** Per harness, read from the
//!    adapter descriptor: a rendered file, plus an optional live channel
//!    labelled `confirmed` or `unconfirmed`. A harness with no instruction
//!    destination at all is reported as such rather than omitted, because an
//!    adapter that silently disappears from a coverage list reads as covered.
//!
//! # This module resolves and describes; it never writes
//!
//! Everything here reads a manifest, a registry and (for a library-sourced
//! fragment) one `instruction.toml`. Nothing writes, renders, or spawns.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use agentstack_core::manifest::{select_variant, Instruction, InstructionVariant};
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::adapter::{AdapterDescriptor, Confirmation, Registry};
use crate::library::{Library, INSTRUCTION_FILE};
use crate::manifest::Manifest;
use crate::scope::Scope;

// ---------------------------------------------------------------- the bodies

/// Where one fragment's bodies came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// The manifest entry declares its own `path`.
    Inline,
    /// The manifest entry is sourceless and the bodies resolve from a linked
    /// library source, by the entry's key (`ExtensionOrigin::Library`'s rule,
    /// applied to house rules).
    Library { source: String },
}

/// The base body plus every variant of one fragment, with the directory their
/// declared paths are anchored against.
///
/// One struct for both origins on purpose: every consumer downstream —
/// compiling, pinning, drift-checking — wants the same four facts, and giving
/// a library fragment its own shape is how the two would drift apart.
#[derive(Debug, Clone)]
pub struct Bodies {
    pub anchor: PathBuf,
    pub base: String,
    pub variants: Vec<InstructionVariant>,
    pub origin: Origin,
}

/// The `instruction.toml` at the root of a library instruction body — the same
/// `path` + `[[variant]]` grammar the manifest uses.
///
/// `deny_unknown_fields`: this is hostile input (somebody's folder), and a
/// typo'd key that parsed silently would deliver bytes nobody selected.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LibraryInstructionToml {
    path: String,
    #[serde(default, rename = "variant")]
    variants: Vec<InstructionVariant>,
}

/// Largest `instruction.toml` we will read. A declaration file is a handful of
/// lines; bounding the read is invariant 7 applied to a folder we do not own.
const MAX_INSTRUCTION_TOML: u64 = 64 * 1024;

/// Collect one fragment's bodies.
///
/// An inline fragment keeps today's anchoring exactly (absolute passes
/// through, relative joins the manifest dir). A sourceless one resolves
/// through the ordered linked sources by `name` and reads its
/// `instruction.toml`, with every declared path containment-checked inside the
/// body directory before any read.
pub fn bodies(
    name: &str,
    instr: &Instruction,
    manifest_dir: &Path,
    library: &Library,
) -> Result<Bodies> {
    if let Some(path) = instr.path.as_deref() {
        return Ok(Bodies {
            anchor: manifest_dir.to_path_buf(),
            base: path.to_string(),
            variants: instr.variants.clone(),
            origin: Origin::Inline,
        });
    }

    // Sourceless: the linked sources supply the bodies. Resolution walks the
    // ordered list and takes the first source holding the name — `PATH`
    // semantics, the same first-match-wins rule every other library kind uses.
    let entry = library.get_instruction(name).ok_or_else(|| {
        anyhow!(
            "house rule '{name}' declares no `path` and no linked library source holds it — \
             add a path, or create <source>/instructions/{name}/{INSTRUCTION_FILE}"
        )
    })?;
    let (source, root) = match library.linked.find(crate::library::Kind::Instruction, name) {
        Some((index, _)) => (index.name.clone(), index.root.clone()),
        // A single-file index (no linked view): the central library is the
        // only source, and it is the one every management command passes.
        None => ("local".to_string(), crate::util::paths::lib_home()),
    };
    let body_dir = entry.body_dir(&root);
    let decl_path = body_dir.join(INSTRUCTION_FILE);
    let text = crate::util::read_to_string_bounded(&decl_path, MAX_INSTRUCTION_TOML)
        .with_context(|| format!("reading {}", decl_path.display()))?;
    let decl: LibraryInstructionToml =
        toml::from_str(&text).with_context(|| format!("parsing {}", decl_path.display()))?;

    // Containment before any body is read: `..`, an absolute path, and a
    // symlink anywhere on the way out are refused, so a linked folder can
    // never aim a fragment at a file outside its own body.
    let checked = |declared: &str| -> Result<()> {
        agentstack_core::digest::resolve_contained(&body_dir, declared)
            .map(|_| ())
            .with_context(|| {
                format!("house rule '{name}': body '{declared}' must stay inside its folder")
            })
    };
    checked(&decl.path)?;
    for v in &decl.variants {
        checked(&v.path)?;
    }

    Ok(Bodies {
        anchor: body_dir,
        base: decl.path,
        variants: decl.variants,
        origin: Origin::Library { source },
    })
}

impl Bodies {
    /// Anchor a declared body path the way the compiler does.
    pub fn source_of(&self, declared: &str) -> PathBuf {
        crate::render::instructions::fragment_source(&self.anchor, declared)
    }

    /// The winning body for `(cli, model)`, plus how it was chosen.
    pub fn choose(&self, cli: &str, model: Option<&str>) -> Chosen {
        match select_variant(&self.variants, cli, model) {
            Some(i) => {
                let v = &self.variants[i];
                Chosen {
                    declared: v.path.clone(),
                    path: self.source_of(&v.path),
                    variant: Some(v.clone()),
                }
            }
            None => Chosen {
                declared: self.base.clone(),
                path: self.source_of(&self.base),
                variant: None,
            },
        }
    }

    /// Every body this fragment declares — the base first, then each variant in
    /// declaration order.
    ///
    /// This is the pinning and drift-checking set, and it is deliberately
    /// *every* body rather than the selected one: consent is over content, so a
    /// variant nothing currently selects is still pinned and still fails closed
    /// when its bytes move.
    pub fn all(&self) -> Vec<(Option<&InstructionVariant>, &str, PathBuf)> {
        let mut out = vec![(None, self.base.as_str(), self.source_of(&self.base))];
        for v in &self.variants {
            out.push((Some(v), v.path.as_str(), self.source_of(&v.path)));
        }
        out
    }
}

/// The body one `(cli, model)` pair selects.
#[derive(Debug, Clone)]
pub struct Chosen {
    pub path: PathBuf,
    pub declared: String,
    /// `None` = the base body, the least specific match.
    pub variant: Option<InstructionVariant>,
}

impl Chosen {
    /// A short label for the selected body: `claude-code+opus`, `codex`,
    /// `opus`, or `base`.
    pub fn label(&self) -> String {
        match &self.variant {
            None => "base".to_string(),
            Some(v) => match (v.cli.as_deref(), v.model.as_deref()) {
                (Some(c), Some(m)) => format!("{c}+{m}"),
                (Some(c), None) => c.to_string(),
                (None, Some(m)) => m.to_string(),
                (None, None) => "base".to_string(),
            },
        }
    }
}

/// The base body's file for a fragment, resolved through the library when the
/// entry is sourceless. `None` when it cannot be resolved at all.
///
/// For the surfaces that need *one* path for a fragment — the grant's bound
/// instruction, the re-gate diff, `explain`'s Source line. Every variant is
/// pinned in the lock and checked by [`crate::resolve::instruction_lock_status_with`];
/// this is the fragment's identity file, not the whole content surface.
pub fn base_source(
    name: &str,
    instr: &Instruction,
    manifest_dir: &Path,
    library: &Library,
) -> Option<PathBuf> {
    bodies(name, instr, manifest_dir, library)
        .ok()
        .map(|b| b.source_of(&b.base))
}

/// How a fragment declares its bodies, for display: the declared path, or the
/// library reference a sourceless entry resolves by.
pub fn declared_label(name: &str, instr: &Instruction) -> String {
    match instr.path.as_deref() {
        Some(p) => format!("path {p}"),
        None => format!("library house rule '{name}'"),
    }
}

/// What a command needs in hand before it can pick variants: the linked
/// library view (for sourceless fragments) and the toolset it was explicitly
/// asked for.
///
/// Built once per command and passed down, rather than loaded inside the
/// per-target compile: the library is one read, and doing it per harness per
/// scope would multiply it by thirteen for nothing.
#[derive(Debug)]
pub struct Selecting {
    pub library: Library,
    pub toolset: Option<String>,
}

impl Selecting {
    /// Best-effort: an unreadable library degrades to inline-only resolution
    /// with a warning, exactly as every other rendering surface does, rather
    /// than failing a compile that may not need the library at all.
    pub fn for_command(toolset: Option<&str>) -> Self {
        Selecting {
            library: Library::load_default_or_warn(),
            toolset: toolset.map(str::to_string),
        }
    }

    /// No library and no selection — for callers that plan a region *away*
    /// (`unrender`) or otherwise resolve nothing.
    pub fn none() -> Self {
        Selecting {
            library: Library::default(),
            toolset: None,
        }
    }

    pub fn toolset(&self) -> Option<&str> {
        self.toolset.as_deref()
    }
}

// ---------------------------------------------------------------- the model

/// Where a model came from — always reported beside the variant it selected,
/// so a wrong variant is diagnosable from the line that chose it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSource {
    /// `[toolsets.<name>] model` on the toolset a command explicitly named.
    Toolset(String),
    /// `[settings.<cli>] model` — the value AgentStack itself compiles into
    /// that CLI's native config. If we wrote it, we know it.
    Settings,
    /// Nothing declares it. The least specific matching variant is used, and
    /// every surface says so.
    Unknown,
}

/// A model and its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub model: Option<String>,
    pub source: ModelSource,
}

impl ModelChoice {
    pub fn as_deref(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// The clause every surface appends to a selected-variant line.
    pub fn clause(&self) -> String {
        match (&self.model, &self.source) {
            (Some(m), ModelSource::Toolset(t)) => format!("model {m}, from toolset {t}"),
            (Some(m), ModelSource::Settings) => format!("model {m}, from settings"),
            _ => "model unknown — least specific match used".to_string(),
        }
    }
}

/// Determine the model for `cli`, given the toolset a command explicitly named.
///
/// Order — most deliberate first:
///
/// 1. the **explicitly selected** toolset's `model` (a default toolset nobody
///    named contributes nothing: a default is not a selection);
/// 2. `[settings.<cli>] model`, the value AgentStack renders into that CLI;
/// 3. unknown.
///
/// A toolset's `model` is an *intent* and the settings value is a *fact we
/// write*; when both are declared and disagree the toolset wins because it is
/// the narrower act, and the reported source is how a user sees that it did.
/// AgentStack does not reconcile them — that would mean a toolset selection
/// silently rewriting a harness's native settings file.
pub fn model_for(manifest: &Manifest, cli: &str, selected_toolset: Option<&str>) -> ModelChoice {
    if let Some(name) = selected_toolset {
        if let Some(model) = manifest.profiles.get(name).and_then(|p| p.model.as_deref()) {
            return ModelChoice {
                model: Some(model.to_string()),
                source: ModelSource::Toolset(name.to_string()),
            };
        }
    }
    if let Some(model) = manifest
        .settings
        .get(cli)
        .and_then(|v| v.get("model"))
        .and_then(|v| v.as_str())
    {
        return ModelChoice {
            model: Some(model.to_string()),
            source: ModelSource::Settings,
        };
    }
    ModelChoice {
        model: None,
        source: ModelSource::Unknown,
    }
}

// ------------------------------------------------------------- the channels

/// What actually carries house rules to one harness, and how well that is
/// known. The three shapes are the honesty matrix.
#[derive(Debug, Clone)]
pub struct HarnessChannel {
    pub id: String,
    pub display: String,
    /// The instruction file for the scope in play, when the harness has one.
    /// `None` is the "no instruction channel" state.
    pub file: Option<PathBuf>,
    /// The live channel the descriptor declares, if any.
    pub live: Option<LiveChannel>,
    /// The variant this harness would receive, when the project declares any
    /// house rules at all.
    pub selection: Option<Selection>,
}

/// A declared live channel, flattened out of the descriptor.
#[derive(Debug, Clone)]
pub struct LiveChannel {
    pub id: String,
    pub display: String,
    pub confirmation: Confirmation,
}

/// Which body one fragment sends to one harness, and why.
#[derive(Debug, Clone)]
pub struct Selection {
    pub fragment: String,
    pub variant: String,
    pub model: ModelChoice,
}

/// The one sentence that explains why a *confirmed* live channel still is not
/// carrying house rules. Stated once so no surface can improvise a different
/// reason (`docs/design/instruction-variants.md` §"Do instructions route
/// dynamically for Claude Code? No — and why").
pub const CONFIRMED_BUT_UNUSED: &str = "confirmed for this tool, not used for house rules — \
                                        no live channel varies by model";

/// The one sentence for an **unconfirmed** channel. It is never used as though
/// it worked, and the wording may not soften into "supported".
pub const UNCONFIRMED_NEVER_USED: &str = "unconfirmed — never used as though it worked";

/// The one sentence for a harness with no instruction destination at all.
pub const NO_CHANNEL: &str = "no instruction channel; house rules do not reach this tool";

impl HarnessChannel {
    /// The plain-language row a person reads. Three shapes, one per state.
    pub fn sentence(&self) -> String {
        let Some(file) = &self.file else {
            return format!("{} — {NO_CHANNEL}", self.display);
        };
        let mut out = format!("{} — {}", self.display, file.display());
        if let Some(sel) = &self.selection {
            let _ = write!(
                out,
                "; {} → {} ({})",
                sel.fragment,
                sel.variant,
                sel.model.clause()
            );
        }
        if let Some(live) = &self.live {
            let tail = match live.confirmation {
                Confirmation::Confirmed => CONFIRMED_BUT_UNUSED,
                Confirmation::Unconfirmed => UNCONFIRMED_NEVER_USED,
            };
            let _ = write!(out, "; live channel {}: {tail}", live.display);
        }
        out
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "display": self.display,
            "file": self.file.as_ref().map(|p| p.display().to_string()),
            "live_channel": self.live.as_ref().map(|l| serde_json::json!({
                "id": l.id,
                "display": l.display,
                "confirmation": l.confirmation.slug(),
                "used": false,
            })),
            "selection": self.selection.as_ref().map(|s| serde_json::json!({
                "fragment": s.fragment,
                "variant": s.variant,
                "model": s.model.model,
                "model_source": match &s.model.source {
                    ModelSource::Toolset(t) => format!("toolset:{t}"),
                    ModelSource::Settings => "settings".to_string(),
                    ModelSource::Unknown => "unknown".to_string(),
                },
            })),
            "sentence": self.sentence(),
        })
    }
}

/// Build the honesty matrix for `target_ids`, in the order given.
///
/// `selected_toolset` is the toolset a command explicitly named, or `None`.
/// A target with no descriptor is skipped rather than guessed at — the same
/// rule the delivery planner follows.
pub fn channels(
    manifest: &Manifest,
    registry: &Registry,
    target_ids: &[String],
    scope: Scope,
    project_dir: &Path,
    library: &Library,
    selected_toolset: Option<&str>,
) -> Vec<HarnessChannel> {
    target_ids
        .iter()
        .filter_map(|id| registry.get(id))
        .map(|desc| {
            harness_channel(
                manifest,
                desc,
                scope,
                project_dir,
                library,
                selected_toolset,
            )
        })
        .collect()
}

fn harness_channel(
    manifest: &Manifest,
    desc: &AdapterDescriptor,
    scope: Scope,
    project_dir: &Path,
    library: &Library,
    selected_toolset: Option<&str>,
) -> HarnessChannel {
    let spec = desc.instructions.as_ref();
    let file = spec.and_then(|s| s.path_for(scope, project_dir));
    let live = spec.and_then(|s| s.live.as_ref()).map(|l| LiveChannel {
        id: l.channel.clone(),
        display: l.display.clone(),
        confirmation: l.confirmation,
    });

    // The variant this harness receives, reported for the first fragment that
    // actually compiles here. One row is the orientation screen's budget; the
    // rest are `agentstack instructions`'s job.
    let model = model_for(manifest, &desc.id, selected_toolset);
    let selection = file.as_ref().and_then(|_| {
        manifest
            .instructions
            .iter()
            .find(|(_, i)| i.compiles_at(&desc.id, scope))
            .map(|(name, instr)| {
                let variant = bodies(name, instr, project_dir, library)
                    .map(|b| b.choose(&desc.id, model.as_deref()).label())
                    // An unresolvable fragment is the compiler's error to
                    // report; here it simply has no variant to name.
                    .unwrap_or_else(|_| "unresolved".to_string());
                Selection {
                    fragment: name.clone(),
                    variant,
                    model: model.clone(),
                }
            })
    });

    HarnessChannel {
        id: desc.id.clone(),
        display: desc.display.clone(),
        file,
        live,
        selection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(text: &str) -> Manifest {
        toml::from_str(text).expect("parse manifest")
    }

    /// The two declarations that name a model, and the honest silence when
    /// neither does.
    #[test]
    fn the_model_comes_from_a_selection_or_a_setting_and_is_otherwise_unknown() {
        let m = manifest(
            r#"
            version = 1
            [profiles.backend]
            model = "opus"
            [settings.claude-code]
            model = "sonnet"
            "#,
        );

        // An explicitly named toolset is the narrower act and wins.
        let chosen = model_for(&m, "claude-code", Some("backend"));
        assert_eq!(chosen.model.as_deref(), Some("opus"));
        assert_eq!(chosen.source, ModelSource::Toolset("backend".into()));

        // No toolset named: the value we compile into the CLI's own config.
        let chosen = model_for(&m, "claude-code", None);
        assert_eq!(chosen.model.as_deref(), Some("sonnet"));
        assert_eq!(chosen.source, ModelSource::Settings);

        // A harness nothing declares a model for is unknown, not defaulted.
        let chosen = model_for(&m, "codex", None);
        assert_eq!(chosen.model, None);
        assert_eq!(chosen.source, ModelSource::Unknown);
        assert!(chosen.clause().contains("unknown"));
    }

    /// A default toolset nobody named contributes no model — a default is not
    /// a selection.
    #[test]
    fn an_unnamed_toolset_contributes_no_model() {
        let m = manifest("version = 1\n[profiles.backend]\nmodel = \"opus\"\n");
        assert_eq!(
            model_for(&m, "claude-code", None).source,
            ModelSource::Unknown
        );
    }
}
