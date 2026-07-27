//! Structured `agent()` results (scaling plan Phase 2a): the output contract
//! appended to a child's prompt, the tolerant extraction of JSON from that
//! child's stdout, and a bounded structural validator.
//!
//! # Why this exists
//!
//! Without it every reduce stage parses model prose. That is survivable at
//! width 5 and fatal at width 500: the probability that *every* mapper emits
//! parseable output collapses, and there are no keys to partition on — so it
//! also blocks the partitioner (Phase 3).
//!
//! # What it does NOT do — the honest boundary (rule 8)
//!
//! - **A schema-validated result is not a trusted result.** Validation
//!   constrains SHAPE, never content. `agent()` output is model output and
//!   remains untrusted data flowing into later prompts; the §7 data-flow
//!   caveat and the taint marks are unchanged by it. A prompt-injected child
//!   can return perfectly schema-valid lies.
//! - **This is not a conformant JSON Schema implementation.** It is a small,
//!   reviewable subset chosen to avoid taking a new dependency (`type`,
//!   `properties`, `required`, `items`, `enum`, `additionalProperties:
//!   false`). Anything else in a schema document is IGNORED, so a workflow
//!   author must not read an unsupported keyword as enforcement. Unsupported
//!   keywords are listed by `agentstack workflow list`-adjacent tooling only
//!   if they ever become load-bearing; today the rule is simply "ignored".
//! - **There is no automatic re-ask.** A CLI-side retry would spawn a child
//!   the engine never counted against `max_agents`, which is a ceiling
//!   bypass. A validation failure fails the step closed and the script
//!   decides. Retry accounting belongs to Phase 4, alongside purity.
//!
//! # Determinism
//!
//! Extraction and validation are pure functions of (stdout bytes, opts), so
//! the Stage F replay path can — and must — apply exactly the same transform
//! to the verified stdout artifact. Applying it in only one place would make
//! a resumed run feed a *string* where the original fed an *object*.

use serde_json::Value;

/// Hard bound on the schema document itself before it is rendered into a
/// prompt (rule 7: it arrives from the pinned script, but bounding every
/// ingestion is the house discipline). A schema past this is refused rather
/// than truncated — a truncated schema would silently weaken the contract.
const MAX_SCHEMA_BYTES: usize = 32 * 1024;

/// How deep the validator will walk. Mirrors the engine's `MAX_JSON_DEPTH`
/// discipline: the recursion below is bounded by this constant, so a
/// pathologically nested schema or instance is refused at the bound rather
/// than overflowing the native stack.
const MAX_VALIDATE_DEPTH: usize = 64;

/// Why a child's stdout could not become the `agent()` result. These are
/// launcher-authored categories (redaction gate 3): they are OURS, never
/// upstream text and never script text, so they are safe to record verbatim
/// in `StepFailed.reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultError {
    /// No JSON value could be located in the child's output at all.
    NoJson,
    /// JSON was located but did not match the declared schema.
    SchemaMismatch,
    /// The schema itself is unusable (oversized, or not an object).
    BadSchema,
}

impl ResultError {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            ResultError::NoJson => "schema_no_json",
            ResultError::SchemaMismatch => "schema_mismatch",
            ResultError::BadSchema => "schema_invalid",
        }
    }
}

/// The declared schema for one `agent()` call, if any. Read from the opts
/// object the script passed, which rides the `SpawnRequest` unchanged.
pub(crate) fn declared_schema(opts: &Value) -> Option<&Value> {
    opts.get("schema").filter(|s| !s.is_null())
}

/// The output contract appended to a child's prompt when a schema is
/// declared. Delivered as prompt DATA through the descriptor's argv or stdin
/// like any other prompt text — never shell-interpolated (rule 7).
///
/// Deliberately short and imperative: a long contract crowds out the actual
/// task, and every model in the shipped set already knows JSON Schema.
pub(crate) fn contract_suffix(schema: &Value) -> Result<String, ResultError> {
    let rendered = serde_json::to_string(schema).map_err(|_| ResultError::BadSchema)?;
    if rendered.len() > MAX_SCHEMA_BYTES || !schema.is_object() {
        return Err(ResultError::BadSchema);
    }
    Ok(format!(
        "\n\n---\nRespond with a single JSON value and nothing else — no prose before or \
         after, no explanation. It must validate against this JSON Schema:\n{rendered}"
    ))
}

/// Turn one child's raw stdout into the value that resolves its `agent()`
/// promise.
///
/// With no declared schema this is the shipped behaviour, unchanged: the
/// bounded stdout is the result, verbatim, as a JSON string (F5 — no trim, no
/// re-encode). With a schema it is the extracted and validated JSON value.
pub(crate) fn child_result_value(raw: &str, opts: &Value) -> Result<Value, ResultError> {
    let Some(schema) = declared_schema(opts) else {
        return Ok(Value::String(raw.to_string()));
    };
    if !schema.is_object() {
        return Err(ResultError::BadSchema);
    }
    let found = extract_json(raw).ok_or(ResultError::NoJson)?;
    if validate(&found, schema, 0) {
        Ok(found)
    } else {
        Err(ResultError::SchemaMismatch)
    }
}

