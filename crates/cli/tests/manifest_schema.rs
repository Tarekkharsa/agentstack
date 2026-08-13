// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The manifest JSON Schema: its drift gate, its emitters, and proof that it
//! accepts the manifests this repository actually ships.
//!
//! `docs/agentstack.schema.json` is generated from the Rust manifest model by
//! `agentstack self docs --write` and committed, so an editor with a TOML
//! language server can complete keys and show hover docs. Three things have to
//! hold for that to be worth anything:
//!
//! 1. **No drift** — the committed file is what the current model generates.
//!    Same pattern, same command, and the same failure message as the
//!    generated command inventory in `docs_commands.rs`.
//! 2. **It accepts real manifests** — a schema that rejects a manifest the
//!    parser accepts is not a nuisance, it is a shipped bug: the editor
//!    underlines working configuration in red. Every example manifest in
//!    `examples/` is validated here, against a small draft-7 validator that
//!    refuses to run on a keyword it does not implement (so it cannot pass by
//!    quietly ignoring half the schema), plus a self-test proving it rejects
//!    what it should.
//! 3. **Manifests point at it** — every emitter writes the `#:schema` line
//!    first, and the `toml_edit` editors that rewrite manifests in place leave
//!    it alone.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use agentstack_core::manifest::schema::{
    manifest_schema, manifest_schema_json, SCHEMA_DIRECTIVE, SCHEMA_DOC_PATH,
};
use serde_json::Value;

/// The repository root. `CARGO_MANIFEST_DIR` is `crates/cli`.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// ── 1. Drift gate ──────────────────────────────────────────────────────────

#[test]
fn committed_schema_matches_the_model() {
    let path = repo_root().join(SCHEMA_DOC_PATH);
    let on_disk = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    assert_eq!(
        on_disk,
        manifest_schema_json(),
        "{SCHEMA_DOC_PATH} is stale — the manifest model changed but the \
         generated schema did not ↳ run `agentstack self docs --write` (or \
         `cargo run -p agentstack -- self docs --write`)"
    );
}

#[test]
fn the_committed_schema_is_the_one_manifests_point_at() {
    // The `$id` is how a resolver recognises a fetched copy. If it and the
    // `#:schema` line ever disagreed, every emitted manifest would name a
    // document that identifies itself as something else.
    let schema = manifest_schema();
    let id = schema.get("$id").and_then(Value::as_str).unwrap();
    assert_eq!(SCHEMA_DIRECTIVE, format!("#:schema {id}"));
}

// ── 2. The schema accepts real manifests ───────────────────────────────────

/// `.toml` files under `examples/` that are deliberately NOT manifests, and so
/// are not expected to parse as one. An explicit list, not a heuristic: a
/// manifest that stops parsing must fail this test rather than quietly drop out
/// of the validated set.
const NOT_MANIFESTS: &[&str] = &[
    // A central-library server DEFINITION (`type`/`url`/`[headers]` at the top
    // level) — the schema for one `[servers.<name>]` entry, not for a manifest.
    "examples/sandbox/fixtures/central-library/kibana.toml",
];

fn toml_files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    // Sorted so a failure names the same file on every machine.
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            toml_files_under(&path, out);
        } else if path.extension().is_some_and(|e| e == "toml") {
            out.push(path);
        }
    }
}

