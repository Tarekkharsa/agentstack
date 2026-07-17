//! `agentstack run <harness> --locked` — the Protected host activation tier
//! (locked-run contract §3). A fail-closed promotion of the plain host run:
//! recorder-open (`AttemptStarted`) → trust (enforced) → strict lock verify →
//! policy admission → freeze `AuthorityGrant` (`GrantFrozen`) → launch →
//! recorded outcome. No Docker. Fails closed at the first blocking gate,
//! before the harness binary is spawned; `--plan` instead aggregates every
//! blocker without mutating anything (§2.2).
//!
//! Honest limits (§3.1) are printed, not implied away: this is pre-launch
//! content trust and policy admissibility, not kernel isolation, and the
//! evidence is a cooperative local audit trail, not tamper-proof attestation.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as AnyhowContext, Result};
use owo_colors::OwoColorize;

use crate::calllog::{RunEvent, RunLog};
use crate::cli::RunArgs;
use crate::executable::ExecutableLockStatus;
use crate::grant::{ArtifactMode, EgressMode, RuntimeImage};
use crate::grant::{
    AuthorityGrant, ConsentDigest, ContentDigest, GrantBuilder, GrantDigest, GrantPath,
    GrantPosture, GrantedAdapter, GrantedInstruction, GrantedServer, GrantedSkill,
    HarnessExecutable, InputOrigin, Invocation, PolicyGrant, PolicyProvenance, PolicySource,
    ProfileEffect, SecretGrant, SecretLifetimeBinding, SecretScope, SkillSource,
};
use crate::resolve::{FrozenServer, InstructionLockStatus, SkillLockStatus};
use crate::trust::{self, TrustState};

use super::Context;

pub fn run_locked(manifest_dir: Option<&Path>, args: &RunArgs) -> Result<()> {
    // Named limitations, checked before anything resolves. Refusing loudly is
    // honest; silently degrading the contract's semantics is not.
    if args.profile.is_some() {
        anyhow::bail!(
            "--locked --profile is not available yet: under --locked, profile application \
             must render from the frozen AuthorityGrant (contract §2.1/§7), which lands \
             with the D2 render/session unification. Run without --profile or without --locked."
        );
    }
    if args.sandbox || args.lockdown {
        anyhow::bail!(
            "--locked --sandbox/--lockdown is not wired yet: the strict gate must run \
             before the container starts (contract §2.1). Use --locked alone (host) or \
             --sandbox/--lockdown alone for now."
        );
    }

    let ctx = super::load(manifest_dir)?;
    let base = crate::manifest::project_root_of(&ctx.dir);
    if args.plan {
        plan(&ctx, &base, args)
    } else {
        live(&ctx, &base, args)
    }
}

fn ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// The per-run evidence channel. Material events (contract §9) go through the
/// CHECKED append; a failure to record is itself a reason not to launch.
struct Evidence {
    log: RunLog,
    run_id: String,
    started: Instant,
}

impl Evidence {
    /// Record a material event; an append failure becomes an error the run
    /// must treat as blocking.
    fn material(&self, ev: &RunEvent) -> Result<()> {
        self.log.append_checked(ev).with_context(|| {
            format!(
                "refusing to proceed: material evidence for run {} could not be recorded",
                self.run_id
            )
        })
    }

    /// Record a gate refusal + terminal outcome, then return the refusal as an
    /// error. If recording itself fails, the contract requires surfacing BOTH
    /// — the refusal and the evidence failure — and still never launching
    /// (§3 step 2). The gate refusal is always the primary error.
    fn refuse(&self, gate: &str, why: anyhow::Error) -> anyhow::Error {
        let record = self
            .material(&RunEvent::GateDecision {
                ts: ts(),
                gate: gate.to_string(),
                passed: false,
                detail: Some(format!("{why:#}")),
            })
            .and_then(|()| {
                self.material(&RunEvent::LockedOutcome {
                    ts: ts(),
                    outcome: "refused".to_string(),
                    exit_code: None,
                    duration_ms: self.started.elapsed().as_millis() as u64,
                    grant_digest: None,
                    usage: "unavailable".to_string(),
                })
            });
        match record {
            Ok(()) => why,
            Err(rec) => why.context(format!(
                "ALSO: this refusal's evidence could not be fully recorded ({rec:#}) — \
                 the run does not launch either way"
            )),
        }
    }

    fn passed(&self, gate: &str, detail: Option<String>) -> Result<()> {
        self.material(&RunEvent::GateDecision {
            ts: ts(),
            gate: gate.to_string(),
            passed: true,
            detail,
        })
    }
}

/// Everything strict verification looks at, resolved ONCE per attempt and
/// reused by the verify gate, the admission gate, and grant assembly — never a
/// second resolution between check and use.
struct LockedInputs {
    lock: agentstack_core::lock::Lock,
    library: crate::library::Library,
    lib_home: PathBuf,
    skill_statuses: Vec<(String, SkillLockStatus)>,
    instruction_statuses: Vec<(String, InstructionLockStatus)>,
    extension_statuses: Vec<(String, crate::resolve::ExtensionLockStatus)>,
    frozen: Vec<FrozenServer>,
    executable_statuses: Vec<(String, ExecutableLockStatus)>,
    /// The derived executable pins, keyed by owning server — the EXACT content
    /// identities the verify gate judged; grant assembly freezes these, never
    /// a second derivation.
    executable_pins: Vec<(String, agentstack_core::lock::LockedExecutable)>,
}

fn resolve_inputs(ctx: &Context) -> Result<LockedInputs> {
    let m = &ctx.loaded.manifest;
    // A broken lockfile is a refusal, not a default: its pins are exactly what
    // strict verification is about to assert.
    let lock = crate::lock::Lock::load(&ctx.dir)?;
    let library = crate::library::Library::load_default_or_warn();
    let lib_home = crate::util::paths::lib_home();
    let store = crate::store::Store::default_store();

    let skill_statuses: Vec<(String, SkillLockStatus)> = super::trust::review_skill_names(m)
        .into_iter()
        .map(|name| {
            let report = crate::resolve::skill_lock_status(
                &name,
                m,
                &ctx.dir,
                &library,
                &lib_home,
                &store,
                &lock,
                crate::resolve::ResolveMode::NoFetch,
            );
            (name, report.status)
        })
        .collect();

    let instruction_statuses: Vec<(String, InstructionLockStatus)> = m
        .instructions
        .iter()
        .filter(|(_, i)| !i.from_user_layer)
        .map(|(name, instr)| {
            (
                name.clone(),
                crate::resolve::instruction_lock_status(name, instr, &ctx.dir, &lock),
            )
        })
        .collect();

    // Native extensions (D6): their pins are part of the locked surface — a
    // drifted, unpinned, or offline extension must refuse the run like every
    // other kind. Resolution is library-aware and NoFetch (offline), matching
    // the skill statuses above.
    let extension_statuses: Vec<(String, crate::resolve::ExtensionLockStatus)> = m
        .extensions
        .iter()
        .map(|(name, ext)| {
            (
                name.clone(),
                crate::resolve::extension_lock_status(
                    name,
                    ext,
                    &ctx.dir,
                    &library,
                    &lib_home,
                    &store,
                    &lock,
                    crate::resolve::ResolveMode::NoFetch,
                )
                .status,
            )
        })
        .collect();

    let frozen = crate::resolve::frozen_runtime_servers(m, &library, &lib_home, &ctx.dir, None)?;

    let exec_servers: Vec<(String, crate::manifest::Server)> = frozen
        .iter()
        .filter_map(|(n, r)| r.as_ref().ok().map(|r| (n.clone(), r.server.clone())))
        .collect();
    let (executable_statuses, executable_pins) =
        crate::executable::executable_lock_statuses_and_pins(&ctx.dir, &exec_servers, &lock);

    Ok(LockedInputs {
        lock,
        library,
        lib_home,
        skill_statuses,
        instruction_statuses,
        extension_statuses,
        frozen,
        executable_statuses,
        executable_pins,
    })
}

