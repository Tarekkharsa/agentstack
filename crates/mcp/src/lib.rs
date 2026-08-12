//! MCP protocol boundary for AgentStack.
//!
//! RMCP owns wire models, lifecycle negotiation, framing, and HTTP semantics.
//! The rest of AgentStack talks to this crate through [`Backend`] using plain
//! JSON values, so protocol SDK types do not leak into trust, policy, or
//! manifest code.

use std::{borrow::Cow, future::Future, sync::Arc};

use rmcp::{
    model::{
        CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ClientCapabilities,
        ClientInfo, CompleteRequestMethod, CompleteRequestParams, CompleteResult, ContentBlock,
        Implementation, JsonObject, ListPromptsRequestMethod, ListPromptsResult,
        ListResourceTemplatesRequestMethod, ListResourceTemplatesResult,
        ListResourcesRequestMethod, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        ProtocolVersion, ResultType, ServerCapabilities, ServerInfo, Tool,
    },
    service::{NotificationContext, RequestContext, RoleClient, RoleServer, RunningService},
    ClientHandler, ClientLifecycleMode, ClientServiceExt, ErrorData as McpError, ServerHandler,
};
use serde_json::Value;
use tower_service::Service;

/// Protocol-neutral description of one dynamically discovered tool.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: JsonObject,
}

impl TryFrom<Value> for ToolDefinition {
    type Error = anyhow::Error;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let object = value
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("tool definition is not an object"))?;
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("tool definition has no name"))?
            .to_owned();
        let description = object
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let input_schema = object
            .get("inputSchema")
            .or_else(|| object.get("input_schema"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_else(|| {
                serde_json::Map::from_iter([("type".into(), Value::String("object".into()))])
            });
        Ok(Self {
            name,
            description,
            input_schema,
        })
    }
}

/// Completed tool call in AgentStack's domain shape.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutcome {
    pub value: Value,
    pub is_error: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolEra {
    Modern,
    Legacy,
}

impl ToolOutcome {
    pub fn success(value: Value) -> Self {
        Self {
            value,
            is_error: false,
        }
    }

    pub fn error(value: Value) -> Self {
        Self {
            value,
            is_error: true,
        }
    }

    /// Preserve an existing MCP tool-result body while moving its envelope to
    /// RMCP. This is the transition seam used by the current upstream gateway.
    pub fn from_mcp_result(value: Value) -> Self {
        let is_error = value
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Self { value, is_error }
    }
}

/// AgentStack-owned behavior consumed by the MCP protocol adapter.
///
/// Implementations may derive their surface on every call. That is important
/// for modern stateless HTTP, where two requests need not share a handler.
pub trait Backend: Send + Sync + 'static {
    fn instructions(&self) -> Option<String> {
        None
    }

    fn list_tools(&self, era: ProtocolEra) -> Result<Vec<ToolDefinition>, String>;

    fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        era: ProtocolEra,
    ) -> Result<ToolOutcome, String>;

    /// Legacy-only project roots reported by an initialized client. Modern
    /// 2026-07-28 clients never trigger this callback.
    fn set_legacy_roots(&self, _roots: Vec<String>) {}

    /// Take (and clear) the "my tool list changed" signal.
    ///
    /// The adapter polls this after every request it serves and, when it is
    /// set, sends `notifications/tools/list_changed`. This is not cosmetic for
    /// a legacy client: it fetches `tools/list` once and, per spec, refetches
    /// only on that notification — so a surface that appears later (roots-driven
    /// project binding, a lease opening or closing, any rebuild that changes the
    /// list) is unreachable without it. A modern client re-derives the list per
    /// request and simply ignores the extra frame.
    fn take_tool_list_changed(&self) -> bool {
        false
    }
}

/// Manual dynamic RMCP handler. AgentStack does not use static tool macros
/// because trust and toolset selection determine the surface at request time.
#[derive(Clone)]
pub struct AgentStackServer<B> {
    backend: Arc<B>,
    name: Cow<'static, str>,
    version: Cow<'static, str>,
}

impl<B> AgentStackServer<B> {
    pub fn new(
        backend: Arc<B>,
        name: impl Into<Cow<'static, str>>,
        version: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            backend,
            name: name.into(),
            version: version.into(),
        }
    }
}

