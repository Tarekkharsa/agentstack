//! Panel edit actions (launch plan Lane B1/B2): the digest-bound, fixed CLI
//! verbs t3code drives to load/add skills & servers, create toolsets, and
//! activate them — plus the library-index read that feeds the browser.
//!
//! The panel bridge is CLI-argv, not MCP: these are new fixed actions in the
//! closed set (pinned by `crates/cli/tests/t3code_parity.rs`), never MCP tools
//! wired into a browser. Reads carry the versioned `ui_contract::envelope`.
//!
//! Every MUTATION follows the house pipeline:
//!   mutate manifest → re-lock → re-render → digest-bound consent.
//! The manifest edit goes through the SAME single-authority JSON builders the
//! MCP tools use (`add::add_skill_json`, `add::add_server_json`,
//! `add::add_profile_json`, `add::add_to_profile`) — no second mutation path.
//! Re-lock + re-render is the one activation path (`use_profile::run --write`).
//! Consent is bound exactly like `apply-setup`: a `--preview` read returns a
//! `consent_digest` over the intended change AND the current manifest bytes; the
//! apply recomputes it and refuses on any drift before writing a byte.
//! Activation fails closed on an unresolved `${REF}` secret — a feature, not a
//! bug: the manifest keeps the `${REF}` (never a value) and the render is
//! blocked until the human sets the secret.
//!
//! Two verbs sit outside that pipeline on purpose:
//!
//! - `remove-from-library` edits the MACHINE-WIDE central library, not this
//!   project, so it re-locks and re-renders nothing and its consent digest binds
//!   `library.toml` bytes instead of manifest bytes. It is still previewed,
//!   digest-bound, and — because the entry moves to `lib/.trash` rather than
//!   being deleted — recoverable.
//! - `create-profile` stops after `mutate manifest → re-lock` (review finding
//!   H3). Naming a toolset is not switching to it, so nothing renders and no
//!   `${REF}` is resolved; activation is `use-profile` or `session start`.
//!   Panels gate on `toolset-create-v2` for this.

use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

use crate::cli::{
    PanelAddServerArgs, PanelAddSkillArgs, PanelCreateProfileArgs, PanelLibraryKind,
    PanelRemoveFromLibraryArgs, PanelUseProfileArgs,
};
use crate::manifest::Manifest;

/// Domain separator for the profiles-edit consent digest. Distinct from the
/// `init-plan` and `trust` digest domains — these are three independent schemes
/// by design (each hashes exactly what its own preview displays).
const DIGEST_DOMAIN: &[u8] = b"agentstack:profiles-edit:v1\n";

/// SHA-256 over `(action, params, current-manifest-bytes)`, each segment
/// length-framed so distinct triples cannot collide by concatenation. Binding
/// the manifest bytes means any drift between preview and apply — a concurrent
/// edit, a different pending change — flips the digest and the apply refuses.
fn action_digest(action: &str, params: &Value, manifest_bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(DIGEST_DOMAIN);
    // `seg` is preceded by its little-endian u64 length so `("ab","c")` and
    // `("a","bc")` hash differently.
    let mut frame = |seg: &[u8]| {
        h.update((seg.len() as u64).to_le_bytes());
        h.update(seg);
    };
    frame(action.as_bytes());
    frame(params.to_string().as_bytes());
    frame(manifest_bytes);
    format!("sha256:{:x}", h.finalize())
}

/// The manifest bytes the digest binds to, and that the mutation builders will
/// edit. Errors (fail closed) when the project has no manifest — these actions
/// only extend an initialized project.
fn manifest_bytes(dir: Option<&Path>) -> Result<Vec<u8>> {
    let base = crate::commands::project_base(dir)?;
    let mdir = crate::manifest::resolve_manifest_dir(&base);
    let path = mdir.join(crate::manifest::load::MANIFEST_FILE);
    std::fs::read(&path)
        .with_context(|| format!("no manifest at {} (run `agentstack init`)", path.display()))
}

/// Parse the current manifest for existence checks (profiles, shadowing).
fn load_manifest(dir: Option<&Path>) -> Result<Manifest> {
    let bytes = manifest_bytes(dir)?;
    let text = String::from_utf8(bytes).context("manifest is not valid UTF-8")?;
    toml::from_str(&text).context("parsing manifest")
}

/// Build the enveloped preview Value: the intended change plus the
/// `consent_digest` the panel echoes back on apply. `body` carries the
/// per-action detail. Returning the `Value` (rather than printing it straight
/// away) lets the apply path reuse the exact digest the preview computed, and
/// lets callers/tests read it without parsing stdout — the same role
/// `init::plan_json` plays for setup.
fn build_preview(action: &str, digest: &str, mut body: Map<String, Value>) -> Value {
    body.insert("action".into(), action.into());
    body.insert("consent_digest".into(), digest.to_string().into());
    // `create-profile` is the one action here that no longer renders (it names a
    // toolset; activating it is a separate verb), so it cannot carry the shared
    // note's render/`${REF}` clauses — they would be false. Every other action
    // still re-locks and re-renders.
    let note = if action == "create-profile" {
        format!(
            "Review, then apply with --yes --consented {digest}. Applying writes the \
             manifest entry and re-locks — nothing is rendered, so no ${{REF}} secret is \
             resolved. Activate it separately with `use-profile` or `session start`."
        )
    } else {
        format!(
            "Review, then apply with --yes --consented {digest}. Applying re-locks and \
             re-renders the toolset; an unresolved ${{REF}} secret blocks the render \
             (set it with `agentstack secret set`, then re-apply)."
        )
    };
    body.insert("note".into(), note.into());
    crate::ui_contract::envelope(Value::Object(body))
}