/// Contract §3 step 5: evaluate the statically-declared admission surface
/// against the compiled ruleset. Refusals name the offender and the rule;
/// an unclassifiable declared host (e.g. a `${REF}` in the host portion)
/// blocks, because it cannot be checked against the machine egress ceiling.
fn admission_refusals(
    ruleset: &agentstack_policy::CompiledRuleset,
    frozen: &[FrozenServer],
) -> Vec<(String, String)> {
    use agentstack_core::manifest::{host_from_url, normalize_host, ServerType};
    let mut refusals = Vec::new();
    for (name, resolved) in frozen {
        let Ok(r) = resolved else {
            continue; // strict verification already blocks unresolved servers
        };
        if r.server.server_type == ServerType::Http {
            let url = r.server.url.as_deref().unwrap_or("");
            match host_from_url(url) {
                None => refusals.push((
                    format!("server '{name}'"),
                    format!(
                        "declared URL {url:?} has no classifiable host (empty, malformed, or an \
                         unresolved ${{REF}} in the host portion) — it cannot be checked against \
                         the machine egress ceiling"
                    ),
                )),
                Some(host) => {
                    let host = normalize_host(&host);
                    if let Err(rule) = ruleset.egress_decision(name, &host, None) {
                        refusals.push((
                            format!("server '{name}'"),
                            format!("declared host {host:?} is denied by policy: {rule}"),
                        ));
                    }
                }
            }
        }
        for reference in r.server.referenced_secrets() {
            if let Err(rule) = ruleset.secret_decision(name, &reference) {
                refusals.push((
                    format!("server '{name}'"),
                    format!(
                        "declared secret reference ${{{reference}}} is denied by policy: {rule}"
                    ),
                ));
            }
        }
    }
    refusals
}

fn admission_error(refusals: &[(String, String)]) -> anyhow::Error {
    let lines: Vec<String> = refusals
        .iter()
        .map(|(who, why)| format!("  {who}  {why}"))
        .collect();
    anyhow::anyhow!(
        "refusing to launch: {} declared request(s) fall outside the machine policy ceiling —\n{}",
        refusals.len(),
        lines.join("\n")
    )
}

/// Assemble and freeze the `AuthorityGrant` from the SAME verified inputs the
/// gates accepted (contract §6): frozen servers, derived executable pins,
/// resolved skills/instructions, the compiled ruleset, and the exact
/// invocation. Never re-resolves; anything that fails here refuses the run.
#[allow(clippy::too_many_arguments)]
fn freeze_grant(
    ctx: &Context,
    base: &Path,
    args: &RunArgs,
    inputs: &LockedInputs,
    ruleset: agentstack_policy::CompiledRuleset,
    machine_policy: &crate::manifest::Policy,
    bin_path: &Path,
) -> Result<AuthorityGrant> {
    let m = &ctx.loaded.manifest;
    let consent = trust::digest_for(base)
        .context("project is trusted but its consent digest could not be recomputed")?;
    let project =
        crate::grant::ProjectIdentity::new(GrantPath::new(base)?, ConsentDigest::parse(&consent)?);

    let adapter = GrantedAdapter::from_registry(&ctx.registry, &args.harness)?;
    let invocation = Invocation::new(
        adapter,
        HarnessExecutable::external(GrantPath::new(bin_path)?),
        args.args.clone(),
        GrantPath::new(&ctx.dir)?,
        ProfileEffect::None,
    );

    // Policy provenance: the digest of each layer's INPUT (what was compiled),
    // explicitly absent when the layer declares nothing.
    let machine_src = if machine_policy.is_empty() {
        PolicySource::Absent
    } else {
        let text = toml::to_string(machine_policy).context("serializing machine policy")?;
        PolicySource::Digest(ContentDigest::parse(&agentstack_core::digest::sha256_hex(
            text.as_bytes(),
        ))?)
    };
    let project_src = if m.policy.is_empty() {
        PolicySource::Absent
    } else {
        let text = toml::to_string(&m.policy).context("serializing project policy")?;
        PolicySource::Digest(ContentDigest::parse(&agentstack_core::digest::sha256_hex(
            text.as_bytes(),
        ))?)
    };
    let policy = PolicyGrant::new(ruleset, PolicyProvenance::new(machine_src, project_src));

    let mut b = GrantBuilder::new(
        project,
        invocation,
        policy,
        RuntimeImage::Host,
        GrantPosture::Host,
        EgressMode::Unconfined,
        ArtifactMode::CleanAtRest,
    );

    // Servers: bound from the resolution machinery's own output (the frozen
    // set strict verification just accepted). Executables and secret
    // authorizations derive per server.
    for (name, resolved) in &inputs.frozen {
        let Ok(r) = resolved else {
            anyhow::bail!("server '{name}' failed after verification — refusing to freeze");
        };
        b.add_server(name, GrantedServer::from_resolved(r)?)?;
        for reference in r.server.referenced_secrets() {
            b.add_secret(SecretGrant::new(
                &reference,
                SecretScope::Server(name.clone()),
                SecretLifetimeBinding::Unbound,
            )?)?;
        }
    }
    // Executables: the EXACT pins strict verification derived and judged — no
    // second derivation between check and freeze.
    for (server, pin) in &inputs.executable_pins {
        b.add_executable(
            &pin.path,
            pin.kind,
            ContentDigest::parse(pin.checksum.hex())?,
            server,
        )?;
    }

    // Skills: resolve after verification passed (every status was Matches, so
    // this NoFetch resolution reproduces the verified content identities).
    let store = crate::store::Store::default_store();
    for (name, _) in &inputs.skill_statuses {
        let r = crate::resolve::resolve_skill(
            m,
            &ctx.dir,
            &inputs.library,
            &inputs.lib_home,
            &store,
            name,
            crate::resolve::ResolveMode::NoFetch,
        )
        .map_err(|e| anyhow::anyhow!("skill '{name}' failed after verification: {e}"))?;
        let origin = match r.origin {
            crate::resolve::SkillOrigin::Inline => InputOrigin::Inline,
            crate::resolve::SkillOrigin::Library => InputOrigin::Library,
        };
        let source = match r.rev.clone() {
            Some(revision) => SkillSource::Git { revision },
            None => SkillSource::Path,
        };
        b.add_skill(
            name,
            GrantedSkill::new(
                GrantPath::new(&r.path)?,
                origin,
                source,
                ContentDigest::parse(&r.checksum)?,
                r.provenance.clone(),
            ),
        )?;
    }

    // Instructions: pinned digests come from the lock entries strict
    // verification just proved current.
    for (name, _) in &inputs.instruction_statuses {
        let instr = m
            .instructions
            .get(name)
            .context("instruction disappeared after verification")?;
        let entry = inputs.lock.get_instruction(name).with_context(|| {
            format!("instruction '{name}' lost its lock pin after verification")
        })?;
        let src = crate::render::instructions::fragment_source(&ctx.dir, &instr.path);
        b.add_instruction(
            name,
            GrantedInstruction::project_pinned(
                GrantPath::new(&src)?,
                ContentDigest::parse(entry.checksum.hex())?,
                instr.targets.iter().cloned().collect::<BTreeSet<String>>(),
            ),
        )?;
    }

    b.build()
}