impl<B: Backend> ServerHandler for AgentStackServer<B> {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(vec![
            ProtocolVersion::V_2026_07_28,
            ProtocolVersion::V_2025_11_25,
            ProtocolVersion::V_2025_06_18,
            ProtocolVersion::V_2025_03_26,
        ])
    }

    fn get_info(&self) -> ServerInfo {
        // `listChanged` is declared, not optional: a legacy client only
        // refetches `tools/list` when it is told the list changed, and
        // AgentStack's proxied surface appears after the first fetch (roots
        // binding, lease open/close, gateway rebuild). Without the capability
        // the client is entitled to ignore our notification entirely.
        let info = ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2026_07_28)
        .with_server_info(Implementation::new(
            self.name.as_ref(),
            self.version.as_ref(),
        ));
        match self.backend.instructions() {
            Some(instructions) => info.with_instructions(instructions),
            None => info,
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let era = if context
            .protocol_version()
            .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28)
        {
            ProtocolEra::Modern
        } else {
            ProtocolEra::Legacy
        };
        // AgentStack's domain boundary is deliberately synchronous and may
        // construct a short-lived Tokio runtime while connecting to an
        // upstream MCP server. Keep that work off RMCP's async worker so a
        // legacy upstream cannot trigger Tokio's nested-runtime panic.
        let backend = Arc::clone(&self.backend);
        let tools = tokio::task::spawn_blocking(move || backend.list_tools(era))
            .await
            .map_err(|error| {
                McpError::internal_error(format!("tool discovery task failed: {error}"), None)
            })?
            .map_err(|message| McpError::internal_error(message, None))?
            .into_iter()
            .map(|tool| {
                Tool::new_with_raw(
                    tool.name,
                    tool.description.map(Cow::Owned),
                    Arc::new(tool.input_schema),
                )
            })
            .collect();
        let result = ListToolsResult::with_all_items(tools);
        // Serving the list can itself be the moment the surface changes (a
        // modern request re-derives the project default). Clear and honour the
        // signal here too, or the next legacy client would wait for a
        // notification that was already consumed.
        self.announce_tool_list_change(&context).await;
        Ok(if era == ProtocolEra::Modern {
            result.with_ttl_ms(0).with_cache_scope(CacheScope::Private)
        } else {
            result
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let era = if context
            .protocol_version()
            .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28)
        {
            ProtocolEra::Modern
        } else {
            ProtocolEra::Legacy
        };
        let backend = Arc::clone(&self.backend);
        let name = request.name.into_owned();
        let outcome = tokio::task::spawn_blocking(move || backend.call_tool(&name, arguments, era))
            .await
            .map_err(|error| {
                McpError::internal_error(format!("tool call task failed: {error}"), None)
            })?
            .map_err(|message| McpError::invalid_params(message, None))?;
        let mut result = if outcome.value.is_object()
            && (outcome.value.get("content").is_some()
                || outcome.value.get("structuredContent").is_some())
        {
            serde_json::from_value::<CallToolResult>(outcome.value).map_err(|error| {
                McpError::internal_error(format!("invalid tool result from backend: {error}"), None)
            })?
        } else {
            let content = match outcome.value {
                Value::String(text) => vec![ContentBlock::text(text)],
                value => vec![ContentBlock::text(value.to_string())],
            };
            CallToolResult::success(content)
        };
        // A result produced by an older upstream has no SEP-2322
        // discriminator. RMCP will strip it again for legacy clients.
        result.result_type.get_or_insert(ResultType::COMPLETE);
        result.is_error = Some(outcome.is_error);
        // A control-plane call is the usual way the served surface changes
        // (lease open/close, a rebuild after a trust flip). Tell the client
        // before its own result reaches it is fine — order does not matter, the
        // notification only invalidates a cache.
        self.announce_tool_list_change(&context).await;
        Ok(result.into())
    }

    fn complete(
        &self,
        _request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CompleteResult, McpError>> + Send + '_ {
        std::future::ready(Err(McpError::method_not_found::<CompleteRequestMethod>()))
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + Send + '_ {
        std::future::ready(Err(McpError::method_not_found::<ListPromptsRequestMethod>()))
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
        std::future::ready(Err(
            McpError::method_not_found::<ListResourcesRequestMethod>(),
        ))
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, McpError>> + Send + '_ {
        std::future::ready(Err(McpError::method_not_found::<
            ListResourceTemplatesRequestMethod,
        >()))
    }

    #[allow(deprecated)]
    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        let supports_roots = context
            .peer
            .peer_info()
            .and_then(|info| info.capabilities.roots.clone())
            .is_some();
        if !supports_roots {
            return;
        }
        if let Ok(result) = context.peer.list_roots().await {
            let backend = Arc::clone(&self.backend);
            let roots = result.roots.into_iter().map(|root| root.uri).collect();
            let _ = tokio::task::spawn_blocking(move || backend.set_legacy_roots(roots)).await;
        }
        // Roots are how a legacy client tells us which project this session is
        // about, so this is the moment the proxied surface can appear. The
        // client has already fetched (or is about to fetch) an empty
        // `tools/list`; without this frame it never asks again.
        if self.backend.take_tool_list_changed() {
            let _ = context.peer.notify_tool_list_changed().await;
        }
    }
}