#[test]
fn every_example_manifest_validates_against_the_schema() {
    let root = repo_root();
    let mut files = Vec::new();
    toml_files_under(&root.join("examples"), &mut files);
    assert!(
        files.len() > 10,
        "expected the examples tree to have manifests"
    );

    let schema = manifest_schema();
    let mut validated = 0usize;
    let mut skipped: BTreeSet<String> = BTreeSet::new();

    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        // "Is this a manifest?" is answered by the real model, not by the file
        // name — the same parse the CLI performs.
        let Ok(_) = toml::from_str::<agentstack::manifest::Manifest>(&text) else {
            let rel = path.strip_prefix(&root).unwrap_or(path);
            // Paths are compared with `/` so the list reads the same on
            // Windows; `examples/` has no exotic names.
            skipped.insert(rel.to_string_lossy().replace('\\', "/"));
            continue;
        };
        let as_json = toml_to_json(&text);
        if let Err(why) = validate(&schema, &as_json, &schema, "") {
            panic!(
                "{} is a manifest the parser accepts, but the generated schema \
                 rejects it — an editor would underline working configuration:\n  {why}",
                path.display()
            );
        }
        validated += 1;
    }

    assert!(
        validated >= 15,
        "only {validated} example manifests validated"
    );
    let expected: BTreeSet<String> = NOT_MANIFESTS.iter().map(|s| (*s).to_string()).collect();
    assert_eq!(
        skipped, expected,
        "the set of examples/*.toml that are not manifests changed — update \
         NOT_MANIFESTS (and check that a real manifest did not just stop parsing)"
    );
}

/// The validator's own self-test: it must reject what the schema forbids.
///
/// Without this, "every example validates" could equally well mean "the
/// validator says yes to everything", which is the failure mode a hand-rolled
/// checker actually has.
#[test]
fn the_validator_catches_what_the_schema_forbids() {
    let schema = manifest_schema();

    // A typo in a `deny_unknown_fields` table.
    let bad = toml_to_json("version = 1\n[policy.filesystem]\nwriteable = [\"src\"]\n");
    let err = validate(&schema, &bad, &schema, "").expect_err("unknown key must be refused");
    assert!(err.contains("writeable"), "got: {err}");

    // A wrong scalar type.
    let bad = toml_to_json("version = \"one\"\n");
    validate(&schema, &bad, &schema, "").expect_err("`version` is an integer");

    // A missing required field.
    let bad = toml_to_json("[servers.x]\ntype = \"stdio\"\n");
    validate(&schema, &bad, &schema, "").expect_err("`version` is required");

    // A value outside an enum.
    let bad = toml_to_json("version = 1\n[servers.x]\ntype = \"grpc\"\n");
    validate(&schema, &bad, &schema, "").expect_err("`type` is http or stdio");

    // A wrong array element type.
    let bad = toml_to_json("version = 1\n[servers.x]\ntype = \"stdio\"\nargs = [1, 2]\n");
    validate(&schema, &bad, &schema, "").expect_err("`args` holds strings");

    // And the control: a real manifest still passes.
    let good = toml_to_json(
        "version = 1\n[servers.x]\ntype = \"stdio\"\ncommand = \"npx\"\nargs = [\"-y\", \"p\"]\n",
    );
    validate(&schema, &good, &schema, "").unwrap();
}

/// Parse TOML and re-encode it as JSON, which is the shape a JSON Schema
/// describes. `toml::Value` maps onto `serde_json::Value` one-for-one for
/// everything the manifest model uses (no datetimes anywhere in it).
fn toml_to_json(text: &str) -> Value {
    let parsed: toml::Value = toml::from_str(text).expect("test input parses as TOML");
    serde_json::to_value(parsed).expect("TOML value re-encodes as JSON")
}

/// Every keyword this validator implements. Anything else in the schema is a
/// hard error rather than a silent skip: an unimplemented keyword means the
/// validator is checking less than the schema claims, and a green test would
/// then be telling us nothing.
const KNOWN_KEYWORDS: &[&str] = &[
    // Assertions this validator applies.
    "$ref",
    "type",
    "properties",
    "additionalProperties",
    "required",
    "items",
    "allOf",
    "enum",
    "minimum",
    // Annotations, carrying no assertion.
    "$id",
    "$schema",
    "definitions",
    "description",
    "title",
    "default",
    "format",
];

