//! Shared helpers for the Codex protocol conversion proxy.
//!
//! Ported from cc-switch `proxy/json_canonical.rs` and
//! `proxy/providers/codex_chat_common.rs` (kept verbatim).

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Canonical JSON helpers (from cc-switch json_canonical.rs)
// ---------------------------------------------------------------------------

#[allow(dead_code)] // ported from cc-switch; used by tests, kept for future callers
pub(crate) fn canonicalize_value(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize_value).collect()),
        Value::Object(map) => {
            let mut entries = map.into_iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));

            let mut sorted = serde_json::Map::new();
            for (key, value) in entries {
                sorted.insert(key, canonicalize_value(value));
            }
            Value::Object(sorted)
        }
        other => other,
    }
}

pub(crate) fn canonical_json_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value)
            .expect("serializing a JSON string for canonical output should not fail"),
        Value::Array(values) => {
            let parts = values.iter().map(canonical_json_string).collect::<Vec<_>>();
            format!("[{}]", parts.join(","))
        }
        Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(left, _)| *left);
            let parts = entries
                .into_iter()
                .map(|(key, value)| {
                    let key = serde_json::to_string(key).expect(
                        "serializing a JSON object key for canonical output should not fail",
                    );
                    format!("{key}:{}", canonical_json_string(value))
                })
                .collect::<Vec<_>>();
            format!("{{{}}}", parts.join(","))
        }
    }
}

pub(crate) fn canonicalize_json_string_if_parseable(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return value.to_string();
    }

    serde_json::from_str::<Value>(trimmed)
        .map(|parsed| canonical_json_string(&parsed))
        .unwrap_or_else(|_| value.to_string())
}

/// Normalize a tool-call `arguments` string into a valid JSON payload.
///
/// Identical to [`canonicalize_json_string_if_parseable`] except that an empty
/// (or whitespace-only) value is coerced to `"{}"` instead of being passed
/// through verbatim. A no-argument tool call must serialize as `"{}"`; strict
/// upstreams such as Minimax reject `arguments: ""` with a 400
/// `invalid function arguments json string` error, whereas lenient ones
/// (OpenAI, Kimi) silently treat it as an empty object.
pub(crate) fn canonicalize_tool_arguments_str(value: &str) -> String {
    if value.trim().is_empty() {
        return "{}".to_string();
    }
    canonicalize_json_string_if_parseable(value)
}

/// Normalize a tool-call `arguments` field from a Responses/Chat item.
///
/// Mirrors the inline `match` that several transform paths used to duplicate:
/// a string is canonicalized (with empty coerced to `"{}"`), a structured
/// value is serialized canonically, and a missing field defaults to `"{}"`.
pub(crate) fn canonicalize_tool_arguments(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => canonicalize_tool_arguments_str(s),
        Some(v) => canonical_json_string(v),
        None => "{}".to_string(),
    }
}

#[allow(dead_code)] // ported from cc-switch; used by tests, kept for future callers
pub(crate) fn short_value_hash(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "absent".to_string();
    };
    short_sha256_hex(canonical_json_string(value).as_bytes())
}

pub(crate) fn short_sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

// ---------------------------------------------------------------------------
// Reasoning / think helpers (from cc-switch codex_chat_common.rs)
// ---------------------------------------------------------------------------

const THINK_OPEN_TAG: &str = "<think>";
const THINK_CLOSE_TAG: &str = "</think>";

// 穷举上游可能的 reasoning 回传字段，优先级：reasoning_content > reasoning(字符串/对象) > reasoning_details。
// 不依赖 provider meta 的 outputFormat 声明，因此对各家 Chat 兼容接口都能兜底提取。
pub(crate) fn extract_reasoning_field_text(value: &Value) -> Option<String> {
    for key in ["reasoning_content", "reasoning"] {
        if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }

    if let Some(reasoning) = value.get("reasoning") {
        for key in ["content", "text", "summary"] {
            if let Some(text) = reasoning.get(key).and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }

    if let Some(details) = value.get("reasoning_details") {
        if let Some(text) = extract_reasoning_details_text(details) {
            return Some(text);
        }
    }

    None
}

fn extract_reasoning_details_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => (!text.is_empty()).then(|| text.to_string()),
        Value::Array(parts) => {
            let text = parts
                .iter()
                .filter_map(extract_reasoning_detail_part_text)
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            (!text.is_empty()).then_some(text)
        }
        Value::Object(_) => extract_reasoning_detail_part_text(value),
        _ => None,
    }
}

