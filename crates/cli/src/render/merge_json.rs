//! Non-destructive JSON merge.
//!
//! JSON has no comment/format-preserving editor in the standard ecosystem, and
//! a naive parse→`to_string_pretty` round-trip silently reformats untouched
//! content (notably floats, whose Rust and JS string forms differ). That would
//! turn a two-line change into a thousand-line diff and break the
//! "preserve exactly" promise.
//!
//! So we splice surgically: locate the managed section's value span in the
//! original text, reserialize only that section, and leave every other byte of
//! the file untouched.

use anyhow::{Context, Result};
use serde_json::{Map, Value};

/// Upsert `entries` (name → rendered body) under top-level key `location`,
/// preserving the rest of the file verbatim.
pub fn merge(existing: &str, location: &str, entries: &[(String, Value)]) -> Result<String> {
    merge_with_removals(existing, location, entries, &[])
}

/// Like [`merge`], but also removes `removals` (names we used to manage but no
/// longer do) from the section.
pub fn merge_with_removals(
    existing: &str,
    location: &str,
    entries: &[(String, Value)],
    removals: &[String],
) -> Result<String> {
    // Build the merged section: existing managed-or-not entries first, then our
    // upserts (so unmanaged servers like Claude's `tldraw` survive).
    let root: Value = if existing.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(existing).context("existing config is not valid JSON")?
    };
    let root_obj = root
        .as_object()
        .context("top level of JSON config is not an object")?;

    let mut section: Map<String, Value> = root_obj
        .get(location)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for name in removals {
        section.shift_remove(name);
    }
    for (name, body) in entries {
        section.insert(name.clone(), body.clone());
    }
    let section_val = Value::Object(section);

    // Empty root, or absent section in an empty root → safe to emit fresh.
    if root_obj.is_empty() {
        let mut out = serde_json::to_string_pretty(&Value::Object(
            [(location.to_string(), section_val)].into_iter().collect(),
        ))?;
        out.push('\n');
        return Ok(out);
    }

    // Section already present → splice in place.
    if let Some((start, end)) = find_top_level_value_span(existing, location) {
        let base_indent = line_indent(existing, start);
        let pretty = serde_json::to_string_pretty(&section_val)?;
        let reindented = reindent(&pretty, &base_indent);
        let mut out = String::with_capacity(existing.len() + reindented.len());
        out.push_str(&existing[..start]);
        out.push_str(&reindented);
        out.push_str(&existing[end..]);
        return Ok(out);
    }

    // Section absent in a non-empty object → insert after the opening brace,
    // preserving the rest of the file.
    let brace = existing
        .find('{')
        .context("config has no top-level object")?;
    let pretty = serde_json::to_string_pretty(&section_val)?;
    let reindented = reindent(&pretty, "  ");
    let insertion = format!("\n  {}: {},", serde_json::to_string(location)?, reindented);
    let mut out = String::with_capacity(existing.len() + insertion.len());
    out.push_str(&existing[..=brace]);
    out.push_str(&insertion);
    out.push_str(&existing[brace + 1..]);
    Ok(out)
}

/// Upsert `entries` as *top-level* keys (and remove `removals`), preserving the
/// rest of the file verbatim. Used for settings files where we own a set of
/// root keys (e.g. `permissions`, `env`) rather than entries under one section.
pub fn merge_top_level(
    existing: &str,
    entries: &[(String, Value)],
    removals: &[String],
) -> Result<String> {
    let mut text = if existing.trim().is_empty() {
        "{}\n".to_string()
    } else {
        // Validate up front so we fail loudly rather than splicing garbage.
        serde_json::from_str::<Value>(existing).context("existing settings is not valid JSON")?;
        existing.to_string()
    };
    for name in removals {
        text = splice_top_level(&text, name, None)?;
    }
    for (name, body) in entries {
        text = splice_top_level(&text, name, Some(body))?;
    }
    Ok(text)
}

