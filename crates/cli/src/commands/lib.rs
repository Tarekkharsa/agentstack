//! `agentstack lib` — manage the central capability library
//! (`~/.agentstack/lib/`) that projects reference by name instead of copying
//! files (see `docs/reference.md#the-central-library`).
//!
//! This module owns the **library write contract**: [`add_skill`] is the single
//! insertion path — how an item enters `library.toml`, how its files land under
//! `lib/skills/`, and how its checksum + provenance are recorded. Future
//! surface (`lib migrate`, `consolidate` emitting name refs) reuses it rather
//! than inventing its own file/index logic.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use owo_colors::OwoColorize;

use crate::cli::{
    LibAddArgs, LibAddServerArgs, LibArgs, LibKind, LibMigrateArgs, LibRemoveArgs,
    LibRemoveServerArgs, LibSyncArgs,
};
use crate::library::{Library, LibraryServer, LibrarySkill};
use crate::manifest::{Server, Skill};
use crate::scan::Severity;
use crate::store::{dir_digest, dir_size, Store};
use crate::util::paths;

/// Above this, a skill is almost certainly carrying vendored dependencies —
/// and every full-library pass (doctor, install, use) pays to read it.
const LARGE_SKILL_BYTES: u64 = 10 * 1024 * 1024;

pub fn run(args: &LibArgs, manifest_dir: Option<&Path>) -> Result<()> {
    match &args.kind {
        LibKind::Add(a) => add(a),
        LibKind::AddServer(a) => add_server_cli(a, manifest_dir),
        LibKind::List => list(),
        LibKind::Remove(a) => remove(a),
        LibKind::RemoveServer(a) => remove_server_cli(a),
        LibKind::Migrate(a) => migrate(a),
        LibKind::Sync(a) => sync(a),
    }
}

/// Where a library skill's content is being added from.
pub enum LibSource<'a> {
    /// A local skill directory (copied into `lib/skills/<name>`).
    Path(&'a Path),
    /// A git source (resolved via the store; referenced, not copied).
    /// `subpath` selects the skill's directory within the repo (subdir layouts).
    Git {
        url: &'a str,
        rev: Option<&'a str>,
        subpath: Option<&'a str>,
    },
}

/// The result of a library insertion (or a dry-run preview of one).
#[derive(Debug)]
pub struct AddOutcome {
    pub name: String,
    /// `"path"` or `"git"`.
    pub source_kind: &'static str,
    /// SHA-256 of the resolved content.
    pub checksum: String,
    /// The `lib/skills/<name>` directory for path sources; `None` for git.
    pub dest: Option<PathBuf>,
    /// The absolutized source directory for path sources; `None` for git.
    pub source_path: Option<PathBuf>,
    /// Advisory notes — e.g. a temp-dir source whose recorded provenance
    /// will dangle once the OS cleans it up.
    pub warnings: Vec<String>,
    /// False on a dry run (nothing was written).
    pub written: bool,
    /// True when an existing entry of the same name was overwritten.
    pub replaced: bool,
    /// Total content size in bytes — surfaced so callers can warn on skills
    /// large enough to slow every full-library scan.
    pub total_bytes: u64,
}

/// Insert a skill into the central library at `lib_home`. The single library
/// write path, reused by the CLI and (later) migrate/consolidate.
///
/// - `Path`: validated to contain `SKILL.md`, copied into `lib/skills/<name>`,
///   digested there, recorded as `path = "<name>"`.
/// - `Git`: resolved through the store (records `git`, resolved `rev`, and
///   checksum); the body stays in the store, referenced by the entry.
///
/// A same-named entry is a hard error unless `replace` is set. When `write` is
/// false, nothing is mutated and the returned outcome is a preview.
pub fn add_skill(
    lib_home: &Path,
    name: &str,
    source: LibSource,
    replace: bool,
    write: bool,
    allow_flagged: bool,
) -> Result<AddOutcome> {
    add_skill_inner(lib_home, name, source, replace, write, allow_flagged, None)
}

/// [`add_skill`] with an explicit provenance string, for callers whose true
/// origin is richer than the copied path — e.g. `plugins adopt` lifting skill
/// bodies out of a native plugin cache records the marketplace, plugin, and
/// version instead of a cache path that dies on the next plugin update.
pub fn add_skill_with_provenance(
    lib_home: &Path,
    name: &str,
    source: LibSource,
    replace: bool,
    write: bool,
    allow_flagged: bool,
    provenance: &str,
) -> Result<AddOutcome> {
    add_skill_inner(
        lib_home,
        name,
        source,
        replace,
        write,
        allow_flagged,
        Some(provenance),
    )
}

#[allow(clippy::too_many_arguments)]
fn add_skill_inner(
    lib_home: &Path,
    name: &str,
    source: LibSource,
    replace: bool,
    write: bool,
    allow_flagged: bool,
    provenance_override: Option<&str>,
) -> Result<AddOutcome> {
    if !valid_lib_name(name) {
        bail!("invalid library skill name '{name}' — must be non-empty and contain no path separators");
    }

    let mut library = Library::load(lib_home)?;
    let replacing = library.get(name).is_some();
    if replacing && !replace {
        bail!("'{name}' is already in the central library — pass --replace to overwrite");
    }

    let mut warnings = Vec::new();
    let (entry, dest, checksum, source_kind, source_path, total_bytes) = match source {
        LibSource::Path(src) => {
            let src = absolutize(src)?;
            require_skill_md(&src)?;
            // Supply-chain gate: scan the source before any of it becomes the
            // canonical library copy (plan §3).
            scan_gate(name, &src, allow_flagged, &mut warnings)?;
            let dest = lib_home.join("skills").join(name);
            if same_dir(&src, &dest) {
                bail!(
                    "source {} is already the library location — nothing to add",
                    src.display()
                );
            }
            // Provenance records the source verbatim; flag it when that path
            // won't outlive an OS temp cleanup (lib list/explain would show a
            // dead path forever).
            if is_temp_path(&src) {
                warnings.push(format!(
                    "source {} is a temporary directory — the recorded provenance will \
                     dangle once it is cleaned up (the library copy is unaffected)",
                    src.display()
                ));
            }
            // Digest the source now so the preview reflects what would land; a
            // write copies first and re-digests the destination.
            let (checksum, total_bytes) = if write {
                if dest.exists() {
                    std::fs::remove_dir_all(&dest)
                        .with_context(|| format!("removing {}", dest.display()))?;
                }
                crate::consolidate::copy_dir(&src, &dest)?;
                (dir_digest(&dest)?, dir_size(&dest))
            } else {
                (dir_digest(&src)?, dir_size(&src))
            };
            let entry = LibrarySkill {
                name: name.to_string(),
                source: "path".into(),
                path: Some(name.to_string()),
                git: None,
                rev: None,
                subpath: None,
                checksum: Some(checksum.clone()),
                version: None,
                provenance: Some(
                    provenance_override
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("path:{}", src.display())),
                ),
            };
            (entry, Some(dest), checksum, "path", Some(src), total_bytes)
        }
        LibSource::Git { url, rev, subpath } => {
            // Resolving fetches into the store (needed to learn rev + checksum and
            // to validate SKILL.md) — this touches the network even on a dry run.
            let store = Store::default_store();
            let subpath = subpath.map(str::trim).filter(|s| !s.is_empty());
            let skill = Skill {
                path: None,
                git: Some(url.to_string()),
                rev: rev.map(str::to_string),
                subpath: subpath.map(str::to_string),
            };
            // `resolved.path` is the subpath directory within the clone, so
            // SKILL.md validation and the scan both cover the actual skill body.
            let resolved = store
                .resolve(&skill, lib_home, rev)
                .with_context(|| format!("resolving git source {url}"))?;
            require_skill_md(&resolved.path)?;
            scan_gate(name, &resolved.path, allow_flagged, &mut warnings)?;
            // Truthful provenance: url @ resolved rev, with the subpath fragment
            // when the skill lives in a subdir (plan §6).
            let mut provenance = format!("git:{url}");
            if let Some(rev) = &resolved.rev {
                provenance.push_str(&format!("@{rev}"));
            }
            if let Some(sub) = subpath {
                provenance.push_str(&format!("#{sub}"));
            }
            let entry = LibrarySkill {
                name: name.to_string(),
                source: "git".into(),
                path: None,
                git: Some(url.to_string()),
                rev: resolved.rev.clone(),
                subpath: subpath.map(str::to_string),
                checksum: Some(resolved.checksum.clone()),
                version: None,
                provenance: Some(
                    provenance_override
                        .map(str::to_string)
                        .unwrap_or(provenance),
                ),
            };
            let total_bytes = dir_size(&resolved.path);
            (entry, None, resolved.checksum, "git", None, total_bytes)
        }
    };

    // Oversized skills make every full-library pass (doctor, install, use)
    // expensive for every consumer — surface it on the outcome so the CLI,
    // MCP, and consolidate callers all warn uniformly.
    if total_bytes > LARGE_SKILL_BYTES {
        warnings.push(format!(
            "'{name}' is {} — every full-library pass (doctor, install, use) reads all of it. \
             Vendored dependencies (node_modules, venvs, build output) don't belong in a skill; \
             ship the instructions and fetch dependencies at run time.",
            human_bytes(total_bytes)
        ));
    }

    if write {
        library.upsert(entry);
        library.save(lib_home)?;
    }

    Ok(AddOutcome {
        name: name.to_string(),
        source_kind,
        checksum,
        dest,
        source_path,
        warnings,
        written: write,
        replaced: replacing,
        total_bytes,
    })
}

