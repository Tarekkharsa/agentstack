//! Project-import writes are transactional and undoable; machine initialization
//! is a separate path described below.
//!
//! `agentstack init` — never a blank page. Detect installed CLIs, import their
//! existing MCP servers into one manifest, and lift inline secrets into
//! `${REF}`s whose values land wherever you choose (P2) — a gitignored project
//! `.env` (the default), the OS keychain, or skipped for you to provide later.
//!
//! Every file this writes — the manifest, a created/updated `.env`, and the
//! `.gitignore` line that keeps `.env` out of git — is captured in the same undo
//! ledger `restore` reads (P30). Keychain values deliberately never enter file
//! history; setup names their explicit `secret rm` recovery command.

use std::path::{Path, PathBuf};

use agentstack_core::paint::OwoColorize;
use anyhow::{Context, Result};
use indexmap::IndexMap;

use crate::adapter::{extract_servers_with_skips, extract_settings, Registry};
use crate::cli::{ConnectArgs, InitArgs, SecretStore};
use crate::discover::{lift_secrets, merge_servers, Lifted};
use crate::manifest::load::MANIFEST_FILE;
use crate::manifest::model::{Delivery, Manifest, Meta, Server, Targets};
use crate::secret::{env_file, keychain};

/// The command that registers the bridge — what `--connect` does for you, and
/// what every one of init's "not connected yet" disclosures points at. One
/// constant so those surfaces cannot drift into naming different commands.
const GATEWAY_CONNECT: &str = "agentstack x gateway connect --all --write";

/// Store lifted secret values, collecting the references whose store write
/// failed instead of aborting init or silently dropping them. The manifest
/// holds `${REF}`s either way; an unstored value simply stays unresolved and
/// every use site fails closed on it by name (rule 5) — so the honest behavior
/// is to finish init and report the gap, never abort halfway (the old
/// interactive path) or pretend it stored (a non-interactive UI path).
#[cfg(test)]
fn store_lifted(lifted: &[Lifted], mut store: impl FnMut(&str, &str) -> Result<()>) -> Vec<String> {
    let mut unstored = Vec::new();
    for l in lifted {
        if store(&l.reference, &l.value).is_err() {
            unstored.push(l.reference.clone());
        }
    }
    unstored
}

/// In-memory rollback metadata for keychain writes made before the manifest is
/// durable. Values never enter file history; they live only for this call so a
/// later file/history failure can restore the credential store exactly.
struct KeychainChange {
    name: String,
    before: Option<String>,
}

fn store_lifted_reversibly(lifted: &[Lifted]) -> (Vec<String>, Vec<KeychainChange>) {
    let mut unstored = Vec::new();
    let mut changes = Vec::new();
    for lifted_secret in lifted {
        // Do not overwrite a value we cannot snapshot: without `before`, a
        // later rollback could destroy a pre-existing credential.
        let Ok(before) = keychain::get(&lifted_secret.reference) else {
            unstored.push(lifted_secret.reference.clone());
            continue;
        };
        if keychain::set(&lifted_secret.reference, &lifted_secret.value).is_err() {
            unstored.push(lifted_secret.reference.clone());
            continue;
        }
        changes.push(KeychainChange {
            name: lifted_secret.reference.clone(),
            before,
        });
    }
    (unstored, changes)
}

fn rollback_keychain(changes: &[KeychainChange]) -> Result<()> {
    for change in changes.iter().rev() {
        match &change.before {
            Some(value) => keychain::set(&change.name, value)?,
            None => {
                keychain::delete(&change.name)?;
            }
        }
    }
    Ok(())
}

/// Decide where lifted token values go (P2). Explicit flags always win; an
/// interactive run with no flag prompts; otherwise the non-interactive default
/// is the keychain — CI and scripts must never *start* writing plaintext files
/// just because init grew a new option. `allow_prompt` is false on the dry-run
/// path (a preview must never block on a prompt).
fn resolve_secret_store(args: &InitArgs, allow_prompt: bool) -> Result<SecretStore> {
    if let Some(store) = args.secrets {
        return Ok(store);
    }
    // `--no-keychain` is the deprecated alias for `--secrets skip`.
    if args.no_keychain {
        return Ok(SecretStore::Skip);
    }
    if allow_prompt && crate::util::confirm::is_interactive() {
        return prompt_secret_store();
    }
    Ok(SecretStore::Keychain)
}

/// The P2 storage menu, shown when init lifts tokens interactively. `.env` is
/// preselected as the maintainer's decided default: it is what users already
/// know, and the guard deny-list plus the managed gitignore are what make the
/// plaintext default defensible.
///
/// The full multi-line help prints once above the selector; on a real terminal
/// the choice is an arrow-key `dialoguer::Select` (matching the wizard's mode
/// fork). A non-TTY caller falls back to the numbered stdin prompt so a piped
/// run never panics inside dialoguer — this function is only reached after the
/// caller checked `is_interactive()`, so the fallback is belt-and-suspenders.
// The credential store's user-facing NAME is platform-specific: on macOS it is
// the Keychain, elsewhere it is the desktop keyring (Secret Service/libsecret).
// Chosen with `cfg` at compile time rather than a run-time branch because a
// given binary can only ever talk to the store it was built against — there is
// no state to inspect later, so the label is a constant like any other.
// Behaviour is identical on every platform; only these display strings change.
#[cfg(target_os = "macos")]
const KEYCHAIN_LABEL: &str = "macOS keychain";
#[cfg(not(target_os = "macos"))]
const KEYCHAIN_LABEL: &str = "system keyring";

#[cfg(target_os = "macos")]
const KEYCHAIN_VIEW_HINT: &str = "them in Keychain Access, or with `agentstack secret set <NAME>`.";
#[cfg(not(target_os = "macos"))]
const KEYCHAIN_VIEW_HINT: &str =
    "them in your desktop keyring app, or with `agentstack secret set <NAME>`.";

fn prompt_secret_store() -> Result<SecretStore> {
    print_secret_store_help();
    if crate::util::confirm::is_interactive() {
        // Each item carries the terse consequence; the full help is above.
        // Owned, because the label is assembled from the platform constant.
        let keychain_item = format!(
            "{KEYCHAIN_LABEL} — migrated into the OS credential store (service `agentstack`)"
        );
        let items = [
            "Project .env  (default) — plaintext file next to the manifest, gitignored, guard-blocked",
            keychain_item.as_str(),
            "Skip / decide later — write only ${REF} placeholders; nothing runs until provided",
        ];
        let idx = dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
            .with_prompt("Where should these token values live?")
            .items(items)
            .default(0)
            .interact()?;
        Ok(secret_store_at(idx))
    } else {
        read_numbered_secret_choice()
    }
}

/// Offer this project a varlock `.env.schema` — the opt-in for the recommended
/// vault ([varlock.dev](https://varlock.dev)), which resolves names from
/// 1Password, a cloud secret manager, or device-local encryption instead of a
/// file next to the code.
///
/// Returns the captured pre-write state when the file was written, so the
/// caller folds it into init's ONE undoable transaction.
///
/// Three silent refusals, each deliberate: nothing to declare, a `.env.schema`
/// already present (never overwrite the user's own schema), and a
/// non-interactive run — `confirm` answers false without prompting there, which
/// is the CI and t3code contract, so scripted `init` writes exactly what it
/// wrote before.
///
/// **Nothing here writes a secret value.** The schema declares NAMES; values
/// stay in the vault, and a name with no value still fails closed at use time,
/// which is the whole point of `${REF}`.
fn offer_env_schema(dir: &Path, names: &[String]) -> Result<Option<crate::history::FileChange>> {
    let path = dir.join(".env.schema");
    let declared = schema_names(names);
    if declared.is_empty() || path.exists() {
        return Ok(None);
    }
    // `confirm` answers false without prompting when there is no terminal, so
    // a scripted run always ended here — after three lines of context for a
    // question nobody was asked, plus a line saying it had been skipped. The
    // recommendation still has to reach that reader (it is how a project opts
    // into a vault at all), so it becomes one line instead of disappearing.
    if !crate::util::confirm::is_interactive() {
        println!(
            "{}  varlock ({}) can resolve these names from a vault instead — drop a \
             .env.schema in this project to opt in.",
            "·".dimmed(),
            "https://varlock.dev".dimmed()
        );
        return Ok(None);
    }
    println!(
        "\nvarlock ({}) can resolve these names from 1Password, a cloud secret \
         manager, or device-local encryption — no values in this project at all. A \
         .env.schema opts in; it declares names only, so it is safe to commit.",
        "https://varlock.dev".dimmed()
    );
    if !crate::util::confirm::confirm(&format!(
        "Write .env.schema declaring {}?",
        super::count(declared.len(), "name")
    ))? {
        println!("  · skipped — drop a .env.schema in this project anytime to opt in.");
        return Ok(None);
    }
    let change = crate::history::capture(&path, ".env.schema · varlock declarations");
    crate::util::atomic::write(&path, &env_schema_body(&declared))
        .with_context(|| format!("writing {}", path.display()))?;
    println!(
        "{}  Wrote .env.schema — `agentstack doctor` reports varlock's health from here on.",
        "🔑".dimmed()
    );
    Ok(Some(change))
}

/// One imported server whose name is already taken in the target library
/// source by a DIFFERENT definition.
struct LibraryCollision {
    name: String,
    /// What the library's existing definition runs or contacts.
    existing: String,
    /// What this import would write in its place.
    incoming: String,
}

/// Split imported native server names by whether the library can store them.
///
/// Native MCP configs commonly use namespaced identifiers such as
/// `upstash/context7`, and renaming one during an import would break every
/// reference to it. The library keeps one definition file per server and now
/// derives that file name through [`crate::resolve::library_file_stem`], which
/// encodes anything a path cannot carry — so a namespaced name is stored under
/// its own name like any other.
///
/// The split survives for what encoding cannot fix: a name with no printable
/// form, or one whose encoded file name would not fit. Those definitions stay
/// inline in the project manifest, under their original names, rather than
/// failing the whole import.
fn partition_library_servers(
    servers: &IndexMap<String, Server>,
) -> (IndexMap<String, Server>, IndexMap<String, Server>) {
    let mut library = IndexMap::new();
    let mut inline = IndexMap::new();
    for (name, server) in servers {
        if super::lib::valid_lib_server_name(name).is_ok() {
            library.insert(name.clone(), server.clone());
        } else {
            inline.insert(name.clone(), server.clone());
        }
    }
    (library, inline)
}

/// AgentStack's own MCP server. It is the one import a fresh project almost
/// certainly wants, so it is what the lean answer keeps.
const SELF_SERVER: &str = "agentstack";

/// Which imported servers this project's default toolset names.
///
/// The answer is the project's, not the machine's — see the call site for why
/// the two were conflated. Returns every name unless a person chooses the lean
/// set, which keeps the historical behaviour for scripts and CI.
///
/// The question is skipped where it cannot mean anything: `--project-servers`
/// has no library to leave the rest in, a single server is not a choice, and an
/// import without AgentStack's own server has no obvious lean set to offer.
fn choose_toolset_servers(
    servers: &IndexMap<String, Server>,
    args: &InitArgs,
) -> Result<Vec<String>> {
    let all: Vec<String> = servers.keys().cloned().collect();
    if args.project_servers || servers.len() < 2 || !servers.contains_key(SELF_SERVER) {
        return Ok(all);
    }
    let lean_label = format!("just {SELF_SERVER} — add any of the others later");
    let all_label = format!(
        "all {} — this project declares the whole machine",
        all.len()
    );
    let answer = crate::util::confirm::choose(
        &format!(
            "\n{}  Which of these does this project use?\n      {}\n      {}",
            "🎯".dimmed(),
            "All of them are imported either way — this sets the default toolset only.".dimmed(),
            "What you leave out stays in your library and in each CLI's own config.".dimmed()
        ),
        &[("lean", lean_label.as_str()), ("all", all_label.as_str())],
    )?;
    // `choose` returns None for a bare Enter, an unreadable answer, and every
    // non-interactive run alike, and its contract says None must leave existing
    // behaviour untouched. Existing behaviour is the full set.
    Ok(match answer.as_deref() {
        Some("lean") => vec![SELF_SERVER.to_string()],
        _ => all,
    })
}

/// Explain the exceptional placement before the consent gate. The important
/// promise is that the imported identifier is preserved byte-for-byte.
fn render_inline_library_servers(names: &[String]) -> String {
    let names = names
        .iter()
        .map(|name| crate::text::sanitize_line(name))
        .collect::<Vec<_>>()
        .join(" · ");
    format!(
        "\n{}  Kept inline in this manifest: {names}\n\
         \x20     These native names cannot be library filenames; their names and definitions stay unchanged.\n",
        "·".dimmed()
    )
}

/// Imported servers whose name the library already holds with different bytes
/// (review finding 3).
///
/// **Identical content is not a collision.** Re-importing the same machine
/// config into a second project is the ordinary case, and asking a question
/// with no consequence is how a person learns to answer without reading.
///
/// The comparison is over the NORMALIZED definition — `toml::to_string_pretty`
/// of the `Server` table, which is exactly the byte string `lib::add_server_def`
/// writes and digests. Comparing raw file bytes instead would report key order
/// or whitespace as a difference and produce that meaningless question.
fn library_collisions(
    lib_root: &Path,
    servers: &IndexMap<String, Server>,
) -> Vec<LibraryCollision> {
    let Ok(library) = crate::library::Library::load(lib_root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, incoming) in servers {
        if library.get_server(name).is_none() {
            continue;
        }
        let path = crate::resolve::library_server_path(lib_root, name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue; // an indexed server with no readable body is not a
                      // definition this import can be said to overwrite
        };
        let Ok(existing) = toml::from_str::<Server>(&text) else {
            continue;
        };
        let (Ok(a), Ok(b)) = (
            toml::to_string_pretty(&existing),
            toml::to_string_pretty(incoming),
        ) else {
            continue;
        };
        if a == b {
            continue; // same definition — nothing is being replaced
        }
        out.push(LibraryCollision {
            name: name.clone(),
            existing: server_identity_line(&existing),
            incoming: server_identity_line(incoming),
        });
    }
    out
}

/// What a server does, in one line — the same two facts the import review and
/// the trust card lead with.
fn server_identity_line(server: &Server) -> String {
    match server.server_type {
        crate::manifest::ServerType::Stdio => {
            format!("runs {}", super::trust::server_stdio_identity(server))
        }
        crate::manifest::ServerType::Http => {
            format!("contacts {}", super::trust::server_http_identity(server))
        }
    }
}

/// The collision block, in the pre-write review: which name clashes, what the
/// library has now, and what the import would put there instead.
///
/// Pure so its wording is unit-testable. Every value is hostile input (other
/// CLIs' configs on one side, a possibly shared library folder on the other),
/// so both sides are sanitized before display.
fn render_library_collisions(source_name: &str, collisions: &[LibraryCollision]) -> String {
    let mut out = format!(
        "\n{}  {} in library source '{}' already {} a different definition:\n",
        "⚠".yellow(),
        super::count(collisions.len(), "name"),
        crate::text::sanitize_line(source_name),
        if collisions.len() == 1 {
            "holds"
        } else {
            "hold"
        }
    );
    for c in collisions {
        out.push_str(&format!(
            "      {}\n        in the library:  {}\n        this import:     {}\n",
            crate::text::sanitize_line(&c.name),
            crate::text::truncate_chars(&crate::text::sanitize_line(&c.existing), 64),
            crate::text::truncate_chars(&crate::text::sanitize_line(&c.incoming), 64),
        ));
    }
    out.push_str(
        "      The library is shared: replacing a definition makes every other project \
         that\n      pinned the old one report drift until it re-locks. Nothing is \
         replaced without\n      a yes below.\n",
    );
    out
}

/// The tool-managed block in the pre-write review: how many servers another
/// application owns, which names, who appears to own each and on what
/// evidence, and the flag that overrides the default.
///
/// The block exists because an exclusion nobody can see is a silent drop.
/// "Left alone" and "not found" are different claims, and only one of them is
/// true here — so the names are printed either way, with the wording changing
/// to say which of the two outcomes happened.
///
/// Pure so its wording is unit-testable. Every value came out of another CLI's
/// config file — hostile input — so names and paths are sanitized and bounded
/// before display.
fn render_tool_managed(entries: &[ToolManagedServer], included: bool) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let names: Vec<String> = entries
        .iter()
        .map(|t| crate::text::sanitize_line(&t.name))
        .collect();
    let verb = if entries.len() == 1 { "is" } else { "are" };
    let outcome = if included {
        "imported at your request"
    } else {
        "left alone"
    };
    let mut out = format!(
        "{}  {} {verb} managed by the apps that installed them and {} {}: {}\n",
        "⚠".yellow(),
        super::count(entries.len(), "server"),
        if entries.len() == 1 { "was" } else { "were" },
        outcome,
        names.join(", ")
    );
    for t in entries {
        out.push_str(&format!(
            "      {}  —  {}\n        {}\n",
            crate::text::sanitize_line(&t.name),
            crate::text::truncate_chars(&crate::text::sanitize_line(&t.application), 48),
            // Bounded, but generously: the path IS the evidence, and evidence
            // clipped mid-bundle cannot be checked against the config it came
            // from. The plan JSON carries it in full either way.
            crate::text::truncate_chars(&crate::text::sanitize_line(&t.evidence), 160),
        ));
    }
    out.push_str(&format!("      The rule: {TOOL_MANAGED_REASON}.\n"));
    if included {
        out.push_str(
            "      That application rewrites the entry on its own schedule, so expect the \
             pinned\n      bytes to change and re-gate on something you did not choose.\n",
        );
    } else {
        out.push_str(
            "      They stay in each CLI's own config, unchanged — nothing was deleted.\n\
             \x20     Import one anyway with `agentstack init --include-tool-managed`.\n",
        );
    }
    out
}