/// Set (or, with `body: None`, remove) a single top-level key, leaving every
/// other byte untouched.
fn splice_top_level(text: &str, key: &str, body: Option<&Value>) -> Result<String> {
    match (find_top_level_value_span(text, key), body) {
        // Replace an existing key's value in place.
        (Some((start, _)), Some(body)) => {
            let end = find_top_level_value_span(text, key).unwrap().1;
            let base_indent = line_indent(text, start);
            let pretty = serde_json::to_string_pretty(body)?;
            let reindented = reindent(&pretty, &base_indent);
            let mut out = String::with_capacity(text.len() + reindented.len());
            out.push_str(&text[..start]);
            out.push_str(&reindented);
            out.push_str(&text[end..]);
            Ok(out)
        }
        // Remove an existing key (and its surrounding separator) cleanly by
        // reserializing the root object — only used on prune, where a tidy
        // result matters more than byte-preservation of the dropped key.
        (Some(_), None) => {
            let mut root: Value =
                serde_json::from_str(text).context("settings is not valid JSON")?;
            if let Some(obj) = root.as_object_mut() {
                obj.shift_remove(key);
            }
            let mut out = serde_json::to_string_pretty(&root)?;
            out.push('\n');
            Ok(out)
        }
        // Nothing to remove.
        (None, None) => Ok(text.to_string()),
        // Insert a new key after the opening brace.
        (None, Some(body)) => {
            let brace = text.find('{').context("settings has no top-level object")?;
            let pretty = serde_json::to_string_pretty(body)?;
            let reindented = reindent(&pretty, "  ");
            // If the object is empty (`{}`), don't emit a trailing comma.
            let after = text[brace + 1..].trim_start();
            let sep = if after.starts_with('}') { "" } else { "," };
            let insertion = format!("\n  {}: {}{}", serde_json::to_string(key)?, reindented, sep);
            let mut out = String::with_capacity(text.len() + insertion.len());
            out.push_str(&text[..=brace]);
            out.push_str(&insertion);
            out.push_str(&text[brace + 1..]);
            Ok(out)
        }
    }
}

/// Prefix every line after the first with `indent` (the serializer emits an
/// object whose continuation lines start at column 0).
fn reindent(pretty: &str, indent: &str) -> String {
    let mut out = String::with_capacity(pretty.len());
    for (i, line) in pretty.lines().enumerate() {
        if i > 0 {
            out.push('\n');
            out.push_str(indent);
        }
        out.push_str(line);
    }
    out
}