fn add(args: &LibAddArgs) -> Result<()> {
    let lib_home = paths::lib_home();
    // A subdir may be given as `--subpath skills/foo` or folded into the URL as
    // `--git <url>#skills/foo`. Accept either, but not two conflicting values.
    let (git_url, url_frag) = match &args.git {
        Some(g) => match g.split_once('#') {
            Some((u, frag)) => (Some(u.to_string()), Some(frag.to_string())),
            None => (Some(g.clone()), None),
        },
        None => (None, None),
    };
    let subpath = match (&args.subpath, &url_frag) {
        (Some(a), Some(b)) if a != b => {
            bail!("subpath given twice and they differ: --subpath '{a}' vs '#{b}' in --git")
        }
        (Some(s), _) | (_, Some(s)) => Some(s.clone()),
        (None, None) => None,
    };
    if subpath.is_some() && git_url.is_none() {
        bail!("--subpath applies to git sources only — pass it with --git <url>");
    }
    let source = match (&args.path, &git_url) {
        (Some(p), None) => LibSource::Path(Path::new(p)),
        (None, Some(url)) => LibSource::Git {
            url,
            rev: args.rev.as_deref(),
            subpath: subpath.as_deref(),
        },
        (None, None) => bail!("specify a source: --path <dir> or --git <url>"),
        (Some(_), Some(_)) => bail!("--path and --git are mutually exclusive"),
    };

    let outcome = add_skill(
        &lib_home,
        &args.name,
        source,
        args.replace,
        args.write,
        args.allow_flagged,
    )?;

    for w in &outcome.warnings {
        println!("  {} {w}", "⚠".yellow());
    }
    let verb = if outcome.replaced { "replace" } else { "add" };
    let past = if outcome.replaced {
        "replaced"
    } else {
        "added"
    };
    if outcome.written {
        println!(
            "{} {past} '{}' ({}) in the central library",
            "✓".green(),
            outcome.name,
            outcome.source_kind
        );
        if let Some(dest) = &outcome.dest {
            match &outcome.source_path {
                Some(src) => println!("  copied {} → {}", src.display(), dest.display()),
                None => println!("  files → {}", dest.display()),
            }
            println!("  the library copy is now canonical — edits to the source have no effect");
        }
        println!("  checksum {}", short(&outcome.checksum));
    } else {
        println!(
            "Would {verb} '{}' ({}) into the central library:",
            outcome.name.bold(),
            outcome.source_kind
        );
        if let Some(dest) = &outcome.dest {
            match &outcome.source_path {
                Some(src) => println!(
                    "  {} copy {} → {} (the library copy becomes canonical)",
                    "→".cyan(),
                    src.display(),
                    dest.display()
                ),
                None => println!("  {} files → {}", "→".cyan(), dest.display()),
            }
        }
        println!("  {} checksum {}", "→".cyan(), short(&outcome.checksum));
        println!("\nDry run. Re-run with {} to apply.", "--write".bold());
    }
    Ok(())
}

/// The result of a server insertion (or a dry-run preview of one).
#[derive(Debug)]
pub struct ServerAddOutcome {
    pub name: String,
    /// SHA-256 of the normalized definition written to `lib/servers/<name>.toml`.
    pub checksum: String,
    pub dest: PathBuf,
    pub written: bool,
    pub replaced: bool,
    /// Literal values that look like plaintext secrets (should be `${REF}`s).
    pub warnings: Vec<String>,
}

/// Insert an MCP server **definition** into the central library at `lib_home`.
/// The file must parse as a `manifest::Server`; it is normalized (re-serialized)
/// into `lib/servers/<name>.toml`, digested, and indexed in `library.toml`.
/// `${REF}` secrets are preserved verbatim and never resolved. Literal
/// secret-looking values are surfaced as warnings (not scrubbed, not blocked).
///
/// A same-named entry is a hard error unless `replace`. When `write` is false,
/// nothing is mutated.
pub fn add_server(
    lib_home: &Path,
    name: &str,
    file: &Path,
    replace: bool,
    write: bool,
) -> Result<ServerAddOutcome> {
    let raw =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let server: Server = toml::from_str(&raw)
        .with_context(|| format!("{} is not a valid MCP server definition", file.display()))?;
    let src = absolutize(file)?;
    let provenance = format!("file:{}", src.display());
    let mut outcome = add_server_def(lib_home, name, &server, provenance, replace, write)?;
    // Same honesty as `lib add --path`: a temp-dir source leaves a provenance
    // path that lib list/explain will show long after the OS cleaned it up.
    if is_temp_path(&src) {
        outcome.warnings.push(format!(
            "source {} is a temporary location — the recorded provenance will \
             dangle once it is cleaned up (the library definition is unaffected)",
            src.display()
        ));
    }
    Ok(outcome)
}

/// Add an in-memory server definition to the library (the core of
/// [`add_server`]; also used by `--from-manifest`).
pub fn add_server_def(
    lib_home: &Path,
    name: &str,
    server: &Server,
    provenance: String,
    replace: bool,
    write: bool,
) -> Result<ServerAddOutcome> {
    if !valid_lib_name(name) {
        bail!("invalid library server name '{name}' — must be non-empty and contain no path separators");
    }

    let mut library = Library::load(lib_home)?;
    let replacing = library.get_server(name).is_some();
    if replacing && !replace {
        bail!("'{name}' is already in the central library — pass --replace to overwrite");
    }

    let warnings = suspicious_secrets(server);
    // Normalize: re-serialize so exactly a Server table is stored (drops junk).
    let normalized = toml::to_string_pretty(server).context("serializing server definition")?;
    let checksum = crate::resolve::sha256_hex(normalized.as_bytes());
    let dest = lib_home.join("servers").join(format!("{name}.toml"));

    if write {
        std::fs::create_dir_all(dest.parent().unwrap())
            .with_context(|| format!("creating {}", dest.parent().unwrap().display()))?;
        std::fs::write(&dest, &normalized)
            .with_context(|| format!("writing {}", dest.display()))?;
        library.upsert_server(LibraryServer {
            name: name.to_string(),
            checksum: Some(checksum.clone()),
            version: None,
            provenance: Some(provenance),
        });
        library.save(lib_home)?;
    }

    Ok(ServerAddOutcome {
        name: name.to_string(),
        checksum,
        dest,
        written: write,
        replaced: replacing,
        warnings,
    })
}

fn add_server_cli(args: &LibAddServerArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let lib_home = paths::lib_home();
    let outcome = if args.from_manifest {
        let ctx = super::load(manifest_dir)?;
        let Some(server) = ctx.loaded.manifest.servers.get(&args.name) else {
            let available: Vec<&str> = ctx
                .loaded
                .manifest
                .servers
                .keys()
                .map(String::as_str)
                .collect();
            bail!(
                "no [servers.{}] in the manifest — available: {}",
                args.name,
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available.join(", ")
                }
            );
        };
        add_server_def(
            &lib_home,
            &args.name,
            server,
            format!("manifest:{}", ctx.dir.display()),
            args.replace,
            args.write,
        )?
    } else {
        let Some(file) = args.file.as_deref() else {
            bail!("pass --file <definition.toml> or --from-manifest");
        };
        add_server(
            &lib_home,
            &args.name,
            Path::new(file),
            args.replace,
            args.write,
        )?
    };

    for w in &outcome.warnings {
        println!("  {} {w}", "⚠".yellow());
    }
    let verb = if outcome.replaced { "replace" } else { "add" };
    let past = if outcome.replaced {
        "replaced"
    } else {
        "added"
    };
    if outcome.written {
        println!(
            "{} {past} server '{}' in the central library",
            "✓".green(),
            outcome.name
        );
        println!("  files → {}", outcome.dest.display());
        println!("  checksum {}", short(&outcome.checksum));
    } else {
        println!(
            "Would {verb} server '{}' into the central library:",
            outcome.name.bold()
        );
        println!("  {} files → {}", "→".cyan(), outcome.dest.display());
        println!("  {} checksum {}", "→".cyan(), short(&outcome.checksum));
        println!("\nDry run. Re-run with {} to apply.", "--write".bold());
    }
    Ok(())
}

/// Values in a server definition that look like literal secrets (not `${REF}`s).
/// Covers every field a real credential could hide in: headers, env, the `url`
/// (userinfo password + secretish query params), and `args` (secretish
/// `key=value` and the value following a secretish flag).
fn suspicious_secrets(server: &Server) -> Vec<String> {
    let mut out = Vec::new();
    let mut scan = |k: &str, v: &str| {
        if !v.contains("${") && looks_secretish(k, v) {
            out.push(format!(
                "'{k}' has a literal value that looks like a secret — use ${{REF}} instead"
            ));
        }
    };
    for (k, v) in &server.headers {
        scan(k, v);
    }
    for (k, v) in &server.env {
        scan(k, v);
    }
    if let Some(url) = &server.url {
        out.extend(url_secrets(url));
    }
    out.extend(arg_secrets(&server.args));
    out
}

/// Literal-secret findings in a server `url`: a password embedded in the
/// userinfo (`https://user:TOKEN@host`) or a secretish query parameter
/// (`?api_key=…`). `${REF}` values are exempt.
fn url_secrets(url: &str) -> Vec<String> {
    let mut out = Vec::new();
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    // The authority is everything before the path/query/fragment.
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    if let Some((userinfo, _host)) = authority.rsplit_once('@') {
        // A password (the part after ':') is a credential by construction.
        if let Some((_user, pass)) = userinfo.split_once(':') {
            if !pass.is_empty() && !pass.contains("${") {
                out.push(
                    "url embeds a literal password in its userinfo — use ${REF} instead".into(),
                );
            }
        }
    }
    if let Some((_, query)) = after_scheme.split_once('?') {
        let query = query.split('#').next().unwrap_or(query);
        for pair in query.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if !v.contains("${") && looks_secretish(k, v) {
                    out.push(format!(
                        "url query parameter '{k}' has a literal value that looks like a secret — use ${{REF}} instead"
                    ));
                }
            }
        }
    }
    out
}

/// Literal-secret findings in server `args`: a `key=value` (or `--key=value`)
/// whose key is secretish, or the value following a secretish flag
/// (`--token VALUE`). `${REF}` values are exempt.
fn arg_secrets(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        if let Some((k, v)) = arg.split_once('=') {
            let key = k.trim_start_matches('-');
            if !v.contains("${") && looks_secretish(key, v) {
                out.push(format!(
                    "arg '{key}' has a literal value that looks like a secret — use ${{REF}} instead"
                ));
                continue;
            }
        }
        // A secretish flag names a credential; its value is the next arg.
        if arg.starts_with('-') && key_is_secretish(arg.trim_start_matches('-')) {
            if let Some(v) = args.get(i + 1) {
                if !v.is_empty() && !v.contains("${") {
                    out.push(format!(
                        "arg following '{arg}' looks like a literal secret — use ${{REF}} instead"
                    ));
                }
            }
        }
    }
    out
}