impl<B: Backend> AgentStackServer<B> {
    /// Emit `notifications/tools/list_changed` when the backend reports that
    /// the surface it just served is no longer the surface it would serve now.
    /// Best-effort: a client that has gone away fails the send, and the
    /// connection is about to end anyway.
    async fn announce_tool_list_change(&self, context: &RequestContext<RoleServer>) {
        if self.backend.take_tool_list_changed() {
            let _ = context.peer.notify_tool_list_changed().await;
        }
    }
}

/// Run one dual-era MCP server over newline-framed stdio.
pub async fn serve_stdio<B: Backend>(server: AgentStackServer<B>) -> anyhow::Result<()> {
    serve_io(server, rmcp::transport::stdio()).await
}

/// Serve stdin while writing protocol frames to a previously reserved stdout
/// handle. The adapter performs only small line writes, so the synchronous
/// writer does not broaden async requirements into AgentStack's domain code.
pub async fn serve_stdio_with_writer<B: Backend>(
    server: AgentStackServer<B>,
    writer: Box<dyn std::io::Write + Send>,
) -> anyhow::Result<()> {
    serve_io(server, (tokio::io::stdin(), SyncWriter(writer))).await
}

pub fn run_stdio_with_writer<B: Backend>(
    server: AgentStackServer<B>,
    writer: Box<dyn std::io::Write + Send>,
) -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(serve_stdio_with_writer(server, writer))
}

struct SyncWriter(Box<dyn std::io::Write + Send>);

impl tokio::io::AsyncWrite for SyncWriter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Ready(self.0.write(buf))
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(self.0.flush())
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(self.0.flush())
    }
}

/// Serve through a caller-provided RMCP transport (used by AgentStack to keep
/// its stdout-reservation safety wrapper).
pub async fn serve_io<B, T, E, A>(server: AgentStackServer<B>, transport: T) -> anyhow::Result<()>
where
    B: Backend,
    T: rmcp::transport::IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    use rmcp::ServiceExt;

    let running = server.serve(transport).await?;
    running.waiting().await?;
    Ok(())
}

/// Materialized HTTP response returned to AgentStack's authenticated outer
/// listener. Keeping the socket/token gate outside RMCP preserves the sandbox
/// security boundary while RMCP owns all MCP headers and lifecycle behavior.
pub struct HttpResponse {
    pub status: u16,
    pub headers: http::HeaderMap,
    pub body: Vec<u8>,
}

pub struct HttpRequest {
    pub method: String,
    pub uri: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Dual-era Streamable HTTP protocol engine.
///
/// The service keeps legacy sessions for old clients. Requests selecting
/// 2026-07-28 are always handled statelessly by RMCP and do not receive an
/// `Mcp-Session-Id`.
pub struct HttpServer<B: Backend> {
    runtime: tokio::runtime::Runtime,
    service: rmcp::transport::streamable_http_server::tower::StreamableHttpService<
        AgentStackServer<B>,
        rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
    >,
}

impl<B: Backend> HttpServer<B> {
    pub fn new(
        backend: Arc<B>,
        name: impl Into<Cow<'static, str>>,
        version: impl Into<Cow<'static, str>>,
    ) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let name = name.into();
        let version = version.into();
        let factory_backend = Arc::clone(&backend);
        let mut config =
            rmcp::transport::streamable_http_server::tower::StreamableHttpServerConfig::default();
        config.legacy_session_mode = true;
        config.json_response = true;
        config.max_request_body_bytes = 4 * 1024 * 1024;
        // The outer listener is deliberately reachable from a sandbox and
        // authenticates every request with a per-run token before it gets
        // here. Host validation would reject the container alias.
        config.allowed_hosts.clear();
        let service = rmcp::transport::streamable_http_server::tower::StreamableHttpService::new(
            move || {
                Ok(AgentStackServer::new(
                    Arc::clone(&factory_backend),
                    name.clone(),
                    version.clone(),
                ))
            },
            Arc::new(
                rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default(),
            ),
            config,
        );
        Ok(Self { runtime, service })
    }

