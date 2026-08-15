//! Local conversion proxy server (axum) for Codex.
//!
//! Each Codex profile with protocol conversion enabled runs one axum server on
//! a random loopback port. The Codex client talks Responses API to this
//! server; it converts the request to Chat Completions, forwards it to the
//! real upstream, and converts the (streaming) response back to Responses SSE.
//!
//! Orchestration mirrors cc-switch `proxy/handlers.rs` +
//! `proxy/forwarder.rs`, minus failover/retry/usage-logging.

use super::content_encoding::{
    decompress_body_with_limit, get_content_encoding, is_supported_content_encoding,
    DecompressError,
};
use super::history::CodexChatHistoryStore;
use super::stream::create_responses_sse_stream_from_chat_with_context;
use super::transform::{
    build_codex_tool_context_from_request, chat_completion_to_response_with_context,
    chat_error_to_response_error, responses_to_chat_compaction,
    responses_to_chat_completions_with_reasoning,
};
use axum::body::{to_bytes, Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures::{stream, Stream, StreamExt};
use serde_json::{json, Value};
use url::Url;
#[cfg(debug_assertions)]
use std::fs::{create_dir_all, OpenOptions};
#[cfg(debug_assertions)]
use std::io::Write;
#[cfg(debug_assertions)]
use std::path::Path;
#[cfg(debug_assertions)]
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
#[cfg(debug_assertions)]
use std::time::{SystemTime, UNIX_EPOCH};

/// Mirrors cc-switch's 200 MB request body ceiling.
const REQUEST_BODY_LIMIT: usize = 200 * 1024 * 1024;
const MAX_RESPONSE_BODY_BYTES: usize = 128 * 1024 * 1024;
const UPSTREAM_RESPONSE_HEADERS_TIMEOUT: Duration = Duration::from_secs(60);
const UPSTREAM_STREAM_FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(60);
const UPSTREAM_BODY_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const UPSTREAM_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) struct ProxyState {
    pub upstream_base_url: String,
    /// 上游 API key（仅内存，不落盘）。Codex 桌面端 / VSCode 扩展读取不到
    /// 启动器 DPAPI 加密的 key，转发时必须由代理注入 Authorization 头。
    pub api_key: Option<String>,
    pub provider_name: String,
    pub default_model: String,
    pub chat_upstream_model: Option<String>,
    pub catalog_model_ids: Vec<String>,
    pub prompt_cache_routing: String,
    pub reasoning_override: Option<super::reasoning::CodexChatReasoningConfig>,
    pub history: Arc<CodexChatHistoryStore>,
    pub http: reqwest::Client,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompactRequestInfo {
    is_compact: bool,
    source: &'static str,
    client_metadata: &'static str,
    turn_metadata: &'static str,
    request_kind: &'static str,
    trigger: String,
    reason: String,
    implementation: String,
    phase: String,
    strategy: String,
}

impl CompactRequestInfo {
    fn detect(body: &Value, explicit_compact_endpoint: bool) -> Self {
        let client_metadata = body.get("client_metadata");
        let client_metadata_state = match client_metadata {
            None => "absent",
            Some(Value::Object(_)) => "object",
            Some(_) => "invalid",
        };

        let turn_metadata = client_metadata
            .and_then(Value::as_object)
            .and_then(|metadata| metadata.get("x-codex-turn-metadata"));
        let (turn_metadata_state, turn_metadata_value) = match turn_metadata {
            None => ("absent", None),
            Some(Value::String(encoded)) => match serde_json::from_str::<Value>(encoded) {
                Ok(value) if value.is_object() => ("valid", Some(value)),
                Ok(_) => ("invalid-object", None),
                Err(_) => ("invalid-json", None),
            },
            Some(Value::Object(value)) => ("valid", Some(Value::Object(value.clone()))),
            Some(_) => ("invalid-type", None),
        };

        let request_kind_value = turn_metadata_value
            .as_ref()
            .and_then(|metadata| metadata.get("request_kind"));
        let request_kind = match request_kind_value.and_then(Value::as_str) {
            Some("compaction") => "compaction",
            Some(_) => "other",
            None if request_kind_value.is_some() => "invalid",
            None => "absent",
        };
        let metadata_compact = request_kind == "compaction";
        let is_compact = explicit_compact_endpoint || metadata_compact;
        let source = match (explicit_compact_endpoint, metadata_compact) {
            (true, true) => "endpoint+metadata",
            (true, false) => "endpoint",
            (false, true) => "metadata",
            (false, false) => "none",
        };

        let metadata_field = |key: &str| {
            turn_metadata_value
                .as_ref()
                .and_then(|metadata| {
                    let (group, field) = key.split_once('.')?;
                    metadata.get(group)?.get(field)
                })
                .and_then(Value::as_str)
                .map(safe_compact_metadata_label)
                .unwrap_or_else(|| "-".to_string())
        };

        Self {
            is_compact,
            source,
            client_metadata: client_metadata_state,
            turn_metadata: turn_metadata_state,
            request_kind,
            trigger: metadata_field("compaction.trigger"),
            reason: metadata_field("compaction.reason"),
            implementation: metadata_field("compaction.implementation"),
            phase: metadata_field("compaction.phase"),
            strategy: metadata_field("compaction.strategy"),
        }
    }
}

fn safe_compact_metadata_label(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() || value.len() > 64 {
        return "<redacted>".to_string();
    }
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'))
    {
        value.to_string()
    } else {
        "<redacted>".to_string()
    }
}

#[cfg(debug_assertions)]
const DEFAULT_COMPACT_DEBUG_LOG: &str =
    r"D:\project\cc-launcher\log\codex-compact-debug.log";
#[cfg(debug_assertions)]
const DEFAULT_COMPACT_DEBUG_MARKER: &str =
    r"D:\project\cc-launcher\log\codex-compact-debug.enabled";

#[cfg(debug_assertions)]
fn compact_debug_enabled() -> bool {
    std::env::var_os("CODEX_PROXY_DEBUG_COMPACT").is_some()
        || Path::new(DEFAULT_COMPACT_DEBUG_MARKER).exists()
}

#[cfg(debug_assertions)]
fn compact_debug_log_path() -> PathBuf {
    std::env::var_os("CODEX_PROXY_DEBUG_COMPACT_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_COMPACT_DEBUG_LOG))
}

#[cfg(debug_assertions)]
fn compact_debug_emit(message: impl AsRef<str>) {
    if !compact_debug_enabled() {
        return;
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let line = format!("{timestamp} {}", message.as_ref());

    let path = compact_debug_log_path();
    if let Some(parent) = path.parent() {
        let _ = create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }

    eprintln!("{line}");
}

#[cfg(debug_assertions)]
fn compact_input_shape(value: Option<&Value>) -> String {
    let Some(items) = value.and_then(Value::as_array) else {
        return "not-array".to_string();
    };

    items
        .iter()
        .map(|item| {
            let item_type = item
                .get("type")
                .and_then(Value::as_str)
                .or_else(|| item.get("role").and_then(Value::as_str))
                .unwrap_or("unknown");
            let role = item.get("role").and_then(Value::as_str).unwrap_or("-");
            let content_blocks = item
                .get("content")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let text_chars = ["text", "output", "content"]
                .iter()
                .filter_map(|key| item.get(*key))
                .map(compact_text_len)
                .sum::<usize>();
            format!(
                "{item_type}(role={role},content_blocks={content_blocks},call_id={},text_chars={text_chars})",
                item.get("call_id").is_some()
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(debug_assertions)]
fn compact_text_len(value: &Value) -> usize {
    match value {
        Value::String(text) => text.chars().count(),
        Value::Array(values) => values.iter().map(compact_text_len).sum(),
        Value::Object(object) => object
            .values()
            .map(compact_text_len)
            .sum(),
        _ => 0,
    }
}

#[cfg(debug_assertions)]
fn log_compact_request(path: &str, body: &Value, compact: &CompactRequestInfo) {
    if !compact_debug_enabled() {
        return;
    }

    let model = body.get("model").and_then(Value::as_str).unwrap_or("-");
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let input = body.get("input");
    let input_count = input
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let tools_count = body
        .get("tools")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);

    compact_debug_emit(format!(
        "[codex-proxy][compact-debug] request path={path} source={} model={model} stream={stream} previous_response_id_present={} instructions_present={} input_count={input_count} tools_count={tools_count} metadata={} turn_metadata={} request_kind={} trigger={} reason={} implementation={} phase={} strategy={} input=[{}]",
        compact.source,
        body.get("previous_response_id").is_some(),
        body.get("instructions").is_some(),
        compact.client_metadata,
        compact.turn_metadata,
        compact.request_kind,
        compact.trigger,
        compact.reason,
        compact.implementation,
        compact.phase,
        compact.strategy,
        compact_input_shape(input),
    ));
}

#[cfg(debug_assertions)]
fn log_responses_request(path: &str, body: &Value, compact: &CompactRequestInfo) {
    if !compact_debug_enabled() {
        return;
    }

    let input = body.get("input");
    let input_count = input
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let context_management = body.get("context_management").is_some();
    compact_debug_emit(format!(
        "[codex-proxy][responses-debug] request path={path} compact_endpoint={} compact_source={} model={} stream={stream} previous_response_id_present={} context_management_present={context_management} instructions_present={} input_count={input_count} metadata={} turn_metadata={} request_kind={} trigger={} reason={} implementation={} phase={} strategy={} input=[{}]",
        compact.is_compact,
        compact.source,
        body.get("model").and_then(Value::as_str).unwrap_or("-"),
        body.get("previous_response_id").is_some(),
        body.get("instructions").is_some(),
        compact.client_metadata,
        compact.turn_metadata,
        compact.request_kind,
        compact.trigger,
        compact.reason,
        compact.implementation,
        compact.phase,
        compact.strategy,
        compact_input_shape(input),
    ));
}

#[cfg(debug_assertions)]
fn log_compact_chat_request(body: &Value, compact: &CompactRequestInfo) {
    if !compact_debug_enabled() {
        return;
    }

    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .map(|message| {
                    let role = message
                        .get("role")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    let chars = message
                        .get("content")
                        .map(compact_text_len)
                        .unwrap_or(0);
                    format!("{role}(text_chars={chars})")
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "not-array".to_string());

    compact_debug_emit(format!(
        "[codex-proxy][compact-debug] chat-request source={} conversion=responses_to_chat_compaction message_count={} tools_present={} messages=[{messages}]",
        compact.source,
        body.get("messages")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        body.get("tools").is_some(),
    ));
}

#[cfg(debug_assertions)]
fn log_compact_responses_body(body: &Value) {
    if !compact_debug_enabled() {
        return;
    }

    let output_types = body
        .get("output")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_else(|| "not-array".to_string());

    compact_debug_emit(format!(
        "[codex-proxy][compact-debug] responses-response object={} status={} output_count={} output_types=[{output_types}]",
        body.get("object").and_then(Value::as_str).unwrap_or("-"),
        body.get("status").and_then(Value::as_str).unwrap_or("-"),
        body.get("output")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
    ));
}

pub(crate) fn router(state: Arc<ProxyState>) -> Router {
    #[cfg(debug_assertions)]
    if compact_debug_enabled() {
        compact_debug_emit("[codex-proxy][debug] proxy-ready");
    }

    Router::new()
        .route("/v1/responses", post(handle_responses))
        .route("/responses", post(handle_responses))
        .route("/v1/responses/compact", post(handle_responses_compact))
        .route("/responses/compact", post(handle_responses_compact))
        .route("/v1/models", get(handle_models))
        .route("/models", get(handle_models))
        .with_state(state)
}

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    let body = json!({
        "error": {
            "message": message.into(),
            "type": "proxy_error",
            "code": Value::Null,
            "param": Value::Null,
        }
    });
    (
        status,
        [(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        Body::from(serde_json::to_vec(&body).unwrap_or_default()),
    )
        .into_response()
}

fn response_body_error(message: impl Into<String>) -> Response {
    error_response(StatusCode::BAD_GATEWAY, message)
}

const MAX_ERROR_TEXT_BYTES: usize = 16 * 1024;
const ERROR_TRUNCATION_SUFFIX: &str = "...[truncated]";

fn truncate_error_strings(value: &mut Value) {
    match value {
        Value::String(text) => {
            if text.len() > MAX_ERROR_TEXT_BYTES {
                let mut end = 0;
                for (index, character) in text.char_indices() {
                    let next = index + character.len_utf8();
                    if next > MAX_ERROR_TEXT_BYTES {
                        break;
                    }
                    end = next;
                }
                text.truncate(end);
                text.push_str(ERROR_TRUNCATION_SUFFIX);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(truncate_error_strings),
        Value::Object(object) => object.values_mut().for_each(truncate_error_strings),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// Attach safe, structured routing context to an upstream error without
/// echoing the upstream URL (which may contain credentials in a query string).
fn annotate_upstream_error(
    value: &mut Value,
    provider_name: &str,
    model: &str,
    endpoint: &str,
    status: StatusCode,
) {
    let Some(error) = value
        .get_mut("error")
        .and_then(Value::as_object_mut)
    else {
        return;
    };

    error.insert(
        "provider".to_string(),
        Value::String(provider_name.to_string()),
    );
    error.insert("model".to_string(), Value::String(model.to_string()));
    error.insert(
        "endpoint".to_string(),
        Value::String(endpoint.to_string()),
    );
    error.insert(
        "upstream_status".to_string(),
        Value::Number(serde_json::Number::from(status.as_u16())),
    );
}

/// 构建 Chat Completions 上游 URL，保留客户端 query（与 cc-switch 的
/// `rewrite_codex_responses_endpoint_to_chat` 行为一致）。
fn append_query(url: &mut Url, query: Option<&str>) {
    let Some(query) = query.filter(|query| !query.is_empty()) else {
        return;
    };
    let merged = match url.query().filter(|value| !value.is_empty()) {
        Some(existing) => format!("{existing}&{query}"),
        None => query.to_string(),
    };
    url.set_query(Some(&merged));
}

fn endpoint_url(upstream_base_url: &str, endpoint: &str, query: Option<&str>) -> String {
    let Ok(mut url) = Url::parse(upstream_base_url.trim()) else {
        // The profile validator rejects this in normal operation. Keeping a
        // deterministic fallback makes this helper total for diagnostics and
        // unit tests without hiding the eventual reqwest error.
        let base = upstream_base_url.trim_end_matches('/');
        return if query.filter(|query| !query.is_empty()).is_some() {
            format!("{base}/{endpoint}?{}", query.unwrap_or_default())
        } else {
            format!("{base}/{endpoint}")
        };
    };

    let path = url.path().trim_end_matches('/');
    let endpoint_suffix = format!("/{endpoint}");
    let path = if path == endpoint_suffix || path.ends_with(&endpoint_suffix) {
        path.to_string()
    } else if path.is_empty() {
        format!("/{endpoint}")
    } else {
        format!("{path}/{endpoint}")
    };
    url.set_path(&path);
    append_query(&mut url, query);
    url.to_string()
}

fn chat_completions_url(upstream_base_url: &str, query: Option<&str>) -> String {
    endpoint_url(upstream_base_url, "chat/completions", query)
}

fn models_url(upstream_base_url: &str, query: Option<&str>) -> String {
    endpoint_url(upstream_base_url, "models", query)
}

fn is_sse_content_type(value: &str) -> bool {
    value
        .split(';')
        .next()
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("text/event-stream"))
}

fn body_looks_like_sse(body: &[u8]) -> bool {
    let Ok(body) = std::str::from_utf8(body) else {
        return false;
    };
    let trimmed = body.trim_start_matches('\u{feff}').trim_start();
    ["data:", "event:", "id:", "retry:", ":"]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

fn should_send_prompt_cache_key(routing: &str, base_url: &str) -> bool {
    match routing.trim().to_ascii_lowercase().as_str() {
        "enabled" => return true,
        "disabled" => return false,
        _ => {}
    }
    let Ok(url) = Url::parse(base_url) else {
        return false;
    };
    match url.host_str() {
        Some("api.openai.com") => true,
        Some("api.kimi.com") => {
            let path = url.path().trim_end_matches('/');
            path == "/coding" || path.starts_with("/coding/")
        }
        _ => false,
    }
}

fn apply_chat_upstream_model(body: &mut Value, state: &ProxyState) -> String {
    let had_explicit_model = body
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(|model| !model.trim().is_empty());
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(state.default_model.as_str())
        .to_string();
    if body.get("model").is_none() {
        body["model"] = Value::String(requested_model.clone());
    }

    if !had_explicit_model
        || !state.catalog_model_ids.iter().any(|model| model == &requested_model)
    {
        if let Some(upstream_model) = state
            .chat_upstream_model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
        {
            body["model"] = Value::String(upstream_model.to_string());
            return upstream_model.to_string();
        }
    }
    requested_model
}

/// Forward selected request headers to the upstream.
///
/// Allowlist mode: only headers that are safe and needed pass through.
/// Everything else (host, trace headers, SDK fingerprint headers, content
/// entity headers) is dropped — Cloudflare-backed upstreams answer 400 for
/// some of the headers the Codex desktop client sends. `Authorization` is
/// replaced by the registered API key and `content-type` /
/// `accept-encoding` are forced in the caller.
fn build_forward_headers(incoming: &HeaderMap) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in incoming.iter() {
        match name.as_str() {
            "x-session-id"
            | "openai-beta"
            | "openai-organization"
            | "openai-project"
            | "originator"
            | "user-agent"
            | "x-request-id"
            | "x-client-request-id"
            | "traceparent"
            | "tracestate" => {
                headers.insert(name.clone(), value.clone());
            }
            _ => {}
        }
    }
    headers
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

/// Rebuild upstream response headers for a rewritten body: keep non-entity
/// headers, drop hop-by-hop and content-encoding headers, force JSON type.
fn rebuilt_response_headers(upstream: &HeaderMap, content_type: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in upstream.iter() {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        match name.as_str() {
            "content-type" | "content-length" | "content-encoding" | "transfer-encoding" => {}
            _ => {
                headers.insert(name.clone(), value.clone());
            }
        }
    }
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type).unwrap_or_else(|_| HeaderValue::from_static("application/json")),
    );
    headers
}

async fn handle_responses(State(state): State<Arc<ProxyState>>, request: Request) -> Response {
    handle_responses_inner(state, request, false).await
}

/// Handle the standalone Responses compaction endpoint.
///
/// Chat-only upstreams have no native compaction endpoint. The request still
/// needs its own route so Codex can continue its context-management flow; it is
/// converted through the same Responses -> Chat pipeline and the returned Chat
/// message is converted back into a Responses item that can be replayed.
async fn handle_responses_compact(
    State(state): State<Arc<ProxyState>>,
    request: Request,
) -> Response {
    handle_responses_inner(state, request, true).await
}

async fn handle_responses_inner(
    state: Arc<ProxyState>,
    request: Request,
    explicit_compact_endpoint: bool,
) -> Response {
    let (parts, body) = request.into_parts();
    let incoming_headers = parts.headers.clone();

    // 诊断：打印 Codex 客户端发来的请求头（dev 控制台可见）。
    #[cfg(debug_assertions)]
    {
        let header_summary = incoming_headers
            .iter()
            .map(|(name, value)| {
                let value = value.to_str().unwrap_or("<binary>");
                if name.as_str().eq_ignore_ascii_case("authorization") {
                    format!("{name}: <redacted>")
                } else {
                    format!("{name}: {value}")
                }
            })
            .collect::<Vec<_>>()
            .join(" | ");
        eprintln!("[codex-proxy] incoming headers: {header_summary}");
    }

    let raw_body = match to_bytes(body, REQUEST_BODY_LIMIT).await {
        Ok(bytes) => bytes,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, format!("读取请求体失败：{error}")),
    };
    let raw_body = match decode_request_body(&incoming_headers, raw_body) {
        Ok(bytes) => bytes,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };
    let mut body: Value = match serde_json::from_slice(&raw_body) {
        Ok(value) => value,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("请求体不是合法 JSON：{error}"),
            )
        }
    };

    let compact = CompactRequestInfo::detect(&body, explicit_compact_endpoint);

    #[cfg(debug_assertions)]
    log_responses_request(parts.uri.path(), &body, &compact);

    #[cfg(debug_assertions)]
    if compact.is_compact {
        log_compact_request(parts.uri.path(), &body, &compact);
    }

    if compact.is_compact {
        // The Chat upstream receives the compact request as a normal context
        // transformation. Do not add a synthetic user prompt: the upstream
        // must see the complete input window supplied by Codex.
        if let Some(object) = body.as_object_mut() {
            object.remove("previous_response_id");
        }
    }

    // 多轮工具调用恢复：Codex 第二轮只发 previous_response_id + function_call_output，
    // Chat 上游要求 tool result 前紧跟带原始 function_call 的 assistant 消息。
    state.history.enrich_request(&mut body).await;

    let tool_context = build_codex_tool_context_from_request(&body);
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| state.default_model.clone());
    let outbound_model = apply_chat_upstream_model(&mut body, &state);
    let explicit_prompt_cache_key = body
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(ToString::to_string);
    let inferred_reasoning_config = super::reasoning::infer_reasoning_config_for_provider(
        &state.provider_name,
        &state.upstream_base_url,
        &outbound_model,
    );
    let reasoning_config = state
        .reasoning_override
        .as_ref()
        .or(inferred_reasoning_config.as_ref());
    let mapped = match if compact.is_compact {
        responses_to_chat_compaction(body, reasoning_config)
    } else {
        responses_to_chat_completions_with_reasoning(body, reasoning_config)
    } {
        Ok(mapped) => mapped,
        Err(error) => {
            return error_response(StatusCode::UNPROCESSABLE_ENTITY, error.to_string())
        }
    };

    let client_session_id = incoming_headers
        .get("x-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut mapped = mapped;
    if should_send_prompt_cache_key(&state.prompt_cache_routing, &state.upstream_base_url) {
        if let Some(key) = explicit_prompt_cache_key.as_deref().or(client_session_id) {
            mapped["prompt_cache_key"] = Value::String(key.to_string());
        }
    }

    #[cfg(debug_assertions)]
    if compact.is_compact {
        log_compact_chat_request(&mapped, &compact);
    }

    let query = parts
        .uri
        .query()
        .map(ToString::to_string)
        .filter(|query| !query.is_empty());
    let url = chat_completions_url(&state.upstream_base_url, query.as_deref());
    let mut builder = state.http.post(&url).headers(build_forward_headers(&incoming_headers));
    builder = builder
        .header(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .header(header::ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    // 用注册的 API key 覆盖 Authorization：客户端（桌面端）发出的可能是登录态
    // token 或缺失，而启动器持有正确 key。
    if let Some(api_key) = state.api_key.as_deref() {
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {api_key}")) {
            builder = builder.header(header::AUTHORIZATION, value);
        }
    }
    builder = builder.body(serde_json::to_vec(&mapped).unwrap_or_default());

    let upstream = match tokio::time::timeout(UPSTREAM_RESPONSE_HEADERS_TIMEOUT, builder.send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                format!("连接上游失败：{error}"),
            )
        }
        Err(_) => return response_body_error("等待上游响应头超时"),
    };

    let status = upstream.status();
    #[cfg(debug_assertions)]
    if compact.is_compact && compact_debug_enabled() {
        let content_type = upstream
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("-");
        compact_debug_emit(format!(
            "[codex-proxy][compact-debug] upstream-response status={status} content_type={content_type}"
        ));
    }
    if !status.is_success() {
        let upstream_headers = upstream.headers().clone();
        #[cfg(debug_assertions)]
        eprintln!(
            "[codex-proxy] upstream {status} for {url}; response headers: {}",
            upstream
                .headers()
                .iter()
                .map(|(name, value)| format!("{name}: {}", value.to_str().unwrap_or("<binary>")))
                .collect::<Vec<_>>()
                .join(" | ")
        );
        let response_encoding = get_content_encoding(&upstream_headers);
        let body_bytes = match read_response_body(upstream, UPSTREAM_BODY_TIMEOUT).await {
            Ok(bytes) => bytes,
            Err(error) => {
                return response_body_error(format!("读取上游错误响应失败：{error}"))
            }
        };
        let body_bytes = match response_encoding {
            Some(encoding) => match decode_response_body(&encoding, &body_bytes) {
                Ok(bytes) => bytes,
                Err(error) => return response_body_error(format!("解压上游错误响应失败：{error}")),
            },
            None => body_bytes,
        };
        // 上游错误体归一化为 Responses 风格，保留状态码；非 JSON 文本也透传给
        // 用户排查（否则 403/401 只剩占位文案）。
        let error_body: Option<Value> = serde_json::from_slice(&body_bytes).ok();
        let mut responses_error = match error_body {
            Some(value) => chat_error_to_response_error(Some(&value)),
            None => {
                let text = String::from_utf8_lossy(&body_bytes).trim().to_string();
                if text.is_empty() {
                    chat_error_to_response_error(None)
                } else {
                    json!({
                        "error": {
                            "message": text,
                            "type": "upstream_error",
                            "code": Value::Null,
                            "param": Value::Null,
                        }
                    })
                }
            }
        };
        annotate_upstream_error(
            &mut responses_error,
            &state.provider_name,
            &requested_model,
            "chat/completions",
            status,
        );
        truncate_error_strings(&mut responses_error);
        let headers = rebuilt_response_headers(&upstream_headers, "application/json");
        return (
            status,
            headers,
            Body::from(serde_json::to_vec(&responses_error).unwrap_or_default()),
        )
            .into_response();
    }

    let upstream_is_sse = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(is_sse_content_type);
    let request_is_stream = mapped.get("stream").and_then(Value::as_bool).unwrap_or(false);

    if upstream_is_sse {
        let upstream_headers = upstream.headers().clone();
        let response_encoding = get_content_encoding(&upstream_headers);
        let stream: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>> =
            if let Some(encoding) = response_encoding {
                let raw = match read_response_body(upstream, UPSTREAM_BODY_TIMEOUT).await {
                    Ok(bytes) => bytes,
                    Err(error) => return response_body_error(format!("读取上游流响应失败：{error}")),
                };
                let decoded = match decode_response_body(&encoding, &raw) {
                    Ok(bytes) => bytes,
                    Err(error) => return response_body_error(format!("解压上游流响应失败：{error}")),
                };
                Box::pin(stream::once(async move { Ok(decoded) }))
            } else {
                let primed = match prime_stream_with_timeout(
                    upstream.bytes_stream(),
                    UPSTREAM_STREAM_FIRST_BYTE_TIMEOUT,
                )
                .await
                {
                    Ok(stream) => stream,
                    Err(message) => return response_body_error(message),
                };
                Box::pin(stream_with_idle_timeout(primed))
            };
        let sse_stream = create_responses_sse_stream_from_chat_with_context(stream, tool_context);
        let sse_stream = super::history::record_responses_sse_stream(
            sse_stream,
            state.history.clone(),
        );

        // Headers were copied before creating the stream, so the proxy can
        // expose retry/request diagnostics without forwarding entity headers.
        let mut headers = rebuilt_response_headers(&upstream_headers, "text/event-stream");
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        );
        return (headers, Body::from_stream(sse_stream)).into_response();
    }

    let upstream_headers = upstream.headers().clone();
    let response_encoding = get_content_encoding(&upstream_headers);
    let body_bytes = match read_response_body(upstream, UPSTREAM_BODY_TIMEOUT).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return response_body_error(format!("读取上游响应失败：{error}"))
        }
    };
    let body_bytes = match response_encoding {
        Some(encoding) => match decode_response_body(&encoding, &body_bytes) {
            Ok(bytes) => bytes,
            Err(error) => return response_body_error(format!("解压上游响应失败：{error}")),
        },
        None => body_bytes,
    };
    if request_is_stream && body_looks_like_sse(&body_bytes) {
        let stream = stream::once(async move { Ok::<Bytes, std::io::Error>(body_bytes) });
        let sse_stream = create_responses_sse_stream_from_chat_with_context(stream, tool_context);
        let sse_stream = super::history::record_responses_sse_stream(
            sse_stream,
            state.history.clone(),
        );
        let mut headers = rebuilt_response_headers(&upstream_headers, "text/event-stream");
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        return (headers, Body::from_stream(sse_stream)).into_response();
    }
    let chat_response: Value = match serde_json::from_slice(&body_bytes) {
        Ok(value) => value,
        Err(error) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("上游响应不是合法 JSON：{error}"),
            )
        }
    };
    let responses_response = match chat_completion_to_response_with_context(
        chat_response,
        &tool_context,
    ) {
        Ok(value) => value,
        Err(error) => {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("上游响应转换失败：{error}"),
            )
        }
    };
    #[cfg(debug_assertions)]
    if compact.is_compact {
        log_compact_responses_body(&responses_response);
    }
    state.history.record_response(&responses_response).await;

    let headers = rebuilt_response_headers(&upstream_headers, "application/json");
    (
        headers,
        Body::from(serde_json::to_vec(&responses_response).unwrap_or_default()),
    )
        .into_response()
}

