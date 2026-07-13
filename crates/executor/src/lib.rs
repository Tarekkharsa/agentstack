//! Governed, ephemeral code execution as a policy-agnostic domain.
//!
//! This crate validates execution requests, freezes exact tool grants and
//! resource limits into immutable plans, and defines the backend seams used by
//! the CLI. It deliberately does not read manifests, trust state, machine
//! policy, sessions, environment variables, or secrets. The CLI supplies an
//! already-authorized [`ToolAuthority`]; the gateway remains the only policy
//! and upstream-call enforcement point.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const DEFAULT_TIMEOUT_MS: u64 = 15_000;
pub const MAX_TIMEOUT_MS: u64 = 60_000;
pub const DEFAULT_MAX_CALLS: u32 = 20;
pub const MAX_CALLS: u32 = 100;
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;
pub const MAX_SOURCE_BYTES: usize = 256 * 1024;
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;
pub const MAX_RESULT_BYTES: usize = 1024 * 1024;
pub const MAX_GRANTED_TOOLS: usize = 100;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecuteRequest {
    pub code: String,
    pub allow_tools: Vec<String>,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub limits: RequestedLimits,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestedLimits {
    pub timeout_ms: Option<u64>,
    pub max_calls: Option<u32>,
    pub max_output_bytes: Option<usize>,
}

/// Machine-owned ceilings supplied by CLI composition. Request values can
/// only reduce these values, never increase them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineLimits {
    pub timeout_ms: u64,
    pub max_calls: u32,
    pub max_output_bytes: usize,
}

