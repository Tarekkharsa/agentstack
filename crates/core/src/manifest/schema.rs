//! The editor-facing JSON Schema for `agentstack.toml`.
//!
//! An editor with a TOML language server (taplo / Even Better TOML, Zed,
//! IntelliJ) offers key completion and hover documentation for a file that
//! names its schema. The schema here is DERIVED from
//! [`crate::manifest::model`] rather than written beside it, so the thing the
//! editor promises and the thing the parser accepts cannot drift: they are one
//! set of Rust types, read twice.
//!
//! Two halves, deliberately split:
//!
//! * [`SCHEMA_URL`] and [`SCHEMA_DIRECTIVE`] are plain strings and always
//!   compiled. Every manifest emitter writes the directive as its first line,
//!   and that must not depend on a feature flag.
//! * [`manifest_schema_json`] needs `schemars` and is therefore behind the
//!   `schema` feature. Only the `cli` crate turns it on — see the feature's
//!   comment in `crates/core/Cargo.toml` for why `trust` and `policy` must not
//!   inherit it.

/// Where the generated schema is published. GitHub Pages serves `docs/` at the
/// site root, so the committed `docs/agentstack.schema.json` lands here.
pub const SCHEMA_URL: &str = "https://tarekkharsa.github.io/agentstack/agentstack.schema.json";

/// The first line of every manifest AgentStack writes.
///
/// `#:schema` is a plain TOML comment to any parser, and a schema association
/// to taplo and the editors built on it. Writing it means a new project gets
/// completion and hover docs with no editor configuration at all; an existing
/// project adds this one line by hand.
pub const SCHEMA_DIRECTIVE: &str =
    "#:schema https://tarekkharsa.github.io/agentstack/agentstack.schema.json";

/// The path, relative to the repository root, where the generated schema is
/// committed. Shared by the emitter and its drift gate so neither can name a
/// file the other does not.
pub const SCHEMA_DOC_PATH: &str = "docs/agentstack.schema.json";

/// The generated schema, pretty-printed with a trailing newline — the exact
/// bytes of `docs/agentstack.schema.json`.
///
/// Draft 7, not the 2020-12 that `schemars` defaults to. This file exists to be
/// read by editors, and draft 7 (`definitions`, `$ref`) is the dialect all of
/// them understand; IntelliJ in particular does not follow `$defs`. Nothing in
/// the manifest model needs a keyword newer than draft 7, so the older dialect
/// costs no expressiveness and buys the wider audience.
///
/// Deterministic: `schemars` builds the document in a fixed order and
/// `serde_json`'s `preserve_order` keeps it, so two runs of one binary produce
/// identical bytes. That is what makes the regenerate-and-diff drift gate in
/// `crates/cli/tests/manifest_schema.rs` meaningful.
#[cfg(feature = "schema")]
pub fn manifest_schema_json() -> String {
    let mut json = serde_json::to_string_pretty(&manifest_schema())
        .expect("a schemars Schema is a serde_json::Value and always serializes");
    json.push('\n');
    json
}

/// The schema as a JSON value, with the root's identity filled in.
///
/// `schemars` titles the root after the Rust type (`Manifest`), which is the
/// implementation's name for it, not the user's. The `$id` is what lets a
/// resolver recognise a cached copy as this schema.
#[cfg(feature = "schema")]
pub fn manifest_schema() -> serde_json::Value {
    use schemars::generate::SchemaSettings;

    let schema = SchemaSettings::draft07()
        .into_generator()
        .into_root_schema_for::<crate::manifest::model::Manifest>();
    let mut value = serde_json::Value::from(schema);

    if let Some(root) = value.as_object_mut() {
        root.insert("$id".into(), SCHEMA_URL.into());
        root.insert("title".into(), "AgentStack manifest".into());
        root.insert(
            "description".into(),
            "The portable manifest (`.agentstack/agentstack.toml`): the one file a \
             project authors. It contains no secret literals — only `${REF}` \
             references resolved per-machine at render time."
                .into(),
        );
    }
    value
}

#[cfg(all(test, feature = "schema"))]
mod tests {
    use super::*;

    #[test]
    fn the_directive_names_the_published_url() {
        // One typo here would ship manifests pointing at nothing, and the
        // 404 would only surface in somebody else's editor.
        assert_eq!(SCHEMA_DIRECTIVE, format!("#:schema {SCHEMA_URL}"));
    }

    #[test]
    fn generation_is_deterministic() {
        assert_eq!(manifest_schema_json(), manifest_schema_json());
    }

    #[test]
    fn the_root_describes_the_manifest_tables() {
        let schema = manifest_schema();
        let props = schema
            .get("properties")
            .and_then(|p| p.as_object())
            .expect("the root schema describes an object with properties");
        // The capability kinds and config tables a user actually writes. A
        // field that stops being schematized stops being completable, which is
        // the whole feature.
        for key in [
            "version",
            "meta",
            "servers",
            "skills",
            "toolsets",
            "instructions",
            "settings",
            "hooks",
            "extensions",
            "workflows",
            "packs",
            "package_overrides",
            "targets",
            "policy",
            "guard",
            "experimental",
            "delivery",
            "default_toolset",
        ] {
            assert!(props.contains_key(key), "root schema is missing `{key}`");
        }
        // `toolsets` is what serde writes; `profiles` is the accepted older
        // spelling. The schema follows serialization, so an editor completing
        // `profiles` would be completing a key we never emit.
        assert!(!props.contains_key("profiles"));
    }

    #[test]
    fn deny_unknown_fields_surfaces_as_additional_properties_false() {
        let schema = manifest_schema();
        let defs = schema
            .get("definitions")
            .and_then(|d| d.as_object())
            .expect("draft 7 puts subschemas under `definitions`");
        // Every model type carrying `#[serde(deny_unknown_fields)]` must reject
        // unknown keys in the editor too — that is the typo-catching half of
        // the feature, and it is the one thing schemars could silently drop.
        for name in [
            "Delivery",
            "HarnessDelivery",
            "ExperimentalConfig",
            "ExperimentalExecuteLimits",
            "GuardConfig",
            "WorkflowPolicy",
            "FsPolicy",
            "PackageOverride",
            "InstructionVariant",
            "Workflow",
            "RoleScheduling",
        ] {
            let def = defs.get(name).expect("model type is in the schema");
            assert_eq!(
                def.get("additionalProperties"),
                Some(&serde_json::Value::Bool(false)),
                "`{name}` denies unknown fields in serde but not in the schema"
            );
        }
    }

    #[test]
    fn doc_comments_become_hover_text() {
        let schema = manifest_schema();
        let servers = schema
            .pointer("/properties/servers/description")
            .and_then(|d| d.as_str())
            .expect("`servers` carries its doc comment as a description");
        assert!(servers.contains("MCP servers"), "got: {servers}");
    }
}