/// Print an enveloped Value as pretty JSON — the preview branch of every action.
fn emit(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Pull the `consent_digest` back out of a built preview so the apply path binds
/// to the exact digest the preview showed.
fn preview_digest(preview: &Value) -> Result<&str> {
    preview["consent_digest"]
        .as_str()
        .context("preview is missing consent_digest — internal error")
}

/// The non-interactive consent contract, identical in spirit to
/// `apply-setup`/`trust-consent`: applying requires a `--consented <digest>`
/// that matches the freshly recomputed digest. Refuses before any write.
fn verify_consent(consented: Option<&str>, actual: &str) -> Result<()> {
    let consented = consented.context(
        "refusing to apply without --consented <digest> — run --preview first, review, \
         then pass the digest it printed",
    )?;
    anyhow::ensure!(
        consented == actual,
        "consent digest mismatch: the manifest or the requested change moved since the \
         preview — re-preview and confirm again",
    );
    Ok(())
}

/// Re-lock + re-render: activate `profile` through the one activation path.
/// `write = true` so this materializes skills and renders server configs, and
/// fails closed (nonzero) when a `${REF}` did not resolve unless
/// `allow_unresolved`.
fn activate(profile: &str, allow_unresolved: bool, dir: Option<&Path>) -> Result<()> {
    let args = crate::cli::UseArgs {
        profile: Some(profile.to_string()),
        targets: vec![],
        scope: None,
        write: true,
        allow_unresolved,
        prune_foreign: false,
        no_gitignore: false,
        list: false,
        json: false,
    };
    crate::commands::use_profile::run(&args, dir)
}

/// Require `profile` to already exist. Toolsets are created with `create-profile`;
/// `add-*-to-profile` extends an existing one (crisp verb contract, and it keeps
/// the panel's "add to toolset" vs "new toolset" affordances distinct).
fn ensure_profile_exists(manifest: &Manifest, profile: &str) -> Result<()> {
    anyhow::ensure!(
        manifest.profiles.contains_key(profile),
        "no toolset '{profile}' — create it first (create-profile)",
    );
    Ok(())
}

/// Append `name` to `profiles.<profile>.<field>` and write the manifest. Used by
/// the enroll-existing branch (a library or already-inline capability referenced
/// by bare name — no new definition table).
fn enroll(dir: Option<&Path>, profile: &str, field: &str, name: &str) -> Result<()> {
    let base = crate::commands::project_base(dir)?;
    let mdir = crate::manifest::resolve_manifest_dir(&base);
    let path = mdir.join(crate::manifest::load::MANIFEST_FILE);
    let original =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let new_text = crate::commands::add::add_to_profile(&original, profile, field, name)?;
    crate::util::atomic::write(&path, &new_text)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

// ── add-skill-to-profile ────────────────────────────────────────────────────

/// `add-skill-to-profile` — add a skill to the manifest and enroll it in a
/// toolset (new git/path source), OR enroll an existing library/inline skill by
/// name (no source), then activate the toolset. Preview by default; apply with
/// `--yes --consented`.
pub fn add_skill(args: &PanelAddSkillArgs, dir: Option<&Path>) -> Result<()> {
    // The preview both validates every precondition and computes the digest;
    // running it on apply too re-validates (the CLI re-checks on every call) and
    // yields the exact digest to bind consent against.
    let preview = add_skill_preview(args, dir)?;
    if !args.consent.yes {
        return emit(&preview);
    }
    verify_consent(args.consent.consented.as_deref(), preview_digest(&preview)?)?;

    // The write decision is pure over the flags (`--git`/`--path` defines a new
    // skill; neither enrolls an existing one), so recomputing it here can never
    // disagree with the preview's `creates_manifest_entry`.
    if args.git.is_some() || args.path.is_some() {
        let source = if args.path.is_some() { "path" } else { "git" };
        // New skill: ONE atomic write adds `[skills.<name>]` AND the toolset
        // enrollment, through the shared MCP builder (`profile` field).
        let json = json!({
            "name": args.name,
            "source": source,
            "git": args.git,
            "rev": args.rev,
            "subpath": args.subpath,
            "path": args.path,
            "profile": args.profile,
        });
        crate::commands::add::add_skill_json(dir, &json)?;
    } else {
        enroll(dir, &args.profile, "skills", &args.name)?;
    }
    activate(&args.profile, args.consent.allow_unresolved, dir)
}

/// Validate the request and build its enveloped preview (with the
/// `consent_digest`). Fails closed before any write on a missing toolset, a bad
/// flag combination, or a name that resolves nowhere. Public so the panel (and
/// the parity witness) can read the digest without parsing stdout.
pub fn add_skill_preview(args: &PanelAddSkillArgs, dir: Option<&Path>) -> Result<Value> {
    let manifest = load_manifest(dir)?;
    ensure_profile_exists(&manifest, &args.profile)?;

    anyhow::ensure!(
        !(args.git.is_some() && args.path.is_some()),
        "pass only one of --git or --path",
    );
    // A `--git` or `--path` means "define a new skill"; neither means "enroll an
    // existing library/inline skill by name".
    let new_source = args.git.is_some() || args.path.is_some();
    let source = if args.path.is_some() {
        "path"
    } else if args.git.is_some() {
        "git"
    } else {
        "existing"
    };
    if !new_source {
        // Enroll-existing: the name must resolve (inline or library) so we never
        // write a dangling toolset reference.
        let libctx = crate::commands::load(dir)?.library_ctx();
        anyhow::ensure!(
            manifest.skills.contains_key(&args.name) || libctx.library.get(&args.name).is_some(),
            "no skill '{}' in the manifest or central library — add it with --git/--path, \
             or pick a library name",
            args.name,
        );
    } else {
        anyhow::ensure!(
            !manifest.skills.contains_key(&args.name),
            "skill '{}' already exists in the manifest — enroll it by name (omit --git/--path)",
            args.name,
        );
    }

    let params = json!({
        "profile": args.profile,
        "name": args.name,
        "source": source,
        "git": args.git,
        "rev": args.rev,
        "subpath": args.subpath,
        "path": args.path,
    });
    let digest = action_digest("add-skill-to-profile", &params, &manifest_bytes(dir)?);

    let mut body = Map::new();
    body.insert("profile".into(), args.profile.clone().into());
    body.insert(
        "skill".into(),
        json!({
            "name": args.name,
            "source": source,
            "creates_manifest_entry": new_source,
            "git": args.git,
            "rev": args.rev,
            "subpath": args.subpath,
            "path": args.path,
        }),
    );
    Ok(build_preview("add-skill-to-profile", &digest, body))
}

// ── add-server-to-profile ───────────────────────────────────────────────────

/// `add-server-to-profile` — add a server to the manifest and enroll it in a
/// toolset (new server definition), OR enroll an existing library/inline server
/// by name, then activate the toolset.
pub fn add_server(args: &PanelAddServerArgs, dir: Option<&Path>) -> Result<()> {
    let preview = add_server_preview(args, dir)?;
    if !args.consent.yes {
        return emit(&preview);
    }
    verify_consent(args.consent.consented.as_deref(), preview_digest(&preview)?)?;

    if server_defines_new(args) {
        // Header/env `Key=Value` pairs become JSON objects; `${REF}`s pass
        // through untouched (never resolved here — secrets never serialize).
        let headers = kv_to_object(&args.headers)?;
        let env = kv_to_object(&args.env)?;
        let json = json!({
            "name": args.name,
            "transport": args.transport,
            "url": args.url,
            "command": args.command,
            "args": args.args,
            "cwd": args.cwd,
            "headers": headers,
            "env": env,
            "profile": args.profile,
        });
        crate::commands::add::add_server_json(dir, &json)?;
    } else {
        enroll(dir, &args.profile, "servers", &args.name)?;
    }
    activate(&args.profile, args.consent.allow_unresolved, dir)
}

/// A server definition is present iff any wire field was given; otherwise the
/// action enrolls an existing library/inline server by name. Pure over the
/// flags so the apply and the preview never disagree about the branch.
fn server_defines_new(args: &PanelAddServerArgs) -> bool {
    args.url.is_some()
        || args.command.is_some()
        || !args.args.is_empty()
        || !args.headers.is_empty()
        || !args.env.is_empty()
        || args.cwd.is_some()
}

/// Validate the request and build its enveloped preview (with the
/// `consent_digest`). Public so the panel (and the parity witness) can read the
/// digest without parsing stdout.
pub fn add_server_preview(args: &PanelAddServerArgs, dir: Option<&Path>) -> Result<Value> {
    let manifest = load_manifest(dir)?;
    ensure_profile_exists(&manifest, &args.profile)?;

    let new_source = server_defines_new(args);
    if !new_source {
        let libctx = crate::commands::load(dir)?.library_ctx();
        anyhow::ensure!(
            manifest.servers.contains_key(&args.name)
                || libctx.library.get_server(&args.name).is_some(),
            "no server '{}' in the manifest or central library — define it with \
             --type/--url/--command, or pick a library name",
            args.name,
        );
    } else {
        anyhow::ensure!(
            !manifest.servers.contains_key(&args.name),
            "server '{}' already exists in the manifest — enroll it by name (omit the \
             definition flags)",
            args.name,
        );
    }

    // The server-definition JSON shape `add_server_json` consumes. Header/env
    // `Key=Value` pairs become JSON objects; `${REF}`s pass through untouched
    // (never resolved here — secrets never serialize into the manifest).
    let headers = kv_to_object(&args.headers)?;
    let env = kv_to_object(&args.env)?;
    let params = json!({
        "profile": args.profile,
        "name": args.name,
        "transport": args.transport,
        "url": args.url,
        "command": args.command,
        "args": args.args,
        "cwd": args.cwd,
        "headers": headers,
        "env": env,
    });
    let digest = action_digest("add-server-to-profile", &params, &manifest_bytes(dir)?);

    let mut body = Map::new();
    body.insert("profile".into(), args.profile.clone().into());
    body.insert(
        "server".into(),
        json!({
            "name": args.name,
            "creates_manifest_entry": new_source,
            "transport": args.transport,
            "url": args.url,
            "command": args.command,
            "args": args.args,
        }),
    );
    Ok(build_preview("add-server-to-profile", &digest, body))
}

/// `--header`/`--env` `Key=Value` pairs → a JSON object of string values. The
/// value may contain `${REF}` — it is preserved verbatim, never resolved here.
fn kv_to_object(pairs: &[String]) -> Result<Value> {
    let mut map = Map::new();
    for p in pairs {
        let (k, v) = p
            .split_once('=')
            .with_context(|| format!("expected Key=Value, got '{p}'"))?;
        map.insert(k.trim().to_string(), Value::String(v.to_string()));
    }
    Ok(Value::Object(map))
}

// ── create-profile ──────────────────────────────────────────────────────────

/// `create-profile` — create a new toolset from existing/library skills and
/// servers. Naming is not activating: this writes the manifest entry and
/// re-locks, and nothing is rendered. Preview by default; apply with
/// `--yes --consented`.
pub fn create_profile(args: &PanelCreateProfileArgs, dir: Option<&Path>) -> Result<()> {
    create_profile_gated(args, dir, std::io::stdin().is_terminal())
}

/// The create path with the TTY probe injected, so the consent gate is testable
/// without a real terminal. `interactive` is whether stdin is a TTY;
/// production passes `std::io::stdin().is_terminal()`.
///
/// Three callers, three shapes, one authority path:
///
/// - **The panel** (headless, `--preview` then `--yes --consented <digest>`) —
///   the digest it echoes back must match a freshly recomputed one or nothing
///   is written.
/// - **Any other headless caller** (a pipe, a script, an agent) — `--preview`
///   is the JSON envelope, and applying still needs the digest. A *bare*
///   headless call refuses with a sentence naming the flag it needs (review
///   finding H3): the two-step digest contract is right for machines and wrong
///   as the thing a person sees when they pipe the command somewhere.
/// - **A human at a terminal** — new. Creating a toolset was previously only
///   reachable by hand-editing TOML or by performing the panel's two-step
///   digest dance, which left the product's own retention feature with no
///   usable on-ramp (review finding F05). Typing the command at a terminal is
///   the consent, exactly as `agentstack trust` treats it, with the same
///   honest limit: `isatty` proves stdin is a terminal device, not that a
///   human is attending it. The enforceable property is unchanged — headless
///   callers still cannot create without presenting the reviewed digest.
///
/// The interactive path is strictly *stronger* than a bare `--yes` would be:
/// the digest is recomputed after the human answers and compared against the
/// one they were shown, so a manifest edited during the prompt refuses instead
/// of silently creating against bytes nobody reviewed.
fn create_profile_gated(
    args: &PanelCreateProfileArgs,
    dir: Option<&Path>,
    interactive: bool,
) -> Result<()> {
    let preview = create_profile_preview(args, dir)?;
    if !args.consent.yes {
        if args.consent.preview {
            return emit(&preview);
        }
        if !interactive {
            // The envelope is a machine contract, so it is behind the flag a
            // machine passes. A bare headless call gets prose, not JSON.
            anyhow::bail!(
                "nothing was created — this is not a terminal, so there is no one to ask.\n\
                 Run it at a terminal, or pass --preview to get the plan and its consent \
                 digest, then re-run with --yes --consented <digest>."
            );
        }
        print_create_review(args, preview_digest(&preview)?);
        if !confirm("Create it?")? {
            println!("cancelled — nothing was written.");
            return Ok(());
        }
        // Re-read and re-derive: the human reviewed the digest above, so bind
        // to it rather than trusting that the manifest sat still while they
        // thought about it.
        let fresh = create_profile_preview(args, dir)?;
        verify_consent(Some(preview_digest(&preview)?), preview_digest(&fresh)?)
            .context("the manifest changed while you were reviewing")?;
    } else {
        verify_consent(args.consent.consented.as_deref(), preview_digest(&preview)?)?;
    }

    let create = json!({
        "name": args.name,
        "skills": args.skills,
        "servers": args.servers,
    });
    crate::commands::add::add_profile_json(dir, &create)?;

    // Naming a toolset is not switching to it (review finding H3). The manifest
    // entry is written and the new members are pinned — but nothing renders, so
    // no native config file moves and no `${REF}` is resolved. Re-locking goes
    // through `lock::run`, the existing lock-only path, rather than a second
    // pinning path of our own; it also carries the "trusted project, new pins,
    // trust is now stale" notice, which a fresh toolset legitimately triggers.
    crate::commands::lock::run(
        &crate::cli::LockArgs {
            profile: Some(args.name.clone()),
            ..Default::default()
        },
        dir,
    )?;
    print_created(&args.name);
    Ok(())
}

/// What a human sees after a create: what happened, and the one command that
/// makes it take effect. `restore --last` is deliberately NOT offered — create
/// writes only the manifest, and the undo ledger captures rendered config
/// files, so pointing at it here would undo somebody else's earlier write.
fn print_created(name: &str) {
    use owo_colors::OwoColorize;
    println!("\n{} toolset {} created.", "✓".green(), name.bold());
    println!(
        "  {}",
        "Nothing was rendered — your CLIs are unchanged.".dimmed()
    );
    println!("  Activate it:  agentstack session start {name}");
    println!(
        "  {}",
        format!("Undo: delete the [profiles.{name}] block from agentstack.toml").dimmed()
    );
}

/// The interactive review: the same facts the JSON preview carries, in prose.
/// Deliberately short — a toolset is a named subset, and the consequence a
/// human needs before saying yes is "which capabilities, and what happens on
/// yes", not the envelope's schema fields.
fn print_create_review(args: &PanelCreateProfileArgs, digest: &str) {
    use owo_colors::OwoColorize;
    println!("New toolset {}\n", args.name.bold());
    if !args.servers.is_empty() {
        println!("  servers  {}", args.servers.join(", "));
    }
    if !args.skills.is_empty() {
        println!("  skills   {}", args.skills.join(", "));
    }
    println!(
        "\n  {}",
        "Creating it writes the manifest entry and re-locks. Nothing is rendered —".dimmed()
    );
    println!(
        "  {}",
        format!(
            "activate it afterwards with `agentstack session start {}`.",
            args.name
        )
        .dimmed()
    );
    println!("  {}\n", format!("consent digest: {digest}").dimmed());
}

/// A y/N prompt on stdin. Only ever reached on the interactive path, where
/// stdin is a terminal; anything that is not an explicit yes cancels.
fn confirm(question: &str) -> Result<bool> {
    use std::io::Write;
    print!("{question} [y/N] ");
    std::io::stdout().flush().ok();
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("could not read the answer from stdin")?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes"))
}

/// Validate the request and build its enveloped preview (with the
/// `consent_digest`). Public so the panel (and the parity witness) can read the
/// digest without parsing stdout.
pub fn create_profile_preview(args: &PanelCreateProfileArgs, dir: Option<&Path>) -> Result<Value> {
    let manifest = load_manifest(dir)?;
    anyhow::ensure!(
        !manifest.profiles.contains_key(&args.name),
        "toolset '{}' already exists",
        args.name,
    );
    anyhow::ensure!(
        !args.skills.is_empty() || !args.servers.is_empty(),
        "pick at least one skill or server for the toolset",
    );

    // Members are bare names; validate each resolves (inline or library) before
    // creating so activation never trips on a dangling reference. `"*"` is the
    // legal inline-all-skills wildcard.
    let libctx = crate::commands::load(dir)?.library_ctx();
    for s in &args.skills {
        anyhow::ensure!(
            s == "*" || manifest.skills.contains_key(s) || libctx.library.get(s).is_some(),
            "no skill '{s}' in the manifest or central library",
        );
    }
    for s in &args.servers {
        anyhow::ensure!(
            manifest.servers.contains_key(s) || libctx.library.get_server(s).is_some(),
            "no server '{s}' in the manifest or central library",
        );
    }

    let params = json!({
        "name": args.name,
        "skills": args.skills,
        "servers": args.servers,
    });
    let digest = action_digest("create-profile", &params, &manifest_bytes(dir)?);

    let mut body = Map::new();
    body.insert("profile".into(), args.name.clone().into());
    body.insert("skills".into(), json!(args.skills));
    body.insert("servers".into(), json!(args.servers));
    Ok(build_preview("create-profile", &digest, body))
}

// ── use-profile ─────────────────────────────────────────────────────────────

/// `use-profile` — activate an existing toolset (re-lock + re-render) with no
/// manifest change. The digest still binds the manifest bytes, so if the
/// toolset's contents changed since the preview the apply refuses.
pub fn use_profile(args: &PanelUseProfileArgs, dir: Option<&Path>) -> Result<()> {
    let preview = use_profile_preview(args, dir)?;
    if !args.consent.yes {
        return emit(&preview);
    }
    verify_consent(args.consent.consented.as_deref(), preview_digest(&preview)?)?;
    activate(&args.profile, args.consent.allow_unresolved, dir)
}

/// Validate the request and build its enveloped preview (with the
/// `consent_digest`). Public so the panel (and the parity witness) can read the
/// digest without parsing stdout.
pub fn use_profile_preview(args: &PanelUseProfileArgs, dir: Option<&Path>) -> Result<Value> {
    let manifest = load_manifest(dir)?;
    ensure_profile_exists(&manifest, &args.profile)?;

    let params = json!({ "profile": args.profile });
    let digest = action_digest("use-profile", &params, &manifest_bytes(dir)?);

    let mut body = Map::new();
    body.insert("profile".into(), args.profile.clone().into());
    Ok(build_preview("use-profile", &digest, body))
}

// ── library-index read (B2) ─────────────────────────────────────────────────

/// `library-index` — the enveloped catalog the panel's library browser reads:
/// the central library's skills and servers (name + best-effort description +
/// origin), each flagged if the current manifest already carries it, plus the
/// existing toolset names to add into. A READ: nothing resolves, renders, or
/// executes; no secret is touched. It reuses `agentstack_list_loadable`'s
/// underlying data — `Library::load_default` + the manifest — behind a fixed
/// argv read instead of the MCP tool.
///
/// It reads only the user's own central-library descriptions (not repo bundle
/// content) and manifest capability *names*, so it is safe regardless of project
/// trust; adding and activating are separately digest-gated and fail closed.
pub fn library_index(dir: Option<&Path>) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&library_index_value(dir)?)?
    );
    Ok(())
}