/// `/v1/models` passthrough so the Codex client can list upstream models.
async fn handle_models(State(state): State<Arc<ProxyState>>, request: Request) -> Response {
    let (parts, _body) = request.into_parts();
    let url = models_url(&state.upstream_base_url, parts.uri.query());
    let mut builder = state
        .http
        .get(&url)
        .headers(build_forward_headers(&parts.headers));
    builder = builder.header(header::ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    if let Some(api_key) = state.api_key.as_deref() {
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {api_key}")) {
            builder = builder.header(header::AUTHORIZATION, value);
        }
    }

    match tokio::time::timeout(UPSTREAM_RESPONSE_HEADERS_TIMEOUT, builder.send()).await {
        Ok(Ok(response)) => {
            let status = response.status();
            let upstream_headers = response.headers().clone();
            let encoding = get_content_encoding(&upstream_headers);
            let bytes = match read_response_body(response, UPSTREAM_BODY_TIMEOUT).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    return response_body_error(format!("读取上游 models 响应失败：{error}"))
                }
            };
            let bytes = match encoding {
                Some(encoding) => match decode_response_body(&encoding, &bytes) {
                    Ok(bytes) => bytes,
                    Err(error) => return response_body_error(format!("解压上游 models 响应失败：{error}")),
                },
                None => bytes,
            };
            let bytes = normalize_models_response(&bytes);
            let headers = rebuilt_response_headers(&upstream_headers, "application/json");
            (status, headers, Body::from(bytes)).into_response()
        }
        Ok(Err(error)) => error_response(StatusCode::BAD_GATEWAY, format!("连接上游失败：{error}")),
        Err(_) => response_body_error("等待上游 models 响应头超时"),
    }
}