    pub fn handle(&self, request: http::Request<Vec<u8>>) -> anyhow::Result<HttpResponse> {
        use http_body_util::{BodyExt, Full};

        let (parts, body) = request.into_parts();
        let request = http::Request::from_parts(parts, Full::new(bytes::Bytes::from(body)));
        let mut service = self.service.clone();
        self.runtime.block_on(async move {
            let response = service
                .call(request)
                .await
                .expect("RMCP HTTP service is infallible");
            let (parts, body) = response.into_parts();
            let body = body.collect().await?.to_bytes().to_vec();
            Ok(HttpResponse {
                status: parts.status.as_u16(),
                headers: parts.headers,
                body,
            })
        })
    }

    pub fn handle_parts(&self, request: HttpRequest) -> anyhow::Result<HttpResponse> {
        let mut builder = http::Request::builder()
            .method(request.method.as_str())
            .uri(request.uri.as_str());
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        self.handle(builder.body(request.body)?)
    }
}

#[derive(Clone, Default)]
struct UpstreamHandler;

impl ClientHandler for UpstreamHandler {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new("agentstack-gateway", env!("CARGO_PKG_VERSION")),
        )
        .with_protocol_version(ProtocolVersion::V_2025_11_25)
    }
}

/// Why one request to an upstream server failed.
///
/// The split exists for exactly one decision the caller has to make: whether
/// the cached client is still usable. A timeout leaves a healthy-but-slow
/// server connected — the pre-RMCP gateway deliberately kept the child in that
/// case — while a transport or protocol failure means the connection is gone
/// and the client must be dropped so the next call reconnects (and, for stdio,
/// respawns the child). Anything else failed after the reply arrived, so the
/// connection is fine.
#[derive(Debug)]
pub enum UpstreamError {
    /// AgentStack's own deadline elapsed. The connection stays.
    Timeout(String),
    /// The transport or the MCP session failed. Discard the client.
    Transport(String),
    /// The reply arrived but could not be converted. The connection stays.
    Decode(String),
}

impl UpstreamError {
    /// Whether this failure invalidates the connection it came from.
    pub fn is_transport(&self) -> bool {
        matches!(self, UpstreamError::Transport(_))
    }

    fn detail(&self) -> &str {
        match self {
            UpstreamError::Timeout(detail)
            | UpstreamError::Transport(detail)
            | UpstreamError::Decode(detail) => detail,
        }
    }
}

impl std::fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.detail())
    }
}

impl std::error::Error for UpstreamError {}

/// Result of one upstream request.
pub type UpstreamResult<T> = Result<T, UpstreamError>;

/// The HTTP authorization status an upstream refusal carries, if any: `401`
/// when the server demanded credentials, `403` when the ones it got were not
/// enough. `None` for every other failure.
///
/// Read from RMCP's own typed refusals rather than scraped out of the message.
/// The rendered chain routinely embeds the URL, so a substring search for
/// "401"/"403" called `http://127.0.0.1:4013/mcp` an authentication failure and
/// sent the user to fix a credential that was never the problem.
pub fn auth_status(error: &anyhow::Error) -> Option<u16> {
    use rmcp::transport::streamable_http_client::{AuthRequiredError, InsufficientScopeError};

    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error.as_ref());
    while let Some(current) = source {
        if current.is::<AuthRequiredError>() {
            return Some(401);
        }
        if current.is::<InsufficientScopeError>() {
            return Some(403);
        }
        source = current.source();
    }
    None
}

/// Synchronous facade over RMCP's asynchronous Streamable HTTP client.
/// AgentStack's gateway remains synchronous; the runtime is contained here.
pub struct HttpUpstreamClient {
    runtime: tokio::runtime::Runtime,
    running: Option<RunningService<RoleClient, UpstreamHandler>>,
    timeout: std::time::Duration,
}