/// Launch-scoped PROJECT MCP config (contract §3 step 7, host tier): for the
/// run's lifetime the harness's project-scope MCP config exposes ONLY the
/// synthetic gateway entry (`agentstack mcp --auto-project`); any pre-existing
/// project config is parked beside itself and restored on exit. The gateway
/// process re-gates trust and pins over the same lock bytes the grant froze,
/// so nothing reachable through it can widen authority. USER/GLOBAL-scope
/// entries are NOT swapped (harness apps rewrite their own global configs
/// mid-run — racing that risks clobbering user state); they are named in a
/// standalone honest warning instead.
struct ScopedMcpConfig {
    path: PathBuf,
    /// The kernel-enforced mutual-exclusion sentinel beside the config
    /// (`<config>.agentstack-locked.lock`), created with `create_new` so two
    /// racing locked runs can never both pass the guard. Contains only the
    /// run id — SECRET-FREE by construction, so a crash leftover in the repo
    /// can never leak anything.
    sentinel: PathBuf,
    /// The original config bytes (`None` when no config pre-existed). The
    /// secret-bearing original is parked OUTSIDE the repo, in the run's
    /// private 0700 recorder dir — a crash can never leave secrets sitting in
    /// the project one `git add -A` away from publication.
    original: Option<String>,
    /// The on-disk crash-recovery copy of `original`, in the run dir.
    parked_copy: Option<PathBuf>,
    /// The gateway-only body THIS run wrote — restore refuses to delete
    /// anything else (a swapped file means another process intervened).
    body: String,
    restored: bool,
}

impl ScopedMcpConfig {
    /// Guard + park + write, or `Ok(None)` when this harness has no
    /// project-scope MCP config to scope (stated honestly by the caller).
    fn apply(
        desc: &crate::adapter::AdapterDescriptor,
        project_dir: &Path,
        run_id: &str,
        run_dir: &Path,
    ) -> Result<Option<ScopedMcpConfig>> {
        use crate::adapter::descriptor::Format;
        let (Some((path, format)), Some(mcp)) = (
            desc.config_for(agentstack_core::scope::Scope::Project, project_dir),
            desc.mcp.as_ref(),
        ) else {
            return Ok(None);
        };
        let bridge = super::connect::bridge_server(&super::connect::bridge_command(None), false);
        let rendered =
            crate::adapter::render_server(desc, &bridge, &crate::secret::MapResolver::default());
        if !rendered.representable {
            return Ok(None);
        }
        let entries = vec![(super::connect::BRIDGE_ENTRY.to_string(), rendered.value)];
        let body = match format {
            Format::Json => crate::render::merge_json::merge("", &mcp.location, &entries)?,
            Format::Toml => crate::render::merge_toml::merge_with_removals(
                "",
                &mcp.location,
                &entries,
                &[],
                mcp.headers_as_subtable,
            )?,
        };

        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "mcp-config".to_string());
        // Concurrency guard, ATOMIC (`create_new`): a scan-then-rename guard
        // races — two runs passing the scan together would stack parks, so
        // the first run's restore silently WIDENS the second mid-flight. The
        // kernel arbitrates this one: exactly one creator wins, the loser
        // refuses naming the holder (or a crash leftover, with instructions).
        let sentinel = path.with_file_name(format!("{file_name}.agentstack-locked.lock"));
        {
            use std::io::Write;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            match opts.open(&sentinel) {
                Ok(mut f) => {
                    let _ = f.write_all(run_id.as_bytes());
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let holder = std::fs::read_to_string(&sentinel).unwrap_or_default();
                    anyhow::bail!(
                        "another locked run ({}) is scoping {} — wait for it to finish; if it \
                         crashed, its original config is under ~/.agentstack/runs/<that run>/ \
                         and removing {} re-enables locked runs here",
                        if holder.trim().is_empty() {
                            "unknown"
                        } else {
                            holder.trim()
                        },
                        path.display(),
                        sentinel.display()
                    );
                }
                Err(e) => {
                    return Err(e)
                        .context(format!("creating the scope guard {}", sentinel.display()))
                }
            }
        }

        // Park the (possibly secret-bearing) original OUTSIDE the repo.
        let original = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                let _ = std::fs::remove_file(&sentinel);
                return Err(e).context(format!("reading {}", path.display()));
            }
        };
        let parked_copy = match &original {
            Some(text) => {
                let copy = run_dir.join(format!("parked-{file_name}"));
                if let Err(e) = std::fs::write(&copy, text) {
                    let _ = std::fs::remove_file(&sentinel);
                    return Err(e).context("parking the original config in the run dir");
                }
                Some(copy)
            }
            None => None,
        };
        if let Err(e) =
            crate::util::atomic::write(&path, &body).context("writing the gateway-only config")
        {
            // Fail closed but leave nothing half-scoped; a rollback failure
            // is chained, never swallowed (the run-dir copy still exists).
            if let Some(orig) = &original {
                if let Err(roll) = crate::util::atomic::write(&path, orig) {
                    // Double failure: the config's on-disk state is unknown.
                    // KEEP the sentinel — the state needs attention, and a
                    // subsequent locked run must not park the corrupted state
                    // as its "original" (same rule as restore()'s failure).
                    return Err(e.context(format!(
                        "ALSO: restoring the original failed ({roll:#}) — your config is \
                         preserved at {}; the scope guard {} stays until this is resolved",
                        parked_copy.as_deref().unwrap_or(Path::new("?")).display(),
                        sentinel.display()
                    )));
                }
            }
            let _ = std::fs::remove_file(&sentinel);
            return Err(e);
        }
        Ok(Some(ScopedMcpConfig {
            path,
            sentinel,
            original,
            parked_copy,
            body,
            restored: false,
        }))
    }

    /// Put the original back and release the guard. Called explicitly on
    /// every exit path; `Drop` is the best-effort backstop. A crash before
    /// restore leaves the MORE restrictive state (gateway-only + sentinel +
    /// the original safe in the run dir) — fail-safe direction, never wider,
    /// never a secret in the repo. Every failure mode is LOUD: a restore
    /// that couldn't complete must never look like one that did.
    fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        // Only replace what this run wrote: different current content means
        // another process swapped the file — leave it intact.
        match std::fs::read_to_string(&self.path) {
            Ok(current) if current == self.body => {
                let outcome = match &self.original {
                    Some(orig) => crate::util::atomic::write(&self.path, orig),
                    None => std::fs::remove_file(&self.path).map_err(Into::into),
                };
                if let Err(e) = outcome {
                    eprintln!(
                        "  ⚠ could not restore {} ({e:#}) — your original is preserved at {}",
                        self.path.display(),
                        self.parked_copy
                            .as_deref()
                            .unwrap_or(Path::new("(it did not exist)"))
                            .display()
                    );
                    return; // keep the sentinel: the state needs attention
                }
                // Restored successfully: the crash-recovery copy (possibly
                // secret-bearing) has served its purpose — don't leave
                // plaintext secrets lingering in the run dir.
                if let Some(copy) = &self.parked_copy {
                    let _ = std::fs::remove_file(copy);
                }
            }
            _ => {
                eprintln!(
                    "  ⚠ {} changed during the run (not this run's gateway-only content) — \
                     leaving it in place; your original is preserved at {}",
                    self.path.display(),
                    self.parked_copy
                        .as_deref()
                        .unwrap_or(Path::new("(it did not exist)"))
                        .display()
                );
                // The swapper owns the file now; release the guard below.
            }
        }
        if let Err(e) = std::fs::remove_file(&self.sentinel) {
            eprintln!(
                "  ⚠ could not remove the scope guard {} ({e}) — remove it manually to \
                 re-enable locked runs here",
                self.sentinel.display()
            );
        }
    }
}