/// Codex 客户端可能对请求体启用 zstd 压缩，解析前解压并剥掉已失真的实体头
/// （content-encoding / content-length / transfer-encoding）——转发层会基于
/// 解压后的明文 JSON 重新生成正确的头。
fn decode_request_body(headers: &HeaderMap, body_bytes: Bytes) -> Result<Bytes, String> {
    let Some(encoding) = get_content_encoding(headers) else {
        return Ok(body_bytes);
    };

    if !is_supported_content_encoding(&encoding) {
        return Err(format!("不支持的请求 content-encoding：{encoding}"));
    }

    let decompressed = decompress_body_with_limit(&encoding, &body_bytes, REQUEST_BODY_LIMIT)
        .map_err(|error| format!("请求体解压失败（{encoding}）：{error}"))?;
    match decompressed {
        Some(decompressed) => Ok(Bytes::from(decompressed)),
        None => Err(format!("不支持的请求 content-encoding：{encoding}")),
    }
}

async fn read_response_body(
    response: reqwest::Response,
    timeout: Duration,
) -> Result<Bytes, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BODY_BYTES as u64)
    {
        return Err(format!("上游响应体超过 {} 字节上限", MAX_RESPONSE_BODY_BYTES));
    }

    let mut stream = response.bytes_stream();
    let read = async move {
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| error.to_string())?;
            if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BODY_BYTES {
                return Err(format!("上游响应体超过 {} 字节上限", MAX_RESPONSE_BODY_BYTES));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Bytes::from(body))
    };
    tokio::time::timeout(timeout, read)
        .await
        .map_err(|_| format!("读取上游响应体超时（{} 秒）", timeout.as_secs()))?
}