impl HttpUpstreamClient {
    pub fn connect(
        url: &str,
        headers: &[(String, String)],
        timeout: std::time::Duration,
    ) -> anyhow::Result<Self> {
        use rmcp::transport::{
            streamable_http_client::StreamableHttpClientTransportConfig,
            StreamableHttpClientTransport,
        };
        use std::collections::HashMap;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let mut parsed = HashMap::new();
        for (name, value) in headers {
            parsed.insert(
                name.parse::<http::HeaderName>()?,
                value.parse::<http::HeaderValue>()?,
            );
        }
        let config =
            StreamableHttpClientTransportConfig::with_uri(url.to_owned()).custom_headers(parsed);
        let running = runtime.block_on(async {
            let deadline = tokio::time::Instant::now() + timeout;
            // One connection, the whole budget. `automatic_lifecycle()` probes
            // with `server/discover` and, when the peer answers Method Not
            // Found, negotiates the dated handshake on that SAME connection —
            // so a modern server is reachable at all (a short probe cap made it
            // unreachable) and a compliant legacy server costs no second
            // connection.
            let modern = tokio::time::timeout(
                timeout,
                UpstreamHandler.serve_with_lifecycle(
                    StreamableHttpClientTransport::from_config(config.clone()),
                    automatic_lifecycle(),
                ),
            )
            .await;
            let error = match modern {
                Ok(Ok(running)) => return Ok(running),
                // Out of budget: a second attempt has no time to run in, and
                // pretending otherwise would double the caller's ceiling.
                Err(_) => {
                    return Err(anyhow::anyhow!(
                        "MCP startup timed out after {}s",
                        timeout.as_secs()
                    ))
                }
                Ok(Err(error)) => error,
            };
            // The peer answered, but not in a way discovery could use — a
            // pre-discovery server that mis-answers `server/discover` instead
            // of refusing it. Only such a server pays for a second connection,
            // and only with the budget the probe left.
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(anyhow::Error::from(error));
            }
            tokio::time::timeout(
                remaining,
                UpstreamHandler.serve_with_lifecycle(
                    StreamableHttpClientTransport::from_config(config),
                    ClientLifecycleMode::Initialize,
                ),
            )
            .await
            .map_err(|_| anyhow::anyhow!("MCP startup timed out after {}s", timeout.as_secs()))?
            .map_err(anyhow::Error::from)
        })?;
        Ok(Self {
            runtime,
            running: Some(running),
            timeout,
        })
    }

    pub fn list_tools(&self) -> UpstreamResult<Vec<Value>> {
        let running = self
            .running
            .as_ref()
            .ok_or_else(|| UpstreamError::Transport("client closed".into()))?;
        // `list_all_tools` follows `nextCursor` to the end. `list_tools(None)`
        // returned page one and silently dropped the rest, so a paginating
        // upstream lost most of its surface.
        let tools = self.runtime.block_on(async {
            tokio::time::timeout(self.timeout, running.list_all_tools())
                .await
                .map_err(|_| {
                    UpstreamError::Timeout(format!(
                        "tools/list timed out after {}s",
                        self.timeout.as_secs()
                    ))
                })?
                .map_err(|error| UpstreamError::Transport(error.to_string()))
        })?;
        tools_to_values(tools)
    }

    pub fn server_identity(&self) -> (Option<String>, Option<String>) {
        let Some(running) = self.running.as_ref() else {
            return (None, None);
        };
        let Some(info) = running.peer_info() else {
            return (None, None);
        };
        (
            info.server_info.as_ref().map(|server| server.name.clone()),
            Some(info.protocol_version.to_string()),
        )
    }

    pub fn call_tool(&self, name: &str, arguments: Value) -> UpstreamResult<Value> {
        let running = self
            .running
            .as_ref()
            .ok_or_else(|| UpstreamError::Transport("client closed".into()))?;
        let params = call_params(name, arguments)?;
        let result = self.runtime.block_on(async {
            tokio::time::timeout(self.timeout, running.call_tool(params))
                .await
                .map_err(|_| {
                    UpstreamError::Timeout(format!(
                        "tools/call timed out after {}s",
                        self.timeout.as_secs()
                    ))
                })?
                .map_err(|error| UpstreamError::Transport(error.to_string()))
        })?;
        serde_json::to_value(result).map_err(|error| UpstreamError::Decode(error.to_string()))
    }
}

fn automatic_lifecycle() -> ClientLifecycleMode {
    ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        legacy_version: Some(ProtocolVersion::V_2025_11_25),
    }
}

/// Build one `tools/call` request. A non-object argument is the caller's bug,
/// not the connection's, so it never invalidates the client.
fn call_params(name: &str, arguments: Value) -> UpstreamResult<CallToolRequestParams> {
    let arguments = arguments
        .as_object()
        .cloned()
        .ok_or_else(|| UpstreamError::Decode("tool arguments must be an object".into()))?;
    Ok(CallToolRequestParams::new(name.to_owned()).with_arguments(arguments))
}