impl Default for MachineLimits {
    fn default() -> Self {
        Self {
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_calls: DEFAULT_MAX_CALLS,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

impl MachineLimits {
    pub fn validate(self) -> Result<Self, ExecuteError> {
        if self.timeout_ms == 0 || self.timeout_ms > MAX_TIMEOUT_MS {
            return Err(ExecuteError::invalid(
                "machine timeout is outside the hard ceiling",
            ));
        }
        if self.max_calls == 0 || self.max_calls > MAX_CALLS {
            return Err(ExecuteError::invalid(
                "machine call limit is outside the hard ceiling",
            ));
        }
        if self.max_output_bytes == 0 || self.max_output_bytes > MAX_OUTPUT_BYTES {
            return Err(ExecuteError::invalid(
                "machine output limit is outside the hard ceiling",
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveLimits {
    pub timeout_ms: u64,
    pub max_calls: u32,
    pub max_output_bytes: usize,
}

impl EffectiveLimits {
    fn resolve(request: &RequestedLimits, machine: MachineLimits) -> Result<Self, ExecuteError> {
        let machine = machine.validate()?;
        let timeout_ms = request.timeout_ms.unwrap_or(machine.timeout_ms);
        let max_calls = request.max_calls.unwrap_or(machine.max_calls);
        let max_output_bytes = request.max_output_bytes.unwrap_or(machine.max_output_bytes);
        if timeout_ms == 0 || max_calls == 0 || max_output_bytes == 0 {
            return Err(ExecuteError::invalid(
                "execution limits must be greater than zero",
            ));
        }
        Ok(Self {
            timeout_ms: timeout_ms.min(machine.timeout_ms),
            max_calls: max_calls.min(machine.max_calls),
            max_output_bytes: max_output_bytes.min(machine.max_output_bytes),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub input_schema: Value,
}

/// Exact, normalized authority. No wildcards or prefix semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolGrant {
    tools: BTreeSet<String>,
}

impl ToolGrant {
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.tools.iter().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    fn build(names: &[String], authority: &dyn ToolAuthority) -> Result<Self, ExecuteError> {
        if names.is_empty() {
            return Err(ExecuteError::invalid(
                "allowTools must name at least one tool",
            ));
        }
        if names.len() > MAX_GRANTED_TOOLS {
            return Err(ExecuteError::invalid("allowTools exceeds the hard limit"));
        }
        let mut tools = BTreeSet::new();
        for raw in names {
            let name = raw.trim();
            if name.is_empty() || name.contains('*') || name.contains('?') {
                return Err(ExecuteError::invalid(
                    "allowTools accepts exact tool names only",
                ));
            }
            if authority.describe(name).is_none() {
                return Err(ExecuteError::unknown_tool(name));
            }
            tools.insert(name.to_string());
        }
        Ok(Self { tools })
    }
}

pub trait ToolAuthority: Send + Sync {
    fn describe(&self, name: &str) -> Option<ToolDescriptor>;
    fn call(&self, grant: &ToolGrant, name: &str, args: Value) -> Result<Value, AuthorityError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthorityError {
    #[error("tool is not granted")]
    NotGranted,
    #[error("tool is denied by policy")]
    PolicyDenied,
    #[error("tool is unavailable")]
    Unavailable,
    #[error("tool call failed")]
    CallFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutePlan {
    pub execution_id: String,
    pub parent_run_id: Option<String>,
    pub project_digest: String,
    pub authority_digest: String,
    pub source_digest: String,
    pub input_digest: String,
    pub grant: ToolGrant,
    pub limits: EffectiveLimits,
    pub runtime: RuntimeIdentity,
    pub code: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeIdentity {
    pub name: String,
    pub image: String,
    pub protocol: u32,
}

#[derive(Debug, Clone)]
pub struct PlanContext {
    pub execution_id: String,
    pub parent_run_id: Option<String>,
    pub project_digest: String,
    pub authority_digest: String,
    pub runtime: RuntimeIdentity,
    pub machine_limits: MachineLimits,
}

pub fn build_plan(
    request: ExecuteRequest,
    context: PlanContext,
    authority: &dyn ToolAuthority,
) -> Result<ExecutePlan, ExecuteError> {
    if context.execution_id.is_empty() || context.execution_id.len() > 128 {
        return Err(ExecuteError::invalid("execution id is invalid"));
    }
    let source_len = request.code.len();
    if source_len == 0 || source_len > MAX_SOURCE_BYTES {
        return Err(ExecuteError::invalid(
            "source size is outside the allowed range",
        ));
    }
    let input_bytes = serde_json::to_vec(&request.input)
        .map_err(|_| ExecuteError::invalid("input is not serializable JSON"))?;
    if input_bytes.len() > MAX_INPUT_BYTES {
        return Err(ExecuteError::invalid("input exceeds the hard size limit"));
    }
    let grant = ToolGrant::build(&request.allow_tools, authority)?;
    let limits = EffectiveLimits::resolve(&request.limits, context.machine_limits)?;
    Ok(ExecutePlan {
        execution_id: context.execution_id,
        parent_run_id: context.parent_run_id,
        project_digest: context.project_digest,
        authority_digest: context.authority_digest,
        source_digest: digest_bytes(request.code.as_bytes()),
        input_digest: digest_bytes(&input_bytes),
        grant,
        limits,
        runtime: context.runtime,
        code: request.code,
        input: request.input,
    })
}

pub trait Executor {
    fn execute(
        &self,
        plan: &ExecutePlan,
        authority: &dyn ToolAuthority,
    ) -> Result<ExecutionOutput, ExecuteError>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionOutput {
    pub result: Value,
    pub calls: u32,
    pub duration_ms: u64,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
}

impl ExecutionOutput {
    pub fn validate(self, plan: &ExecutePlan) -> Result<Self, ExecuteError> {
        let result_len = serde_json::to_vec(&self.result)
            .map_err(|_| ExecuteError::invalid_result())?
            .len();
        if result_len > MAX_RESULT_BYTES
            || self.calls > plan.limits.max_calls
            || self.stdout_bytes.saturating_add(self.stderr_bytes) > plan.limits.max_output_bytes
        {
            return Err(ExecuteError::resource_limit());
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCategory {
    Untrusted,
    InvalidInput,
    UnknownTool,
    ToolNotGranted,
    PolicyDenied,
    RuntimeUnavailable,
    Timeout,
    ResourceLimit,
    ExecutionError,
    InvalidResult,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ExecuteError {
    pub category: ErrorCategory,
    message: String,
}

impl ExecuteError {
    pub fn public_message(&self) -> &'static str {
        match self.category {
            ErrorCategory::Untrusted => "the project is not trusted for execution",
            ErrorCategory::InvalidInput => "the execution request is invalid",
            ErrorCategory::UnknownTool => "a requested tool is unavailable",
            ErrorCategory::ToolNotGranted => "the program requested an ungranted tool",
            ErrorCategory::PolicyDenied => "a tool call was denied by policy",
            ErrorCategory::RuntimeUnavailable => "the isolated execution runtime is unavailable",
            ErrorCategory::Timeout => "execution exceeded its time limit",
            ErrorCategory::ResourceLimit => "execution exceeded a resource limit",
            ErrorCategory::ExecutionError => "the program failed",
            ErrorCategory::InvalidResult => "the program returned an invalid result",
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::InvalidInput,
            message: message.into(),
        }
    }

    fn unknown_tool(name: &str) -> Self {
        Self {
            category: ErrorCategory::UnknownTool,
            message: format!("unknown tool: {name}"),
        }
    }

    pub fn invalid_result() -> Self {
        Self {
            category: ErrorCategory::InvalidResult,
            message: "result is invalid".into(),
        }
    }

    pub fn resource_limit() -> Self {
        Self {
            category: ErrorCategory::ResourceLimit,
            message: "resource limit exceeded".into(),
        }
    }

    pub fn untrusted() -> Self {
        Self {
            category: ErrorCategory::Untrusted,
            message: "project trust is missing or stale".into(),
        }
    }

    pub fn runtime_unavailable() -> Self {
        Self {
            category: ErrorCategory::RuntimeUnavailable,
            message: "isolated runtime unavailable".into(),
        }
    }

    pub fn execution_error() -> Self {
        Self {
            category: ErrorCategory::ExecutionError,
            message: "isolated execution failed".into(),
        }
    }

    pub fn timeout() -> Self {
        Self {
            category: ErrorCategory::Timeout,
            message: "execution timed out".into(),
        }
    }
}

pub fn digest_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    struct FakeAuthority;
    impl ToolAuthority for FakeAuthority {
        fn describe(&self, name: &str) -> Option<ToolDescriptor> {
            matches!(name, "github__get_issue" | "github__list_comments").then(|| ToolDescriptor {
                name: name.into(),
                input_schema: json!({"type":"object"}),
            })
        }
        fn call(
            &self,
            grant: &ToolGrant,
            name: &str,
            _args: Value,
        ) -> Result<Value, AuthorityError> {
            if !grant.contains(name) {
                return Err(AuthorityError::NotGranted);
            }
            Ok(json!({"tool":name}))
        }
    }

    fn context() -> PlanContext {
        PlanContext {
            execution_id: "x-123".into(),
            parent_run_id: Some("r-123".into()),
            project_digest: "project".into(),
            authority_digest: "authority".into(),
            runtime: RuntimeIdentity {
                name: "fake".into(),
                image: "sha256:test".into(),
                protocol: 1,
            },
            machine_limits: MachineLimits::default(),
        }
    }

    fn request() -> ExecuteRequest {
        ExecuteRequest {
            code: "export default 1".into(),
            allow_tools: vec!["github__get_issue".into()],
            input: json!({"n":1}),
            limits: RequestedLimits::default(),
        }
    }

    #[test]
    fn plan_is_deterministic_and_grant_is_exact() {
        let one = build_plan(request(), context(), &FakeAuthority).unwrap();
        let two = build_plan(request(), context(), &FakeAuthority).unwrap();
        assert_eq!(one, two);
        assert!(one.grant.contains("github__get_issue"));
        assert!(!one.grant.contains("github__get"));
    }

    #[test]
    fn duplicate_grants_are_normalized_and_sorted() {
        let mut req = request();
        req.allow_tools = vec![
            "github__list_comments".into(),
            "github__get_issue".into(),
            "github__get_issue".into(),
        ];
        let plan = build_plan(req, context(), &FakeAuthority).unwrap();
        assert_eq!(
            plan.grant.iter().collect::<Vec<_>>(),
            ["github__get_issue", "github__list_comments"]
        );
    }

    #[test]
    fn wildcard_empty_and_unknown_grants_fail_closed() {
        for name in ["", "github__*", "missing__tool"] {
            let mut req = request();
            req.allow_tools = vec![name.into()];
            assert!(build_plan(req, context(), &FakeAuthority).is_err());
        }
    }

    #[test]
    fn request_limits_can_only_reduce_machine_limits() {
        let mut req = request();
        req.limits = RequestedLimits {
            timeout_ms: Some(MAX_TIMEOUT_MS),
            max_calls: Some(MAX_CALLS),
            max_output_bytes: Some(MAX_OUTPUT_BYTES),
        };
        let plan = build_plan(req, context(), &FakeAuthority).unwrap();
        assert_eq!(plan.limits.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert_eq!(plan.limits.max_calls, DEFAULT_MAX_CALLS);
        assert_eq!(plan.limits.max_output_bytes, DEFAULT_MAX_OUTPUT_BYTES);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(serde_json::from_value::<ExecuteRequest>(
            json!({"code":"x","allowTools":["github__get_issue"],"surprise":true})
        )
        .is_err());
    }

    #[test]
    fn source_input_and_result_boundaries_are_exact() {
        for size in [MAX_SOURCE_BYTES - 1, MAX_SOURCE_BYTES] {
            let mut req = request();
            req.code = "x".repeat(size);
            assert!(build_plan(req, context(), &FakeAuthority).is_ok());
        }
        let mut too_large = request();
        too_large.code = "x".repeat(MAX_SOURCE_BYTES + 1);
        assert_eq!(
            build_plan(too_large, context(), &FakeAuthority)
                .unwrap_err()
                .category,
            ErrorCategory::InvalidInput
        );

        // A JSON string contributes its two quote bytes.
        for serialized_size in [MAX_INPUT_BYTES - 1, MAX_INPUT_BYTES] {
            let mut req = request();
            req.input = Value::String("x".repeat(serialized_size - 2));
            assert_eq!(
                serde_json::to_vec(&req.input).unwrap().len(),
                serialized_size
            );
            assert!(build_plan(req, context(), &FakeAuthority).is_ok());
        }
        let mut input_too_large = request();
        input_too_large.input = Value::String("x".repeat(MAX_INPUT_BYTES - 1));
        assert!(build_plan(input_too_large, context(), &FakeAuthority).is_err());

        let plan = build_plan(request(), context(), &FakeAuthority).unwrap();
        for serialized_size in [MAX_RESULT_BYTES - 1, MAX_RESULT_BYTES] {
            let result = Value::String("x".repeat(serialized_size - 2));
            assert!(ExecutionOutput {
                result,
                calls: 0,
                duration_ms: 1,
                stdout_bytes: 0,
                stderr_bytes: 0,
            }
            .validate(&plan)
            .is_ok());
        }
        let result = Value::String("x".repeat(MAX_RESULT_BYTES - 1));
        assert_eq!(
            ExecutionOutput {
                result,
                calls: 0,
                duration_ms: 1,
                stdout_bytes: 0,
                stderr_bytes: 0,
            }
            .validate(&plan)
            .unwrap_err()
            .category,
            ErrorCategory::ResourceLimit
        );
    }

    #[test]
    fn malformed_numeric_limits_and_overflow_fail() {
        for value in [json!(0), json!(-1), json!(u64::MAX)] {
            let parsed = serde_json::from_value::<ExecuteRequest>(json!({
                "code": "x",
                "allowTools": ["github__get_issue"],
                "limits": { "maxCalls": value }
            }));
            assert!(
                parsed.is_err() || build_plan(parsed.unwrap(), context(), &FakeAuthority).is_err()
            );
        }
        assert!(serde_json::from_str::<ExecuteRequest>(
            r#"{"code":"x","allowTools":["github__get_issue"],"limits":{"timeoutMs":18446744073709551616}}"#
        )
        .is_err());
    }

    #[test]
    fn fake_backend_proves_authority_and_timeout_mapping_seams() {
        struct FakeExecutor(bool);
        impl Executor for FakeExecutor {
            fn execute(
                &self,
                plan: &ExecutePlan,
                authority: &dyn ToolAuthority,
            ) -> Result<ExecutionOutput, ExecuteError> {
                if self.0 {
                    return Err(ExecuteError::timeout());
                }
                let result = authority
                    .call(&plan.grant, "github__get_issue", json!({"n": 1}))
                    .map_err(|_| ExecuteError::execution_error())?;
                ExecutionOutput {
                    result,
                    calls: 1,
                    duration_ms: 2,
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                }
                .validate(plan)
            }
        }
        let plan = build_plan(request(), context(), &FakeAuthority).unwrap();
        assert_eq!(
            FakeExecutor(false)
                .execute(&plan, &FakeAuthority)
                .unwrap()
                .result["tool"],
            "github__get_issue"
        );
        assert_eq!(
            FakeExecutor(true)
                .execute(&plan, &FakeAuthority)
                .unwrap_err()
                .category,
            ErrorCategory::Timeout
        );
    }

    #[test]
    fn every_error_category_has_a_fixed_public_message() {
        let categories = [
            ErrorCategory::Untrusted,
            ErrorCategory::InvalidInput,
            ErrorCategory::UnknownTool,
            ErrorCategory::ToolNotGranted,
            ErrorCategory::PolicyDenied,
            ErrorCategory::RuntimeUnavailable,
            ErrorCategory::Timeout,
            ErrorCategory::ResourceLimit,
            ErrorCategory::ExecutionError,
            ErrorCategory::InvalidResult,
        ];
        for category in categories {
            let error = ExecuteError {
                category,
                message: "internal secret detail".into(),
            };
            assert!(!error.public_message().contains("secret"));
            assert!(!error.public_message().is_empty());
        }
    }

    proptest! {
        #[test]
        fn request_never_widens_machine_limits(timeout in 1u64..100_000, calls in 1u32..1000, bytes in 1usize..1_000_000) {
            let requested = RequestedLimits { timeout_ms: Some(timeout), max_calls: Some(calls), max_output_bytes: Some(bytes) };
            let effective = EffectiveLimits::resolve(&requested, MachineLimits::default()).unwrap();
            prop_assert!(effective.timeout_ms <= DEFAULT_TIMEOUT_MS);
            prop_assert!(effective.max_calls <= DEFAULT_MAX_CALLS);
            prop_assert!(effective.max_output_bytes <= DEFAULT_MAX_OUTPUT_BYTES);
        }
    }
}