/// The enveloped `library-index` body as a `Value` — the Rust-callable primitive
/// (like `doctor::collect`) other callers and tests use without shelling out.
pub fn library_index_value(dir: Option<&Path>) -> Result<Value> {
    let library = crate::library::Library::load_default_or_warn();
    let lib_home = crate::util::paths::lib_home();
    // A manifest is optional here: a fresh project can still browse the library.
    let manifest = load_manifest(dir).ok();

    let mut skills: Vec<Value> = Vec::new();
    for entry in &library.skills {
        let in_manifest = manifest
            .as_ref()
            .is_some_and(|m| m.skills.contains_key(&entry.name));
        skills.push(json!({
            "name": entry.name,
            "description": entry.description(&lib_home),
            "origin": "library",
            "in_manifest": in_manifest,
        }));
    }
    // Inline manifest skills the library doesn't carry — so the browser shows
    // project-local capabilities too. Names only (inline SKILL.md bodies are
    // project content); the panel can `explain` for detail.
    if let Some(m) = &manifest {
        for name in m.skills.keys() {
            if library.get(name).is_none() {
                skills.push(json!({
                    "name": name,
                    "description": Value::Null,
                    "origin": "manifest",
                    "in_manifest": true,
                }));
            }
        }
    }

    let mut servers: Vec<Value> = Vec::new();
    for entry in &library.servers {
        let in_manifest = manifest
            .as_ref()
            .is_some_and(|m| m.servers.contains_key(&entry.name));
        servers.push(json!({
            "name": entry.name,
            "provenance": entry.provenance,
            "origin": "library",
            "in_manifest": in_manifest,
        }));
    }
    if let Some(m) = &manifest {
        for name in m.servers.keys() {
            if library.get_server(name).is_none() {
                servers.push(json!({
                    "name": name,
                    "provenance": Value::Null,
                    "origin": "manifest",
                    "in_manifest": true,
                }));
            }
        }
    }

    let profiles: Vec<&String> = manifest
        .as_ref()
        .map(|m| m.profiles.keys().collect())
        .unwrap_or_default();

    let body = json!({
        "skills": skills,
        "servers": servers,
        "profiles": profiles,
    });
    Ok(crate::ui_contract::envelope(body))
}