fn tools_to_values(tools: Vec<Tool>) -> UpstreamResult<Vec<Value>> {
    tools
        .into_iter()
        .map(|tool| {
            serde_json::to_value(tool).map_err(|error| UpstreamError::Decode(error.to_string()))
        })
        .collect()
}

impl Drop for HttpUpstreamClient {
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            let _ = self.runtime.block_on(running.cancel());
        }
    }
}

/// RMCP stdio client with AgentStack's process-tree cleanup contract.
pub struct StdioUpstreamClient {
    runtime: tokio::runtime::Runtime,
    running: Option<RunningService<RoleClient, UpstreamHandler>>,
    timeout: std::time::Duration,
    stderr: CapturedStderr,
}

const STDERR_CAPTURE_BYTES: usize = 4096;
type CapturedStderr = Arc<std::sync::Mutex<Vec<u8>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdioUpstreamErrorKind {
    Spawn,
    /// The child started and then ended before completing the handshake — the
    /// stream closed with no reply. Usually a bad argument or a rejected
    /// credential, which is a different answer to the user than a hang.
    Exited,
    Timeout,
    Protocol,
}

#[derive(Debug)]
pub struct StdioUpstreamError {
    pub kind: StdioUpstreamErrorKind,
    pub detail: String,
    pub stderr: String,
}

impl std::fmt::Display for StdioUpstreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail)
    }
}

impl std::error::Error for StdioUpstreamError {}

/// Tell "the child died" apart from "the child answered wrongly".
///
/// RMCP reports a stream that ended before the reply as `ConnectionClosed`,
/// and for a stdio child that is exactly one thing: the process is gone. The
/// distinction is the whole reason the caller classifies at all — a server that
/// exits is a bad argument or a rejected credential, while one that answers
/// nonsense is not speaking MCP.
fn handshake_failure_kind(error: &rmcp::service::ClientInitializeError) -> StdioUpstreamErrorKind {
    match error {
        rmcp::service::ClientInitializeError::ConnectionClosed(_) => StdioUpstreamErrorKind::Exited,
        _ => StdioUpstreamErrorKind::Protocol,
    }
}

fn stderr_text(capture: &CapturedStderr) -> String {
    let bytes = capture.lock().unwrap_or_else(|error| error.into_inner());
    String::from_utf8_lossy(&bytes).into_owned()
}

fn capture_stderr(mut stderr: tokio::process::ChildStderr) -> CapturedStderr {
    use tokio::io::AsyncReadExt;

    let capture = Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = Arc::clone(&capture);
    tokio::spawn(async move {
        let mut chunk = [0_u8; 4096];
        while let Ok(count) = stderr.read(&mut chunk).await {
            if count == 0 {
                break;
            }
            let mut kept = writer.lock().unwrap_or_else(|error| error.into_inner());
            let room = STDERR_CAPTURE_BYTES.saturating_sub(kept.len());
            if room > 0 {
                kept.extend_from_slice(&chunk[..count.min(room)]);
            }
        }
    });
    capture
}

