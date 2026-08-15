//! Tool-output media extraction for Chat Completions upstreams.
//!
//! Chat tool messages are text-only, while Responses tool outputs can contain
//! image, audio, and file content parts. Move supported parts into a synthetic
//! user message and leave a small marker in the tool result so text-only
//! providers do not silently lose the payload.

use super::common::canonical_json_string;
use serde_json::{json, Map, Value};

const WHOLE_DATA_URL_MIN_BYTES: usize = 8 * 1024;
const BASE64ISH_MIN_BYTES: usize = 16 * 1024;
const MAX_MEDIA_TRAVERSAL_DEPTH: usize = 32;

pub(crate) const TOOL_RESULT_MEDIA_MOVED_MARKER: &str =
    "[media moved to the following user message]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolMediaScope {
    AllSupported,
}

#[derive(Debug)]
pub(crate) struct ChatToolOutputMediaPlan {
    pub tool_content: String,
    pub media_parts: Vec<Value>,
}

pub(crate) fn plan_chat_tool_output_media(mut output: Value) -> Option<ChatToolOutputMediaPlan> {
    let output_was_string = output.is_string();
    let replacement_block = json!({
        "type": "text",
        "text": TOOL_RESULT_MEDIA_MOVED_MARKER
    });
    let mut media_parts = Vec::new();
    let replaced = strip_and_clamp_media_from_tool_value(
        &mut output,
        &mut media_parts,
        ToolMediaScope::AllSupported,
        &replacement_block,
        TOOL_RESULT_MEDIA_MOVED_MARKER,
    );
    if replaced == 0 {
        return None;
    }

    let tool_content = if output_was_string {
        output.as_str().unwrap_or_default().to_string()
    } else {
        canonical_json_string(&output)
    };
    Some(ChatToolOutputMediaPlan {
        tool_content,
        media_parts,
    })
}

pub(crate) fn queue_chat_tool_output_media(
    pending: &mut Vec<Value>,
    call_id: &str,
    parts: Vec<Value>,
) {
    if parts.is_empty() {
        return;
    }
    pending.push(json!({
        "type": "text",
        "text": format!("[media output of tool call {call_id}]")
    }));
    pending.extend(parts);
}

pub(crate) fn flush_pending_chat_tool_media(
    messages: &mut Vec<Value>,
    pending: &mut Vec<Value>,
) {
    if pending.is_empty() {
        return;
    }
    messages.push(json!({
        "role": "user",
        "content": std::mem::take(pending)
    }));
}

pub(crate) fn strip_and_clamp_media_from_tool_value(
    value: &mut Value,
    media_parts: &mut Vec<Value>,
    scope: ToolMediaScope,
    replacement_block: &Value,
    replacement_text: &str,
) -> usize {
    let replaced = strip_media_at_depth(
        value,
        media_parts,
        scope,
        replacement_block,
        replacement_text,
        0,
    );
    if replaced > 0 {
        clamp_base64ish_strings(value);
    }
    replaced
}