// ── remove-from-library ─────────────────────────────────────────────────────

/// `remove-from-library` — drop a skill or server from the CENTRAL library
/// (machine-wide), moving it to `lib/.trash` so the removal is recoverable.
///
/// This is the odd one out among the panel mutations, deliberately:
///  - It edits machine state (`~/.agentstack/lib`), not the project manifest,
///    so it does NOT re-lock or re-render. A project that references the name
///    keeps its manifest entry and its pinned lock row untouched; the next
///    `use`/`doctor` is where an unresolvable name surfaces — which is why the
///    preview says out loud whether this project depends on it.
///  - Its consent digest therefore binds `library.toml` bytes (the state that
///    actually changes), not manifest bytes, so a concurrent library edit
///    between preview and apply invalidates consent exactly the way a
///    concurrent manifest edit does for the other verbs.
pub fn remove_from_library(args: &PanelRemoveFromLibraryArgs, dir: Option<&Path>) -> Result<()> {
    let preview = remove_from_library_preview(args, dir)?;
    if !args.consent.yes {
        return emit(&preview);
    }
    verify_consent(args.consent.consented.as_deref(), preview_digest(&preview)?)?;

    let lib_home = crate::util::paths::lib_home();
    let trashed_to = match args.kind {
        PanelLibraryKind::Skill => {
            crate::commands::lib::remove_skill(&lib_home, &args.name, true)?.trashed_to
        }
        PanelLibraryKind::Server => {
            crate::commands::lib::remove_server(&lib_home, &args.name, true)?.trashed_to
        }
    };
    let id = trashed_to
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    println!(
        "removed {} '{}' from the central library — restore with `agentstack lib trash --restore {} --write`",
        args.kind.as_str(),
        args.name,
        id,
    );
    Ok(())
}