fn looks_secretish(key: &str, val: &str) -> bool {
    !val.is_empty() && key_is_secretish(key)
}

/// Whether `key` names a secret (case-insensitive substring match).
fn key_is_secretish(key: &str) -> bool {
    let k = key.to_lowercase();
    [
        "authorization",
        "token",
        "secret",
        "api-key",
        "apikey",
        "api_key",
        "password",
        "bearer",
        "key",
    ]
    .iter()
    .any(|s| k.contains(s))
}

/// The secretish `key = "value"` assignments in a single line of TOML-ish text
/// (also `key: "value"` inside an inline table): each offending key. A value
/// containing `${` is a reference, not a literal — exempt. Used where a file
/// won't parse (F3) and when scanning outgoing commits (F6).
fn secretish_keys_in_line(line: &str) -> Vec<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re =
        RE.get_or_init(|| regex::Regex::new(r#"([A-Za-z0-9_.-]+)\s*[=:]\s*"([^"]*)""#).unwrap());
    re.captures_iter(line)
        .filter_map(|c| {
            let key = c.get(1)?.as_str();
            let val = c.get(2)?.as_str();
            (!val.is_empty() && !val.contains("${") && key_is_secretish(key))
                .then(|| key.to_string())
        })
        .collect()
}

/// The result of a server removal (or a dry-run preview of one).
#[derive(Debug)]
pub struct ServerRemoveOutcome {
    pub name: String,
    /// The `lib/servers/<name>.toml` file that would be / was deleted (`None` if
    /// the name is unsafe — then only the index entry is dropped).
    pub removed_file: Option<PathBuf>,
    pub written: bool,
}

/// Remove a server from the central library: drop the `library.toml` entry and
/// delete its `lib/servers/<name>.toml` definition. The file path derives solely
/// from the (validated) name, so it can never escape `lib/servers`. A missing
/// name is a hard error; `write=false` mutates nothing.
pub fn remove_server(lib_home: &Path, name: &str, write: bool) -> Result<ServerRemoveOutcome> {
    let mut library = Library::load(lib_home)?;
    if library.get_server(name).is_none() {
        bail!("'{name}' is not a server in the central library");
    }
    // The definition file is always `lib/servers/<name>.toml`; only compute it
    // for a safe name so a hand-edited index can never target an outside path.
    let removed_file =
        valid_lib_name(name).then(|| lib_home.join("servers").join(format!("{name}.toml")));

    if write {
        if let Some(f) = &removed_file {
            if f.exists() {
                std::fs::remove_file(f).with_context(|| format!("removing {}", f.display()))?;
            }
        }
        library.remove_server(name);
        library.save(lib_home)?;
    }

    Ok(ServerRemoveOutcome {
        name: name.to_string(),
        removed_file,
        written: write,
    })
}

fn remove_server_cli(args: &LibRemoveServerArgs) -> Result<()> {
    let lib_home = paths::lib_home();
    let outcome = remove_server(&lib_home, &args.name, args.write)?;
    if outcome.written {
        println!(
            "{} removed server '{}' from the central library",
            "✓".green(),
            outcome.name
        );
        if let Some(f) = &outcome.removed_file {
            println!("  deleted {}", f.display());
        }
    } else {
        println!(
            "Would remove server '{}' from the central library:",
            outcome.name.bold()
        );
        match &outcome.removed_file {
            Some(f) => println!("  {} delete {}", "−".yellow(), f.display()),
            None => println!("  {} index entry only", "−".yellow()),
        }
        println!("\nDry run. Re-run with {} to apply.", "--write".bold());
    }
    Ok(())
}

/// `lib list` — a plain read of the index. No resolver, no store, no filesystem
/// validation: it reports what `library.toml` records, nothing more.
fn list() -> Result<()> {
    let lib_home = paths::lib_home();
    let library = Library::load(&lib_home)?;
    print!("{}", render_list(&library));
    Ok(())
}

/// Render the library index as plain tables grouped by kind (shared with tests).
/// Rows are sorted by name for stable output regardless of on-disk order.
fn render_list(library: &Library) -> String {
    if library.skills.is_empty() && library.servers.is_empty() {
        return "No skills or servers installed in the central library.\n".to_string();
    }
    let mut skills = library.skills.clone();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    let mut servers = library.servers.clone();
    servers.sort_by(|a, b| a.name.cmp(&b.name));

    let mut o = String::new();
    o.push_str("Skills\n");
    if skills.is_empty() {
        o.push_str("  (none)\n");
    }
    for s in &skills {
        o.push_str(&format!(
            "  {:<20} {:<6} {:<16} {}\n",
            s.name,
            s.source,
            locator(s),
            s.provenance.as_deref().unwrap_or("-")
        ));
    }

    o.push_str("\nServers\n");
    if servers.is_empty() {
        o.push_str("  (none)\n");
    }
    for s in &servers {
        let sum = s.checksum.as_deref().map(short).unwrap_or("-");
        o.push_str(&format!(
            "  {:<20} {:<16} {}\n",
            s.name,
            sum,
            s.provenance.as_deref().unwrap_or("-")
        ));
    }
    o
}

/// A short, glanceable locator for a row: the git rev if present, else the
/// content checksum, both truncated.
fn locator(s: &LibrarySkill) -> String {
    if let Some(rev) = &s.rev {
        return format!("rev {}", short(rev));
    }
    match &s.checksum {
        Some(c) => short(c).to_string(),
        None => "-".to_string(),
    }
}

/// The result of a library removal (or a dry-run preview of one).
#[derive(Debug)]
pub struct RemoveOutcome {
    pub name: String,
    /// `"path"` or `"git"`, from the removed index entry.
    pub source_kind: String,
    /// The contained `lib/skills/<name>` dir that would be / was deleted
    /// (path skills only; `None` for git-backed or uncontained entries).
    pub removed_dir: Option<PathBuf>,
    /// False on a dry run (nothing was mutated).
    pub written: bool,
}

/// Remove a skill from the central library at `lib_home`. The inverse of
/// [`add_skill`]: drops the `library.toml` entry and, for a path skill, deletes
/// its contained `lib/skills/<name>` directory. Git-backed entries leave the
/// shared store cache untouched. Does not touch project manifests or lockfiles.
///
/// A missing name is a hard error. When `write` is false, nothing is mutated.
pub fn remove_skill(lib_home: &Path, name: &str, write: bool) -> Result<RemoveOutcome> {
    let mut library = Library::load(lib_home)?;
    let Some(entry) = library.get(name).cloned() else {
        bail!("'{name}' is not in the central library");
    };

    // Only path skills own files to delete, and only within lib/skills. A git
    // entry references the shared store cache — never delete that here.
    let removed_dir = if entry.source == "path" {
        entry
            .path
            .as_deref()
            .and_then(|p| contained_lib_skill_dir(lib_home, p))
    } else {
        None
    };

    if write {
        if let Some(dir) = &removed_dir {
            if dir.exists() {
                std::fs::remove_dir_all(dir)
                    .with_context(|| format!("removing {}", dir.display()))?;
            }
        }
        library.remove(name);
        library.save(lib_home)?;
    }

    Ok(RemoveOutcome {
        name: name.to_string(),
        source_kind: entry.source,
        removed_dir,
        written: write,
    })
}

fn remove(args: &LibRemoveArgs) -> Result<()> {
    let lib_home = paths::lib_home();
    let outcome = remove_skill(&lib_home, &args.name, args.write)?;

    if outcome.written {
        println!(
            "{} removed '{}' ({}) from the central library",
            "✓".green(),
            outcome.name,
            outcome.source_kind
        );
        if let Some(dir) = &outcome.removed_dir {
            println!("  deleted {}", dir.display());
        }
    } else {
        println!(
            "Would remove '{}' ({}) from the central library:",
            outcome.name.bold(),
            outcome.source_kind
        );
        match &outcome.removed_dir {
            Some(dir) => println!("  {} delete {}", "−".yellow(), dir.display()),
            None if outcome.source_kind == "git" => {
                println!(
                    "  {} index entry only (store cache left in place)",
                    "−".yellow()
                )
            }
            None => println!("  {} index entry only", "−".yellow()),
        }
        println!("\nDry run. Re-run with {} to apply.", "--write".bold());
    }
    Ok(())
}

/// A migration plan/result: which skills were (or would be) migrated and which
/// on-disk entries were skipped and why.
#[derive(Debug)]
pub struct MigrateReport {
    pub migrated: Vec<String>,
    /// `(entry name, reason)` for directories that were not valid skills.
    pub skipped: Vec<(String, String)>,
    /// `(adapter id, link path)` for provider symlinks that were (or would be)
    /// re-pointed from the legacy home to the library.
    pub repointed: Vec<(String, PathBuf)>,
    pub written: bool,
}

/// Migrate skills from the legacy skills home (`old`) into the central library
/// at `lib_home`, reusing [`add_skill`] for each. **Copy-first and reversible**:
/// originals under `old` are never touched, so a failed or unwanted migration
/// leaves the source intact.
///
/// Only directories containing `SKILL.md` with a safe name are migrated; other
/// entries are recorded in `skipped`. Collisions with existing library entries
/// are a hard error (checked up front, before any write) unless `replace`.
pub fn migrate_skills(
    old: &Path,
    lib_home: &Path,
    replace: bool,
    write: bool,
) -> Result<MigrateReport> {
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();

    if old.exists() {
        let mut entries: Vec<_> = std::fs::read_dir(old)
            .with_context(|| format!("reading {}", old.display()))?
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if !path.is_dir() {
                continue; // ignore stray files at the skills-home root
            }
            if !path.join("SKILL.md").exists() {
                skipped.push((name, "no SKILL.md".into()));
                continue;
            }
            if !valid_lib_name(&name) {
                skipped.push((name, "unsafe name".into()));
                continue;
            }
            candidates.push((name, path));
        }
    }

    // Fail fast on collisions (matching `lib add`) before mutating anything.
    if !replace {
        let library = Library::load(lib_home)?;
        let collisions: Vec<String> = candidates
            .iter()
            .filter(|(n, _)| library.get(n).is_some())
            .map(|(n, _)| n.clone())
            .collect();
        if !collisions.is_empty() {
            bail!(
                "already in the central library: {} — pass --replace to overwrite",
                collisions.join(", ")
            );
        }
    }

    let mut migrated = Vec::new();
    for (name, path) in &candidates {
        // Migration adopts the user's own existing skills; scan findings are
        // surfaced by audit/doctor, never a reason to block a move here.
        add_skill(lib_home, name, LibSource::Path(path), replace, write, true)?;
        migrated.push(name.clone());
    }

    // Finish the move: any CLI's global skills symlink still targeting the
    // legacy home is re-pointed at the library copy, so harnesses actually
    // read from the library after a migration (legacy content stays intact,
    // so this remains reversible).
    let repointed = repoint_provider_links(old, lib_home, &migrated, write)?;

    Ok(MigrateReport {
        migrated,
        skipped,
        repointed,
        written: write,
    })
}