/// The names a schema may declare: sorted, de-duplicated, and restricted to
/// well-formed reference names. The filter is not defensive decoration — these
/// names come from imported third-party CLI configs, and anything that is not a
/// `${REF}` name has no business being written into a file at all.
fn schema_names(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = names
        .iter()
        .filter(|n| agentstack_core::refs::is_ref_name(n))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The `.env.schema` body: varlock's root decorators, the `# ---` divider that
/// ends them, and one declaration per name with an EMPTY value.
///
/// Empty is the contract, not an omission. A value written here would be a
/// secret serialized into the repository — the exact thing `${REF}` exists to
/// prevent — so the file is a declaration and the vault holds the values.
fn env_schema_body(names: &[String]) -> String {
    let mut out = String::from(
        "# varlock schema — https://varlock.dev\n\
         # Written by `agentstack init`. Declares the names this project needs.\n\
         # Values are NEVER written here: agentstack keeps ${REF} placeholders and\n\
         # resolves them in memory at run time. Safe to commit.\n\
         # @defaultSensitive=true\n\
         # @defaultRequired=true\n\
         # ---\n",
    );
    for name in names {
        out.push_str(name);
        out.push_str("=\n");
    }
    out
}

/// Print the three storage options' full help text plus the varlock note — the
/// context that prints once, above whichever selector runs.
fn print_secret_store_help() {
    println!("\nWhere should these token values live?\n");
    println!(
        "  {}) Project .env  (default) — Your tokens are written to .env next to the",
        "1".bold()
    );
    println!("     manifest, in plain text. agentstack keeps this file out of git and its");
    println!("     guard blocks agents from reading it. Edit it with any editor.");
    println!(
        "  {}) {KEYCHAIN_LABEL} — Your tokens are migrated into the OS credential store",
        "2".bold()
    );
    println!("     (service `agentstack`). Nothing secret sits in a file. View or change");
    println!("     {KEYCHAIN_VIEW_HINT}");
    println!(
        "  {}) Skip / decide later — Only ${{REF}} placeholders are written. Nothing runs",
        "3".bold()
    );
    println!("     until you provide values (env, varlock, keychain, or .env) —");
    println!("     `agentstack doctor` lists what's missing.");
    println!(
        "\n  {}",
        "Already using 1Password or a secrets manager? Drop a .env.schema in the".dimmed()
    );
    println!(
        "  {}",
        "project and refs resolve through varlock instead.".dimmed()
    );
}

/// Non-TTY fallback: the numbered stdin prompt (the shape that predated the
/// arrow-key selector). Never panics on a pipe — a closed stdin reads empty and
/// falls through to the `.env` default via `parse_secret_choice`.
fn read_numbered_secret_choice() -> Result<SecretStore> {
    use std::io::Write;
    print!("\nChoice [1]: ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok();
    Ok(parse_secret_choice(&line))
}

/// The store at a 0-based `Select` index: 0 → `.env` (default), 1 → keychain,
/// 2 → skip. Pure, so the index mapping is unit-testable without a terminal.
fn secret_store_at(idx: usize) -> SecretStore {
    match idx {
        1 => SecretStore::Keychain,
        2 => SecretStore::Skip,
        _ => SecretStore::Env,
    }
}

/// Map the numbered-prompt input to a store. Bare Enter (empty), `1`, or
/// anything unrecognized selects the `.env` default — the safe, familiar choice
/// for a write; only an explicit `2`/`3` picks the alternatives.
fn parse_secret_choice(input: &str) -> SecretStore {
    match input.trim() {
        "2" => SecretStore::Keychain,
        "3" => SecretStore::Skip,
        _ => SecretStore::Env,
    }
}

/// Report `${REF}`s the keychain refused to store (unreachable credential
/// store), each with the exact command to store it later.
fn report_unstored_keychain(unstored: &[String]) {
    println!(
        "{}  {}",
        "⚠".yellow(),
        format!(
            "The OS credential store is unreachable — {} not stored:",
            super::count(unstored.len(), "value")
        )
        .yellow()
        .bold()
    );
    for r in unstored {
        println!(
            "      {}   agentstack secret set {r}",
            format!("${{{r}}}").yellow()
        );
    }
    println!(
        "      {}",
        "The manifest keeps ${REF}s. Provide values via env, varlock, or a project .env; apply/run block on unresolved refs by name.".dimmed()
    );
}

/// Report the values init deliberately did NOT store (skip path), each with the
/// one-liner to store it. This replaces `--no-keychain`'s old silent value-drop.
fn report_skipped(lifted: &[Lifted]) {
    // One token is one fact, so it gets one line rather than a header over a
    // single-row table.
    if let [one] = lifted {
        println!(
            "{}  {} — provide it before running: agentstack secret set {}",
            "·".dimmed(),
            "1 token not stored".bold(),
            one.reference
        );
        return;
    }
    let pronoun = if lifted.len() == 1 { "it" } else { "each" };
    println!(
        "{}  {}",
        "·".dimmed(),
        format!(
            "{} not stored — provide {pronoun} before running:",
            super::count(lifted.len(), "token")
        )
        .bold()
    );
    let width = lifted.iter().map(|l| l.reference.len()).max().unwrap_or(0);
    for l in lifted {
        println!(
            "      {} {}  agentstack secret set {}",
            format!("${{{}}}", l.reference).yellow(),
            " ".repeat(width.saturating_sub(l.reference.len())),
            l.reference
        );
    }
}

pub fn run(args: &InitArgs, manifest_dir: Option<&Path>) -> Result<()> {
    // The TTY probe is injected so the non-interactive refusal below is
    // testable without a real terminal (the same seam as `trust::grant_gated`).
    run_gated(args, manifest_dir, crate::util::confirm::is_interactive())
}

/// The `init` dispatch with the interactive probe injected. `interactive` is
/// whether this is an attended terminal session; production passes
/// `crate::util::confirm::is_interactive()`.
fn run_gated(args: &InitArgs, manifest_dir: Option<&Path>, interactive: bool) -> Result<()> {
    if args.plan {
        // Read-only, so it bypasses the bare/TTY gating below by design; the
        // global template has no detection to plan over.
        anyhow::ensure!(
            !args.global,
            "--plan applies to project import, not --global"
        );
        return run_plan(args, manifest_dir);
    }
    if args.global {
        return run_global(args);
    }
    // A truly flagless invocation: no flag opts into either the guided path or
    // the scripted primitive. `--yes` counts as an init-shaping flag — it is
    // the explicit acknowledgement that the scripted import will write.
    let bare =
        !args.force && !args.dry_run && args.secrets.is_none() && !args.no_keychain && !args.yes;
    if bare {
        // P27 — one verb: a bare interactive `init` IS the guided wizard (the
        // former `setup`).
        if interactive {
            let wizard = crate::cli::SetupArgs {
                targets: Vec::new(),
                profile: None,
                scope: None,
                // `--project-servers` shapes the import, not the writing, so it
                // does not make the run scripted (it is absent from `bare`
                // above on purpose) — but the wizard must still honour it.
                project_servers: args.project_servers,
                include_tool_managed: args.include_tool_managed,
                // `--connect` is consent already given, so the wizard states
                // the registration instead of asking a question the user has
                // answered. It deliberately does NOT make the run scripted
                // (see `bare` above): the import still shows its review and
                // still asks its own confirm.
                connect: args.connect,
                // `--verbose` is about volume, not about what runs, so it
                // travels the same route the import does.
                verbose: args.verbose,
            };
            return super::setup::run(&wizard, manifest_dir);
        }
        // Non-TTY with no flags: refuse before writing anything. A flagless
        // `init` here would import configs and lift live token values into
        // files with no prompt — the help promises scripts opt in via flags, so
        // honor it. Naming both escapes keeps the scripted path discoverable.
        //
        // But adapt to state first: when a manifest already exists, the
        // generic escapes mislead — `--yes` walks into the --force wall and
        // `--dry-run` previews a from-scratch replacement. The scripted next
        // steps for an initialized project are the render/activate commands.
        if let Some(path) = existing_manifest(manifest_dir)? {
            return Err(already_initialized(&path));
        }
        anyhow::bail!(
            "refusing to init without a terminal: a flagless `agentstack init` imports your \
             CLI configs and can lift live token values into files, so it never runs without \
             a prompt or an explicit flag\n\
             \n  \
             preview only (writes nothing):  agentstack init --dry-run\n  \
             import without prompts:         agentstack init --yes   (secrets → keychain)\n  \
             choose the secret store:        agentstack init --secrets <env|keychain|skip>\n\
             \n\
             (in a terminal, plain `agentstack init` is the guided wizard)"
        );
    }
    // Any explicit flag (or --yes) proceeds promptlessly as the scriptable
    // primitive: import, write, no prompts beyond what flags allow.
    run_impl(args, manifest_dir, true, false).map(|_| ())
}

/// `init --plan` — Lane A's read primitive (UI control-plane §4): run init's
/// DETECTION only and emit the import plan as structured JSON. Writes
/// nothing, prompts nothing, stores nothing. Reuses the exact discovery and
/// secret-lifting code paths the real import runs — this is the same plan,
/// minus the writes — and emits only each lifted secret's `${REF}` name and
/// origin, NEVER its value: the values live in memory for the lifetime of
/// this call and are dropped.
///
/// The emitted `plan_digest` identifies this exact plan: a later scripted
/// apply may present it as `--consented-plan` and the write then refuses if
/// re-running detection yields a different plan — the same reviewed-bytes
/// binding `trust --preview` / `--consented-digest` gives the trust grant.
fn run_plan(args: &InitArgs, manifest_dir: Option<&Path>) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&crate::ui_contract::envelope(plan_json(
            args,
            manifest_dir
        )?))?
    );
    Ok(())
}