/// Validate the request and build its enveloped preview. Fails closed when the
/// name is not in the library. The body names what moves to the trash, whether
/// the current project still references the name, and which of its toolsets do —
/// the panel needs all three to write an honest confirmation card.
pub fn remove_from_library_preview(
    args: &PanelRemoveFromLibraryArgs,
    dir: Option<&Path>,
) -> Result<Value> {
    let lib_home = crate::util::paths::lib_home();
    let kind = args.kind.as_str();

    // Preview the removal through the same function the apply calls, with
    // `write = false`: nothing is mutated, and a missing name errors here —
    // before a digest exists to consent to.
    let (source, body, trash_dest) = match args.kind {
        PanelLibraryKind::Skill => {
            let outcome = crate::commands::lib::remove_skill(&lib_home, &args.name, false)?;
            (
                outcome.source_kind,
                outcome.removed_dir.map(|p| p.display().to_string()),
                outcome.trashed_to,
            )
        }
        PanelLibraryKind::Server => {
            let outcome = crate::commands::lib::remove_server(&lib_home, &args.name, false)?;
            (
                "file".to_string(),
                outcome.removed_file.map(|p| p.display().to_string()),
                outcome.trashed_to,
            )
        }
    };

    // Is this project relying on the name? The manifest is optional — the
    // library browser works without one — and a project that defines the
    // capability inline does not depend on the library copy at all.
    let manifest = load_manifest(dir).ok();
    let (in_manifest, inline, profiles) = match &manifest {
        None => (false, false, Vec::new()),
        Some(m) => {
            let (declared, field): (bool, fn(&crate::manifest::Profile) -> &Vec<String>) =
                match args.kind {
                    PanelLibraryKind::Skill => (m.skills.contains_key(&args.name), |p| &p.skills),
                    PanelLibraryKind::Server => {
                        (m.servers.contains_key(&args.name), |p| &p.servers)
                    }
                };
            let profiles: Vec<String> = m
                .profiles
                .iter()
                .filter(|(_, p)| field(p).iter().any(|n| n == &args.name))
                .map(|(name, _)| name.clone())
                .collect();
            (declared || !profiles.is_empty(), declared, profiles)
        }
    };

    let params = json!({ "kind": kind, "name": args.name });
    // The library index — not the manifest — is the state this action changes,
    // so that is what consent binds to.
    let library_bytes = std::fs::read(crate::library::Library::path(&lib_home)).unwrap_or_default();
    let digest = action_digest("remove-from-library", &params, &library_bytes);

    let trash_id = trash_dest
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut body_map = Map::new();
    body_map.insert(
        "removal".into(),
        json!({
            "kind": kind,
            "name": args.name,
            "source": source,
            // What leaves its place on disk (a git-backed skill has no local
            // body — the shared store cache is never touched by a removal).
            "body": body,
            "trash_id": trash_id,
            "trash_path": trash_dest.display().to_string(),
            "restore_command": format!("agentstack lib trash --restore {trash_id} --write"),
            // Machine-wide: every project referencing this name is affected,
            // not just the one the panel happens to be open on.
            "scope": "machine",
            // This project's stake in it, so the card can warn instead of
            // pretending a shared library is project-local.
            "used_by_this_project": in_manifest,
            "defined_inline_here": inline,
            "profiles": profiles,
        }),
    );
    Ok(build_remove_preview(
        "remove-from-library",
        &digest,
        body_map,
    ))
}