impl StdioUpstreamClient {
    pub fn connect(
        command: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: &std::path::Path,
        startup_timeout: std::time::Duration,
        call_timeout: std::time::Duration,
    ) -> Result<Self, StdioUpstreamError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| StdioUpstreamError {
                kind: StdioUpstreamErrorKind::Protocol,
                detail: error.to_string(),
                stderr: String::new(),
            })?;
        let make_transport = || -> Result<_, StdioUpstreamError> {
            use process_wrap::tokio::{CommandWrap, KillOnDrop};
            let mut wrapped = CommandWrap::with_new(command, |process| {
                process
                    .args(args)
                    .envs(env.iter().cloned())
                    .current_dir(cwd);
            });
            #[cfg(unix)]
            wrapped.wrap(process_wrap::tokio::ProcessGroup::leader());
            #[cfg(windows)]
            wrapped.wrap(process_wrap::tokio::JobObject);
            wrapped.wrap(KillOnDrop);
            let (transport, stderr) = rmcp::transport::TokioChildProcess::builder(wrapped)
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|error| StdioUpstreamError {
                    kind: StdioUpstreamErrorKind::Spawn,
                    detail: format!("spawning '{}' in {}: {error}", command, cwd.display()),
                    stderr: String::new(),
                })?;
            let capture = stderr.map(capture_stderr).unwrap_or_default();
            Ok((transport, capture))
        };
        let deadline = tokio::time::Instant::now() + startup_timeout;
        let (running, stderr) = runtime.block_on(async {
            let (modern_transport, modern_stderr) = make_transport()?;
            // ONE spawn, the WHOLE startup budget. `automatic_lifecycle()`
            // probes with `server/discover` and falls back to the dated
            // handshake on the same child when the peer answers Method Not
            // Found, which is what the MCP 2026-07-28 migration guidance
            // prescribes. Capping this probe at 100ms made almost every real
            // stdio server fail it, get killed, and be spawned a second time —
            // modern protocol unreachable and every start-up side effect done
            // twice.
            let modern = tokio::time::timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
                UpstreamHandler.serve_with_lifecycle(modern_transport, automatic_lifecycle()),
            )
            .await;
            match modern {
                Ok(Ok(running)) => Ok((running, modern_stderr)),
                // The budget is spent. There is no second attempt to make, and
                // inventing one would double the caller's ceiling.
                Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    Err(StdioUpstreamError {
                        kind: StdioUpstreamErrorKind::Timeout,
                        detail: format!(
                            "MCP startup timed out after {}s",
                            startup_timeout.as_secs_f64()
                        ),
                        stderr: stderr_text(&modern_stderr),
                    })
                }
                // The child answered, but not in a way discovery could use: a
                // pre-discovery server that mis-answers `server/discover`
                // rather than refusing it. Only such a server pays for a second
                // spawn, and only out of the budget the probe left.
                Ok(Err(_)) => {
                    // Cancellation drops the RMCP child transport, whose
                    // process-wrapper kill runs asynchronously. Keep this
                    // runtime alive briefly so the failed probe cannot leave
                    // the launcher or any descendant behind.
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining.is_zero() {
                        return Err(StdioUpstreamError {
                            kind: StdioUpstreamErrorKind::Timeout,
                            detail: format!(
                                "MCP startup timed out after {}s",
                                startup_timeout.as_secs_f64()
                            ),
                            stderr: stderr_text(&modern_stderr),
                        });
                    }
                    let (legacy_transport, legacy_stderr) = make_transport()?;
                    match tokio::time::timeout(
                        remaining,
                        UpstreamHandler.serve_with_lifecycle(
                            legacy_transport,
                            ClientLifecycleMode::Initialize,
                        ),
                    )
                    .await
                    {
                        Ok(Ok(running)) => Ok((running, legacy_stderr)),
                        Ok(Err(error)) => {
                            tokio::task::yield_now().await;
                            Err(StdioUpstreamError {
                                kind: handshake_failure_kind(&error),
                                detail: error.to_string(),
                                stderr: stderr_text(&legacy_stderr),
                            })
                        }
                        Err(_) => {
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            Err(StdioUpstreamError {
                                kind: StdioUpstreamErrorKind::Timeout,
                                detail: format!(
                                    "MCP startup timed out after {}s",
                                    startup_timeout.as_secs_f64()
                                ),
                                stderr: stderr_text(&legacy_stderr),
                            })
                        }
                    }
                }
            }
        })?;
        Ok(Self {
            runtime,
            running: Some(running),
            timeout: call_timeout,
            stderr,
        })
    }

    pub fn list_tools(&self) -> UpstreamResult<Vec<Value>> {
        self.list_tools_with_timeout(self.timeout)
    }

    pub fn list_tools_with_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> UpstreamResult<Vec<Value>> {
        let running = self
            .running
            .as_ref()
            .ok_or_else(|| UpstreamError::Transport("client closed".into()))?;
        // Every page, not just the first: `list_tools(None)` dropped
        // `nextCursor` and truncated a paginating upstream to page one.
        let tools = self.runtime.block_on(async {
            tokio::time::timeout(timeout, running.list_all_tools())
                .await
                .map_err(|_| {
                    UpstreamError::Timeout(format!(
                        "tools/list timed out after {}s",
                        timeout.as_secs_f64()
                    ))
                })?
                .map_err(|error| UpstreamError::Transport(error.to_string()))
        })?;
        tools_to_values(tools)
    }

    pub fn captured_stderr(&self) -> String {
        stderr_text(&self.stderr)
    }

    pub fn server_identity(&self) -> (Option<String>, Option<String>) {
        let Some(running) = self.running.as_ref() else {
            return (None, None);
        };
        let Some(info) = running.peer_info() else {
            return (None, None);
        };
        (
            info.server_info.as_ref().map(|server| server.name.clone()),
            Some(info.protocol_version.to_string()),
        )
    }

    pub fn call_tool(&self, name: &str, arguments: Value) -> UpstreamResult<Value> {
        let running = self
            .running
            .as_ref()
            .ok_or_else(|| UpstreamError::Transport("client closed".into()))?;
        let params = call_params(name, arguments)?;
        let result = self.runtime.block_on(async {
            tokio::time::timeout(self.timeout, running.call_tool(params))
                .await
                .map_err(|_| {
                    UpstreamError::Timeout(format!(
                        "tools/call timed out after {}s",
                        self.timeout.as_secs()
                    ))
                })?
                .map_err(|error| UpstreamError::Transport(error.to_string()))
        })?;
        serde_json::to_value(result).map_err(|error| UpstreamError::Decode(error.to_string()))
    }
}