/// Whether a symlink (at `link_dir/<name>`, pointing at `target`) resolves
/// into the legacy skills home `old`.
fn link_targets_legacy(target: &Path, link_dir: &Path, old: &Path) -> bool {
    let absolute = if target.is_absolute() {
        target.to_path_buf()
    } else {
        link_dir.join(target)
    };
    // Normalize `..` components lexically (no fs access, so dry-run works on
    // links whose targets are gone).
    let mut norm = PathBuf::new();
    for c in absolute.components() {
        match c {
            std::path::Component::ParentDir => {
                norm.pop();
            }
            std::path::Component::CurDir => {}
            other => norm.push(other),
        }
    }
    norm.starts_with(old)
}

/// Re-point per-CLI global skills symlinks from the legacy home to the
/// library for every migrated skill. Returns `(adapter id, link path)` pairs.
fn repoint_provider_links(
    old: &Path,
    lib_home: &Path,
    migrated: &[String],
    write: bool,
) -> Result<Vec<(String, PathBuf)>> {
    let mut repointed = Vec::new();
    let registry = crate::adapter::Registry::load()?;
    for desc in registry.iter() {
        let Some(dir) = desc.skills_dir_for(crate::scope::Scope::Global, Path::new(".")) else {
            continue;
        };
        for name in migrated {
            let link = dir.join(name);
            let Ok(target) = std::fs::read_link(&link) else {
                continue; // not a symlink (or absent) — never touch real dirs
            };
            if !link_targets_legacy(&target, &dir, old) {
                continue;
            }
            let dest = lib_home.join("skills").join(name);
            if !dest.exists() {
                continue;
            }
            if write {
                std::fs::remove_file(&link)
                    .with_context(|| format!("removing old link {}", link.display()))?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(&dest, &link)
                    .with_context(|| format!("linking {} → {}", link.display(), dest.display()))?;
                #[cfg(not(unix))]
                bail!("re-pointing symlinks is unix-only for now");
            }
            repointed.push((desc.id.clone(), link));
        }
    }
    Ok(repointed)
}

fn migrate(args: &LibMigrateArgs) -> Result<()> {
    let old = paths::skills_home();
    let lib_home = paths::lib_home();
    let report = migrate_skills(&old, &lib_home, args.replace, args.write)?;

    if report.migrated.is_empty() && report.skipped.is_empty() {
        println!("Nothing to migrate — {} is empty or absent.", old.display());
        return Ok(());
    }

    let verb = if report.written {
        "Migrated"
    } else {
        "Would migrate"
    };
    println!(
        "{verb} {} skill(s) from {} → {}:",
        report.migrated.len(),
        old.display(),
        lib_home.join("skills").display()
    );
    for n in &report.migrated {
        let mark = if report.written {
            "✓".green().to_string()
        } else {
            "→".cyan().to_string()
        };
        println!("  {mark} {n}");
    }
    for (n, why) in &report.skipped {
        println!("  {} skipped {n} — {why}", "⚠".yellow());
    }
    if !report.repointed.is_empty() {
        let verb = if report.written {
            "Re-pointed"
        } else {
            "Would re-point"
        };
        println!(
            "\n{verb} {} CLI symlink(s) from the legacy home to the library:",
            report.repointed.len()
        );
        for (cli, link) in &report.repointed {
            let mark = if report.written {
                "✓".green().to_string()
            } else {
                "→".cyan().to_string()
            };
            println!("  {mark} {} ({cli})", link.display());
        }
    }

    if report.written {
        println!(
            "\nOriginals left in place at {} (migration is reversible).",
            old.display()
        );
    } else {
        println!("\nDry run. Re-run with {} to apply.", "--write".bold());
    }
    Ok(())
}

/// Resolve a library entry's `path` to the exact contained `lib/skills/<...>`
/// dir that is safe to `remove_dir_all`. Rejects any path with a `.`, `..`,
/// root, or drive-prefix component so a hand-edited index can never delete
/// outside the library. `None` → leave the filesystem untouched.
fn contained_lib_skill_dir(lib_home: &Path, path: &str) -> Option<PathBuf> {
    let rel = Path::new(path.trim_start_matches("./"));
    let mut comps = 0;
    for c in rel.components() {
        if !matches!(c, std::path::Component::Normal(_)) {
            return None;
        }
        comps += 1;
    }
    if comps == 0 {
        return None;
    }
    Some(lib_home.join("skills").join(rel))
}

/// A name safe to use as a `lib/skills/<name>` directory and index key.
fn valid_lib_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && name != "." && name != ".."
}

/// Resolve a possibly-relative, possibly-`~` path to an absolute one.
fn absolutize(p: &Path) -> Result<PathBuf> {
    let expanded = paths::expand_tilde(&p.to_string_lossy());
    if expanded.is_absolute() {
        Ok(expanded)
    } else {
        Ok(std::env::current_dir()?.join(expanded))
    }
}

/// Whether a path lives under an ephemeral location — the OS temp dir,
/// `$TMPDIR`, or the conventional `/tmp` / `/private/tmp` — so recording it as
/// provenance would immortalize a path that won't exist later. Raw and
/// canonicalized forms are both compared, so macOS's `/tmp → /private/tmp`
/// symlink can't hide a match.
fn is_temp_path(p: &Path) -> bool {
    let mut roots = vec![
        std::env::temp_dir(),
        PathBuf::from("/tmp"),
        PathBuf::from("/private/tmp"),
    ];
    if let Ok(t) = std::env::var("TMPDIR") {
        if !t.is_empty() {
            roots.push(PathBuf::from(t));
        }
    }
    let p_canon = std::fs::canonicalize(p).ok();
    roots.iter().any(|r| {
        let r_canon = std::fs::canonicalize(r).ok();
        let under = |base: &Path| {
            p.starts_with(base) || p_canon.as_deref().is_some_and(|pc| pc.starts_with(base))
        };
        under(r) || r_canon.as_deref().is_some_and(under)
    })
}

// ---------- lib sync (cross-machine) ----------
//
// The central library (`~/.agentstack/lib`) is versioned as an ordinary git
// repo the user pushes/pulls across machines. The content-store cache lives
// *outside* it (`~/.agentstack/store`), so it never travels; server definitions
// carry `${REF}` placeholders only, so no secret value is ever committed.

const LIB_GITIGNORE: &str = "# agentstack central library — synced across machines.\n\
     # The content store cache lives outside this repo (~/.agentstack/store) and\n\
     # never travels. Nothing secret belongs here: server defs are ${REF} only.\n\
     .DS_Store\n";