fn decode_response_body(encoding: &str, body: &[u8]) -> Result<Bytes, String> {
    if !is_supported_content_encoding(encoding) {
        return Err(format!("不支持的上游 content-encoding：{encoding}"));
    }
    match decompress_body_with_limit(encoding, body, MAX_RESPONSE_BODY_BYTES) {
        Ok(Some(bytes)) => Ok(Bytes::from(bytes)),
        Ok(None) => Err(format!("不支持的上游 content-encoding：{encoding}")),
        Err(DecompressError::TooLarge { limit }) => {
            Err(format!("上游解压响应体超过 {limit} 字节上限"))
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Wait for the first non-empty upstream stream chunk before committing the
/// downstream SSE response, then replay that chunk to the normal converter.
///
/// `reqwest::Response::send()` only waits for response headers. This helper
/// closes the gap for uncompressed SSE without losing the chunk consumed by
/// the priming read.
async fn prime_stream_with_timeout<S, E>(
    stream: S,
    timeout: Duration,
) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>, String>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    let mut stream = Box::pin(stream);
    let first_chunk = async {
        loop {
            match stream.next().await {
                Some(Ok(bytes)) if !bytes.is_empty() => return Ok(Some(bytes)),
                Some(Ok(_)) => {}
                Some(Err(error)) => {
                    return Err(format!("读取上游流式首包失败：{error}"));
                }
                None => return Ok(None),
            }
        }
    };

    let first = if timeout.is_zero() {
        first_chunk.await
    } else {
        match tokio::time::timeout(timeout, first_chunk).await {
            Ok(result) => result,
            Err(_) => return Err(format!("等待上游流式首包超时（{} 秒）", timeout.as_secs())),
        }
    }?;

    let Some(first) = first else {
        return Err("上游流式响应在首包到达前结束".to_string());
    };

    let replay = stream::once(async move { Ok::<Bytes, std::io::Error>(first) }).chain(
        stream.map(|result| result.map_err(|error| std::io::Error::other(error.to_string()))),
    );
    Ok(Box::pin(replay))
}

fn stream_with_idle_timeout<E>(
    stream: impl Stream<Item = Result<Bytes, E>> + Send + 'static,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    E: std::error::Error + Send + Sync + 'static,
{
    async_stream::stream! {
        tokio::pin!(stream);
        loop {
            match tokio::time::timeout(UPSTREAM_STREAM_IDLE_TIMEOUT, stream.next()).await {
                Ok(Some(Ok(bytes))) => yield Ok(bytes),
                Ok(Some(Err(error))) => yield Err(std::io::Error::other(error.to_string())),
                Ok(None) => break,
                Err(_) => {
                    yield Err(std::io::Error::other(format!(
                        "上游流式响应空闲超时（{} 秒）",
                        UPSTREAM_STREAM_IDLE_TIMEOUT.as_secs()
                    )));
                    break;
                }
            }
        }
    }
}

fn normalize_models_response(bytes: &[u8]) -> Bytes {
    let Ok(mut value) = serde_json::from_slice::<Value>(bytes) else {
        return Bytes::copy_from_slice(bytes);
    };
    if value.get("data").is_none() {
        if let Some(models) = value.get("models").cloned() {
            if let Some(object) = value.as_object_mut() {
                object.remove("models");
                object.insert("object".to_string(), Value::String("list".to_string()));
                object.insert("data".to_string(), models);
            }
        }
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .unwrap_or_else(|_| Bytes::copy_from_slice(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;
    use std::io::Write;
    use std::net::TcpListener as StdTcpListener;
    use std::sync::{Mutex, OnceLock};

    // ------------------------------------------------------------------
    // Integration tests: mock upstream + real proxy server over HTTP
    // ------------------------------------------------------------------

    struct MockUpstream {
        origin: String,
        handle: tokio::task::JoinHandle<()>,
    }

    async fn start_mock_upstream(chat_router: Router) -> MockUpstream {
        let listener = StdTcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/v1/models", get(mock_models))
            .nest("/v1", chat_router);
        let handle = tokio::spawn(async move {
            listener.set_nonblocking(true).unwrap();
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let _ = axum::serve(listener, router).await;
        });
        MockUpstream {
            origin: format!("http://127.0.0.1:{}", addr.port()),
            handle,
        }
    }

    impl Drop for MockUpstream {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    async fn start_proxy_for(upstream_origin: &str) -> (String, tokio::task::JoinHandle<()>) {
        let listener = StdTcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(ProxyState {
            upstream_base_url: format!("{upstream_origin}/v1"),
            api_key: None,
            provider_name: "Test Provider".to_string(),
            default_model: "kimi-k2".to_string(),
            chat_upstream_model: None,
            catalog_model_ids: Vec::new(),
            prompt_cache_routing: "auto".to_string(),
            reasoning_override: None,
            history: Arc::new(CodexChatHistoryStore::default()),
            http: reqwest::Client::new(),
        });
        let router = router(state);
        let handle = tokio::spawn(async move {
            listener.set_nonblocking(true).unwrap();
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let _ = axum::serve(listener, router).await;
        });
        (format!("http://127.0.0.1:{}", addr.port()), handle)
    }

    async fn mock_chat_sse(request: Request) -> Response {
        let _ = request;
        let sse = concat!(
            "data: {\"id\":\"chatcmpl-1\",\"model\":\"kimi-k2\",\"created\":123,\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"thinking\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":3,\"total_tokens\":13}}\n\n",
            "data: [DONE]\n\n",
        );
        (
            [(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"))],
            Body::from(sse),
        )
            .into_response()
    }

    async fn mock_chat_json(request: Request) -> Response {
        let _ = request;
        (StatusCode::OK, Body::from(chat_json_bytes())).into_response()
    }

    async fn mock_chat_sse_unmarked(request: Request) -> Response {
        let _ = request;
        let sse = concat!(
            "data: {\"id\":\"chatcmpl-unmarked\",\"model\":\"kimi-k2\",\"created\":123,\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl-unmarked\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"unmarked\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n",
            "data: [DONE]\n\n",
        );
        (StatusCode::OK, Body::from(sse)).into_response()
    }

    async fn mock_chat_capture_with_sse(request: Request) -> Response {
        let body = to_bytes(request.into_body(), REQUEST_BODY_LIMIT)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        *CAPTURED_CHAT_REQUEST
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = Some(value);
        mock_chat_sse(
            Request::builder()
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    fn chat_json_bytes() -> Vec<u8> {
        let body = json!({
            "id": "chatcmpl-2",
            "model": "kimi-k2",
            "created": 456,
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "plain answer"
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
        });
        serde_json::to_vec(&body).unwrap()
    }

    fn gzip_bytes(data: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        );
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    async fn mock_chat_gzip_json(request: Request) -> Response {
        let _ = request;
        (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, HeaderValue::from_static("application/json")),
                (header::CONTENT_ENCODING, HeaderValue::from_static("gzip")),
            ],
            Body::from(gzip_bytes(&chat_json_bytes())),
        )
            .into_response()
    }

    async fn mock_chat_zstd_json(request: Request) -> Response {
        let _ = request;
        let encoded = zstd::stream::encode_all(std::io::Cursor::new(chat_json_bytes()), 3).unwrap();
        (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, HeaderValue::from_static("application/json")),
                (header::CONTENT_ENCODING, HeaderValue::from_static("zstd")),
            ],
            Body::from(encoded),
        )
            .into_response()
    }

    static CAPTURED_CHAT_REQUEST: OnceLock<Mutex<Option<Value>>> = OnceLock::new();
    static CAPTURED_CHAT_TEST_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

    async fn captured_chat_test_guard() -> tokio::sync::MutexGuard<'static, ()> {
        CAPTURED_CHAT_TEST_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await
    }

    async fn mock_chat_capture(request: Request) -> Response {
        let body = to_bytes(request.into_body(), REQUEST_BODY_LIMIT)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        *CAPTURED_CHAT_REQUEST
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap() = Some(value);
        mock_chat_json(
            Request::builder()
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    fn take_captured_chat_request() -> Value {
        CAPTURED_CHAT_REQUEST
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap()
            .take()
            .expect("mock upstream did not receive a Chat request")
    }

    #[test]
    fn compact_metadata_detection_accepts_codex_turn_metadata() {
        let body = json!({
            "client_metadata": {
                "x-codex-turn-metadata": "{\"request_kind\":\"compaction\",\"compaction\":{\"trigger\":\"manual\",\"reason\":\"user_requested\",\"implementation\":\"responses\",\"phase\":\"standalone_turn\",\"strategy\":\"memento\"}}"
            }
        });

        let compact = CompactRequestInfo::detect(&body, false);

        assert!(compact.is_compact);
        assert_eq!(compact.source, "metadata");
        assert_eq!(compact.client_metadata, "object");
        assert_eq!(compact.turn_metadata, "valid");
        assert_eq!(compact.request_kind, "compaction");
        assert_eq!(compact.trigger, "manual");
        assert_eq!(compact.reason, "user_requested");
        assert_eq!(compact.implementation, "responses");
        assert_eq!(compact.phase, "standalone_turn");
        assert_eq!(compact.strategy, "memento");
    }

    #[test]
    fn ordinary_responses_request_is_not_compact_without_matching_metadata() {
        let body = json!({
            "client_metadata": {
                "x-codex-turn-metadata": "{\"request_kind\":\"user_input\"}"
            }
        });

        let compact = CompactRequestInfo::detect(&body, false);

        assert!(!compact.is_compact);
        assert_eq!(compact.source, "none");
        assert_eq!(compact.request_kind, "other");
    }

    #[test]
    fn malformed_compact_metadata_does_not_promote_ordinary_request() {
        let body = json!({
            "client_metadata": {
                "x-codex-turn-metadata": "not-json"
            }
        });

        let compact = CompactRequestInfo::detect(&body, false);

        assert!(!compact.is_compact);
        assert_eq!(compact.client_metadata, "object");
        assert_eq!(compact.turn_metadata, "invalid-json");
        assert_eq!(compact.request_kind, "absent");
    }

    async fn mock_chat_unauthorized(request: Request) -> Response {
        let _ = request;
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("7"));
        headers.insert("x-ratelimit-limit-requests", HeaderValue::from_static("100"));
        headers.insert("x-ratelimit-remaining-requests", HeaderValue::from_static("0"));
        headers.insert("x-ratelimit-reset-requests", HeaderValue::from_static("2s"));
        headers.insert("x-request-id", HeaderValue::from_static("req_upstream_1"));
        (
            StatusCode::UNAUTHORIZED,
            headers,
            Body::from(r#"{"error":{"message":"bad key","type":"invalid_request_error","code":"invalid_api_key","param":null}}"#),
        )
            .into_response()
    }

    async fn mock_models(request: Request) -> Response {
        let _ = request;
        let body = json!({"models": [{"id": "kimi-k2", "object": "model"}]});
        (StatusCode::OK, Body::from(serde_json::to_vec(&body).unwrap())).into_response()
    }

    fn mock_chat_router(handler: axum::routing::MethodRouter) -> Router {
        Router::new().route("/chat/completions", handler)
    }

    #[tokio::test]
    async fn prime_stream_replays_first_non_empty_chunk() {
        let upstream = stream::iter(vec![
            Ok::<Bytes, std::io::Error>(Bytes::new()),
            Ok(Bytes::from_static(b"first")),
            Ok(Bytes::from_static(b"second")),
        ]);

        let primed = prime_stream_with_timeout(upstream, Duration::from_secs(1))
            .await
            .expect("stream should be primed");
        let mut primed = primed;
        let mut chunks = Vec::new();
        while let Some(chunk) = primed.next().await {
            chunks.push(chunk.unwrap());
        }

        assert_eq!(chunks, vec![Bytes::from_static(b"first"), Bytes::from_static(b"second")]);
    }

    #[tokio::test]
    async fn prime_stream_times_out_after_response_headers_without_body() {
        let upstream = stream::once(async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<Bytes, std::io::Error>(Bytes::from_static(b"late"))
        });

        let error = match prime_stream_with_timeout(upstream, Duration::from_millis(5)).await {
            Ok(_) => panic!("stream should time out before its first chunk"),
            Err(error) => error,
        };

        assert!(error.contains("首包超时"), "{error}");
    }

    #[tokio::test]
    async fn streaming_conversion_produces_responses_sse() {
        let upstream = start_mock_upstream(mock_chat_router(post(mock_chat_sse))).await;
        let (proxy_origin, _proxy_handle) = start_proxy_for(&upstream.origin).await;
        let url = format!("{proxy_origin}/v1/responses");

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header(header::AUTHORIZATION, "Bearer test-key")
            .body(serde_json::to_vec(&json!({
                "model": "kimi-k2",
                "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}],
                "stream": true
            }))
            .unwrap())
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(content_type.contains("text/event-stream"), "{content_type}");

        let text = response.text().await.unwrap();
        assert!(text.contains("event: response.created"), "{text}");
        assert!(text.contains("event: response.output_item.added"));
        assert!(text.contains("event: response.reasoning_summary_text.delta"));
        assert!(text.contains("\"delta\":\"thinking\""));
        assert!(text.contains("event: response.output_text.delta"));
        assert!(text.contains("\"delta\":\"Hello\""));
        assert!(text.contains("\"delta\":\" world\""));
        assert!(text.contains("event: response.completed"));
        assert!(text.contains("\"input_tokens\":10"));
        assert!(text.contains("\"output_tokens\":3"));
        assert!(!text.contains("event: response.failed"), "{text}");
    }

    #[tokio::test]
    async fn streaming_conversion_injects_stream_options_include_usage() {
        let upstream = start_mock_upstream(mock_chat_router(post(mock_chat_sse))).await;
        let (proxy_origin, _proxy_handle) = start_proxy_for(&upstream.origin).await;
        let url = format!("{proxy_origin}/v1/responses");

        let response = reqwest::Client::new()
            .post(&url)
            .json(&json!({
                "model": "kimi-k2",
                "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}],
                "stream": true
            }))
            .send()
            .await
            .unwrap();
        let text = response.text().await.unwrap();
        assert!(text.contains("event: response.completed"), "{text}");
    }

    #[tokio::test]
    async fn unmarked_sse_stream_request_is_converted_to_responses_sse() {
        let upstream = start_mock_upstream(mock_chat_router(post(mock_chat_sse_unmarked))).await;
        let (proxy_origin, _proxy_handle) = start_proxy_for(&upstream.origin).await;
        let response = reqwest::Client::new()
            .post(format!("{proxy_origin}/v1/responses"))
            .json(&json!({
                "model": "kimi-k2",
                "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}],
                "stream": true
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/event-stream")));
        let body = response.text().await.unwrap();
        assert!(body.contains("event: response.output_text.delta"), "{body}");
        assert!(body.contains("unmarked"), "{body}");
    }

    #[tokio::test]
    async fn chat_mapping_and_prompt_cache_key_are_sent_to_upstream() {
        let _capture_guard = captured_chat_test_guard().await;
        let upstream = start_mock_upstream(mock_chat_router(post(mock_chat_capture_with_sse))).await;
        let listener = StdTcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(ProxyState {
            upstream_base_url: format!("{}/v1", upstream.origin),
            api_key: None,
            provider_name: "Kimi".to_string(),
            default_model: "codex-alias".to_string(),
            chat_upstream_model: Some("kimi-k2.5".to_string()),
            catalog_model_ids: vec!["codex-catalog-model".to_string()],
            prompt_cache_routing: "enabled".to_string(),
            reasoning_override: None,
            history: Arc::new(CodexChatHistoryStore::default()),
            http: reqwest::Client::new(),
        });
        let router = router(state);
        let handle = tokio::spawn(async move {
            listener.set_nonblocking(true).unwrap();
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let _ = axum::serve(listener, router).await;
        });
        let response = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{}/v1/responses", addr.port()))
            .header("x-session-id", "session-123")
            .json(&json!({
                "model": "codex-alias",
                "prompt_cache_key": "explicit-key",
                "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}],
                "stream": true
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let _ = response.bytes().await.unwrap();
        let chat = take_captured_chat_request();
        assert_eq!(chat["model"], "kimi-k2.5");
        assert_eq!(chat["prompt_cache_key"], "explicit-key");
        handle.abort();
    }

    #[tokio::test]
    async fn non_streaming_conversion_produces_responses_json() {
        let upstream = start_mock_upstream(mock_chat_router(post(mock_chat_json))).await;
        let (proxy_origin, _proxy_handle) = start_proxy_for(&upstream.origin).await;
        let url = format!("{proxy_origin}/v1/responses");

        let response = reqwest::Client::new()
            .post(&url)
            .json(&json!({
                "model": "kimi-k2",
                "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}],
                "stream": false
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["object"], "response");
        assert_eq!(body["status"], "completed");
        assert_eq!(body["output"][0]["type"], "message");
        assert_eq!(body["output"][0]["content"][0]["text"], "plain answer");
        assert_eq!(body["usage"]["input_tokens"], 5);
        assert_eq!(body["usage"]["output_tokens"], 2);
    }

    #[tokio::test]
    async fn stream_request_falls_back_to_json_when_upstream_ignores_stream() {
        let upstream = start_mock_upstream(mock_chat_router(post(mock_chat_json))).await;
        let (proxy_origin, _proxy_handle) = start_proxy_for(&upstream.origin).await;
        let response = reqwest::Client::new()
            .post(format!("{proxy_origin}/v1/responses"))
            .json(&json!({
                "model": "kimi-k2",
                "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}],
                "stream": true
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("application/json")));
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["object"], "response");
        assert_eq!(body["output"][0]["content"][0]["text"], "plain answer");
    }

    #[tokio::test]
    async fn gzip_upstream_response_is_decoded_before_conversion() {
        let upstream = start_mock_upstream(mock_chat_router(post(mock_chat_gzip_json))).await;
        let (proxy_origin, _proxy_handle) = start_proxy_for(&upstream.origin).await;
        let response = reqwest::Client::new()
            .post(format!("{proxy_origin}/v1/responses"))
            .json(&json!({
                "model": "kimi-k2",
                "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}]
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["output"][0]["content"][0]["text"], "plain answer");
    }

    #[tokio::test]
    async fn zstd_upstream_response_is_decoded_before_conversion() {
        let upstream = start_mock_upstream(mock_chat_router(post(mock_chat_zstd_json))).await;
        let (proxy_origin, _proxy_handle) = start_proxy_for(&upstream.origin).await;
        let response = reqwest::Client::new()
            .post(format!("{proxy_origin}/v1/responses"))
            .json(&json!({
                "model": "kimi-k2",
                "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}]
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["output"][0]["content"][0]["text"], "plain answer");
    }

    #[tokio::test]
    async fn compact_routes_use_chat_fallback_and_round_trip() {
        let _capture_guard = captured_chat_test_guard().await;
        let upstream = start_mock_upstream(mock_chat_router(post(mock_chat_capture))).await;
        let (proxy_origin, _proxy_handle) = start_proxy_for(&upstream.origin).await;
        let client = reqwest::Client::new();

        for path in ["/responses/compact", "/v1/responses/compact"] {
            let response = client
                .post(format!("{proxy_origin}{path}"))
                .json(&json!({
                    "model": "kimi-k2",
                    "instructions": "Keep the task context.",
                    "input": [{
                        "role": "user",
                        "content": [{"type": "input_text", "text": "compact me"}]
                    }, {
                        "type": "tool_search_output",
                        "call_id": "search_1",
                        "tools": [{
                            "type": "function",
                            "name": "reconstructed_tool",
                            "parameters": {"type": "object"}
                        }]
                    }],
                    "tools": [{
                        "type": "function",
                        "name": "should_not_be_forwarded",
                        "parameters": {"type": "object"}
                    }],
                    "tool_choice": "auto",
                    "parallel_tool_calls": true,
                    "previous_response_id": "resp_old"
                }))
                .send()
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK, "path={path}");
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["object"], "response", "path={path}");
            assert_eq!(body["output"][0]["content"][0]["text"], "plain answer");

            let chat = take_captured_chat_request();
            assert_eq!(chat["messages"][0]["role"], "system");
            assert!(chat["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("context compaction service"));
            assert!(chat.get("tools").is_none());
            assert!(chat.get("tool_choice").is_none());
            assert!(chat.get("parallel_tool_calls").is_none());
            assert!(!chat["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["content"] == "Keep the task context."));

            // The normal Responses mapper must accept the fallback output as
            // an assistant message in the next stateless context window.
            let response_output = body["output"].clone();
            let follow_up = client
                .post(format!("{proxy_origin}/v1/responses"))
                .json(&json!({
                    "model": "kimi-k2",
                    "input": [
                        response_output[0].clone(),
                        {"role": "user", "content": [{"type": "input_text", "text": "continue"}]}
                    ]
                }))
                .send()
                .await
                .unwrap();
            assert_eq!(follow_up.status(), StatusCode::OK);
            let follow_up_chat = take_captured_chat_request();
            assert!(follow_up_chat["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["role"] == "assistant"
                    && message["content"] == "plain answer"));
        }
    }

    #[tokio::test]
    async fn compact_metadata_on_responses_route_uses_chat_fallback() {
        let _capture_guard = captured_chat_test_guard().await;
        let upstream = start_mock_upstream(mock_chat_router(post(mock_chat_capture))).await;
        let (proxy_origin, _proxy_handle) = start_proxy_for(&upstream.origin).await;
        let response = reqwest::Client::new()
            .post(format!("{proxy_origin}/v1/responses"))
            .json(&json!({
                "model": "kimi-k2",
                "instructions": "Keep the task context.",
                "input": [{
                    "role": "user",
                    "content": [{"type": "input_text", "text": "compact me"}]
                }],
                "client_metadata": {
                    "x-codex-turn-metadata": "{\"request_kind\":\"compaction\",\"compaction\":{\"trigger\":\"manual\",\"reason\":\"user_requested\",\"implementation\":\"responses\",\"phase\":\"standalone_turn\",\"strategy\":\"memento\"}}"
                },
                "tools": [{
                    "type": "function",
                    "name": "should_not_be_forwarded",
                    "parameters": {"type": "object"}
                }],
                "tool_choice": "auto",
                "parallel_tool_calls": true,
                "previous_response_id": "resp_old"
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["object"], "response");
        assert_eq!(body["output"][0]["content"][0]["text"], "plain answer");

        let chat = take_captured_chat_request();
        assert!(chat["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("context compaction service"));
        assert!(chat.get("tools").is_none());
        assert!(chat.get("tool_choice").is_none());
        assert!(chat.get("parallel_tool_calls").is_none());
    }

    #[tokio::test]
    async fn compact_streaming_uses_chat_fallback_and_responses_sse() {
        let upstream = start_mock_upstream(mock_chat_router(post(mock_chat_sse))).await;
        let (proxy_origin, _proxy_handle) = start_proxy_for(&upstream.origin).await;
        let response = reqwest::Client::new()
            .post(format!("{proxy_origin}/v1/responses/compact"))
            .json(&json!({
                "model": "kimi-k2",
                "input": [{
                    "role": "user",
                    "content": [{"type": "input_text", "text": "compact me"}]
                }],
                "stream": true
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/event-stream")));
        let text = response.text().await.unwrap();
        assert!(text.contains("event: response.completed"), "{text}");
        assert!(!text.contains("event: response.failed"), "{text}");
    }

    #[tokio::test]
    async fn upstream_error_keeps_status_and_normalizes_body() {
        let upstream = start_mock_upstream(mock_chat_router(post(mock_chat_unauthorized))).await;
        let (proxy_origin, _proxy_handle) = start_proxy_for(&upstream.origin).await;
        let url = format!("{proxy_origin}/v1/responses");

        let response = reqwest::Client::new()
            .post(&url)
            .json(&json!({
                "model": "kimi-k2",
                "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}]
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let retry_after = response.headers().get("retry-after").cloned();
        let rate_limit_remaining = response
            .headers()
            .get("x-ratelimit-remaining-requests")
            .cloned();
        let request_id = response.headers().get("x-request-id").cloned();
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"]["message"], "bad key");
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert_eq!(body["error"]["code"], "invalid_api_key");
        assert_eq!(body["error"]["provider"], "Test Provider");
        assert_eq!(body["error"]["model"], "kimi-k2");
        assert_eq!(body["error"]["endpoint"], "chat/completions");
        assert_eq!(body["error"]["upstream_status"], 401);
        assert_eq!(retry_after.as_ref().unwrap(), "7");
        assert_eq!(rate_limit_remaining.as_ref().unwrap(), "0");
        assert_eq!(request_id.as_ref().unwrap(), "req_upstream_1");
    }

    #[tokio::test]
    async fn models_passthrough_forwards_auth() {
        let upstream = start_mock_upstream(Router::new()).await;
        let (proxy_origin, _proxy_handle) = start_proxy_for(&upstream.origin).await;
        let url = format!("{proxy_origin}/v1/models");

        let response = reqwest::Client::new()
            .get(&url)
            .header(header::AUTHORIZATION, "Bearer forwarded")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["data"][0]["id"], "kimi-k2");
    }

    #[test]
    fn chat_completions_url_handles_base_variants() {
        assert_eq!(
            chat_completions_url("https://api.example.com/v1", None),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://api.example.com/v1/", None),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://api.example.com/chat/completions", None),
            "https://api.example.com/chat/completions"
        );
    }

    #[test]
    fn chat_completions_url_preserves_and_merges_query() {
        assert_eq!(
            chat_completions_url("https://api.example.com/v1", Some("include=usage")),
            "https://api.example.com/v1/chat/completions?include=usage"
        );
        assert_eq!(
            chat_completions_url("https://api.example.com/v1?key=abc", Some("include=usage")),
            "https://api.example.com/v1/chat/completions?key=abc&include=usage"
        );
        assert_eq!(
            chat_completions_url(
                "https://api.example.com/v1/chat/completions?key=abc",
                None,
            ),
            "https://api.example.com/v1/chat/completions?key=abc"
        );
        assert_eq!(
            chat_completions_url("https://api.example.com/v1", Some("")),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn models_url_handles_base_variants() {
        assert_eq!(
            models_url("https://api.example.com/v1", None),
            "https://api.example.com/v1/models"
        );
        assert_eq!(
            models_url("https://api.example.com/v1/models?key=abc", None),
            "https://api.example.com/v1/models?key=abc"
        );
        assert_eq!(
            models_url("https://api.example.com/v1?key=abc", Some("limit=10")),
            "https://api.example.com/v1/models?key=abc&limit=10"
        );
    }

    #[test]
    fn sse_content_type_is_case_insensitive_and_parameter_aware() {
        assert!(is_sse_content_type("text/event-stream"));
        assert!(is_sse_content_type("TEXT/EVENT-STREAM; charset=utf-8"));
        assert!(is_sse_content_type(" text/event-stream ; charset=UTF-8"));
        assert!(!is_sse_content_type("application/json"));
        assert!(!is_sse_content_type("text/event-streaming"));
    }

    #[test]
    fn unmarked_sse_body_sniff_accepts_sse_fields_but_not_json_or_text() {
        assert!(body_looks_like_sse(b"data: {\"id\":\"1\"}\n\n"));
        assert!(body_looks_like_sse(b"\xEF\xBB\xBF\n  event: message\n\n"));
        assert!(body_looks_like_sse(b": keep-alive\n\ndata: {}\n\n"));
        assert!(!body_looks_like_sse(br#"{"object":"chat.completion"}"#));
        assert!(!body_looks_like_sse(b"Bad Gateway"));
    }

    #[test]
    fn chat_model_mapping_preserves_catalog_model_and_maps_unknown_model() {
        let state = ProxyState {
            upstream_base_url: "https://api.kimi.com/coding".to_string(),
            api_key: None,
            provider_name: "Kimi".to_string(),
            default_model: "kimi-codex".to_string(),
            chat_upstream_model: Some("kimi-k2.5".to_string()),
            catalog_model_ids: vec!["kimi-codex".to_string()],
            prompt_cache_routing: "auto".to_string(),
            reasoning_override: None,
            history: Arc::new(CodexChatHistoryStore::default()),
            http: reqwest::Client::new(),
        };
        let mut catalog_body = json!({"model":"kimi-codex"});
        assert_eq!(apply_chat_upstream_model(&mut catalog_body, &state), "kimi-codex");
        assert_eq!(catalog_body["model"], "kimi-codex");

        let mut alias_body = json!({"model":"display-alias"});
        assert_eq!(apply_chat_upstream_model(&mut alias_body, &state), "kimi-k2.5");
        assert_eq!(alias_body["model"], "kimi-k2.5");

        let mut missing_body = json!({});
        assert_eq!(apply_chat_upstream_model(&mut missing_body, &state), "kimi-k2.5");
        assert_eq!(missing_body["model"], "kimi-k2.5");
    }

    #[test]
    fn prompt_cache_routing_matches_cc_switch_known_endpoint_policy() {
        assert!(should_send_prompt_cache_key("auto", "https://api.openai.com/v1"));
        assert!(should_send_prompt_cache_key("auto", "https://api.kimi.com/coding"));
        assert!(!should_send_prompt_cache_key("auto", "https://relay.example/v1"));
        assert!(should_send_prompt_cache_key("enabled", "https://relay.example/v1"));
        assert!(!should_send_prompt_cache_key("disabled", "https://api.openai.com/v1"));
    }

    #[test]
    fn forward_headers_drop_client_auth_and_entity_headers() {
        let mut incoming = HeaderMap::new();
        incoming.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer secret"));
        incoming.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        incoming.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("zstd"));
        incoming.insert(header::CONTENT_ENCODING, HeaderValue::from_static("zstd"));
        incoming.insert(header::HOST, HeaderValue::from_static("127.0.0.1:57118"));
        incoming.insert("x-session-id", HeaderValue::from_static("sess_1"));

        let forwarded = build_forward_headers(&incoming);
        assert!(forwarded.get(header::AUTHORIZATION).is_none());
        assert_eq!(forwarded.get("x-session-id").unwrap(), "sess_1");
        assert!(forwarded.get(header::CONTENT_TYPE).is_none());
        assert!(forwarded.get(header::ACCEPT_ENCODING).is_none());
        // 本机代理地址的 Host / 压缩编码头绝不能透传（上游会 400）。
        assert!(forwarded.get(header::HOST).is_none());
        assert!(forwarded.get(header::CONTENT_ENCODING).is_none());
    }

    #[tokio::test]
    async fn error_response_shapes_match_responses_protocol() {
        let response = error_response(StatusCode::BAD_GATEWAY, "boom");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let bytes = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["type"], "proxy_error");
        assert_eq!(body["error"]["message"], "boom");
    }

    #[test]
    fn truncating_error_strings_keeps_multibyte_text_valid() {
        let mut value = Value::String("界".repeat(MAX_ERROR_TEXT_BYTES));
        truncate_error_strings(&mut value);
        let text = value.as_str().unwrap();
        assert!(text.ends_with(ERROR_TRUNCATION_SUFFIX));
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
        assert!(text.len() <= MAX_ERROR_TEXT_BYTES + ERROR_TRUNCATION_SUFFIX.len());
    }
}