impl Drop for StdioUpstreamClient {
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            let _ = self.runtime.block_on(running.cancel());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::{ClientHandler, ClientLifecycleMode, ClientServiceExt, ServiceExt};

    #[derive(Default)]
    struct TestBackend {
        changed: std::sync::atomic::AtomicBool,
    }

    impl Backend for TestBackend {
        fn take_tool_list_changed(&self) -> bool {
            self.changed
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        }

        fn list_tools(&self, _era: ProtocolEra) -> Result<Vec<ToolDefinition>, String> {
            Ok(vec![ToolDefinition {
                name: "echo".into(),
                description: Some("Echo input".into()),
                input_schema: serde_json::from_value(serde_json::json!({
                    "type": "object",
                    "properties": { "text": { "type": "string" } }
                }))
                .expect("object schema"),
            }])
        }

        fn call_tool(
            &self,
            _name: &str,
            arguments: Value,
            _era: ProtocolEra,
        ) -> Result<ToolOutcome, String> {
            Ok(ToolOutcome::success(arguments))
        }
    }

    #[derive(Clone, Default)]
    struct TestClient {
        list_changed: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl ClientHandler for TestClient {
        async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
            self.list_changed
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn modern_discovery_and_legacy_initialize_share_one_handler() {
        for lifecycle in [
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
            ClientLifecycleMode::Initialize,
        ] {
            let (server_io, client_io) = tokio::io::duplex(64 * 1024);
            let server =
                AgentStackServer::new(Arc::new(TestBackend::default()), "agentstack-test", "1");
            let server_task = tokio::spawn(async move {
                let running = server.serve(server_io).await.expect("serve server");
                running.waiting().await.expect("server closes");
            });
            let client = TestClient::default()
                .serve_with_lifecycle(client_io, lifecycle)
                .await
                .expect("serve client");
            let listed = client
                .list_tools(Default::default())
                .await
                .expect("list tools");
            assert_eq!(listed.tools.len(), 1);
            assert_eq!(listed.tools[0].name, "echo");
            client.cancel().await.expect("cancel client");
            server_task.await.expect("server task");
        }
    }

    /// A legacy client fetches `tools/list` once and, per spec, refetches only
    /// when told the list changed — and it is entitled to ignore the
    /// notification unless the capability was declared. AgentStack's proxied
    /// surface appears AFTER that first fetch (roots binding, lease open), so
    /// without both halves of this the tools are simply unreachable.
    #[tokio::test]
    async fn a_changed_tool_list_is_declared_and_then_announced() {
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let backend = Arc::new(TestBackend::default());
        let server = AgentStackServer::new(Arc::clone(&backend), "agentstack-test", "1");
        let server_task = tokio::spawn(async move {
            let running = server.serve(server_io).await.expect("serve server");
            running.waiting().await.expect("server closes");
        });
        let client = TestClient::default();
        let seen = Arc::clone(&client.list_changed);
        let client = client
            .serve_with_lifecycle(client_io, ClientLifecycleMode::Initialize)
            .await
            .expect("serve client");

        assert_eq!(
            client
                .peer_info()
                .and_then(|info| info.capabilities.tools.clone())
                .and_then(|tools| tools.list_changed),
            Some(true),
            "listChanged must be declared or the client may ignore the notification"
        );

        // Nothing changed yet: serving a list must not invent a notification.
        client
            .list_tools(Default::default())
            .await
            .expect("list tools");
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 0);

        // The surface changes between requests, exactly as a lease opening or
        // a roots-driven project binding changes it.
        backend
            .changed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        client
            .list_tools(Default::default())
            .await
            .expect("list tools");
        // The notification travels on its own frame, so give the client task a
        // turn to receive it.
        for _ in 0..100 {
            if seen.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(
            seen.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the client was never told its tool list went stale"
        );

        client.cancel().await.expect("cancel client");
        server_task.await.expect("server task");
    }
}