/// The plan `--plan` prints, without the envelope: body plus `plan_digest`.
/// Public read API so integrations and the race witnesses exercise the exact
/// production plan/digest pair instead of re-deriving one.
pub fn plan_json(args: &InitArgs, manifest_dir: Option<&Path>) -> Result<serde_json::Value> {
    let base = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let dir = crate::manifest::new_manifest_dir(&base);
    let manifest_path = dir.join(MANIFEST_FILE);
    let already_initialized = existing_manifest(manifest_dir)?.is_some();
    let det = detect_import(&dir, args.include_tool_managed)?;
    let destination = store_label(resolve_secret_store(args, false)?);
    let digest = plan_digest(&det, &base, already_initialized, destination);

    // Imported names/targets come from other CLIs' config files — hostile
    // input; sanitize display strings exactly like the trust preview.
    let servers_json: Vec<serde_json::Value> = det
        .servers
        .iter()
        .map(|(name, s)| {
            let (kind, target) = server_kind_target(s);
            let mut entry = serde_json::json!({
                "name": crate::text::sanitize_line(name),
                "kind": kind,
                "target": crate::text::sanitize_line(&target),
            });
            // Operational context the digest binds — surfaced so a reviewer
            // sees what distinguishes two otherwise identical-looking plans.
            // Env VAR NAMES only: values may hold non-lifted plaintext.
            if let serde_json::Value::Object(map) = &mut entry {
                if !s.env.is_empty() {
                    let names: Vec<String> = s
                        .env
                        .keys()
                        .map(|k| crate::text::sanitize_line(k))
                        .collect();
                    map.insert("env".into(), names.into());
                }
                if let Some(cwd) = &s.cwd {
                    map.insert("cwd".into(), crate::text::sanitize_line(cwd).into());
                }
            }
            entry
        })
        .collect();

    Ok(serde_json::json!({
        "path": base.display().to_string(),
        "manifest_path": manifest_path.display().to_string(),
        "already_initialized": already_initialized,
        // Stage 1.2: each detected CLI carries its evidence — binary on PATH
        // and the exact native config files found — so the first screen can
        // state what was found, not just that something was.
        "detected": det
            .detected
            .iter()
            .map(|c| serde_json::json!({
                "id": c.id,
                "display": c.display,
                "bin_on_path": c.bin_on_path,
                "configs": c
                    .configs
                    .iter()
                    .map(|p| crate::text::sanitize_line(&p.display().to_string()))
                    .collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
        // Stage 1.2: where a follow-up `apply --write` renders this import —
        // per CLI, with the scope in plain terms — so destination files are
        // reviewable without adapter knowledge.
        "destinations": det
            .destinations
            .iter()
            .map(|d| serde_json::json!({
                "id": d.id,
                "display": d.display,
                "scope": d.scope.as_str(),
                "path": crate::text::sanitize_line(&d.path.display().to_string()),
                "writes": d.writes,
            }))
            .collect::<Vec<_>>(),
        "servers": servers_json,
        "settings_from": det.settings.keys().collect::<Vec<_>>(),
        "conflicts": det
            .conflict_counts
            .iter()
            .map(|(name, extra)| serde_json::json!({
                "name": crate::text::sanitize_line(name),
                "other_definitions": extra,
            }))
            .collect::<Vec<_>>(),
        "secrets": det
            .lifted
            .iter()
            .map(|l| serde_json::json!({
                "reference": l.reference,
                "origin": crate::text::sanitize_line(&l.origin),
            }))
            .collect::<Vec<_>>(),
        "secrets_destination": destination,
        // Lossy-import honesty (Stage 1.2): entries the import must leave in
        // their CLI's own config, each with a plain-language reason. Purely
        // informational — they never enter the written manifest, so they do
        // not participate in the plan digest.
        "unsupported": det
            .skipped
            .iter()
            .map(|(cli, s)| serde_json::json!({
                "cli": cli,
                "name": crate::text::sanitize_line(&s.name),
                "reason": s.reason,
            }))
            .collect::<Vec<_>>(),
        // Servers another application installed and keeps updated
        // (`init-tool-managed-v1`). Left out of the import by default and
        // NAMED here, so a panel renders "left alone" — an entry that already
        // has an owner — rather than showing nothing or a duplicate. Each
        // carries the evidence behind the reading, because the classification
        // is a heuristic over paths and a user must be able to check it.
        // `imported` states which of the two default/override outcomes
        // happened. Informational, like `unsupported`: it does not join the
        // digest, and it does not need to — an excluded server is absent from
        // `servers`, which the digest already binds.
        "tool_managed": det
            .tool_managed
            .iter()
            .map(|t| serde_json::json!({
                "name": crate::text::sanitize_line(&t.name),
                "application": crate::text::sanitize_line(&t.application),
                "path": crate::text::sanitize_line(&t.evidence),
                "reason": TOOL_MANAGED_REASON,
                "imported": det.tool_managed_included,
            }))
            .collect::<Vec<_>>(),
        "plan_digest": digest,
    }))
}

/// One detected CLI, with the facts the first screen states (Stage 1.2):
/// whether its binary is on PATH and which native config files detection
/// actually found on disk — never just "detected" with the evidence hidden.
struct DetectedCli {
    id: String,
    display: String,
    bin_on_path: bool,
    /// Native config files of this CLI that exist on disk (global MCP config
    /// and settings file), deduped — the exact files the import reads.
    configs: Vec<PathBuf>,
}

/// One native file the recommended `apply --write` will manage after this
/// import, in user terms — which CLI, which file, which scope, and what lands
/// there — so destinations are visible without adapter knowledge (Stage 1.2).
struct PlanDestination {
    id: String,
    display: String,
    scope: crate::scope::Scope,
    path: PathBuf,
    /// What renders into this file: "MCP servers" and/or "settings".
    writes: Vec<&'static str>,
}

/// Everything one detection pass finds — computed ONCE and consumed by both
/// the plan (display + digest) and the consented write, so the plan a user
/// reviewed and the import the write performs are the same in-memory objects,
/// never two detections that could observe different disk states
/// (independent review, 2026-07-23).
struct DetectedImport {
    detected: Vec<DetectedCli>,
    /// Adapter IDS of the CLIs that actually contributed servers or settings —
    /// the honest "imported from" list (a detected CLI with an empty config is
    /// not a source). Ids rather than display names because this list is also
    /// the manifest's default targets (M4); display names are derived with
    /// [`det_display`] wherever a human reads it.
    contributing: Vec<String>,
    /// Post-lift: inline token values already rewritten to `${REF}`.
    servers: IndexMap<String, Server>,
    /// Full imported settings values per contributing CLI id — exactly what
    /// the written manifest will hold.
    settings: IndexMap<String, serde_json::Value>,
    conflict_counts: IndexMap<String, usize>,
    lifted: Vec<crate::discover::Lifted>,
    /// Entries a CLI's config declares that the import had to leave behind,
    /// as `(cli display name, skip)` — surfaced in the plan and the write
    /// output so a lossy import is explained, never silent.
    skipped: Vec<(String, crate::adapter::SkippedImport)>,
    /// Whether any server in the merged set came from a PROJECT-scope config
    /// file (a `.mcp.json` in the repo, not the user's machine files). Those
    /// bytes arrive with a clone — repo-supplied, hostile-input class — so
    /// the post-import convenience grant must not cover them (F7): they take
    /// the ordinary `agentstack trust .` review instead. Conflicts don't
    /// count: a project copy that lost the merge contributed nothing to the
    /// manifest being granted.
    project_sourced: bool,
    /// The native files a follow-up `apply --write` (at the default scope for
    /// this manifest) would manage — derived from the same detection, so the
    /// plan and the terminal review state identical destinations.
    destinations: Vec<PlanDestination>,
    /// Servers whose executable lives inside another application's bundle, in
    /// first-seen order and deduplicated by name (the desktop apps register
    /// the same entry into every tool config on the machine, so one server
    /// arrives six times). Recorded whether or not they were imported: an
    /// exclusion nobody can see is exactly the silent drop this exists to
    /// prevent.
    tool_managed: Vec<ToolManagedServer>,
    /// Whether `--include-tool-managed` overrode the default, so the plan and
    /// the review can state which of the two happened instead of leaving a
    /// reader to infer it from a list.
    tool_managed_included: bool,
    /// The gateway-bridge entries this import passed over, deduplicated by name
    /// — our own registration, read back out of the harness configs we wrote it
    /// into. See [`crate::commands::connect::is_bridge_entry`]. Never a warning
    /// and never overridable: there is no reading of "import agentstack into
    /// agentstack" that is what the user meant.
    bridge_entries: Vec<String>,
}

/// One server the classifier read as belonging to the application that
/// installed it, carrying the evidence behind that reading so a user can check
/// it rather than take it on faith.
struct ToolManagedServer {
    name: String,
    /// The owning application as DETECTED — a bundle directory name, not a
    /// verified publisher (see [`crate::adapter::tool_managed`]).
    application: String,
    /// The path that matched, verbatim.
    evidence: String,
}

/// Why a tool-managed server is left out, in one plain sentence. A constant so
/// the JSON a panel renders and the line a terminal prints cannot drift apart.
/// Phrased as the RULE rather than as a fact about one entry, so it reads
/// correctly both as a per-server `reason` and under a list of several.
const TOOL_MANAGED_REASON: &str =
    "an executable inside another application's bundle belongs to that application: it \
     installs, updates and owns the entry";

impl DetectedImport {
    /// The detected CLI ids, in detection order — the manifest's default
    /// targets and the digest's `detected` binding.
    fn detected_ids(&self) -> Vec<String> {
        self.detected.iter().map(|c| c.id.clone()).collect()
    }
}

/// Display name for a detected CLI id (falls back to the id itself).
fn det_display(detected: &[DetectedCli], id: &str) -> String {
    detected
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.display.clone())
        .unwrap_or_else(|| id.to_string())
}

/// One detection pass. `include_tool_managed` decides only whether servers the
/// classifier attributes to another application STAY in the import; they are
/// recorded and reported either way.
fn detect_import(dir: &Path, include_tool_managed: bool) -> Result<DetectedImport> {
    let registry = Registry::load()?;
    let mut detected: Vec<DetectedCli> = Vec::new();
    let mut contributing: Vec<String> = Vec::new();
    let mut servers: IndexMap<String, Server> = IndexMap::new();
    let mut settings: IndexMap<String, serde_json::Value> = IndexMap::new();
    let mut conflict_counts: IndexMap<String, usize> = IndexMap::new();
    let mut skipped: Vec<(String, crate::adapter::SkippedImport)> = Vec::new();
    let mut tool_managed: IndexMap<String, ToolManagedServer> = IndexMap::new();
    let mut bridge_entries: Vec<String> = Vec::new();
    let mut project_sourced = false;
    // Which scopes this import reads. A project manifest imports what is
    // configured in the project too — before this, `init` asked the machine-scope
    // `detected()` and then read only global files, so a repo whose whole setup
    // lived in `.mcp.json` got "No supported CLIs detected to import" and an
    // empty starter manifest (pilot Run B). At global scope there is no project
    // to read, so the extra pass is skipped rather than pointed at the cwd.
    let import_project = crate::scope::Scope::default_for(dir) == crate::scope::Scope::Project;
    for desc in registry.iter() {
        if !(desc.detected() || (import_project && desc.project_config_present(dir))) {
            continue;
        }
        // The evidence behind "detected": which files exist. The settings file
        // can be the same file as the MCP config (Codex) — dedup.
        let mut configs: Vec<PathBuf> = Vec::new();
        if let Some(config) = desc.config.as_ref() {
            let path = crate::util::paths::expand_tilde(&config.path);
            if path.exists() {
                configs.push(path);
            }
        }
        if import_project {
            if let Some((path, _)) = desc.config_for(crate::scope::Scope::Project, dir) {
                if path.exists() && !configs.contains(&path) {
                    configs.push(path);
                }
            }
        }
        if let Some((path, _)) = desc.settings_for(crate::scope::Scope::Global, dir) {
            if path.exists() && !configs.contains(&path) {
                configs.push(path);
            }
        }
        detected.push(DetectedCli {
            id: desc.id.clone(),
            display: desc.display.clone(),
            bin_on_path: desc.is_installed(),
            configs,
        });
        let mut contributed = false;
        // Global first, then project: `merge_servers` keeps the first definition
        // of a name, so a machine-wide server stays the one imported and the
        // project copy is reported as a conflict rather than silently winning.
        let mut scopes = vec![crate::scope::Scope::Global];
        if import_project {
            scopes.push(crate::scope::Scope::Project);
        }
        for scope in scopes {
            let Some(value) = desc.read_config_value_for(scope, dir)? else {
                continue;
            };
            let (extracted, skips) = extract_servers_with_skips(desc, &value);
            skipped.extend(skips.into_iter().map(|s| (desc.display.clone(), s)));
            // Another application's plumbing is not this user's setup. Record
            // every such entry (once, by name — the desktop apps register the
            // same server into every tool config here), and drop it from the
            // import unless the flag says otherwise. This happens BEFORE the
            // merge so an excluded name never becomes a conflict nothing
            // imported, and BEFORE `contributed` so a CLI whose only offering
            // was someone else's plumbing does not become a default target.
            let mut imported = Vec::with_capacity(extracted.len());
            for (name, server) in extracted {
                // Our own gateway bridge, read back out of a config THIS tool
                // wrote. Dropped first and unconditionally: it is not the
                // user's setup, and a manifest that declares it would ask the
                // gateway to serve the gateway. Like the tool-managed check
                // below, this runs before `contributed`, so a CLI whose only
                // MCP entry was our bridge does not become a default target on
                // the strength of our own plumbing.
                if crate::commands::connect::is_bridge_entry(&name, &server) {
                    if !bridge_entries.contains(&name) {
                        bridge_entries.push(name);
                    }
                    continue;
                }
                let Some(found) = crate::adapter::tool_managed(&server) else {
                    imported.push((name, server));
                    continue;
                };
                if !tool_managed.contains_key(&name) {
                    tool_managed.insert(
                        name.clone(),
                        ToolManagedServer {
                            name: name.clone(),
                            application: found.application,
                            evidence: found.evidence,
                        },
                    );
                }
                if include_tool_managed {
                    imported.push((name, server));
                }
            }
            contributed |= !imported.is_empty();
            let offered = imported.len();
            let mut collided = 0usize;
            for c in merge_servers(&mut servers, imported) {
                collided += 1;
                *conflict_counts.entry(c).or_insert(0usize) += 1;
            }
            // A project-scope server that actually LANDED makes the merged
            // manifest partly repo-supplied — see `project_sourced`.
            if scope == crate::scope::Scope::Project && offered > collided {
                project_sourced = true;
            }
        }
        if let Some(value) = desc.read_settings_value(dir)? {
            let imported = extract_settings(desc, &value);
            if !imported.is_empty() {
                contributed = true;
                settings.insert(desc.id.clone(), serde_json::Value::Object(imported));
            }
        }
        if contributed {
            contributing.push(desc.id.clone());
        }
    }
    // Lifting rewrites the in-memory servers to `${REF}` placeholders and
    // returns the values; only reference + origin ever serialize.
    let lifted = lift_secrets(&mut servers);

    // Proposed destinations: the files `apply --write` at this manifest's
    // default scope would manage, per detected CLI, merged when servers and
    // settings share one file (Codex). Derived from the same pass, so the
    // reviewed destinations can't disagree with a later apply.
    let scope = crate::scope::Scope::default_for(dir);
    let mut destinations: Vec<PlanDestination> = Vec::new();
    for cli in &detected {
        let Some(desc) = registry.get(&cli.id) else {
            continue;
        };
        let mut files: Vec<(PathBuf, Vec<&'static str>)> = Vec::new();
        // Servers only appear here when the delivery planner routes them to
        // FILES. `apply` honours that routing, so listing a `.mcp.json` for an
        // MCP-capable CLI would promise a file nothing ever writes — and it sat
        // one line above the routing block saying those servers are served
        // live. The two blocks used to contradict each other; now they cannot,
        // because both read the same planner. No manifest exists yet at import
        // time, so the routing is the default one (no `[delivery]` override).
        let servers_render = crate::delivery::route(
            crate::delivery::Kind::Server,
            desc.mcp.is_some(),
            Delivery::default().renders_locally(&desc.id),
        )
        .lane
            == crate::delivery::Lane::Rendered;
        if !servers.is_empty() && desc.mcp.is_some() && servers_render {
            if let Some((path, _)) = desc.config_for(scope, dir) {
                files.push((path, vec!["MCP servers"]));
            }
        }
        for (path, writes) in files {
            destinations.push(PlanDestination {
                id: cli.id.clone(),
                display: cli.display.clone(),
                scope,
                path,
                writes,
            });
        }
        // Settings are declared in the MACHINE manifest (they were read from
        // each CLI's machine-wide config, and are personal preferences), so the
        // file a later apply manages for them is that CLI's own GLOBAL settings
        // file — never a project copy. Listing the project path here was the
        // review promising the wrong destination for the wrong layer.
        if settings.contains_key(&cli.id) {
            if let Some((path, _)) = desc.settings_for(crate::scope::Scope::Global, dir) {
                destinations.push(PlanDestination {
                    id: cli.id.clone(),
                    display: cli.display.clone(),
                    scope: crate::scope::Scope::Global,
                    path,
                    writes: vec!["settings"],
                });
            }
        }
    }

    Ok(DetectedImport {
        detected,
        contributing,
        servers,
        settings,
        conflict_counts,
        lifted,
        skipped,
        project_sourced,
        destinations,
        tool_managed: tool_managed.into_values().collect(),
        tool_managed_included: include_tool_managed,
        bridge_entries,
    })
}

/// One server's user-facing shape: its transport kind and what it runs
/// (stdio: command + argv joined for display) or contacts (http: URL). Shared
/// by the plan JSON and the terminal review so both describe a server
/// identically. Display-only — the digest binds the full `Server` object.
fn server_kind_target(s: &Server) -> (&'static str, String) {
    match s.server_type {
        crate::manifest::ServerType::Stdio => (
            "stdio",
            format!(
                "{} {}",
                s.command.as_deref().unwrap_or("?"),
                s.args.join(" ")
            )
            .trim()
            .to_string(),
        ),
        crate::manifest::ServerType::Http => ("http", s.url.clone().unwrap_or_default()),
    }
}

fn store_label(store: SecretStore) -> &'static str {
    match store {
        SecretStore::Env => "env",
        SecretStore::Keychain => "keychain",
        SecretStore::Skip => "skip",
    }
}

/// The stable identity of a computed plan (v2): a domain-separated digest
/// over the COMPLETE import — full `Server` objects (env, cwd, headers, argv
/// as arrays), imported settings values, conflicts, secret reference names
/// and origins (never values), and the destination store. v1 hashed the
/// sanitized display summary, which omitted operational fields and flattened
/// argv with spaces, so two plans that would write different manifests could
/// share a digest (independent review, 2026-07-23).
fn plan_digest(
    det: &DetectedImport,
    base: &Path,
    already_initialized: bool,
    destination: &str,
) -> String {
    use sha2::{Digest, Sha256};
    let full = serde_json::json!({
        "path": base.display().to_string(),
        "already_initialized": already_initialized,
        "detected": det.detected_ids(),
        "servers": serde_json::to_value(&det.servers)
            .expect("derive(Serialize) manifest types always serialize"),
        "settings": det.settings,
        "conflicts": det.conflict_counts,
        "secrets": det
            .lifted
            .iter()
            .map(|l| serde_json::json!({ "reference": l.reference, "origin": l.origin }))
            .collect::<Vec<_>>(),
        "secrets_destination": destination,
    });
    let mut hasher = Sha256::new();
    hasher.update(b"agentstack:init-plan:v2\n");
    hasher.update(full.to_string().as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

/// Template for the machine-level manifest. Deliberately NOT an import: the
/// personal layer starts empty and explicit — it carries intent that travels
/// with the user (instruction fragments, eventually more), not a copy of
/// whatever the CLIs happen to hold today (that's project `init`'s job).
const GLOBAL_MANIFEST_TEMPLATE: &str = "\
# Machine-level agentstack manifest — the personal layer.
# Cross-project intent that travels with YOU, not with a repo: instruction
# fragments compiled into each CLI's global CLAUDE.md / AGENTS.md.
#
# Declare a fragment, drop its markdown in ./instructions/, then compile:
#
#   [instructions.style]
#   path = \"./instructions/style.md\"   # relative to this directory
#   targets = [\"*\"]                     # or [\"claude-code\", \"codex\"]
#
version = 1

[instructions]
";

/// `agentstack init --global` — seed `~/.agentstack/agentstack.toml` (honoring
/// `AGENTSTACK_HOME`) with an empty `[instructions]` block and an
/// `instructions/` dir. This blesses the home layer as a first-class manifest:
/// `agentstack instructions` run from `$HOME` (or with `--manifest-dir`)
/// compiles its fragments into each CLI's global instruction file. The
/// zero-files gateway deliberately never discovers this layer as a project
/// (see `manifest::discover_project_base`).
fn run_global(args: &InitArgs) -> Result<()> {
    let home = crate::util::paths::agentstack_home();
    let manifest_path = home.join(MANIFEST_FILE);
    let instr_dir = home.join("instructions");
    if manifest_path.exists() && !args.force && !args.dry_run {
        anyhow::bail!(
            "{} already exists — use --force to overwrite or --dry-run to preview",
            manifest_path.display()
        );
    }

    // Preview before ANY filesystem write (and before the house-rules prompt).
    // The preview is the SEEDED template — [guard] + [policy.filesystem]
    // included — because seed_machine_toml is pure (A1 witness: --dry-run
    // shows the policy blocks and writes nothing).
    if args.dry_run {
        let seeded = super::guard::seed_machine_toml(GLOBAL_MANIFEST_TEMPLATE)?;
        println!("\n{} (preview — nothing written)\n", MANIFEST_FILE.bold());
        println!("{seeded}");
        println!("Would write {}", manifest_path.display());
        println!("Would create {}/", instr_dir.display());
        println!(
            "Would offer to install the host guard into detected CLIs \
             (never installed without an explicit yes)."
        );
        println!(
            "Would offer the agentstack house rules fragment ([instructions.{HOUSE_RULES_NAME}])."
        );
        return Ok(());
    }
    if manifest_path.exists() {
        // --force: start over from the template (ensure_global_manifest would
        // keep the existing file).
        std::fs::remove_file(&manifest_path)
            .with_context(|| format!("removing {}", manifest_path.display()))?;
    }

    ensure_global_manifest()?;
    // Seed [guard] + [policy.filesystem] deny through the SAME path as
    // `guard install` — one canonical default list, idempotent, and an
    // explicitly-empty user deny list is respected as an opt-out.
    super::guard::seed_machine_config()?;
    println!("{}  Wrote {}", "✅".dimmed(), manifest_path.display());
    println!("{}  Created {}/", "📁".dimmed(), instr_dir.display());
    println!(
        "{}  Seeded [guard] + [policy.filesystem] deny ({} default entries — edit anytime)",
        "🛡️".dimmed(),
        super::guard::DEFAULT_DENY.len()
    );

    // The guard-install offer (A1). Never silent: installing edits other
    // CLIs' config files, so it happens only on an explicit yes — and
    // `confirm` returns false without prompting when non-interactive, which
    // is exactly t3code/CI contract (report the pending offer, never
    // auto-install).
    println!(
        "\nThe host guard enforces that deny list inside each CLI's own hook system —\n\
         it blocks accidental secret reads and destructive commands; it is not a sandbox."
    );
    let detected = super::guard::detected_target_ids();
    if detected.is_empty() {
        println!(
            "  {} no hook-capable CLIs detected — run `agentstack guard install` after installing one.",
            "·".dimmed()
        );
    } else {
        println!("  Detected CLIs: {}", detected.join(" · "));
        let prompt = if detected.len() == 1 {
            "Install the guard into this CLI?".to_string()
        } else {
            format!("Install the guard into these {} CLIs?", detected.len())
        };
        if crate::util::confirm::confirm(&prompt)? {
            super::guard::install()?;
        } else {
            println!(
                "  {} skipped — run `agentstack guard install` anytime.",
                "·".dimmed()
            );
        }
    }

    // Offer the agentstack house rules — the fragment that teaches every agent
    // the manifest-first workflow. Opt-in (it steers the daily-driver agent),
    // like pack instructions. Non-interactive shells skip; `setup` re-offers.
    if crate::util::confirm::confirm(
        "\nInstall the agentstack house rules fragment (teaches agents the manifest-first workflow)?",
    )? {
        if seed_house_rules(&home)? {
            println!(
                "  {} installed [instructions.{HOUSE_RULES_NAME}] → {}/{HOUSE_RULES_NAME}.md",
                "✓".green(),
                instr_dir.display()
            );
        }
    } else {
        println!(
            "  {} skipped — `agentstack init` will offer them again.",
            "·".dimmed()
        );
    }

    println!(
        "\nNext: drop fragments in {}/, declare them under [instructions.*], then:",
        instr_dir.display()
    );
    println!("    {}", instructions_hint(&home).bold());
    Ok(())
}

/// Name and bundled source of the agentstack house-rules fragment, shared by
/// `init --global` and `setup` so both seed the same thing.
pub const HOUSE_RULES_NAME: &str = "agentstack";
const HOUSE_RULES_ASSET: &str = "instructions/agentstack/rules.md";

/// Ensure the machine-level manifest exists (seeding the template and the
/// `instructions/` dir if needed); returns the home manifest dir.
pub fn ensure_global_manifest() -> Result<PathBuf> {
    let home = crate::util::paths::agentstack_home();
    let manifest_path = home.join(MANIFEST_FILE);
    let instr_dir = home.join("instructions");
    std::fs::create_dir_all(&instr_dir)
        .with_context(|| format!("creating {}", instr_dir.display()))?;
    if !manifest_path.exists() {
        crate::util::atomic::write(&manifest_path, GLOBAL_MANIFEST_TEMPLATE)
            .with_context(|| format!("writing {}", manifest_path.display()))?;
    }
    Ok(home)
}

/// Install the agentstack house-rules fragment into the manifest at `dir`:
/// extract the bundled markdown to `instructions/agentstack.md` (an existing
/// file — possibly user-edited — is kept) and declare it under
/// `[instructions.agentstack]`, preserving manifest comments. Returns `false`
/// when the manifest already declares the fragment.
pub fn seed_house_rules(dir: &Path) -> Result<bool> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest: Manifest =
        toml::from_str(&text).with_context(|| format!("parsing {}", manifest_path.display()))?;
    if manifest.instructions.contains_key(HOUSE_RULES_NAME) {
        return Ok(false);
    }

    let dest = dir
        .join("instructions")
        .join(format!("{HOUSE_RULES_NAME}.md"));
    if !dest.exists() {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let body = crate::catalog::read_asset_file(HOUSE_RULES_ASSET)?;
        crate::util::atomic::write(&dest, &body)
            .with_context(|| format!("writing {}", dest.display()))?;
    }

    let entry = crate::manifest::Instruction {
        path: Some(format!("./instructions/{HOUSE_RULES_NAME}.md")),
        targets: vec!["*".into()],
        variants: Vec::new(),
        from_user_layer: false,
    };
    let new_text = super::add::build_manifest_with(
        &text,
        "instructions",
        HOUSE_RULES_NAME,
        &serde_json::to_value(&entry)?,
        None,
    )?;
    crate::util::atomic::write(&manifest_path, &new_text)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    Ok(true)
}

/// `~/.agentstack` where that is what it is, the real path otherwise. The
/// machine layer is a home-relative idea, and an absolute `/Users/...` in a
/// sentence about "your machine layer" reads as a path to memorize.
fn display_home(home: &Path) -> String {
    let default_home = dirs::home_dir().map(|h| h.join(".agentstack"));
    if default_home.as_deref() == Some(home) {
        "~/.agentstack/agentstack.toml".to_string()
    } else {
        home.join(MANIFEST_FILE).display().to_string()
    }
}

/// The imported per-CLI settings this run may declare in the MACHINE manifest:
/// the ones that layer does not already speak for.
///
/// An existing `[settings.<cli>]` there is the user's own answer, possibly
/// hand-edited and certainly not this project's business to rewrite. The same
/// rule the library-collision path follows: shared state is never silently
/// replaced by an import.
///
/// A machine manifest that cannot be read at all yields "declare everything",
/// which is the correct reading of an absent layer and, for an unparseable one,
/// fails later at the write with the parse error rather than here in a review.
fn machine_settings_to_declare(
    imported: &IndexMap<String, serde_json::Value>,
) -> IndexMap<String, serde_json::Value> {
    if imported.is_empty() {
        return IndexMap::new();
    }
    let path = crate::util::paths::agentstack_home().join(MANIFEST_FILE);
    let existing: Vec<String> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| toml::from_str::<Manifest>(&text).ok())
        .map(|m| m.settings.keys().cloned().collect())
        .unwrap_or_default();
    imported
        .iter()
        .filter(|(id, _)| !existing.contains(id))
        .map(|(id, value)| (id.clone(), value.clone()))
        .collect()
}

/// Declare `settings` under `[settings.<cli>]` in the machine manifest,
/// capturing every file first so the whole thing stays inside `init`'s one
/// undoable transaction.
///
/// `ensure_global_manifest` is the same seeder `init --global` and the wizard's
/// house-rules step use — there is one way the machine manifest comes into
/// existence, and this is not a second one. The capture is taken BEFORE that
/// call, so an undo of an import that created the layer deletes the file it
/// created rather than leaving a manifest nobody asked for.
fn write_machine_settings(
    settings: &IndexMap<String, serde_json::Value>,
    backups: &mut Vec<crate::history::FileChange>,
) -> Result<()> {
    if settings.is_empty() {
        return Ok(());
    }
    let home = crate::util::paths::agentstack_home();
    let path = home.join(MANIFEST_FILE);
    backups.push(crate::history::capture(
        &path,
        "machine manifest · settings",
    ));
    ensure_global_manifest()?;
    let mut text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    for (id, value) in settings {
        text = super::add::build_manifest_with(&text, "settings", id, value, None)
            .with_context(|| format!("declaring [settings.{id}] in {}", path.display()))?;
    }
    crate::util::atomic::write(&path, &text)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// The commented preamble a manifest gets when the import found no capability
/// to declare — otherwise nothing.
///
/// The empty tables it replaces (`[servers]`, `[skills]`, `[instructions]`)
/// were not documentation, they were serialization: `toml` emitted a header for
/// every collection whether or not it held anything, so a first manifest opened
/// on three headings that say nothing and answer nothing. A reader's actual
/// question at that moment is "how do I put something in this?", so that is
/// what the file says, in commands they can run. When there IS content the
/// preamble is omitted entirely — the declarations are the better answer, and a
/// permanent banner is read by nobody.
fn capability_header(manifest: &Manifest) -> String {
    let declares_something = !manifest.servers.is_empty()
        || !manifest.skills.is_empty()
        || !manifest.instructions.is_empty()
        || !manifest.profiles.is_empty();
    if declares_something {
        return String::new();
    }
    "\
# No capabilities declared yet. Add one:
#
#   agentstack search <query>            find a server or skill in the catalog
#   agentstack add from <id> --write     add one to this project
#   agentstack add skill <source>        add a SKILL.md directory
#   agentstack lib link <folder>         reuse a folder of capabilities you keep
#
"
    .to_string()
}

/// The exact `instructions --write` invocation for the machine-level manifest:
/// plain from `$HOME` when the layer lives at the default `~/.agentstack`,
/// spelled with `--manifest-dir` when `AGENTSTACK_HOME` relocated it.
pub(crate) fn instructions_hint(home: &Path) -> String {
    let default_home = dirs::home_dir().map(|h| h.join(".agentstack"));
    if default_home.as_deref() == Some(home) {
        "agentstack instructions --manifest-dir ~ --write".to_string()
    } else {
        format!(
            "agentstack instructions --manifest-dir {} --write",
            home.display()
        )
    }
}

/// The import step as `setup` drives it: `setup` prints its own guidance and
/// continues automatically, so the standalone "run bootstrap next" tail is
/// suppressed. The wizard path gates the write on a confirm shown AFTER the
/// review — the user sees which CLIs/configs were found, which servers and
/// secret names import, and the destination files BEFORE saying yes (Stage
/// 1.2). Returns whether the import actually wrote (false = declined).
pub(crate) fn run_for_setup(args: &InitArgs, manifest_dir: Option<&Path>) -> Result<bool> {
    run_impl(args, manifest_dir, false, true)
}

/// The manifest this invocation would collide with, if one already exists:
/// the explicit `--manifest-dir`'s manifest, or the nearest ancestor
/// project's (the same walk every other command does).
fn existing_manifest(manifest_dir: Option<&Path>) -> Result<Option<std::path::PathBuf>> {
    Ok(match manifest_dir {
        Some(d) => {
            let path = crate::manifest::resolve_manifest_dir(d).join(MANIFEST_FILE);
            path.exists().then_some(path)
        }
        None => crate::manifest::discover_project_base(&std::env::current_dir()?)
            .map(|root| crate::manifest::resolve_manifest_dir(&root).join(MANIFEST_FILE)),
    })
}

/// The refusal for a scripted `init` against an already-initialized project.
/// Another import is almost never what the script wants — name the actual
/// next steps, and keep `--force` available but labeled as the destructive
/// path. (Interactive bare `init` never hits this: the wizard resumes.)
fn already_initialized(manifest_path: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "{} already exists — init has nothing left to do here\n\
         \n  \
         render it into your CLIs:  agentstack apply --write\n  \
         activate a profile:        agentstack use <profile> --write\n  \
         re-import from scratch:    agentstack init --force   (replaces the manifest)\n\
         \n\
         (in a terminal, plain `agentstack init` resumes the wizard: preview, apply, verify)",
        manifest_path.display()
    )
}

/// Refuse a bare `init` from inside an already-initialized project: every
/// other command walks up to that root's manifest (`commands::project_base`),
/// so silently creating a NESTED one here would fork the project into two
/// manifests that fight over the same tree. Nesting stays possible, but only
/// deliberately — `--force` or an explicit `--manifest-dir`.
fn refuse_nested_init(cwd: &Path) -> Result<()> {
    if let Some(root) = crate::manifest::discover_project_base(cwd) {
        if root != cwd {
            anyhow::bail!(
                "this project is already initialized at {} — commands run from here \
                 find that manifest; pass --force (or --manifest-dir {}) to nest a \
                 separate project in this directory",
                crate::manifest::resolve_manifest_dir(&root)
                    .join(MANIFEST_FILE)
                    .display(),
                cwd.display()
            );
        }
    }
    Ok(())
}

/// The import itself. `gate_write` (the wizard path) inserts one confirm
/// between the printed review — CLIs/configs found, servers by name, lifted
/// secret references, destination files — and the first write, so consent
/// follows the evidence (Stage 1.2). Returns whether the import proceeded
/// (`false` only when that gate was declined; nothing was written).
fn run_impl(
    args: &InitArgs,
    manifest_dir: Option<&Path>,
    show_next: bool,
    gate_write: bool,
) -> Result<bool> {
    let base = match manifest_dir {
        Some(d) => d.to_path_buf(),
        None => {
            let cwd = std::env::current_dir()?;
            // Same escape hatches as the "already exists" check below:
            // --force nests deliberately, --dry-run only previews.
            if !args.force && !args.dry_run {
                refuse_nested_init(&cwd)?;
            }
            cwd
        }
    };
    // Create new manifests in `.agentstack/`; keep updating a legacy root one.
    let dir = crate::manifest::new_manifest_dir(&base);
    let manifest_path = dir.join(MANIFEST_FILE);
    if manifest_path.exists() && !args.force {
        if !args.dry_run {
            return Err(already_initialized(&manifest_path));
        }
        // The preview below is a fresh re-import, not the file on disk — say
        // so, or a reader assumes init merges and that their current servers
        // survived (they would not: a write replaces the manifest).
        println!(
            "{} existing manifest at {} — this preview shows a fresh re-import, not the file \
             on disk; writing it takes `agentstack init --force` and replaces the manifest",
            "⚠".yellow(),
            manifest_path.display()
        );
    }

    // ONE detection pass: the consent check below and the writes both consume
    // this same instance. A verify-then-redetect sequence would let a CLI
    // config that changed between the two reads be imported (and its token
    // stored) without ever being compared against the reviewed plan
    // (independent review, 2026-07-23).
    let det = detect_import(&dir, args.include_tool_managed)?;
    // Reviewed-plan binding: refuse before ANY print or mutation when this
    // detection no longer digests to the reviewed plan. The destination is
    // resolved non-interactively exactly as `--plan` resolved it, so both
    // digests describe the same store choice; the write path below reuses
    // this resolution instead of prompting to a different one.
    let preresolved_store = match args.consented_plan {
        Some(_) => Some(resolve_secret_store(args, false)?),
        None => None,
    };
    if let Some(consented) = args.consented_plan.as_deref() {
        let already = existing_manifest(manifest_dir)?.is_some();
        let store = preresolved_store.expect("resolved right above for Some(consented)");
        let actual = plan_digest(&det, &base, already, store_label(store));
        anyhow::ensure!(
            consented == actual,
            "refusing to apply: the detected setup changed since this plan was reviewed \
             (consented {consented}, current {actual}) — re-run `agentstack init --plan`, \
             review the new plan, and apply with its plan_digest"
        );
    }
    let DetectedImport {
        detected,
        contributing,
        servers,
        settings,
        conflict_counts,
        lifted,
        skipped,
        project_sourced,
        destinations,
        tool_managed,
        tool_managed_included,
        bridge_entries,
    } = det;
    let detected_ids: Vec<String> = detected.iter().map(|c| c.id.clone()).collect();
    let display_names: Vec<String> = detected.iter().map(|c| c.display.clone()).collect();

    // M4: target only the CLIs that actually contributed configuration. Every
    // detected binary used to become a target, so `apply --write` created a
    // `.gemini/settings.json` and an `opencode.json` in the repo of someone who
    // has never opened either tool — unexplained files in a user's project, and
    // diff noise for every operation afterwards. The import summary already
    // draws exactly this distinction ("contributed content" vs "binary on PATH
    // — no config files found"); this makes the manifest agree with it.
    //
    // The fallback matters: a machine with agent CLIs installed but no config
    // anywhere contributes nothing, and an empty `[targets] default` would
    // render to nothing at all and read as a broken import. There, every
    // detected CLI is the best available guess.
    let target_defaults: Vec<String> = if contributing.is_empty() {
        detected_ids.clone()
    } else {
        contributing.clone()
    };
    // The detected-but-silent CLIs, for the summary's "also detected" line.
    let also_detected: Vec<String> = detected
        .iter()
        .filter(|c| !target_defaults.contains(&c.id))
        .map(|c| c.display.clone())
        .collect();
    for (name, extra) in &conflict_counts {
        let clis = super::count(*extra, "other CLI");
        let others = if *extra == 1 {
            "the other stays in its"
        } else {
            "the others stay in their"
        };
        println!(
            "{} server '{name}' is defined differently by {clis} — kept the first \
             definition imported ({others} CLI's own config)",
            "⚠".yellow()
        );
    }

    if detected.is_empty() {
        // Backstop for the silent-empty-manifest shape: if ANY native config is
        // readable here, say so and name the command that takes it, instead of
        // writing an empty manifest and sending the user to a catalog search.
        // Detection above should now cover every such file — this fires only
        // when a config exists that detection could not claim (an unparseable
        // file, a shape we cannot read), and it must still not be silent.
        let leftovers =
            crate::discover::native_configs(&Registry::load()?, &dir, &Default::default(), false);
        if !leftovers.is_empty() {
            println!(
                "{} Config files for {} are present here but nothing could be imported from them:",
                "⚠".yellow(),
                leftovers
                    .iter()
                    .map(|n| n.display.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for n in &leftovers {
                println!(
                    "    {}",
                    crate::text::sanitize_line(&n.path.display().to_string())
                );
            }
            println!(
                "  {}",
                "Run `agentstack adopt` after this to bring them in.".dimmed()
            );
        }
        // A clean machine is a first-timer's machine. Refusing to create a
        // manifest here is a circular blocker — every other command's error
        // says "run `agentstack init`" — so scaffold a commented starter
        // manifest instead of importing nothing.
        const STARTER: &str = "\
version = 1

# Fresh manifest — no agent CLIs were detected to import from.
# Declare MCP servers here; secrets stay ${REF} placeholders (never values):
#
# [servers.filesystem]
# type = \"stdio\"
# command = \"npx\"
# args = [\"-y\", \"@modelcontextprotocol/server-filesystem\", \"./\"]
#
# Next steps:
#   agentstack search <query>          find servers/skills in the catalog
#   agentstack add from <id> --write   add one to this manifest
#   agentstack apply                   preview what renders into each CLI
#   agentstack x gateway connect --all --write   or skip rendered files entirely:
#   agentstack trust .                 serve this repo through the gateway
";
        if args.dry_run {
            println!("No supported CLIs detected — would write a starter manifest:\n\n{STARTER}");
            return Ok(true);
        }
        // The wizard's consent gate applies to the starter write too: nothing
        // lands without a yes that follows the stated plan.
        if gate_write {
            println!("No supported CLIs detected to import — the next step writes a starter manifest at {}.", manifest_path.display());
            if !crate::util::confirm::confirm("\nWrite the starter manifest now?")? {
                println!("\n{} Nothing written.", "·".dimmed());
                return Ok(false);
            }
        }
        // Capture the pre-write state (the file is absent → `before: None`, so
        // undo deletes it) BEFORE writing, then record one undoable entry — the
        // same ledger `restore` reads. Best-effort: history never breaks init.
        let cap = crate::history::capture(&manifest_path, "manifest · starter");
        if let Err(err) = crate::util::atomic::write(&manifest_path, STARTER)
            .with_context(|| format!("writing {}", manifest_path.display()))
        {
            let _ = crate::history::rollback(std::slice::from_ref(&cap));
            return Err(err);
        }
        if let Err(err) = crate::history::record("project", "init", Vec::new(), vec![cap.clone()]) {
            crate::history::rollback(&[cap]).context(
                "history recording failed and the starter manifest could not be rolled back",
            )?;
            return Err(err).context("recording the starter manifest for undo");
        }
        println!(
            "No supported CLIs detected to import — wrote a starter manifest instead.\n{}  Wrote {}\n\nAdd a server with `agentstack search <query>` + `agentstack add from <id> --write`,\nor edit the manifest directly (it has a commented example).",
            "✅".dimmed(),
            manifest_path.display()
        );
        // `--connect` asked for a registration there is nowhere to make: no
        // supported CLI was detected, so there is no config to put a bridge
        // in. Say so rather than letting the flag pass silently — a flag that
        // parses and then does nothing without a word is how a user comes to
        // believe a machine is wired when it is not.
        if args.connect {
            println!(
                "\n{} --connect had nothing to register: no supported CLI was detected here. \
                 Install one, then run `{GATEWAY_CONNECT}`.",
                "·".dimmed()
            );
        }
        return Ok(true);
    }

    // ── The pre-write review (Stage 1.2): what was found, what imports, and
    // where it lands — every fact stated BEFORE anything is written.
    let project_root = crate::manifest::project_root_of(&dir);
    // Progressive disclosure (2026-08-12): the default states one line per
    // fact and `--verbose` spells the same facts out. Nothing moves behind the
    // flag that changes what the run DOES — only how much of the evidence is
    // enumerated. The consent-critical half (which servers, which secrets,
    // what is written) stays on the default screen, as names on one line.
    if args.verbose {
        print!("{}", render_found_clis(&detected, &project_root));
        if servers.is_empty() {
            println!(
                "{}  No MCP servers found in those configs — importing settings only",
                "📥".dimmed()
            );
        } else {
            print!("{}", render_import_servers(&servers));
        }
    } else {
        print!(
            "{}",
            render_review_headline(detected.len(), &servers, settings.len())
        );
    }
    // "What does this machine have?" and "what does this project use?" are two
    // questions, and `init` used to answer only the first: every detected
    // server went into the project's default toolset, so a first manifest read
    // as a dump of the laptop rather than as this project. The definitions are
    // imported either way — this choice sets the TOOLSET alone. What is left
    // out stays in the library for any project to name, and stays in each CLI's
    // own global config, which `apply` never writes. So a lean answer takes
    // nothing away from the machine.
    let toolset_servers = choose_toolset_servers(&servers, args)?;
    // Lossy imports are explained, never silent: name each entry the import
    // left behind, why, and that nothing was deleted. Names come from other
    // CLIs' config files — hostile input; sanitize before display.
    //
    // Two shapes of skip, one statement. By default they collapse to a single
    // counted line — the fact ("N entries were left behind, nothing deleted")
    // is what a reader must not miss, and the per-entry reasons are evidence
    // for the reader who asks. `--verbose` prints both blocks in full.
    //
    // Which of the imported settings this run will actually declare: the ones
    // the machine layer does not already speak for. Read here, before the
    // review, so the line below and the write cannot disagree — and outside
    // the `--verbose` arm, because the WRITE reads it too.
    let machine_settings = machine_settings_to_declare(&settings);
    if args.verbose {
        for (cli, skip) in &skipped {
            println!(
                "{} not imported from {cli}: '{}' — {}; it stays in {cli}'s own config, \
                 nothing was deleted",
                "⚠".yellow(),
                crate::text::sanitize_line(&skip.name),
                skip.reason
            );
        }
        // Servers another application owns: excluded by default, and named
        // here whichever way it went. `--dry-run` reaches this same review, so
        // the preview and the real run make the identical statement.
        print!(
            "{}",
            render_tool_managed(&tool_managed, tool_managed_included)
        );
        // Ours, not theirs, and not a decision anyone has to make — so it
        // belongs to this evidence block rather than to a warning. Silence
        // would be wrong too: a reader comparing the import against their own
        // config file should be able to account for every entry in it.
        if !bridge_entries.is_empty() {
            println!(
                "{} not imported: {} — agentstack's own gateway bridge, registered by \
                 `{GATEWAY_CONNECT}`; it stays in each CLI's config and serves this project from there",
                "·".dimmed(),
                bridge_entries
                    .iter()
                    .map(|n| crate::text::sanitize_line(n))
                    .collect::<Vec<_>>()
                    .join(", ")
                    .dimmed()
            );
        }
        if !settings.is_empty() {
            let from: Vec<String> = settings
                .keys()
                .map(|id| det_display(&detected, id))
                .collect();
            println!(
                "{}  Importing settings from {}",
                "⚙".dimmed(),
                from.join(" · ")
            );
            println!(
                "      {}",
                "Only settings agentstack understands are imported; every other setting stays in its CLI's own file, untouched.".dimmed()
            );
            // Where they land, and why it is not here. A model choice, a
            // reasoning effort, a theme — these were read from each CLI's
            // MACHINE-WIDE config, and putting them in the project manifest
            // turned a personal preference into a repo file every teammate
            // would render and commit. The machine manifest is that layer
            // (docs/reference.md §"The machine layer"), so that is where they
            // go.
            println!(
                "      {}",
                format!(
                    "They are personal, machine-wide preferences, so they go to your machine layer \
                     ({}) — not into this repo. Declare a project-specific value with \
                     `agentstack x settings set <cli> <key> <value>`.",
                    display_home(&crate::util::paths::agentstack_home())
                )
                .dimmed()
            );
            let already: Vec<String> = settings
                .keys()
                .filter(|id| !machine_settings.contains_key(*id))
                .map(|id| det_display(&detected, id))
                .collect();
            if !already.is_empty() {
                println!(
                    "      {}",
                    format!(
                        "Your machine layer already declares settings for {} — those are kept as \
                         they are, not overwritten.",
                        already.join(" · ")
                    )
                    .dimmed()
                );
            }
        }
    } else if let Some(line) =
        render_skipped_summary(&skipped, &tool_managed, tool_managed_included)
    {
        print!("{line}");
    }

    // Inline secrets were lifted during detection. This is the moment that
    // matters: plaintext tokens were sitting in live CLI configs — show
    // exactly where each one was.
    //
    // The wording is deliberately "copied", not "lifted"/"moved": import reads
    // the source config and never edits it, so the original plaintext is still
    // there afterwards. Saying "lifted" would let a security-conscious reader
    // believe their live config had been cleaned when it hasn't — the one
    // place this output could overstate, so it doesn't.
    //
    // Both the counted default and the itemized `--verbose` form keep the two
    // security sentences whole: the manifest holds references (never values),
    // and each value was COPIED — the original plaintext is still in the CLI's
    // own config. Those are the promises a reader could otherwise misread in
    // the safe direction, so neither is behind a flag.
    if !lifted.is_empty() {
        if args.verbose {
            println!(
                "{}  {} — replaced with secure references here:",
                "🔐".dimmed(),
                format!(
                    "Found {} in your live CLI configs",
                    super::count(lifted.len(), "plaintext token")
                )
                .yellow()
                .bold()
            );
            let width = lifted.iter().map(|l| l.reference.len()).max().unwrap_or(0);
            for l in &lifted {
                println!(
                    "      {} {}  {}",
                    format!("${{{}}}", l.reference).green(),
                    " ".repeat(width.saturating_sub(l.reference.len())),
                    l.origin.dimmed()
                );
            }
            println!(
                "      {}",
                "The manifest stays commit-safe; real values resolve locally at apply time."
                    .dimmed()
            );
            println!(
                "      {}",
                "Each value was COPIED — the original is still in the CLI's own config, unchanged."
                    .dimmed()
            );
        } else {
            print!("{}", render_lifted_summary(&lifted));
        }
    }

    // Where it all lands: the manifest this import writes, and each CLI's
    // native destination a follow-up `apply --write` manages — scope spelled
    // out, printed before any write or consent question (Stage 1.2).
    if args.verbose {
        print!(
            "{}",
            render_managed_files(&manifest_path, &destinations, &project_root)
        );
    }

    // W4: the delivery planner's routing, stated per tool in plain language,
    // in the same pre-write review. A fresh manifest carries no override, so
    // this is the Automatic answer — skills and MCP servers served live to the
    // tools that can take them, everything else written into files.
    // Invariant 8: the routing may only say "served live" when a bridge really
    // is registered. `unconnected_live_harnesses` is empty in exactly the two
    // honest cases (nothing routes live, or the bridge IS registered), so it
    // doubles as the connection reading here.
    // `--connect` is the second honest case's twin: this very run registers the
    // bridge a few lines below, so stating "planned live (not connected)" here
    // would describe a state that ends before the command does. The claim is
    // still bound to enforcement — the flag is what makes the registration
    // happen, and the closing summary re-reads the real per-harness state
    // afterwards, so a registration that failed is reported as such there.
    //
    // ONE routing table per run, and only under `--verbose`. This block, the
    // closing summary's `Delivery:` lines, and the wizard's own Delivery
    // screen used to print the same answer three times in a single `init`.
    // The default keeps the honest one-line reading instead — and invariant 8's
    // "planned live, not connected" disclosure is never inside the flag: it
    // rides the closing summary, which every route reaches.
    if args.verbose && !gate_write {
        print!(
            "{}",
            render_delivery_routing(
                &target_defaults,
                args.connect || unconnected_live_harnesses(&target_defaults, None).is_empty()
            )
        );
    }

    // Counts for the closing summary — `servers`/`settings` move into the
    // manifest below.
    let server_count = servers.len();
    let settings_count = settings.len();

    // Library-first import (`docs/design/linked-library-sources.md` §"What
    // `init` imports"). A server that was already configured globally in three
    // CLIs was never this project's own content: filename-safe definitions go
    // into the first linked library source, and the project references them by
    // name. Namespaced native identifiers such as `upstash/context7` cannot be
    // represented by the library's one-file-per-server layout, so they remain
    // inline without being renamed. `--project-servers` keeps every definition
    // inline for a caller that wants it.
    let (library_servers, inline_servers) = if args.project_servers {
        (IndexMap::new(), servers.clone())
    } else {
        partition_library_servers(&servers)
    };
    let library_import = !library_servers.is_empty();
    if !args.project_servers && !inline_servers.is_empty() {
        let names = inline_servers.keys().cloned().collect::<Vec<_>>();
        print!("{}", render_inline_library_servers(&names));
    }
    let library_source = crate::sources::Sources::load_or_warn().primary();
    let library_root_display = library_source.root.display().to_string();

    // Review finding 3: the library is SHARED state, and a second project's
    // import must not silently rewrite what a first project pinned. Collisions
    // are found before anything is written, shown in the review, and answered
    // by a person; `keep_library` names the servers whose existing definition
    // wins. A non-interactive run answers "keep" without prompting, which is
    // the safe default for automation — `confirm` returns false there.
    let collisions = if library_import {
        library_collisions(&library_source.root, &library_servers)
    } else {
        Vec::new()
    };
    let mut keep_library: Vec<String> = Vec::new();
    if !collisions.is_empty() {
        print!(
            "{}",
            render_library_collisions(&library_source.name, &collisions)
        );
        // A preview asks nothing, and `--yes` is a promise that this run does
        // not stop to ask. Both therefore take the non-destructive answer, and
        // say which answer they took — a scripted `init` must never be the
        // thing that rewrites another project's pinned definition.
        let ask = !args.dry_run && !args.yes;
        for c in &collisions {
            let replace = ask
                && crate::util::confirm::confirm(&format!(
                    "Replace the library's '{}' with the imported definition?",
                    crate::text::sanitize_line(&c.name)
                ))?;
            if replace {
                println!(
                    "  {} '{}' will be replaced — other projects pinned to the old \
                     definition will report drift until they re-lock.",
                    "⚠".yellow(),
                    crate::text::sanitize_line(&c.name)
                );
            } else {
                println!(
                    "  {} keeping the library's '{}' — this project will use it.",
                    "·".dimmed(),
                    crate::text::sanitize_line(&c.name)
                );
                keep_library.push(c.name.clone());
            }
        }
    }

    // A toolset is an answer to "what does THIS task use", so it needs
    // something to name. An import that brought in no server had one
    // synthesized around whatever was left — for a while, around agentstack's
    // own bridge — and a first manifest opened on `[toolsets.default] servers
    // = ["agentstack"]`, which is the tool describing itself rather than the
    // project. `default_toolset` follows the same condition, and must: it is a
    // validation error for it to name a toolset that is not declared.
    let synthesize_toolset = library_import && !toolset_servers.is_empty();
    let (manifest_servers, profiles) = if library_import {
        let mut profiles = IndexMap::new();
        if synthesize_toolset {
            profiles.insert(
                "default".to_string(),
                crate::manifest::Profile {
                    servers: toolset_servers.clone(),
                    ..Default::default()
                },
            );
        }
        (inline_servers.clone(), profiles)
    } else {
        (servers.clone(), IndexMap::new())
    };

    // `[targets]` is a NARROWING. Absent, `apply` targets the CLIs it detects
    // at render time (`render::apply::resolve_targets`), which is exactly what
    // this list says when the import narrowed nothing — so writing it out adds
    // no meaning and pins one machine's inventory into a file that travels to
    // other machines and into a teammate's checkout. Compared as sets: the two
    // lists are built in different orders and the ORDER is not the claim.
    let narrows_targets = {
        let mut pinned = target_defaults.clone();
        let mut detected = detected_ids.clone();
        pinned.sort();
        pinned.dedup();
        detected.sort();
        detected.dedup();
        pinned != detected
    };

    // Assemble the manifest.
    let manifest = Manifest {
        version: 1,
        meta: Meta {
            name: None,
            gitignore: None,
        },
        servers: manifest_servers,
        skills: IndexMap::new(),
        default_toolset: synthesize_toolset.then(|| "default".to_string()),
        profiles,
        instructions: IndexMap::new(),
        // Never here. Imported native settings came from each CLI's
        // MACHINE-WIDE config and are personal preferences; they are declared
        // in the machine manifest by `write_machine_settings` below. A project
        // may still declare its own `[settings.*]` — that is what
        // `x settings set` is for — but an IMPORT must not manufacture one.
        settings: IndexMap::new(),
        hooks: IndexMap::new(),
        extensions: IndexMap::new(),
        workflows: IndexMap::new(),
        packs: IndexMap::new(),
        package_overrides: IndexMap::new(),
        targets: Targets {
            default: if narrows_targets {
                target_defaults.clone()
            } else {
                Vec::new()
            },
        },
        policy: Default::default(),
        guard: Default::default(),
        experimental: Default::default(),
        // Absent = Automatic: the delivery planner routes each capability by
        // kind and harness. `init` never writes an override — the escape hatch
        // is something a person asks for.
        delivery: Default::default(),
    };
    let toml_text = format!(
        "{}{}",
        capability_header(&manifest),
        toml::to_string_pretty(&manifest).context("serializing manifest to TOML")?
    );

    if args.dry_run {
        println!("\n{} (preview — nothing written)\n", MANIFEST_FILE.bold());
        println!("{toml_text}");
        if !machine_settings.is_empty() {
            println!(
                "Would declare {} in your machine layer ({}); this project's manifest carries \
                 no settings of its own.",
                super::count(machine_settings.len(), "CLI's settings"),
                display_home(&crate::util::paths::agentstack_home())
            );
        }
        if library_import {
            println!(
                "Would write {} into library source '{}' ({}); the manifest above references \
                 them by name.",
                super::count(library_servers.len(), "server definition"),
                library_source.name,
                library_root_display
            );
        }
        if !lifted.is_empty() {
            // A preview never prompts, so resolve the store non-interactively.
            match preresolved_store.map_or_else(|| resolve_secret_store(args, false), Ok)? {
                SecretStore::Env => println!(
                    "Would store {} in .env (gitignored).",
                    super::count(lifted.len(), "secret")
                ),
                SecretStore::Keychain => {
                    println!(
                        "Would store {} in the OS keychain.",
                        super::count(lifted.len(), "secret")
                    )
                }
                SecretStore::Skip => {
                    let values = if lifted.len() == 1 {
                        "value not stored"
                    } else {
                        "values not stored"
                    };
                    println!(
                        "Would write {}; {values} (--secrets skip).",
                        super::count(lifted.len(), "${REF} placeholder")
                    )
                }
            }
        }
        if args.connect {
            // A preview never writes, and that includes the machine-wide half.
            // Naming the exact files keeps `--dry-run --connect` a real
            // preview of the consent the flag carries; the diff itself is one
            // command away (`agentstack x gateway connect --all`, dry-run by
            // default), which is where a per-file review belongs.
            println!(
                "Would also register the agentstack bridge in the CLIs installed here \
                 (their own global config files); nothing was written."
            );
        }
        return Ok(true);
    }

    // The wizard's consent gate: the review above is the evidence; this is
    // the one question. Declining writes nothing (the caller closes the run).
    //
    // This question is the ONE place the untouched-configs promise is made on
    // the guided route: the review above used to repeat it three times, once
    // per block, which is how a promise stops being read. The question itself
    // stays a question, and its default stays no.
    if gate_write
        && !crate::util::confirm::confirm(&format!(
            "\nImport into {}? Only the manifest and any lifted token values are \
             written — your CLIs' own configs stay untouched.",
            display_path(&manifest_path, &project_root)
        ))?
    {
        println!("\n{} Nothing written.", "·".dimmed());
        return Ok(false);
    }

    // Every file init writes is captured (pre-write) into `backups`, then
    // recorded as ONE undoable history entry below — the same ledger `restore`
    // reads (P30). Capturing before each write is what lets undo restore the
    // exact prior bytes (or delete a file that did not exist before).
    let mut backups: Vec<crate::history::FileChange> = Vec::new();
    let mut keychain_changes: Vec<KeychainChange> = Vec::new();
    let mut secret_notice: Option<String> = None;
    // `${REF}`s whose values are NOT stored anywhere after this init (the skip
    // store, or a keychain that refused a write) — the success summary names
    // each one so "what still needs a value" is never buried in scrollback.
    let mut refs_needing_values: Vec<String> = Vec::new();

    let writes = (|| -> Result<()> {
        // Store lifted secret VALUES in the chosen backend (P2). The manifest
        // only ever holds `${REF}` placeholders. File captures and temporary
        // keychain snapshots make every pre-commit mutation reversible if a
        // later write or the history record fails.
        if !lifted.is_empty() {
            // A consented apply must store into the digested destination —
            // never re-prompt into a different one.
            match preresolved_store.map_or_else(|| resolve_secret_store(args, true), Ok)? {
                SecretStore::Keychain => {
                    let (unstored, changes) = store_lifted_reversibly(&lifted);
                    let stored = changes.len();
                    keychain_changes = changes;
                    if stored > 0 {
                        secret_notice = Some(format!(
                            "{}  Stored {} in the OS keychain (service `agentstack`)",
                            "🔑".dimmed(),
                            super::count(stored, "token")
                        ));
                    }
                    if !unstored.is_empty() {
                        report_unstored_keychain(&unstored);
                        refs_needing_values = unstored;
                    }
                }
                SecretStore::Env => {
                    let entries: Vec<(String, String)> = lifted
                        .iter()
                        .map(|l| (l.reference.clone(), l.value.clone()))
                        .collect();
                    backups.push(crate::history::capture(
                        &dir.join(".env"),
                        ".env · lifted secrets",
                    ));
                    env_file::write(&dir, &entries)?;
                    let project_root = crate::manifest::project_root_of(&dir);
                    let is_git = project_root.join(".git").exists();
                    if is_git {
                        // Capture before attempting the write. If it was already
                        // ignored, remove the unused capture from the transaction.
                        backups.push(crate::history::capture(
                            &project_root.join(".gitignore"),
                            ".gitignore · .env rule",
                        ));
                        if !env_file::ensure_gitignored(&project_root, &dir, true)? {
                            backups.pop();
                        }
                    }
                    secret_notice = Some(format!(
                        "{}  Stored {} in .env{}",
                        "🔑".dimmed(),
                        super::count(entries.len(), "token"),
                        if is_git { " (gitignored)" } else { "" }
                    ));
                }
                SecretStore::Skip => {
                    report_skipped(&lifted);
                    refs_needing_values = lifted.iter().map(|l| l.reference.clone()).collect();
                }
            }
            // Varlock is the recommended vault, and `.env.schema` is how a
            // project opts into it. Offer it here, where the names this project
            // needs have just been discovered. Whatever the user answers, the
            // chain and `${REF}` resolution are unchanged: this only adds a
            // layer between env and the keychain.
            let names: Vec<String> = lifted.iter().map(|l| l.reference.clone()).collect();
            if let Some(change) = offer_env_schema(&dir, &names)? {
                backups.push(change);
            }
        }

        // The imported servers land in the first linked library source, through
        // the one library write path (`lib::add_server_def`) rather than a
        // second importer. Each definition file is captured first, so this stays
        // inside init's single undoable transaction: a failure here rolls the
        // library writes back along with the manifest.
        if library_import {
            // Capture BEFORE any library write — a capture taken afterwards
            // would record the new bytes and make undo a no-op.
            backups.push(crate::history::capture(
                &crate::library::Library::path(&library_source.root),
                "library · index",
            ));
            for (name, server) in &library_servers {
                // Finding 3: a name the user chose to keep is NOT written. The
                // project still references it, and resolution serves the
                // library's existing definition — which is what the review said
                // would happen.
                if keep_library.contains(name) {
                    continue;
                }
                let dest = crate::resolve::library_server_path(&library_source.root, name);
                backups.push(crate::history::capture(&dest, format!("library · {name}")));
                super::lib::add_server_def(
                    &library_source.root,
                    name,
                    server,
                    format!("init:{}", crate::text::sanitize_line(&library_source.name)),
                    // `replace` is now an ANSWERED question, not a default. A
                    // differing definition reached this line only because a
                    // person said yes to replacing it above; an identical one
                    // is not a replacement at all.
                    true,
                    true,
                )?;
            }
        }

        // The machine layer's half of the import, written through the same
        // transaction: a failure below rolls this back with everything else.
        write_machine_settings(&machine_settings, &mut backups)?;

        backups.push(crate::history::capture(&manifest_path, "manifest · import"));
        crate::util::atomic::write(&manifest_path, &toml_text)
            .with_context(|| format!("writing {}", manifest_path.display()))?;

        // A manifest that references library capabilities by name needs pins
        // before anything can serve them, and pinning is the machine's job
        // (STRATEGY.md §"The design law": the manifest and lock are
        // system-maintained; the one thing never automated is the yes). Without
        // this the library-first import would end on "library server, not
        // locked" and cost the user two extra commands for a decision they
        // never had to make. Inline-only imports pin nothing and skip it.
        if library_import {
            backups.push(crate::history::capture(
                &dir.join(agentstack_core::lock::LOCK_FILE),
                "lockfile · import",
            ));
            super::lock::run(
                &crate::cli::LockArgs {
                    quiet: true,
                    ..Default::default()
                },
                Some(&base),
            )
            .context("pinning the imported library servers")?;
        }
        Ok(())
    })();

    if let Err(err) = writes {
        let file_rollback = crate::history::rollback(&backups);
        let keychain_rollback = rollback_keychain(&keychain_changes);
        if let Err(rollback_err) = file_rollback.and(keychain_rollback) {
            return Err(err).context(format!(
                "initialization failed and rollback also failed: {rollback_err:#}"
            ));
        }
        return Err(err).context("initialization failed; completed writes were rolled back");
    }

    // The history record is part of the commit contract. If it cannot be made,
    // restore the files and temporary keychain changes instead of claiming an
    // undo that does not exist.
    //
    // `display_names`, not `detected_ids`: the ledger's `targets` feed
    // straight into the recorded summary `restore` prints, and an adapter id
    // like `claude-code` there (instead of "Claude Code") was review finding
    // H7's second bug — undo history naming internal ids on some rows and
    // display names on others.
    if let Err(err) =
        crate::history::record("project", "init", display_names.clone(), backups.clone())
    {
        let file_rollback = crate::history::rollback(&backups);
        let keychain_rollback = rollback_keychain(&keychain_changes);
        if let Err(rollback_err) = file_rollback.and(keychain_rollback) {
            return Err(err).context(format!(
                "recording initialization history failed and rollback also failed: {rollback_err:#}"
            ));
        }
        return Err(err)
            .context("recording initialization history failed; writes were rolled back");
    }

    // H1: a consented import also records trust for the manifest it just wrote,
    // so a newcomer never meets the trust gate as an error in their own repo.
    //
    // Why this is not a hole — and where its boundary is (F7). The grant
    // covers ONLY a manifest built from the user's own machine-global CLI
    // configs (`~/.claude.json`, `~/.codex/config.toml`, …), importing servers
    // and settings only: no skills, workflows, extensions, or instructions.
    // Since project-scope discovery landed (v0.17.1), `detect_import` ALSO
    // reads a repo's own `.mcp.json` — bytes that arrive with a clone. When
    // any of those landed in the manifest (`det.project_sourced`), this grant
    // is withheld and the project meets the ordinary `agentstack trust .`
    // review: a documented `init --yes` in automation must never promptlessly
    // bless a stdio command line the repository authored (invariant 6 — one
    // gated grant path over repo content, no shortcut around it). `init` also
    // refuses outright when a manifest already exists.
    //
    // Best-effort by construction: a failure to record trust must not undo a
    // good import. The cost of not granting is one `agentstack trust` prompt.
    //
    // Deliberately silent when it grants. Stage 1.4's witness
    // (`ordinary_journey_vocab`) holds the ordinary journey to no trust
    // vocabulary at all until the user reaches for it. The withheld case is
    // the one moment the boundary IS relevant, so it says so once, with the
    // exact next command — progressive disclosure rule 3, not a violation of
    // the vocabulary witness (whose journey has no project-scope config).
    let trusted = if project_sourced {
        false
    } else {
        grant_trust_for_import(&base, &toml_text, &manifest, library_import)
    };
    // The withheld grant is stated once. On the scripted route the close owns
    // it — `agentstack trust .` becomes that run's single `Next:` — so saying
    // it here too would be the same instruction twice, three lines apart.
    if project_sourced && !show_next {
        println!(
            "  {} servers from this repo's own config were imported — review what they run: `agentstack trust .`",
            "·".dimmed()
        );
    }
    let _ = trusted;

    if let Some(notice) = secret_notice {
        println!("{notice}");
    }

    println!("{}  Wrote {}", "✅".dimmed(), manifest_path.display());

    // `--connect` — the one step that makes the default (live) lane deliver
    // anything. It runs HERE, before the closing summary, so the summary's
    // per-harness bridge reading is the state this run actually left behind
    // instead of a "NOT YET CONNECTED" line contradicted three lines later.
    //
    // Consent: the flag is the consent, and there is no other trigger. Nothing
    // infers it — not `--yes`, not an interactive terminal, not a detected
    // harness. The wizard route never reaches this branch (it passes
    // `connect: false` and asks/states it in its own ceremony), so a single
    // registration happens on either route, never two.
    let bridge_registered_now = args.connect && register_bridge();

    if show_next {
        // The one concise success summary (Stage 1.2): manifest path, source
        // CLIs, what was imported, which secrets still need values, and the
        // exact next commands. The wizard has its own richer close, so this
        // prints only on the scripted primitive.
        // The honest source list: only CLIs that contributed content, by
        // display name. A run that imported nothing falls back to what was
        // detected — the same fallback `target_defaults` makes.
        let sources: Vec<String> = if contributing.is_empty() {
            display_names.clone()
        } else {
            contributing
                .iter()
                .map(|id| det_display(&detected, id))
                .collect()
        };
        print!(
            "{}",
            ImportSummary {
                manifest_path: &manifest_path.display().to_string(),
                sources: &sources,
                server_count,
                settings_count,
                needing_values: &refs_needing_values,
                also_detected: &also_detected,
                // Every file this import read lives inside the project root.
                sources_are_project_scope: detected
                    .iter()
                    .flat_map(|c| c.configs.iter())
                    .all(|p| p.starts_with(&project_root)),
                unconnected_live: &unconnected_live_harnesses(&target_defaults, Some(&manifest)),
                library_dest: library_import.then_some((
                    library_source.name.as_str(),
                    library_root_display.as_str(),
                    library_servers.len(),
                    inline_servers.len(),
                )),
                servers_rendered: renders_servers(&target_defaults, &manifest),
                rendered_work: renders_anything(&target_defaults, &manifest, server_count),
                bridge_registered_now,
                needs_trust: project_sourced,
                verbose: args.verbose,
            }
            .render()
        );
    }
    Ok(true)
}

/// Register the bridge for `init --connect`, on the scripted route.
///
/// Straight through [`super::connect::run_connect`] — the shipped
/// `gateway connect` path, called as a library exactly as the wizard's
/// `offer_bridge` calls it. There is no second registration writer, so the two
/// routes cannot disagree about what lands in a harness config, and the write
/// keeps `gateway connect`'s own undo entry.
///
/// Failure is reported, never fatal: an import that succeeded must not be
/// reported as a failure because no harness here can host a bridge. The manual
/// command is named so the run still ends with a usable next step — the same
/// rule the wizard's offer follows.
///
/// Returns whether the registration ran clean, which is the only thing the
/// closing summary may treat as "these configs now carry the bridge". A failure
/// wrote nothing, so the summary keeps its ordinary untouched-configs note.
fn register_bridge() -> bool {
    match super::connect::run_connect(&ConnectArgs {
        harnesses: Vec::new(),
        all: true,
        transparent: false,
        write: true,
        command: None,
    }) {
        Ok(()) => true,
        Err(err) => {
            println!(
                "{} bridge registration failed ({err:#}) — register it later with:\n    {}",
                "⚠".yellow(),
                GATEWAY_CONNECT.bold()
            );
            false
        }
    }
}

/// Record trust for the manifest `init` just wrote (review finding H1).
///
/// Digested from the bytes we wrote, never a disk re-read — the rule `apply`'s
/// re-pin already follows. If anything edits the manifest between that write
/// and this grant, the store holds OUR digest, the project immediately reads
/// `Changed`, and every use site fails closed, rather than silently blessing
/// bytes nobody reviewed.
///
/// Fails closed on the two files `init` does NOT write. An
/// `agentstack.local.toml` overlay or an `agentstack.lock` that was already on
/// disk (reachable under `--force`) is content this import never showed anyone,
/// and both are part of the consent digest — so when either exists we grant
/// nothing and leave the project to the normal `agentstack trust` review.
///
/// Goes through [`trust::trust_reviewed`], the existing constructor for "record
/// trust at the digest of the snapshot whose review the caller just rendered".
/// No second grant constructor is introduced (workspace invariant 6), and
/// `trust_unreviewed` — the deliberately greppable test-only path — is not used.
///
/// Returns whether trust was actually recorded, so the closing summary can say
/// so honestly instead of the user discovering it later.
fn grant_trust_for_import(
    base: &Path,
    manifest_bytes: &str,
    manifest: &Manifest,
    // True when this same import wrote the lockfile (the library-first path).
    // A lock THIS run produced from the manifest it just showed is part of what
    // was reviewed; a lock that was merely lying on disk is not, and still
    // fails closed below.
    wrote_lock: bool,
) -> bool {
    let dir = crate::manifest::resolve_manifest_dir(base);
    let local = dir.join(crate::manifest::load::LOCAL_FILE);
    let lock = dir.join(agentstack_core::lock::LOCK_FILE);
    if local.exists() {
        return false;
    }
    let lock_bytes = match (wrote_lock, lock.exists()) {
        // Our own pins, read back from the file we just wrote — the same rule
        // the manifest half follows (bind to the bytes on disk now, so a later
        // edit reads `Changed` instead of being silently blessed).
        (true, true) => match std::fs::read(&lock) {
            Ok(bytes) => Some(bytes),
            Err(_) => return false,
        },
        (_, true) => return false,
        (_, false) => None,
    };

    // The snapshot this grant binds to: our bytes, and the explicit absence of
    // the ones we did not write — which is what `ConsentSnapshot::read` would
    // observe a moment from now, so `digest_for` agrees and the project reads
    // `Trusted`.
    let snapshot = crate::trust::ConsentSnapshot {
        manifest: manifest_bytes.as_bytes().to_vec(),
        local: None,
        lock: lock_bytes,
    };

    // The reviewed surface, in the same shape `trust`'s own review records, so
    // a later re-trust diffs against it instead of seeing an unrecognized
    // baseline: per-server identity is the command line for stdio (what
    // actually runs) or the URL for http, plus one aggregate line for the
    // secret refs.
    //
    // Resolved, not read off `manifest.servers` (review finding 2). Library-first
    // import moves every imported server OUT of `[servers.*]` and into a linked
    // library source, leaving the default toolset to reference them by name — so
    // reading the manifest map here recorded an EMPTY surface on the default
    // path, while the digest above blessed every one of those servers through
    // the lock. A re-gate then had nothing to diff against: each server could
    // only ever read `+ added`, never `~ changed`, which is the one thing the
    // diff exists to say. `effective_runtime_servers` is the resolver `trust`'s
    // own review walks and the gateway serves from, so this surface names the
    // definitions that will actually run.
    let library = crate::library::Library::load_default_or_warn();
    let lib_home = crate::util::paths::lib_home();
    let resolved = crate::resolve::effective_runtime_servers(manifest, &library, &lib_home, None);
    let mut surface: Vec<crate::trust::SurfaceItem> = Vec::with_capacity(resolved.len());
    for (name, r) in resolved {
        // Fail closed. A server that does not resolve is a server this grant
        // cannot describe, and recording a surface that is missing an entry the
        // digest blesses is exactly the defect above. Withhold the grant and
        // let the ordinary `agentstack trust` review ask the question — the
        // documented cost of not granting is one prompt.
        let Ok(r) = r else {
            return false;
        };
        surface.push(crate::trust::SurfaceItem {
            kind: "server".to_string(),
            name,
            // The SAME two helpers `trust`'s review marks with, so the baseline
            // recorded here and the identity computed at the next re-gate are
            // byte-identical by construction.
            identity: match r.server.server_type {
                crate::manifest::ServerType::Stdio => {
                    super::trust::server_stdio_identity(&r.server)
                }
                crate::manifest::ServerType::Http => {
                    super::trust::server_http_identity(&r.server).to_string()
                }
            },
            // `None`, matching what `trust`'s review records for a server:
            // `identity` is the diff key, and a server has no body of bytes for
            // a re-gate to diff. Recording a pin here that the review does not
            // would make every server read `~ changed` on the first re-trust.
            pin: None,
        });
    }
    let refs = manifest.referenced_secrets();
    if !refs.is_empty() {
        surface.push(crate::trust::SurfaceItem {
            kind: "secrets".to_string(),
            name: String::new(),
            identity: refs.join(", "),
            // An aggregate row over the referenced refs; nothing to pin.
            pin: None,
        });
    }

    crate::trust::trust_reviewed(base, snapshot.digest(), surface).is_ok()
}

/// Every display fact the scripted-import close states, gathered so the
/// formatter stays pure (and unit-testable without touching real CLI configs)
/// while the caller names each value once.
///
/// It became a struct when progressive disclosure added the last two fields:
/// fifteen positional booleans past a `#[allow(too_many_arguments)]` is a
/// formatter nobody can call correctly, and `..Default::default()` is what
/// keeps each test naming only the facts it is about.
#[derive(Default)]
struct ImportSummary<'a> {
    manifest_path: &'a str,
    sources: &'a [String],
    server_count: usize,
    settings_count: usize,
    needing_values: &'a [String],
    also_detected: &'a [String],
    /// True when every source config this import read is a project-scope file —
    /// i.e. exactly the files `apply --write` will manage here. The
    /// two-uncoordinated-places note is then false, and pointing at
    /// `--scope global` would send the user to write machine-wide files they
    /// never asked about.
    sources_are_project_scope: bool,
    /// Display names of harnesses the plan routes to the LIVE lane while the
    /// bridge is registered in no detected CLI. Non-empty means "planned live,
    /// delivering nothing" — invariant 8 forbids the ordinary "served live"
    /// wording there, so this replaces the routing lines above, at every
    /// verbosity.
    unconnected_live: &'a [String],
    /// Where the imported servers landed: the linked library source's name and
    /// folder, or `None` when `--project-servers` kept them inline.
    library_dest: Option<(&'a str, &'a str, usize, usize)>,
    /// Will `apply --write` write a server config anywhere? False for an
    /// ordinary project of MCP-capable tools, where the servers travel live.
    servers_rendered: bool,
    /// Is there ANY rendered-lane work here? When false, `apply --write` writes
    /// nothing and must not be offered as the next step.
    rendered_work: bool,
    /// Did THIS run register the bridge (`--connect`)? Then the source configs
    /// are no longer untouched — each gained exactly one entry — and the note
    /// below must say which, or it contradicts the diff printed above it.
    bridge_registered_now: bool,
    /// Did this import take servers out of the repository's own config, so the
    /// trust grant was withheld? Then the review is the single most relevant
    /// next step, and it takes the one `Next:` line.
    needs_trust: bool,
    /// Spell every fact out (`--verbose`) rather than stating it as a count.
    verbose: bool,
}

impl ImportSummary<'_> {
    /// The scripted-import close.
    ///
    /// Default shape: headline, the manifest, what landed, what still blocks
    /// the setup, ONE `Next:` line, and one compact line of everything else.
    /// `--verbose` re-adds the source list, the left-out CLIs' explanation, the
    /// per-tool routing, and the long-form library sentence.
    fn render(&self) -> String {
        let mut out = String::new();
        // The headline is the one line a reader is guaranteed to take away, so
        // it must not say "complete" about a setup that delivers nothing. When
        // the live lane has no bridge anywhere, this run imported and stopped —
        // the wizard's close says exactly that ("Setup imported, not yet
        // delivering"), and the scripted close agrees with it instead of
        // opening with a success the Delivery line below then retracts.
        if self.unconnected_live.is_empty() {
            out.push_str("\nImport complete.\n");
        } else {
            out.push_str("\nImported, and not yet delivering.\n");
        }
        out.push_str(&format!("  Manifest:  {}\n", self.manifest_path));
        if self.verbose {
            out.push_str(&format!("  From:      {}\n", self.sources.join(" · ")));
        }
        let mut imported = super::count(self.server_count, "MCP server");
        if self.settings_count > 0 {
            imported.push_str(&format!(
                " · settings from {}",
                super::count(self.settings_count, "CLI")
            ));
        }
        // Library-first: say where the reusable half went, so nobody has to
        // guess why the manifest lists names instead of commands. One clause on
        // the Imported line by default; the full sentence under --verbose.
        if let Some((name, root, library_count, inline_count)) = self.library_dest {
            if self.verbose {
                out.push_str(&format!("  Imported:  {imported}\n"));
                if inline_count == 0 {
                    out.push_str(&format!(
                        "  Library:   the servers landed in '{name}' ({root}); the manifest\n\
                         \x20            references them by name, so this project stays clean\n"
                    ));
                } else {
                    out.push_str(&format!(
                        "  Library:   {} landed in '{name}' ({root}); {} with native names that\n\
                         \x20            cannot be library filenames stayed inline, unchanged\n",
                        super::count(library_count, "server"),
                        super::count(inline_count, "server")
                    ));
                }
            } else if inline_count == 0 {
                out.push_str(&format!(
                    "  Imported:  {imported} → library '{name}', referenced by name\n"
                ));
            } else {
                out.push_str(&format!(
                    "  Imported:  {imported} → library '{name}' ({} kept inline, unchanged)\n",
                    super::count(inline_count, "server")
                ));
            }
        } else {
            out.push_str(&format!("  Imported:  {imported}\n"));
        }
        // M4: the CLIs deliberately left out of `[targets] default`. Naming them
        // is what keeps "we only targeted two of your six tools" from looking
        // like a detection failure; WHY, and the two ways to add one, are the
        // evidence behind that and live under --verbose.
        if !self.also_detected.is_empty() {
            if self.verbose {
                out.push_str(&format!(
                    "  Also seen: {} — installed, but no config to import, so not\n\
                     \x20            targeted yet. Add one to [targets].default in the manifest, or\n\
                     \x20            render to it once: agentstack apply --target <id> --write\n",
                    self.also_detected.join(" · ")
                ));
            } else {
                out.push_str(&format!(
                    "  Also seen: {} — no config to import, so not targeted\n",
                    self.also_detected.join(" · ")
                ));
            }
        }
        if !self.needing_values.is_empty() {
            let verb = if self.needing_values.len() == 1 {
                "needs"
            } else {
                "need"
            };
            out.push_str(&format!(
                "  Secrets:   {} still {verb} a value before this setup can run:\n",
                self.needing_values.len()
            ));
            for name in self.needing_values {
                out.push_str(&format!("               agentstack secret set {name}\n"));
            }
        }
        // Import reads the CLIs' own configs and never edits them, so the
        // entries it copied still live there. This is the scripted route's one
        // statement of that promise (the guided route makes it in its confirm),
        // so it is never behind the flag — only compressed to a single line.
        if self.server_count > 0 && self.bridge_registered_now {
            // `--connect` just edited these very files, so "unchanged" would be
            // a lie told directly under the diff that changed them. "Now carry"
            // is the reading that stays true whether the entry landed in this
            // run or was already there (a re-import on a connected machine).
            out.push_str(
                "  Note:      those CLI configs now carry one agentstack entry — the bridge; no\n\
                 \x20            server was copied back, they are served from this manifest.\n",
            );
        } else if self.server_count > 0 && !self.servers_rendered {
            // The double-delivery note is false now: `apply` honours the
            // delivery planner, so these servers are never copied into a native
            // config again. The manifest is the one description of them.
            out.push_str(
                "  Note:      the CLI configs above are unchanged — nothing copies these servers\n\
                 \x20            back into them; they are served from this manifest.\n",
            );
        } else if self.server_count > 0 && !self.sources_are_project_scope {
            out.push_str(
                "  Note:      the CLI configs above are unchanged — after `apply --write` these\n\
                 \x20            servers are described in two places (agentstack apply --scope\n\
                 \x20            global --write manages the originals from this manifest too).\n",
            );
        } else if self.server_count > 0 {
            out.push_str(
                "  Note:      those config files are this project's own — `apply --write`\n\
                 \x20            manages them from this manifest, so there is no second copy.\n",
            );
        }
        // Invariant 8, at every verbosity: the live lane is planned and inert,
        // so the summary states that rather than a delivery that is not
        // happening. The repair command is the `Next:` line below.
        //
        // The per-tool routing that used to follow this branch is gone from
        // here: it is the same table `render_delivery_routing` prints in the
        // pre-write review, and one run printed both plus the wizard's own
        // Delivery screen. The review's copy is the one that survives — routing
        // is evidence for a decision the reader has not made yet — so this line
        // says only what a summary must never leave unsaid.
        if !self.unconnected_live.is_empty() {
            out.push_str(&format!(
                "  Delivery:  planned live for {} — NOT YET CONNECTED, so nothing is served yet\n",
                self.unconnected_live.join(", ")
            ));
        }

        // ONE `Next:`, and it is the single most relevant step — a list of
        // three "next" commands is a list of none. Order of relevance: a
        // withheld trust grant blocks everything downstream, an unregistered
        // bridge means nothing is served, then the rendered lane, then the
        // check. Everything else goes on the compact `Then:` line, which is
        // where `x delivery`, `apply --write` and undo stay reachable.
        //
        // `gateway` and `trust` are mechanism nouns the ordinary journey
        // normally suppresses (`tests/ordinary_journey_vocab.rs`); both appear
        // only in the state that earns them, which is the same carve-out that
        // file already makes for the "NOT YET CONNECTED" disclosure.
        let (next, why) = if self.needs_trust {
            (
                "agentstack trust .",
                "review what this repo's own servers run",
            )
        } else if !self.unconnected_live.is_empty() {
            (
                "agentstack x gateway connect --all --write",
                "start serving what routes live",
            )
        } else if self.rendered_work {
            (
                "agentstack apply --write",
                "write the files your tools read",
            )
        } else {
            ("agentstack doctor", "check the result")
        };
        out.push_str(&format!("  Next:      {next}   ({why})\n"));

        let mut also: Vec<String> = Vec::new();
        if self.needs_trust && !self.unconnected_live.is_empty() {
            also.push("agentstack x gateway connect --all --write".to_string());
        }
        if self.rendered_work && next != "agentstack apply --write" {
            also.push("agentstack apply --write".to_string());
        }
        if next != "agentstack doctor" {
            also.push("agentstack doctor".to_string());
        }
        also.push("undo: agentstack x restore --last --write".to_string());
        out.push_str(&format!("  Then:      {}\n", also.join("  ·  ")));
        // The same import in ONE command next time: a script author needs the
        // flag that stops the not-yet-delivering gap from happening at all.
        if !self.unconnected_live.is_empty() {
            out.push_str(
                "             (or import and register in one step: agentstack init --connect)\n",
            );
        }
        if !self.verbose {
            out.push_str(
                "             the config paths, the per-tool routing and every skipped entry: \
                 --verbose\n",
            );
        }
        // Toolsets are deliberately NOT offered here (review finding H3). Import
        // is the moment a user has just learned what the manifest is; a
        // first-time user with a handful of servers has nothing to subset yet,
        // and naming a subset is a question that only becomes real once they
        // have felt the whole set be wrong for a task. The recurring loop is
        // taught where it is needed instead — `doctor`'s next action, and
        // `session start`'s own empty-state hint.
        out
    }
}

/// Compact an absolute path for display: inside the project → relative to the
/// project root; under `$HOME` → `~/…`; otherwise unchanged. Display-only —
/// JSON contracts always carry the full path. Shared with the session
/// start/end reports, which state the same kinds of native paths.
pub(crate) fn display_path(path: &Path, project_root: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(project_root) {
        return rel.display().to_string();
    }
    if let Some(home) = dirs::home_dir() {
        if let Ok(rel) = path.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    path.display().to_string()
}

/// How many imported server names the default review spells out before it
/// stops counting. Past this the line stops being readable and the names are
/// evidence for `--verbose`, not the fact.
const NAMES_INLINE: usize = 6;

/// The default review's opening line: how many CLIs were read, and exactly
/// what is coming out of them.
///
/// Stage 1.2's claim — the import states its evidence BEFORE writing — is kept
/// whole here rather than traded for brevity: the server NAMES are what a
/// person consents to, so they stay on the default screen. What moved behind
/// `--verbose` is the per-CLI config-path table and what each server runs, both
/// of which are evidence for the names rather than the names themselves.
///
/// Pure (no color), and every name comes from another CLI's config file —
/// hostile input — so each is sanitized and the list is bounded.
fn render_review_headline(
    cli_count: usize,
    servers: &IndexMap<String, Server>,
    settings_count: usize,
) -> String {
    let mut imported = format!("importing {}", super::count(servers.len(), "MCP server"));
    if !servers.is_empty() {
        let shown: Vec<String> = servers
            .keys()
            .take(NAMES_INLINE)
            .map(|n| crate::text::truncate_chars(&crate::text::sanitize_line(n), 40))
            .collect();
        let more = if servers.len() > NAMES_INLINE {
            format!(" + {} more", servers.len() - NAMES_INLINE)
        } else {
            String::new()
        };
        imported.push_str(&format!(" ({}{more})", shown.join(" · ")));
    }
    if settings_count > 0 {
        imported.push_str(&format!(
            " + settings from {}",
            super::count(settings_count, "CLI")
        ));
    }
    format!(
        "🔍  Found {} · {imported}\n",
        super::count(cli_count, "coding tool"),
    )
}

/// The default review's one line for everything this import deliberately left
/// behind: entries no reader could parse, and servers another application owns.
///
/// Both shapes collapse into one counted statement because the fact that
/// matters is identical for both — something was left out, and nothing was
/// deleted. The per-entry reasons are the evidence, and `--verbose` prints
/// them. `None` when there was nothing to leave behind, so the line is never
/// an empty one.
fn render_skipped_summary(
    skipped: &[(String, crate::adapter::SkippedImport)],
    tool_managed: &[ToolManagedServer],
    tool_managed_included: bool,
) -> Option<String> {
    let owned = if tool_managed_included {
        0
    } else {
        tool_managed.len()
    };
    let total = skipped.len() + owned;
    if total == 0 {
        return None;
    }
    let reason = match (skipped.is_empty(), owned == 0) {
        (true, _) => "owned by the apps that installed them",
        (_, true) => "could not be read",
        _ => "app-owned or unreadable",
    };
    // `super::count` pluralizes by appending "s"; "entry" does not take one.
    let noun = if total == 1 { "entry" } else { "entries" };
    Some(format!(
        "⚠  {total} {noun} not imported ({reason}) — they stay in each CLI's own config, \
         nothing was deleted; details: --verbose\n"
    ))
}

/// The default review's one line for the plaintext tokens this import found.
///
/// Both security sentences survive the compression, because both are places a
/// reader could otherwise be wrong in the dangerous direction: the manifest
/// holds `${REF}` placeholders (never values), and each value was COPIED — the
/// original is still sitting in the CLI's own config. Only the per-token origin
/// table moved behind `--verbose`.
fn render_lifted_summary(lifted: &[Lifted]) -> String {
    let refs: Vec<String> = lifted
        .iter()
        .take(NAMES_INLINE)
        .map(|l| format!("${{{}}}", l.reference))
        .collect();
    let more = if lifted.len() > NAMES_INLINE {
        format!(" + {} more", lifted.len() - NAMES_INLINE)
    } else {
        String::new()
    };
    format!(
        "🔐  {} in your live CLI configs → {}{more} here; each value was COPIED, the original\n\
         \x20   is still in its CLI's own config, unchanged\n",
        super::count(lifted.len(), "plaintext token"),
        refs.join(" · ")
    )
}

/// Stage 1.2 first screen: every detected CLI with the evidence — the exact
/// native config files found on disk, or the honest "binary only" fact. Pure
/// (no color), so the shape is unit-testable.
fn render_found_clis(detected: &[DetectedCli], project_root: &Path) -> String {
    let mut out = String::new();
    let pronoun = if detected.len() == 1 { "its" } else { "their" };
    out.push_str(&format!(
        "🔍  Found {} and {pronoun} native configs:\n",
        super::count(detected.len(), "coding tool")
    ));
    let width = detected.iter().map(|c| c.display.len()).max().unwrap_or(0);
    for c in detected {
        let facts = if c.configs.is_empty() {
            if c.bin_on_path {
                "binary on PATH — no config files found".to_string()
            } else {
                "no config files found".to_string()
            }
        } else {
            c.configs
                .iter()
                .map(|p| display_path(p, project_root))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        out.push_str(&format!("      {:width$}   {facts}\n", c.display));
    }
    out
}

/// Stage 1.2: the servers this import brings in, BY NAME with what each runs
/// or contacts — shown before anything is written. Names/targets come from
/// other CLIs' config files (hostile input): sanitized and bounded.
fn render_import_servers(servers: &IndexMap<String, Server>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "📥  Importing {} from those configs:\n",
        super::count(servers.len(), "MCP server")
    ));
    let width = servers
        .keys()
        .map(|n| crate::text::sanitize_line(n).len())
        .max()
        .unwrap_or(0)
        .min(30);
    for (name, s) in servers {
        let (kind, target) = server_kind_target(s);
        let verb = if kind == "http" { "contacts" } else { "runs" };
        out.push_str(&format!(
            "      {:width$}   {verb} {}\n",
            crate::text::sanitize_line(name),
            crate::text::truncate_chars(&crate::text::sanitize_line(&target), 64),
        ));
    }
    out
}

/// Stage 1.2: the files this setup will manage, in user terms — the manifest
/// (written by the import itself) and each CLI's native destination with its
/// scope spelled out ("this project" / "machine-wide"), no adapter vocabulary.
fn render_managed_files(
    manifest_path: &Path,
    destinations: &[PlanDestination],
    project_root: &Path,
) -> String {
    let mut out = String::new();
    out.push_str("📦  Files agentstack will manage:\n");
    let manifest_display = display_path(manifest_path, project_root);
    let width = std::iter::once(manifest_display.len())
        .chain(
            destinations
                .iter()
                .map(|d| display_path(&d.path, project_root).len()),
        )
        .max()
        .unwrap_or(0);
    out.push_str(&format!(
        "      {manifest_display:width$}   the manifest — written by this import\n"
    ));
    for d in destinations {
        let scope = match d.scope {
            crate::scope::Scope::Project => "this project",
            crate::scope::Scope::Global => "machine-wide",
        };
        out.push_str(&format!(
            "      {:width$}   {} · {} ({scope})\n",
            display_path(&d.path, project_root),
            d.display,
            d.writes.join(" + "),
        ));
    }
    if !destinations.is_empty() {
        // "Will manage" is a claim about what `apply` renders, and the routing
        // block printed straight after this one says several of these
        // capabilities are served live instead. Both are true — `apply` is the
        // rendered lane's command and renders everything it is asked to
        // (docs/design/automatic-delivery.md, "What 'default' means here") — but
        // side by side and unqualified they read as a contradiction. Naming
        // `apply` as a choice rather than as an inevitability is what separates
        // them.
        out.push_str(
            "      None of these is written now, and none is written unless you ask:\n\
             \x20     `agentstack apply --write` renders them; the routing below says what\n\
             \x20     reaches each tool live instead.\n",
        );
    }
    out
}

/// The delivery planner's answer for a fresh project, one line per tool
/// (W4, `docs/design/automatic-delivery.md` §"The decision").
///
/// Two things make this honest rather than decorative:
///
/// - it says *served live* and *written to files* per tool, so a project in
///   both lanes at once — the normal case — reads as being in both;
/// - it never claims "0 files". The `rendered lane:` line names what really
///   gets written and where, and the live-lane note names what stays behind in
///   the project even when nothing is rendered for a tool.
///
/// A fresh manifest carries no `[delivery]` override, so this is Automatic by
/// construction; the override is something a person asks for later.
/// The live-lane harnesses that can receive nothing yet, because THEY have no
/// bridge registered.
///
/// Read per harness, not any-of: a bridge registered in one CLI delivers
/// nothing to the others, and reporting an empty list because a fifth CLI is
/// connected made the summary claim live delivery for four that had none.
/// The one shared reading is `overview::bridge_registered`; only the registry
/// load is here, because `init` has no `Context`.
///
/// `manifest` gates the finding on the same
/// [`declares_something_live`](crate::commands::delivery::declares_something_live)
/// predicate `status`, `doctor`, and `delivery` use: the plan reports a live
/// lane for what a harness *can* take, so an import declaring only
/// instructions was told to connect a bridge it does not need. Pass `None`
/// where no manifest exists yet — the caller that asks "is the bridge
/// connected?" must not have that answer softened by what is declared.
fn unconnected_live_harnesses(target_ids: &[String], manifest: Option<&Manifest>) -> Vec<String> {
    let Ok(registry) = Registry::load() else {
        // A registry we cannot load is a reason to say nothing extra, never a
        // reason to guess that delivery is broken.
        return Vec::new();
    };
    let plan = crate::delivery::Plan::build(&Delivery::default(), &registry, target_ids);
    if manifest.is_some_and(|m| !crate::commands::delivery::declares_something_live(m, &plan)) {
        return Vec::new();
    }
    crate::commands::delivery::unconnected_live(&plan, &registry)
}

/// Do this project's MCP servers reach the RENDERED lane on any target — i.e.
/// will `apply` ever write a server config for them?
///
/// Since `apply` honours the delivery planner, the answer is no for an ordinary
/// project of MCP-capable tools, and every sentence that assumed a second copy
/// of the servers on disk is false there.
fn renders_servers(target_ids: &[String], manifest: &Manifest) -> bool {
    let Ok(registry) = Registry::load() else {
        return false;
    };
    let plan = crate::delivery::Plan::build(&manifest.delivery, &registry, target_ids);
    plan.harnesses.iter().any(|h| {
        h.kinds_in(crate::delivery::Lane::Rendered)
            .contains(&crate::delivery::Kind::Server)
    })
}

/// Is there any rendered-lane work at all — anything `apply --write` would
/// actually write? Naming `apply` as a next step when the answer is no is how
/// the default onboarding path came to recommend writing nine config files into
/// a project the strategy says stays clean.
fn renders_anything(target_ids: &[String], manifest: &Manifest, server_count: usize) -> bool {
    if !manifest.settings.is_empty()
        || !manifest.instructions.is_empty()
        || !manifest.hooks.is_empty()
        || !manifest.extensions.is_empty()
    {
        return true;
    }
    server_count > 0 && renders_servers(target_ids, manifest)
}

/// `assume_connected` states the plan as if every bridge were registered — the
/// honest reading for `init`'s preview only when the summary discloses the
/// un-registered harnesses separately. Otherwise each harness's own bridge
/// state is read.
fn render_delivery_routing(target_ids: &[String], assume_connected: bool) -> String {
    let Ok(registry) = Registry::load() else {
        // The routing is a statement, not a gate: a registry we cannot load is
        // a reason to say nothing, never a reason to guess.
        return String::new();
    };
    let plan = crate::delivery::Plan::build(&Delivery::default(), &registry, target_ids);
    if plan.harnesses.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str("🚚  How each tool gets them:\n");
    let width = plan
        .harnesses
        .iter()
        .map(|h| h.display.len())
        .max()
        .unwrap_or(0);
    for h in &plan.harnesses {
        out.push_str(&format!(
            "      {:width$}   {}\n",
            h.display,
            crate::commands::delivery::harness_sentence(
                h,
                assume_connected || crate::commands::overview::bridge_registered(&registry, &h.id),
            )
        ));
    }
    if plan.has_dynamic_lane() {
        out.push_str(&format!("      {}\n", crate::delivery::ZERO_ARTIFACTS));
    }
    if let Some(line) = crate::delivery::rendered_lane_line(&plan) {
        out.push_str(&format!("      {line}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W4: the routing screen states both lanes per tool, names what is really
    /// written, and never degrades into a "0 files" claim.
    #[test]
    fn delivery_routing_states_both_lanes_and_never_claims_zero_files() {
        // `true` = the bridge is registered; that is the case whose wording
        // this test pins. The unconnected wording is covered by
        // `delivery::harness_sentence`'s own tests.
        let text = render_delivery_routing(&["claude-code".to_string()], true);
        assert!(text.contains("Claude Code"), "{text}");
        assert!(text.contains("served live"), "{text}");
        assert!(text.contains("rendered lane:"), "{text}");
        assert!(text.contains("0 project artifacts"), "{text}");
        assert!(!text.contains("0 files"), "{text}");
        // An instruction is never described as going live. Each harness row
        // keeps its lanes in separate clauses, so the live clause can be read
        // on its own — and it must never name a file-only kind.
        for line in text.lines().filter(|l| l.contains("served live")) {
            let live_clause = line.split(" · ").find(|c| c.contains("served live"));
            let live_clause = live_clause.unwrap_or(line);
            assert!(!live_clause.contains("house rules"), "{live_clause}");
            assert!(!live_clause.contains("settings"), "{live_clause}");
            assert!(!live_clause.contains("hooks"), "{live_clause}");
        }
    }

    /// Consent-fidelity witness (independent review, 2026-07-23): the plan
    /// digest must cover the FULL import the write performs, not the display
    /// summary. v1 flattened argv with spaces and omitted env/cwd, so plans
    /// that would write operationally different manifests shared a digest.
    /// NEVER weaken this to a display-derived digest.
    #[test]
    fn plan_digest_binds_operational_fields_the_display_summary_hides() {
        let base = Path::new("/tmp/proj");
        let mk = |args: &[&str], env_val: &str| {
            let mut servers: IndexMap<String, Server> = IndexMap::new();
            let s: Server = serde_json::from_value(serde_json::json!({
                "type": "stdio",
                "command": "npx",
                "args": args,
                "env": { "MODE": env_val },
            }))
            .expect("valid server literal");
            servers.insert("srv".into(), s);
            DetectedImport {
                detected: vec![DetectedCli {
                    id: "claude-code".into(),
                    display: "Claude Code".into(),
                    bin_on_path: true,
                    configs: Vec::new(),
                }],
                contributing: vec!["Claude Code".into()],
                servers,
                project_sourced: false,
                settings: IndexMap::new(),
                conflict_counts: IndexMap::new(),
                lifted: Vec::new(),
                skipped: Vec::new(),
                destinations: Vec::new(),
                tool_managed: Vec::new(),
                tool_managed_included: false,
                bridge_entries: Vec::new(),
            }
        };

        let baseline = plan_digest(&mk(&["a", "b"], "safe"), base, false, "keychain");
        // Same display target ("npx a b"), different argv boundaries.
        let joined_argv = plan_digest(&mk(&["a b"], "safe"), base, false, "keychain");
        assert_ne!(baseline, joined_argv, "argv boundaries must be bound");
        // Same display target, different env VALUE.
        let env_changed = plan_digest(&mk(&["a", "b"], "unsafe"), base, false, "keychain");
        assert_ne!(baseline, env_changed, "env values must be bound");
        // Destination participates too.
        let dest_changed = plan_digest(&mk(&["a", "b"], "safe"), base, false, "env");
        assert_ne!(baseline, dest_changed, "secret destination must be bound");
        // And the digest is stable for identical inputs.
        assert_eq!(
            baseline,
            plan_digest(&mk(&["a", "b"], "safe"), base, false, "keychain")
        );
    }

    /// Stage 1.2: the first screen states WHICH CLIs and WHICH native config
    /// files detection found — the evidence, not just a count — and stays
    /// honest for a binary-only CLI with no config files.
    #[test]
    fn found_clis_names_each_cli_and_its_config_files() {
        let root = Path::new("/repo");
        let detected = vec![
            DetectedCli {
                id: "claude-code".into(),
                display: "Claude Code".into(),
                bin_on_path: true,
                configs: vec![
                    PathBuf::from("/somewhere/.claude.json"),
                    PathBuf::from("/somewhere/.claude/settings.json"),
                ],
            },
            DetectedCli {
                id: "cursor".into(),
                display: "Cursor".into(),
                bin_on_path: true,
                configs: Vec::new(),
            },
        ];
        let out = render_found_clis(&detected, root);
        assert!(out.contains("Found 2 coding tools"));
        assert!(out.contains("Claude Code"));
        assert!(out.contains("/somewhere/.claude.json · /somewhere/.claude/settings.json"));
        assert!(
            out.contains("binary on PATH — no config files found"),
            "a binary-only CLI states the honest fact:\n{out}"
        );
    }

    /// An exclusion nobody can read is a silent drop. The block must state the
    /// count, every name, the apparent owner, the evidence, and the way to
    /// override it — and it must say "left alone", never let absence speak.
    #[test]
    fn the_tool_managed_block_names_the_servers_the_evidence_and_the_override() {
        let entries = vec![
            ToolManagedServer {
                name: "node_repl".into(),
                application: "ChatGPT".into(),
                evidence: "/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node_repl"
                    .into(),
            },
            ToolManagedServer {
                name: "computer-use".into(),
                application: "Codex Computer Use".into(),
                evidence: "./Codex Computer Use.app/Contents/MacOS/SkyComputerUseClient".into(),
            },
        ];
        let out = render_tool_managed(&entries, false);
        assert!(
            out.contains(
                "2 servers are managed by the apps that installed them and were left alone: \
                 node_repl, computer-use"
            ),
            "the count and the names lead the block:\n{out}"
        );
        assert!(out.contains("ChatGPT"), "{out}");
        assert!(
            out.contains("/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node_repl"),
            "the evidence is shown so the reading can be checked:\n{out}"
        );
        assert!(
            out.contains("nothing was deleted"),
            "the entries survive in their own CLI's config:\n{out}"
        );
        assert!(
            out.contains("--include-tool-managed"),
            "the override is named where the exclusion is stated:\n{out}"
        );

        // Opting in is stated too, with its cost — the same names, the other
        // outcome. Silence would be wrong in this direction as well.
        let included = render_tool_managed(&entries, true);
        assert!(included.contains("imported at your request"), "{included}");
        assert!(
            included.contains("re-gate"),
            "the churn an owned entry causes is stated:\n{included}"
        );

        // Nothing found, nothing claimed.
        assert!(render_tool_managed(&[], false).is_empty());
    }

    /// Every value in the block came from another CLI's config file. Both the
    /// name and the path are hostile input.
    #[test]
    fn the_tool_managed_block_sanitizes_names_and_paths() {
        let out = render_tool_managed(
            &[ToolManagedServer {
                name: "ev\u{1b}[31mil".into(),
                application: "App\u{1b}[0m".into(),
                evidence: "/Applications/E\u{1b}[2Kvil.app/Contents/x\u{7}".into(),
            }],
            false,
        );
        // Every hostile value is neutralized. (The block's own `⚠` still
        // carries this crate's colour codes — those are ours, not input.)
        assert!(out.contains("left alone: evil\n"), "{out:?}");
        assert!(
            !out.contains("\u{1b}[31m"),
            "no colour injection from input: {out:?}"
        );
        assert!(
            !out.contains("\u{1b}[2K"),
            "no cursor control from input: {out:?}"
        );
        assert!(!out.contains('\u{7}'), "no bell from input: {out:?}");
    }

    /// Review finding 3: the collision block names the clash, BOTH sides, and
    /// what replacing costs. A block that named only the clash would leave the
    /// person answering a yes/no with nothing to answer it from.
    #[test]
    fn the_collision_block_names_both_sides_and_the_cost() {
        let out = render_library_collisions(
            "local",
            &[LibraryCollision {
                name: "search".into(),
                existing: "runs npx -y search-mcp".into(),
                incoming: "runs npx -y search-FORK".into(),
            }],
        );
        assert!(out.contains("search"), "{out}");
        assert!(
            out.contains("in the library:  runs npx -y search-mcp"),
            "{out}"
        );
        assert!(
            out.contains("this import:     runs npx -y search-FORK"),
            "{out}"
        );
        assert!(
            out.contains("report drift until it re-locks"),
            "the cost of replacing is stated, not implied: {out}"
        );
    }

    /// Hostile input on both sides: the library folder may be someone else's,
    /// and the incoming definition came from another CLI's config file.
    #[test]
    fn the_collision_block_sanitizes_both_sides() {
        let out = render_library_collisions(
            "src\u{1b}[31m",
            &[LibraryCollision {
                name: "ev\u{1b}[2Kil".into(),
                existing: "runs a\nb".into(),
                incoming: "runs c\u{7}d".into(),
            }],
        );
        // Every hostile value is neutralized. (The block's own `⚠` still
        // carries this crate's colour codes — those are ours, not input.)
        assert!(
            out.contains("source 'src'"),
            "the source name is stripped: {out:?}"
        );
        assert!(
            out.contains("      evil\n"),
            "the server name is stripped: {out:?}"
        );
        assert!(
            !out.contains("\u{1b}[2K"),
            "no cursor control from input: {out:?}"
        );
        assert!(
            !out.contains("\u{1b}[31m"),
            "no colour injection from input: {out:?}"
        );
        assert!(!out.contains('\u{7}'), "no bell from input: {out:?}");
        // A newline inside a value cannot forge a line of its own.
        assert!(out.contains("runs a b"), "{out:?}");
    }

    /// Stage 1.2: imported servers are listed BY NAME with what each runs or
    /// contacts, before anything is written. Hostile names/targets are
    /// sanitized and bounded.
    #[test]
    fn import_servers_lists_names_and_targets() {
        let mut servers: IndexMap<String, Server> = IndexMap::new();
        let stdio: Server = serde_json::from_value(serde_json::json!({
            "type": "stdio", "command": "npx", "args": ["-y", "github-mcp"],
        }))
        .unwrap();
        let http: Server = serde_json::from_value(serde_json::json!({
            "type": "http", "url": "https://mcp.example.com/sse",
        }))
        .unwrap();
        servers.insert("github".into(), stdio);
        servers.insert("ctx".into(), http);
        let out = render_import_servers(&servers);
        assert!(out.contains("Importing 2 MCP servers"));
        assert!(out.contains("github"));
        assert!(out.contains("runs npx -y github-mcp"));
        assert!(out.contains("contacts https://mcp.example.com/sse"));
    }

    /// Stage 1.2: destinations are visible in user terms — each native file
    /// with its CLI, what lands there, and the scope in plain words ("this
    /// project"), plus the manifest the import itself writes. No adapter
    /// vocabulary required.
    #[test]
    fn managed_files_name_manifest_and_native_destinations_with_scope() {
        let root = Path::new("/repo");
        let dests = vec![
            PlanDestination {
                id: "claude-code".into(),
                display: "Claude Code".into(),
                scope: crate::scope::Scope::Project,
                path: PathBuf::from("/repo/.mcp.json"),
                writes: vec!["MCP servers"],
            },
            PlanDestination {
                id: "codex".into(),
                display: "Codex CLI".into(),
                scope: crate::scope::Scope::Project,
                path: PathBuf::from("/repo/.codex/config.toml"),
                writes: vec!["MCP servers", "settings"],
            },
        ];
        let out =
            render_managed_files(Path::new("/repo/.agentstack/agentstack.toml"), &dests, root);
        assert!(out.contains("Files agentstack will manage"));
        assert!(out.contains(".agentstack/agentstack.toml"));
        assert!(out.contains("the manifest — written by this import"));
        // Project-scope paths render relative to the repo root (alignment
        // padding between path and facts is not part of the contract).
        assert!(out.contains(".mcp.json"));
        assert!(out.contains("Claude Code · MCP servers (this project)"));
        assert!(out.contains(".codex/config.toml"));
        assert!(out.contains("Codex CLI · MCP servers + settings (this project)"));
        // The import writes none of them, and neither does anything else until
        // the user asks. This block sits directly above the routing block that
        // says what reaches each tool live instead, and the old wording ("Native
        // files are written by the next `agentstack apply --write`") made the
        // render read as scheduled — so the two blocks contradicted each other.
        // `apply` is unchanged and still renders everything it is asked to; only
        // the claim that it is coming anyway is gone.
        assert!(out.contains("none is written unless you ask"));
        assert!(out.contains("agentstack apply --write"));
    }

    /// Stage 1.2, at the default verbosity: the scripted import ends with ONE
    /// concise summary carrying the facts a new user needs — manifest path,
    /// what landed, which secrets still block the setup (with the exact
    /// command), one next step, and one compact line of everything else.
    #[test]
    fn import_summary_names_path_counts_secrets_and_one_next() {
        let out = ImportSummary {
            manifest_path: "/tmp/proj/.agentstack/agentstack.toml",
            sources: &["Claude Code".to_string(), "Codex CLI".to_string()],
            server_count: 8,
            settings_count: 2,
            needing_values: &["GITHUB_TOKEN".to_string()],
            also_detected: &["Gemini CLI".to_string(), "OpenCode".to_string()],
            rendered_work: true,
            ..Default::default()
        }
        .render();
        assert!(out.contains("Manifest:  /tmp/proj/.agentstack/agentstack.toml"));
        assert!(out.contains("8 MCP servers · settings from 2 CLIs"));
        assert!(out.contains("1 still needs a value"));
        assert!(out.contains("agentstack secret set GITHUB_TOKEN"));
        assert!(out.contains("agentstack x restore --last --write"));
        assert!(out.contains("agentstack apply --write"));
        assert!(out.contains("agentstack doctor"));

        // Progressive disclosure: the source list is evidence for the counts,
        // so it rides `--verbose` and the default names the flag rather than
        // dropping the fact silently.
        assert!(!out.contains("From:      Claude Code"), "{out}");
        assert!(out.contains("--verbose"), "{out}");

        // Exactly ONE `Next:`. The three-command list this replaced was three
        // recommendations and therefore none; the rest stay reachable on the
        // compact line under it.
        assert_eq!(out.matches("Next:").count(), 1, "{out}");
        assert!(out.contains("Then:"), "{out}");

        // The per-tool routing belongs to the pre-write review and appears in
        // exactly one place per run, so the close carries no copy of it — at
        // either verbosity. The only `Delivery:` line it may print is the
        // invariant-8 disclosure, which this connected case does not earn.
        assert!(!out.contains("Delivery:"), "{out}");
        let verbose = ImportSummary {
            manifest_path: "/tmp/proj/.agentstack/agentstack.toml",
            sources: &["Claude Code".to_string(), "Codex CLI".to_string()],
            server_count: 8,
            verbose: true,
            ..Default::default()
        }
        .render();
        assert!(
            verbose.contains("From:      Claude Code · Codex CLI"),
            "{verbose}"
        );
        assert!(!verbose.contains("Delivery:"), "{verbose}");

        // F09: import copies, it does not move — say so. With the servers on
        // the live lane, nothing ever copies them back into a native config, so
        // the old "described in two places" note (and the `--scope global`
        // command that answered it) would now be false. This promise is the
        // scripted route's only statement of it, so it is never behind a flag.
        assert!(out.contains("the CLI configs above are unchanged"));
        assert!(!out.contains("described in two places"), "{out}");
        assert!(out.contains("they are served from this manifest"));
        // H3: the summary teaches one next step and stops. No toolset offer, in
        // any shape — not the command, not a `[profiles.*]` block to paste, not
        // a forward reference to sessions.
        assert!(!out.contains("create-profile"));
        assert!(!out.contains("[profiles."));
        assert!(!out.contains("session start"));

        // M4: the detected-but-silent CLIs are named, with why — an unexplained
        // two-of-six looks like a detection failure. How to add one is the
        // evidence half, so it rides `--verbose`.
        assert!(out.contains("Also seen: Gemini CLI · OpenCode"));
        assert!(out.contains("no config to import"));
        let also_verbose = ImportSummary {
            manifest_path: "/m",
            sources: &["Claude Code".to_string()],
            also_detected: &["Gemini CLI".to_string()],
            verbose: true,
            ..Default::default()
        }
        .render();
        assert!(also_verbose.contains("agentstack apply --target <id> --write"));

        // Nothing left out → no "also seen" line at all, not an empty one.
        let all_contributed = ImportSummary {
            manifest_path: "/m",
            sources: &["Claude Code".to_string()],
            server_count: 2,
            ..Default::default()
        }
        .render();
        assert!(!all_contributed.contains("Also seen:"));

        // Nothing pending → no secrets section at all, not an empty one.
        let clean = ImportSummary {
            manifest_path: "/m",
            sources: &["Claude Code".to_string()],
            server_count: 1,
            ..Default::default()
        }
        .render();
        assert!(!clean.contains("Secrets:"));
        assert!(!clean.contains("settings from"));
        assert!(clean.contains("agentstack doctor"));
        assert!(!clean.contains("create-profile"));

        // No servers at all → nothing was copied, so no duplication note.
        let empty = ImportSummary {
            manifest_path: "/m",
            sources: &["Claude Code".to_string()],
            settings_count: 1,
            rendered_work: true,
            ..Default::default()
        }
        .render();
        assert!(!empty.contains("the CLI configs above are unchanged"));
        assert!(!empty.contains("create-profile"));
    }

    /// The library destination is one clause on the `Imported:` line by
    /// default and its own sentence under `--verbose` — never a fact that
    /// disappears, because a manifest that lists names instead of commands is
    /// otherwise unexplained.
    #[test]
    fn import_summary_says_where_the_definitions_landed_at_both_verbosities() {
        let facts = |verbose| {
            ImportSummary {
                manifest_path: "/m",
                sources: &["Claude Code".to_string()],
                server_count: 2,
                library_dest: Some(("local", "~/.agentstack/lib", 2, 0)),
                verbose,
                ..Default::default()
            }
            .render()
        };
        assert!(
            facts(false).contains("→ library 'local'"),
            "{}",
            facts(false)
        );
        assert!(
            facts(true).contains("the servers landed in 'local' (~/.agentstack/lib)"),
            "{}",
            facts(true)
        );
    }

    /// Invariant 8 on the scripted path: the live lane with no CLI connected
    /// delivers nothing, so the summary must not say "served live". It states
    /// the plan, the consequence, and the one deliberate command instead —
    /// `init --yes` never registers the bridge for anyone. At EVERY verbosity:
    /// this disclosure is about a claim the output must not make, so it can
    /// never be the thing a flag hides.
    #[test]
    fn import_summary_never_claims_live_delivery_without_a_connected_cli() {
        let unwired = ImportSummary {
            manifest_path: "/m",
            sources: &["Claude Code".to_string()],
            server_count: 3,
            unconnected_live: &["Claude Code".to_string(), "Codex CLI".to_string()],
            ..Default::default()
        }
        .render();
        assert!(
            unwired
                .contains("Delivery:  planned live for Claude Code, Codex CLI — NOT YET CONNECTED"),
            "{unwired}"
        );
        assert!(unwired.contains("nothing is served yet"), "{unwired}");
        assert!(unwired.contains("agentstack x gateway connect --all --write"));
        assert!(!unwired.contains("served live"), "{unwired}");

        // Connected → no disclosure to make, at any verbosity, and no claim of
        // delivery either: the routing lives in the pre-write review.
        let wired = ImportSummary {
            manifest_path: "/m",
            sources: &["Claude Code".to_string()],
            server_count: 3,
            verbose: true,
            ..Default::default()
        }
        .render();
        assert!(!wired.contains("Delivery:"), "{wired}");
        assert!(!wired.contains("NOT YET CONNECTED"));
    }

    /// The headline is the one line a reader is guaranteed to keep, so it may
    /// not say "complete" about an import that delivers nothing — and it may
    /// not say "unchanged" about configs `--connect` just edited.
    ///
    /// The negative control is the second half of each pair: the wording only
    /// changes in the state that earns it, so a future edit cannot make the
    /// warning unconditional (which would be its own dishonesty) without
    /// failing here.
    #[test]
    fn the_headline_matches_whether_anything_is_actually_delivered() {
        let summarize = |unconnected: &[String], bridge_now: bool| {
            ImportSummary {
                manifest_path: "/m",
                sources: &["Claude Code".to_string()],
                server_count: 2,
                unconnected_live: unconnected,
                bridge_registered_now: bridge_now,
                ..Default::default()
            }
            .render()
        };

        let stranded = summarize(&["Claude Code".to_string()], false);
        assert!(
            stranded.contains("Imported, and not yet delivering."),
            "{stranded}"
        );
        assert!(!stranded.contains("Import complete."), "{stranded}");
        // Both repairs: the command for this machine, and the flag that stops
        // the gap happening at all.
        assert!(stranded.contains("agentstack x gateway connect --all --write"));
        assert!(stranded.contains("agentstack init --connect"), "{stranded}");
        // Nothing was registered, so the ordinary untouched-configs note holds.
        assert!(
            stranded.contains("the CLI configs above are unchanged"),
            "{stranded}"
        );

        // Negative control 1: a connected live lane keeps the plain headline
        // and never mentions the flag that fixes a problem it does not have.
        let delivering = summarize(&[], false);
        assert!(delivering.contains("Import complete."), "{delivering}");
        assert!(!delivering.contains("not yet delivering"), "{delivering}");
        assert!(!delivering.contains("--connect"), "{delivering}");

        // Negative control 2: `--connect` registered the bridge in this run, so
        // the source configs are no longer untouched and must not be called so.
        let connected_now = summarize(&[], true);
        assert!(
            connected_now.contains("Import complete."),
            "{connected_now}"
        );
        assert!(
            !connected_now.contains("the CLI configs above are unchanged"),
            "{connected_now}"
        );
        assert!(
            connected_now.contains("now carry one agentstack entry"),
            "{connected_now}"
        );
    }

    /// The scripted close ends on ONE step, and it is the one standing between
    /// this run and a delivery.
    ///
    /// It used to end on a list: the bridge, `apply --write` and `doctor`, all
    /// labeled `Next:`. Three next steps are none. The order of relevance is
    /// the order in which each one blocks the others — a withheld trust grant
    /// first, then an unregistered bridge, then the rendered lane, then the
    /// check.
    #[test]
    fn the_scripted_close_ends_on_exactly_one_next_step() {
        // Servers only, routed live, no bridge: the bridge is the next step and
        // `apply --write` is not offered at all — it would write nothing.
        let live_only = ImportSummary {
            manifest_path: "/m",
            sources: &["Claude Code".to_string()],
            server_count: 2,
            unconnected_live: &["Claude Code".to_string()],
            ..Default::default()
        }
        .render();
        assert!(
            live_only.contains("Next:      agentstack x gateway connect --all --write"),
            "{live_only}"
        );
        assert_eq!(live_only.matches("Next:").count(), 1, "{live_only}");
        assert!(
            !live_only.contains("agentstack apply --write"),
            "{live_only}"
        );
        assert!(live_only.contains("agentstack doctor"));

        // Genuine rendered-lane work (settings) keeps the rendered command
        // reachable — on the secondary line, not as a second `Next:`.
        let both = ImportSummary {
            manifest_path: "/m",
            sources: &["Claude Code".to_string()],
            server_count: 2,
            settings_count: 1,
            unconnected_live: &["Claude Code".to_string()],
            rendered_work: true,
            ..Default::default()
        }
        .render();
        assert!(both.contains("Next:      agentstack x gateway connect --all --write"));
        assert_eq!(both.matches("Next:").count(), 1, "{both}");
        assert!(
            both.contains("Then:      agentstack apply --write"),
            "{both}"
        );

        // A withheld trust grant outranks the bridge: until the repo's own
        // servers are reviewed, registering the bridge serves nothing.
        let untrusted = ImportSummary {
            manifest_path: "/m",
            sources: &["Claude Code".to_string()],
            server_count: 2,
            unconnected_live: &["Claude Code".to_string()],
            needs_trust: true,
            ..Default::default()
        }
        .render();
        assert!(
            untrusted.contains("Next:      agentstack trust ."),
            "{untrusted}"
        );
        assert_eq!(untrusted.matches("Next:").count(), 1, "{untrusted}");
        assert!(
            untrusted.contains("Then:      agentstack x gateway connect --all --write"),
            "{untrusted}"
        );

        // Nothing pending anywhere → the check is the step, and it is not
        // repeated on the secondary line.
        let quiet = ImportSummary {
            manifest_path: "/m",
            sources: &["Claude Code".to_string()],
            server_count: 1,
            ..Default::default()
        }
        .render();
        assert!(quiet.contains("Next:      agentstack doctor"), "{quiet}");
        assert_eq!(quiet.matches("agentstack doctor").count(), 1, "{quiet}");
    }

    /// S1 witness (init-secrets design §7): a failing credential store must
    /// not abort init or silently drop values — failed refs are reported by
    /// name while the values that CAN store still do.
    #[test]
    fn store_lifted_reports_failures_by_name_and_keeps_storing() {
        let lifted = vec![
            Lifted {
                reference: "BROKEN".into(),
                value: "v1".into(),
                origin: "server 'a'".into(),
            },
            Lifted {
                reference: "OK".into(),
                value: "v2".into(),
                origin: "server 'b'".into(),
            },
        ];
        let mut stored = Vec::new();
        let unstored = store_lifted(&lifted, |name, _value| {
            if name == "BROKEN" {
                anyhow::bail!("no secret-service bus");
            }
            stored.push(name.to_string());
            Ok(())
        });
        assert_eq!(unstored, vec!["BROKEN".to_string()]);
        assert_eq!(stored, vec!["OK".to_string()]);
    }

    /// P2: the interactive menu preselects `.env` — bare Enter and `1` both
    /// pick it, and only an explicit `2`/`3` selects an alternative.
    #[test]
    fn parse_secret_choice_defaults_to_env() {
        assert_eq!(parse_secret_choice(""), SecretStore::Env);
        assert_eq!(parse_secret_choice("\n"), SecretStore::Env);
        assert_eq!(parse_secret_choice("1"), SecretStore::Env);
        assert_eq!(parse_secret_choice("2"), SecretStore::Keychain);
        assert_eq!(parse_secret_choice("3"), SecretStore::Skip);
        // Anything unrecognized falls back to the safe familiar default.
        assert_eq!(parse_secret_choice("garbage"), SecretStore::Env);
    }

    /// P28: the arrow-key selector maps its 0-based index to the same three
    /// stores, `.env` first (preselected). Item order must stay in lock-step
    /// with the numbered fallback above.
    #[test]
    fn secret_store_at_index_matches_menu_order() {
        assert_eq!(secret_store_at(0), SecretStore::Env);
        assert_eq!(secret_store_at(1), SecretStore::Keychain);
        assert_eq!(secret_store_at(2), SecretStore::Skip);
    }

    /// FIX D witness: a flagless `init` with no terminal must REFUSE before
    /// writing anything — otherwise it would silently import configs and lift
    /// live token values into files, contradicting its own help ("scripts get
    /// the promptless primitive via flags"). The TTY probe is injected
    /// (`interactive: false`) so the refusal path runs without a real terminal.
    #[test]
    fn non_tty_flagless_init_refuses_and_writes_nothing() {
        let dir = assert_fs::TempDir::new().unwrap();
        let args = InitArgs {
            global: false,
            force: false,
            dry_run: false,
            plan: false,
            secrets: None,
            no_keychain: false,
            project_servers: false,
            include_tool_managed: false,
            yes: false,
            consented_plan: None,
            connect: false,
            verbose: false,
        };
        let err = run_gated(&args, Some(dir.path()), false)
            .expect_err("a flagless non-TTY init must refuse");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("--yes") && msg.contains("without a terminal"),
            "the refusal names the scripted escape and the reason: {msg}"
        );
        // Nothing was written under either manifest layout.
        assert!(!dir.path().join(".agentstack/agentstack.toml").exists());
        assert!(!dir.path().join("agentstack.toml").exists());
    }

    /// Lane A witness (UI control-plane §10): `init --plan` is detection-only
    /// — it must write NOTHING, under either manifest layout, even when run
    /// non-interactively with no other flags (the read primitive an external
    /// wizard calls headlessly). Detection reads the real machine's CLI
    /// configs, which is fine: the assertion is about writes, not findings.
    #[test]
    fn plan_emits_json_and_writes_nothing() {
        let dir = assert_fs::TempDir::new().unwrap();
        let args = InitArgs {
            global: false,
            force: false,
            dry_run: false,
            plan: true,
            secrets: None,
            no_keychain: false,
            project_servers: false,
            include_tool_managed: false,
            yes: false,
            consented_plan: None,
            connect: false,
            verbose: false,
        };
        run_gated(&args, Some(dir.path()), false).expect("plan is read-only and never refuses");
        assert!(!dir.path().join(".agentstack").exists());
        assert!(!dir.path().join("agentstack.toml").exists());
        assert!(!dir.path().join(".env").exists());
    }

    /// T4 (third-pass DX audit): scripted `init` against an initialized
    /// project must recommend the real next steps (`apply --write`), not the
    /// generic escapes — `--yes` would hit the --force wall and `--dry-run`
    /// previews a from-scratch replacement. Both the flagless non-TTY path
    /// and the explicit `--yes` path land on the same adapted refusal.
    #[test]
    fn scripted_init_with_existing_manifest_names_apply_not_yes() {
        let dir = assert_fs::TempDir::new().unwrap();
        std::fs::write(dir.path().join("agentstack.toml"), "version = 1\n").unwrap();

        let flagless = InitArgs {
            global: false,
            force: false,
            dry_run: false,
            plan: false,
            secrets: None,
            no_keychain: false,
            project_servers: false,
            include_tool_managed: false,
            yes: false,
            consented_plan: None,
            connect: false,
            verbose: false,
        };
        let with_yes = InitArgs {
            yes: true,
            ..flagless.clone()
        };
        for args in [flagless, with_yes] {
            let err = run_gated(&args, Some(dir.path()), false)
                .expect_err("init over an existing manifest must refuse");
            let msg = format!("{err:#}");
            assert!(msg.contains("already exists"), "{msg}");
            assert!(
                msg.contains("agentstack apply --write"),
                "names the real scripted next step: {msg}"
            );
            assert!(
                !msg.contains("--yes"),
                "no escape that would just error again: {msg}"
            );
        }
        // The manifest survived untouched.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("agentstack.toml")).unwrap(),
            "version = 1\n"
        );
    }

    /// The vault opt-in declares NAMES. A value here would be a secret
    /// serialized into the repository — the precise thing `${REF}` exists to
    /// prevent — so this asserts the file can never carry one, and that the
    /// names it does carry are well-formed and de-duplicated.
    #[test]
    fn the_env_schema_declares_names_and_never_a_value() {
        let names = schema_names(&[
            "B_TOKEN".into(),
            "A_TOKEN".into(),
            "B_TOKEN".into(),
            "not a ref".into(),
            "$(rm -rf /)".into(),
        ]);
        assert_eq!(names, vec!["A_TOKEN".to_string(), "B_TOKEN".to_string()]);
        let body = env_schema_body(&names);
        assert!(body.contains("\n# ---\n"), "{body}");
        assert!(body.contains("\nA_TOKEN=\n"), "{body}");
        assert!(body.contains("\nB_TOKEN=\n"), "{body}");
        for line in body.lines().filter(|l| !l.starts_with('#')) {
            assert!(
                line.is_empty() || line.ends_with('='),
                "every declaration ends at the `=`: {line:?}"
            );
        }
    }
}