impl Drop for ScopedMcpConfig {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Resolve the harness binary to the full path that would execute — the grant
/// records the resolved identity, not the bare name (§6.1).
fn resolve_bin_path(bin: &str) -> Result<PathBuf> {
    let path = std::env::var_os("PATH").context("PATH is not set")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|p| p.is_file())
        .with_context(|| format!("'{bin}' is not on your PATH"))
}

fn print_posture_and_limits() {
    println!("  posture: {}", "HOST / PROTECTED".green().bold());
    eprintln!(
        "  {} protected host run: content trust, strict lock verification, and policy \
         admission are enforced BEFORE launch, and decisions are recorded. Not kernel \
         isolation: the harness runs as you, on the host; the harness/interpreter binary \
         itself is an unpinned $PATH executable; evidence is a cooperative local audit \
         trail. Use --sandbox/--lockdown for runtime containment.",
        "ℹ".cyan()
    );
    // Contract §3 step 7, the honest remainder — its own loud line, not a
    // buried clause: USER/GLOBAL-scope native MCP entries are not swapped
    // (harness apps rewrite their own global configs mid-run; racing that
    // risks clobbering user state). Project scope IS launch-scoped below.
    eprintln!(
        "  {} user/global-scope native MCP entries are NOT shadowed for this run — only \
         the project scope is launch-scoped to the gateway (run-grant handoff for the \
         bridge lands in the follow-up D2 session). Host-guard hooks apply where the \
         machine [guard] config installed them (cooperative).",
        "⚠".yellow()
    );
}

fn live(ctx: &Context, base: &Path, args: &RunArgs) -> Result<()> {
    let m = &ctx.loaded.manifest;
    let desc = ctx.registry.get(&args.harness).with_context(|| {
        format!(
            "unknown harness '{}' — see `agentstack adapters list`",
            args.harness
        )
    })?;
    let bin = desc
        .detect
        .bin
        .clone()
        .with_context(|| format!("{} has no known launch binary to run", desc.display))?;
    let display = desc.display.clone();
    let bin_path = resolve_bin_path(&bin)?;

    println!(
        "{} launching {} with --locked…",
        "▶".green(),
        args.harness.bold()
    );
    print_posture_and_limits();

    // §3 step 2: run identity + recorder BEFORE any gate, so a refusal is
    // itself recorded evidence. No recorder, no run.
    let run_id = crate::runs::gen_id();
    let log = RunLog::create(&run_id)
        .context("could not create the per-run flight recorder — refusing to run unobserved")?;
    let ev = Evidence {
        log,
        run_id: run_id.clone(),
        started: Instant::now(),
    };
    ev.material(&RunEvent::AttemptStarted {
        ts: ts(),
        harness: args.harness.clone(),
        posture: "host-protected".to_string(),
    })?;

    // §3 step 3: trust, enforced.
    match trust::check(base) {
        TrustState::Trusted => {
            ev.passed("trust", trust::digest_for(base))?;
            println!("  {} trust: explicitly trusted", "✓".green());
        }
        TrustState::Untrusted => {
            return Err(ev.refuse(
                "trust",
                anyhow::anyhow!(
                    "this project is not trusted — review it, then run `agentstack trust .` \
                     (trust is content-bound: it pins the manifest + lockfile digest)"
                ),
            ));
        }
        TrustState::Changed => {
            return Err(ev.refuse(
                "trust",
                anyhow::anyhow!(
                    "the agent configuration changed since you trusted it — review the changes \
                     (`agentstack trust .` shows the surface), then re-trust"
                ),
            ));
        }
    }

    // §3 step 4: strict locked-input verification; missing pins block.
    let inputs = match resolve_inputs(ctx) {
        Ok(i) => i,
        Err(e) => return Err(ev.refuse("locked-verify", e)),
    };
    if let Err(e) = crate::verify::ensure_locked_inputs(
        &args.harness,
        &inputs.skill_statuses,
        &inputs.instruction_statuses,
        &inputs.frozen,
        &inputs.executable_statuses,
        &inputs.extension_statuses,
    ) {
        return Err(ev.refuse("locked-verify", e));
    }
    ev.passed("locked-verify", None)?;
    println!(
        "  {} locked inputs: {} skill(s), {} instruction(s), {} server(s), {} executable pin(s), {} extension(s) verified",
        "✓".green(),
        inputs.skill_statuses.len(),
        inputs.instruction_statuses.len(),
        inputs.frozen.len(),
        inputs.executable_statuses.len(),
        inputs.extension_statuses.len(),
    );

    // §3 step 4b: rendered-copy verification (E2b). locked-verify proved the
    // extension SOURCE bytes still match the pin; this proves the COPY already
    // delivered into this harness's extension directory does too — a rendered
    // extension tampered after render (source untouched) would otherwise reach
    // the harness unreviewed. Nothing rendered for this harness = nothing to
    // verify, never a refusal.
    let rendered = match crate::render::extensions::verify_rendered(
        m,
        &ctx.registry,
        &args.harness,
        args.scope,
        &ctx.dir,
        &inputs.lock,
    ) {
        Ok(r) => r,
        Err(e) => return Err(ev.refuse("rendered-verify", e)),
    };
    ev.passed("rendered-verify", None)?;
    println!(
        "  {} rendered extensions: {} verified, {} not rendered",
        "✓".green(),
        rendered.verified.len(),
        rendered.absent.len(),
    );

    // §3 step 5: compile once, then check the enumerable admission surface.
    let machine_policy = match crate::machine_policy::load() {
        Ok(p) => p,
        Err(e) => return Err(ev.refuse("policy-admission", e)),
    };
    let names: Vec<&str> = m.servers.keys().map(String::as_str).collect();
    let ruleset = agentstack_policy::compile(&machine_policy, &m.policy, &names);
    let refusals = admission_refusals(&ruleset, &inputs.frozen);
    if !refusals.is_empty() {
        return Err(ev.refuse("policy-admission", admission_error(&refusals)));
    }
    ev.passed("policy-admission", None)?;
    println!(
        "  {} policy: declared requests fit under the machine ceiling",
        "✓".green()
    );

    // §3 step 6 + §4: freeze the grant under the machine-local commitment key.
    // No key, no invocation-binding digest, no launch — never an unkeyed
    // fallback. Provisioning is machine-local key material, not repo content.
    if let Err(e) = crate::grant::provision_commitment_key() {
        return Err(ev.refuse("grant-freeze", e));
    }
    let key = match crate::grant::load_commitment_key() {
        Ok(k) => k,
        Err(e) => return Err(ev.refuse("grant-freeze", e)),
    };
    let grant = match freeze_grant(
        ctx,
        base,
        args,
        &inputs,
        ruleset,
        &machine_policy,
        &bin_path,
    ) {
        Ok(g) => g,
        Err(e) => return Err(ev.refuse("grant-freeze", e)),
    };
    let digest: GrantDigest = match grant.digest(&key) {
        Ok(d) => d,
        Err(e) => return Err(ev.refuse("grant-freeze", e)),
    };
    ev.material(&RunEvent::GrantFrozen {
        ts: ts(),
        grant_digest: digest.to_string(),
    })?;
    println!("  {} authority grant frozen: {}", "✓".green(), digest);

    // §6.2: the evidence identity around this frozen grant — the one place
    // the grant digest lives from here on.
    let envelope =
        crate::grant::RunEnvelope::new(run_id.clone(), ev.log.path().display().to_string(), digest);

    // §3 step 7 (host tier, per ruling): launch-scope the PROJECT MCP config
    // to the synthetic gateway entry for the run's lifetime; restore after.
    // Global scope stays honestly labeled (see print_posture_and_limits).
    let run_dir = ev
        .log
        .path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            crate::util::paths::agentstack_home()
                .join("runs")
                .join(&run_id)
        });
    let mut scoped = match ScopedMcpConfig::apply(desc, &ctx.dir, &run_id, &run_dir) {
        Ok(Some(s)) => {
            println!(
                "  {} project MCP config launch-scoped to the gateway ({})",
                "✓".green(),
                s.path.display()
            );
            Some(s)
        }
        Ok(None) => {
            println!(
                "  {} {} has no project-scope MCP config to launch-scope (stated honestly; \
                 the global bridge entry still gates declared servers)",
                "ℹ".cyan(),
                display
            );
            None
        }
        Err(e) => return Err(ev.refuse("launch-scope", e)),
    };

    // Spawn on the host under the SAME run id the evidence carries, with the
    // run id exported for gateway audit attribution.
    // Spawn at the PROJECT root, not the manifest dir — under the preferred
    // layout ctx.dir is `.agentstack/`, and a harness opened there sees no
    // source code (and sits one mistake away from the rendered configs).
    let status = crate::runs::launch_attached(
        &bin_path.to_string_lossy(),
        &args.args,
        base,
        &run_id,
        &args.harness,
        &display,
        None,
        args.scope,
    );
    if let Some(s) = scoped.as_mut() {
        s.restore();
    }

    // §3 step 8: terminal outcome — observed evidence or explicit
    // "unavailable", never fabricated.
    match status {
        Ok(st) => {
            ev.material(&RunEvent::LockedOutcome {
                ts: ts(),
                outcome: "completed".to_string(),
                exit_code: st.code(),
                duration_ms: ev.started.elapsed().as_millis() as u64,
                grant_digest: Some(envelope.grant_digest().to_string()),
                usage: "unavailable".to_string(),
            })
            .context("the harness ran, but its outcome could not be recorded")?;
            Ok(())
        }
        Err(e) => {
            // Not a gate refusal: every gate passed and the grant froze; the
            // spawn (or the wait on a process that may have started) failed.
            // Record a distinct terminal outcome carrying the grant digest.
            let record = ev.material(&RunEvent::LockedOutcome {
                ts: ts(),
                outcome: "launch-failed".to_string(),
                exit_code: None,
                duration_ms: ev.started.elapsed().as_millis() as u64,
                grant_digest: Some(envelope.grant_digest().to_string()),
                usage: "unavailable".to_string(),
            });
            match record {
                Ok(()) => Err(e),
                Err(rec) => Err(e.context(format!(
                    "ALSO: the launch failure's evidence could not be recorded ({rec:#})"
                ))),
            }
        }
    }
}

