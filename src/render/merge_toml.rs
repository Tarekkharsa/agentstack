//! Non-destructive TOML merge.
//!
//! Uses `toml_edit` so comments and formatting of untouched sections survive.
//! Only the server entries we manage are upserted under `location`; everything
//! else in the file (Codex's `[projects.*]`, `[features]`, …) is left byte-for-
//! byte intact.

use anyhow::{Context, Result};
use serde_json::Value;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table};

/// Upsert `entries` (name → rendered body) under the `location` table.
/// `nested_as_subtable` controls whether nested objects (headers/env) render as
/// standalone subtables (`true`) or inline tables (`false`).
pub fn merge(
    existing: &str,
    location: &str,
    entries: &[(String, Value)],
    nested_as_subtable: bool,
) -> Result<String> {
    merge_with_removals(existing, location, entries, &[], nested_as_subtable)
}

/// Like [`merge`], but also removes `removals` (names we used to manage but no
/// longer do) from the section table.
pub fn merge_with_removals(
    existing: &str,
    location: &str,
    entries: &[(String, Value)],
    removals: &[String],
    nested_as_subtable: bool,
) -> Result<String> {
    let mut doc: DocumentMut = if existing.trim().is_empty() {
        DocumentMut::new()
    } else {
        existing
            .parse()
            .context("existing config is not valid TOML")?
    };

    // Ensure `location` exists as an implicit table (so no bare `[mcp_servers]`
    // header is emitted — only `[mcp_servers.<name>]`).
    if doc.get(location).is_none() {
        let mut t = Table::new();
        t.set_implicit(true);
        doc.insert(location, Item::Table(t));
    }
    let section = doc
        .get_mut(location)
        .unwrap()
        .as_table_mut()
        .with_context(|| format!("`{location}` in config is not a table"))?;
    section.set_implicit(true);

    for name in removals {
        section.remove(name);
    }
    for (name, body) in entries {
        let table = value_to_table(body, nested_as_subtable)
            .with_context(|| format!("rendering server '{name}' to TOML"))?;
        section.insert(name, Item::Table(table));
    }

    Ok(doc.to_string())
}

/// Upsert `entries` as *top-level* keys (and remove `removals`), preserving
/// comments and untouched tables. Used for settings files (Codex `config.toml`)
/// where we own a set of root keys rather than entries under one section.
pub fn merge_top_level(
    existing: &str,
    entries: &[(String, Value)],
    removals: &[String],
) -> Result<String> {
    let mut doc: DocumentMut = if existing.trim().is_empty() {
        DocumentMut::new()
    } else {
        existing
            .parse()
            .context("existing settings is not valid TOML")?
    };
    for name in removals {
        doc.remove(name);
    }
    for (name, body) in entries {
        // Objects become standalone tables; scalars/arrays become values.
        let item = if body.is_object() {
            Item::Table(value_to_table(body, true)?)
        } else {
            value_to_item(body, true)?
        };
        doc.insert(name, item);
    }
    Ok(doc.to_string())
}

/// Convert a JSON object (one server body) into a `toml_edit` Table.
fn value_to_table(value: &Value, nested_as_subtable: bool) -> Result<Table> {
    let obj = value.as_object().context("server body is not an object")?;
    let mut table = Table::new();
    for (k, v) in obj {
        table.insert(k, value_to_item(v, nested_as_subtable)?);
    }
    Ok(table)
}

fn value_to_item(value: &Value, nested_as_subtable: bool) -> Result<Item> {
    Ok(match value {
        Value::String(s) => Item::Value(s.as_str().into()),
        Value::Bool(b) => Item::Value((*b).into()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Item::Value(i.into())
            } else if let Some(f) = n.as_f64() {
                Item::Value(f.into())
            } else {
                Item::Value(n.to_string().into())
            }
        }
        Value::Array(arr) => {
            let mut a = Array::new();
            for el in arr {
                a.push(value_to_toml_value(el)?);
            }
            Item::Value(toml_edit::Value::Array(a))
        }
        Value::Object(_) => {
            if nested_as_subtable {
                Item::Table(value_to_table(value, nested_as_subtable)?)
            } else {
                Item::Value(toml_edit::Value::InlineTable(value_to_inline(value)?))
            }
        }
        Value::Null => anyhow::bail!("null is not representable in TOML"),
    })
}

