//! The delivery planner (W4, `docs/design/automatic-delivery.md` §"The
//! decision").
//!
//! For each capability, AgentStack chooses a **delivery lane** from two facts:
//! the capability's *kind*, and the *harness* it is going to. That is the whole
//! decision, and it is routing — **not** a mode switch. Static rendering is not
//! being removed by any of this; it stays the only correct answer for what MCP
//! cannot inject (instructions, settings) and for harnesses that cannot take a
//! live channel at all. A project can be, and normally will be, in both lanes
//! at once.
//!
//! ```text
//! kind                     lane        why
//! skills                   dynamic     served on demand, digest-verified per load
//! MCP servers              dynamic     brokered, policy-checked, recorded
//! instructions             rendered    MCP cannot inject these
//! settings                 rendered    native config file, nothing else reads it
//! hooks · extensions       rendered    executable kinds — full consent ceremony, always
//! any kind, non-MCP CLI    rendered    the CLI has no live channel
//! ```
//!
//! The one override is **Render locally** ([`agentstack_core::manifest::Delivery`]),
//! settable per project or per harness. It forces the rendered lane where the
//! live channel would have worked. There is no "prefer gateway" and no mode:
//! the automatic answer is the routed one, and the escape hatch writes files.
//!
//! # This module decides; it never writes
//!
//! Everything here is a pure function over a manifest's `[delivery]` table and
//! the adapter registry. Nothing in it touches disk, spawns anything, or
//! renders. Surfaces call it to *state* the routing (`init`, `status`,
//! `delivery`); the write paths that already own their writes are unchanged.

use agentstack_core::manifest::Delivery;

use crate::adapter::{AdapterDescriptor, Registry};

/// A capability kind, as the delivery matrix names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Skill,
    Server,
    Instruction,
    Setting,
    Hook,
    Extension,
}

impl Kind {
    /// Every kind, in matrix order — so a surface that enumerates them cannot
    /// silently drop one when a kind is added.
    pub const ALL: &'static [Kind] = &[
        Kind::Skill,
        Kind::Server,
        Kind::Instruction,
        Kind::Setting,
        Kind::Hook,
        Kind::Extension,
    ];

    /// The machine name (JSON, `--json` consumers).
    pub fn slug(self) -> &'static str {
        match self {
            Kind::Skill => "skills",
            Kind::Server => "servers",
            Kind::Instruction => "instructions",
            Kind::Setting => "settings",
            Kind::Hook => "hooks",
            Kind::Extension => "extensions",
        }
    }

    /// The plural noun a person reads.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Skill => "skills",
            Kind::Server => "MCP servers",
            Kind::Instruction => "house rules",
            Kind::Setting => "settings",
            Kind::Hook => "hooks",
            Kind::Extension => "extensions",
        }
    }

    /// Is this an **executable** kind — code that runs in or around the harness
    /// at the user's permission?
    ///
    /// Standing classification (CLAUDE.md): hooks and extensions are executable
    /// capability kinds, so the full consent ceremony always applies to them and
    /// no compressed-consent path may ever cover them. They are also, and for
    /// the same reason, permanently in the rendered lane: there is no version of
    /// "served live" for something whose whole purpose is to run.
    pub fn is_executable(self) -> bool {
        matches!(self, Kind::Hook | Kind::Extension)
    }
}

/// Which lane a capability travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// Served live over the project's MCP connection — never written to disk.
    Dynamic,
    /// Written into the harness's native files, exactly as it always was.
    Rendered,
}

impl Lane {
    pub fn slug(self) -> &'static str {
        match self {
            Lane::Dynamic => "dynamic",
            Lane::Rendered => "rendered",
        }
    }
}

/// Why a capability landed in its lane. Ordered most-specific first, because
/// that is the order the planner tests them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// **Render locally** is set for this harness (or for the whole project).
    RenderLocally,
    /// This CLI has no MCP channel, so there is nothing to serve over.
    NoLiveChannel,
    /// This kind is executable — hooks and extensions never leave the rendered
    /// lane, override or not.
    ExecutableKind,
    /// MCP cannot inject this kind; it only exists as a file the CLI reads.
    FileOnlyKind,
    /// The routed default: served live.
    Routed,
}

