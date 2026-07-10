//! `agentstack settings set|unset <target> <key> [value]` — edit a target's
//! native `[settings.<target>]` entries (e.g. Claude Code `model`) without
//! hand-editing the manifest. Values are validated against the adapter's known
//! settings catalog and coerced to the declared type; unknown keys are honored
//! as strings. Flag-driven, dry-run by default, and comment-preserving via
//! `toml_edit` — the same machinery `add`/`remove` use.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use toml_edit::{DocumentMut, Item, Table, Value as TomlValue};

use crate::adapter::descriptor::{SettingField, SettingKind};
use crate::cli::{SettingsArgs, SettingsKind, SettingsSetArgs, SettingsUnsetArgs};
use crate::util::diff;

pub fn run(args: &SettingsArgs, manifest_dir: Option<&Path>) -> Result<()> {
    match &args.kind {
        SettingsKind::Set(a) => set(a, manifest_dir),
        SettingsKind::Unset(a) => unset(a, manifest_dir),
    }
}

fn set(a: &SettingsSetArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;

    // The target must be a real adapter id — else the setting would render
    // nowhere. Fail loudly with the known ids.
    let desc = ctx.registry.get(&a.target).ok_or_else(|| {
        let mut ids: Vec<&str> = ctx.registry.ids().collect();
        ids.sort_unstable();
        anyhow::anyhow!(
            "unknown target '{}' — expected an adapter id (one of: {})",
            a.target,
            ids.join(", ")
        )
    })?;

    validate_key(&a.key)?;

    // Coerce/validate against the catalog where the key is known; unknown keys
    // are honored as strings (still hand-editable for exotic types).
    let field = desc
        .settings
        .as_ref()
        .and_then(|s| s.fields.iter().find(|f| f.key == a.key));
    let value = coerce(field, &a.value, &a.key, &a.target)?;

    let original = fs::read_to_string(&ctx.loaded.manifest_path)
        .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;
    let new_text = set_setting(&original, &a.target, &a.key, value)?;

    println!(
        "{} set [settings.{}] {} = {} in {}",
        "→".cyan(),
        a.target,
        a.key.bold(),
        a.value,
        ctx.loaded.manifest_path.display()
    );
    print_diff(&original, &new_text);
    finish(&ctx, &new_text, a.write, &format!("set '{}'", a.key))
}

fn unset(a: &SettingsUnsetArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;

    // Unlike `set`, an unknown target simply has nothing to remove — but the
    // more useful signal is that the setting isn't there, so let the key check
    // below carry the error with the exact `[settings.<target>]` context.
    validate_key(&a.key)?;

    let original = fs::read_to_string(&ctx.loaded.manifest_path)
        .with_context(|| format!("reading {}", ctx.loaded.manifest_path.display()))?;
    let new_text = unset_setting(&original, &a.target, &a.key)?;

    println!(
        "{} unset [settings.{}] {} from {}",
        "−".yellow(),
        a.target,
        a.key.bold(),
        ctx.loaded.manifest_path.display()
    );
    print_diff(&original, &new_text);
    finish(&ctx, &new_text, a.write, &format!("unset '{}'", a.key))
}

/// Shared dry-run/`--write` tail (mirrors `add`/`remove`).
fn finish(ctx: &super::Context, new_text: &str, write: bool, action: &str) -> Result<()> {
    if write {
        crate::util::atomic::write(&ctx.loaded.manifest_path, new_text)
            .with_context(|| format!("writing {}", ctx.loaded.manifest_path.display()))?;
        println!("{} {}.", "✓".green(), action);
    } else {
        println!(
            "\nDry run. Re-run with {} to update the manifest.",
            "--write".bold()
        );
    }
    Ok(())
}

fn print_diff(original: &str, new_text: &str) {
    print!(
        "{}",
        diff::render(original, new_text)
            .lines()
            .map(|l| format!("  {l}\n"))
            .collect::<String>()
    );
}

/// A key is a dotted path; every segment must be non-empty.
fn validate_key(key: &str) -> Result<()> {
    if key.is_empty() || key.split('.').any(str::is_empty) {
        anyhow::bail!(
            "invalid setting key '{key}' — expected a dotted path like `permissions.defaultMode`"
        );
    }
    Ok(())
}

