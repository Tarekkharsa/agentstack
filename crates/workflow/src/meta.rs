//! Parse-only extraction of the static `meta` block.
//!
//! The security claim this module makes: the `meta` block is validated
//! **without executing any script code**. We hand the (wrapped) source to
//! Boa's *parser* — never a `Context` — walk the resulting AST, and enforce a
//! pure-literal rule on `meta`'s property values. No `eval`, no interpreter, no
//! side effects: a hostile `meta` can at worst be rejected, never run.

use std::borrow::Cow;

use boa_engine::ast::declaration::{Binding, LexicalDeclaration};
use boa_engine::ast::expression::literal::{LiteralKind, PropertyDefinition};
use boa_engine::ast::expression::Expression;
use boa_engine::ast::property::PropertyName;
use boa_engine::ast::scope::Scope;
use boa_engine::ast::{Declaration, StatementListItem};
use boa_engine::interner::Interner;
use boa_engine::parser::{Parser, Source};

use crate::error::WorkflowError;

/// Validated, static workflow metadata. Everything here was proven to be a
/// compile-time literal by [`extract_meta`]; nothing was evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    /// The roles the script is allowed to spawn. The bridge enforces that
    /// every `agent()` call names one of these (script-internal consistency).
    pub roles: Vec<String>,
    /// Request view of the agent cap; the machine-limit intersection is Stage D.
    pub max_agents: Option<u32>,
    /// Request view of the wall-clock cap; enforcement is Stage D.
    pub max_wall_seconds: Option<u64>,
}

/// The wrapper that turns the raw script into a parseable async function body,
/// so top-level `await` / `return` (the Claude-Code workflow-script shape, A1)
/// are legal. Meta extraction only needs the body statements; the evaluator
/// (`lib.rs`) uses a matching async-IIFE wrapper to produce the root promise.
/// Both keep the script text byte-identical inside an async function body.
fn wrap_for_parse(script: &str) -> String {
    format!("async function __agentstack_workflow__() {{\n{script}\n}}")
}

/// AL4: a §3 workflow script begins with an `export const meta = {…}` (the
/// Claude-Code shape). `export` is a module-only statement and is illegal
/// inside the async-function body both wrappers produce, so the leading
/// `export ` token of that one declaration must be removed before wrapping —
/// and *only* that token. This returns the script with exactly the leading
/// `export` keyword bytes elided (the surrounding whitespace is untouched), or
/// the script unchanged for the bare `const meta` form.
///
/// Why this is safe from string/comment confusion: only whitespace and comments
/// can legally precede a script's very first token. The scan skips exactly those
/// and then matches `export` at a token boundary, so it can never be fooled by
/// an `export` substring inside a later string, template, or comment — it stops
/// at the first real token and touches nothing after the keyword.
pub(crate) fn deexport(script: &str) -> Cow<'_, str> {
    match export_keyword_offset(script) {
        Some(start) => {
            let mut out = String::with_capacity(script.len() - EXPORT_KW.len());
            out.push_str(&script[..start]);
            out.push_str(&script[start + EXPORT_KW.len()..]);
            Cow::Owned(out)
        }
        None => Cow::Borrowed(script),
    }
}

const EXPORT_KW: &str = "export";