/// Minimal JSON Schema draft-7 validator, scoped to the keywords the generated
/// schema uses (see `KNOWN_KEYWORDS`).
///
/// Hand-rolled on purpose: pulling a validator crate in would be a new
/// dependency for one test, and the alternative — trusting the schema
/// unchecked — is what let a schema reject a valid manifest in the first place.
/// `root` carries the document `$ref`s resolve against; `at` is the JSON
/// pointer used in failure messages.
fn validate(schema: &Value, value: &Value, root: &Value, at: &str) -> Result<(), String> {
    let obj = match schema {
        Value::Bool(true) => return Ok(()),
        Value::Bool(false) => return Err(format!("{at}: no value is allowed here")),
        Value::Object(o) => o,
        other => return Err(format!("{at}: schema is not an object: {other}")),
    };

    for key in obj.keys() {
        assert!(
            KNOWN_KEYWORDS.contains(&key.as_str()),
            "the generated schema uses `{key}`, which this validator does not \
             implement — teach it the keyword rather than trusting it blind"
        );
    }

    if let Some(Value::String(reference)) = obj.get("$ref") {
        let name = reference
            .strip_prefix("#/definitions/")
            .ok_or_else(|| format!("{at}: unsupported $ref {reference}"))?;
        let target = root
            .pointer(&format!("/definitions/{name}"))
            .ok_or_else(|| format!("{at}: dangling $ref {reference}"))?;
        return validate(target, value, root, at);
    }

    if let Some(Value::Array(all)) = obj.get("allOf") {
        for sub in all {
            validate(sub, value, root, at)?;
        }
    }

    if let Some(types) = obj.get("type") {
        let allowed: Vec<&str> = match types {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            other => return Err(format!("{at}: unsupported `type`: {other}")),
        };
        if !allowed.iter().any(|t| matches_type(t, value)) {
            return Err(format!(
                "{at}: expected {}, found {}",
                allowed.join(" or "),
                type_name(value)
            ));
        }
    }

    if let Some(Value::Array(choices)) = obj.get("enum") {
        if !choices.contains(value) {
            return Err(format!("{at}: {value} is not one of {choices:?}"));
        }
    }

    if let Some(min) = obj.get("minimum").and_then(Value::as_f64) {
        if let Some(n) = value.as_f64() {
            if n < min {
                return Err(format!("{at}: {n} is below the minimum {min}"));
            }
        }
    }

    if let Some(Value::Array(required)) = obj.get("required") {
        if let Value::Object(map) = value {
            for key in required.iter().filter_map(Value::as_str) {
                if !map.contains_key(key) {
                    return Err(format!("{at}: missing required key `{key}`"));
                }
            }
        }
    }

    if let Value::Object(map) = value {
        let properties = obj.get("properties").and_then(Value::as_object);
        for (key, child) in map {
            let at = format!("{at}/{key}");
            if let Some(sub) = properties.and_then(|p| p.get(key)) {
                validate(sub, child, root, &at)?;
                continue;
            }
            match obj.get("additionalProperties") {
                // `false` is what `#[serde(deny_unknown_fields)]` becomes.
                Some(Value::Bool(false)) => {
                    return Err(format!("{at}: unknown key `{key}` is not allowed here"))
                }
                Some(sub) => validate(sub, child, root, &at)?,
                None => {}
            }
        }
    }

    if let (Value::Array(items), Some(sub)) = (value, obj.get("items")) {
        for (i, child) in items.iter().enumerate() {
            validate(sub, child, root, &format!("{at}/{i}"))?;
        }
    }

    Ok(())
}

// ── 3. Manifests point at the schema ───────────────────────────────────────
//
// These drive the real `init` paths, so they set HOME / AGENTSTACK_HOME, which
// are process-global; serialize them against each other.

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn init_args(global: bool) -> agentstack::cli::InitArgs {
    agentstack::cli::InitArgs {
        global,
        force: false,
        dry_run: false,
        plan: false,
        secrets: None,
        no_keychain: true,
        project_servers: false,
        include_tool_managed: false,
        yes: true,
        consented: None,
        connect: false,
        verbose: false,
    }
}

/// An isolated, empty HOME, so nothing on the real machine is mistaken for a
/// detected CLI. Returns the project dir to run `init` in.
fn isolated_project(tmp: &Path) -> PathBuf {
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
    let proj = tmp.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    proj
}

fn first_line(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
    text.lines().next().unwrap_or_default().to_string()
}

