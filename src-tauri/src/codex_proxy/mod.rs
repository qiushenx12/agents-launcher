//! Codex protocol conversion proxy.
//!
//! Codex CLI speaks the OpenAI Responses API, while providers such as
//! Kimi For Coding only expose an OpenAI-compatible Chat Completions API.
//! When a Codex profile enables protocol conversion, the launcher runs a
//! local per-profile HTTP proxy that translates Responses requests into
//! Chat Completions requests and converts the (streaming) responses back.
//!
//! Conversion logic is ported from cc-switch (`proxy/providers/`).

pub(crate) mod common;
pub(crate) mod content_encoding;
pub(crate) mod history;
pub(crate) mod reasoning;
pub(crate) mod responses_sse;
pub(crate) mod server;
pub(crate) mod sse;
pub(crate) mod stream;
pub(crate) mod tool_media;
pub(crate) mod transform;

use crate::codex_config::{CodexAuthMode, CodexProfile};
use server::{router, ProxyState};
use std::collections::HashMap;
use std::net::TcpListener as StdTcpListener;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ProxyMode {
    /// 启动器终端启动路径（resolve 每次重渲染 profile TOML，随机端口无缓存问题）。
    Managed,
    /// 全局配置接管路径（Codex 桌面端缓存 base_url，必须固定端口防漂移断连）。
    Global,
}

pub(crate) struct ProxyInstance {
    port: u16,
    task: tauri::async_runtime::JoinHandle<()>,
    upstream_base_url: String,
    /// 注册时的 API key，用于检测 key 变更后重建实例。
    api_key: String,
    reasoning_override: Option<reasoning::CodexChatReasoningConfig>,
    mode: ProxyMode,
}

/// 全局接管的固定端口。Codex 桌面端 / VSCode 扩展在加载配置时读取一次
/// base_url 并缓存，随机端口在 launcher 重启后漂移会导致"连接被拒/重连失败"。
/// 避开 cc-switch 的默认端口 15721。
pub(crate) const GLOBAL_PROXY_PORT: u16 = 15800;

type ProxyKey = (String, ProxyMode);

static REGISTRY: OnceLock<Mutex<HashMap<ProxyKey, ProxyInstance>>> = OnceLock::new();
static HISTORY_REGISTRY: OnceLock<Mutex<HashMap<String, Arc<history::CodexChatHistoryStore>>>> =
    OnceLock::new();
static ENSURE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<ProxyKey, ProxyInstance>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ensure_lock() -> &'static Mutex<()> {
    ENSURE_LOCK.get_or_init(|| Mutex::new(()))
}

fn history_for_profile(profile_id: &str) -> Arc<history::CodexChatHistoryStore> {
    let histories = HISTORY_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = histories.lock().expect("history registry lock poisoned");
    guard
        .entry(profile_id.to_string())
        .or_insert_with(|| Arc::new(history::CodexChatHistoryStore::persistent(profile_id)))
        .clone()
}

fn reasoning_override_from_profile(
    profile: &CodexProfile,
) -> Option<reasoning::CodexChatReasoningConfig> {
    profile
        .extra
        .get("codexChatReasoning")
        .or_else(|| profile.extra.get("codex_chat_reasoning"))
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .map(reasoning::normalize_reasoning_config)
}

pub(crate) fn forget_history(profile_id: &str) {
    if let Some(histories) = HISTORY_REGISTRY.get() {
        if let Ok(mut guard) = histories.lock() {
            guard.remove(profile_id);
        }
    }
    history::remove_persistent(profile_id);
}

fn local_base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1")
}

/// 确保 profile 的转换代理在运行（启动器终端路径，随机端口）。
pub(crate) fn ensure_conversion(profile: &CodexProfile) -> Result<Option<String>, String> {
    ensure_conversion_on(profile, None, ProxyMode::Managed)
}

/// 确保 profile 的转换代理在运行（全局接管路径，固定端口防漂移）。
pub(crate) fn ensure_conversion_fixed_port(
    profile: &CodexProfile,
) -> Result<Option<String>, String> {
    ensure_conversion_on(profile, Some(GLOBAL_PROXY_PORT), ProxyMode::Global)
}