/// Byte offset of the leading `export` keyword iff it is the script's first
/// meaningful token, else `None`. Skips only leading whitespace and `//` / `/*
/// */` comments — the sole trivia that may precede a first token.
fn export_keyword_offset(script: &str) -> Option<usize> {
    let b = script.as_bytes();
    let n = b.len();
    let mut i = 0usize;
    loop {
        // Skip ASCII whitespace and a leading UTF-8 BOM.
        while i < n {
            match b[i] {
                b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C => i += 1,
                0xEF if b[i..].starts_with(&[0xEF, 0xBB, 0xBF]) => i += 3,
                _ => break,
            }
        }
        if i >= n {
            return None;
        }
        // Skip comments; anything else is the first real token.
        if b[i] == b'/' && i + 1 < n {
            if b[i + 1] == b'/' {
                i += 2;
                while i < n && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if b[i + 1] == b'*' {
                i += 2;
                while i + 1 < n && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                if i + 1 >= n {
                    // Unterminated block comment — let the parser report it.
                    return None;
                }
                i += 2;
                continue;
            }
        }
        break;
    }
    // The first real token starts at `i`. It is the `export` keyword only if the
    // bytes match AND the next byte is not an identifier char (so `exported`,
    // `exports`, … are not mistaken for the keyword).
    if b[i..].starts_with(EXPORT_KW.as_bytes()) {
        let after = i + EXPORT_KW.len();
        if after >= n || !is_ident_continue_byte(b[after]) {
            return Some(i);
        }
    }
    None
}

/// Identifier-continuation test for the boundary check. ASCII identifier bytes
/// plus any non-ASCII byte (conservatively treated as a possible Unicode
/// identifier char, so we never strip when `export` is only a prefix).
fn is_ident_continue_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// Parse the wrapped script (parser only, no `Context`) and extract `meta`.
///
/// Returns `Err(MetaViolation)` if `meta` is absent, is not an object literal,
/// or carries any non-literal value; `Err(InvalidScript)` if the source does
/// not parse at all.
pub fn extract_meta(script: &str) -> Result<Meta, WorkflowError> {
    // AL4: strip a leading `export ` (the §3 export-meta form) before wrapping;
    // `export` is illegal inside the async-function body. Still parse-only.
    let wrapped = wrap_for_parse(&deexport(script));
    let mut interner = Interner::new();
    let scope = Scope::new_global();
    let mut parser = Parser::new(Source::from_bytes(wrapped.as_str()));
    // Parser only — this never constructs a Boa `Context`, so no untrusted
    // code can run during meta extraction. That is the whole guarantee.
    let ast = parser
        .parse_script(&scope, &mut interner)
        .map_err(|e| WorkflowError::invalid_script(e.to_string()))?;

    // Top-level statement is the async function declaration we wrapped with.
    let body = ast
        .statements()
        .statements()
        .iter()
        .find_map(|item| match item {
            StatementListItem::Declaration(decl) => match decl.as_ref() {
                Declaration::AsyncFunctionDeclaration(afd) => Some(afd.body()),
                _ => None,
            },
            _ => None,
        })
        .ok_or_else(|| WorkflowError::internal("workflow wrapper did not parse as expected"))?;

    // Find `const meta = <object literal>` (or `let`) inside the body.
    let meta_object = body
        .statements()
        .iter()
        .find_map(|item| meta_initializer(item, &interner))
        .ok_or_else(|| WorkflowError::meta_violation("no top-level `const meta = {…}` found"))?;

    let object = match meta_object {
        Expression::ObjectLiteral(obj) => obj,
        _ => {
            return Err(WorkflowError::meta_violation(
                "meta must be initialized to an object literal",
            ))
        }
    };

    let mut roles: Option<Vec<String>> = None;
    let mut max_agents: Option<u32> = None;
    let mut max_wall_seconds: Option<u64> = None;

    for prop in object.properties() {
        let (name, value) = match prop {
            PropertyDefinition::Property(PropertyName::Literal(id), value) => {
                (interner.resolve_expect(id.sym()).to_string(), value)
            }
            // Shorthand (`{meta}`), methods, spreads, or computed names all
            // imply evaluation or reference — reject them outright.
            _ => {
                return Err(WorkflowError::meta_violation(
                    "meta may only contain plain literal properties",
                ))
            }
        };

        match name.as_str() {
            "roles" => {
                let items = literal_array(value)?;
                let mut collected = Vec::with_capacity(items.len());
                for item in items {
                    match pure_literal(item, &interner)? {
                        LiteralValue::String(s) => collected.push(s),
                        _ => {
                            return Err(WorkflowError::meta_violation(
                                "meta.roles must be string literals",
                            ))
                        }
                    }
                }
                roles = Some(collected);
            }
            "maxAgents" => {
                max_agents = Some(literal_u32(value, &interner, "meta.maxAgents")?);
            }
            "maxWallSeconds" => {
                max_wall_seconds = Some(literal_u64(value, &interner, "meta.maxWallSeconds")?);
            }
            // Unknown keys are allowed, but still must be pure literals (or
            // arrays of them) so the "checked without executing" guarantee
            // covers the whole object.
            _ => {
                ensure_pure(value, &interner)?;
            }
        }
    }

    let roles = roles.ok_or_else(|| WorkflowError::meta_violation("meta.roles is required"))?;

    Ok(Meta {
        roles,
        max_agents,
        max_wall_seconds,
    })
}

/// If `item` is `const/let meta = <expr>`, return the initializer expression.
fn meta_initializer<'a>(
    item: &'a StatementListItem,
    interner: &Interner,
) -> Option<&'a Expression> {
    let StatementListItem::Declaration(decl) = item else {
        return None;
    };
    let Declaration::Lexical(lex) = decl.as_ref() else {
        return None;
    };
    let (LexicalDeclaration::Const(list) | LexicalDeclaration::Let(list)) = lex;
    for var in list.as_ref() {
        let Binding::Identifier(id) = var.binding() else {
            continue;
        };
        if interner.resolve_expect(id.sym()).to_string() == "meta" {
            return var.init();
        }
    }
    None
}