/// Coerce a CLI string into the TOML value the catalog expects. Unknown keys
/// (no `field`) are stored verbatim as strings — matching how the codebase
/// treats uncatalogued settings elsewhere. Type mismatches fail with a message
/// that names the expected shape.
fn coerce(field: Option<&SettingField>, raw: &str, key: &str, target: &str) -> Result<TomlValue> {
    let Some(field) = field else {
        return Ok(raw.into());
    };
    match field.kind {
        SettingKind::String => Ok(raw.into()),
        SettingKind::Bool => match raw {
            "true" => Ok(true.into()),
            "false" => Ok(false.into()),
            _ => anyhow::bail!(
                "setting '{key}' for {target} is a boolean — expected `true` or `false`, got '{raw}'"
            ),
        },
        SettingKind::Number => {
            if let Ok(i) = raw.parse::<i64>() {
                Ok(i.into())
            } else if let Ok(f) = raw.parse::<f64>() {
                Ok(f.into())
            } else {
                anyhow::bail!("setting '{key}' for {target} is a number — got '{raw}'")
            }
        }
        SettingKind::Enum => {
            if field.options.iter().any(|o| o == raw) {
                Ok(raw.into())
            } else {
                anyhow::bail!(
                    "setting '{key}' for {target} must be one of [{}] — got '{raw}'",
                    field.options.join(", ")
                )
            }
        }
    }
}

/// Upsert `key` (a dotted path) = `value` under `[settings.<target>]`, creating
/// the intermediate tables as needed and leaving every other line — comments,
/// formatting, unrelated tables — byte-for-byte intact.
pub(crate) fn set_setting(text: &str, target: &str, key: &str, value: TomlValue) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parsing manifest as TOML")?;

    // `[settings]` and `[settings.<target>]` render header-less until they hold
    // a key, so a fresh block comes out as `[settings.<target>]` only.
    let settings = doc
        .entry("settings")
        .or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        })
        .as_table_mut()
        .context("`settings` is not a table")?;
    settings.set_implicit(true);
    let mut tbl = settings
        .entry(target)
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .with_context(|| format!("settings.{target} is not a table"))?;

    let parts: Vec<&str> = key.split('.').collect();
    for parent in &parts[..parts.len() - 1] {
        tbl = tbl
            .entry(parent)
            .or_insert_with(|| {
                let mut t = Table::new();
                t.set_implicit(true);
                Item::Table(t)
            })
            .as_table_mut()
            .with_context(|| format!("settings.{target}.{parent} is not a table"))?;
    }
    let leaf = parts.last().expect("validate_key guarantees a segment");
    tbl[*leaf] = Item::Value(value);
    Ok(doc.to_string())
}

/// Remove `key` (a dotted path) from `[settings.<target>]`, then prune any
/// now-empty ancestor tables (including the target block and `[settings]`
/// itself). Errors if the key isn't present.
pub(crate) fn unset_setting(text: &str, target: &str, key: &str) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parsing manifest as TOML")?;
    let missing = || anyhow::anyhow!("no setting '{key}' under [settings.{target}] to unset");

    let settings = doc
        .get_mut("settings")
        .and_then(Item::as_table_mut)
        .ok_or_else(missing)?;
    let target_tbl = settings
        .get_mut(target)
        .and_then(Item::as_table_mut)
        .ok_or_else(missing)?;

    // Walk to the leaf's parent, recording the chain so we can prune empties.
    let parts: Vec<&str> = key.split('.').collect();
    let leaf = *parts.last().expect("validate_key guarantees a segment");
    let mut tbl = &mut *target_tbl;
    for parent in &parts[..parts.len() - 1] {
        tbl = tbl
            .get_mut(parent)
            .and_then(Item::as_table_mut)
            .ok_or_else(missing)?;
    }
    if tbl.remove(leaf).is_none() {
        return Err(missing());
    }

    // Prune empties bottom-up: nested parents, the target block, then the
    // `[settings]` table if it's left with nothing.
    prune_empty_path(target_tbl, &parts[..parts.len() - 1]);
    if target_tbl.is_empty() {
        settings.remove(target);
    }
    if settings.is_empty() {
        doc.remove("settings");
    }
    Ok(doc.to_string())
}

