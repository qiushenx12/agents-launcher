//! Reasoning parameter inference for Chat Completions upstreams.
//!
//! Ported from cc-switch `proxy/providers/codex.rs`
//! (`infer_codex_chat_reasoning_config` / `infer_aggregator_platform_config`),
//! with the provider metadata retained: inference is driven by the provider
//! name, upstream base URL, and the requested model name.

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodexChatReasoningConfig {
    #[serde(default, alias = "supports_thinking")]
    pub supports_thinking: Option<bool>,
    #[serde(default, alias = "supports_effort")]
    pub supports_effort: Option<bool>,
    #[serde(default, alias = "thinking_param")]
    pub thinking_param: Option<String>,
    #[serde(default, alias = "effort_param")]
    pub effort_param: Option<String>,
    #[serde(default, alias = "effort_value_mode")]
    pub effort_value_mode: Option<String>,
    /// Declarative field: where the upstream returns reasoning
    /// (`reasoning_content` / `reasoning` / `reasoning_details` / `think_tags`).
    /// The response side extracts via `extract_reasoning_field_text` and does
    /// not read this field; kept as documentation and a hook for future use.
    #[serde(default, alias = "output_format")]
    pub output_format: Option<String>,
}

pub(crate) fn normalize_reasoning_config(
    mut config: CodexChatReasoningConfig,
) -> CodexChatReasoningConfig {
    if config.supports_effort.unwrap_or(false) && config.supports_thinking.is_none() {
        config.supports_thinking = Some(true);
    }
    config
}

#[allow(dead_code)] // effort mapping helpers exercised by tests; transform.rs uses its own free function
impl CodexChatReasoningConfig {
    /// DeepSeek-style effort mapping: max/xhigh → max, everything else → high.
    fn map_deepseek_effort(&self, value: &str) -> String {
        if matches!(value, "max" | "xhigh") {
            "max".to_string()
        } else {
            "high".to_string()
        }
    }

    /// OpenRouter effort mapping: valid enums are
    /// xhigh|high|medium|low|minimal (no `max`); `max` is clamped to `xhigh`.
    fn map_openrouter_effort(&self, value: &str) -> String {
        match value {
            "max" => "xhigh".to_string(),
            other => other.to_string(),
        }
    }

    /// Resolve a normalized effort value for the configured effort parameter.
    /// Returns `None` when effort is not applicable (or explicitly disabled).
    pub(crate) fn map_effort_value(&self, value: &str) -> Option<String> {
        if value.is_empty() {
            return None;
        }
        let mode = self.effort_value_mode.as_deref().unwrap_or("passthrough");
        match mode {
            "deepseek" => Some(self.map_deepseek_effort(value)),
            "low_high" => Some(if matches!(value, "low") { "low" } else { "high" }.to_string()),
            "openrouter" => Some(self.map_openrouter_effort(value)),
            _ => Some(value.to_string()),
        }
    }
}

/// Infer the reasoning parameter style from the upstream base URL + model name.
///
/// Platform rules (aggregators / hosted platforms) are decided by the platform
/// identifier alone — never the model name — because the platform's reasoning
/// interface belongs to the platform framework, not the model vendor.
#[cfg(test)]
pub(crate) fn infer_reasoning_config(base_url: &str, model: &str) -> Option<CodexChatReasoningConfig> {
    infer_reasoning_config_for_provider("", base_url, model)
}

/// Infer reasoning settings for the model used by the current request.
///
/// A single Codex profile may expose a model catalog containing providers with
/// different reasoning conventions. The proxy therefore must not freeze this
/// decision at proxy startup using the profile's default model.
pub(crate) fn infer_reasoning_config_for_provider(
    provider_name: &str,
    base_url: &str,
    model: &str,
) -> Option<CodexChatReasoningConfig> {
    let provider_name = provider_name.to_ascii_lowercase();
    let base_url = base_url.to_ascii_lowercase();
    let model = model.to_ascii_lowercase();

    // 平台优先：聚合 / 托管平台的 reasoning 接口由平台的推理框架决定，而非模型官方实现，
    // 因此先按平台标识（仅 base_url）判定并覆盖模型规则。
    if let Some(config) = infer_aggregator_platform_config(&provider_name, &base_url) {
        return Some(config);
    }

    let haystack = format!("{provider_name} {base_url} {model}");

    if haystack.contains("deepseek") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(true),
            thinking_param: Some("thinking".to_string()),
            effort_param: Some("reasoning_effort".to_string()),
            effort_value_mode: Some("deepseek".to_string()),
            output_format: Some("reasoning_content".to_string()),
        });
    }

    // StepFun：仅 step-3.5-flash-2603 这一版支持 reasoning effort（low/high 两档），
    // 其余 step 模型不暴露 effort，故 supports_effort 仅对含 "2603" 的模型置真。
    // 第二个 OR 分支覆盖「经中转/聚合跑该模型、但平台 base_url 不含 stepfun」的情况。
    if haystack.contains("stepfun") || haystack.contains("step-3.5-flash-2603") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(model.contains("2603")),
            thinking_param: Some("none".to_string()),
            effort_param: Some("reasoning_effort".to_string()),
            effort_value_mode: Some("low_high".to_string()),
            output_format: Some("reasoning".to_string()),
        });
    }

    if haystack.contains("kimi") || haystack.contains("moonshot") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
        });
    }

    if haystack.contains("glm") || haystack.contains("zhipu") || haystack.contains("z.ai") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
        });
    }

    if haystack.contains("qwen") || haystack.contains("dashscope") || haystack.contains("bailian") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("enable_thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
        });
    }

    if haystack.contains("minimax") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("reasoning_split".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_details".to_string()),
        });
    }

    if haystack.contains("mimo") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
        });
    }

    None
}