fn value_to_toml_value(value: &Value) -> Result<toml_edit::Value> {
    Ok(match value {
        Value::String(s) => toml_edit::Value::from(s.as_str()),
        Value::Bool(b) => toml_edit::Value::from(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml_edit::Value::from(i)
            } else if let Some(f) = n.as_f64() {
                toml_edit::Value::from(f)
            } else {
                toml_edit::Value::from(n.to_string())
            }
        }
        Value::Object(_) => toml_edit::Value::InlineTable(value_to_inline(value)?),
        Value::Array(arr) => {
            let mut a = Array::new();
            for el in arr {
                a.push(value_to_toml_value(el)?);
            }
            toml_edit::Value::Array(a)
        }
        Value::Null => anyhow::bail!("null is not representable in TOML"),
    })
}

fn value_to_inline(value: &Value) -> Result<InlineTable> {
    let obj = value.as_object().context("expected object")?;
    let mut it = InlineTable::new();
    for (k, v) in obj {
        it.insert(k, value_to_toml_value(v)?);
    }
    Ok(it)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_comments_and_other_tables() {
        let existing = r#"# top comment
model = "gpt-5.5"

[projects."/x"]
trust_level = "trusted"

[mcp_servers.figma]
url = "https://mcp.figma.com/mcp"
"#;
        let entries = vec![(
            "kibana_mcp".to_string(),
            json!({
                "url": "https://k/mcp",
                "http_headers": { "Authorization": "Bearer v" }
            }),
        )];
        let out = merge(existing, "mcp_servers", &entries, true).unwrap();
        assert!(out.contains("# top comment"));
        assert!(out.contains("[projects.\"/x\"]"));
        assert!(out.contains("[mcp_servers.figma]"));
        assert!(out.contains("[mcp_servers.kibana_mcp]"));
        assert!(out.contains("[mcp_servers.kibana_mcp.http_headers]"));
        // No bare [mcp_servers] header.
        assert!(!out.contains("\n[mcp_servers]\n"));
    }

    #[test]
    fn updates_existing_entry_in_place() {
        let existing = "[mcp_servers.kibana_mcp]\nurl = \"https://old\"\n";
        let entries = vec![("kibana_mcp".to_string(), json!({ "url": "https://new" }))];
        let out = merge(existing, "mcp_servers", &entries, true).unwrap();
        assert!(out.contains("https://new"));
        assert!(!out.contains("https://old"));
    }

    #[test]
    fn top_level_merge_keeps_mcp_servers_and_comments() {
        let existing = r#"# my codex config
model = "gpt-5.5"

[mcp_servers.figma]
url = "https://mcp.figma.com/mcp"
"#;
        // Own `approval_policy` (scalar) without disturbing the MCP table.
        let entries = vec![("approval_policy".to_string(), json!("on-request"))];
        let out = merge_top_level(existing, &entries, &[]).unwrap();
        assert!(out.contains("# my codex config"));
        assert!(out.contains("[mcp_servers.figma]"));
        assert!(out.contains("approval_policy = \"on-request\""));
        // Re-parses as valid TOML.
        let _: toml::Value = toml::from_str(&out).unwrap();
    }

    #[test]
    fn top_level_merge_object_becomes_table_and_removal_prunes() {
        let existing = "model = \"x\"\n";
        let entries = vec![("sandbox".to_string(), json!({ "network": false }))];
        let out = merge_top_level(existing, &entries, &[]).unwrap();
        assert!(out.contains("[sandbox]"));
        assert!(out.contains("network = false"));
        // Prune it back out; model stays.
        let pruned = merge_top_level(&out, &[], &["sandbox".into()]).unwrap();
        assert!(!pruned.contains("[sandbox]"));
        assert!(pruned.contains("model = \"x\""));
    }

    #[test]
    fn top_level_merge_supports_nested_inline_arrays_for_hooks() {
        let body = json!({
            "PostToolUse": [
                {
                    "matcher": "Edit|Write",
                    "hooks": [
                        { "type": "command", "command": "prettier --write", "timeout": 5 }
                    ]
                }
            ]
        });
        let out = merge_top_level("model = \"x\"\n", &[("hooks".into(), body)], &[]).unwrap();
        assert!(out.contains("[hooks]"));
        assert!(out.contains("PostToolUse = [{ matcher = \"Edit|Write\""));
        let _: toml::Value = toml::from_str(&out).unwrap();
    }
}