/// Locate a JSON value in model output. Tolerant on purpose: models wrap JSON
/// in code fences and prose no matter how the prompt is worded, and failing a
/// governed child over a pair of backticks would be a self-inflicted
/// reliability problem.
///
/// Order is cheapest-first: whole string, fenced block, then a balanced scan
/// from the first opening bracket. Hostile input is safe here — the scan is a
/// single forward pass bounded by the input length (itself already bounded by
/// the stdout capture cap), and `serde_json` enforces its own depth limit.
fn extract_json(raw: &str) -> Option<Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Some(value);
    }
    if let Some(inner) = fenced_block(trimmed) {
        if let Ok(value) = serde_json::from_str::<Value>(inner.trim()) {
            return Some(value);
        }
    }
    balanced_span(trimmed).and_then(|span| serde_json::from_str::<Value>(span).ok())
}

/// The body of the first ``` fenced block, with an optional language tag
/// (```json) skipped. `None` when there is no closing fence — an unterminated
/// fence is not a block.
fn fenced_block(text: &str) -> Option<&str> {
    let open = text.find("```")?;
    let after = &text[open + 3..];
    // Skip a language tag on the same line as the opening fence.
    let body_start = after.find('\n').map(|i| i + 1).unwrap_or(0);
    let body = &after[body_start..];
    let close = body.find("```")?;
    Some(&body[..close])
}