/// Like [`build_preview`], but with the note this action actually needs:
/// removal is machine-wide, recoverable, and re-renders nothing.
fn build_remove_preview(action: &str, digest: &str, mut body: Map<String, Value>) -> Value {
    body.insert("action".into(), action.into());
    body.insert("consent_digest".into(), digest.to_string().into());
    body.insert(
        "note".into(),
        format!(
            "Review, then apply with --yes --consented {digest}. This removes the capability \
             from the machine-wide central library — every project that references it by name \
             is affected. No manifest, lockfile, or rendered config is touched. The entry moves \
             to the library trash and can be restored."
        )
        .into(),
    );
    crate::ui_contract::envelope(Value::Object(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    /// The consent digest is deterministic and binds all three of its inputs:
    /// change the action, the params, or the manifest bytes and it moves. This
    /// is what lets the apply refuse a preview taken against different state.
    #[test]
    fn digest_is_deterministic_and_binds_every_input() {
        let params = json!({ "profile": "web", "name": "pdf" });
        let base = action_digest("add-skill-to-profile", &params, b"version = 1\n");

        assert_eq!(
            base,
            action_digest("add-skill-to-profile", &params, b"version = 1\n"),
            "same inputs → same digest"
        );
        assert_ne!(
            base,
            action_digest("use-profile", &params, b"version = 1\n"),
            "action is bound"
        );
        assert_ne!(
            base,
            action_digest(
                "add-skill-to-profile",
                &json!({ "profile": "web", "name": "docx" }),
                b"version = 1\n"
            ),
            "params are bound"
        );
        assert_ne!(
            base,
            action_digest("add-skill-to-profile", &params, b"version = 1\n# edited\n"),
            "manifest bytes are bound"
        );
        assert!(base.starts_with("sha256:"));
    }

    /// The apply gate refuses a missing digest and a mismatched one, and only
    /// accepts an exact match — the same non-interactive contract as
    /// `apply-setup`/`trust-consent`.
    #[test]
    fn consent_gate_refuses_missing_and_mismatch() {
        assert!(verify_consent(None, "sha256:aa").is_err());
        assert!(verify_consent(Some("sha256:bb"), "sha256:aa").is_err());
        assert!(verify_consent(Some("sha256:aa"), "sha256:aa").is_ok());
    }

    /// F05 + H3: making `create-profile` usable by a human must not open a hole
    /// for anything else. The *authority* contract is unchanged — no caller
    /// writes without a matching digest — while the *shape* of a bare headless
    /// call moved behind `--preview` (H3): the JSON envelope is a machine
    /// contract and now requires the flag a machine passes, so a person who
    /// pipes the command gets a sentence instead of a digest dump.
    #[test]
    fn create_profile_headless_contract_is_unchanged() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", tmp.path().join("home"));
        std::fs::write(
            tmp.path().join("agentstack.toml"),
            "version = 1\n\n[servers.github]\ntype = \"stdio\"\ncommand = \"npx\"\n",
        )
        .unwrap();

        let args = |preview: bool, yes: bool, consented: Option<&str>| PanelCreateProfileArgs {
            name: "backend".into(),
            skills: vec![],
            servers: vec!["github".into()],
            consent: crate::cli::PanelConsent {
                preview,
                yes,
                consented: consented.map(str::to_string),
                allow_unresolved: false,
            },
        };
        let wrote = || {
            let text = std::fs::read_to_string(tmp.path().join("agentstack.toml")).unwrap();
            text.contains("[profiles.backend]")
        };

        // H3: headless and bare — no envelope, no write, and the refusal names
        // the flag a machine caller is missing rather than printing one.
        let err = create_profile_gated(&args(false, false, None), Some(tmp.path()), false)
            .expect_err("a bare headless call must refuse rather than emit the envelope");
        assert!(
            err.to_string().contains("--preview"),
            "the refusal names the flag it needs: {err}"
        );
        assert!(!wrote(), "a refused headless call must not write");

        // `--preview` forces the same read shape at a terminal, so a graphical
        // client driving a PTY never trips the prompt.
        create_profile_gated(&args(true, false, None), Some(tmp.path()), true).unwrap();
        assert!(!wrote(), "--preview must not write, terminal or not");

        // Applying still needs the digest, terminal or not.
        for interactive in [true, false] {
            let err = create_profile_gated(&args(false, true, None), Some(tmp.path()), interactive)
                .expect_err("--yes without --consented must refuse");
            assert!(
                err.to_string().contains("--consented"),
                "the refusal names the missing flag: {err}"
            );
            assert!(!wrote());

            let err = create_profile_gated(
                &args(false, true, Some("sha256:nope")),
                Some(tmp.path()),
                interactive,
            )
            .expect_err("a mismatched digest must refuse");
            assert!(err.to_string().contains("consent digest mismatch"), "{err}");
            assert!(!wrote());
        }

        std::env::remove_var("AGENTSTACK_HOME");
    }

    /// The single-authority builder writes `[skills.<name>]` AND the toolset
    /// enrollment in ONE manifest — the path `add-skill-to-profile`'s new-skill
    /// branch relies on (the `"profile"` field added for Lane B).
    #[test]
    fn add_skill_json_with_profile_writes_entry_and_enrolls() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", tmp.path().join("home"));
        std::fs::write(
            tmp.path().join("agentstack.toml"),
            "version = 1\n\n[profiles.web]\nskills = []\n",
        )
        .unwrap();

        let args = json!({
            "name": "pdf",
            "source": "path",
            "path": "./skills/pdf",
            "profile": "web",
        });
        crate::commands::add::add_skill_json(Some(tmp.path()), &args).unwrap();

        let text = std::fs::read_to_string(tmp.path().join("agentstack.toml")).unwrap();
        let m: Manifest = toml::from_str(&text).unwrap();
        assert!(m.skills.contains_key("pdf"), "[skills.pdf] written");
        assert!(
            m.profiles["web"].skills.iter().any(|s| s == "pdf"),
            "enrolled in the toolset in the same write: {text}"
        );
        std::env::remove_var("AGENTSTACK_HOME");
    }

    /// The moved `add_server_json` still builds a server and honors the
    /// `"profile"` enrollment — the extraction preserved behavior for both the
    /// MCP tool and the new panel action.
    #[test]
    fn add_server_json_after_move_builds_and_enrolls() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", tmp.path().join("home"));
        std::fs::write(
            tmp.path().join("agentstack.toml"),
            "version = 1\n\n[profiles.web]\nservers = []\n",
        )
        .unwrap();

        let args = json!({
            "name": "search",
            "transport": "stdio",
            "command": "npx",
            "args": ["search-mcp"],
            "profile": "web",
        });
        crate::commands::add::add_server_json(Some(tmp.path()), &args).unwrap();

        let text = std::fs::read_to_string(tmp.path().join("agentstack.toml")).unwrap();
        let m: Manifest = toml::from_str(&text).unwrap();
        assert_eq!(m.servers["search"].command.as_deref(), Some("npx"));
        assert!(
            m.profiles["web"].servers.iter().any(|s| s == "search"),
            "enrolled in the toolset: {text}"
        );
        std::env::remove_var("AGENTSTACK_HOME");
    }

    /// The library-index read lists the central library's skills + servers,
    /// flags what the manifest already carries, surfaces inline skills as
    /// `origin: manifest`, lists the toolsets to add into, and rides the
    /// versioned envelope carrying the new feature name.
    #[test]
    fn library_index_lists_library_manifest_and_profiles() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());

        home.child("lib/skills/sql-review/SKILL.md")
            .write_str("---\ndescription: reviews SQL\n---\n# sql\n")
            .unwrap();
        let mut lib = crate::library::Library::default();
        lib.upsert(crate::library::LibrarySkill {
            name: "sql-review".into(),
            source: "path".into(),
            path: Some("sql-review".into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: None,
        });
        lib.upsert_server(crate::library::LibraryServer {
            name: "github".into(),
            checksum: None,
            version: None,
            provenance: Some("consolidated:github".into()),
        });
        lib.save(&crate::util::paths::lib_home()).unwrap();

        let proj = assert_fs::TempDir::new().unwrap();
        proj.child(".agentstack/agentstack.toml")
            .write_str(
                "version = 1\n\n[skills.local-only]\npath = \"./skills/local\"\n\n\
                 [profiles.web]\nskills = [\"sql-review\"]\n",
            )
            .unwrap();

        let v = library_index_value(Some(proj.path())).unwrap();
        assert_eq!(v["schema_version"], crate::ui_contract::SCHEMA_VERSION);
        assert!(v["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "profiles-edit-v1"));

        let skills = v["skills"].as_array().unwrap();
        let lib_skill = skills
            .iter()
            .find(|s| s["name"] == "sql-review")
            .expect("library skill listed");
        assert_eq!(lib_skill["origin"], "library");
        assert_eq!(lib_skill["description"], "reviews SQL");
        let inline = skills
            .iter()
            .find(|s| s["name"] == "local-only")
            .expect("inline manifest skill listed");
        assert_eq!(inline["origin"], "manifest");

        let servers = v["servers"].as_array().unwrap();
        assert!(
            servers.iter().any(|s| s["name"] == "github"),
            "library server listed"
        );

        let profiles = v["profiles"].as_array().unwrap();
        assert!(profiles.iter().any(|p| p == "web"), "toolsets listed");

        std::env::remove_var("AGENTSTACK_HOME");
    }
}