fn ensure_conversion_on(
    profile: &CodexProfile,
    port: Option<u16>,
    mode: ProxyMode,
) -> Result<Option<String>, String> {
    let _ensure_guard = ensure_lock()
        .lock()
        .map_err(|_| "协议转换代理启动锁不可用".to_string())?;
    let enabled = profile.protocol_conversion && profile.auth_mode == CodexAuthMode::Custom;
    if !enabled {
        stop_mode_unlocked(&profile.id, mode);
        return Ok(None);
    }

    // 解析上游 API key（DPAPI 密文或环境变量）。代理持有 key 用于注入
    // Authorization——Codex 桌面端读取不到启动器加密保存的 key。
    let api_key = crate::codex_config::resolve_profile_api_key(profile)?;
    let reasoning_override = reasoning_override_from_profile(profile);

    {
        let Ok(guard) = registry().lock() else {
            return Err("转换代理注册表不可用".to_string());
        };
        let key = (profile.id.clone(), mode);
        if let Some(instance) = guard.get(&key) {
            if !instance.task.inner().is_finished()
                && instance.mode == mode
                && instance.upstream_base_url == profile.base_url
                && instance.api_key == api_key
                && instance.reasoning_override == reasoning_override
                && port.is_none_or(|expected| instance.port == expected)
            {
                return Ok(Some(local_base_url(instance.port)));
            }
        }
    }

    let listener = match port {
        Some(port) => bind_fixed_with_preemption(port)?,
        None => StdTcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| format!("无法启动协议转换代理：{error}"))?,
    };
    let actual_port = listener
        .local_addr()
        .map_err(|error| format!("无法获取转换代理端口：{error}"))?
        .port();
    let history = history_for_profile(&profile.id);
    let catalog_model_ids = profile
        .model_catalog
        .as_ref()
        .map(|catalog| {
            catalog
                .models
                .iter()
                .map(|model| model.slug.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let state = Arc::new(ProxyState {
        upstream_base_url: profile.base_url.clone(),
        api_key: Some(api_key.clone()),
        provider_name: profile.provider_name.clone(),
        default_model: profile.model.clone(),
        chat_upstream_model: (!profile.chat_upstream_model.trim().is_empty())
            .then(|| profile.chat_upstream_model.trim().to_string()),
        catalog_model_ids,
        prompt_cache_routing: profile.prompt_cache_routing.clone(),
        reasoning_override: reasoning_override.clone(),
        history: history.clone(),
        http: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| format!("无法创建转换代理 HTTP 客户端：{error}"))?,
    });
    let task = start_proxy(listener, state);

    let mut guard = registry()
        .lock()
        .map_err(|_| "转换代理注册表不可用".to_string())?;
    if let Some(previous) = guard.insert(
        (profile.id.clone(), mode),
        ProxyInstance {
            port: actual_port,
            task,
            upstream_base_url: profile.base_url.clone(),
            api_key,
            reasoning_override,
            mode,
        },
    ) {
        // The new listener is ready before the old task is aborted, so a
        // managed profile rebuild does not leave a gap in service.
        previous.task.abort();
    }
    Ok(Some(local_base_url(actual_port)))
}

/// 绑定固定端口。被本进程其他全局实例占用时先停掉它再重试（全局 profile
/// 切换场景）；被外部进程占用时短暂等待（TIME_WAIT / 端口释放延迟）后重试。
fn bind_fixed_with_preemption(port: u16) -> Result<StdTcpListener, String> {
    for attempt in 0..10 {
        match StdTcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => return Ok(listener),
            Err(error) => {
                if attempt == 9 {
                    return Err(format!(
                        "协议转换代理端口 {port} 被占用（请关闭占用该端口的程序后重试）：{error}"
                    ));
                }
                // 本进程其他全局实例占用（全局 profile 切换）→ 停掉它；
                // 外部进程占用 → 等待端口释放（TIME_WAIT）后重试。
                if let Ok(mut guard) = registry().lock() {
                    let stale: Vec<ProxyKey> = guard
                        .iter()
                        .filter(|((_, mode), instance)| {
                            *mode == ProxyMode::Global && instance.port == port
                        })
                        .map(|(key, _)| key.clone())
                        .collect();
                    for key in stale {
                        if let Some(instance) = guard.remove(&key) {
                            instance.task.abort();
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    unreachable!("bind_fixed_with_preemption loop always returns")
}

/// 停止并移除 profile 的转换代理（删除 profile / 关闭转换时调用）。
fn stop_mode_unlocked(profile_id: &str, mode: ProxyMode) {
    if let Ok(mut guard) = registry().lock() {
        if let Some(instance) = guard.remove(&(profile_id.to_string(), mode)) {
            instance.task.abort();
        }
    }
}

/// Stop both proxy modes owned by a profile.
pub(crate) fn stop(profile_id: &str) {
    let Ok(_ensure_guard) = ensure_lock().lock() else {
        return;
    };
    stop_mode_unlocked(profile_id, ProxyMode::Managed);
    stop_mode_unlocked(profile_id, ProxyMode::Global);
}

/// 停止所有转换代理（应用退出 / 无全局 profile 时调用）。
pub(crate) fn stop_all() {
    let Ok(_ensure_guard) = ensure_lock().lock() else {
        return;
    };
    if let Ok(mut guard) = registry().lock() {
        for (_, instance) in guard.drain() {
            instance.task.abort();
        }
    }
}

/// 停止全局接管模式下、不属于当前全局 profile 的代理实例（全局 profile 切换
/// 后清理残留；Managed 实例由 resolve 路径管理，不受影响）。
pub(crate) fn stop_global_instances_except(keep_profile_id: Option<&str>) {
    let Ok(_ensure_guard) = ensure_lock().lock() else {
        return;
    };
    if let Ok(mut guard) = registry().lock() {
        let stale: Vec<ProxyKey> = guard
            .iter()
            .filter(|((id, mode), _)| {
                *mode == ProxyMode::Global && Some(id.as_str()) != keep_profile_id
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in stale {
            if let Some(instance) = guard.remove(&key) {
                instance.task.abort();
            }
        }
    }
}

fn start_proxy(
    listener: StdTcpListener,
    state: Arc<ProxyState>,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        if listener.set_nonblocking(true).is_err() {
            return;
        }
        let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
            return;
        };
        let router = router(state);
        let _ = axum::serve(listener, router).await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_config::CodexProfile;

    fn profile_with(profile_id: &str, protocol_conversion: bool, base_url: &str) -> CodexProfile {
        // ensure_conversion 会解析 API key（DPAPI 或环境变量）；测试用环境变量。
        // 多个测试对同一变量设置相同值，互不干扰。
        std::env::set_var("OPENAI_API_KEY", "test-key");
        CodexProfile {
            id: profile_id.to_string(),
            name: "Test".to_string(),
            auth_mode: CodexAuthMode::Custom,
            model: "kimi-k2".to_string(),
            reasoning_effort: "high".to_string(),
            openai_base_url: String::new(),
            provider_id: "kimi".to_string(),
            provider_name: "Kimi".to_string(),
            base_url: base_url.to_string(),
            wire_api: "responses".to_string(),
            env_key: "OPENAI_API_KEY".to_string(),
            has_stored_api_key: false,
            managed_profile_name: "agents-launcher-profile-test".to_string(),
            model_catalog: None,
            protocol_conversion,
            chat_upstream_model: String::new(),
            prompt_cache_routing: "auto".to_string(),
            extra: Default::default(),
        }
    }

    #[tokio::test]
    async fn disabled_returns_none_and_stops_existing() {
        // 先启用启动一个实例，再禁用确认被停止
        let enabled = profile_with("profile-a", true, "https://api.moonshot.cn/v1");
        let Some(url) = ensure_conversion(&enabled).unwrap() else {
            panic!("expected proxy url");
        };
        assert!(url.starts_with("http://127.0.0.1:"), "url={url}");

        let disabled = profile_with("profile-a", false, "https://api.moonshot.cn/v1");
        assert!(ensure_conversion(&disabled).unwrap().is_none());
        let guard = registry().lock().unwrap();
        assert!(guard
            .get(&("profile-a".to_string(), ProxyMode::Managed))
            .is_none());
    }

    #[tokio::test]
    async fn ensure_is_idempotent() {
        let profile = profile_with("profile-b", true, "https://api.moonshot.cn/v1");
        let first = ensure_conversion(&profile).unwrap().unwrap();
        let second = ensure_conversion(&profile).unwrap().unwrap();
        assert_eq!(first, second, "same profile reuses the same port");
        stop(&profile.id);
    }

    #[tokio::test]
    async fn upstream_change_triggers_rebuild() {
        let profile = profile_with("profile-c", true, "https://api.moonshot.cn/v1");
        let first = ensure_conversion(&profile).unwrap().unwrap();

        let changed = profile_with("profile-c", true, "https://api.deepseek.com/v1");
        let second = ensure_conversion(&changed).unwrap().unwrap();
        assert_ne!(first, second, "upstream change must allocate a new port");

        let guard = registry().lock().unwrap();
        let instance = guard
            .get(&("profile-c".to_string(), ProxyMode::Managed))
            .unwrap();
        assert_eq!(instance.upstream_base_url, "https://api.deepseek.com/v1");
        drop(guard);
        stop(&changed.id);
    }

    #[tokio::test]
    async fn custom_only_gate() {
        let mut profile = profile_with("profile-d", true, "https://api.moonshot.cn/v1");
        profile.auth_mode = CodexAuthMode::Official;
        assert!(ensure_conversion(&profile).unwrap().is_none());
    }

    #[tokio::test]
    async fn managed_and_global_instances_for_one_profile_are_isolated() {
        let profile = profile_with("profile-e", true, "https://api.moonshot.cn/v1");
        let managed = ensure_conversion(&profile).unwrap().unwrap();
        let global = ensure_conversion_fixed_port(&profile).unwrap().unwrap();

        assert_ne!(managed, global);
        let guard = registry().lock().unwrap();
        assert!(guard.contains_key(&(profile.id.clone(), ProxyMode::Managed)));
        assert!(guard.contains_key(&(profile.id.clone(), ProxyMode::Global)));
        drop(guard);
        stop(&profile.id);
    }

    #[test]
    fn profile_reasoning_metadata_is_explicit_override() {
        let mut profile = profile_with("profile-f", true, "https://relay.example.com/v1");
        profile.extra.insert(
            "codexChatReasoning".to_string(),
            serde_json::json!({
                "supportsThinking": false,
                "supportsEffort": true,
                "effortParam": "reasoning.effort"
            }),
        );

        let config = reasoning_override_from_profile(&profile).expect("override");
        assert_eq!(config.supports_thinking, Some(false));
        assert_eq!(config.supports_effort, Some(true));
        assert_eq!(config.effort_param.as_deref(), Some("reasoning.effort"));
    }
}