fn extract_reasoning_detail_part_text(value: &Value) -> Option<String> {
    for key in ["text", "content", "summary"] {
        if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }

    if let Some(parts) = value.get("parts").and_then(|v| v.as_array()) {
        let text = parts
            .iter()
            .filter_map(extract_reasoning_detail_part_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        return (!text.is_empty()).then_some(text);
    }

    None
}

pub(crate) fn extract_reasoning_summary_text(value: &Value) -> Option<String> {
    for key in ["reasoning_content", "content", "text"] {
        if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }

    if let Some(reasoning) = value.get("reasoning") {
        for key in ["content", "text", "summary"] {
            if let Some(text) = reasoning.get(key).and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }

    let summary = value.get("summary")?;
    if let Some(text) = summary.as_str() {
        return (!text.is_empty()).then(|| text.to_string());
    }

    let parts = summary.as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| {
            part.get("text")
                .and_then(|v| v.as_str())
                .or_else(|| part.get("content").and_then(|v| v.as_str()))
                .or_else(|| part.as_str())
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    (!text.is_empty()).then_some(text)
}

pub(crate) fn append_reasoning_content(message: &mut Map<String, Value>, reasoning: &str) -> bool {
    let reasoning = reasoning.trim();
    if reasoning.is_empty() {
        return false;
    }

    match message.get_mut("reasoning_content") {
        Some(Value::String(existing)) if !existing.is_empty() => {
            existing.push_str("\n\n");
            existing.push_str(reasoning);
        }
        _ => {
            message.insert(
                "reasoning_content".to_string(),
                Value::String(reasoning.to_string()),
            );
        }
    }
    true
}

pub(crate) fn attach_reasoning_content_field(item: &mut Value, reasoning: &str) -> bool {
    let reasoning = reasoning.trim();
    if reasoning.is_empty() {
        return false;
    }

    if let Some(obj) = item.as_object_mut() {
        obj.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning.to_string()),
        );
        return true;
    }

    false
}

pub(crate) fn attach_optional_reasoning_content_field(
    item: &mut Value,
    reasoning: Option<&str>,
) -> bool {
    let Some(reasoning) = reasoning else {
        return false;
    };
    attach_reasoning_content_field(item, reasoning)
}

pub(crate) fn response_function_call_item(
    item_id: &str,
    status: &str,
    call_id: &str,
    name: &str,
    arguments: &str,
    reasoning: Option<&str>,
) -> Value {
    let mut item = json!({
        "id": item_id,
        "type": "function_call",
        "status": status,
        "call_id": call_id,
        "name": name,
        "arguments": arguments
    });
    attach_optional_reasoning_content_field(&mut item, reasoning);
    item
}

pub(crate) fn response_function_call_item_with_namespace(
    item_id: &str,
    status: &str,
    call_id: &str,
    name: &str,
    namespace: Option<&str>,
    arguments: &str,
    reasoning: Option<&str>,
) -> Value {
    let mut item =
        response_function_call_item(item_id, status, call_id, name, arguments, reasoning);
    if let Some(namespace) = namespace.filter(|value| !value.is_empty()) {
        if let Some(obj) = item.as_object_mut() {
            obj.insert("namespace".to_string(), json!(namespace));
        }
    }
    item
}

pub(crate) fn response_item_call_id(item: &Value) -> Option<String> {
    item.get("call_id")
        .or_else(|| item.get("id"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub(crate) fn is_empty_value(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.trim().is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
        _ => false,
    }
}

pub(crate) fn split_leading_think_block(text: &str) -> Option<(String, String)> {
    let leading_ws_len = text.len() - text.trim_start().len();
    let after_ws = &text[leading_ws_len..];
    if !after_ws.starts_with(THINK_OPEN_TAG) {
        return None;
    }

    let body_start = leading_ws_len + THINK_OPEN_TAG.len();
    let close_relative = text[body_start..].find(THINK_CLOSE_TAG)?;
    let close_start = body_start + close_relative;
    let answer_start = close_start + THINK_CLOSE_TAG.len();

    Some((
        text[body_start..close_start].trim().to_string(),
        strip_think_answer_separator(&text[answer_start..]).to_string(),
    ))
}

pub(crate) fn strip_leading_think_open_tag(text: &str) -> Option<String> {
    let leading_ws_len = text.len() - text.trim_start().len();
    let after_ws = &text[leading_ws_len..];
    after_ws
        .strip_prefix(THINK_OPEN_TAG)
        .map(|value| value.trim().to_string())
}

fn strip_think_answer_separator(text: &str) -> &str {
    text.trim_start_matches(['\r', '\n', '\t', ' '])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ------------------------------------------------------------------
    // json_canonical tests
    // ------------------------------------------------------------------

    #[test]
    fn canonical_json_string_sorts_nested_object_keys() {
        let left = json!({
            "b": 2,
            "a": {
                "d": true,
                "c": [3, {"z": 1, "y": 2}]
            }
        });
        let right = json!({
            "a": {
                "c": [3, {"y": 2, "z": 1}],
                "d": true
            },
            "b": 2
        });

        assert_eq!(canonical_json_string(&left), canonical_json_string(&right));
        assert_eq!(
            short_value_hash(Some(&left)),
            short_value_hash(Some(&right))
        );
    }

    #[test]
    fn canonicalize_value_sorts_map_storage_order() {
        let value = canonicalize_value(json!({"b": 2, "a": 1}));

        assert_eq!(serde_json::to_string(&value).unwrap(), r#"{"a":1,"b":2}"#);
    }

    #[test]
    fn canonicalize_json_string_if_parseable_sorts_keys_and_removes_whitespace() {
        assert_eq!(
            canonicalize_json_string_if_parseable(r#"{ "b": 2, "a": 1 }"#),
            r#"{"a":1,"b":2}"#
        );
    }

    #[test]
    fn canonicalize_json_string_if_parseable_preserves_plain_text() {
        assert_eq!(
            canonicalize_json_string_if_parseable("plain text"),
            "plain text"
        );
    }

    #[test]
    fn canonicalize_tool_arguments_str_coerces_empty_to_object() {
        assert_eq!(canonicalize_tool_arguments_str(""), "{}");
        assert_eq!(canonicalize_tool_arguments_str("   "), "{}");
        assert_eq!(canonicalize_tool_arguments_str("\n\t"), "{}");
    }

    #[test]
    fn canonicalize_tool_arguments_str_canonicalizes_valid_json() {
        assert_eq!(
            canonicalize_tool_arguments_str(r#"{ "b": 2, "a": 1 }"#),
            r#"{"a":1,"b":2}"#
        );
    }

    #[test]
    fn canonicalize_tool_arguments_handles_field_variants() {
        // Missing field -> empty object.
        assert_eq!(canonicalize_tool_arguments(None), "{}");
        // Empty string field -> empty object.
        assert_eq!(canonicalize_tool_arguments(Some(&json!(""))), "{}");
        // String field with JSON -> canonicalized.
        assert_eq!(
            canonicalize_tool_arguments(Some(&json!(r#"{"b":2,"a":1}"#))),
            r#"{"a":1,"b":2}"#
        );
        // Structured (non-string) field -> canonical serialization.
        assert_eq!(
            canonicalize_tool_arguments(Some(&json!({"b": 2, "a": 1}))),
            r#"{"a":1,"b":2}"#
        );
    }

    // ------------------------------------------------------------------
    // reasoning / think helpers tests
    // ------------------------------------------------------------------

    #[test]
    fn extract_reasoning_field_text_priority() {
        assert_eq!(
            extract_reasoning_field_text(&json!({"reasoning_content": "a", "reasoning": "b"})),
            Some("a".to_string())
        );
        assert_eq!(
            extract_reasoning_field_text(&json!({"reasoning": {"content": "b"}})),
            Some("b".to_string())
        );
        assert_eq!(
            extract_reasoning_field_text(&json!({"reasoning_details": [{"text": "c"}]})),
            Some("c".to_string())
        );
        assert_eq!(extract_reasoning_field_text(&json!({"x": 1})), None);
    }

    #[test]
    fn split_leading_think_block_splits_and_trims() {
        assert_eq!(
            split_leading_think_block("<think>because</think> answer"),
            Some(("because".to_string(), "answer".to_string()))
        );
        assert_eq!(
            split_leading_think_block("  <think>  a  </think>  b"),
            Some(("a".to_string(), "b".to_string()))
        );
        assert_eq!(split_leading_think_block("no think here"), None);
    }

    #[test]
    fn append_reasoning_content_appends_and_dedups() {
        let mut message = serde_json::Map::new();
        assert!(append_reasoning_content(&mut message, " first "));
        assert_eq!(message["reasoning_content"], "first");
        assert!(append_reasoning_content(&mut message, "second"));
        assert_eq!(message["reasoning_content"], "first\n\nsecond");
        // Empty reasoning is a no-op.
        assert!(!append_reasoning_content(&mut message, "  "));
    }

    #[test]
    fn response_function_call_item_shapes() {
        let item = response_function_call_item("i1", "completed", "c1", "foo", "{}", Some("why"));
        assert_eq!(item["type"], "function_call");
        assert_eq!(item["reasoning_content"], "why");
        assert!(item.get("namespace").is_none());

        let ns = response_function_call_item_with_namespace(
            "i2", "in_progress", "c2", "bar", Some("n"), "{}", None,
        );
        assert_eq!(ns["namespace"], "n");
    }
}
