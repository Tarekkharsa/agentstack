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
            ContentDigest::parse(&pin.checksum)?,
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
                ContentDigest::parse(&entry.checksum)?,
                instr.targets.iter().cloned().collect::<BTreeSet<String>>(),
            ),
        )?;
    }

    b.build()
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
    // Contract §3 step 7 is NOT yet delivered — its own loud line, not a
    // buried clause: pre-existing native MCP entries stay reachable around
    // the grant until the D2 unification lands launch-scoped config.
    eprintln!(
        "  {} not yet launch-scoped: MCP servers already in this harness's native config \
         remain reachable OUTSIDE the frozen grant for this run (launch-scoped MCP \
         config lands with the D2 unification).",
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
    ) {
        return Err(ev.refuse("locked-verify", e));
    }
    ev.passed("locked-verify", None)?;
    println!(
        "  {} locked inputs: {} skill(s), {} instruction(s), {} server(s), {} executable pin(s) verified",
        "✓".green(),
        inputs.skill_statuses.len(),
        inputs.instruction_statuses.len(),
        inputs.frozen.len(),
        inputs.executable_statuses.len(),
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

    // §3 step 7 (this increment's honest subset): spawn on the host under the
    // SAME run id the evidence carries, with the run id exported for gateway
    // audit attribution. Launch-scoped MCP shadowing lands with D2.
    let status = crate::runs::launch_attached(
        &bin_path.to_string_lossy(),
        &args.args,
        &ctx.dir,
        &run_id,
        &args.harness,
        &display,
        None,
        args.scope,
    );

    // §3 step 8: terminal outcome — observed evidence or explicit
    // "unavailable", never fabricated.
    match status {
        Ok(st) => {
            ev.material(&RunEvent::LockedOutcome {
                ts: ts(),
                outcome: "completed".to_string(),
                exit_code: st.code(),
                duration_ms: ev.started.elapsed().as_millis() as u64,
                grant_digest: Some(digest.to_string()),
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
                grant_digest: Some(digest.to_string()),
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

    // §4: --plan never provisions; a missing commitment key is a blocker and
    // no invocation-binding digest exists without it.
    let key = match crate::grant::load_commitment_key() {
        Ok(k) => Some(k),
        Err(e) => {
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
            "    inputs: {} skill(s), {} instruction(s), {} executable pin(s)",
            inputs.skill_statuses.len(),
            inputs.instruction_statuses.len(),
            inputs.executable_statuses.len()
        );
    }

    if blockers.is_empty() {
        // All gates green: the digest of the exact grant a live run would freeze.
        let (ruleset, machine) = ruleset_and_machine.expect("no blockers implies policy compiled");
        let inputs = inputs
            .as_ref()
            .expect("no blockers implies inputs resolved");
        let bin_path = bin_path.expect("no blockers implies harness resolved");
        let grant = freeze_grant(ctx, base, args, inputs, ruleset, &machine, &bin_path)?;
        let digest = grant.digest(&key.expect("no blockers implies key loaded"))?;
        println!("    digest: {digest}");
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
}