/// Run git in `dir`, returning trimmed stdout; a non-zero exit is an error
/// carrying git's stderr.
fn git_out(dir: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("running git (is it installed?)")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Whether a git invocation succeeds — for probing state (remote set, upstream).
fn git_ok(dir: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn sync(args: &LibSyncArgs) -> Result<()> {
    let lib = paths::lib_home();

    if args.init {
        return sync_init(&lib, args.remote.as_deref());
    }
    if !lib.join(".git").exists() {
        bail!(
            "the central library at {} is not a git repo yet — run \
             `agentstack lib sync --init [--remote <url>]`",
            lib.display()
        );
    }
    if let Some(url) = &args.remote {
        set_remote(&lib, url)?;
        println!("{} remote set → {url}", "✓".green());
    }
    if args.status {
        return sync_status(&lib);
    }
    sync_now(&lib, args.message.as_deref(), args.allow_secrets)
}

/// First-time setup. With a remote and an empty/absent library, clone it (fresh
/// machine); otherwise `git init` the local library in place.
fn sync_init(lib: &Path, remote: Option<&str>) -> Result<()> {
    if lib.join(".git").exists() {
        println!("{} already a git repo at {}", "✓".green(), lib.display());
        if let Some(url) = remote {
            set_remote(lib, url)?;
            println!("  remote → {url}");
        }
        return Ok(());
    }

    let empty = std::fs::read_dir(lib)
        .map(|mut d| d.next().is_none())
        .unwrap_or(true);
    if empty {
        let Some(url) = remote else {
            bail!(
                "the central library at {} is empty — add a skill/server first, \
                 or pass --remote <url> to clone an existing library",
                lib.display()
            );
        };
        if let Some(parent) = lib.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let out = std::process::Command::new("git")
            .args(["clone", url, &lib.to_string_lossy()])
            .output()
            .context("running git clone")?;
        if !out.status.success() {
            bail!(
                "git clone failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        println!(
            "{} cloned library from {url} → {}",
            "✓".green(),
            lib.display()
        );
        return Ok(());
    }

    std::fs::create_dir_all(lib)?;
    git_out(lib, &["init"])?;
    let gitignore = lib.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(&gitignore, LIB_GITIGNORE)
            .with_context(|| format!("writing {}", gitignore.display()))?;
    }
    git_out(lib, &["add", "-A"])?;
    git_out(lib, &["commit", "-m", "agentstack library snapshot"])?;
    println!(
        "{} initialized library git repo at {}",
        "✓".green(),
        lib.display()
    );
    if let Some(url) = remote {
        set_remote(lib, url)?;
        println!("  remote → {url}");
        println!("  run `agentstack lib sync` to push");
    }
    Ok(())
}

/// Add or update the `origin` remote.
fn set_remote(lib: &Path, url: &str) -> Result<()> {
    if git_ok(lib, &["remote", "get-url", "origin"]) {
        git_out(lib, &["remote", "set-url", "origin", url])?;
    } else {
        git_out(lib, &["remote", "add", "origin", url])?;
    }
    Ok(())
}

/// Read-only report: working-tree changes + ahead/behind vs. the remote.
fn sync_status(lib: &Path) -> Result<()> {
    let dirty = git_out(lib, &["status", "--short"])?;
    if dirty.is_empty() {
        println!("{} working tree clean", "✓".green());
    } else {
        println!("{} local changes:", "→".cyan());
        for line in dirty.lines() {
            println!("    {line}");
        }
    }
    if git_ok(lib, &["rev-parse", "--abbrev-ref", "@{u}"]) {
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(lib)
            .args(["fetch", "--quiet"])
            .output();
        if let Ok(counts) = git_out(lib, &["rev-list", "--left-right", "--count", "@{u}...HEAD"]) {
            let mut it = counts.split_whitespace();
            let behind = it.next().unwrap_or("0");
            let ahead = it.next().unwrap_or("0");
            println!("  {ahead} ahead, {behind} behind the remote");
        }
    } else {
        println!("  no remote tracking branch yet (run `agentstack lib sync` to push)");
    }
    Ok(())
}

/// Commit local changes, then pull + push if a remote is configured.
fn sync_now(lib: &Path, message: Option<&str>, allow_secrets: bool) -> Result<()> {
    // A half-finished rebase must be resolved first: committing on top of one
    // and re-pulling loops forever, so don't send the user in a circle.
    if lib.join(".git/rebase-merge").exists() || lib.join(".git/rebase-apply").exists() {
        let d = lib.display();
        bail!(
            "a rebase is already in progress in {d} — finish it first:\n  \
             git -C {d} rebase --continue   (after resolving conflicts)\n  \
             git -C {d} rebase --abort      (to back out)"
        );
    }

    // The library's core promise is that secrets never travel. Enforce it: a
    // server definition with a literal (non-`${REF}`) secret blocks the push,
    // across every server field (headers, env, url, args). The outgoing-history
    // scan below covers a secret that was committed once and later edited out —
    // still in the commits that carry it, even though the working tree is clean.
    let leaks = library_secret_leaks(lib);
    if !leaks.is_empty() && !allow_secrets {
        let list = leaks
            .iter()
            .map(|l| format!("    {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "refusing to sync — a server definition holds a literal secret (use ${{REF}} \
             instead, or pass --allow-secrets to override):\n{list}"
        );
    }

    // Commit any local changes.
    let dirty = git_out(lib, &["status", "--short"])?;
    if dirty.is_empty() {
        println!("{} no local changes", "·".dimmed());
    } else {
        git_out(lib, &["add", "-A"])?;
        git_out(
            lib,
            &["commit", "-m", message.unwrap_or("agentstack library sync")],
        )?;
        println!("{} committed local changes", "✓".green());
    }

    // Nothing committed yet (a fresh clone of an empty remote, no local changes).
    if !git_ok(lib, &["rev-parse", "--verify", "HEAD"]) {
        println!("  nothing to push yet — add a skill/server first");
        return Ok(());
    }
    if !git_ok(lib, &["remote", "get-url", "origin"]) {
        println!("  no remote configured — run `agentstack lib sync --remote <url>` to set one");
        return Ok(());
    }
    let branch = git_out(lib, &["rev-parse", "--abbrev-ref", "HEAD"])?;

    // Bring the remote in before pushing. Three cases: an upstream is set
    // (normal), no upstream but the remote branch already exists (a second
    // machine that ran --init locally), or an empty remote (first push).
    let has_upstream = git_ok(lib, &["rev-parse", "--abbrev-ref", "@{u}"]);
    // The explicit fetch + probe only serve the no-upstream case (a second
    // machine whose remote branch exists but isn't tracked yet). With an
    // upstream, `git pull` fetches for us — fetching here too is a wasted
    // round-trip.
    let mut remote_has_branch = false;
    if !has_upstream {
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(lib)
            .args(["fetch", "origin", "--quiet"])
            .output();
        remote_has_branch = git_ok(lib, &["rev-parse", "--verify", &format!("origin/{branch}")]);
    }

    if has_upstream || remote_has_branch {
        // Record HEAD so an unchanged pull can skip re-scanning long-accepted
        // content (a no-op "Already up to date" leaves HEAD untouched).
        let before = git_out(lib, &["rev-parse", "HEAD"]).ok();
        let mut pull = vec!["pull", "--rebase"];
        if !has_upstream {
            pull.extend(["origin", branch.as_str()]);
        }
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(lib)
            .args(&pull)
            .output()
            .context("running git pull")?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let el = err.to_lowercase();
            let d = lib.display();
            // Only point at `rebase --continue` when a rebase is actually
            // paused; offline/auth failures leave none and that command errors.
            let rebasing = lib.join(".git/rebase-merge").exists()
                || lib.join(".git/rebase-apply").exists()
                || el.contains("conflict");
            let network_auth = [
                "could not resolve host",
                "could not read from remote",
                "authentication failed",
                "permission denied",
                "unable to access",
            ]
            .iter()
            .any(|s| el.contains(s));
            let hint = if err.contains("unrelated histories") {
                format!(
                    "\nthe local and remote libraries have separate histories — back up {d} and \
                     re-clone with `agentstack lib sync --init --remote <url>` into an empty \
                     library, or reconcile manually: \
                     `git -C {d} pull --rebase --allow-unrelated-histories origin {branch}`"
                )
            } else if rebasing {
                format!(
                    "\nresolve conflicts in {d}, then `git -C {d} rebase --continue` and re-run"
                )
            } else if network_auth {
                "\ncheck your connection and credentials, then re-run `agentstack lib sync`".into()
            } else {
                format!("\ngit pull failed in {d} — see the error above and re-run")
            };
            bail!("git pull --rebase failed: {}{hint}", err.trim());
        }
        println!("{} pulled from remote", "✓".green());
        // The same supply-chain gate as `lib add`, applied to pulled content —
        // warn-only, since blocking a completed pull would strand the working
        // tree. Scan only what the pull actually moved (F8): an unchanged HEAD
        // skips it; a moved HEAD scans just the changed skills.
        match before {
            Some(old) => {
                let now = git_out(lib, &["rev-parse", "HEAD"]).unwrap_or_default();
                if now != old {
                    match git_out(lib, &["diff", "--name-only", &format!("{old}..HEAD")]) {
                        Ok(changed) => scan_changed_skills(lib, &changed),
                        Err(_) => scan_pulled_skills(lib),
                    }
                }
            }
            None => scan_pulled_skills(lib),
        }
    }

    // A secret committed once (via plain git or --allow-secrets) and later
    // edited out is gone from the working tree but still in the commits about to
    // be pushed — scan the outgoing range so the plaintext can't ride along.
    let range = if has_upstream {
        Some("@{u}..HEAD".to_string())
    } else if remote_has_branch {
        Some(format!("origin/{branch}..HEAD"))
    } else {
        None // first push: the whole local history is outgoing
    };
    let outgoing = outgoing_secret_leaks(lib, range.as_deref());
    if !outgoing.is_empty() && !allow_secrets {
        let list = outgoing
            .iter()
            .map(|l| format!("    {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "refusing to sync — a secret in an earlier commit is still in the outgoing history \
             (rewrite that history, or pass --allow-secrets to override):\n{list}"
        );
    }

    if has_upstream {
        git_out(lib, &["push"])?;
    } else {
        git_out(lib, &["push", "-u", "origin", &branch])?;
    }
    println!("{} pushed to remote", "✓".green());
    Ok(())
}

/// Server definitions in the library that carry a literal (non-`${REF}`) secret,
/// across every field (headers, env, url, args). An unreadable or unparseable
/// `servers/*.toml` is itself a leak entry: the gate fails closed rather than
/// waving through a hand-edited file it can't inspect (a broken TOML still
/// carries its plaintext into the push).
fn library_secret_leaks(lib: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(lib.join("servers")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let Ok(text) = std::fs::read_to_string(&path) else {
            out.push(format!("{name}: cannot be read — fix it before syncing"));
            continue;
        };
        match toml::from_str::<Server>(&text) {
            Ok(server) => {
                for w in suspicious_secrets(&server) {
                    out.push(format!("{name}: {w}"));
                }
            }
            Err(_) => {
                out.push(format!(
                    "{name}: cannot be parsed as a server definition — fix it before syncing"
                ));
                // Won't parse, but a secret-looking line still travels — surface
                // it by name from the raw text so the message is actionable.
                for line in text.lines() {
                    for key in secretish_keys_in_line(line) {
                        out.push(format!(
                            "{name}: '{key}' has a literal value that looks like a secret — use ${{REF}} instead"
                        ));
                    }
                }
            }
        }
    }
    out
}

/// Best-effort content scan of pulled skills (warn-only — a compromised or
/// shared remote is an ingestion path, but blocking a finished pull strands the
/// tree, so surface findings for review rather than failing).
fn scan_pulled_skills(lib: &Path) {
    let skills = lib.join("skills");
    if !skills.exists() {
        return;
    }
    if let Ok(findings) = crate::scan::scan_tree(&skills) {
        for f in &findings {
            println!("  {} pulled content: {}", "⚠".yellow(), f.describe());
        }
    }
}

/// Scan only the skills a pull touched (paths from `git diff --name-only`), so
/// long-accepted content isn't re-flagged every sync (F8). Each changed
/// `skills/<name>/…` path maps to its skill subtree, scanned once.
fn scan_changed_skills(lib: &Path, changed: &str) {
    let mut seen = std::collections::BTreeSet::new();
    for path in changed.lines() {
        let Some(rest) = path.strip_prefix("skills/") else {
            continue;
        };
        let name = rest.split('/').next().unwrap_or("");
        if name.is_empty() || !seen.insert(name.to_string()) {
            continue;
        }
        let subtree = lib.join("skills").join(name);
        if !subtree.exists() {
            continue; // removed by the pull
        }
        if let Ok(findings) = crate::scan::scan_tree(&subtree) {
            for f in &findings {
                println!("  {} pulled content: {}", "⚠".yellow(), f.describe());
            }
        }
    }
}

/// Literal secrets in the commits about to be pushed. The working-tree gate only
/// sees current files; a secret committed once and edited out still travels in
/// its commit. Scan the added `servers/…` lines across the outgoing range
/// (`None` = the whole history, for a first push). Best-effort: a git failure
/// yields no leaks rather than bricking sync on an odd repo state — but any leak
/// found blocks the push.
fn outgoing_secret_leaks(lib: &Path, range: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = ["log", "-p", "--no-color", "-U0"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Some(r) = range {
        args.push(r.to_string());
    }
    args.push("--".into());
    args.push("servers".into());
    let argrefs: Vec<&str> = args.iter().map(String::as_str).collect();
    let Ok(diff) = git_out(lib, &argrefs) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut commit = String::new();
    let mut file = String::new();
    for line in diff.lines() {
        if let Some(h) = line.strip_prefix("commit ") {
            commit = h
                .split_whitespace()
                .next()
                .unwrap_or(h)
                .chars()
                .take(8)
                .collect();
        } else if let Some(f) = line.strip_prefix("+++ b/") {
            file = f.to_string();
        } else if let Some(added) = line.strip_prefix('+') {
            if added.starts_with("++") {
                continue; // a `+++` header, already handled above
            }
            for key in secretish_keys_in_line(added) {
                out.push(format!(
                    "commit {commit} {file}: '{key}' has a literal value that looks like a secret — use ${{REF}} instead"
                ));
            }
        }
    }
    out
}

fn require_skill_md(dir: &Path) -> Result<()> {
    if !dir.join("SKILL.md").exists() {
        bail!(
            "no SKILL.md in {} — not a valid skill directory",
            dir.display()
        );
    }
    Ok(())
}

/// Supply-chain content gate for `lib add` (plan §3): scan the resolved skill
/// dir before it becomes the canonical library copy. High findings (hidden
/// Unicode) block the add — the same philosophy as unresolved secrets blocking
/// writes — unless `allow_flagged` overrides. Warn findings (injection
/// heuristics) are appended to `warnings` and never block. Pulls in the same
/// scanner `agentstack audit`/`install` use, so the trust story is uniform.
fn scan_gate(
    name: &str,
    dir: &Path,
    allow_flagged: bool,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let findings =
        crate::scan::scan_tree(dir).with_context(|| format!("scanning {}", dir.display()))?;
    let high: Vec<_> = findings
        .iter()
        .filter(|f| f.severity == Severity::High)
        .collect();
    if !high.is_empty() && !allow_flagged {
        let list = high
            .iter()
            .map(|f| format!("    {}", f.describe()))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "'{name}': {} high-severity content finding(s) — add blocked \
             (pass --allow-flagged to add anyway):\n{list}",
            high.len()
        );
    }
    for f in &findings {
        warnings.push(format!("[{}] {}", f.severity.label(), f.describe()));
    }
    Ok(())
}

/// Whether two paths point at the same directory (best-effort via canonicalize).
fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// First 12 chars of a checksum, for a glanceable pin.
fn short(sum: &str) -> &str {
    &sum[..sum.len().min(12)]
}

/// Human-readable byte count (binary units, one decimal).
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    fn src_skill(dir: &assert_fs::TempDir, body: &str) -> PathBuf {
        dir.child("src/SKILL.md").write_str(body).unwrap();
        dir.child("src").path().to_path_buf()
    }

    #[test]
    fn add_path_copies_digests_and_records_provenance() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src = src_skill(&work, "# body\n");

        let out = add_skill(
            lib.path(),
            "sql-review",
            LibSource::Path(&src),
            false,
            true,
            false,
        )
        .unwrap();

        assert!(out.written);
        assert_eq!(out.source_kind, "path");
        assert_eq!(out.checksum.len(), 64);
        // Files landed under lib/skills/<name>.
        assert!(lib.child("skills/sql-review/SKILL.md").path().exists());
        // Index records the entry with checksum + provenance.
        let library = Library::load(lib.path()).unwrap();
        let entry = library.get("sql-review").unwrap();
        assert_eq!(entry.path.as_deref(), Some("sql-review"));
        assert_eq!(entry.checksum.as_deref(), Some(out.checksum.as_str()));
        assert!(entry.provenance.as_deref().unwrap().starts_with("path:"));
    }

    #[test]
    fn temp_dir_source_warns_and_records_source_path() {
        let lib = assert_fs::TempDir::new().unwrap();
        // assert_fs temp dirs live under the OS temp dir — exactly the
        // ephemeral-provenance case.
        let work = assert_fs::TempDir::new().unwrap();
        let src = src_skill(&work, "# body\n");

        let out = add_skill(lib.path(), "eph", LibSource::Path(&src), false, true, false).unwrap();

        assert!(out.source_path.is_some(), "source path surfaced for output");
        assert!(
            out.warnings.iter().any(|w| w.contains("temporary")),
            "temp source flagged: {:?}",
            out.warnings
        );
        // The copy itself still lands and is indexed as usual.
        assert!(lib.child("skills/eph/SKILL.md").path().exists());
    }

    #[test]
    fn is_temp_path_matches_temp_roots_only() {
        assert!(is_temp_path(&std::env::temp_dir().join("x")));
        assert!(is_temp_path(Path::new("/tmp/skill-src")));
        assert!(is_temp_path(Path::new("/private/tmp/skill-src")));
        assert!(!is_temp_path(Path::new("/opt/team/skills/sql-review")));
    }

    #[test]
    fn add_reports_total_bytes_for_size_warnings() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        work.child("src/SKILL.md").write_str("# body\n").unwrap();
        work.child("src/vendored/blob.bin")
            .write_binary(&[0u8; 4096])
            .unwrap();
        let src = work.child("src").path().to_path_buf();

        // Preview and write both surface the size (the preview is where the
        // warning is most useful — before the copy happens).
        let dry = add_skill(
            lib.path(),
            "big",
            LibSource::Path(&src),
            false,
            false,
            false,
        )
        .unwrap();
        assert!(dry.total_bytes >= 4096, "got {}", dry.total_bytes);
        let wet = add_skill(lib.path(), "big", LibSource::Path(&src), false, true, false).unwrap();
        assert_eq!(wet.total_bytes, dry.total_bytes);
    }

    #[test]
    fn human_bytes_picks_sane_units() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(10 * 1024 * 1024 + 512 * 1024), "10.5 MiB");
        assert_eq!(human_bytes(389 * 1024 * 1024), "389.0 MiB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn oversized_skill_warns_in_outcome() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        work.child("src/SKILL.md").write_str("# body\n").unwrap();
        work.child("src/vendored/blob.bin")
            .write_binary(&vec![0u8; LARGE_SKILL_BYTES as usize + 1])
            .unwrap();
        let src = work.child("src").path().to_path_buf();

        let out = add_skill(
            lib.path(),
            "huge",
            LibSource::Path(&src),
            false,
            false,
            false,
        )
        .unwrap();
        assert!(
            out.warnings.iter().any(|w| w.contains("full-library pass")),
            "size warning surfaced on the outcome: {:?}",
            out.warnings
        );
    }

    #[test]
    fn dry_run_writes_nothing() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src = src_skill(&work, "# body\n");

        let out = add_skill(lib.path(), "x", LibSource::Path(&src), false, false, false).unwrap();

        assert!(!out.written);
        assert_eq!(out.checksum.len(), 64, "preview still digests the source");
        assert!(!lib.child("skills/x").path().exists(), "no files written");
        assert!(Library::load(lib.path()).unwrap().get("x").is_none());
    }

    #[test]
    fn collision_without_replace_is_error() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src = src_skill(&work, "# body\n");
        add_skill(lib.path(), "x", LibSource::Path(&src), false, true, false).unwrap();

        let err =
            add_skill(lib.path(), "x", LibSource::Path(&src), false, true, false).unwrap_err();
        assert!(err.to_string().contains("--replace"));
    }

    #[test]
    fn replace_overwrites_content_and_digest() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src1 = src_skill(&work, "# original\n");
        let first = add_skill(lib.path(), "x", LibSource::Path(&src1), false, true, false).unwrap();

        // A different source body under the same name, with --replace.
        let work2 = assert_fs::TempDir::new().unwrap();
        let src2 = src_skill(&work2, "# changed\n");
        let second = add_skill(lib.path(), "x", LibSource::Path(&src2), true, true, false).unwrap();

        assert!(second.replaced);
        assert_ne!(first.checksum, second.checksum);
        let body = std::fs::read_to_string(lib.child("skills/x/SKILL.md").path()).unwrap();
        assert_eq!(body, "# changed\n");
    }

    #[test]
    fn missing_skill_md_is_error() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        work.child("src/notes.txt").write_str("x").unwrap();
        let src = work.child("src").path().to_path_buf();

        let err =
            add_skill(lib.path(), "x", LibSource::Path(&src), false, true, false).unwrap_err();
        assert!(err.to_string().contains("SKILL.md"));
    }

    #[test]
    fn invalid_name_is_error() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src = src_skill(&work, "# body\n");
        let err = add_skill(
            lib.path(),
            "../escape",
            LibSource::Path(&src),
            false,
            true,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid library skill name"));
    }

    fn path_entry(name: &str, checksum: &str) -> LibrarySkill {
        LibrarySkill {
            name: name.into(),
            source: "path".into(),
            path: Some(name.into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: Some(checksum.into()),
            version: None,
            provenance: Some(format!("path:/src/{name}")),
        }
    }

    #[test]
    fn list_empty_says_none() {
        let out = render_list(&Library::default());
        assert!(out.contains("No skills or servers installed"));
    }

    #[test]
    fn list_path_row_shows_name_source_checksum_provenance() {
        let mut library = Library::default();
        library.upsert(path_entry("sql-review", "abcdef0123456789deadbeef"));
        let out = render_list(&library);
        assert!(out.contains("sql-review"));
        assert!(out.contains("path"));
        assert!(out.contains("abcdef012345"), "short checksum (12 chars)");
        assert!(out.contains("path:/src/sql-review"), "provenance");
    }

    #[test]
    fn list_git_row_shows_short_rev() {
        let mut library = Library::default();
        library.upsert(LibrarySkill {
            name: "gitskill".into(),
            source: "git".into(),
            path: None,
            git: Some("https://example.com/x.git".into()),
            rev: Some("0123456789abcdef0123456789abcdef01234567".into()),
            subpath: None,
            checksum: Some("feedface00001111".into()),
            version: None,
            provenance: Some("git:https://example.com/x.git".into()),
        });
        let out = render_list(&library);
        assert!(out.contains("git"));
        assert!(
            out.contains("rev 0123456789ab"),
            "short rev preferred for git"
        );
    }

    #[test]
    fn list_is_sorted_by_name() {
        let mut library = Library::default();
        // Insert out of order; render must sort.
        library.skills.push(path_entry("zebra", "1111"));
        library.skills.push(path_entry("alpha", "2222"));
        let out = render_list(&library);
        let a = out.find("alpha").unwrap();
        let z = out.find("zebra").unwrap();
        assert!(a < z, "rows sorted by name");
    }

    #[test]
    fn add_git_records_rev_and_checksum() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());

        // A local git repo used as the source.
        let work = assert_fs::TempDir::new().unwrap();
        let repo = work.child("repo");
        repo.create_dir_all().unwrap();
        let git = |a: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(a)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {a:?} failed");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@e.st"]);
        git(&["config", "user.name", "t"]);
        repo.child("SKILL.md").write_str("# git skill\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);

        let lib_home = home.child("lib");
        let url = format!("file://{}", repo.path().display());
        let out = add_skill(
            lib_home.path(),
            "gitskill",
            LibSource::Git {
                url: &url,
                rev: None,
                subpath: None,
            },
            false,
            true,
            false,
        )
        .unwrap();

        assert_eq!(out.source_kind, "git");
        assert_eq!(out.checksum.len(), 64);
        let library = Library::load(lib_home.path()).unwrap();
        let entry = library.get("gitskill").unwrap();
        assert_eq!(entry.git.as_deref(), Some(url.as_str()));
        assert!(entry.rev.is_some());
        assert!(entry.provenance.as_deref().unwrap().starts_with("git:"));

        std::env::remove_var("AGENTSTACK_HOME");
    }

    #[test]
    fn add_git_subpath_installs_subdir_skill() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());

        // A repo whose skill lives in a subdir (marketplace/monorepo layout),
        // with NO SKILL.md at the root — the shape that blocked before.
        let work = assert_fs::TempDir::new().unwrap();
        let repo = work.child("repo");
        repo.create_dir_all().unwrap();
        let git = |a: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(a)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {a:?} failed");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@e.st"]);
        git(&["config", "user.name", "t"]);
        repo.child("README.md").write_str("# monorepo\n").unwrap();
        repo.child("skills/improve/SKILL.md")
            .write_str("# improve\n")
            .unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);

        let lib_home = home.child("lib");
        let url = format!("file://{}", repo.path().display());
        let out = add_skill(
            lib_home.path(),
            "improve",
            LibSource::Git {
                url: &url,
                rev: None,
                subpath: Some("skills/improve"),
            },
            false,
            true,
            false,
        )
        .unwrap();

        assert_eq!(out.source_kind, "git");
        let library = Library::load(lib_home.path()).unwrap();
        let entry = library.get("improve").unwrap();
        assert_eq!(entry.subpath.as_deref(), Some("skills/improve"));
        // Truthful provenance: url @ rev # subpath (plan §6).
        let prov = entry.provenance.as_deref().unwrap();
        assert!(prov.starts_with("git:"), "{prov}");
        assert!(prov.contains('@'), "records the resolved rev: {prov}");
        assert!(prov.ends_with("#skills/improve"), "records subpath: {prov}");

        std::env::remove_var("AGENTSTACK_HOME");
    }

    #[test]
    fn add_blocks_on_hidden_unicode_unless_allowed() {
        let lib = assert_fs::TempDir::new().unwrap();
        let src = assert_fs::TempDir::new().unwrap();
        // A zero-width space smuggled into the skill body — a high finding.
        src.child("SKILL.md")
            .write_str("# skill\nhidden\u{200B}payload\n")
            .unwrap();

        let blocked = add_skill(
            lib.path(),
            "sneaky",
            LibSource::Path(src.path()),
            false,
            true,
            false,
        )
        .unwrap_err();
        assert!(
            blocked.to_string().contains("high-severity"),
            "scan should block: {blocked}"
        );
        assert!(
            Library::load(lib.path()).unwrap().get("sneaky").is_none(),
            "nothing written when blocked"
        );

        // --allow-flagged (last arg) overrides the block.
        let out = add_skill(
            lib.path(),
            "sneaky",
            LibSource::Path(src.path()),
            false,
            true,
            true,
        )
        .unwrap();
        assert!(out.written);
        assert!(
            out.warnings.iter().any(|w| w.contains("hidden unicode")),
            "finding still surfaced as a warning: {:?}",
            out.warnings
        );
    }

    // ---------- remove ----------

    #[test]
    fn remove_dry_run_leaves_entry_and_files() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src = src_skill(&work, "# body\n");
        add_skill(lib.path(), "x", LibSource::Path(&src), false, true, false).unwrap();

        let out = remove_skill(lib.path(), "x", false).unwrap();

        assert!(!out.written);
        assert!(lib.child("skills/x/SKILL.md").path().exists(), "files kept");
        assert!(
            Library::load(lib.path()).unwrap().get("x").is_some(),
            "entry kept"
        );
    }

    #[test]
    fn remove_write_deletes_path_entry_and_files() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src = src_skill(&work, "# body\n");
        add_skill(lib.path(), "x", LibSource::Path(&src), false, true, false).unwrap();

        let out = remove_skill(lib.path(), "x", true).unwrap();

        assert!(out.written);
        assert_eq!(
            out.removed_dir.as_deref(),
            Some(lib.child("skills/x").path())
        );
        assert!(!lib.child("skills/x").path().exists(), "dir deleted");
        assert!(
            Library::load(lib.path()).unwrap().get("x").is_none(),
            "entry gone"
        );
    }

    #[test]
    fn remove_git_leaves_store_cache_alone() {
        let lib = assert_fs::TempDir::new().unwrap();
        // A git entry whose "cache" lives outside lib/skills — must be untouched.
        let cache = assert_fs::TempDir::new().unwrap();
        cache.child("SKILL.md").write_str("# cached\n").unwrap();
        let mut library = Library::default();
        library.upsert(LibrarySkill {
            name: "gitskill".into(),
            source: "git".into(),
            path: None,
            git: Some("https://example.com/x.git".into()),
            rev: Some("abc123".into()),
            subpath: None,
            checksum: Some("deadbeef".into()),
            version: None,
            provenance: Some("git:https://example.com/x.git".into()),
        });
        library.save(lib.path()).unwrap();

        let out = remove_skill(lib.path(), "gitskill", true).unwrap();

        assert!(out.written);
        assert_eq!(out.removed_dir, None, "git entries delete no files");
        assert!(
            cache.child("SKILL.md").path().exists(),
            "store cache untouched"
        );
        assert!(Library::load(lib.path()).unwrap().get("gitskill").is_none());
    }

    #[test]
    fn remove_missing_name_errors() {
        let lib = assert_fs::TempDir::new().unwrap();
        let err = remove_skill(lib.path(), "nope", true).unwrap_err();
        assert!(err.to_string().contains("not in the central library"));
    }

    #[test]
    fn remove_never_deletes_outside_the_library() {
        let lib = assert_fs::TempDir::new().unwrap();
        // A directory outside the library that a malicious index path targets.
        let outside = assert_fs::TempDir::new().unwrap();
        outside.child("keep.txt").write_str("important\n").unwrap();

        // Hand-crafted index entry with an escaping path.
        let mut library = Library::default();
        library.upsert(LibrarySkill {
            name: "evil".into(),
            source: "path".into(),
            path: Some("../../../../../../../../etc".into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: Some("x".into()),
            version: None,
            provenance: None,
        });
        library.save(lib.path()).unwrap();

        let out = remove_skill(lib.path(), "evil", true).unwrap();

        // Uncontained path → no directory targeted for deletion...
        assert_eq!(out.removed_dir, None);
        // ...nothing outside is touched...
        assert!(outside.child("keep.txt").path().exists());
        // ...but the bogus index entry is still cleaned up.
        assert!(Library::load(lib.path()).unwrap().get("evil").is_none());
    }

    // ---------- migrate ----------

    /// Create `<old>/<name>/SKILL.md` in a legacy skills-home layout.
    fn old_skill(old: &Path, name: &str, body: &str) {
        let dir = old.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), body).unwrap();
    }

    #[test]
    fn migrate_empty_source_migrates_nothing() {
        let lib = assert_fs::TempDir::new().unwrap();
        let old = assert_fs::TempDir::new().unwrap();
        let report = migrate_skills(old.path(), lib.path(), false, true).unwrap();
        assert!(report.migrated.is_empty());
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn migrate_dry_run_writes_nothing() {
        let lib = assert_fs::TempDir::new().unwrap();
        let old = assert_fs::TempDir::new().unwrap();
        old_skill(old.path(), "a", "# a\n");
        old_skill(old.path(), "b", "# b\n");

        let report = migrate_skills(old.path(), lib.path(), false, false).unwrap();

        assert_eq!(report.migrated, vec!["a".to_string(), "b".to_string()]);
        assert!(!lib.child("skills/a").path().exists(), "no files written");
        assert!(
            Library::load(lib.path()).unwrap().skills.is_empty(),
            "index untouched"
        );
    }

    #[test]
    fn migrate_write_copies_and_indexes_multiple() {
        let lib = assert_fs::TempDir::new().unwrap();
        let old = assert_fs::TempDir::new().unwrap();
        old_skill(old.path(), "a", "# a\n");
        old_skill(old.path(), "b", "# b\n");

        let report = migrate_skills(old.path(), lib.path(), false, true).unwrap();

        assert_eq!(report.migrated.len(), 2);
        assert!(lib.child("skills/a/SKILL.md").path().exists());
        assert!(lib.child("skills/b/SKILL.md").path().exists());
        let library = Library::load(lib.path()).unwrap();
        assert!(library.get("a").is_some() && library.get("b").is_some());
        // Copy-first: originals remain.
        assert!(
            old.child("a/SKILL.md").path().exists(),
            "source left in place"
        );
        assert!(old.child("b/SKILL.md").path().exists());
    }

    #[test]
    fn migrate_collision_fails_without_replace_and_succeeds_with() {
        let lib = assert_fs::TempDir::new().unwrap();
        let old = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let src = src_skill(&work, "# existing\n");
        add_skill(lib.path(), "a", LibSource::Path(&src), false, true, false).unwrap();
        old_skill(old.path(), "a", "# migrated\n");

        // Same name already in the library → hard error, nothing written.
        let err = migrate_skills(old.path(), lib.path(), false, true).unwrap_err();
        assert!(err.to_string().contains("--replace"));

        // With --replace it overwrites.
        let report = migrate_skills(old.path(), lib.path(), true, true).unwrap();
        assert_eq!(report.migrated, vec!["a".to_string()]);
        let body = std::fs::read_to_string(lib.child("skills/a/SKILL.md").path()).unwrap();
        assert_eq!(body, "# migrated\n");
    }

    #[test]
    fn migrate_reports_dirs_without_skill_md() {
        let lib = assert_fs::TempDir::new().unwrap();
        let old = assert_fs::TempDir::new().unwrap();
        old_skill(old.path(), "good", "# good\n");
        // A directory that is not a valid skill.
        std::fs::create_dir_all(old.path().join("notaskill")).unwrap();
        std::fs::write(old.path().join("notaskill/readme.txt"), "x").unwrap();

        let report = migrate_skills(old.path(), lib.path(), false, true).unwrap();

        assert_eq!(report.migrated, vec!["good".to_string()]);
        assert!(report
            .skipped
            .iter()
            .any(|(n, why)| n == "notaskill" && why.contains("SKILL.md")));
        assert!(Library::load(lib.path())
            .unwrap()
            .get("notaskill")
            .is_none());
    }

    // ---------- servers (add / list / remove) ----------

    /// Write a server definition `.toml` (with a `${REF}` header) and return it.
    fn server_file(dir: &assert_fs::TempDir, name: &str, url: &str) -> PathBuf {
        let f = dir.child(format!("{name}.toml"));
        f.write_str(&format!(
            "type = \"http\"\nurl = \"{url}\"\nheaders = {{ Authorization = \"Bearer ${{TOKEN}}\" }}\n"
        ))
        .unwrap();
        f.path().to_path_buf()
    }

    #[test]
    fn add_server_writes_definition_and_index() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let file = server_file(&work, "kibana", "https://k/mcp");

        let out = add_server(lib.path(), "kibana", &file, false, true).unwrap();

        assert!(out.written);
        assert_eq!(out.checksum.len(), 64);
        // Definition file landed under lib/servers/<name>.toml.
        let dest = lib.child("servers/kibana.toml");
        assert!(dest.path().exists());
        // ${REF} preserved verbatim in the stored definition.
        let stored = std::fs::read_to_string(dest.path()).unwrap();
        assert!(stored.contains("${TOKEN}"), "ref preserved: {stored}");
        // Indexed with the checksum + provenance.
        let library = Library::load(lib.path()).unwrap();
        let entry = library.get_server("kibana").unwrap();
        assert_eq!(entry.checksum.as_deref(), Some(out.checksum.as_str()));
        assert!(entry.provenance.as_deref().unwrap().starts_with("file:"));
    }

    #[test]
    fn add_server_dry_run_writes_nothing() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let file = server_file(&work, "kibana", "https://k/mcp");

        let out = add_server(lib.path(), "kibana", &file, false, false).unwrap();
        assert!(!out.written);
        assert_eq!(out.checksum.len(), 64, "preview still digests");
        assert!(!lib.child("servers/kibana.toml").path().exists());
        assert!(Library::load(lib.path())
            .unwrap()
            .get_server("kibana")
            .is_none());
    }

    #[test]
    fn add_server_collision_and_replace() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let f1 = server_file(&work, "kibana", "https://one/mcp");
        add_server(lib.path(), "kibana", &f1, false, true).unwrap();

        let err = add_server(lib.path(), "kibana", &f1, false, true).unwrap_err();
        assert!(err.to_string().contains("--replace"));

        let work2 = assert_fs::TempDir::new().unwrap();
        let f2 = server_file(&work2, "kibana", "https://two/mcp");
        add_server(lib.path(), "kibana", &f2, true, true).unwrap();
        let stored = std::fs::read_to_string(lib.child("servers/kibana.toml").path()).unwrap();
        assert!(stored.contains("two/mcp"), "replaced definition");
    }

    #[test]
    fn add_server_malformed_file_errors() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let bad = work.child("bad.toml");
        bad.write_str("this is = not a { valid server").unwrap();
        let err = add_server(lib.path(), "kibana", bad.path(), false, true).unwrap_err();
        assert!(
            err.to_string().contains("valid MCP server") || err.to_string().contains("kibana"),
            "{err}"
        );
    }

    #[test]
    fn add_server_warns_on_literal_secret() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let f = work.child("kibana.toml");
        // A literal Authorization value (no ${REF}) is suspicious. The value is
        // obviously fake — the warning keys off the *header name*, not a pattern.
        f.write_str("type = \"http\"\nurl = \"https://k/mcp\"\nheaders = { Authorization = \"Bearer NOT-A-REAL-SECRET-example\" }\n")
            .unwrap();
        let out = add_server(lib.path(), "kibana", f.path(), false, true).unwrap();
        assert!(
            out.warnings.iter().any(|w| w.contains("Authorization")),
            "warns on literal secret: {:?}",
            out.warnings
        );
        // Warned, but not blocked and not scrubbed.
        assert!(out.written);
    }

    #[test]
    fn add_server_invalid_name_cannot_escape() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let f = server_file(&work, "x", "https://k/mcp");
        let err = add_server(lib.path(), "../evil", &f, false, true).unwrap_err();
        assert!(err.to_string().contains("invalid library server name"));
    }

    #[test]
    fn list_shows_servers() {
        let mut library = Library::default();
        library.upsert_server(LibraryServer {
            name: "kibana".into(),
            checksum: Some("abcdef0123456789".into()),
            version: None,
            provenance: Some("file:/x/kibana.toml".into()),
        });
        let out = render_list(&library);
        assert!(out.contains("Servers"));
        assert!(out.contains("kibana"));
        assert!(out.contains("abcdef012345"), "short checksum shown");
    }

    #[test]
    fn remove_server_deletes_index_and_file() {
        let lib = assert_fs::TempDir::new().unwrap();
        let work = assert_fs::TempDir::new().unwrap();
        let file = server_file(&work, "kibana", "https://k/mcp");
        add_server(lib.path(), "kibana", &file, false, true).unwrap();

        let out = remove_server(lib.path(), "kibana", true).unwrap();
        assert!(out.written);
        assert!(
            !lib.child("servers/kibana.toml").path().exists(),
            "file deleted"
        );
        assert!(Library::load(lib.path())
            .unwrap()
            .get_server("kibana")
            .is_none());
    }

    #[test]
    fn remove_server_missing_errors() {
        let lib = assert_fs::TempDir::new().unwrap();
        let err = remove_server(lib.path(), "nope", true).unwrap_err();
        assert!(err.to_string().contains("not a server"));
    }

    #[test]
    fn remove_server_unsafe_name_deletes_nothing_outside() {
        let lib = assert_fs::TempDir::new().unwrap();
        // A directory outside the library a malicious index name might target.
        let outside = assert_fs::TempDir::new().unwrap();
        outside.child("keep.txt").write_str("important\n").unwrap();
        // Hand-craft an index entry with an unsafe name.
        let mut library = Library::default();
        library.upsert_server(LibraryServer {
            name: "../../../../etc".into(),
            checksum: None,
            version: None,
            provenance: None,
        });
        library.save(lib.path()).unwrap();

        let out = remove_server(lib.path(), "../../../../etc", true).unwrap();
        assert_eq!(out.removed_file, None, "unsafe name → no file targeted");
        assert!(outside.child("keep.txt").path().exists());
        assert!(Library::load(lib.path())
            .unwrap()
            .get_server("../../../../etc")
            .is_none());
    }

    #[test]
    fn legacy_link_detection() {
        let old = Path::new("/home/u/.agentstack/skills");
        let dir = Path::new("/home/u/.claude/skills");
        // Absolute target into the legacy home.
        assert!(link_targets_legacy(
            Path::new("/home/u/.agentstack/skills/figma"),
            dir,
            old
        ));
        // Relative target that resolves into the legacy home.
        assert!(link_targets_legacy(
            Path::new("../../.agentstack/skills/figma"),
            dir,
            old
        ));
        // Target owned by a different manager — leave alone.
        assert!(!link_targets_legacy(
            Path::new("../../.agents/skills/find-skills"),
            dir,
            old
        ));
        // Already pointing at the library — leave alone.
        assert!(!link_targets_legacy(
            Path::new("/home/u/.agentstack/lib/skills/figma"),
            dir,
            old
        ));
    }
}