/// A recognized pure literal value. The boolean case carries no payload: a
/// boolean is a valid meta literal, but no meta field reads its value.
enum LiteralValue {
    String(String),
    Number(f64),
    Bool,
}

/// Unwrap redundant parentheses, e.g. `("reviewer")`.
fn unparen(expr: &Expression) -> &Expression {
    match expr {
        Expression::Parenthesized(p) => unparen(p.expression()),
        other => other,
    }
}

/// A scalar literal (string / number / boolean). Anything else — identifiers,
/// calls, member access, templates with substitutions, `null`, `BigInt` — is a
/// meta-rule violation.
fn pure_literal(expr: &Expression, interner: &Interner) -> Result<LiteralValue, WorkflowError> {
    match unparen(expr) {
        Expression::Literal(lit) => match lit.kind() {
            LiteralKind::String(sym) => Ok(LiteralValue::String(
                interner.resolve_expect(*sym).to_string(),
            )),
            LiteralKind::Num(n) => Ok(LiteralValue::Number(*n)),
            LiteralKind::Int(i) => Ok(LiteralValue::Number(f64::from(*i))),
            LiteralKind::Bool(_) => Ok(LiteralValue::Bool),
            LiteralKind::Null | LiteralKind::Undefined | LiteralKind::BigInt(_) => Err(
                WorkflowError::meta_violation("meta values must be string, number, or boolean"),
            ),
        },
        _ => Err(WorkflowError::meta_violation(
            "meta values must be plain literals, not expressions",
        )),
    }
}

/// Require `expr` to be a pure literal or an array of pure literals.
fn ensure_pure(expr: &Expression, interner: &Interner) -> Result<(), WorkflowError> {
    match unparen(expr) {
        Expression::ArrayLiteral(_) => {
            for item in literal_array(expr)? {
                pure_literal(item, interner)?;
            }
            Ok(())
        }
        _ => pure_literal(expr, interner).map(|_| ()),
    }
}

/// Return the elements of an array literal, refusing elisions (holes) and
/// spreads (`None` slots come from `[ , ]` or `[...x]`).
fn literal_array(expr: &Expression) -> Result<Vec<&Expression>, WorkflowError> {
    match unparen(expr) {
        Expression::ArrayLiteral(arr) => {
            let mut out = Vec::new();
            for slot in arr.as_ref() {
                match slot {
                    Some(e) => out.push(e),
                    None => {
                        return Err(WorkflowError::meta_violation(
                            "meta arrays may not contain holes",
                        ))
                    }
                }
            }
            Ok(out)
        }
        _ => Err(WorkflowError::meta_violation("expected an array literal")),
    }
}

fn literal_u32(expr: &Expression, interner: &Interner, field: &str) -> Result<u32, WorkflowError> {
    match pure_literal(expr, interner)? {
        LiteralValue::Number(n) if n.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&n) => {
            Ok(n as u32)
        }
        _ => Err(WorkflowError::meta_violation(format!(
            "{field} must be a non-negative integer"
        ))),
    }
}

fn literal_u64(expr: &Expression, interner: &Interner, field: &str) -> Result<u64, WorkflowError> {
    // f64 can represent integers exactly up to 2^53; that is a generous ceiling
    // for a wall-clock second count and keeps the "literal only" story simple.
    match pure_literal(expr, interner)? {
        LiteralValue::Number(n)
            if n.fract() == 0.0 && (0.0..=9_007_199_254_740_992.0).contains(&n) =>
        {
            Ok(n as u64)
        }
        _ => Err(WorkflowError::meta_violation(format!(
            "{field} must be a non-negative integer"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::WorkflowErrorKind;

    #[test]
    fn meta_rule_rejects_impure_literals() {
        // Pure literals extract cleanly.
        let ok = extract_meta("const meta = { roles: ['reviewer'], maxAgents: 2 };\nreturn 1;")
            .expect("pure literal meta should extract");
        assert_eq!(ok.roles, vec!["reviewer".to_string()]);
        assert_eq!(ok.max_agents, Some(2));
        assert_eq!(ok.max_wall_seconds, None);

        // A call expression as a value is not a literal.
        let call = extract_meta("const meta = { roles: [pick()] };\nreturn 1;");
        assert_eq!(call.unwrap_err().kind, WorkflowErrorKind::MetaViolation);

        // An identifier reference is not a literal.
        let ident = extract_meta("const meta = { roles: names };\nreturn 1;");
        assert_eq!(ident.unwrap_err().kind, WorkflowErrorKind::MetaViolation);

        // Missing meta is a violation, not a silent default.
        let missing = extract_meta("return 1;");
        assert_eq!(missing.unwrap_err().kind, WorkflowErrorKind::MetaViolation);
    }
}