fn strip_media_at_depth(
    value: &mut Value,
    media_parts: &mut Vec<Value>,
    scope: ToolMediaScope,
    replacement_block: &Value,
    replacement_text: &str,
    depth: usize,
) -> usize {
    if depth > MAX_MEDIA_TRAVERSAL_DEPTH {
        return 0;
    }

    match value {
        Value::String(text) => {
            if let Some(media) = whole_string_image_data_url(text) {
                media_parts.push(media);
                *text = replacement_text.to_string();
                return 1;
            }

            let trimmed = text.trim();
            if trimmed.is_empty() {
                return 0;
            }
            let Ok(mut parsed) = serde_json::from_str::<Value>(trimmed) else {
                return 0;
            };
            let replaced = strip_media_at_depth(
                &mut parsed,
                media_parts,
                scope,
                replacement_block,
                replacement_text,
                depth + 1,
            );
            if replaced > 0 {
                clamp_base64ish_strings(&mut parsed);
                *text = canonical_json_string(&parsed);
            }
            replaced
        }
        Value::Array(items) => items
            .iter_mut()
            .map(|item| {
                strip_media_at_depth(
                    item,
                    media_parts,
                    scope,
                    replacement_block,
                    replacement_text,
                    depth + 1,
                )
            })
            .sum(),
        Value::Object(_) => {
            let media = chat_media_part_from_tool_part(value, scope);
            if let Some(media) = media {
                media_parts.push(media);
                *value = replacement_block.clone();
                return 1;
            }

            value
                .as_object_mut()
                .expect("object match arm must remain an object")
                .get_mut("content")
                .map(|content| {
                    strip_media_at_depth(
                        content,
                        media_parts,
                        scope,
                        replacement_block,
                        replacement_text,
                        depth + 1,
                    )
                })
                .unwrap_or(0)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
    }
}

fn chat_media_part_from_tool_part(part: &Value, _scope: ToolMediaScope) -> Option<Value> {
    let object = part.as_object()?;
    match object.get("type").and_then(Value::as_str) {
        Some("input_image" | "image_url") => normalized_image_url(part).map(|image_url| {
            json!({"type": "image_url", "image_url": image_url})
        }),
        Some("input_file") => chat_file_from_input_file(part).map(|file| {
            json!({"type": "file", "file": file})
        }),
        Some("input_audio") => object
            .get("input_audio")
            .filter(|value| value.is_object())
            .map(|input_audio| json!({"type": "input_audio", "input_audio": input_audio})),
        Some("image") => typed_image_url(part).map(|image_url| {
            json!({"type": "image_url", "image_url": image_url})
        }),
        None => normalized_image_url(part)
            .filter(|image| {
                image
                    .get("url")
                    .and_then(Value::as_str)
                    .is_some_and(|url| url.trim_start().to_ascii_lowercase().starts_with("data:image/")
                        && url.to_ascii_lowercase().contains(";base64,"))
            })
            .map(|image_url| json!({"type": "image_url", "image_url": image_url})),
        _ => None,
    }
}

pub(crate) fn chat_file_from_input_file(part: &Value) -> Option<Value> {
    let mut file = Map::new();
    if part.get("file_id").is_none() && part.get("file_data").is_none() {
        return None;
    }
    for key in ["file_id", "file_data", "filename"] {
        if let Some(value) = part.get(key) {
            file.insert(key.to_string(), value.clone());
        }
    }
    Some(Value::Object(file))
}

fn normalized_image_url(part: &Value) -> Option<Value> {
    let image_url = part.get("image_url")?;
    let mut image = match image_url {
        Value::String(url) if !url.trim().is_empty() => {
            let mut map = Map::new();
            map.insert("url".to_string(), Value::String(url.clone()));
            map
        }
        Value::Object(map)
            if map
                .get("url")
                .and_then(Value::as_str)
                .is_some_and(|url| !url.trim().is_empty()) => map.clone(),
        _ => return None,
    };
    if image.get("detail").is_none() {
        if let Some(detail) = part.get("detail") {
            image.insert("detail".to_string(), detail.clone());
        }
    }
    Some(Value::Object(image))
}

fn typed_image_url(part: &Value) -> Option<Value> {
    let object = part.as_object()?;
    if let Some(source) = object.get("source").and_then(Value::as_object) {
        let mime = source
            .get("media_type")
            .or_else(|| source.get("mime_type"))
            .or_else(|| source.get("mimeType"))
            .and_then(Value::as_str)
            .unwrap_or("image/png");
        if !mime.to_ascii_lowercase().starts_with("image/") {
            return None;
        }
        if let Some(url) = source.get("url").and_then(Value::as_str).filter(|v| !v.is_empty()) {
            return Some(json!({"url": url}));
        }
        if let Some(data) = source.get("data").and_then(Value::as_str).filter(|v| !v.is_empty()) {
            return Some(json!({"url": format!("data:{mime};base64,{data}")}));
        }
    }

    let data = object.get("data").and_then(Value::as_str).filter(|v| !v.is_empty())?;
    let mime = object
        .get("mimeType")
        .or_else(|| object.get("mime_type"))
        .and_then(Value::as_str)
        .filter(|v| v.to_ascii_lowercase().starts_with("image/"))?;
    Some(json!({"url": format!("data:{mime};base64,{data}")}))
}

fn whole_string_image_data_url(value: &str) -> Option<Value> {
    let trimmed = value.trim();
    if trimmed.len() < WHOLE_DATA_URL_MIN_BYTES {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("data:image/") || !lower.contains(";base64,") {
        return None;
    }
    Some(json!({"type": "image_url", "image_url": {"url": trimmed}}))
}

fn clamp_base64ish_strings(value: &mut Value) {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            let lower = trimmed.to_ascii_lowercase();
            if (trimmed.len() >= WHOLE_DATA_URL_MIN_BYTES && lower.starts_with("data:"))
                || (trimmed.len() >= BASE64ISH_MIN_BYTES
                    && trimmed
                        .bytes()
                        .all(|byte| matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'/' | b'=')))
            {
                *text = format!("[omitted {} bytes]", text.len());
            }
        }
        Value::Array(items) => items.iter_mut().for_each(clamp_base64ish_strings),
        Value::Object(object) => object.values_mut().for_each(clamp_base64ish_strings),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn large_data_url() -> String {
        format!("data:image/png;base64,{}", "A".repeat(9_000))
    }

    #[test]
    fn extracts_image_audio_and_file_from_tool_output() {
        let mut value = json!({
            "content": [
                {"type": "input_image", "image_url": large_data_url()},
                {"type": "input_audio", "input_audio": {"data": "AAAA", "format": "wav"}},
                {"type": "input_file", "file_id": "file_1", "filename": "a.txt"}
            ]
        });
        let replacement = json!({"type": "text", "text": "moved"});
        let mut parts = Vec::new();
        assert_eq!(
            strip_and_clamp_media_from_tool_value(
                &mut value,
                &mut parts,
                ToolMediaScope::AllSupported,
                &replacement,
                "moved",
            ),
            3
        );
        assert_eq!(parts[0]["type"], "image_url");
        assert_eq!(parts[1]["type"], "input_audio");
        assert_eq!(parts[2]["type"], "file");
        assert!(!value.to_string().contains(&large_data_url()));
    }

    #[test]
    fn plan_keeps_scalar_tool_output_unquoted() {
        let plan = plan_chat_tool_output_media(Value::String(large_data_url())).unwrap();
        assert_eq!(plan.tool_content, TOOL_RESULT_MEDIA_MOVED_MARKER);
        assert_eq!(plan.media_parts[0]["type"], "image_url");
    }

    #[test]
    fn no_media_is_a_no_op() {
        let mut value = json!({"content": [{"type": "text", "text": "hello"}]});
        let before = value.clone();
        let mut parts = Vec::new();
        assert_eq!(
            strip_and_clamp_media_from_tool_value(
                &mut value,
                &mut parts,
                ToolMediaScope::AllSupported,
                &json!({"type": "text", "text": "moved"}),
                "moved",
            ),
            0
        );
        assert_eq!(value, before);
        assert!(parts.is_empty());
    }
}