impl Reason {
    /// A clause a surface can drop straight into a sentence. Plain language on
    /// purpose — this copy reaches `init`, which is the first screen a person
    /// ever sees.
    pub fn why(self) -> &'static str {
        match self {
            Reason::RenderLocally => "render locally is set here",
            Reason::NoLiveChannel => "this tool reads files only",
            Reason::ExecutableKind => "it runs code, so it is always reviewed and written",
            Reason::FileOnlyKind => "only a file can carry it",
            Reason::Routed => "served live, on demand",
        }
    }
}

/// One routing decision: a kind, its lane, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub kind: Kind,
    pub lane: Lane,
    pub reason: Reason,
}

impl Route {
    /// Does this route demand the full consent ceremony regardless of lane?
    ///
    /// True for the executable kinds, always. Kept on the route rather than
    /// derived at each call site so a surface cannot render a hook's row while
    /// forgetting what a hook is.
    pub fn full_ceremony(self) -> bool {
        self.kind.is_executable()
    }
}

/// The single routing decision, for one kind on one harness.
///
/// The order below is the contract's, and it is deliberate: the three physical
/// facts are tested *before* the user's override, because the override can only
/// ever move a capability **towards** files. Nothing can move an instruction, a
/// hook, or a capability bound for a file-only CLI the other way — there is no
/// channel that would carry it — so an override tested first would merely be
/// redundant there, and would suggest a symmetry that does not exist.
///
/// 1. A harness with no MCP channel renders everything, automatically.
/// 2. Executable kinds (hooks, extensions) render, always.
/// 3. File-only kinds (instructions, settings) render — MCP cannot inject them.
/// 4. **Render locally** pulls what is left back to files.
/// 5. Everything remaining — skills and servers on an MCP-capable harness with
///    no override — is served live. **This is the default** (the flip,
///    2026-08-03).
pub fn route(kind: Kind, mcp_capable: bool, render_locally: bool) -> Route {
    let (lane, reason) = if !mcp_capable {
        (Lane::Rendered, Reason::NoLiveChannel)
    } else if kind.is_executable() {
        (Lane::Rendered, Reason::ExecutableKind)
    } else if matches!(kind, Kind::Instruction | Kind::Setting) {
        (Lane::Rendered, Reason::FileOnlyKind)
    } else if render_locally {
        (Lane::Rendered, Reason::RenderLocally)
    } else {
        (Lane::Dynamic, Reason::Routed)
    };
    Route { kind, lane, reason }
}

/// Where a harness's **Render locally** answer came from, so a surface can name
/// the scope the user actually set rather than the value it resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideSource {
    None,
    Project,
    Harness,
}

impl OverrideSource {
    pub fn slug(self) -> &'static str {
        match self {
            OverrideSource::None => "none",
            OverrideSource::Project => "project",
            OverrideSource::Harness => "harness",
        }
    }
}

/// The plan for one harness: what it is, whether it has a live channel, and
/// where each kind goes.
#[derive(Debug, Clone)]
pub struct HarnessPlan {
    pub id: String,
    pub display: String,
    pub mcp_capable: bool,
    pub render_locally: bool,
    pub override_source: OverrideSource,
    pub routes: Vec<Route>,
}

impl HarnessPlan {
    /// The kinds in one lane, in matrix order.
    pub fn kinds_in(&self, lane: Lane) -> Vec<Kind> {
        self.routes
            .iter()
            .filter(|r| r.lane == lane)
            .map(|r| r.kind)
            .collect()
    }

    /// One plain-language sentence naming both lanes for this harness.
    ///
    /// Both lanes are named on the same line **per harness**, never blended
    /// into one claim: the honesty rule the contract binds is that a surface
    /// reporting both lanes must carry a separate `rendered lane:` line, which
    /// [`rendered_lane_line`] provides for the project-wide report. Here the
    /// harness is the subject and each lane keeps its own clause and verb.
    pub fn sentence(&self) -> String {
        let names = |kinds: &[Kind]| -> String {
            kinds
                .iter()
                .map(|k| k.label())
                .collect::<Vec<_>>()
                .join(" + ")
        };
        let live = self.kinds_in(Lane::Dynamic);
        let files = self.kinds_in(Lane::Rendered);
        match (live.is_empty(), files.is_empty()) {
            (false, false) => format!(
                "{} served live · {} written to files",
                names(&live),
                names(&files)
            ),
            (false, true) => format!("{} served live", names(&live)),
            (true, false) if !self.mcp_capable => {
                format!(
                    "{} written to files — this tool reads files only",
                    names(&files)
                )
            }
            (true, false) if self.render_locally => {
                format!("{} written to files — render locally is set", names(&files))
            }
            (true, false) => format!("{} written to files", names(&files)),
            (true, true) => "nothing to deliver".to_string(),
        }
    }
}