/// The first balanced `{...}` or `[...]` span, string- and escape-aware so a
/// bracket inside a string literal cannot end the span early.
fn balanced_span(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = bytes.iter().position(|b| *b == b'{' || *b == b'[')?;
    let (open, close) = if bytes[start] == b'{' {
        (b'{', b'}')
    } else {
        (b'[', b']')
    };

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b if b == open => depth += 1,
            b if b == close => {
                depth -= 1;
                if depth == 0 {
                    // `i` indexes a single-byte ASCII delimiter, so `i + 1`
                    // is always a char boundary.
                    return Some(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// The bounded structural validator. Returns a plain bool: the drive loop
/// reports one honest category (`schema_mismatch`) rather than a path-level
/// diagnostic, because the step fails closed either way and a per-field
/// message would be model-influenced text on the evidence path.
fn validate(value: &Value, schema: &Value, depth: usize) -> bool {
    if depth > MAX_VALIDATE_DEPTH {
        return false;
    }
    let Some(map) = schema.as_object() else {
        // A non-object subschema constrains nothing. Permissive by design:
        // this validator's contract is "the subset it understands is
        // enforced; everything else is ignored", not "unknown means invalid".
        return true;
    };

    if let Some(Value::Array(allowed)) = map.get("enum") {
        if !allowed.iter().any(|a| a == value) {
            return false;
        }
    }

    if let Some(Value::String(ty)) = map.get("type") {
        if !type_matches(ty, value) {
            return false;
        }
    }

    match value {
        Value::Object(fields) => {
            if let Some(Value::Array(required)) = map.get("required") {
                for name in required {
                    let Some(name) = name.as_str() else { continue };
                    if !fields.contains_key(name) {
                        return false;
                    }
                }
            }
            if let Some(Value::Object(props)) = map.get("properties") {
                if map.get("additionalProperties") == Some(&Value::Bool(false))
                    && fields.keys().any(|k| !props.contains_key(k))
                {
                    return false;
                }
                for (name, sub) in props {
                    if let Some(field) = fields.get(name) {
                        if !validate(field, sub, depth + 1) {
                            return false;
                        }
                    }
                }
            }
            true
        }
        Value::Array(items) => match map.get("items") {
            Some(sub) => items.iter().all(|i| validate(i, sub, depth + 1)),
            None => true,
        },
        _ => true,
    }
}

fn type_matches(ty: &str, value: &Value) -> bool {
    match ty {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        // JSON Schema's `integer` accepts a float with a zero fraction
        // (`2.0`), which is what a model emitting a round number produces.
        "integer" => value.as_i64().is_some() || value.as_f64().is_some_and(|f| f.fract() == 0.0),
        // An unrecognized type name constrains nothing — same permissive rule
        // as an unrecognized keyword above.
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn opts_with(schema: serde_json::Value) -> serde_json::Value {
        json!({ "role": "r", "schema": schema })
    }

    #[test]
    fn no_schema_returns_stdout_verbatim() {
        // F5 unchanged: without a schema the bounded stdout IS the result,
        // with no trim and no re-encode — including surrounding whitespace.
        let out = child_result_value("  raw text\n", &json!({ "role": "r" })).unwrap();
        assert_eq!(out, json!("  raw text\n"));
    }

    #[test]
    fn bare_json_validates_against_the_schema() {
        let schema = json!({
            "type": "object",
            "required": ["file", "severity"],
            "properties": {
                "file": { "type": "string" },
                "severity": { "enum": ["low", "high"] },
            },
        });
        let out = child_result_value(
            r#"{"file":"src/a.rs","severity":"high"}"#,
            &opts_with(schema),
        )
        .unwrap();
        assert_eq!(out["file"], "src/a.rs");
    }

    #[test]
    fn fenced_and_prose_wrapped_json_is_still_extracted() {
        // Models wrap output in fences and commentary regardless of the
        // prompt; failing a governed child over backticks would be a
        // self-inflicted reliability problem.
        let schema = json!({ "type": "object", "required": ["ok"] });
        for raw in [
            "```json\n{\"ok\": true}\n```",
            "```\n{\"ok\": true}\n```",
            "Here is the result:\n{\"ok\": true}\nHope that helps!",
        ] {
            let out = child_result_value(raw, &opts_with(schema.clone()))
                .unwrap_or_else(|e| panic!("{raw:?} failed to extract: {e:?}"));
            assert_eq!(out, json!({ "ok": true }), "{raw:?}");
        }
    }

    #[test]
    fn a_brace_inside_a_string_does_not_end_the_span() {
        let schema = json!({ "type": "object", "required": ["msg"] });
        let raw = r#"prose {"msg": "a } and a \" quote", "n": 1} tail"#;
        let out = child_result_value(raw, &opts_with(schema)).unwrap();
        assert_eq!(out["msg"], r#"a } and a " quote"#);
        assert_eq!(out["n"], 1);
    }

    #[test]
    fn missing_required_field_is_a_mismatch_not_a_pass() {
        let schema = json!({ "type": "object", "required": ["file"] });
        let err = child_result_value(r#"{"other": 1}"#, &opts_with(schema)).unwrap_err();
        assert_eq!(err, ResultError::SchemaMismatch);
    }

    #[test]
    fn wrong_nested_type_is_a_mismatch() {
        let schema = json!({
            "type": "object",
            "properties": {
                "items": { "type": "array", "items": { "type": "string" } },
            },
        });
        let err = child_result_value(r#"{"items":["a",2]}"#, &opts_with(schema)).unwrap_err();
        assert_eq!(err, ResultError::SchemaMismatch);
    }

    #[test]
    fn additional_properties_false_rejects_unknown_keys() {
        let schema = json!({
            "type": "object",
            "properties": { "a": { "type": "string" } },
            "additionalProperties": false,
        });
        assert!(child_result_value(r#"{"a":"x","b":1}"#, &opts_with(schema.clone())).is_err());
        assert!(child_result_value(r#"{"a":"x"}"#, &opts_with(schema)).is_ok());
    }

    #[test]
    fn prose_with_no_json_at_all_fails_closed() {
        let schema = json!({ "type": "object" });
        let err = child_result_value("I could not do that.", &opts_with(schema)).unwrap_err();
        assert_eq!(err, ResultError::NoJson);
    }

    #[test]
    fn integer_accepts_a_round_float() {
        // A model emitting `2.0` for an integer field is not an error worth
        // failing a governed child over; JSON Schema agrees.
        let schema = json!({ "type": "object", "properties": { "n": { "type": "integer" } } });
        assert!(child_result_value(r#"{"n":2.0}"#, &opts_with(schema.clone())).is_ok());
        assert!(child_result_value(r#"{"n":2.5}"#, &opts_with(schema)).is_err());
    }

    #[test]
    fn an_oversized_or_non_object_schema_is_refused_not_ignored() {
        // Failing closed matters more than convenience here: silently
        // ignoring an unusable schema would hand the script unvalidated model
        // output while the author believes it was checked.
        let huge = json!({ "type": "object", "description": "x".repeat(MAX_SCHEMA_BYTES) });
        assert_eq!(contract_suffix(&huge).unwrap_err(), ResultError::BadSchema);
        assert_eq!(
            contract_suffix(&json!("not-a-schema")).unwrap_err(),
            ResultError::BadSchema
        );
        assert_eq!(
            child_result_value("{}", &opts_with(json!("not-a-schema"))).unwrap_err(),
            ResultError::BadSchema
        );
    }

    #[test]
    fn adversarial_nesting_is_refused_at_the_depth_bound() {
        // Both the instance and the schema recurse together, so the bound has
        // to hold for a hostile pair, not just a hostile instance.
        let mut schema = json!({ "type": "string" });
        let mut instance = json!("x");
        for _ in 0..(MAX_VALIDATE_DEPTH + 20) {
            schema = json!({ "type": "array", "items": schema });
            instance = json!([instance]);
        }
        let raw = serde_json::to_string(&instance).unwrap();
        assert_eq!(
            child_result_value(&raw, &opts_with(schema)).unwrap_err(),
            ResultError::SchemaMismatch
        );
    }

    #[test]
    fn the_contract_names_the_schema_and_forbids_prose() {
        let suffix = contract_suffix(&json!({ "type": "object" })).unwrap();
        assert!(suffix.contains("JSON Schema"));
        assert!(suffix.contains(r#"{"type":"object"}"#));
    }
}