/// `--locked --plan` (§2.2): a non-mutating, secret-free evaluation. Applies
/// no profile, resolves no secret, creates no recorder log, invents no run id,
/// AGGREGATES every blocker, prints the proposed grant, and exits nonzero when
/// a live launch would be refused.
fn plan(ctx: &Context, base: &Path, args: &RunArgs) -> Result<()> {
    let m = &ctx.loaded.manifest;
    println!(
        "{} plan for `run {} --locked` (nothing will be mutated)",
        "→".cyan(),
        args.harness.bold()
    );
    print_posture_and_limits();

    let mut blockers: Vec<(String, String)> = Vec::new();

    let desc = ctx.registry.get(&args.harness);
    let bin_path = match desc {
        None => {
            blockers.push((
                "harness".into(),
                format!("unknown harness '{}'", args.harness),
            ));
            None
        }
        Some(d) => match d.detect.bin.as_deref().map(resolve_bin_path) {
            None => {
                blockers.push((
                    "harness".into(),
                    format!("{} has no known launch binary", d.display),
                ));
                None
            }
            Some(Err(e)) => {
                blockers.push(("harness".into(), format!("{e:#}")));
                None
            }
            Some(Ok(p)) => Some(p),
        },
    };

    match trust::check(base) {
        TrustState::Trusted => println!("  {} trust: explicitly trusted", "✓".green()),
        TrustState::Untrusted => blockers.push((
            "trust".into(),
            "project is not trusted — run `agentstack trust .` after reviewing".into(),
        )),
        TrustState::Changed => blockers.push((
            "trust".into(),
            "configuration changed since it was trusted — re-review and re-trust".into(),
        )),
    }

    let inputs = match resolve_inputs(ctx) {
        Ok(inputs) => {
            if let Err(e) = crate::verify::ensure_locked_inputs(
                &args.harness,
                &inputs.skill_statuses,
                &inputs.instruction_statuses,
                &inputs.frozen,
                &inputs.executable_statuses,
                &inputs.extension_statuses,
            ) {
                blockers.push(("locked-verify".into(), format!("{e:#}")));
            }
            Some(inputs)
        }
        Err(e) => {
            blockers.push(("locked-verify".into(), format!("{e:#}")));
            None
        }
    };

    // Rendered-copy verification (E2b), aggregated like every other blocker.
    // Non-mutating: it only reads the delivered artifacts and their ledger.
    let rendered = inputs.as_ref().and_then(|inputs| {
        match crate::render::extensions::verify_rendered(
            m,
            &ctx.registry,
            &args.harness,
            args.scope,
            &ctx.dir,
            &inputs.lock,
        ) {
            Ok(r) => Some(r),
            Err(e) => {
                blockers.push(("rendered-verify".into(), format!("{e:#}")));
                None
            }
        }
    });

    let mut ruleset_and_machine = None;
    match crate::machine_policy::load() {
        Ok(machine) => {
            let names: Vec<&str> = m.servers.keys().map(String::as_str).collect();
            let ruleset = agentstack_policy::compile(&machine, &m.policy, &names);
            if let Some(inputs) = &inputs {
                for (who, why) in admission_refusals(&ruleset, &inputs.frozen) {
                    blockers.push((format!("policy-admission {who}"), why));
                }
            }
            ruleset_and_machine = Some((ruleset, machine));
        }
        Err(e) => blockers.push(("policy-admission".into(), format!("{e:#}"))),
    }

    // §4: --plan never provisions. A commitment key that was simply never
    // provisioned (a fresh home with no `grant/` yet) is NOT a blocker — the
    // first LIVE run creates it, so the plan must not contradict live for the
    // cautious first-time user. It reports the key will be created and proceeds
    // without the invocation-binding digest. A present-but-broken key
    // (corrupt/insecure/symlink) still blocks.
    let key = match crate::grant::plan_commitment_key() {
        crate::grant::PlanKeyState::Ready(k) => Some(k),
        crate::grant::PlanKeyState::WillProvision => {
            println!(
                "  {} commitment key: will be created on first live run",
                "ℹ".cyan()
            );
            None
        }
        crate::grant::PlanKeyState::Blocked(e) => {
            blockers.push((
                "argv-commitment".into(),
                format!("no invocation-binding digest without the machine commitment key: {e:#}"),
            ));
            None
        }
    };

    // The proposed grant, redacted: identities and counts, never argv values.
    println!("  proposed grant:");
    println!("    project: {}", base.display());
    println!(
        "    harness: {} ({} redacted argument(s))",
        args.harness,
        args.args.len()
    );
    if let Some(inputs) = &inputs {
        let servers: Vec<&str> = inputs.frozen.iter().map(|(n, _)| n.as_str()).collect();
        println!(
            "    servers: {}",
            if servers.is_empty() {
                "(none)".to_string()
            } else {
                servers.join(", ")
            }
        );
        println!(
            "    inputs: {} skill(s), {} instruction(s), {} executable pin(s), {} extension(s)",
            inputs.skill_statuses.len(),
            inputs.instruction_statuses.len(),
            inputs.executable_statuses.len(),
            inputs.extension_statuses.len()
        );
        if let Some(rendered) = &rendered {
            println!(
                "    rendered extensions: {} verified, {} not rendered",
                rendered.verified.len(),
                rendered.absent.len()
            );
        }
    }

    if blockers.is_empty() {
        // All gates green: the digest of the exact grant a live run would freeze.
        let (ruleset, machine) = ruleset_and_machine.expect("no blockers implies policy compiled");
        let inputs = inputs
            .as_ref()
            .expect("no blockers implies inputs resolved");
        let bin_path = bin_path.expect("no blockers implies harness resolved");
        let grant = freeze_grant(ctx, base, args, inputs, ruleset, &machine, &bin_path)?;
        match key {
            // The key is present: show the exact binding digest a live run
            // would freeze (the "plan matches run" property).
            Some(key) => println!("    digest: {}", grant.digest(&key)?),
            // The key will be provisioned on first live run; the invocation-
            // binding digest can only be computed once it exists.
            None => {
                println!("    digest: (bound on first live run, once the commitment key exists)")
            }
        }
        println!("{} live launch would proceed", "✓".green());
        return Ok(());
    }

    let lines: Vec<String> = blockers
        .iter()
        .map(|(gate, why)| format!("  [{gate}] {why}"))
        .collect();
    anyhow::bail!(
        "a live `run {} --locked` would be REFUSED — {} blocker(s):\n{}",
        args.harness,
        blockers.len(),
        lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    /// Serialize env mutation (AGENTSTACK_HOME + PATH) and isolate the trust
    /// store, library, machine policy, commitment key, and recorder under a
    /// temp home. Returns (home, project) tempdirs plus a PATH dir carrying a
    /// fake `claude` harness script that exits 0.
    fn locked_fixture(f: impl FnOnce(&assert_fs::TempDir, &assert_fs::TempDir)) {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        let proj = assert_fs::TempDir::new().unwrap();
        let bins = home.child("fakebin");
        bins.create_dir_all().unwrap();
        let fake = bins.child("claude");
        fake.write_str("#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(fake.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let old_path = std::env::var_os("PATH");
        let new_path = std::env::join_paths(
            std::iter::once(bins.path().to_path_buf())
                .chain(old_path.iter().flat_map(std::env::split_paths)),
        )
        .unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        std::env::set_var("PATH", &new_path);

        f(&home, &proj);

        std::env::remove_var("AGENTSTACK_HOME");
        match old_path {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
    }

    /// A trusted project with one inline stdio server whose local script is
    /// pinned — the clean state a protected run should accept.
    fn pinned_and_trusted(proj: &assert_fs::TempDir) {
        proj.child("tool.sh")
            .write_str("#!/bin/sh\necho v1\n")
            .unwrap();
        proj.child("agentstack.toml")
            .write_str(
                "version = 1\n\n[servers.agent]\ntype = \"stdio\"\ncommand = \"./tool.sh\"\n",
            )
            .unwrap();
        // Pin the executable surface the way `agentstack lock` does.
        let manifest: crate::manifest::Manifest =
            toml::from_str(&std::fs::read_to_string(proj.child("agentstack.toml").path()).unwrap())
                .unwrap();
        let mut lock = agentstack_core::lock::Lock::default();
        for pin in crate::executable::derive_executable_pins(
            proj.path(),
            "agent",
            manifest.servers.get("agent").unwrap(),
        )
        .unwrap()
        {
            lock.upsert_executable(pin);
        }
        lock.save(proj.path()).unwrap();
        trust::trust(proj.path()).unwrap();
    }

    fn run_args(plan: bool) -> RunArgs {
        RunArgs {
            harness: "claude-code".to_string(),
            locked: true,
            profile: None,
            scope: agentstack_core::scope::Scope::Project,
            keep: false,
            sandbox: false,
            lockdown: false,
            plan,
            args: Vec::new(),
        }
    }

    /// Read the single recorded run's events under the isolated home.
    fn recorded_events(home: &assert_fs::TempDir) -> Vec<RunEvent> {
        let runs = home.path().join("runs");
        let mut entries: Vec<_> = std::fs::read_dir(&runs)
            .map(|d| d.filter_map(|e| e.ok()).collect())
            .unwrap_or_default();
        assert_eq!(entries.len(), 1, "exactly one recorded run expected");
        let id = entries.remove(0).file_name().to_string_lossy().into_owned();
        crate::calllog::RunLog::read(&id)
    }

    /// E2b witness (design doc §6): a one-byte edit to a RENDERED extension
    /// copy — the SOURCE left untouched, so trust and source locked-verify both
    /// still pass — refuses the locked run before launch, names the extension
    /// (kind-qualified), and records the `rendered-verify` gate refusal with no
    /// grant frozen. NEVER delete or weaken this test.
    #[cfg(unix)]
    #[test]
    fn tampered_rendered_extension_refuses_before_launch() {
        locked_fixture(|home, proj| {
            use std::os::unix::fs::PermissionsExt;
            // A fake `pi` harness on PATH — the extension's target adapter.
            let pi = home.path().join("fakebin/pi");
            std::fs::write(&pi, "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();

            // A pi project declaring one directory-source extension.
            proj.child("extensions/checkpoint/index.ts")
                .write_str("export default (pi) => {} // v1\n")
                .unwrap();
            proj.child("agentstack.toml")
                .write_str(
                    "version = 1\n\n[extensions.checkpoint]\npath = \"./extensions/checkpoint\"\ntarget = \"pi\"\n",
                )
                .unwrap();
            let manifest: crate::manifest::Manifest = toml::from_str(
                &std::fs::read_to_string(proj.child("agentstack.toml").path()).unwrap(),
            )
            .unwrap();

            // Pin the source, trust, then render the copy into .pi/extensions.
            crate::commands::lock::record_extension_pins(
                proj.path(),
                &manifest,
                &crate::library::Library::default(),
                &crate::util::paths::lib_home(),
                &crate::store::Store::default_store(),
            )
            .unwrap();
            trust::trust(proj.path()).unwrap();
            let registry = crate::adapter::registry::Registry::load().unwrap();
            crate::render::extensions::render(
                &manifest,
                &registry,
                agentstack_core::scope::Scope::Project,
                proj.path(),
                true,
            )
            .unwrap();
            let copy = proj.child(".pi/extensions/checkpoint/index.ts");
            assert!(copy.path().exists(), "render must deliver the copy");

            // Tamper ONLY the rendered copy: the source (and thus the lock and
            // the trust digest) is untouched, so trust + source locked-verify
            // pass and only the rendered-copy check can catch this.
            copy.write_str("export default (pi) => {} // TAMPERED\n")
                .unwrap();

            let args = RunArgs {
                harness: "pi".to_string(),
                locked: true,
                profile: None,
                scope: agentstack_core::scope::Scope::Project,
                keep: false,
                sandbox: false,
                lockdown: false,
                plan: false,
                args: Vec::new(),
            };
            let err = run_locked(Some(proj.path()), &args).unwrap_err();
            let msg = format!("{err:#}");
            assert!(
                msg.contains("extension 'checkpoint'"),
                "offender named: {msg}"
            );
            assert!(msg.contains("rendered copy"), "kind-qualified: {msg}");

            let events = recorded_events(home);
            // Source locked-verify passed; the rendered-verify gate refused.
            assert!(
                events.iter().any(|e| matches!(
                    e,
                    RunEvent::GateDecision { gate, passed: true, .. } if gate == "locked-verify"
                )),
                "{events:?}"
            );
            assert!(
                events.iter().any(|e| matches!(
                    e,
                    RunEvent::GateDecision { gate, passed: false, detail: Some(d), .. }
                        if gate == "rendered-verify" && d.contains("checkpoint")
                )),
                "{events:?}"
            );
            // The grant never froze — the refusal happened before launch.
            assert!(!events
                .iter()
                .any(|e| matches!(e, RunEvent::GrantFrozen { .. })));
        });
    }

    /// Demo 3 (contract §10): a one-byte edit to a pinned repository-local
    /// executable refuses BEFORE launch, names the offender, directs to
    /// `agentstack lock`, and the refusal is recorded with no grant digest.
    ///
    /// NEVER delete or weaken this test.
    #[test]
    fn one_byte_executable_edit_refuses_before_launch_and_is_recorded() {
        locked_fixture(|home, proj| {
            pinned_and_trusted(proj);
            // The drift: one byte in the pinned script, AFTER trust. The trust
            // digest (manifest + lock) is untouched — strict verification is
            // what must catch this.
            proj.child("tool.sh")
                .write_str("#!/bin/sh\necho v2\n")
                .unwrap();

            let err = run_locked(Some(proj.path()), &run_args(false)).unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains("tool.sh"), "offender named: {msg}");
            assert!(
                msg.contains("`agentstack lock`"),
                "directed to re-lock: {msg}"
            );

            let events = recorded_events(home);
            assert!(
                matches!(&events[0], RunEvent::AttemptStarted { posture, .. } if posture == "host-protected"),
                "{events:?}"
            );
            // Trust passed (its digest is unchanged), the verify gate refused.
            assert!(events.iter().any(|e| matches!(
                e,
                RunEvent::GateDecision { gate, passed: true, .. } if gate == "trust"
            )));
            assert!(events.iter().any(|e| matches!(
                e,
                RunEvent::GateDecision { gate, passed: false, detail: Some(d), .. }
                    if gate == "locked-verify" && d.contains("tool.sh")
            )));
            // Terminal refusal with NO grant digest (the grant never froze),
            // and no GrantFrozen event anywhere.
            assert!(events.iter().any(|e| matches!(
                e,
                RunEvent::LockedOutcome { outcome, grant_digest: None, .. } if outcome == "refused"
            )));
            assert!(!events
                .iter()
                .any(|e| matches!(e, RunEvent::GrantFrozen { .. })));
        });
    }

    /// An untrusted project refuses at the trust gate — and the refusal is
    /// still recorded evidence (recorder opens before any gate).
    #[test]
    fn untrusted_project_refuses_at_the_trust_gate() {
        locked_fixture(|home, proj| {
            proj.child("agentstack.toml")
                .write_str("version = 1\n")
                .unwrap();

            let err = run_locked(Some(proj.path()), &run_args(false)).unwrap_err();
            assert!(format!("{err:#}").contains("agentstack trust"), "{err:#}");

            let events = recorded_events(home);
            assert!(events.iter().any(|e| matches!(
                e,
                RunEvent::GateDecision { gate, passed: false, .. } if gate == "trust"
            )));
        });
    }

    /// Demo 1 shape: a clean trusted project passes every gate, freezes the
    /// grant (recorded with its digest), launches the harness, and records the
    /// outcome with an explicit `unavailable` usage — never a fabricated one.
    #[cfg(unix)]
    #[test]
    fn clean_trusted_project_freezes_grant_launches_and_records_outcome() {
        locked_fixture(|home, proj| {
            pinned_and_trusted(proj);

            run_locked(Some(proj.path()), &run_args(false)).unwrap();

            let events = recorded_events(home);
            let frozen_digest = events.iter().find_map(|e| match e {
                RunEvent::GrantFrozen { grant_digest, .. } => Some(grant_digest.clone()),
                _ => None,
            });
            let digest = frozen_digest.expect("grant froze");
            assert!(digest.starts_with("sha256:"), "{digest}");
            assert!(events.iter().any(|e| matches!(
                e,
                RunEvent::LockedOutcome { outcome, exit_code: Some(0), grant_digest: Some(d), usage, .. }
                    if outcome == "completed" && *d == digest && usage == "unavailable"
            )), "{events:?}");
        });
    }

    /// Demo 2 (contract §10): a declared host that CANNOT be classified (a
    /// `${REF}` in the host portion) blocks policy admission before launch —
    /// it can't be checked against the machine egress ceiling — and the
    /// refusal is recorded.
    #[test]
    fn unclassifiable_declared_host_refuses_at_policy_admission() {
        locked_fixture(|home, proj| {
            proj.child("agentstack.toml")
                .write_str(
                    "version = 1\n\n[servers.api]\ntype = \"http\"\nurl = \"https://${API_HOST}/mcp\"\n",
                )
                .unwrap();
            trust::trust(proj.path()).unwrap();

            let err = run_locked(Some(proj.path()), &run_args(false)).unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains("no classifiable host"), "{msg}");
            assert!(msg.contains("server 'api'"), "{msg}");

            let events = recorded_events(home);
            assert!(events.iter().any(|e| matches!(
                e,
                RunEvent::GateDecision { gate, passed: false, .. } if gate == "policy-admission"
            )));
            assert!(!events
                .iter()
                .any(|e| matches!(e, RunEvent::GrantFrozen { .. })));
        });
    }

    /// §3 step 2 (round-3 correction): when recording a refusal itself fails,
    /// BOTH the original refusal and the evidence failure surface — and the
    /// run still never launches (refuse() returns an error either way).
    #[cfg(unix)]
    #[test]
    fn refusal_recording_failure_is_surfaced_alongside_the_refusal() {
        locked_fixture(|_home, _proj| {
            use std::os::unix::fs::PermissionsExt;
            let log = crate::calllog::RunLog::create("r-surfaceboth").unwrap();
            // A read-only run dir: events.jsonl can no longer be created, so
            // every append fails.
            let dir = crate::util::paths::agentstack_home().join("runs/r-surfaceboth");
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();

            let ev = Evidence {
                log,
                run_id: "r-surfaceboth".to_string(),
                started: Instant::now(),
            };
            let err = ev.refuse("trust", anyhow::anyhow!("the original refusal"));
            let msg = format!("{err:#}");
            assert!(msg.contains("the original refusal"), "{msg}");
            assert!(msg.contains("ALSO"), "{msg}");
            assert!(msg.contains("could not be fully recorded"), "{msg}");

            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        });
    }

    /// Contract §3 step 7 (host tier): for the run's lifetime the project
    /// MCP config exposes ONLY the synthetic gateway entry — observed from
    /// INSIDE the run by the harness itself — and the pre-existing project
    /// config comes back byte-identical afterward, with no leftovers.
    #[cfg(unix)]
    #[test]
    fn project_mcp_config_is_gateway_only_during_the_run_and_restored_after() {
        locked_fixture(|home, proj| {
            pinned_and_trusted(proj);
            // A pre-existing project MCP config with an ambient entry that
            // must NOT be reachable during the locked run.
            let ambient = r#"{"mcpServers":{"ambient":{"command":"evil"}}}"#;
            proj.child(".mcp.json").write_str(ambient).unwrap();
            // The fake harness snapshots what it actually sees.
            let fake = home.path().join("fakebin/claude");
            std::fs::write(
                &fake,
                "#!/bin/sh\ncat .mcp.json > mcp-during-run.json\nexit 0\n",
            )
            .unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

            run_locked(Some(proj.path()), &run_args(false)).unwrap();

            let during = std::fs::read_to_string(proj.child("mcp-during-run.json").path()).unwrap();
            assert!(
                during.contains("agentstack"),
                "gateway entry present: {during}"
            );
            assert!(
                !during.contains("ambient") && !during.contains("evil"),
                "ambient project entry shadowed during the run: {during}"
            );

            let after = std::fs::read_to_string(proj.child(".mcp.json").path()).unwrap();
            assert_eq!(after, ambient, "original restored byte-identical");
            // No sentinel or park artifact remains in the PROJECT — the
            // secret-bearing original is parked in the run dir, never in the
            // repo (a crash must not leave secrets one `git add` away).
            let leftovers: Vec<_> = std::fs::read_dir(proj.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .contains("agentstack-locked")
                })
                .collect();
            assert!(
                leftovers.is_empty(),
                "no scope artifacts left: {leftovers:?}"
            );
        });
    }

    /// The harness must launch at the PROJECT root even when the manifest
    /// lives in the preferred `.agentstack/` layout — a session opened inside
    /// `.agentstack/` sees no source code and sits next to rendered configs.
    /// (The other tests use the legacy root layout, where the two coincide.)
    #[cfg(unix)]
    #[test]
    fn harness_launches_at_the_project_root_for_the_preferred_layout() {
        locked_fixture(|home, proj| {
            // Preferred layout: manifest (+ lock) under .agentstack/, the
            // pinned tool at the project root where `./tool.sh` resolves.
            proj.child("tool.sh")
                .write_str("#!/bin/sh\necho v1\n")
                .unwrap();
            proj.child(".agentstack/agentstack.toml")
                .write_str(
                    "version = 1\n\n[servers.agent]\ntype = \"stdio\"\ncommand = \"./tool.sh\"\n",
                )
                .unwrap();
            let manifest: crate::manifest::Manifest = toml::from_str(
                &std::fs::read_to_string(proj.child(".agentstack/agentstack.toml").path()).unwrap(),
            )
            .unwrap();
            let mut lock = agentstack_core::lock::Lock::default();
            for pin in crate::executable::derive_executable_pins(
                proj.path(),
                "agent",
                manifest.servers.get("agent").unwrap(),
            )
            .unwrap()
            {
                lock.upsert_executable(pin);
            }
            lock.save(proj.child(".agentstack").path()).unwrap();
            trust::trust(proj.path()).unwrap();

            // The fake harness records the directory it was launched from.
            let fake = home.path().join("fakebin/claude");
            std::fs::write(&fake, "#!/bin/sh\npwd > launched-from.txt\nexit 0\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

            run_locked(Some(proj.path()), &run_args(false)).unwrap();

            // Written at the project root, not inside .agentstack/ …
            assert!(
                !proj.child(".agentstack/launched-from.txt").path().exists(),
                "harness must not launch inside the manifest dir"
            );
            let recorded = std::fs::read_to_string(proj.child("launched-from.txt").path()).unwrap();
            // … and the recorded cwd IS the project root (canonicalized: the
            // kernel reports /private/var/… where TempDir says /var/…).
            assert_eq!(
                PathBuf::from(recorded.trim()).canonicalize().unwrap(),
                proj.path().canonicalize().unwrap(),
                "harness cwd must be the project root"
            );
        });
    }

    /// Concurrency guard: a parked backup beside the project config (another
    /// locked run in flight, or a crash leftover) refuses the run instead of
    /// stacking parks — stacked parks would widen the sibling mid-run and
    /// destroy the true original on out-of-order exits.
    #[cfg(unix)]
    #[test]
    fn overlapping_locked_run_refuses_instead_of_stacking_parks() {
        locked_fixture(|_home, proj| {
            pinned_and_trusted(proj);
            // The atomic sentinel another in-flight run would hold.
            proj.child(".mcp.json.agentstack-locked.lock")
                .write_str("r-other")
                .unwrap();

            let err = run_locked(Some(proj.path()), &run_args(false)).unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains("another locked run (r-other)"), "{msg}");
        });
    }

    /// §2.2: `--plan` aggregates EVERY blocker (here: untrusted AND drifted),
    /// mutates nothing, and creates no run id / recorder log.
    #[test]
    fn plan_aggregates_blockers_and_records_nothing() {
        locked_fixture(|home, proj| {
            pinned_and_trusted(proj);
            proj.child("tool.sh")
                .write_str("#!/bin/sh\necho v2\n")
                .unwrap();
            trust::revoke(proj.path()).unwrap();

            let err = run_locked(Some(proj.path()), &run_args(true)).unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains("[trust]"), "{msg}");
            assert!(msg.contains("[locked-verify]"), "{msg}");
            assert!(msg.contains("tool.sh"), "{msg}");

            // Non-mutating: no recorder directory, no run id was invented.
            assert!(
                !home.path().join("runs").exists(),
                "--plan must not create recorder state"
            );
        });
    }

    /// Issue #21: on a FRESH home (no `grant/` yet), `--locked --plan` for a
    /// trusted, pinned project must NOT contradict the live run. The live run
    /// provisions the commitment key on first use, so the plan treats a
    /// never-provisioned key as informational — not an `argv-commitment`
    /// blocker — and still reports the run would proceed, WITHOUT provisioning
    /// anything itself. A key that is present but broken STILL blocks.
    #[test]
    fn plan_treats_never_provisioned_commitment_key_as_informational() {
        locked_fixture(|home, proj| {
            pinned_and_trusted(proj);
            // Fresh home: the commitment key was never provisioned.
            assert!(
                !home.path().join("grant").exists(),
                "fixture must start without a commitment key"
            );

            // Plan succeeds — a never-provisioned key is not a blocker for the
            // cautious first-time user, so live and plan agree.
            run_locked(Some(proj.path()), &run_args(true)).unwrap();

            // Non-mutating: plan neither provisioned the key (no `grant/`) nor
            // opened a recorder (no `runs/`). Only the live run creates the key.
            assert!(
                !home.path().join("grant").exists(),
                "--plan must not provision the commitment key"
            );
            assert!(
                !home.path().join("runs").exists(),
                "--plan must not create recorder state"
            );

            // But a PRESENT-but-broken key still blocks: a zero-byte key file
            // is malformed (exactly 32 bytes are required), so plan must refuse
            // and name the `argv-commitment` gate.
            let grant = home.child("grant");
            grant.create_dir_all().unwrap();
            grant.child("commit-key").write_str("").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(grant.path(), std::fs::Permissions::from_mode(0o700))
                    .unwrap();
                std::fs::set_permissions(
                    grant.child("commit-key").path(),
                    std::fs::Permissions::from_mode(0o600),
                )
                .unwrap();
            }
            let err = run_locked(Some(proj.path()), &run_args(true)).unwrap_err();
            assert!(
                format!("{err:#}").contains("[argv-commitment]"),
                "a present-but-broken key must still block: {err:#}"
            );
        });
    }
}