/// The whole project's routing, one entry per harness in play.
#[derive(Debug, Clone)]
pub struct Plan {
    pub harnesses: Vec<HarnessPlan>,
}

impl Plan {
    /// Build the plan for `target_ids` under a project's `[delivery]` table.
    ///
    /// Takes the override table rather than the whole manifest so the planner
    /// cannot reach anything else: the routing depends on exactly two inputs,
    /// and a function that could see the servers list would eventually be
    /// tempted to consult it.
    ///
    /// A target id with no descriptor in the registry is skipped rather than
    /// guessed at: the planner may not invent a capability set for a CLI it
    /// cannot describe.
    pub fn build(delivery: &Delivery, registry: &Registry, target_ids: &[String]) -> Plan {
        let harnesses = target_ids
            .iter()
            .filter_map(|id| registry.get(id))
            .map(|desc| harness_plan(desc, delivery))
            .collect();
        Plan { harnesses }
    }

    /// Is any capability actually served live in this project?
    pub fn has_dynamic_lane(&self) -> bool {
        self.harnesses
            .iter()
            .any(|h| !h.kinds_in(Lane::Dynamic).is_empty())
    }

    /// Is anything actually written for this project?
    pub fn has_rendered_lane(&self) -> bool {
        self.harnesses
            .iter()
            .any(|h| !h.kinds_in(Lane::Rendered).is_empty())
    }