/// Leading whitespace of the line containing byte offset `pos`.
fn line_indent(text: &str, pos: usize) -> String {
    let line_start = text[..pos].rfind('\n').map(|n| n + 1).unwrap_or(0);
    text[line_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// Find the byte span `[start, end)` of the value of a top-level object key.
/// Returns `None` if the key is absent (or the text is not a top-level object).
/// Strings and escapes are respected, so an occurrence of the key name inside a
/// nested string is never mistaken for the key.
fn find_top_level_value_span(text: &str, key: &str) -> Option<(usize, usize)> {
    let b = text.as_bytes();
    let mut i = skip_ws(b, 0);
    if i >= b.len() || b[i] != b'{' {
        return None;
    }
    i += 1;
    loop {
        i = skip_ws(b, i);
        if i >= b.len() {
            return None;
        }
        match b[i] {
            b'}' => return None,
            b',' => {
                i += 1;
                continue;
            }
            b'"' => {}
            _ => return None,
        }
        let key_start = i;
        let key_end = skip_string(b, i); // index just past closing quote
        let this_key = &text[key_start + 1..key_end - 1];
        i = skip_ws(b, key_end);
        if i >= b.len() || b[i] != b':' {
            return None;
        }
        i = skip_ws(b, i + 1);
        let val_start = i;
        let val_end = skip_value(b, i);
        if this_key == key {
            return Some((val_start, val_end));
        }
        i = val_end;
    }
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

/// `i` at an opening quote; returns the index just past the closing quote.
fn skip_string(b: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    i
}

/// `i` at the first byte of a value; returns the index just past the value.
fn skip_value(b: &[u8], i: usize) -> usize {
    match b.get(i) {
        Some(b'"') => skip_string(b, i),
        Some(b'{') => skip_container(b, i, b'{', b'}'),
        Some(b'[') => skip_container(b, i, b'[', b']'),
        _ => {
            // number / true / false / null — run until a structural byte.
            let mut j = i;
            while j < b.len() && !matches!(b[j], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
            {
                j += 1;
            }
            j
        }
    }
}

fn skip_container(b: &[u8], i: usize, open: u8, close: u8) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    while j < b.len() {
        match b[j] {
            b'"' => {
                j = skip_string(b, j);
                continue;
            }
            x if x == open => depth += 1,
            x if x == close => {
                depth -= 1;
                if depth == 0 {
                    return j + 1;
                }
            }
            _ => {}
        }
        j += 1;
    }
    j
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn upserts_without_disturbing_other_keys() {
        let existing = "{\n  \"numStartups\": 5,\n  \"mcpServers\": {\n    \"tldraw\": {\n      \"command\": \"node\"\n    }\n  }\n}\n";
        let entries = vec![(
            "kibana".to_string(),
            json!({ "type": "http", "url": "https://x" }),
        )];
        let out = merge(existing, "mcpServers", &entries).unwrap();
        assert!(out.contains("\"numStartups\": 5"));
        assert!(out.contains("\"tldraw\""));
        assert!(out.contains("\"kibana\""));
        // Still valid JSON.
        serde_json::from_str::<Value>(&out).unwrap();
    }

    #[test]
    fn floats_elsewhere_are_preserved_byte_for_byte() {
        // A float that Rust would reformat if it round-tripped the whole doc.
        let existing =
            "{\n  \"stats\": {\n    \"avg\": 0.9402052562189797\n  },\n  \"mcpServers\": {}\n}\n";
        let out = merge(
            existing,
            "mcpServers",
            &[("x".into(), json!({ "url": "u" }))],
        )
        .unwrap();
        assert!(
            out.contains("0.9402052562189797"),
            "float must be preserved exactly, got:\n{out}"
        );
    }

    #[test]
    fn key_named_like_location_in_nested_string_is_ignored() {
        let existing = "{\n  \"note\": \"talk about mcpServers here\",\n  \"mcpServers\": {}\n}\n";
        let out = merge(existing, "mcpServers", &[("x".into(), json!({"url":"u"}))]).unwrap();
        assert!(out.contains("talk about mcpServers here"));
        assert!(out.contains("\"x\""));
        serde_json::from_str::<Value>(&out).unwrap();
    }

    #[test]
    fn inserts_section_when_absent() {
        let existing = "{\n  \"numStartups\": 5\n}\n";
        let out = merge(existing, "mcpServers", &[("x".into(), json!({"url":"u"}))]).unwrap();
        assert!(out.contains("\"mcpServers\""));
        assert!(out.contains("\"numStartups\": 5"));
        serde_json::from_str::<Value>(&out).unwrap();
    }

    #[test]
    fn creates_section_in_empty_object() {
        let out = merge("{}", "mcpServers", &[("x".into(), json!({"url":"u"}))]).unwrap();
        assert!(out.contains("\"mcpServers\""));
        serde_json::from_str::<Value>(&out).unwrap();
    }

    #[test]
    fn empty_input_is_treated_as_object() {
        let out = merge("", "mcpServers", &[("x".into(), json!({"url":"u"}))]).unwrap();
        assert!(out.contains("\"mcpServers\""));
        serde_json::from_str::<Value>(&out).unwrap();
    }

    #[test]
    fn top_level_upserts_and_preserves_unmanaged_keys() {
        let existing =
            "{\n  \"theme\": \"dark\",\n  \"permissions\": {\n    \"deny\": [\"x\"]\n  }\n}\n";
        let entries = vec![(
            "permissions".to_string(),
            json!({ "allow": ["Bash(git:*)"] }),
        )];
        let out = merge_top_level(existing, &entries, &[]).unwrap();
        // Hand-set sibling key survives.
        assert!(out.contains("\"theme\": \"dark\""));
        // Managed key replaced wholesale (top-level ownership).
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["permissions"]["allow"][0], "Bash(git:*)");
        assert!(v["permissions"].get("deny").is_none());
    }

    #[test]
    fn top_level_inserts_into_empty_and_into_existing() {
        let out = merge_top_level("", &[("model".into(), json!("opus"))], &[]).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&out).unwrap()["model"],
            "opus"
        );
        let out2 = merge_top_level(
            "{\n  \"a\": 1\n}\n",
            &[("model".into(), json!("opus"))],
            &[],
        )
        .unwrap();
        let v: Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["model"], "opus");
    }

    #[test]
    fn top_level_removal_prunes_key() {
        let existing = "{\n  \"keep\": 1,\n  \"model\": \"opus\"\n}\n";
        let out = merge_top_level(existing, &[], &["model".into()]).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["keep"], 1);
        assert!(v.get("model").is_none());
    }

    #[test]
    fn top_level_preserves_float_in_unmanaged_key() {
        let existing = "{\n  \"avg\": 0.9402052562189797,\n  \"model\": \"x\"\n}\n";
        let out = merge_top_level(existing, &[("model".into(), json!("opus"))], &[]).unwrap();
        assert!(out.contains("0.9402052562189797"));
    }
}