/// Drop each ancestor table along `path` that has been left empty, deepest
/// first, so unsetting the last leaf of a nested block doesn't leave a bare
/// `[settings.<target>.permissions]` header behind.
fn prune_empty_path(root: &mut Table, path: &[&str]) {
    if path.is_empty() {
        return;
    }
    // Recurse first so we prune from the deepest empty table outward.
    if let Some(child) = root.get_mut(path[0]).and_then(Item::as_table_mut) {
        prune_empty_path(child, &path[1..]);
        if child.is_empty() {
            root.remove(path[0]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::descriptor::{SettingField, SettingKind};

    fn field(key: &str, kind: SettingKind, options: &[&str]) -> SettingField {
        SettingField {
            key: key.into(),
            label: None,
            kind,
            options: options.iter().map(|s| s.to_string()).collect(),
            help: None,
            group: None,
            default: None,
        }
    }

    #[test]
    fn coerce_string_keeps_value() {
        let f = field("model", SettingKind::String, &[]);
        assert_eq!(
            coerce(Some(&f), "gpt-5.5", "model", "codex")
                .unwrap()
                .as_str(),
            Some("gpt-5.5")
        );
    }

    #[test]
    fn coerce_bool_produces_toml_boolean() {
        let f = field("autoCompactEnabled", SettingKind::Bool, &[]);
        assert_eq!(
            coerce(Some(&f), "true", "autoCompactEnabled", "claude-code")
                .unwrap()
                .as_bool(),
            Some(true)
        );
        assert!(coerce(Some(&f), "yes", "autoCompactEnabled", "claude-code").is_err());
    }

    #[test]
    fn coerce_number_produces_integer() {
        let f = field("cleanupPeriodDays", SettingKind::Number, &[]);
        assert_eq!(
            coerce(Some(&f), "30", "cleanupPeriodDays", "claude-code")
                .unwrap()
                .as_integer(),
            Some(30)
        );
        assert!(coerce(Some(&f), "soon", "cleanupPeriodDays", "claude-code").is_err());
    }

    #[test]
    fn coerce_enum_validates_options() {
        let f = field("editorMode", SettingKind::Enum, &["normal", "vim"]);
        assert_eq!(
            coerce(Some(&f), "vim", "editorMode", "claude-code")
                .unwrap()
                .as_str(),
            Some("vim")
        );
        let err = coerce(Some(&f), "emacs", "editorMode", "claude-code").unwrap_err();
        assert!(err.to_string().contains("normal, vim"));
    }

    #[test]
    fn coerce_unknown_key_is_string() {
        assert_eq!(
            coerce(None, "true", "someFlag", "claude-code")
                .unwrap()
                .as_str(),
            Some("true")
        );
    }

    #[test]
    fn set_creates_settings_block_and_preserves_comments() {
        let text = "# a comment\nversion = 1\n\n[servers.a]\ntype = \"http\"\nurl = \"u\"\n";
        let out = set_setting(text, "claude-code", "model", "gpt-5.5".into()).unwrap();
        assert!(out.contains("# a comment"));
        assert!(out.contains("[servers.a]"));
        assert!(out.contains("[settings.claude-code]"));
        assert!(out.contains("model = \"gpt-5.5\""));
        // No bare [settings] header.
        assert!(!out.contains("\n[settings]\n"));
        let _: toml::Value = toml::from_str(&out).unwrap();
    }

    #[test]
    fn set_dotted_key_nests_and_updates_in_place() {
        let out = set_setting(
            "version = 1\n",
            "claude-code",
            "permissions.defaultMode",
            "ask".into(),
        )
        .unwrap();
        let doc: DocumentMut = out.parse().unwrap();
        assert_eq!(
            doc["settings"]["claude-code"]["permissions"]["defaultMode"].as_str(),
            Some("ask")
        );
        // Overwrite the same key — no duplicate.
        let out2 = set_setting(
            &out,
            "claude-code",
            "permissions.defaultMode",
            "auto".into(),
        )
        .unwrap();
        assert_eq!(out2.matches("defaultMode").count(), 1);
        assert!(out2.contains("\"auto\""));
    }

    #[test]
    fn set_bool_renders_unquoted() {
        let out = set_setting(
            "version = 1\n",
            "claude-code",
            "autoCompactEnabled",
            true.into(),
        )
        .unwrap();
        assert!(out.contains("autoCompactEnabled = true"));
    }

    #[test]
    fn unset_removes_key_and_prunes_empty_block() {
        let text = "version = 1\n\n[settings.claude-code]\nmodel = \"gpt-5.5\"\n";
        let out = unset_setting(text, "claude-code", "model").unwrap();
        assert!(!out.contains("model"));
        // The now-empty target block and [settings] table are pruned.
        assert!(!out.contains("[settings.claude-code]"));
        assert!(out.contains("version = 1"));
    }

    #[test]
    fn unset_keeps_sibling_keys() {
        let text = "[settings.claude-code]\nmodel = \"gpt-5.5\"\nautoCompactEnabled = true\n";
        let out = unset_setting(text, "claude-code", "model").unwrap();
        assert!(!out.contains("model"));
        assert!(out.contains("autoCompactEnabled = true"));
        assert!(out.contains("[settings.claude-code]"));
    }

    #[test]
    fn unset_nested_prunes_empty_parent() {
        let text = "[settings.claude-code.permissions]\ndefaultMode = \"ask\"\n";
        let out = unset_setting(text, "claude-code", "permissions.defaultMode").unwrap();
        assert!(!out.contains("permissions"));
        assert!(!out.contains("[settings"));
    }

    #[test]
    fn unset_missing_key_errors() {
        let err = unset_setting("version = 1\n", "claude-code", "model").unwrap_err();
        assert!(err.to_string().contains("no setting 'model'"));
        let text = "[settings.claude-code]\nmodel = \"x\"\n";
        let err = unset_setting(text, "claude-code", "language").unwrap_err();
        assert!(err.to_string().contains("language"));
    }
}