    /// The harnesses that would be served live — the honest denominator for any
    /// "served live" claim.
    pub fn live_harnesses(&self) -> Vec<&HarnessPlan> {
        self.harnesses
            .iter()
            .filter(|h| !h.kinds_in(Lane::Dynamic).is_empty())
            .collect()
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "default": "automatic",
            "harnesses": self.harnesses.iter().map(|h| serde_json::json!({
                "id": h.id,
                "display": h.display,
                "mcp_capable": h.mcp_capable,
                "render_locally": h.render_locally,
                "override": h.override_source.slug(),
                "summary": h.sentence(),
                "routes": h.routes.iter().map(|r| serde_json::json!({
                    "kind": r.kind.slug(),
                    "lane": r.lane.slug(),
                    "why": r.reason.why(),
                    "full_ceremony": r.full_ceremony(),
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }
}

/// Route every kind this descriptor can actually carry.
///
/// A kind the harness has no destination for is left out entirely rather than
/// routed to a lane it cannot reach — a CLI with no hooks spec must not appear
/// to be receiving hooks in either lane.
fn harness_plan(desc: &AdapterDescriptor, delivery: &Delivery) -> HarnessPlan {
    let mcp_capable = desc.mcp.is_some();
    let render_locally = delivery.renders_locally(&desc.id);
    let override_source = if delivery
        .harness
        .get(&desc.id)
        .is_some_and(|h| h.render_locally.is_some())
    {
        OverrideSource::Harness
    } else if delivery.render_locally.is_some() {
        OverrideSource::Project
    } else {
        OverrideSource::None
    };

    let routes = Kind::ALL
        .iter()
        .filter(|kind| carries(desc, **kind))
        .map(|kind| route(*kind, mcp_capable, render_locally))
        .collect();

    HarnessPlan {
        id: desc.id.clone(),
        display: desc.display.clone(),
        mcp_capable,
        render_locally,
        override_source,
        routes,
    }
}

/// Can this harness carry this kind at all?
///
/// Skills are the one kind that does not need a native destination: a
/// MCP-capable CLI can be served skills over the live channel even when it has
/// no skills directory of its own — which is exactly what the dynamic lane is
/// for. Every other kind needs somewhere for the bytes to land.
fn carries(desc: &AdapterDescriptor, kind: Kind) -> bool {
    match kind {
        Kind::Skill => desc.skills.is_some() || desc.mcp.is_some(),
        Kind::Server => desc.mcp.is_some(),
        Kind::Instruction => desc.instructions.is_some(),
        Kind::Setting => desc.settings.is_some(),
        Kind::Hook => desc.hooks.is_some(),
        Kind::Extension => desc.extensions.is_some(),
    }
}

/// The **only** sanctioned way to say how little a gateway-served project keeps
/// on disk (contract §"Honesty rules", binding on every surface).
///
/// Never "0 files". The project still holds a manifest and a lock — and, when
/// instructions are used, a managed region in an instruction file. Those are
/// project artifacts; what the dynamic lane removes is the *generated* ones.
pub const ZERO_ARTIFACTS: &str = "0 project artifacts for the capabilities served live \
                                  (the manifest and lock stay, and so does any managed \
                                  region in a house-rules file)";

/// The `rendered lane:` line the honesty rules require on any surface that also
/// reports the dynamic lane — naming what is actually written, and where.
///
/// Returns `None` when nothing renders, because an empty lane line is its own
/// small lie. Callers print this on **its own line**; a blended sentence is how
/// a user comes to believe no file was touched when one was.
pub fn rendered_lane_line(plan: &Plan) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for h in &plan.harnesses {
        let kinds = h.kinds_in(Lane::Rendered);
        if kinds.is_empty() {
            continue;
        }
        parts.push(format!(
            "{} — {}",
            h.display,
            kinds
                .iter()
                .map(|k| k.label())
                .collect::<Vec<_>>()
                .join(" + ")
        ));
    }
    (!parts.is_empty()).then(|| format!("rendered lane: {}", parts.join(" · ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The matrix, kind by kind, in both harness shapes. This is the table in
    /// the design doc; if it changes, this test is where the change is argued.
    #[test]
    fn every_kind_routes_by_the_matrix() {
        // MCP-capable, no override — the default after the flip.
        assert_eq!(route(Kind::Skill, true, false).lane, Lane::Dynamic);
        assert_eq!(route(Kind::Server, true, false).lane, Lane::Dynamic);
        assert_eq!(route(Kind::Instruction, true, false).lane, Lane::Rendered);
        assert_eq!(route(Kind::Setting, true, false).lane, Lane::Rendered);
        assert_eq!(route(Kind::Hook, true, false).lane, Lane::Rendered);
        assert_eq!(route(Kind::Extension, true, false).lane, Lane::Rendered);

        // Non-MCP harness: every kind renders, automatically.
        for kind in Kind::ALL {
            let r = route(*kind, false, false);
            assert_eq!(r.lane, Lane::Rendered, "{kind:?} on a non-MCP harness");
            assert_eq!(r.reason, Reason::NoLiveChannel);
        }

        // Render locally: the lease-capable kinds come back to files, and the
        // reason names the override rather than the kind.
        assert_eq!(
            route(Kind::Skill, true, true),
            Route {
                kind: Kind::Skill,
                lane: Lane::Rendered,
                reason: Reason::RenderLocally
            }
        );
        assert_eq!(route(Kind::Server, true, true).lane, Lane::Rendered);
    }

    /// Hooks and extensions carry the ceremony in every combination — there is
    /// no route through this function that produces an executable kind without
    /// it.
    #[test]
    fn executable_kinds_always_carry_the_full_ceremony() {
        for mcp in [true, false] {
            for local in [true, false] {
                for kind in [Kind::Hook, Kind::Extension] {
                    let r = route(kind, mcp, local);
                    assert_eq!(r.lane, Lane::Rendered);
                    assert!(r.full_ceremony(), "{kind:?} mcp={mcp} local={local}");
                }
            }
        }
        for kind in [Kind::Skill, Kind::Server, Kind::Instruction, Kind::Setting] {
            assert!(!route(kind, true, false).full_ceremony());
        }
    }

    /// The override resolves most-specific-first, and can point either way.
    #[test]
    fn the_override_resolves_harness_before_project() {
        let d: Delivery = toml::from_str(
            r#"
render_locally = true
[harness.codex]
render_locally = false
"#,
        )
        .expect("parse");
        assert!(d.renders_locally("claude-code"), "project-wide applies");
        assert!(!d.renders_locally("codex"), "the harness entry wins");
        assert!(d.overrides_anything());

        let none = Delivery::default();
        assert!(!none.renders_locally("claude-code"));
        assert!(!none.overrides_anything());
    }

    /// The zero-artifacts sentence never degrades into "0 files".
    #[test]
    fn the_zero_artifacts_sentence_names_what_stays() {
        assert!(ZERO_ARTIFACTS.contains("0 project artifacts"));
        assert!(ZERO_ARTIFACTS.contains("manifest"));
        assert!(ZERO_ARTIFACTS.contains("lock"));
        assert!(!ZERO_ARTIFACTS.contains("0 files"));
    }
}