/// 聚合 / 托管平台的 reasoning 接口由平台决定：同一个模型在不同平台参数可能完全不同
/// （DeepSeek 官方用 `thinking:{type}`、SiliconFlow 用 `enable_thinking`、
/// OpenRouter 用原生 `reasoning:{effort}` 对象）。仅以平台标识（base_url）判定，
/// 绝不掺入 model 名——model 名属于模型厂商，会把托管平台误判成模型官方接口。
fn infer_aggregator_platform_config(
    provider_name: &str,
    base_url: &str,
) -> Option<CodexChatReasoningConfig> {
    let platform = format!("{provider_name} {base_url}");

    // OpenRouter：用原生归一化对象 `reasoning: { effort }`（由 OpenRouter 翻译成各底层
    // 模型的正确推理参数，比顶层 OpenAI 别名 reasoning_effort 覆盖面更全）。effort 走
    // "openrouter" 值映射：枚举为 xhigh|high|medium|low|minimal，无 max——max 会触发
    // `400 reasoning_effort: Invalid option`（见 openclaw#77350），故钳到 xhigh。
    // 安全降级：不发 `thinking:{type}`（OpenRouter 不认该字段），避免误配导致请求被拒。
    if platform.contains("openrouter") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(false),
            supports_effort: Some(true),
            thinking_param: Some("none".to_string()),
            effort_param: Some("reasoning.effort".to_string()),
            effort_value_mode: Some("openrouter".to_string()),
            output_format: Some("auto".to_string()),
        });
    }

    // SiliconFlow：平台级统一 `enable_thinking`，思维回传 reasoning_content。
    // 安全降级：不按 reasoning_effort 发 effort（平台用 thinking_budget 控制深度，
    // 发 reasoning_effort 反而可能不被接受）。
    if platform.contains("siliconflow") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("enable_thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kimi() -> CodexChatReasoningConfig {
        CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
        }
    }

    #[test]
    fn infers_kimi_from_base_url_and_model() {
        assert_eq!(
            infer_reasoning_config("https://api.moonshot.cn/v1", "kimi-k2"),
            Some(kimi())
        );
        assert_eq!(
            infer_reasoning_config("https://api.kimi.com/coding/v1", "kimi-for-coding"),
            Some(kimi())
        );
    }

    #[test]
    fn infers_deepseek_reasoning_effort() {
        let config = infer_reasoning_config("https://api.deepseek.com/v1", "deepseek-chat")
            .expect("deepseek config");
        assert_eq!(config.effort_param.as_deref(), Some("reasoning_effort"));
        assert_eq!(config.effort_value_mode.as_deref(), Some("deepseek"));
    }

    #[test]
    fn infers_openrouter_native_effort_object() {
        let config = infer_reasoning_config("https://openrouter.ai/api/v1", "anything")
            .expect("openrouter config");
        assert_eq!(config.effort_param.as_deref(), Some("reasoning.effort"));
        assert_eq!(config.effort_value_mode.as_deref(), Some("openrouter"));
        // OpenRouter does not accept thinking:{type}.
        assert_eq!(config.thinking_param.as_deref(), Some("none"));
    }

    #[test]
    fn effort_value_mapping() {
        let deepseek = infer_reasoning_config("https://api.deepseek.com/v1", "deepseek-chat")
            .unwrap();
        assert_eq!(deepseek.map_effort_value("xhigh").as_deref(), Some("max"));
        assert_eq!(deepseek.map_effort_value("low").as_deref(), Some("high"));
        assert_eq!(deepseek.map_effort_value(""), None);

        let openrouter = infer_reasoning_config("https://openrouter.ai/api/v1", "x").unwrap();
        assert_eq!(openrouter.map_effort_value("max").as_deref(), Some("xhigh"));
        assert_eq!(openrouter.map_effort_value("medium").as_deref(), Some("medium"));
    }

    #[test]
    fn unknown_upstream_has_no_config() {
        assert_eq!(
            infer_reasoning_config("https://example.com/v1", "custom-model"),
            None
        );
    }

    #[test]
    fn requested_model_changes_reasoning_config_for_one_relay_profile() {
        let deepseek = infer_reasoning_config_for_provider(
            "custom relay",
            "https://relay.example.com/v1",
            "deepseek-chat",
        )
        .expect("deepseek config");
        let kimi = infer_reasoning_config_for_provider(
            "custom relay",
            "https://relay.example.com/v1",
            "kimi-k2",
        )
        .expect("kimi config");

        assert_eq!(deepseek.effort_param.as_deref(), Some("reasoning_effort"));
        assert_eq!(kimi.effort_param.as_deref(), Some("none"));
        assert_ne!(deepseek.supports_effort, kimi.supports_effort);
    }
}