/// The machine layer's template.
#[test]
fn init_global_writes_the_schema_directive_first() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join(".agentstack");
    std::env::set_var("AGENTSTACK_HOME", &home);

    agentstack::commands::init::run(&init_args(true), None).unwrap();

    let path = home.join("agentstack.toml");
    assert_eq!(first_line(&path), SCHEMA_DIRECTIVE);
    // Still a manifest: `#:schema` is a comment to every TOML parser.
    toml::from_str::<agentstack::manifest::Manifest>(&std::fs::read_to_string(&path).unwrap())
        .unwrap();

    std::env::remove_var("AGENTSTACK_HOME");
}

/// The starter manifest — the "nothing to import" path.
#[test]
fn init_starter_manifest_writes_the_schema_directive_first() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = isolated_project(tmp.path());

    agentstack::commands::init::run(&init_args(false), Some(&proj)).unwrap();

    let path = agentstack::manifest::resolve_manifest_dir(&proj).join("agentstack.toml");
    assert_eq!(first_line(&path), SCHEMA_DIRECTIVE);

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

/// The import path — the manifest `init` serializes from the model after
/// reading a CLI's native config.
#[test]
fn init_import_writes_the_schema_directive_first() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = isolated_project(tmp.path());
    std::fs::write(
        proj.join(".mcp.json"),
        r#"{"mcpServers":{"filesystem":{"command":"npx","args":["-y","@modelcontextprotocol/server-filesystem","./"]}}}"#,
    )
    .unwrap();

    agentstack::commands::init::run(&init_args(false), Some(&proj)).unwrap();

    let path = agentstack::manifest::resolve_manifest_dir(&proj).join("agentstack.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(first_line(&path), SCHEMA_DIRECTIVE);
    let manifest: agentstack::manifest::Manifest = toml::from_str(&text).unwrap();
    assert!(
        !manifest.profiles.is_empty() || !manifest.servers.is_empty(),
        "this fixture should have imported something: {text}"
    );

    std::env::remove_var("HOME");
    std::env::remove_var("AGENTSTACK_HOME");
}

/// The in-place editors must leave the line alone.
///
/// Every manifest mutation in the CLI goes through `toml_edit`, which preserves
/// comments — but "preserves comments" is a property of the library, not a
/// promise anybody wrote down here, and a leading directive is exactly the
/// comment a rewrite is most likely to lose. This pins it.
#[test]
fn in_place_edits_preserve_the_schema_directive() {
    let original = format!("{SCHEMA_DIRECTIVE}\nversion = 1\n\n# a hand-written note\n");

    // Adding a capability (`add server`, `add skill`, `add from`, the MCP
    // tools, and the house-rules seeder all land here).
    let with_server = agentstack::commands::add::build_manifest_with(
        &original,
        "servers",
        "fs",
        &serde_json::json!({"type": "stdio", "command": "npx", "args": ["-y", "pkg"]}),
        None,
    )
    .unwrap();
    assert_eq!(with_server.lines().next().unwrap(), SCHEMA_DIRECTIVE);
    assert!(with_server.contains("# a hand-written note"));

    // Adding it to a toolset, on top of that edit.
    let with_toolset = agentstack::commands::add::build_manifest_with(
        &with_server,
        "servers",
        "fs2",
        &serde_json::json!({"type": "stdio", "command": "npx"}),
        Some("dev"),
    )
    .unwrap();
    assert_eq!(with_toolset.lines().next().unwrap(), SCHEMA_DIRECTIVE);

    // And a scalar edit through the other `toml_edit` writer.
    let opted_out = agentstack::commands::add::set_meta_gitignore(&with_toolset, false).unwrap();
    assert_eq!(opted_out.lines().next().unwrap(), SCHEMA_DIRECTIVE);
    assert!(opted_out.contains("gitignore = false"));

    // The result is still a manifest, and still one the schema accepts.
    let schema = manifest_schema();
    toml::from_str::<agentstack::manifest::Manifest>(&opted_out).unwrap();
    validate(&schema, &toml_to_json(&opted_out), &schema, "").unwrap();
}

fn matches_type(name: &str, value: &Value) -> bool {
    match name {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        _ => false,
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
