use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use toml_edit::{value, DocumentMut, Item, Table};
#[cfg(any(windows, target_os = "macos"))]
use toml_edit::{Array, Value as TomlValue};
use url::Url;
use uuid::Uuid;

use crate::file_transaction::{restore_json_backup_if_missing, write_json_atomic};
#[cfg(target_os = "macos")]
use crate::file_transaction::{restore_private_json_backup_if_missing, write_private_json_atomic};
use crate::model_fetcher;
use crate::persistent_state::{
    load_profile_index_state, save_profile_index_state, ProfileIndexState,
};
#[cfg(windows)]
use crate::{env_applier, registry};

const CODEX_STATE_VERSION: u32 = 1;
const CODEX_STATE_KEY: &str = "codex";
const MANAGED_PROFILE_PREFIX: &str = "agents-launcher-";
const LEGACY_MANAGED_PROFILE_PREFIX: &str = "cc-launcher-";
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const CODEX_SQLITE_HOME_ENV: &str = "CODEX_SQLITE_HOME";
const CODEX_DESKTOP_STATE_FILE: &str = ".codex-global-state.json";
const DESKTOP_PROJECT_ARRAY_KEYS: [&str; 3] = [
    "electron-saved-workspace-roots",
    "project-order",
    "active-workspace-roots",
];
const DESKTOP_PROJECT_OBJECT_KEYS: [&str; 2] =
    ["local-projects", "electron-workspace-root-labels"];
const DESKTOP_PROJECT_VALUE_KEYS: [&str; 1] = ["selected-project"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct GlobalProxySyncFingerprint([u8; 32]);

#[derive(Debug, Clone)]
struct GlobalProxySyncCacheEntry {
    fingerprint: GlobalProxySyncFingerprint,
    expected_profiles: Vec<CodexProfile>,
}

static GLOBAL_PROXY_SYNC_CACHE: OnceLock<Mutex<Option<GlobalProxySyncCacheEntry>>> =
    OnceLock::new();
static GLOBAL_PROXY_SYNC_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn global_proxy_sync_cache() -> &'static Mutex<Option<GlobalProxySyncCacheEntry>> {
    GLOBAL_PROXY_SYNC_CACHE.get_or_init(|| Mutex::new(None))
}

fn global_proxy_sync_lock() -> &'static Mutex<()> {
    GLOBAL_PROXY_SYNC_LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodexAuthMode {
    Official,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexReasoningLevel {
    pub effort: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexTruncationPolicy {
    pub mode: String,
    pub limit: u64,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexModelDefinition {
    pub slug: String,
    #[serde(default, rename = "displayName", alias = "display_name")]
    pub display_name: String,
    #[serde(
        default = "default_input_modalities",
        rename = "inputModalities",
        alias = "input_modalities"
    )]
    pub input_modalities: Vec<String>,
    #[serde(
        default,
        rename = "supportsImageDetailOriginal",
        alias = "supports_image_detail_original"
    )]
    pub supports_image_detail_original: bool,
    #[serde(rename = "contextWindow", alias = "context_window")]
    pub context_window: u64,
    #[serde(rename = "maxContextWindow", alias = "max_context_window")]
    pub max_context_window: u64,
    #[serde(
        default = "default_effective_context_window_percent",
        rename = "effectiveContextWindowPercent",
        alias = "effective_context_window_percent"
    )]
    pub effective_context_window_percent: u8,
    #[serde(default, rename = "truncationPolicy", alias = "truncation_policy")]
    pub truncation_policy: Option<CodexTruncationPolicy>,
    #[serde(default, rename = "defaultReasoningLevel", alias = "default_reasoning_level")]
    pub default_reasoning_level: String,
    #[serde(
        default,
        rename = "supportedReasoningLevels",
        alias = "supported_reasoning_levels"
    )]
    pub supported_reasoning_levels: Vec<CodexReasoningLevel>,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexModelCatalog {
    #[serde(default)]
    pub models: Vec<CodexModelDefinition>,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

fn default_effective_context_window_percent() -> u8 {
    95
}

fn default_input_modalities() -> Vec<String> {
    vec!["text".to_string()]
}

const DEEPSEEK_MODEL_CATALOG_TEMPLATE: &str = include_str!("../../src/deepseekModelsTemplate.json");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodexProfile {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub auth_mode: CodexAuthMode,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub reasoning_effort: String,
    /// Optional override for the active model's context window in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_context_window: Option<u64>,
    #[serde(default)]
    pub model_context_window_configured: bool,
    /// Optional fraction of the active model's context window at which
    /// automatic history compaction starts. It is converted to Codex's
    /// `model_auto_compact_token_limit` when the TOML is rendered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_auto_compact_ratio: Option<f64>,
    #[serde(default)]
    pub model_auto_compact_ratio_configured: bool,
    #[serde(default)]
    pub openai_base_url: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub provider_name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default = "default_wire_api")]
    pub wire_api: String,
    /// 协议转换开关：启用后启动 CodeX 终端时把 API 地址改写为本机转换代理
    /// （Codex Responses ↔ Chat Completions），用于仅支持 Chat Completions
    /// 的服务（如 Kimi For Coding）。仅对 Custom 模式生效。
    #[serde(default)]
    pub protocol_conversion: bool,
    /// Real model ID sent to a Chat Completions upstream when the Codex model
    /// slug is only a client-facing/catalog identifier.
    #[serde(default)]
    pub chat_upstream_model: String,
    /// Prompt-cache routing policy for the Responses -> Chat proxy.
    /// `auto` only enables the field for known compatible endpoints.
    #[serde(default = "default_prompt_cache_routing")]
    pub prompt_cache_routing: String,
    #[serde(default = "default_env_key")]
    pub env_key: String,
    #[serde(default)]
    pub has_stored_api_key: bool,
    #[serde(default)]
    pub managed_profile_name: String,
    #[serde(default)]
    pub model_catalog: Option<CodexModelCatalog>,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

fn default_wire_api() -> String {
    "responses".to_string()
}

fn default_env_key() -> String {
    "OPENAI_API_KEY".to_string()
}

fn default_prompt_cache_routing() -> String {
    "auto".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct CodexProfileState {
    #[serde(default = "default_state_version")]
    version: u32,
    #[serde(default)]
    profiles: Vec<CodexProfile>,
    #[serde(default)]
    global_profile_id: Option<String>,
    #[serde(default)]
    managed_global_provider_id: Option<String>,
    #[serde(default)]
    managed_global_model_catalog: Option<ManagedGlobalModelCatalogState>,
    #[serde(default)]
    managed_profile_model_catalogs: BTreeMap<String, Option<String>>,
    #[serde(default)]
    managed_model_catalogs: BTreeMap<String, ManagedModelCatalogState>,
    #[serde(default, flatten)]
    extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ManagedModelCatalogState {
    path: String,
    #[serde(default)]
    previous_bytes: Option<Vec<u8>>,
    applied_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ManagedGlobalModelCatalogState {
    previous_value: Option<String>,
    applied_value: String,
}

fn default_state_version() -> u32 {
    CODEX_STATE_VERSION
}

impl Default for CodexProfileState {
    fn default() -> Self {
        Self {
            version: CODEX_STATE_VERSION,
            profiles: Vec::new(),
            global_profile_id: None,
            managed_global_provider_id: None,
            managed_global_model_catalog: None,
            managed_profile_model_catalogs: BTreeMap::new(),
            managed_model_catalogs: BTreeMap::new(),
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAuthStatus {
    pub mode: Option<String>,
    pub has_auth_file: bool,
    pub has_credentials: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexProfilesPayload {
    pub profiles: Vec<CodexProfile>,
    pub order: Vec<String>,
    pub active_profile_id: Option<String>,
    pub global_profile_id: Option<String>,
    pub global_profile_in_sync: bool,
    pub global_sync_repair_required: bool,
    pub profiles_path: String,
    pub global_config_path: String,
    pub auth_path: String,
    pub global_config_error: Option<String>,
    pub auth_status: CodexAuthStatus,
    pub custom_global_sync_supported: bool,
    pub custom_global_key_sync_supported: bool,
    pub secret_storage_kind: &'static str,
    pub platform: &'static str,
    /// 切换配置前的会话完整性预检结果；仅在 apply 流程中填充。
    pub session_issues: Vec<crate::codex_session_chain::CodexSessionIssue>,
    /// true 表示本次 apply 因检测到可修复的会话断链而未执行，前端应引导修复或强制继续。
    pub session_issues_blocked: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveCodexProfileRequest {
    pub profile: CodexProfile,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub active_profile_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteCodexProfileRequest {
    pub profile_id: String,
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub active_profile_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexLaunchContext {
    pub managed_profile_name: String,
    pub model_provider: String,
    pub env_vars: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct CodexRuntimeContext {
    pub profile_name: Option<String>,
    pub model_provider: String,
    pub env_vars: BTreeMap<String, String>,
    pub cache_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyCodexProfileRequest {
    pub profile_id: String,
    #[serde(default)]
    pub apply_to_global: bool,
    /// 用户在断链提示中选择"仍然切换"后置 true，跳过预检拦截。
    #[serde(default)]
    pub allow_session_issues: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchCodexModelsRequest {
    #[serde(default)]
    pub profile_id: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default = "default_env_key")]
    pub env_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ManagedGlobalEnv {
    key: String,
    applied_value: String,
    previous_value: Option<String>,
}

fn codex_data_dir() -> Result<PathBuf, String> {
    Ok(crate::app_paths::app_data_dir_result()?.join("codex"))
}

/// 会话修复备份根目录。必须位于 CODEX_HOME 的 sessions/ 之外，
/// 否则备份文件会被会话扫描再次计入。
pub(crate) fn codex_session_repair_backup_root() -> Result<PathBuf, String> {
    Ok(codex_data_dir()?.join("session-repair-backups"))
}

fn profiles_path() -> Result<PathBuf, String> {
    Ok(codex_data_dir()?.join("profiles.json"))
}

#[cfg(windows)]
fn credentials_dir() -> Result<PathBuf, String> {
    Ok(codex_data_dir()?.join("credentials"))
}

#[cfg(target_os = "macos")]
fn plaintext_credentials_path() -> Result<PathBuf, String> {
    Ok(codex_data_dir()?.join("credentials.json"))
}

fn global_env_path() -> Result<PathBuf, String> {
    Ok(codex_data_dir()?.join("global-env.bin"))
}

fn global_codex_home_env_path() -> Result<PathBuf, String> {
    Ok(codex_data_dir()?.join("global-codex-home-env.bin"))
}

fn legacy_managed_homes_dir() -> Result<PathBuf, String> {
    Ok(codex_data_dir()?.join("homes"))
}

fn legacy_profile_home(profile_id: &str) -> Result<PathBuf, String> {
    if profile_id.is_empty()
        || !profile_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
        })
    {
        return Err("CodeX profile ID 含有不支持的字符".to_string());
    }
    Ok(legacy_managed_homes_dir()?.join(profile_id))
}

fn path_is_legacy_profile_home(path: &Path) -> bool {
    legacy_managed_homes_dir().is_ok_and(|homes| path.starts_with(homes))
}

fn codex_home() -> Result<PathBuf, String> {
    // Once a profile has been synchronized for the desktop app, this process
    // may itself inherit that managed CODEX_HOME on the next launch. Keep the
    // original user home as the migration/bootstrap source instead of
    // recursively treating one isolated profile as the shared source.
    #[cfg(windows)]
    if let Ok(Some(record)) = load_managed_global_codex_home_env() {
        if let Ok(Some(current)) = read_user_env_var(CODEX_HOME_ENV) {
            let current_path = PathBuf::from(&current);
            if current != record.applied_value
                && !current.is_empty()
                && !path_is_legacy_profile_home(&current_path)
            {
                return Ok(current_path);
            }
        }
        if let Some(previous) = record.previous_value.filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(previous));
        }
        return dirs::home_dir()
            .map(|path| path.join(".codex"))
            .ok_or_else(|| "无法确定 CODEX_HOME".to_string());
    }

    #[cfg(windows)]
    if let Ok(Some(current)) = read_user_env_var(CODEX_HOME_ENV) {
        let current_path = PathBuf::from(&current);
        if !current.is_empty() && !path_is_legacy_profile_home(&current_path) {
            return Ok(current_path);
        }
    }

    if let Some(path) = std::env::var_os(CODEX_HOME_ENV).map(PathBuf::from) {
        if !path.as_os_str().is_empty() && !path_is_legacy_profile_home(&path) {
            return Ok(path);
        }
    }
    dirs::home_dir()
        .map(|path| path.join(".codex"))
        .ok_or_else(|| "无法确定 CODEX_HOME".to_string())
}

fn global_config_path_for_profile(profile_id: &str) -> Result<PathBuf, String> {
    let _ = profile_id;
    Ok(codex_home()?.join("config.toml"))
}

fn global_config_path() -> Result<PathBuf, String> {
    Ok(codex_home()?.join("config.toml"))
}

fn auth_path() -> Result<PathBuf, String> {
    Ok(codex_home()?.join("auth.json"))
}

fn managed_profile_name(profile_id: &str) -> String {
    format!(
        "{MANAGED_PROFILE_PREFIX}{}",
        profile_id.trim_start_matches("profile-")
    )
}

fn provider_id_from_profile_name(name: &str, fallback: &str) -> String {
    let mut provider_id = String::new();
    let mut needs_separator = false;
    for character in name.trim().chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            if needs_separator
                && !provider_id.is_empty()
                && !provider_id.ends_with('-')
                && !provider_id.ends_with('_')
            {
                provider_id.push('_');
            }
            provider_id.push(character);
            needs_separator = false;
        } else if !provider_id.is_empty() {
            needs_separator = true;
        }
    }
    while provider_id.ends_with('-') || provider_id.ends_with('_') {
        provider_id.pop();
    }
    if provider_id.is_empty() {
        return fallback.to_string();
    }
    if matches!(
        provider_id.to_ascii_lowercase().as_str(),
        "openai" | "ollama" | "lmstudio"
    ) {
        return format!("{provider_id}_custom");
    }
    provider_id
}

fn sync_provider_identity(profile: &mut CodexProfile) {
    if profile.auth_mode == CodexAuthMode::Custom {
        profile.provider_id = provider_id_from_profile_name(&profile.name, &profile.id);
        profile.provider_name = profile.name.clone();
    } else {
        profile.provider_id.clear();
        profile.provider_name.clear();
    }
}

fn managed_profile_path(profile_id: &str) -> Result<PathBuf, String> {
    Ok(shared_managed_profile_path_at(&codex_home()?, profile_id))
}

fn legacy_isolated_profile_path(profile_id: &str) -> Result<PathBuf, String> {
    Ok(legacy_profile_home(profile_id)?.join(format!(
        "{}.config.toml",
        managed_profile_name(profile_id)
    )))
}

fn legacy_managed_profile_path_at(codex_home: &Path, profile_id: &str) -> PathBuf {
    codex_home.join(format!(
        "{}{}.config.toml",
        LEGACY_MANAGED_PROFILE_PREFIX,
        profile_id.trim_start_matches("profile-")
    ))
}

fn shared_managed_profile_path_at(codex_home: &Path, profile_id: &str) -> PathBuf {
    codex_home.join(format!("{}.config.toml", managed_profile_name(profile_id)))
}

fn copy_valid_toml_if_missing(source: &Path, target: &Path, label: &str) -> Result<(), String> {
    if target.exists() || !source.exists() {
        return Ok(());
    }
    let bytes = fs::read(source)
        .map_err(|error| format!("无法读取{label} {}：{error}", source.display()))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("{label}不是 UTF-8：{error}"))?;
    if let Err(error) = DocumentMut::from_str(text) {
        if target.exists() {
            eprintln!("{label}无法解析，保留现有共享文件 {}：{error}", target.display());
            return Ok(());
        }
        return Err(format!("{label}无法解析：{error}"));
    }
    write_toml_atomic(target, &bytes)
}

fn source_is_newer(source: &Path, target: &Path) -> bool {
    if !target.exists() {
        return true;
    }
    let Ok(source_modified) = fs::metadata(source).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    let Ok(target_modified) = fs::metadata(target).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    source_modified > target_modified
}

fn copy_valid_toml_if_newer(source: &Path, target: &Path, label: &str) -> Result<(), String> {
    if !source.exists() || !source_is_newer(source, target) {
        return Ok(());
    }
    let bytes = fs::read(source)
        .map_err(|error| format!("无法读取{label} {}：{error}", source.display()))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("{label}不是 UTF-8：{error}"))?;
    if let Err(error) = DocumentMut::from_str(text) {
        if target.exists() {
            eprintln!("{label}无法解析，保留现有共享文件 {}：{error}", target.display());
            return Ok(());
        }
        return Err(format!("{label}无法解析：{error}"));
    }
    write_toml_atomic(target, &bytes)
}

fn prepare_shared_managed_profile(profile_id: &str) -> Result<(), String> {
    let shared_home = codex_home()?;
    let target = shared_managed_profile_path_at(&shared_home, profile_id);
    let isolated = legacy_isolated_profile_path(profile_id)?;
    copy_valid_toml_if_newer(&isolated, &target, "隔离的 CodeX profile")?;
    if target.exists() {
        return Ok(());
    }
    let legacy = legacy_managed_profile_path_at(&shared_home, profile_id);
    copy_valid_toml_if_missing(&legacy, &target, "旧版 CodeX profile")
}

fn migrate_legacy_global_config(
    shared_home: &Path,
    managed_home: Option<&ManagedGlobalEnv>,
) -> Result<(), String> {
    let Some(managed_home) = managed_home else {
        return Ok(());
    };
    let legacy_home = PathBuf::from(&managed_home.applied_value);
    if !path_is_legacy_profile_home(&legacy_home) {
        return Ok(());
    }
    let source = legacy_home.join("config.toml");
    let target = shared_home.join("config.toml");
    if !source.exists() || !source_is_newer(&source, &target) {
        return Ok(());
    }
    let raw = fs::read_to_string(&source)
        .map_err(|error| format!("无法读取隔离的全局 CodeX 配置：{error}"))?;
    let mut document = match DocumentMut::from_str(&raw) {
        Ok(document) => document,
        Err(error) if target.exists() => {
            eprintln!(
                "隔离的全局 CodeX 配置无法解析，保留共享配置 {}：{error}",
                target.display()
            );
            return Ok(());
        }
        Err(error) => return Err(format!("隔离的全局 CodeX 配置无法解析：{error}")),
    };
    if document
        .get("sqlite_home")
        .and_then(Item::as_value)
        .and_then(|item| item.as_str())
        .is_some_and(|path| path_is_legacy_profile_home(Path::new(path)))
    {
        document.as_table_mut().remove("sqlite_home");
    }
    write_toml_atomic(&target, document.to_string().as_bytes())
}

#[cfg(test)]
fn migrate_legacy_managed_profile_at(
    profile_id: &str,
    target: &Path,
    codex_home: &Path,
) -> Result<(), String> {
    if target.exists() {
        return Ok(());
    }
    let legacy = legacy_managed_profile_path_at(codex_home, profile_id);
    if !legacy.exists() {
        return Ok(());
    }
    let bytes = fs::read(&legacy)
        .map_err(|error| format!("无法读取旧版 CodeX profile {}：{error}", legacy.display()))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("旧版 CodeX profile 不是 UTF-8：{error}"))?;
    DocumentMut::from_str(text)
        .map_err(|error| format!("旧版 CodeX profile 无法解析：{error}"))?;
    write_toml_atomic(target, &bytes)?;
    let migrated = fs::read(target)
        .map_err(|error| format!("无法回读迁移后的 CodeX profile：{error}"))?;
    if migrated != bytes {
        return Err("旧版 CodeX profile 迁移后回读不一致".to_string());
    }
    remove_if_exists(&legacy)?;
    remove_transaction_sidecars(&legacy)?;
    Ok(())
}

fn copy_json_file_if_newer(source: &Path, target: &Path, label: &str) -> Result<(), String> {
    if !source.exists() || !source_is_newer(source, target) {
        return Ok(());
    }
    let bytes = fs::read(source)
        .map_err(|error| format!("无法读取{label} {}：{error}", source.display()))?;
    serde_json::from_slice::<Value>(&bytes)
        .map_err(|error| format!("{label}无法解析：{error}"))?;
    #[cfg(target_os = "macos")]
    {
        return write_private_json_atomic(target, &bytes, label);
    }
    #[cfg(not(target_os = "macos"))]
    write_json_atomic(target, &bytes, label)
}

fn migrate_latest_official_auth(state: &CodexProfileState) -> Result<(), String> {
    let shared_auth = codex_home()?.join("auth.json");
    let latest = state
        .profiles
        .iter()
        .filter(|profile| profile.auth_mode == CodexAuthMode::Official)
        .map(|profile| legacy_profile_home(&profile.id).map(|home| home.join("auth.json")))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|path| path.is_file())
        .filter(|path| {
            fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_some()
        })
        .max_by_key(|path| fs::metadata(path).and_then(|metadata| metadata.modified()).ok());
    if let Some(latest) = latest {
        copy_json_file_if_newer(&latest, &shared_auth, "CodeX 登录凭据")?;
    }
    Ok(())
}

fn merge_desktop_project_state(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
) -> bool {
    let mut changed = false;

    for key in DESKTOP_PROJECT_ARRAY_KEYS {
        let Some(source_items) = source.get(key).and_then(Value::as_array) else {
            continue;
        };
        match target.get_mut(key) {
            Some(Value::Array(target_items)) => {
                for item in source_items {
                    if !target_items.contains(item) {
                        target_items.push(item.clone());
                        changed = true;
                    }
                }
            }
            Some(Value::Null) | None => {
                target.insert(key.to_string(), Value::Array(source_items.clone()));
                changed = true;
            }
            Some(_) => {}
        }
    }

    for key in DESKTOP_PROJECT_OBJECT_KEYS {
        let Some(source_entries) = source.get(key).and_then(Value::as_object) else {
            continue;
        };
        match target.get_mut(key) {
            Some(Value::Object(target_entries)) => {
                for (entry_key, entry_value) in source_entries {
                    if !target_entries.contains_key(entry_key) {
                        target_entries.insert(entry_key.clone(), entry_value.clone());
                        changed = true;
                    }
                }
            }
            Some(Value::Null) | None => {
                target.insert(key.to_string(), Value::Object(source_entries.clone()));
                changed = true;
            }
            Some(_) => {}
        }
    }

    for key in DESKTOP_PROJECT_VALUE_KEYS {
        let Some(source_value) = source.get(key) else {
            continue;
        };
        let should_copy = match target.get(key) {
            None | Some(Value::Null) => true,
            Some(Value::Array(items)) => items.is_empty(),
            Some(Value::Object(entries)) => entries.is_empty(),
            Some(Value::String(value)) => value.is_empty(),
            Some(_) => false,
        };
        if should_copy {
            target.insert(key.to_string(), source_value.clone());
            changed = true;
        }
    }

    changed
}

fn merge_legacy_desktop_project_state(
    legacy_home: &Path,
    shared_home: &Path,
) -> Result<bool, String> {
    if shared_home == legacy_home {
        return Ok(true);
    }
    let source_path = legacy_home.join(CODEX_DESKTOP_STATE_FILE);
    if !source_path.exists() {
        return Ok(true);
    }
    let source_bytes = fs::read(&source_path).map_err(|error| {
        format!(
            "无法读取旧 CodeX 桌面项目状态 {}：{error}",
            source_path.display()
        )
    })?;
    let source_value: Value = serde_json::from_slice(&source_bytes)
        .map_err(|error| format!("旧 CodeX 桌面项目状态无法解析：{error}"))?;
    let source = source_value
        .as_object()
        .ok_or_else(|| "旧 CodeX 桌面项目状态不是 JSON 对象".to_string())?;

    let target_path = shared_home.join(CODEX_DESKTOP_STATE_FILE);
    let target_existed = target_path.exists();
    let mut target_value = if target_existed {
        let target_bytes = match fs::read(&target_path) {
            Ok(bytes) => bytes,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::WouldBlock
                ) =>
            {
                // Codex Desktop holds this file open while it is running. Do
                // not make profile loading fail; leave the marker absent so a
                // later launcher refresh can retry after Desktop exits.
                eprintln!(
                    "CodeX 桌面项目状态正在使用，暂缓迁移 {}：{error}",
                    target_path.display()
                );
                return Ok(false);
            }
            Err(error) => {
                return Err(format!(
                    "无法读取共享的 CodeX 桌面项目状态 {}：{error}",
                    target_path.display()
                ));
            }
        };
        serde_json::from_slice(&target_bytes)
            .map_err(|error| format!("共享的 CodeX 桌面项目状态无法解析：{error}"))?
    } else {
        Value::Object(Map::new())
    };
    let target = target_value
        .as_object_mut()
        .ok_or_else(|| "隔离的 CodeX 桌面项目状态不是 JSON 对象".to_string())?;

    if merge_desktop_project_state(source, target) {
        let bytes = serde_json::to_vec(&target_value)
            .map_err(|error| format!("无法序列化 CodeX 桌面项目状态：{error}"))?;
        if let Err(error) = write_json_atomic(&target_path, &bytes, "CodeX 桌面项目状态") {
            if target_existed {
                // A running Desktop process can allow reads but deny the
                // atomic rename. Retry later without breaking profile loading.
                eprintln!(
                    "CodeX 桌面项目状态暂时无法写入，稍后重试 {}：{error}",
                    target_path.display()
                );
                return Ok(false);
            }
            return Err(error);
        }
    }

    Ok(true)
}

fn desktop_shared_migration_marker(profile_id: &str) -> Result<PathBuf, String> {
    Ok(codex_data_dir()?
        .join("migrations")
        .join("shared-home-v1")
        .join(format!("{profile_id}.json")))
}

/// 「遗留 home → 共享 home」会话迁移的完成标记。
///
/// 遗留 home 是一份冻结的旧数据：现在只有迁移和 auth 探测会读它，没有任何
/// 写入方。迁移本身又是一次性的——迁完之后再跑一遍只会把两边的目录树重新
/// 遍历一次，什么都不会做。而它偏偏挂在 `prepare_profiles_for_load` 上，也就是
/// 每次「加载 CodeX 页面 / 保存 / 应用」都会跑：实测 6 个 profile（三个遗留
/// home 各有 2000 个左右的会话文件）要 2.2 秒，其中 1.1 秒是把与 profile 无关的
/// `~/.codex/sessions` 重复扫了 12 遍、另 1.1 秒是重走已迁完的遗留 home。
///
/// 所以这里沿用旁边那份桌面状态迁移标记的做法：迁完一个 profile 就落一个标记，
/// 之后的加载直接跳过它。标记只在整趟成功之后才写，中途失败不会把未完成的迁移
/// 标成已完成。
///
/// 唯一的行为差异：共享端彻底删掉某个会话后，旧实现会在下次加载时从遗留 home
/// 把它搬回来，现在不会——共享 home 是权威、不回灌，本就与 `migrate_rollout_tree`
/// 里那套「已存在的线程不复活」的约定一致。
fn rollout_shared_migration_marker(profile_id: &str) -> Result<PathBuf, String> {
    Ok(codex_data_dir()?
        .join("migrations")
        .join("shared-home-v1")
        .join(format!("{profile_id}.rollouts.json")))
}

/// 共享 home 里已有的线程 id（sessions + archived_sessions）。
fn shared_rollout_thread_ids(shared_home: &Path) -> Result<HashSet<String>, String> {
    let mut ids = collect_rollout_ids(&shared_home.join("sessions"))?;
    ids.extend(collect_rollout_ids(&shared_home.join("archived_sessions"))?);
    Ok(ids)
}

/// 迁移后校验：被迁移的线程必须真的出现在共享端。
///
/// 没有任何迁移时直接返回，不读目录——这条最常走，不能为它扫一遍共享 home。
fn verify_migrated_rollouts(
    shared_home: &Path,
    migrated_ids: &HashSet<String>,
) -> Result<(), String> {
    if migrated_ids.is_empty() {
        return Ok(());
    }
    let shared_ids = shared_rollout_thread_ids(shared_home)?;
    let missing = migrated_ids
        .difference(&shared_ids)
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "CodeX 共享会话迁移校验失败，{} 个会话未出现在共享目录",
            missing.len()
        ));
    }
    Ok(())
}

fn copy_file_if_missing(source: &Path, target: &Path) -> Result<(), String> {
    if target.exists() || !source.is_file() {
        return Ok(());
    }
    let parent = target
        .parent()
        .ok_or_else(|| format!("迁移目标没有父目录：{}", target.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("无法创建 CodeX 会话迁移目录：{error}"))?;
    let temporary = sidecar_path(target, "migration-tmp");
    if temporary.exists() {
        remove_if_exists(&temporary)?;
    }
    fs::copy(source, &temporary)
        .map_err(|error| format!("无法复制 CodeX 会话 {}：{error}", source.display()))?;
    match fs::rename(&temporary, target) {
        Ok(()) => Ok(()),
        Err(_) if target.exists() => remove_if_exists(&temporary),
        Err(error) => {
            let _ = remove_if_exists(&temporary);
            Err(format!("无法发布迁移后的 CodeX 会话：{error}"))
        }
    }
}

fn rollout_identity(path: &Path) -> Option<(String, String)> {
    let file = fs::File::open(path).ok()?;
    let first_line = BufReader::new(file).lines().next()?.ok()?;
    let envelope: Value = serde_json::from_str(first_line.trim_start_matches('\u{feff}')).ok()?;
    if envelope.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let payload = envelope.get("payload")?;
    let id = payload.get("id")?.as_str()?.trim();
    if id.is_empty() {
        return None;
    }
    let provider = payload
        .get("model_provider")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("openai");
    Some((id.to_string(), provider.to_string()))
}

fn migration_backup_path(
    backup_root: &Path,
    directory: &str,
    relative: &Path,
    suffix: &str,
) -> Result<PathBuf, String> {
    let filename = relative
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "CodeX 会话备份文件名不是 UTF-8".to_string())?;
    let backup_name = format!("{filename}.{suffix}");
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    Ok(backup_root
        .join(directory)
        .join(parent)
        .join(backup_name))
}

fn merge_rollout_file(
    source: &Path,
    target: &Path,
    profile_id: &str,
    backup_root: &Path,
    directory: &str,
    relative: &Path,
) -> Result<(), String> {
    if !target.exists() {
        return copy_file_if_missing(source, target);
    }
    let source_bytes = fs::read(source)
        .map_err(|error| format!("无法读取隔离的 CodeX 会话 {}：{error}", source.display()))?;
    let target_bytes = fs::read(target)
        .map_err(|error| format!("无法读取共享的 CodeX 会话 {}：{error}", target.display()))?;
    if source_bytes == target_bytes || source_bytes.starts_with(&target_bytes) {
        if source_bytes.len() > target_bytes.len() {
            return write_raw_atomic(target, &source_bytes, "CodeX 共享会话");
        }
        return Ok(());
    }
    if target_bytes.starts_with(&source_bytes) {
        return Ok(());
    }

    let source_id = rollout_identity(source).map(|value| value.0);
    let target_id = rollout_identity(target).map(|value| value.0);
    if source_id.is_some() && target_id.is_some() && source_id != target_id {
        let id = source_id.expect("checked source id");
        let alternate = target.with_file_name(format!("migrated-{profile_id}-{id}.jsonl"));
        return copy_file_if_missing(source, &alternate);
    }

    // 同一线程的会话文件发生分叉时，共享 home 是权威：保留共享侧，
    // 隔离侧进备份。共享侧可能是修复后的合并文件（更短但更完整），
    // 隔离 home 只是冻结存档，没有任何读取方再写它。
    let backup = migration_backup_path(backup_root, directory, relative, "isolated.jsonl")?;
    copy_file_if_missing(source, &backup)
}

fn migrate_rollout_tree(
    source_root: &Path,
    target_root: &Path,
    profile_id: &str,
    backup_root: &Path,
    directory_name: &str,
    migrated_ids: &mut HashSet<String>,
    known_thread_ids: &HashSet<String>,
) -> Result<(), String> {
    if !source_root.is_dir() {
        return Ok(());
    }
    let mut pending = vec![source_root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|error| {
            format!("无法读取 CodeX 会话目录 {}：{error}", directory.display())
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("无法读取 CodeX 会话条目：{error}"))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|error| format!("无法读取 CodeX 会话类型：{error}"))?;
            if file_type.is_dir() {
                pending.push(path);
                continue;
            }
            if !file_type.is_file() || path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let Some((id, _provider)) = rollout_identity(&path) else {
                continue;
            };
            let relative = path.strip_prefix(source_root).map_err(|error| {
                format!("无法计算 CodeX 会话相对路径：{error}")
            })?;
            let target = target_root.join(relative);
            // 共享端已存在该线程（任意文件）而目标路径缺失：说明这个文件是
            // 被会话修复移除的分页/旧副本，或共享端早已分叉。共享 home 是权威，
            // 不回灌，否则修复后的断链会反复复活。
            if !target.exists() && known_thread_ids.contains(&id) {
                continue;
            }
            merge_rollout_file(
                &path,
                &target,
                profile_id,
                backup_root,
                directory_name,
                relative,
            )?;
            migrated_ids.insert(id);
        }
    }
    Ok(())
}

fn collect_rollout_ids(root: &Path) -> Result<HashSet<String>, String> {
    let mut ids = HashSet::new();
    if !root.is_dir() {
        return Ok(ids);
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("无法校验 CodeX 会话目录 {}：{error}", directory.display()))?
        {
            let entry = entry.map_err(|error| format!("无法校验 CodeX 会话条目：{error}"))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|error| format!("无法校验 CodeX 会话类型：{error}"))?;
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file()
                && path.extension().and_then(|value| value.to_str()) == Some("jsonl")
            {
                if let Some((id, _)) = rollout_identity(&path) {
                    ids.insert(id);
                }
            }
        }
    }
    Ok(ids)
}

fn merge_jsonl(
    source: &Path,
    target: &Path,
    label: &str,
) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    let mut lines = Vec::new();
    let mut seen = HashSet::new();
    if target.exists() {
        let file = fs::File::open(target)
            .map_err(|error| format!("无法读取迁移后的 CodeX 索引：{error}"))?;
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| format!("无法读取 CodeX 索引行：{error}"))?;
            if seen.insert(line.clone()) {
                lines.push(line);
            }
        }
    }
    let file = fs::File::open(source)
        .map_err(|error| format!("无法读取旧 CodeX 索引 {}：{error}", source.display()))?;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| format!("无法读取旧 CodeX 索引行：{error}"))?;
        if seen.insert(line.clone()) {
            lines.push(line);
        }
    }
    let mut bytes = lines.join("\n").into_bytes();
    if !bytes.is_empty() {
        bytes.push(b'\n');
    }
    if fs::read(target).ok().as_deref() == Some(bytes.as_slice()) {
        return Ok(());
    }
    write_raw_atomic(target, &bytes, label)
}

/// 把遗留 home 的会话并进共享 home。
///
/// `shared_thread_ids` 是「共享端已有线程」的按需缓存，由调用方持有：它与 profile
/// 无关，一次加载里所有 profile 共用一份。早先的实现为每个 profile 各扫两遍
/// `~/.codex/sessions`（六个 profile 实测约 1.1 秒，全是重复劳动）。
fn prepare_shared_profile_storage(
    profile: &CodexProfile,
    shared_thread_ids: &mut Option<HashSet<String>>,
) -> Result<(), String> {
    let shared_home = codex_home()?;
    fs::create_dir_all(&shared_home)
        .map_err(|error| format!("无法创建 CodeX 共享目录：{error}"))?;
    prepare_shared_managed_profile(&profile.id)?;

    let isolated_home = legacy_profile_home(&profile.id)?;
    if !isolated_home.is_dir() {
        return Ok(());
    }

    // Only project catalog fields are merged. Thread assignments and drafts
    // remain untouched; rollouts below are the canonical, lossless source.
    // The marker lives outside the legacy home so that every old home remains
    // a byte-for-byte recovery source.
    let desktop_marker = desktop_shared_migration_marker(&profile.id)?;
    if !desktop_marker.exists() {
        match merge_legacy_desktop_project_state(&isolated_home, &shared_home) {
            Ok(true) => write_json_atomic(
                &desktop_marker,
                br#"{"version":1}"#,
                "CodeX 共享桌面项目状态迁移标记",
            )?,
            Ok(false) => {}
            Err(error) => eprintln!("CodeX 桌面项目状态迁移已暂缓：{error}"),
        }
    }

    migrate_legacy_home_rollouts(
        &profile.id,
        &isolated_home,
        &shared_home,
        &codex_data_dir()?
            .join("migration-backups")
            .join("shared-home-v1")
            .join(&profile.id),
        shared_thread_ids,
        &rollout_shared_migration_marker(&profile.id)?,
    )
}

/// 会话迁移本体（路径全部由调用方给出，便于按临时目录驱动测试）。
///
/// 已经迁完的 profile 在这里直接返回：遗留 home 此后不会再变，而重走一遍两个
/// 目录树、再扫一遍共享端，正是加载/保存/应用变慢的主因。
fn migrate_legacy_home_rollouts(
    profile_id: &str,
    isolated_home: &Path,
    shared_home: &Path,
    backup_root: &Path,
    shared_thread_ids: &mut Option<HashSet<String>>,
    rollout_marker: &Path,
) -> Result<(), String> {
    if rollout_marker.exists() {
        return Ok(());
    }

    // 共享端已存在的线程集合：迁移只补齐共享端完全没有的线程，
    // 已有线程的缺失文件（例如修复时被移除的分页）不得回灌。
    if shared_thread_ids.is_none() {
        *shared_thread_ids = Some(shared_rollout_thread_ids(shared_home)?);
    }
    let mut migrated_ids = HashSet::new();
    {
        let known_thread_ids = shared_thread_ids.as_ref().expect("filled above");
        for directory in ["sessions", "archived_sessions"] {
            migrate_rollout_tree(
                &isolated_home.join(directory),
                &shared_home.join(directory),
                profile_id,
                backup_root,
                directory,
                &mut migrated_ids,
                known_thread_ids,
            )?;
        }
    }
    // 把刚迁进来的线程并回缓存，供同一轮加载里后面的 profile 使用：它们此刻
    // 确实已经在共享端了，不并进去就等于让下一个 profile 看到一份过时的快照，
    // 从而把本该「已存在、不回灌」的文件重新灌一遍。
    if let Some(ids) = shared_thread_ids.as_mut() {
        ids.extend(migrated_ids.iter().cloned());
    }
    merge_jsonl(
        &isolated_home.join("session_index.jsonl"),
        &shared_home.join("session_index.jsonl"),
        "CodeX 共享会话索引",
    )?;
    merge_jsonl(
        &isolated_home.join("history.jsonl"),
        &shared_home.join("history.jsonl"),
        "CodeX 共享提示历史",
    )?;

    // 先校验再落标记：标记意味着「这一个 profile 不会再迁了」，所以校验失败时
    // 绝不能留下标记，否则这次失败再也不会被复查。
    verify_migrated_rollouts(shared_home, &migrated_ids)?;
    write_json_atomic(rollout_marker, br#"{"version":1}"#, "CodeX 共享会话迁移标记")
}

fn managed_model_catalog_path(profile_id: &str) -> Result<PathBuf, String> {
    Ok(managed_model_catalog_path_at(&codex_home()?, profile_id))
}

fn managed_model_catalog_path_at(codex_home: &Path, profile_id: &str) -> PathBuf {
    codex_home.join(format!("{}.models.json", managed_profile_name(profile_id)))
}

#[cfg(windows)]
fn credential_path(profile_id: &str) -> Result<PathBuf, String> {
    Ok(credentials_dir()?.join(format!("{profile_id}.bin")))
}

fn custom_global_sync_supported() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

fn custom_global_key_sync_supported() -> bool {
    false
}

fn secret_storage_kind() -> &'static str {
    #[cfg(windows)]
    {
        return "windows_dpapi";
    }
    #[cfg(target_os = "macos")]
    {
        return "macos_plaintext";
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    "unsupported"
}

fn platform_name() -> &'static str {
    std::env::consts::OS
}

fn load_profile_state() -> Result<CodexProfileState, String> {
    let path = profiles_path()?;
    restore_json_backup_if_missing(&path, "CodeX 方案索引")?;
    if !path.exists() {
        return Ok(CodexProfileState::default());
    }
    let raw =
        fs::read_to_string(&path).map_err(|error| format!("无法读取 CodeX 方案索引：{error}"))?;
    serde_json::from_str(&raw).map_err(|error| format!("CodeX 方案索引无法解析：{error}"))
}

fn save_profile_state(state: &CodexProfileState) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("无法序列化 CodeX 方案索引：{error}"))?;
    write_json_atomic(&profiles_path()?, &json, "CodeX 方案索引")
}

fn validate_model_slug(slug: &str) -> Result<(), String> {
    if slug.is_empty()
        || !slug.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(format!("模型 slug '{}' 含有不支持的字符", slug));
    }
    Ok(())
}

fn normalize_model_catalog(
    mut catalog: CodexModelCatalog,
) -> Result<CodexModelCatalog, String> {
    if catalog.models.is_empty() {
        return Err("第三方模型目录至少需要配置一个模型".to_string());
    }

    let template_catalog: CodexModelCatalog = serde_json::from_str(DEEPSEEK_MODEL_CATALOG_TEMPLATE)
        .map_err(|error| format!("DeepSeek models.json template parse failed: {error}"))?;
    for (key, value) in template_catalog.extra {
        catalog.extra.entry(key).or_insert(value);
    }

    let mut slugs = HashSet::new();
    for model in &mut catalog.models {
        model.slug = model.slug.trim().to_string();
        validate_model_slug(&model.slug)?;
        if !slugs.insert(model.slug.clone()) {
            return Err(format!("模型 slug '{}' 重复", model.slug));
        }
        let template = template_catalog
            .models
            .iter()
            .find(|template| template.slug == model.slug)
            .or_else(|| template_catalog.models.first())
            .ok_or_else(|| "DeepSeek models.json template has no models".to_string())?;
        model.display_name = model.slug.clone();
        model.max_context_window = model.context_window;
        let mut modalities = Vec::with_capacity(model.input_modalities.len() + 1);
        for modality in model.input_modalities.drain(..) {
            let modality = modality.trim().to_ascii_lowercase();
            if !modality.is_empty() && !modalities.contains(&modality) {
                modalities.push(modality);
            }
        }
        if !modalities.iter().any(|modality| modality == "text") {
            modalities.insert(0, "text".to_string());
        }
        model.input_modalities = modalities;
        if model.context_window == 0 {
            return Err(format!("模型 '{}' 的 context_window 必须大于 0", model.slug));
        }
        if model.max_context_window < model.context_window {
            return Err(format!(
                "模型 '{}' 的 max_context_window 不能小于 context_window",
                model.slug
            ));
        }
        if !(1..=100).contains(&model.effective_context_window_percent) {
            return Err(format!(
                "模型 '{}' 的 effective_context_window_percent 必须在 1 到 100 之间",
                model.slug
            ));
        }
        if model.truncation_policy.is_none() {
            model.truncation_policy = template.truncation_policy.clone();
        }
        if let Some(policy) = model.truncation_policy.as_mut() {
            policy.mode = policy.mode.trim().to_string();
            if policy.mode.is_empty() || policy.limit == 0 {
                return Err(format!("模型 '{}' 的 truncation_policy 无效", model.slug));
            }
            let effective_limit = (model.context_window as u128)
                * (model.effective_context_window_percent as u128)
                / 100;
            if policy.limit as u128 > effective_limit {
                if template.truncation_policy.as_ref() == Some(policy) {
                    policy.limit = effective_limit as u64;
                } else {
                    return Err(format!(
                        "模型 '{}' 的 truncation_policy.limit 超出有效上下文范围",
                        model.slug
                    ));
                }
            }
        }
        if model.default_reasoning_level.is_empty() {
            model.default_reasoning_level = template.default_reasoning_level.clone();
        }
        if model.supported_reasoning_levels.is_empty() {
            model.supported_reasoning_levels = template.supported_reasoning_levels.clone();
        }
        for (key, value) in &template.extra {
            model.extra.entry(key.clone()).or_insert_with(|| value.clone());
        }

        let mut efforts = HashSet::new();
        for level in &mut model.supported_reasoning_levels {
            level.effort = level.effort.trim().to_string();
            if level.effort.is_empty() || !efforts.insert(level.effort.clone()) {
                return Err(format!("模型 '{}' 的推理档位无效或重复", model.slug));
            }
            level.description = level.description.trim().to_string();
        }
        model.default_reasoning_level = model.default_reasoning_level.trim().to_string();
        if !model.default_reasoning_level.is_empty()
            && !efforts.contains(&model.default_reasoning_level)
        {
            return Err(format!(
                "模型 '{}' 的默认推理档位不在 supported_reasoning_levels 中",
                model.slug
            ));
        }
    }

    // `priority` 是 Codex 真正的排序依据，数组顺序不是：`codex debug models`
    // 只是照抄数组，而选择器读的 app-server `model/list` 会按 priority 升序返回，
    // 并把排在首位的那条标成 `isDefault`（实测 Codex 0.154.0：数组 [p9, p1, p5]
    // 返回 p1, p5, p9）。所以启动器里拖动出来的顺序必须投影到 priority 上，否则
    // 配置文件长得像改了顺序，Codex 内外看到的两份顺序仍然是旧的那一份。
    //
    // 位置即名次，从 1 开始，与 DeepSeek 模板自身的取值方式一致。这同时修掉了
    // 另一个隐患：自定义 slug 的模型都从模板兜底继承同一个 priority，排序结果
    // 原本是不确定的；这里按数组位置统一重排，落盘的文件总是自洽的。
    for (index, model) in catalog.models.iter_mut().enumerate() {
        model
            .extra
            .insert("priority".to_string(), Value::Number(((index + 1) as u64).into()));
    }

    Ok(catalog)
}

fn render_model_catalog(profile: &CodexProfile) -> Result<Vec<u8>, String> {
    let catalog = profile
        .model_catalog
        .as_ref()
        .ok_or_else(|| format!("CodeX 配置 '{}' 没有模型目录定义", profile.name))?;
    let catalog = normalize_model_catalog(catalog.clone())?;
    let mut root = catalog.extra;
    let models = catalog
        .models
        .into_iter()
        .map(|model| {
            let mut object = model.extra;
            object.insert("slug".to_string(), Value::String(model.slug));
            object.insert(
                "display_name".to_string(),
                Value::String(model.display_name),
            );
            object.insert(
                "input_modalities".to_string(),
                Value::Array(
                    model
                        .input_modalities
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                ),
            );
            object.insert(
                "supports_image_detail_original".to_string(),
                Value::Bool(model.supports_image_detail_original),
            );
            object.insert(
                "context_window".to_string(),
                Value::Number(model.context_window.into()),
            );
            object.insert(
                "max_context_window".to_string(),
                Value::Number(model.max_context_window.into()),
            );
            object.insert(
                "effective_context_window_percent".to_string(),
                Value::Number((model.effective_context_window_percent as u64).into()),
            );
            if let Some(policy) = model.truncation_policy {
                let mut policy_object = policy.extra;
                policy_object.insert("mode".to_string(), Value::String(policy.mode));
                policy_object.insert(
                    "limit".to_string(),
                    Value::Number(policy.limit.into()),
                );
                object.insert("truncation_policy".to_string(), Value::Object(policy_object));
            }
            if !model.default_reasoning_level.is_empty() {
                object.insert(
                    "default_reasoning_level".to_string(),
                    Value::String(model.default_reasoning_level),
                );
            }
            if !model.supported_reasoning_levels.is_empty() {
                let levels = model
                    .supported_reasoning_levels
                    .into_iter()
                    .map(|level| {
                        let mut level_object = level.extra;
                        level_object.insert("effort".to_string(), Value::String(level.effort));
                        if !level.description.is_empty() {
                            level_object.insert(
                                "description".to_string(),
                                Value::String(level.description),
                            );
                        }
                        Value::Object(level_object)
                    })
                    .collect::<Vec<_>>();
                object.insert("supported_reasoning_levels".to_string(), Value::Array(levels));
            }
            Value::Object(object)
        })
        .collect::<Vec<_>>();
    root.insert("models".to_string(), Value::Array(models));
    let bytes = serde_json::to_vec_pretty(&Value::Object(root))
        .map_err(|error| format!("无法序列化 models.json：{error}"))?;
    validate_model_catalog_bytes(&bytes)?;
    Ok(bytes)
}

fn validate_model_catalog_bytes(bytes: &[u8]) -> Result<(), String> {
    let root: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("models.json 无法解析：{error}"))?;
    let models = root
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| "models.json 的 models 必须是数组".to_string())?;
    if models.is_empty() {
        return Err("models.json 至少需要一个模型".to_string());
    }
    Ok(())
}

fn normalize_profile(mut profile: CodexProfile) -> Result<CodexProfile, String> {
    profile.name = profile.name.trim().to_string();
    if profile.name.is_empty() {
        return Err("请输入 CodeX 配置名称".to_string());
    }
    if profile.id.trim().is_empty() {
        profile.id = format!("profile-{}", Uuid::new_v4());
    }
    if !profile
        .id
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
    {
        return Err("CodeX profile ID 含有不支持的字符".to_string());
    }
    sync_provider_identity(&mut profile);
    profile.managed_profile_name = managed_profile_name(&profile.id);
    profile.model = profile.model.trim().to_string();
    profile.reasoning_effort = profile.reasoning_effort.trim().to_string();
    if let Some(context_window) = profile.model_context_window {
        if context_window == 0 || context_window > i64::MAX as u64 {
            return Err("model_context_window 必须是正整数".to_string());
        }
        profile.model_context_window_configured = true;
    }
    if profile.model_context_window.is_none() {
        // The auto compact ratio needs an explicit context window as its
        // conversion base; clear it when the context window is empty.
        profile.model_auto_compact_ratio = None;
        profile.model_auto_compact_ratio_configured = false;
    } else if let Some(ratio) = profile.model_auto_compact_ratio {
        if !ratio.is_finite() || !(0.0..=1.0).contains(&ratio) {
            return Err("model_auto_compact_ratio 必须是 0 到 1 之间的数值".to_string());
        }
        profile.model_auto_compact_ratio = Some(stabilize_model_auto_compact_ratio(ratio));
        profile.model_auto_compact_ratio_configured = true;
    }
    profile.chat_upstream_model = profile.chat_upstream_model.trim().to_string();
    profile.prompt_cache_routing = profile.prompt_cache_routing.trim().to_ascii_lowercase();
    if profile.prompt_cache_routing.is_empty() {
        profile.prompt_cache_routing = default_prompt_cache_routing();
    }
    if !matches!(
        profile.prompt_cache_routing.as_str(),
        "auto" | "enabled" | "disabled"
    ) {
        return Err("prompt cache routing 只能是 auto、enabled 或 disabled".to_string());
    }
    profile.openai_base_url = profile.openai_base_url.trim().to_string();
    profile.base_url = profile.base_url.trim().to_string();
    profile.env_key = profile.env_key.trim().to_string();
    profile.wire_api = profile.wire_api.trim().to_string();
    if let Some(catalog) = profile.model_catalog.take() {
        let catalog = normalize_model_catalog(catalog)?;
        if profile.auth_mode == CodexAuthMode::Custom {
            if profile.model.is_empty() {
                profile.model = catalog
                    .models
                    .first()
                    .map(|model| model.slug.clone())
                    .unwrap_or_default();
            } else if !catalog
                .models
                .iter()
                .any(|model| model.slug == profile.model)
            {
                return Err(format!(
                    "默认模型 '{}' 不在 models.json 的 models 列表中",
                    profile.model
                ));
            }
        }
        profile.model_catalog = Some(catalog);
    }

    match profile.auth_mode {
        CodexAuthMode::Official => {
            if !profile.openai_base_url.is_empty() {
                validate_http_url(&profile.openai_base_url, "OpenAI Base URL")?;
            }
        }
        CodexAuthMode::Custom => {
            if profile.provider_id.is_empty() {
                return Err("请输入自定义 provider ID".to_string());
            }
            if ["openai", "ollama", "lmstudio"].contains(&profile.provider_id.as_str()) {
                return Err("自定义 provider ID 不能使用 openai、ollama 或 lmstudio".to_string());
            }
            if !profile.provider_id.chars().all(|character| {
                character.is_ascii_alphanumeric() || character == '-' || character == '_'
            }) {
                return Err("provider ID 只能包含字母、数字、短横线和下划线".to_string());
            }
            if profile.provider_name.is_empty() {
                profile.provider_name = profile.provider_id.clone();
            }
            validate_http_url(&profile.base_url, "自定义 Base URL")?;
            if profile.wire_api != "responses" {
                return Err("当前 CodeX 版本只支持 responses wire API".to_string());
            }
            if profile.env_key.is_empty()
                || !profile
                    .env_key
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
            {
                return Err("API Key 环境变量名只能包含字母、数字和下划线".to_string());
            }
            if matches!(
                profile.env_key.to_ascii_uppercase().as_str(),
                CODEX_HOME_ENV | CODEX_SQLITE_HOME_ENV
            ) {
                return Err("API Key 环境变量名不能使用 CODEX_HOME 或 CODEX_SQLITE_HOME".to_string());
            }
        }
    }
    Ok(profile)
}

fn validate_http_url(value: &str, label: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|error| format!("{label} 无效：{error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("{label} 必须使用 http 或 https"));
    }
    Ok(())
}

fn profile_rename_block_reason(
    previous_profile: Option<&CodexProfile>,
    next_profile: &CodexProfile,
    active_profile_id: Option<&str>,
    global_profile_id: Option<&str>,
) -> Option<&'static str> {
    let previous_profile = previous_profile?;
    if previous_profile.name.trim() == next_profile.name.trim() {
        return None;
    }
    let is_active = active_profile_id == Some(next_profile.id.as_str());
    let is_global = global_profile_id == Some(next_profile.id.as_str());
    match (is_active, is_global) {
        (true, true) => Some("启动器当前应用和 Codex 全局配置"),
        (true, false) => Some("启动器当前应用"),
        (false, true) => Some("Codex 全局配置"),
        (false, false) => None,
    }
}

fn profile_is_applied(
    state: &CodexProfileState,
    index: &ProfileIndexState,
    profile_id: &str,
) -> bool {
    state.global_profile_id.as_deref() == Some(profile_id)
        || index.active_profile_id.as_deref() == Some(profile_id)
}

fn set_optional_string(document: &mut DocumentMut, key: &str, value_text: &str) {
    if value_text.is_empty() {
        document.as_table_mut().remove(key);
    } else {
        document[key] = value(value_text);
    }
}

fn set_optional_u64(
    document: &mut DocumentMut,
    key: &str,
    value_number: Option<u64>,
) -> Result<(), String> {
    let Some(value_number) = value_number else {
        document.as_table_mut().remove(key);
        return Ok(());
    };
    let value_number = i64::try_from(value_number)
        .map_err(|_| format!("{key} 必须是 TOML 支持的正整数"))?;
    document[key] = value(value_number);
    Ok(())
}

fn stabilize_model_auto_compact_ratio(ratio: f64) -> f64 {
    (ratio * 1_000_000_000_000.0).round() / 1_000_000_000_000.0
}

fn model_auto_compact_token_limit(profile: &CodexProfile) -> Result<Option<u64>, String> {
    let (Some(context_window), Some(ratio)) = (
        profile.model_context_window,
        profile.model_auto_compact_ratio,
    ) else {
        return Ok(None);
    };
    if !ratio.is_finite() || !(0.0..=1.0).contains(&ratio) {
        return Err("model_auto_compact_ratio 必须是 0 到 1 之间的数值".to_string());
    }
    let token_limit = ((context_window as f64) * ratio).round();
    if !token_limit.is_finite() || !(0.0..=(i64::MAX as f64)).contains(&token_limit) {
        return Err("model_auto_compact_token_limit 换算后不是有效的 TOML 整数".to_string());
    }
    Ok(Some(token_limit as u64))
}

fn u64_setting_from_raw(
    raw: &str,
    key: &str,
    label: &str,
    allow_zero: bool,
) -> Result<Option<u64>, String> {
    let document = DocumentMut::from_str(raw)
        .map_err(|error| format!("{label}无法解析：{error}"))?;
    let Some(item) = document.get(key) else {
        return Ok(None);
    };
    let Some(value_number) = item.as_value().and_then(|item| item.as_integer()) else {
        return Err(format!(
            "{label}的 {key} 必须是{}",
            if allow_zero { "非负整数" } else { "正整数" }
        ));
    };
    if value_number < 0 || (!allow_zero && value_number == 0) {
        return Err(format!(
            "{label}的 {key} 必须是{}",
            if allow_zero { "非负整数" } else { "正整数" }
        ));
    }
    Ok(Some(value_number as u64))
}

fn model_context_window_from_raw(raw: &str, label: &str) -> Result<Option<u64>, String> {
    u64_setting_from_raw(raw, "model_context_window", label, false)
}

fn model_auto_compact_token_limit_from_raw(
    raw: &str,
    label: &str,
) -> Result<Option<u64>, String> {
    u64_setting_from_raw(raw, "model_auto_compact_token_limit", label, true)
}

fn model_auto_compact_ratio_from_token_limit(
    token_limit: u64,
    context_window: u64,
) -> Option<f64> {
    if context_window == 0 {
        return None;
    }
    let ratio = token_limit as f64 / context_window as f64;
    if !ratio.is_finite() || !(0.0..=1.0).contains(&ratio) {
        return None;
    }
    // TOML stores the converted integer threshold. Stabilize the reverse
    // conversion so values such as 0.8 do not come back as a float tail.
    Some(stabilize_model_auto_compact_ratio(ratio))
}

fn model_catalog_json_value(document: &DocumentMut) -> Option<String> {
    document
        .get("model_catalog_json")
        .and_then(Item::as_value)
        .and_then(|item| item.as_str())
        .map(str::to_string)
}

fn model_catalog_json_value_from_raw(raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let document = DocumentMut::from_str(raw)
        .map_err(|error| format!("现有 CodeX config.toml 无法解析：{error}"))?;
    Ok(model_catalog_json_value(&document))
}

fn validate_managed_profile_model_catalog_reference(
    raw: Option<&str>,
    profile_id: &str,
    previous_value: &Option<String>,
    managing: bool,
) -> Result<(), String> {
    let Some(raw) = raw else {
        return Ok(());
    };
    let current = model_catalog_json_value_from_raw(Some(raw))?;
    let expected = Some(
        managed_model_catalog_path(profile_id)?
            .to_string_lossy()
            .to_string(),
    );
    let matches = if managing {
        current == expected
    } else {
        current == expected || current == previous_value.clone()
    };
    if matches {
        Ok(())
    } else {
        Err(format!(
            "CodeX profile '{}' 的 model_catalog_json 已被外部修改，拒绝覆盖",
            profile_id
        ))
    }
}

fn validate_managed_global_model_catalog_reference(
    raw: Option<&str>,
    managed: &ManagedGlobalModelCatalogState,
    managing: bool,
) -> Result<(), String> {
    let Some(raw) = raw else {
        return Ok(());
    };
    let current = model_catalog_json_value_from_raw(Some(raw))?;
    let applied = Some(managed.applied_value.clone());
    let matches = if managing {
        current == applied
    } else {
        current == applied || current == managed.previous_value.clone()
    };
    if matches {
        Ok(())
    } else {
        Err("CodeX 全局 config.toml 的 model_catalog_json 已被外部修改，拒绝覆盖".to_string())
    }
}

fn remove_provider(document: &mut DocumentMut, provider_id: &str) {
    if provider_id.is_empty() {
        return;
    }
    let mut remove_container = false;
    if let Some(providers) = document
        .get_mut("model_providers")
        .and_then(Item::as_table_mut)
    {
        providers.remove(provider_id);
        remove_container = providers.is_empty();
    }
    if remove_container {
        document.as_table_mut().remove("model_providers");
    }
}

fn uses_command_auth(profile: &CodexProfile) -> bool {
    cfg!(any(windows, target_os = "macos"))
        && profile.auth_mode == CodexAuthMode::Custom
        && profile.has_stored_api_key
}

#[cfg(windows)]
fn windows_dpapi_auth_script(path: &Path) -> String {
    let escaped_path = path.to_string_lossy().replace('\'', "''");
    format!(
        "Add-Type -AssemblyName System.Security;$path='{escaped_path}';$bytes=[IO.File]::ReadAllBytes($path);$plain=[System.Security.Cryptography.ProtectedData]::Unprotect($bytes,$null,[System.Security.Cryptography.DataProtectionScope]::CurrentUser);[Console]::Out.Write([Text.Encoding]::UTF8.GetString($plain))"
    )
}

fn configure_provider_credentials(
    provider: &mut Table,
    profile: &CodexProfile,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if uses_command_auth(profile) {
        provider.remove("env_key");
        let mut args = Array::new();
        let credentials_path = plaintext_credentials_path()?;
        for argument in ["-extract", profile.id.as_str(), "raw"] {
            args.push(argument);
        }
        args.push(credentials_path.to_string_lossy().as_ref());
        let mut auth = Table::new();
        auth["command"] = value("/usr/bin/plutil");
        auth["args"] = Item::Value(TomlValue::Array(args));
        auth["timeout_ms"] = value(10_000_i64);
        auth["refresh_interval_ms"] = value(0_i64);
        provider["auth"] = Item::Table(auth);
        return Ok(());
    }

    #[cfg(windows)]
    if uses_command_auth(profile) {
        provider.remove("env_key");
        let mut args = Array::new();
        for argument in ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"] {
            args.push(argument);
        }
        let script = windows_dpapi_auth_script(&credential_path(&profile.id)?);
        args.push(script);
        let mut auth = Table::new();
        auth["command"] = value("powershell.exe");
        auth["args"] = Item::Value(TomlValue::Array(args));
        auth["timeout_ms"] = value(10_000_i64);
        auth["refresh_interval_ms"] = value(0_i64);
        provider["auth"] = Item::Table(auth);
        return Ok(());
    }

    provider["env_key"] = value(&profile.env_key);
    provider.remove("auth");
    Ok(())
}

fn build_codex_toml_with_model_catalog_restore(
    existing: Option<&str>,
    previous_managed_provider_id: Option<&str>,
    profile: &CodexProfile,
    restored_model_catalog_json: Option<&Option<String>>,
    remove_previous_provider: bool,
) -> Result<String, String> {
    let mut document = match existing {
        Some(raw) => DocumentMut::from_str(raw)
            .map_err(|error| format!("现有 CodeX profile TOML 无法解析：{error}"))?,
        None => DocumentMut::new(),
    };

    set_optional_string(&mut document, "model", &profile.model);
    set_optional_string(
        &mut document,
        "model_reasoning_effort",
        &profile.reasoning_effort,
    );
    // The two keys derive from each other (limit = window x ratio), so
    // whichever context setting the profile manages, both keys must stay
    // in sync with the profile state.
    let context_settings_managed = profile.auth_mode == CodexAuthMode::Official
        || profile.model_context_window_configured
        || profile.model_context_window.is_some()
        || profile.model_auto_compact_ratio_configured
        || profile.model_auto_compact_ratio.is_some();
    if context_settings_managed {
        set_optional_u64(
            &mut document,
            "model_context_window",
            profile.model_context_window,
        )?;
        set_optional_u64(
            &mut document,
            "model_auto_compact_token_limit",
            model_auto_compact_token_limit(profile)?,
        )?;
    }
    if document
        .get("sqlite_home")
        .and_then(Item::as_value)
        .and_then(|item| item.as_str())
        .is_some_and(|path| path_is_legacy_profile_home(Path::new(path)))
    {
        document.as_table_mut().remove("sqlite_home");
    }
    let model_catalog_path = if profile.auth_mode == CodexAuthMode::Custom
        && profile.model_catalog.is_some()
    {
        managed_model_catalog_path(&profile.id)?
            .to_string_lossy()
            .to_string()
    } else {
        restored_model_catalog_json
            .and_then(|value| value.as_deref())
            .unwrap_or_default()
            .to_string()
    };
    set_optional_string(&mut document, "model_catalog_json", &model_catalog_path);

    if remove_previous_provider {
        if let Some(previous_provider_id) = previous_managed_provider_id {
            if profile.auth_mode != CodexAuthMode::Custom
                || previous_provider_id != profile.provider_id
            {
                remove_provider(&mut document, previous_provider_id);
            }
        }
    }

    match profile.auth_mode {
        CodexAuthMode::Official => {
            document["model_provider"] = value("openai");
            set_optional_string(&mut document, "openai_base_url", &profile.openai_base_url);
        }
        CodexAuthMode::Custom => {
            document.as_table_mut().remove("openai_base_url");
            document["model_provider"] = value(&profile.provider_id);
            let providers = document
                .as_table_mut()
                .entry("model_providers")
                .or_insert(Item::Table(Table::new()))
                .as_table_mut()
                .ok_or_else(|| "model_providers 不是 TOML 表".to_string())?;
            if !providers.contains_key(&profile.provider_id) {
                providers.insert(&profile.provider_id, Item::Table(Table::new()));
            }
            let provider = providers
                .get_mut(&profile.provider_id)
                .and_then(Item::as_table_mut)
                .ok_or_else(|| format!("provider '{}' 不是 TOML 表", profile.provider_id))?;
            provider["name"] = value(&profile.provider_name);
            provider["base_url"] = value(&profile.base_url);
            provider["wire_api"] = value("responses");
            configure_provider_credentials(provider, profile)?;
            provider.remove("requires_openai_auth");
            provider.remove("experimental_bearer_token");
        }
    }

    let rendered = document.to_string();
    DocumentMut::from_str(&rendered)
        .map_err(|error| format!("生成的 CodeX profile TOML 校验失败：{error}"))?;
    Ok(rendered)
}

fn build_profile_toml_with_model_catalog_restore(
    existing: Option<&str>,
    previous_managed_provider_id: Option<&str>,
    profile: &CodexProfile,
    restored_model_catalog_json: Option<&Option<String>>,
) -> Result<String, String> {
    build_codex_toml_with_model_catalog_restore(
        existing,
        previous_managed_provider_id,
        profile,
        restored_model_catalog_json,
        true,
    )
}

#[cfg(test)]
fn build_profile_toml(
    existing: Option<&str>,
    previous_managed_provider_id: Option<&str>,
    profile: &CodexProfile,
) -> Result<String, String> {
    build_profile_toml_with_model_catalog_restore(
        existing,
        previous_managed_provider_id,
        profile,
        None,
    )
}

#[cfg(test)]
fn build_global_toml(
    existing: Option<&str>,
    previous_managed_provider_id: Option<&str>,
    profile: &CodexProfile,
) -> Result<String, String> {
    build_codex_toml_with_model_catalog_restore(
        existing,
        previous_managed_provider_id,
        profile,
        None,
        false,
    )
}

fn build_global_toml_with_model_catalog_restore(
    existing: Option<&str>,
    previous_managed_provider_id: Option<&str>,
    profile: &CodexProfile,
    restored_model_catalog_json: Option<&Option<String>>,
) -> Result<String, String> {
    // Global config is also used to reopen historical sessions. Keep old
    // managed providers there even when model_provider switches to a new
    // default, otherwise Codex cannot parse rollouts that reference them.
    build_codex_toml_with_model_catalog_restore(
        existing,
        previous_managed_provider_id,
        profile,
        restored_model_catalog_json,
        false,
    )
}

#[cfg(test)]
fn merge_missing_provider_item(
    target: &mut DocumentMut,
    source: &DocumentMut,
    provider_id: &str,
) -> Result<bool, String> {
    let Some(source_provider) = source
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(provider_id))
        .cloned()
    else {
        return Ok(false);
    };

    let target_has_provider = target
        .get("model_providers")
        .and_then(Item::as_table)
        .is_some_and(|providers| providers.contains_key(provider_id));
    if target_has_provider {
        return Ok(false);
    }

    let providers = target
        .as_table_mut()
        .entry("model_providers")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| "model_providers 不是 TOML 表".to_string())?;
    providers.insert(provider_id, source_provider);
    Ok(true)
}

fn upsert_managed_provider_items(
    raw: &str,
    state: &CodexProfileState,
    _global_profile_id: Option<&str>,
) -> Result<Option<String>, String> {
    let mut document = DocumentMut::from_str(raw)
        .map_err(|error| format!("全局 config.toml 无法解析：{error}"))?;
    let before = document.to_string();

    for profile in state
        .profiles
        .iter()
        .filter(|profile| profile.auth_mode == CodexAuthMode::Custom)
    {
        let providers = document
            .as_table_mut()
            .entry("model_providers")
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| "model_providers 不是 TOML 表".to_string())?;
        if !providers.contains_key(&profile.provider_id) {
            providers.insert(&profile.provider_id, Item::Table(Table::new()));
        }
        let provider = providers
            .get_mut(&profile.provider_id)
            .and_then(Item::as_table_mut)
            .ok_or_else(|| format!("provider '{}' 不是 TOML 表", profile.provider_id))?;
        provider["name"] = value(&profile.provider_name);
        let base_url = if profile.protocol_conversion {
            global_conversion_proxy_url(&profile.id)
        } else {
            profile.base_url.clone()
        };
        provider["base_url"] = value(base_url);
        provider["wire_api"] = value("responses");
        configure_provider_credentials(provider, profile)?;
        provider.remove("requires_openai_auth");
        provider.remove("experimental_bearer_token");
    }

    let rendered = document.to_string();
    if rendered == before {
        Ok(None)
    } else {
        DocumentMut::from_str(&rendered)
            .map_err(|error| format!("共享 provider 配置校验失败：{error}"))?;
        Ok(Some(rendered))
    }
}

fn synchronize_shared_provider_registry(state: &CodexProfileState) -> Result<(), String> {
    let path = codex_home()?.join("config.toml");
    let raw = if path.exists() {
        fs::read_to_string(&path)
            .map_err(|error| format!("无法读取共享 config.toml：{error}"))?
    } else {
        String::new()
    };
    if let Some(rendered) =
        upsert_managed_provider_items(&raw, state, state.global_profile_id.as_deref())?
    {
        write_toml_atomic(&path, rendered.as_bytes())?;
    }
    Ok(())
}

/// Restore provider definitions needed to parse historical rollouts after an
/// older launcher version removed the previous global provider on switch.
fn restore_missing_managed_global_providers(
    raw: &str,
    state: &CodexProfileState,
) -> Result<Option<String>, String> {
    upsert_managed_provider_items(raw, state, state.global_profile_id.as_deref())
}

fn managed_provider_id(profile: &CodexProfile) -> Option<&str> {
    (profile.auth_mode == CodexAuthMode::Custom).then_some(profile.provider_id.as_str())
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!("{value}.{suffix}"))
        .unwrap_or_else(|| suffix.to_string());
    path.with_extension(extension)
}

fn write_atomic_validated<F>(
    path: &Path,
    content: &[u8],
    label: &str,
    validate: F,
) -> Result<(), String>
where
    F: Fn(&[u8]) -> Result<(), String>,
{
    validate(content)?;
    let parent = path.parent().ok_or_else(|| format!("{label} 没有父目录"))?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建 {label} 目录：{error}"))?;
    let temporary = sidecar_path(path, "tmp");
    let backup = sidecar_path(path, "bak");
    let result = (|| {
        let mut file = fs::File::create(&temporary)
            .map_err(|error| format!("无法创建 {label} 临时文件：{error}"))?;
        file.write_all(content)
            .map_err(|error| format!("无法写入 {label} 临时文件：{error}"))?;
        file.sync_all()
            .map_err(|error| format!("无法刷新 {label} 临时文件：{error}"))?;
        drop(file);
        let verification =
            fs::read(&temporary).map_err(|error| format!("无法读取 {label} 临时文件：{error}"))?;
        validate(&verification)?;
        if path.exists() {
            if backup.exists() {
                fs::remove_file(&backup)
                    .map_err(|error| format!("无法替换 {label} 备份：{error}"))?;
            }
            fs::rename(path, &backup).map_err(|error| format!("无法备份 {label}：{error}"))?;
        }
        if let Err(error) = fs::rename(&temporary, path) {
            if backup.exists() && !path.exists() {
                let _ = fs::rename(&backup, path);
            }
            return Err(format!("无法提交 {label}：{error}"));
        }
        Ok(())
    })();
    if result.is_err() && temporary.exists() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn write_toml_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    write_atomic_validated(path, content, "CodeX profile TOML", |bytes| {
        let text = std::str::from_utf8(bytes)
            .map_err(|error| format!("CodeX profile TOML 不是 UTF-8：{error}"))?;
        DocumentMut::from_str(text)
            .map(|_| ())
            .map_err(|error| format!("CodeX profile TOML 无法解析：{error}"))
    })
}

fn write_model_catalog_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    write_atomic_validated(path, content, "CodeX models.json", validate_model_catalog_bytes)
}

fn write_raw_atomic(path: &Path, content: &[u8], label: &str) -> Result<(), String> {
    write_atomic_validated(path, content, label, |_| Ok(()))
}

fn write_credential_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    write_atomic_validated(path, content, "CodeX 加密凭据", |bytes| {
        if bytes.is_empty() {
            return Err("CodeX 加密凭据为空".to_string());
        }
        unprotect_secret(bytes).map(|_| ())
    })
}

fn remove_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法删除 {}：{error}", path.display())),
    }
}

fn remove_transaction_sidecars(path: &Path) -> Result<(), String> {
    remove_if_exists(&sidecar_path(path, "tmp"))?;
    remove_if_exists(&sidecar_path(path, "bak"))
}

fn restore_snapshot(
    path: &Path,
    snapshot: Option<&[u8]>,
    kind: SnapshotKind,
) -> Result<(), String> {
    match snapshot {
        Some(content) => match kind {
            SnapshotKind::Json => write_json_atomic(path, content, "CodeX 回滚 JSON"),
            SnapshotKind::Toml => write_toml_atomic(path, content),
            SnapshotKind::Credential => write_credential_atomic(path, content),
            SnapshotKind::Raw => write_raw_atomic(path, content, "CodeX models.json 回滚"),
        },
        None => {
            remove_if_exists(path)?;
            remove_transaction_sidecars(path)
        }
    }
}

#[derive(Clone, Copy)]
enum SnapshotKind {
    Json,
    Toml,
    Credential,
    Raw,
}

fn read_optional_file(path: &Path, label: &str) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("无法读取 {label} {}：{error}", path.display())),
    }
}

fn profile_from_state<'a>(state: &'a CodexProfileState, profile_id: &str) -> Option<&'a CodexProfile> {
    state.profiles.iter().find(|profile| profile.id == profile_id)
}

fn desired_model_catalogs(
    state: &CodexProfileState,
    active_profile_id: Option<&str>,
    global_profile_id: Option<&str>,
    global_provider_id: Option<&str>,
    keep_current_global_projection: bool,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut desired = BTreeMap::new();
    if let Some(profile_id) = active_profile_id {
        if let Some(profile) = profile_from_state(state, profile_id) {
            if profile.auth_mode == CodexAuthMode::Custom && profile.model_catalog.is_some() {
                desired.insert(profile_id.to_string(), render_model_catalog(profile)?);
            }
        }
    }

    if global_provider_id.is_some() {
        if let Some(profile_id) = global_profile_id {
            if keep_current_global_projection {
                if let Some(managed) = state.managed_model_catalogs.get(profile_id) {
                    desired
                        .entry(profile_id.to_string())
                        .or_insert_with(|| managed.applied_bytes.clone());
                } else if let Some(profile) = profile_from_state(state, profile_id) {
                    if profile.auth_mode == CodexAuthMode::Custom
                        && profile.model_catalog.is_some()
                    {
                        desired.insert(profile_id.to_string(), render_model_catalog(profile)?);
                    }
                }
            } else if let Some(profile) = profile_from_state(state, profile_id) {
                if profile.auth_mode == CodexAuthMode::Custom && profile.model_catalog.is_some() {
                    desired.insert(profile_id.to_string(), render_model_catalog(profile)?);
                }
            }
        }
    }
    Ok(desired)
}

fn synchronize_model_catalogs(
    state: &mut CodexProfileState,
    desired: &BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    let codex_home = codex_home()?;
    synchronize_model_catalogs_at(state, desired, &codex_home)
}

fn synchronize_model_catalogs_at(
    state: &mut CodexProfileState,
    desired: &BTreeMap<String, Vec<u8>>,
    codex_home: &Path,
) -> Result<(), String> {
    for (profile_id, content) in desired {
        let path = managed_model_catalog_path_at(codex_home, profile_id);
        let path_text = path.to_string_lossy().to_string();
        if let Some(managed) = state.managed_model_catalogs.get_mut(profile_id) {
            if managed.path != path_text {
                return Err(format!(
                    "CodeX 模型目录路径与 profile '{}' 的管理记录不一致",
                    profile_id
                ));
            }
            let current = read_optional_file(&path, "CodeX models.json")?;
            let previous_applied = Some(managed.applied_bytes.as_slice());
            if current.as_deref() != previous_applied && current.as_deref() != Some(content) {
                return Err(format!(
                    "CodeX 模型目录 {} 已被外部修改，拒绝静默覆盖",
                    path.display()
                ));
            }
            if current.as_deref() != Some(content) {
                write_model_catalog_atomic(&path, content)?;
            }
            managed.applied_bytes = content.clone();
        } else {
            let previous_bytes = read_optional_file(&path, "CodeX models.json")?;
            write_model_catalog_atomic(&path, content)?;
            state.managed_model_catalogs.insert(
                profile_id.clone(),
                ManagedModelCatalogState {
                    path: path_text,
                    previous_bytes,
                    applied_bytes: content.clone(),
                },
            );
        }
    }

    let stale_ids = state
        .managed_model_catalogs
        .keys()
        .filter(|profile_id| !desired.contains_key(*profile_id))
        .cloned()
        .collect::<Vec<_>>();
    for profile_id in stale_ids {
        let managed = state
            .managed_model_catalogs
            .get(&profile_id)
            .cloned()
            .ok_or_else(|| format!("CodeX 模型目录管理记录 '{}' 不存在", profile_id))?;
        let expected_path = managed_model_catalog_path_at(codex_home, &profile_id);
        if PathBuf::from(&managed.path) != expected_path {
            return Err(format!(
                "CodeX 模型目录管理记录 '{}' 指向了非预期路径",
                profile_id
            ));
        }
        let current = read_optional_file(&expected_path, "CodeX models.json")?;
        if current.as_deref() != Some(managed.applied_bytes.as_slice()) {
            return Err(format!(
                "CodeX 模型目录 {} 已被外部修改，拒绝删除或恢复",
                expected_path.display()
            ));
        }
        restore_snapshot(
            &expected_path,
            managed.previous_bytes.as_deref(),
            SnapshotKind::Raw,
        )?;
        state.managed_model_catalogs.remove(&profile_id);
    }
    Ok(())
}

fn model_catalog_paths_for_transition(
    state: &CodexProfileState,
    desired: &BTreeMap<String, Vec<u8>>,
) -> Result<BTreeMap<PathBuf, Option<Vec<u8>>>, String> {
    let codex_home = codex_home()?;
    model_catalog_paths_for_transition_at(state, desired, &codex_home)
}

fn model_catalog_paths_for_transition_at(
    state: &CodexProfileState,
    desired: &BTreeMap<String, Vec<u8>>,
    codex_home: &Path,
) -> Result<BTreeMap<PathBuf, Option<Vec<u8>>>, String> {
    let mut paths = HashSet::new();
    for profile_id in state.managed_model_catalogs.keys() {
        paths.insert(managed_model_catalog_path_at(codex_home, profile_id));
    }
    for profile_id in desired.keys() {
        paths.insert(managed_model_catalog_path_at(codex_home, profile_id));
    }
    paths
        .into_iter()
        .map(|path| {
            let snapshot = read_optional_file(&path, "CodeX models.json")?;
            Ok((path, snapshot))
        })
        .collect()
}

fn restore_model_catalog_paths(
    snapshots: &BTreeMap<PathBuf, Option<Vec<u8>>>,
) -> Result<(), String> {
    for (path, snapshot) in snapshots {
        restore_snapshot(path, snapshot.as_deref(), SnapshotKind::Raw)?;
    }
    Ok(())
}

#[cfg(windows)]
fn protect_secret(secret: &str) -> Result<Vec<u8>, String> {
    use windows::core::w;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let bytes = secret.as_bytes();
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input,
            w!("Agents Launcher Codex API Key"),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|error| format!("Windows DPAPI 加密失败：{error}"))?;
        let encrypted = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData.cast()));
        Ok(encrypted)
    }
}

#[cfg(windows)]
fn unprotect_secret(encrypted: &[u8]) -> Result<String, String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: encrypted.len() as u32,
        pbData: encrypted.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|error| format!("Windows DPAPI 解密失败：{error}"))?;
        let decrypted = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData.cast()));
        String::from_utf8(decrypted).map_err(|error| format!("CodeX 凭据不是 UTF-8：{error}"))
    }
}

#[cfg(not(windows))]
fn unprotect_secret(_encrypted: &[u8]) -> Result<String, String> {
    Err("CodeX 凭据解密仅支持 Windows".to_string())
}

trait ProfileSecretStore {
    fn read(&self, profile_id: &str) -> Result<Option<String>, String>;
    fn write(&self, profile_id: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, profile_id: &str) -> Result<(), String>;
}

struct PlatformProfileSecretStore;

impl ProfileSecretStore for PlatformProfileSecretStore {
    fn read(&self, profile_id: &str) -> Result<Option<String>, String> {
        platform_read_profile_secret(profile_id)
    }

    fn write(&self, profile_id: &str, secret: &str) -> Result<(), String> {
        platform_write_profile_secret(profile_id, secret)
    }

    fn delete(&self, profile_id: &str) -> Result<(), String> {
        platform_delete_profile_secret(profile_id)
    }
}

fn platform_read_profile_secret(profile_id: &str) -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        let path = credential_path(profile_id)?;
        if !path.exists() {
            return Ok(None);
        }
        let encrypted =
            fs::read(&path).map_err(|error| format!("无法读取 CodeX 加密凭据：{error}"))?;
        return unprotect_secret(&encrypted).map(Some);
    }

    #[cfg(target_os = "macos")]
    {
        let credentials = load_plaintext_credentials()?;
        Ok(credentials.get(profile_id).cloned())
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = profile_id;
        Err("当前平台没有可用的 CodeX 安全凭据存储".to_string())
    }
}

fn platform_write_profile_secret(profile_id: &str, secret: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        let path = credential_path(profile_id)?;
        let encrypted = protect_secret(secret)?;
        write_credential_atomic(&path, &encrypted)?;
        return match platform_read_profile_secret(profile_id)? {
            Some(verified) if verified == secret => Ok(()),
            _ => Err("CodeX DPAPI 凭据写入后回读不一致".to_string()),
        };
    }

    #[cfg(target_os = "macos")]
    {
        let mut credentials = load_plaintext_credentials()?;
        credentials.insert(profile_id.to_string(), secret.to_string());
        save_plaintext_credentials(&credentials)?;
        return match load_plaintext_credentials()?.get(profile_id) {
            Some(verified) if verified == secret => Ok(()),
            _ => Err("CodeX 明文凭据写入后回读不一致".to_string()),
        };
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (profile_id, secret);
        Err("当前平台没有可用的 CodeX 安全凭据存储".to_string())
    }
}

fn platform_delete_profile_secret(profile_id: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        let path = credential_path(profile_id)?;
        remove_if_exists(&path)?;
        remove_transaction_sidecars(&path)?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        let mut credentials = load_plaintext_credentials()?;
        credentials.remove(profile_id);
        save_plaintext_credentials(&credentials)?;
        if load_plaintext_credentials()?.contains_key(profile_id) {
            Err("CodeX 明文凭据删除后仍然存在".to_string())
        } else {
            Ok(())
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = profile_id;
        Err("当前平台没有可用的 CodeX 安全凭据存储".to_string())
    }
}

#[cfg(target_os = "macos")]
fn load_plaintext_credentials_from(path: &Path) -> Result<BTreeMap<String, String>, String> {
    restore_private_json_backup_if_missing(path, "CodeX 明文凭据")?;
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let raw =
        fs::read_to_string(path).map_err(|error| format!("无法读取 CodeX 明文凭据：{error}"))?;
    serde_json::from_str(&raw).map_err(|error| format!("CodeX 明文凭据无法解析：{error}"))
}

#[cfg(target_os = "macos")]
fn save_plaintext_credentials_to(
    path: &Path,
    credentials: &BTreeMap<String, String>,
) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(credentials)
        .map_err(|error| format!("无法序列化 CodeX 明文凭据：{error}"))?;
    write_private_json_atomic(path, &json, "CodeX 明文凭据")
}

#[cfg(target_os = "macos")]
fn load_plaintext_credentials() -> Result<BTreeMap<String, String>, String> {
    load_plaintext_credentials_from(&plaintext_credentials_path()?)
}

#[cfg(target_os = "macos")]
fn save_plaintext_credentials(credentials: &BTreeMap<String, String>) -> Result<(), String> {
    save_plaintext_credentials_to(&plaintext_credentials_path()?, credentials)
}

fn read_profile_secret(profile_id: &str) -> Result<Option<String>, String> {
    PlatformProfileSecretStore.read(profile_id)
}

fn write_profile_secret(profile_id: &str, secret: &str) -> Result<(), String> {
    PlatformProfileSecretStore.write(profile_id, secret)
}

fn delete_profile_secret(profile_id: &str) -> Result<(), String> {
    PlatformProfileSecretStore.delete(profile_id)
}

fn restore_profile_secret_with<S: ProfileSecretStore>(
    store: &S,
    profile_id: &str,
    snapshot: Option<&str>,
) -> Result<(), String> {
    match snapshot {
        Some(secret) => store.write(profile_id, secret),
        None => store.delete(profile_id),
    }
}

fn restore_profile_secret(profile_id: &str, snapshot: Option<&str>) -> Result<(), String> {
    restore_profile_secret_with(&PlatformProfileSecretStore, profile_id, snapshot)
}

fn profile_secret_exists(profile_id: &str) -> Result<bool, String> {
    Ok(read_profile_secret(profile_id)?.is_some())
}

fn load_managed_global_env() -> Result<Option<ManagedGlobalEnv>, String> {
    #[cfg(not(windows))]
    {
        return Ok(None);
    }

    #[cfg(windows)]
    {
        load_managed_env_at(&global_env_path()?, "CodeX 全局环境变量")
    }
}

#[cfg(windows)]
fn load_managed_env_at(path: &Path, label: &str) -> Result<Option<ManagedGlobalEnv>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let encrypted = fs::read(path).map_err(|error| format!("无法读取{label}记录：{error}"))?;
    let json = unprotect_secret(&encrypted)?;
    serde_json::from_str(&json)
        .map(Some)
        .map_err(|error| format!("{label}记录无法解析：{error}"))
}

fn load_managed_global_codex_home_env() -> Result<Option<ManagedGlobalEnv>, String> {
    #[cfg(windows)]
    {
        return load_managed_env_at(
            &global_codex_home_env_path()?,
            "CodeX 全局 CODEX_HOME 环境变量",
        );
    }
    #[cfg(not(windows))]
    Ok(None)
}

#[cfg(windows)]
fn save_managed_env_at(
    path: &Path,
    label: &str,
    record: &ManagedGlobalEnv,
) -> Result<(), String> {
    let json = serde_json::to_string(record)
        .map_err(|error| format!("无法序列化{label}记录：{error}"))?;
    let encrypted = protect_secret(&json)?;
    write_credential_atomic(path, &encrypted)
}

#[cfg(windows)]
fn write_user_env_var(name: &str, value: Option<&str>) -> Result<(), String> {
    let mut vars = HashMap::new();
    vars.insert(name.to_string(), value.unwrap_or_default().to_string());
    env_applier::apply_env_vars(vars, "user".to_string())
}

fn restore_user_env_snapshots(snapshots: &HashMap<String, Option<String>>) -> Result<(), String> {
    #[cfg(windows)]
    {
        let vars = snapshots
            .iter()
            .map(|(key, value)| (key.clone(), value.clone().unwrap_or_default()))
            .collect();
        env_applier::apply_env_vars(vars, "user".to_string())
    }

    #[cfg(not(windows))]
    {
        if snapshots.is_empty() {
            Ok(())
        } else {
            Err("当前平台不支持持久化 CodeX 第三方全局环境变量".to_string())
        }
    }
}

fn read_user_env_var(name: &str) -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        registry::read_user_env_var(name)
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        Err("当前平台不支持持久化 CodeX 第三方全局环境变量".to_string())
    }
}

fn transition_managed_global_env(
    next: Option<(&str, &str)>,
    previous: Option<&ManagedGlobalEnv>,
) -> Result<(), String> {
    transition_managed_user_env(next, previous, &global_env_path()?)
}

fn release_managed_global_codex_home_env(
    previous: Option<&ManagedGlobalEnv>,
) -> Result<(), String> {
    transition_managed_user_env(None, previous, &global_codex_home_env_path()?)
}

fn transition_managed_user_env(
    next: Option<(&str, &str)>,
    previous: Option<&ManagedGlobalEnv>,
    record_path: &Path,
) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let _ = (previous, record_path);
        return if next.is_none() {
            Ok(())
        } else {
            Err("macOS 不会持久化第三方 API Key 到用户环境；请仅在启动器内应用该方案".to_string())
        };
    }

    #[cfg(windows)]
    {
        match (previous, next) {
            (Some(previous), Some((next_key, next_value))) if previous.key == next_key => {
                let current = read_user_env_var(next_key)?;
                let previous_value = if current.as_deref() == Some(previous.applied_value.as_str())
                {
                    previous.previous_value.clone()
                } else {
                    current
                };
                write_user_env_var(next_key, Some(next_value))?;
                save_managed_env_at(
                    record_path,
                    "CodeX 全局环境变量",
                    &ManagedGlobalEnv {
                        key: next_key.to_string(),
                        applied_value: next_value.to_string(),
                        previous_value,
                    },
                )?;
            }
            (previous, next) => {
                if let Some(previous) = previous {
                    let current = read_user_env_var(&previous.key)?;
                    if current.as_deref() == Some(previous.applied_value.as_str()) {
                        write_user_env_var(&previous.key, previous.previous_value.as_deref())?;
                    }
                }
                if let Some((next_key, next_value)) = next {
                    let previous_value = read_user_env_var(next_key)?;
                    write_user_env_var(next_key, Some(next_value))?;
                    save_managed_env_at(
                        record_path,
                        "CodeX 全局环境变量",
                        &ManagedGlobalEnv {
                            key: next_key.to_string(),
                            applied_value: next_value.to_string(),
                            previous_value,
                        },
                    )?;
                } else {
                    remove_if_exists(record_path)?;
                    remove_transaction_sidecars(record_path)?;
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn resolve_profile_api_key(profile: &CodexProfile) -> Result<String, String> {
    if let Some(secret) = read_profile_secret(&profile.id)? {
        return Ok(secret);
    }
    std::env::var(&profile.env_key).map_err(|_| {
        format!(
            "CodeX 配置 '{}' 没有已保存的 API Key，环境变量 {} 也不存在",
            profile.name, profile.env_key
        )
    })
}

fn would_remove_profile_secret(
    profile: &CodexProfile,
    clear_api_key: bool,
    previous_secret: Option<&str>,
) -> bool {
    previous_secret.is_some()
        && (profile.auth_mode == CodexAuthMode::Official
            || (profile.auth_mode == CodexAuthMode::Custom && clear_api_key))
}

fn auth_status() -> Result<CodexAuthStatus, String> {
    let path = auth_path()?;
    if !path.exists() {
        return Ok(CodexAuthStatus {
            mode: None,
            has_auth_file: false,
            has_credentials: false,
            error: None,
        });
    }
    let result = (|| {
        let raw =
            fs::read_to_string(&path).map_err(|error| format!("无法读取 auth.json：{error}"))?;
        let value: Value =
            serde_json::from_str(&raw).map_err(|error| format!("auth.json 无法解析：{error}"))?;
        let mode = value
            .get("auth_mode")
            .and_then(Value::as_str)
            .map(str::to_string);
        let has_credentials = value
            .get("tokens")
            .and_then(Value::as_object)
            .map(|tokens| !tokens.is_empty())
            .unwrap_or(false)
            || value
                .get("OPENAI_API_KEY")
                .and_then(Value::as_str)
                .map(|key| !key.is_empty())
                .unwrap_or(false);
        Ok((mode, has_credentials))
    })();
    match result {
        Ok((mode, has_credentials)) => Ok(CodexAuthStatus {
            mode,
            has_auth_file: true,
            has_credentials,
            error: None,
        }),
        Err(error) => Ok(CodexAuthStatus {
            mode: None,
            has_auth_file: true,
            has_credentials: false,
            error: Some(error),
        }),
    }
}

fn global_config_error() -> Result<Option<String>, String> {
    let path = global_config_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        fs::read_to_string(&path).map_err(|error| format!("无法读取全局 config.toml：{error}"))?;
    Ok(DocumentMut::from_str(&raw)
        .err()
        .map(|error| format!("全局 config.toml 无法解析：{error}")))
}

fn normalize_index(
    profiles: &[CodexProfile],
    requested_order: Vec<String>,
    requested_active: Option<String>,
) -> ProfileIndexState {
    let valid_ids = profiles
        .iter()
        .map(|profile| profile.id.clone())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut order = requested_order
        .into_iter()
        .filter(|id| valid_ids.contains(id) && seen.insert(id.clone()))
        .collect::<Vec<_>>();
    for profile in profiles {
        if seen.insert(profile.id.clone()) {
            order.push(profile.id.clone());
        }
    }
    let active_profile_id = requested_active.filter(|id| valid_ids.contains(id));
    let profile_ids = profiles
        .iter()
        .map(|profile| (profile.name.clone(), profile.id.clone()))
        .collect();
    ProfileIndexState {
        order,
        profile_ids,
        active_profile_id,
    }
}

fn prepare_profiles_for_load(state: &mut CodexProfileState) -> Result<(), String> {
    let shared_home = codex_home()?;
    let previous_managed_codex_home = load_managed_global_codex_home_env()?;
    migrate_legacy_global_config(&shared_home, previous_managed_codex_home.as_ref())?;
    release_managed_global_codex_home_env(previous_managed_codex_home.as_ref())?;
    let previous_managed_api_key = load_managed_global_env()?;
    transition_managed_global_env(None, previous_managed_api_key.as_ref())?;
    migrate_latest_official_auth(state)?;
    let (global_model_context_window, global_model_auto_compact_token_limit) = if state
        .global_profile_id
        .is_some()
    {
        let path = global_config_path()?;
        if path.exists() {
            let raw = fs::read_to_string(&path).map_err(|error| {
                format!("无法读取全局 CodeX config.toml：{error}")
            })?;
            (
                model_context_window_from_raw(&raw, "全局 CodeX config.toml")?,
                model_auto_compact_token_limit_from_raw(&raw, "全局 CodeX config.toml")?,
            )
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };
    // 共享端线程集合与 profile 无关，整轮只算一次（见 prepare_shared_profile_storage）。
    let mut shared_thread_ids = None;
    for profile in &mut state.profiles {
        if let Some(ratio) = profile.model_auto_compact_ratio {
            if ratio.is_finite() && (0.0..=1.0).contains(&ratio) {
                profile.model_auto_compact_ratio =
                    Some(stabilize_model_auto_compact_ratio(ratio));
            }
        }

        profile.managed_profile_name = managed_profile_name(&profile.id);
        profile.has_stored_api_key = profile_secret_exists(&profile.id)?;
        prepare_shared_profile_storage(profile, &mut shared_thread_ids)?;
        let profile_path = managed_profile_path(&profile.id)?;
        let mut profile_auto_compact_token_limit = None;
        if profile_path.exists() {
            let raw = fs::read_to_string(&profile_path).map_err(|error| {
                format!("无法读取 CodeX profile {}：{error}", profile_path.display())
            })?;
            DocumentMut::from_str(&raw).map_err(|error| {
                format!("CodeX profile {} 无法解析：{error}", profile_path.display())
            })?;
            profile.model_context_window = model_context_window_from_raw(
                &raw,
                &format!("CodeX profile {} ", profile_path.display()),
            )?;
            profile_auto_compact_token_limit = model_auto_compact_token_limit_from_raw(
                &raw,
                &format!("CodeX profile {} ", profile_path.display()),
            )?;
            if profile_auto_compact_token_limit.is_none() {
                profile.model_auto_compact_ratio = None;
            }
            if profile.model_context_window.is_some() {
                profile.model_context_window_configured = true;
            }
        }
        if !profile.model_context_window_configured
            && profile.model_context_window.is_none()
            && profile.auth_mode == CodexAuthMode::Official
            && state.global_profile_id.as_deref() == Some(profile.id.as_str())
        {
            profile.model_context_window = global_model_context_window;
            if profile.model_context_window.is_some() {
                profile.model_context_window_configured = true;
            }
        }
        if profile.model_context_window.is_none() {
            profile.model_auto_compact_ratio = None;
        }
        let token_limit = if profile.model_auto_compact_ratio.is_none()
            && !profile.model_auto_compact_ratio_configured
        {
            profile_auto_compact_token_limit.or_else(|| {
                (state.global_profile_id.as_deref() == Some(profile.id.as_str()))
                    .then_some(global_model_auto_compact_token_limit)
                    .flatten()
            })
        } else {
            None
        };
        if let (Some(token_limit), Some(context_window)) =
            (token_limit, profile.model_context_window)
        {
            if let Some(ratio) =
                model_auto_compact_ratio_from_token_limit(token_limit, context_window)
            {
                profile.model_auto_compact_ratio = Some(ratio);
                profile.model_auto_compact_ratio_configured = true;
            }
        }
    }
    synchronize_shared_provider_registry(state)?;
    Ok(())
}

fn enrich_profiles(state: &mut CodexProfileState) -> Result<(), String> {
    prepare_profiles_for_load(state)?;
    sync_global_conversion_proxy()?;
    Ok(())
}

fn global_profile_matches_document_with_proxy(
    state: &CodexProfileState,
    raw: &str,
    use_proxy_base_url: bool,
) -> bool {
    let Some(profile_id) = state.global_profile_id.as_deref() else {
        return false;
    };
    let Some(profile) = state
        .profiles
        .iter()
        .find(|profile| profile.id == profile_id)
    else {
        return false;
    };
    let model_catalog_is_managed = profile.auth_mode == CodexAuthMode::Custom
        && profile.model_catalog.is_some();
    if !cfg!(windows) {
        if let Some(managed) = state.managed_global_model_catalog.as_ref() {
            if validate_managed_global_model_catalog_reference(
                Some(raw),
                managed,
                model_catalog_is_managed,
            )
            .is_err()
            {
                return false;
            }
        }
    }
    let restore_value = if cfg!(windows) {
        managed_profile_path(&profile.id)
            .ok()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|profile_raw| model_catalog_json_value_from_raw(Some(&profile_raw)).ok())
    } else if profile.auth_mode == CodexAuthMode::Custom
        && profile.model_catalog.is_some()
    {
        None
    } else if let Some(managed) = state.managed_global_model_catalog.as_ref() {
        Some(managed.previous_value.clone())
    } else {
        let Ok(current) = model_catalog_json_value_from_raw(Some(raw)) else {
            return false;
        };
        Some(current)
    };
    let mut expected_profile = profile.clone();
    if use_proxy_base_url
        && profile.protocol_conversion
        && profile.auth_mode == CodexAuthMode::Custom
    {
        expected_profile.base_url = global_conversion_proxy_url(&profile.id);
    }
    let Ok(expected) = build_global_toml_with_model_catalog_restore(
        Some(raw),
        state.managed_global_provider_id.as_deref(),
        &expected_profile,
        restore_value.as_ref(),
    ) else {
        return false;
    };

    if profile.auth_mode != CodexAuthMode::Official {
        return expected == raw;
    }

    // Official Codex sessions may update these fields themselves (for example
    // when the desktop client changes the selected model). They are not part
    // of the official profile's global consistency check, whether or not the
    // launcher profile has a value configured for them.
    let Ok(mut expected_document) = DocumentMut::from_str(&expected) else {
        return false;
    };
    let Ok(mut actual_document) = DocumentMut::from_str(raw) else {
        return false;
    };
    for document in [&mut expected_document, &mut actual_document] {
        document.as_table_mut().remove("model");
        document.as_table_mut().remove("model_reasoning_effort");
    }
    expected_document.to_string() == actual_document.to_string()
}

fn global_profile_matches_document(state: &CodexProfileState, raw: &str) -> bool {
    global_profile_matches_document_with_proxy(state, raw, true)
}

fn global_profile_is_recoverable_at_startup(state: &CodexProfileState, raw: &str) -> bool {
    global_profile_matches_document_with_proxy(state, raw, true)
        || global_profile_matches_document_with_proxy(state, raw, false)
}

fn global_config_contains_orphaned_conversion_proxy(
    state: &CodexProfileState,
    raw: &str,
) -> bool {
    let Ok(document) = DocumentMut::from_str(raw) else {
        return false;
    };
    document
        .get("model_providers")
        .and_then(Item::as_table)
        .is_some_and(|providers| {
            providers.iter().any(|(provider_id, provider)| {
                let current = provider
                    .as_table()
                    .and_then(|table| table.get("base_url"))
                    .and_then(Item::as_value)
                    .and_then(|value| value.as_str())
                    .filter(|value| is_conversion_proxy_url(value));
                let Some(current) = current else {
                    return false;
                };
                !state.profiles.iter().any(|profile| {
                    profile.auth_mode == CodexAuthMode::Custom
                        && profile.protocol_conversion
                        && profile.provider_id == provider_id
                        && global_conversion_proxy_url(&profile.id) == current
                })
            })
        })
}

fn global_sync_repair_required_for_raw(
    state: &CodexProfileState,
    global_profile_in_sync: bool,
    raw: Option<&str>,
) -> bool {
    let Some(raw) = raw else {
        return state.global_profile_id.is_some() && !global_profile_in_sync;
    };
    if state.global_profile_id.is_some() {
        return !global_profile_is_recoverable_at_startup(state, raw);
    }
    global_config_contains_orphaned_conversion_proxy(state, raw)
}

fn global_sync_repair_required(
    state: &CodexProfileState,
    global_profile_in_sync: bool,
) -> Result<bool, String> {
    let path = global_config_path()?;
    if !path.exists() {
        return Ok(global_sync_repair_required_for_raw(
            state,
            global_profile_in_sync,
            None,
        ));
    }
    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("无法读取全局 config.toml：{error}"))?;
    Ok(global_sync_repair_required_for_raw(
        state,
        global_profile_in_sync,
        Some(&raw),
    ))
}

fn global_profile_in_sync(state: &CodexProfileState) -> Result<bool, String> {
    let Some(profile_id) = state.global_profile_id.as_deref() else {
        return Ok(false);
    };
    let Some(profile) = state
        .profiles
        .iter()
        .find(|profile| profile.id == profile_id)
    else {
        return Ok(false);
    };
    let path = global_config_path()?;
    if !path.exists() {
        return Ok(false);
    }
    let raw =
        fs::read_to_string(&path).map_err(|error| format!("无法读取全局 config.toml：{error}"))?;
    if !global_profile_matches_document(state, &raw) {
        return Ok(false);
    }
    // 全局 config.toml 只引用模型目录文件路径，编辑模型目录（如上下文长度）
    // 后路径本身不变，必须再比较文件内容与 profile 当前渲染结果，否则保存
    // 后会误报“全局应用中”，用户无法直接重新同步。
    if !global_model_catalog_in_sync(profile) {
        return Ok(false);
    }
    #[cfg(windows)]
    {
        // 全局 Key 持久化在用户环境变量中，保存修改 Key 后 config.toml
        // 内容不变，同样需要单独比较实际值，让用户保存后立刻可重新同步。
        if profile.auth_mode == CodexAuthMode::Custom
            && custom_global_key_sync_supported()
            && !global_key_env_in_sync(profile)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn global_model_catalog_in_sync(profile: &CodexProfile) -> bool {
    let path = match managed_model_catalog_path(&profile.id) {
        Ok(path) => path,
        Err(_) => return false,
    };
    let current = match fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return false,
    };
    global_model_catalog_content_in_sync(profile, current.as_deref())
}

fn global_model_catalog_content_in_sync(profile: &CodexProfile, current: Option<&[u8]>) -> bool {
    if profile.auth_mode != CodexAuthMode::Custom || profile.model_catalog.is_none() {
        return true;
    }
    match render_model_catalog(profile) {
        Ok(expected) => current == Some(expected.as_slice()),
        Err(_) => false,
    }
}

#[cfg(windows)]
fn global_key_env_in_sync(profile: &CodexProfile) -> bool {
    let stored_secret = read_profile_secret(&profile.id).ok().flatten();
    let env_value = read_user_env_var(&profile.env_key).ok().flatten();
    let managed_record = load_managed_global_env().ok().flatten();
    global_key_env_content_in_sync(
        profile.has_stored_api_key,
        stored_secret.as_deref(),
        env_value.as_deref(),
        managed_record.as_ref(),
        &profile.env_key,
    )
}

#[allow(dead_code)] // exercised by tests on all platforms; called on Windows only
fn global_key_env_content_in_sync(
    has_stored_api_key: bool,
    stored_secret: Option<&str>,
    env_value: Option<&str>,
    managed_record: Option<&ManagedGlobalEnv>,
    env_key: &str,
) -> bool {
    if has_stored_api_key {
        return matches!(
            (stored_secret, env_value),
            (Some(stored), Some(env)) if env == stored
        );
    }
    // 启动器曾把全局 Key 写入用户环境变量；清除 Key 后残留值也需要重新同步清理。
    managed_record.is_none_or(|record| record.key != env_key)
}

fn load_payload() -> Result<CodexProfilesPayload, String> {
    let mut state = load_profile_state()?;
    enrich_profiles(&mut state)?;
    let global_profile_in_sync = global_profile_in_sync(&state)?;
    let global_sync_repair_required = global_sync_repair_required(&state, global_profile_in_sync)?;
    let stored_index = load_profile_index_state(CODEX_STATE_KEY)?;
    let index = normalize_index(
        &state.profiles,
        stored_index.order,
        stored_index.active_profile_id,
    );
    let global_profile_id = state.global_profile_id.clone();
    Ok(CodexProfilesPayload {
        profiles: state.profiles,
        order: index.order,
        active_profile_id: index.active_profile_id,
        global_profile_id,
        global_profile_in_sync,
        global_sync_repair_required,
        profiles_path: profiles_path()?.display().to_string(),
        global_config_path: global_config_path()?.display().to_string(),
        auth_path: auth_path()?.display().to_string(),
        global_config_error: global_config_error()?,
        auth_status: auth_status()?,
        custom_global_sync_supported: custom_global_sync_supported(),
        custom_global_key_sync_supported: custom_global_key_sync_supported(),
        secret_storage_kind: secret_storage_kind(),
        platform: platform_name(),
        session_issues: Vec::new(),
        session_issues_blocked: false,
    })
}

#[tauri::command]
pub fn load_codex_profiles() -> Result<CodexProfilesPayload, String> {
    load_payload()
}

#[tauri::command]
pub fn reveal_codex_profile_api_key(profile_id: String) -> Result<Option<String>, String> {
    if profile_id.is_empty()
        || !profile_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
    {
        return Err("CodeX profile ID 含有不支持的字符".to_string());
    }

    let state = load_profile_state()?;
    let profile = state
        .profiles
        .iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| format!("CodeX 配置方案 '{}' 不存在", profile_id))?;
    if profile.auth_mode != CodexAuthMode::Custom {
        return Err("官方登录方案没有由启动器保存的第三方 API Key".to_string());
    }

    read_profile_secret(&profile_id)
}

#[tauri::command]
pub async fn fetch_codex_models(request: FetchCodexModelsRequest) -> Result<Vec<String>, String> {
    let base_url = request.base_url.trim();
    validate_http_url(base_url, "第三方 Base URL")?;
    let env_key = request.env_key.trim();
    if !env_key.is_empty()
        && !env_key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err("API Key 环境变量名只能包含字母、数字和下划线".to_string());
    }
    let provided_key = request
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string);
    let stored_key = if provided_key.is_none() && !request.profile_id.is_empty() {
        if !request.profile_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        }) {
            return Err("CodeX profile ID 含有不支持的字符".to_string());
        }
        read_profile_secret(&request.profile_id)?
    } else {
        None
    };
    let environment_key = if provided_key.is_none() && stored_key.is_none() && !env_key.is_empty() {
        std::env::var(env_key).ok()
    } else {
        None
    };
    let api_key = provided_key
        .or(stored_key)
        .or(environment_key)
        .unwrap_or_default();
    model_fetcher::fetch_openai_compatible_models(base_url, &api_key).await
}

#[tauri::command]
pub fn save_codex_profile(
    request: SaveCodexProfileRequest,
) -> Result<CodexProfilesPayload, String> {
    let profile = normalize_profile(request.profile)?;
    // 共享端线程集合的缓存只服务这一次调用，交给 `prepare_shared_profile_storage`
    // 按需填充（该 profile 已迁完时它连目录都不会读）。
    let mut shared_thread_ids = None;
    prepare_shared_profile_storage(&profile, &mut shared_thread_ids)?;
    let metadata_path = profiles_path()?;
    let profile_path = managed_profile_path(&profile.id)?;
    let mut state = load_profile_state()?;
    let previous_profile = state
        .profiles
        .iter()
        .find(|item| item.id == profile.id)
        .cloned();
    let previous_index = load_profile_index_state(CODEX_STATE_KEY)?;
    if let Some(scope) = profile_rename_block_reason(
        previous_profile.as_ref(),
        &profile,
        previous_index.active_profile_id.as_deref(),
        state.global_profile_id.as_deref(),
    ) {
        return Err(format!(
            "当前配置正在{scope}中，不能直接改名。请先将其他配置应用到对应范围，使当前配置不再处于应用状态后再保存"
        ));
    }
    if state
        .profiles
        .iter()
        .any(|item| item.id != profile.id && item.name == profile.name)
    {
        return Err(format!("CodeX 配置名称 '{}' 已存在", profile.name));
    }
    if profile.auth_mode == CodexAuthMode::Custom
        && state.profiles.iter().any(|item| {
            item.id != profile.id
                && item.auth_mode == CodexAuthMode::Custom
                && item.provider_id == profile.provider_id
        })
    {
        return Err(format!(
            "CodeX 配置名称 '{}' 会生成重复的 provider ID '{}'，请换一个名称",
            profile.name, profile.provider_id
        ));
    }

    let existing_toml = if profile_path.exists() {
        Some(
            fs::read_to_string(&profile_path)
                .map_err(|error| format!("无法读取现有 CodeX profile：{error}"))?,
        )
    } else {
        None
    };
    let previous_secret = read_profile_secret(&profile.id)?;
    if state.global_profile_id.as_deref() == Some(profile.id.as_str())
        && would_remove_profile_secret(
            &profile,
            request.clear_api_key,
            previous_secret.as_deref(),
        )
    {
        return Err("不能删除应用中的配置".to_string());
    }
    let profile_model_catalog_is_managed = profile.auth_mode == CodexAuthMode::Custom
        && profile.model_catalog.is_some();
    if let Some(previous) = state.managed_profile_model_catalogs.get(&profile.id) {
        validate_managed_profile_model_catalog_reference(
            existing_toml.as_deref(),
            &profile.id,
            previous,
            profile_model_catalog_is_managed,
        )?;
    }
    let profile_model_catalog_restore = if profile_model_catalog_is_managed {
        None
    } else if let Some(previous) = state.managed_profile_model_catalogs.get(&profile.id) {
        Some(previous.clone())
    } else {
        Some(model_catalog_json_value_from_raw(existing_toml.as_deref())?)
    };
    let mut stored_profile = profile.clone();
    stored_profile.has_stored_api_key = match stored_profile.auth_mode {
        CodexAuthMode::Official => false,
        CodexAuthMode::Custom if request.clear_api_key => false,
        CodexAuthMode::Custom => {
            request
                .api_key
                .as_deref()
                .map(str::trim)
                .is_some_and(|key| !key.is_empty())
                || previous_secret.is_some()
        }
    };
    let previous_managed_provider_id = previous_profile.as_ref().and_then(managed_provider_id);
    let rendered = build_profile_toml_with_model_catalog_restore(
        existing_toml.as_deref(),
        previous_managed_provider_id,
        &stored_profile,
        profile_model_catalog_restore.as_ref(),
    )?;

    let previous_metadata = fs::read(&metadata_path).ok();
    let previous_toml = fs::read(&profile_path).ok();
    let transaction: Result<(), String> = (|| {
        write_toml_atomic(&profile_path, rendered.as_bytes())?;
        if profile_model_catalog_is_managed {
            let previous = state
                .managed_profile_model_catalogs
                .get(&stored_profile.id)
                .cloned()
                .unwrap_or(model_catalog_json_value_from_raw(existing_toml.as_deref())?);
            state
                .managed_profile_model_catalogs
                .entry(stored_profile.id.clone())
                .or_insert(previous);
        } else {
            state
                .managed_profile_model_catalogs
                .remove(&stored_profile.id);
        }

        match stored_profile.auth_mode {
            CodexAuthMode::Official => {
                delete_profile_secret(&stored_profile.id)?;
                stored_profile.has_stored_api_key = false;
            }
            CodexAuthMode::Custom => {
                if request.clear_api_key {
                    delete_profile_secret(&stored_profile.id)?;
                } else if let Some(api_key) = request
                    .api_key
                    .as_deref()
                    .map(str::trim)
                    .filter(|key| !key.is_empty())
                {
                    write_profile_secret(&stored_profile.id, api_key)?;
                }
                stored_profile.has_stored_api_key = profile_secret_exists(&stored_profile.id)?;
            }
        }

        if let Some(existing) = state
            .profiles
            .iter_mut()
            .find(|item| item.id == stored_profile.id)
        {
            *existing = stored_profile.clone();
        } else {
            state.profiles.push(stored_profile.clone());
        }
        save_profile_state(&state)?;

        let index = normalize_index(
            &state.profiles,
            request.order.clone(),
            request.active_profile_id.clone(),
        );
        save_profile_index_state(CODEX_STATE_KEY, &index)?;

        let verified_state = load_profile_state()?;
        if verified_state != state {
            return Err("CodeX 方案索引写入后校验不一致".to_string());
        }
        let verified_toml = fs::read_to_string(&profile_path)
            .map_err(|error| format!("无法回读 CodeX profile：{error}"))?;
        DocumentMut::from_str(&verified_toml)
            .map_err(|error| format!("CodeX profile 回读校验失败：{error}"))?;
        if load_profile_index_state(CODEX_STATE_KEY)? != index {
            return Err("CodeX 活动方案索引写入后校验不一致".to_string());
        }
        if stored_profile.has_stored_api_key {
            read_profile_secret(&stored_profile.id)?
                .ok_or_else(|| "CodeX 安全凭据写入后不存在".to_string())?;
        }
        Ok(())
    })();

    if let Err(error) = transaction {
        let mut rollback_errors = Vec::new();
        if let Err(rollback) = save_profile_index_state(CODEX_STATE_KEY, &previous_index) {
            rollback_errors.push(rollback);
        }
        if let Err(rollback) = restore_snapshot(
            &metadata_path,
            previous_metadata.as_deref(),
            SnapshotKind::Json,
        ) {
            rollback_errors.push(rollback);
        }
        if let Err(rollback) = restore_profile_secret(&profile.id, previous_secret.as_deref()) {
            rollback_errors.push(rollback);
        }
        if let Err(rollback) =
            restore_snapshot(&profile_path, previous_toml.as_deref(), SnapshotKind::Toml)
        {
            rollback_errors.push(rollback);
        }
        if rollback_errors.is_empty() {
            return Err(format!("保存 CodeX 配置失败，旧数据已恢复：{error}"));
        }
        return Err(format!(
            "保存 CodeX 配置失败且回滚不完整：{error}；{}",
            rollback_errors.join("；")
        ));
    }

    // 保持全局协议转换接管状态与配置一致（best-effort：保存本身已成功）。
    if let Err(error) = sync_global_conversion_proxy() {
        eprintln!("CodeX 全局协议转换代理同步失败：{error}");
    }
    load_payload()
}

#[tauri::command]
pub fn apply_codex_profile(
    request: ApplyCodexProfileRequest,
) -> Result<CodexProfilesPayload, String> {
    let mut state = load_profile_state()?;
    enrich_profiles(&mut state)?;
    let profile = state
        .profiles
        .iter()
        .find(|profile| profile.id == request.profile_id)
        .cloned()
        .ok_or_else(|| format!("CodeX 配置方案 '{}' 不存在", request.profile_id))?;
    // 切换配置预检：先做状态库 rollout_path 轻量校正（不改会话文件、无需退出
    // 客户端），把指向已丢失分页的指针改回磁盘上的存活链尖——这正是"切换后
    // resume 报恢复对话失败"的根因。校正后再扫描：只有真正需要合并的断链
    // （孤立段）才弹窗引导重手术修复。
    if !request.allow_session_issues {
        if let Ok(home) = codex_home() {
            let sessions_root = home.join("sessions");
            if sessions_root.is_dir() {
                let (corrected, _) = crate::codex_session_chain::reconcile_state_rollout_paths(
                    &sessions_root,
                    &home,
                );
                if corrected > 0 {
                    eprintln!("[codex_config] 切换预检：已轻量校正 {corrected} 个线程的状态库 rollout_path");
                }
                let local_cli_version = crate::codex_session_chain::local_codex_cli_version();
                let issues: Vec<_> = crate::codex_session_chain::scan_sessions_integrity(
                    &sessions_root,
                    local_cli_version.as_deref(),
                )
                .into_iter()
                .filter(|issue| issue.repairable)
                .collect();
                if !issues.is_empty() {
                    let mut payload = load_payload()?;
                    payload.session_issues = issues;
                    payload.session_issues_blocked = true;
                    return Ok(payload);
                }
            }
        }
    }
    if request.apply_to_global && !custom_global_sync_supported() {
        return Err("当前平台不支持同步 CodeX 全局配置".to_string());
    }
    let profile_path = managed_profile_path(&profile.id)?;
    let existing_profile_toml = fs::read_to_string(&profile_path)
        .map_err(|error| format!("无法读取 CodeX profile：{error}"))?;
    let profile_model_catalog_is_managed = profile.auth_mode == CodexAuthMode::Custom
        && profile.model_catalog.is_some();
    if let Some(previous) = state.managed_profile_model_catalogs.get(&profile.id) {
        validate_managed_profile_model_catalog_reference(
            Some(&existing_profile_toml),
            &profile.id,
            previous,
            profile_model_catalog_is_managed,
        )?;
    }
    let profile_model_catalog_restore = if profile_model_catalog_is_managed {
        None
    } else if let Some(previous) = state.managed_profile_model_catalogs.get(&profile.id) {
        Some(previous.clone())
    } else {
        Some(model_catalog_json_value_from_raw(Some(&existing_profile_toml))?)
    };
    let rendered_profile_toml = build_profile_toml_with_model_catalog_restore(
        Some(&existing_profile_toml),
        managed_provider_id(&profile),
        &profile,
        profile_model_catalog_restore.as_ref(),
    )?;
    let profile_needs_update = rendered_profile_toml != existing_profile_toml;
    let previous_index = load_profile_index_state(CODEX_STATE_KEY)?;
    let previous_metadata = fs::read(profiles_path()?).ok();
    let global_path = global_config_path_for_profile(&profile.id)?;
    let previous_global = fs::read(&global_path).ok();
    let global_env_record_path = global_env_path()?;
    let previous_global_env_file = fs::read(&global_env_record_path).ok();
    let previous_managed_env = load_managed_global_env()?;
    let global_codex_home_record_path = global_codex_home_env_path()?;
    let previous_global_codex_home_env_file = fs::read(&global_codex_home_record_path).ok();
    let previous_managed_codex_home_env = load_managed_global_codex_home_env()?;

    let existing_global_raw = if request.apply_to_global && global_path.exists() {
        Some(
            fs::read_to_string(&global_path)
                .map_err(|error| format!("无法读取全局 config.toml：{error}"))?,
        )
    } else {
        None
    };
    let global_model_catalog_is_managed = profile.auth_mode == CodexAuthMode::Custom
        && profile.model_catalog.is_some();
    if request.apply_to_global && !cfg!(windows) {
        if let Some(managed) = state.managed_global_model_catalog.as_ref() {
            validate_managed_global_model_catalog_reference(
                existing_global_raw.as_deref(),
                managed,
                global_model_catalog_is_managed,
            )?;
        }
    }
    let next_global_model_catalog = if request.apply_to_global
        && global_model_catalog_is_managed
        && !cfg!(windows)
    {
        let previous_value = state
            .managed_global_model_catalog
            .as_ref()
            .map(|managed| managed.previous_value.clone())
            .unwrap_or(model_catalog_json_value_from_raw(existing_global_raw.as_deref())?);
        Some(ManagedGlobalModelCatalogState {
            previous_value,
            applied_value: managed_model_catalog_path(&profile.id)?
                .to_string_lossy()
                .to_string(),
        })
    } else {
        None
    };
    let global_model_catalog_restore = if cfg!(windows) {
        Some(model_catalog_json_value_from_raw(Some(
            &rendered_profile_toml,
        ))?)
    } else if global_model_catalog_is_managed {
        None
    } else if let Some(managed) = state.managed_global_model_catalog.as_ref() {
        Some(managed.previous_value.clone())
    } else {
        Some(model_catalog_json_value_from_raw(existing_global_raw.as_deref())?)
    };
    let rendered_global = if request.apply_to_global {
        // 协议转换：全局配置被 Codex 桌面端 / VSCode 扩展等直接读取（不经启动器
        // 的 resolve 流程），这里在渲染时就接管——base_url 改写为本机转换代理，
        // 应用退出时由 sync_global_conversion_proxy 恢复真实地址。
        let mut global_render_profile = profile.clone();
        if profile.protocol_conversion && profile.auth_mode == CodexAuthMode::Custom {
            // Render the fixed endpoint first; start/rebuild the proxy only
            // after all validation and rendering steps have succeeded.
            global_render_profile.base_url = global_conversion_proxy_url(&profile.id);
        }
        let rendered = build_global_toml_with_model_catalog_restore(
            existing_global_raw.as_deref(),
            state.managed_global_provider_id.as_deref(),
            &global_render_profile,
            if global_model_catalog_is_managed {
                None
            } else {
                global_model_catalog_restore.as_ref()
            },
        )?;
        Some(
            upsert_managed_provider_items(&rendered, &state, Some(profile.id.as_str()))?
                .unwrap_or(rendered),
        )
    } else {
        None
    };
    let next_global_profile_id = if request.apply_to_global {
        Some(profile.id.clone())
    } else {
        state.global_profile_id.clone()
    };
    let next_global_provider_id = if request.apply_to_global {
        managed_provider_id(&profile).map(str::to_string)
    } else {
        state.managed_global_provider_id.clone()
    };
    let desired_model_catalogs = desired_model_catalogs(
        &state,
        Some(profile.id.as_str()),
        next_global_profile_id.as_deref(),
        next_global_provider_id.as_deref(),
        !request.apply_to_global,
    )?;
    let previous_model_catalog_files =
        model_catalog_paths_for_transition(&state, &desired_model_catalogs)?;
    let next_api_key = if request.apply_to_global
        && profile.auth_mode == CodexAuthMode::Custom
        && custom_global_key_sync_supported()
    {
        Some(resolve_profile_api_key(&profile)?)
    } else {
        None
    };

    let mut env_keys = HashSet::new();
    if let Some(previous) = previous_managed_env.as_ref() {
        env_keys.insert(previous.key.clone());
    }
    #[cfg(windows)]
    env_keys.insert(CODEX_HOME_ENV.to_string());
    if next_api_key.is_some() {
        env_keys.insert(profile.env_key.clone());
    }
    let env_snapshots = env_keys
        .into_iter()
        .map(|key| read_user_env_var(&key).map(|value| (key, value)))
        .collect::<Result<HashMap<_, _>, _>>()?;

    if request.apply_to_global
        && profile.protocol_conversion
        && profile.auth_mode == CodexAuthMode::Custom
    {
        crate::codex_proxy::ensure_conversion_fixed_port(&profile)?;
    }

    let transaction = (|| {
        if profile_needs_update {
            write_toml_atomic(&profile_path, rendered_profile_toml.as_bytes())?;
        }
        if profile_model_catalog_is_managed {
            let previous = state
                .managed_profile_model_catalogs
                .get(&profile.id)
                .cloned()
                .unwrap_or(model_catalog_json_value_from_raw(Some(&existing_profile_toml))?);
            state
                .managed_profile_model_catalogs
                .entry(profile.id.clone())
                .or_insert(previous);
        } else {
            state.managed_profile_model_catalogs.remove(&profile.id);
        }
        synchronize_model_catalogs(&mut state, &desired_model_catalogs)?;
        if let Some(rendered) = rendered_global.as_ref() {
            write_toml_atomic(&global_path, rendered.as_bytes())?;
            let next_env = next_api_key
                .as_deref()
                .map(|api_key| (profile.env_key.as_str(), api_key));
            transition_managed_global_env(next_env, previous_managed_env.as_ref())?;
            release_managed_global_codex_home_env(previous_managed_codex_home_env.as_ref())?;
            state.global_profile_id = next_global_profile_id.clone();
            state.managed_global_provider_id = next_global_provider_id.clone();
            state.managed_global_model_catalog = next_global_model_catalog.clone();
        }
        save_profile_state(&state)?;

        let index = normalize_index(
            &state.profiles,
            previous_index.order.clone(),
            Some(profile.id.clone()),
        );
        save_profile_index_state(CODEX_STATE_KEY, &index)?;
        if load_profile_index_state(CODEX_STATE_KEY)? != index {
            return Err("CodeX 活动方案写入后回读不一致".to_string());
        }
        if profile_needs_update {
            let verified_profile = fs::read_to_string(&profile_path)
                .map_err(|error| format!("无法回读 CodeX profile：{error}"))?;
            if verified_profile != rendered_profile_toml {
                return Err("CodeX profile 写入后回读不一致".to_string());
            }
        }

        if let Some(rendered) = rendered_global.as_ref() {
            let verified_global = fs::read_to_string(&global_path)
                .map_err(|error| format!("无法回读全局 config.toml：{error}"))?;
            if verified_global != *rendered {
                return Err("全局 config.toml 写入后回读不一致".to_string());
            }
            DocumentMut::from_str(&verified_global)
                .map_err(|error| format!("全局 config.toml 回读校验失败：{error}"))?;
            if load_profile_state()? != state {
                return Err("CodeX 全局应用状态写入后回读不一致".to_string());
            }
            match next_api_key.as_deref() {
                Some(api_key) => {
                    let verified = load_managed_global_env()?
                        .ok_or_else(|| "CodeX 全局环境变量记录不存在".to_string())?;
                    if verified.key != profile.env_key || verified.applied_value != api_key {
                        return Err("CodeX 全局环境变量记录写入后回读不一致".to_string());
                    }
                    if read_user_env_var(&profile.env_key)?.as_deref() != Some(api_key) {
                        return Err("CodeX 全局环境变量写入后回读不一致".to_string());
                    }
                }
                None if load_managed_global_env()?.is_some() => {
                    return Err("切换官方配置后仍存在启动器管理的全局 API Key".to_string());
                }
                None => {}
            }
            #[cfg(windows)]
            if load_managed_global_codex_home_env()?.is_some() {
                return Err("切换共享配置后仍存在启动器管理的 CODEX_HOME".to_string());
            }
        }
        Ok(())
    })();

    if let Err(error) = transaction {
        let mut rollback_errors = Vec::new();
        if let Err(rollback) = save_profile_index_state(CODEX_STATE_KEY, &previous_index) {
            rollback_errors.push(rollback);
        }
        if profile_needs_update {
            if let Err(rollback) = restore_snapshot(
                &profile_path,
                Some(existing_profile_toml.as_bytes()),
                SnapshotKind::Toml,
            ) {
                rollback_errors.push(rollback);
            }
        }
        if let Err(rollback) = restore_model_catalog_paths(&previous_model_catalog_files) {
            rollback_errors.push(rollback);
        }
        if let Err(rollback) = restore_snapshot(
            &profiles_path()?,
            previous_metadata.as_deref(),
            SnapshotKind::Json,
        ) {
            rollback_errors.push(rollback);
        }
        if request.apply_to_global {
            if let Err(rollback) =
                restore_snapshot(&global_path, previous_global.as_deref(), SnapshotKind::Toml)
            {
                rollback_errors.push(rollback);
            }
            if let Err(rollback) = restore_snapshot(
                &global_env_record_path,
                previous_global_env_file.as_deref(),
                SnapshotKind::Credential,
            ) {
                rollback_errors.push(rollback);
            }
            if let Err(rollback) = restore_snapshot(
                &global_codex_home_record_path,
                previous_global_codex_home_env_file.as_deref(),
                SnapshotKind::Credential,
            ) {
                rollback_errors.push(rollback);
            }
            if let Err(rollback) = restore_user_env_snapshots(&env_snapshots) {
                rollback_errors.push(rollback);
            }
            // The proxy may have been rebuilt before the file transaction.
            // Restore the persisted previous global profile's proxy state too.
            if let Err(recovery) = sync_global_conversion_proxy() {
                rollback_errors.push(format!("恢复协议转换代理失败：{recovery}"));
            }
        }
        if rollback_errors.is_empty() {
            return Err(format!("应用 CodeX 配置失败，旧数据已恢复：{error}"));
        }
        return Err(format!(
            "应用 CodeX 配置失败且回滚不完整：{error}；{}",
            rollback_errors.join("；")
        ));
    }

    // 全局协议转换接管状态可能随全局 profile 切换而变化（best-effort）。
    if let Err(error) = sync_global_conversion_proxy() {
        eprintln!("CodeX 全局协议转换代理同步失败：{error}");
    }
    load_payload()
}

#[tauri::command]
pub fn delete_codex_profile(
    request: DeleteCodexProfileRequest,
) -> Result<CodexProfilesPayload, String> {
    let metadata_path = profiles_path()?;
    let profile_path = managed_profile_path(&request.profile_id)?;
    let mut state = load_profile_state()?;
    let previous_index = load_profile_index_state(CODEX_STATE_KEY)?;
    if !state
        .profiles
        .iter()
        .any(|profile| profile.id == request.profile_id)
    {
        return Err("要删除的 CodeX 配置不存在".to_string());
    }
    if profile_is_applied(&state, &previous_index, &request.profile_id) {
        return Err("不能删除应用中的配置".to_string());
    }
    let deleted_profile = state
        .profiles
        .iter()
        .find(|profile| profile.id == request.profile_id)
        .cloned()
        .ok_or_else(|| "要删除的 CodeX 配置不存在".to_string())?;
    let global_path = global_config_path()?;
    let previous_global = fs::read(&global_path).ok();
    let previous_metadata = fs::read(&metadata_path).ok();
    let previous_toml = fs::read(&profile_path).ok();
    let previous_secret = read_profile_secret(&request.profile_id)?;
    state
        .profiles
        .retain(|profile| profile.id != request.profile_id);
    state
        .managed_profile_model_catalogs
        .remove(&request.profile_id);
    if state.global_profile_id.as_deref() == Some(request.profile_id.as_str()) {
        state.global_profile_id = None;
        state.managed_global_provider_id = None;
        state.managed_global_model_catalog = None;
    }

    let transaction = (|| {
        remove_if_exists(&profile_path)?;
        remove_transaction_sidecars(&profile_path)?;
        delete_profile_secret(&request.profile_id)?;
        if deleted_profile.protocol_conversion
            && deleted_profile.auth_mode == CodexAuthMode::Custom
            && global_path.exists()
        {
            let raw = fs::read_to_string(&global_path)
                .map_err(|error| format!("无法读取全局 config.toml: {error}"))?;
            if let Some(restored) = restore_deleted_global_proxy_base_url(&raw, &deleted_profile)? {
                write_toml_atomic(&global_path, restored.as_bytes())?;
            }
        }
        save_profile_state(&state)?;
        let index = normalize_index(
            &state.profiles,
            request.order.clone(),
            request.active_profile_id.clone(),
        );
        save_profile_index_state(CODEX_STATE_KEY, &index)?;
        if load_profile_state()? != state || load_profile_index_state(CODEX_STATE_KEY)? != index {
            return Err("删除 CodeX 配置后回读校验不一致".to_string());
        }
        Ok(())
    })();

    if let Err(error) = transaction {
        let mut rollback_errors = Vec::new();
        if let Err(rollback) = save_profile_index_state(CODEX_STATE_KEY, &previous_index) {
            rollback_errors.push(rollback);
        }
        for (path, snapshot, kind) in [
            (
                &metadata_path,
                previous_metadata.as_deref(),
                SnapshotKind::Json,
            ),
            (&profile_path, previous_toml.as_deref(), SnapshotKind::Toml),
        ] {
            if let Err(rollback) = restore_snapshot(path, snapshot, kind) {
                rollback_errors.push(rollback);
            }
        }
        if let Err(rollback) =
            restore_profile_secret(&request.profile_id, previous_secret.as_deref())
        {
            rollback_errors.push(rollback);
        }
        if let Err(rollback) = restore_snapshot(
            &global_path,
            previous_global.as_deref(),
            SnapshotKind::Toml,
        ) {
            rollback_errors.push(rollback);
        }
        if rollback_errors.is_empty() {
            return Err(format!("删除 CodeX 配置失败，旧数据已恢复：{error}"));
        }
        return Err(format!(
            "删除 CodeX 配置失败且回滚不完整：{error}；{}",
            rollback_errors.join("；")
        ));
    }

    // 恢复被删除 provider 的稳定代理地址并清理代理
    // （best-effort：删除本身已成功）。
    crate::codex_proxy::stop(&request.profile_id);
    crate::codex_proxy::forget_history(&request.profile_id);
    if let Err(error) = sync_global_conversion_proxy() {
        eprintln!("CodeX 全局协议转换代理同步失败：{error}");
    }
    load_payload()
}

/// 判断 base_url 是否指向全局协议转换代理的固定地址。
fn is_conversion_proxy_url(base_url: &str) -> bool {
    Url::parse(base_url).is_ok_and(|url| {
        url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.path().trim_end_matches('/') == "/v1"
            && url
                .port()
                .is_some_and(crate::codex_proxy::is_global_proxy_port)
    })
}

/// 用 toml_edit 精确替换全局 config.toml 中 provider 的 base_url，保留其余
/// 用户手动改动。返回 Some(新内容) 表示发生替换；None 表示无需改动或无法定位。
fn rewrite_global_provider_base_url(
    raw: &str,
    provider_id: &str,
    base_url: &str,
) -> Result<Option<String>, String> {
    let mut document = DocumentMut::from_str(raw)
        .map_err(|error| format!("全局 config.toml 无法解析：{error}"))?;
    let Some(provider) = document
        .get_mut("model_providers")
        .and_then(Item::as_table_mut)
        .and_then(|providers| providers.get_mut(provider_id))
        .and_then(Item::as_table_mut)
    else {
        return Ok(None);
    };
    let Some(current) = provider
        .get("base_url")
        .and_then(Item::as_value)
        .and_then(|value| value.as_str())
    else {
        return Ok(None);
    };
    if current == base_url {
        return Ok(None);
    }
    provider["base_url"] = value(base_url);
    Ok(Some(document.to_string()))
}

fn global_conversion_proxy_url(profile_id: &str) -> String {
    crate::codex_proxy::global_proxy_url(profile_id)
}

/// Restore a deleted global conversion profile only when its provider still
/// points at this application's fixed global proxy port.
fn restore_deleted_global_proxy_base_url(
    raw: &str,
    profile: &CodexProfile,
) -> Result<Option<String>, String> {
    let proxy_url = global_conversion_proxy_url(&profile.id);
    let document = DocumentMut::from_str(raw)
        .map_err(|error| format!("全局 config.toml 无法解析：{error}"))?;
    let current = document
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(&profile.provider_id))
        .and_then(Item::as_table)
        .and_then(|provider| provider.get("base_url"))
        .and_then(Item::as_value)
        .and_then(|value| value.as_str());
    if current != Some(proxy_url.as_str()) {
        return Ok(None);
    }
    rewrite_global_provider_base_url(raw, &profile.provider_id, &profile.base_url)
}

fn hash_sync_fingerprint_part(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn global_proxy_sync_fingerprint(
    state: &CodexProfileState,
    global_config: Option<&str>,
    secret_fingerprints: &[(String, [u8; 32])],
) -> Result<GlobalProxySyncFingerprint, String> {
    let mut hasher = Sha256::new();
    let state_bytes = serde_json::to_vec(state)
        .map_err(|error| format!("无法计算 CodeX 代理状态指纹：{error}"))?;
    hash_sync_fingerprint_part(&mut hasher, &state_bytes);
    match global_config {
        Some(raw) => {
            hasher.update([1]);
            hash_sync_fingerprint_part(&mut hasher, raw.as_bytes());
        }
        None => hasher.update([0]),
    }
    for (profile_id, secret_fingerprint) in secret_fingerprints {
        hash_sync_fingerprint_part(&mut hasher, profile_id.as_bytes());
        hash_sync_fingerprint_part(&mut hasher, secret_fingerprint);
    }
    Ok(GlobalProxySyncFingerprint(hasher.finalize().into()))
}

fn resolve_conversion_secret_fingerprints(
    conversion_profiles: &[CodexProfile],
) -> Result<Vec<(String, [u8; 32])>, String> {
    conversion_profiles
        .iter()
        .map(|profile| {
            let secret = resolve_profile_api_key(profile)?;
            Ok((profile.id.clone(), Sha256::digest(secret.as_bytes()).into()))
        })
        .collect()
}

fn global_proxy_sync_is_reusable(fingerprint: &GlobalProxySyncFingerprint) -> bool {
    let expected_profiles = global_proxy_sync_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .filter(|cached| &cached.fingerprint == fingerprint)
        .map(|cached| cached.expected_profiles.clone());
    expected_profiles
        .as_deref()
        .is_some_and(crate::codex_proxy::global_instances_match)
}

fn remember_global_proxy_sync(
    fingerprint: GlobalProxySyncFingerprint,
    expected_profiles: Vec<CodexProfile>,
) {
    *global_proxy_sync_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(GlobalProxySyncCacheEntry {
        fingerprint,
        expected_profiles,
    });
}

fn invalidate_global_proxy_sync_cache() {
    *global_proxy_sync_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Keep every shared historical provider usable. Each conversion profile owns
/// a deterministic port, so switching the default profile cannot strand an old
/// rollout on a stopped single-port proxy.
pub fn sync_global_conversion_proxy() -> Result<(), String> {
    let _sync_guard = global_proxy_sync_lock()
        .lock()
        .map_err(|_| "CodeX 全局协议转换同步锁不可用".to_string())?;
    let state = load_profile_state()?;
    let conversion_profiles = state
        .profiles
        .iter()
        .filter(|profile| {
            profile.auth_mode == CodexAuthMode::Custom && profile.protocol_conversion
        })
        .cloned()
        .collect::<Vec<_>>();
    let keep_profile_ids = conversion_profiles
        .iter()
        .map(|profile| profile.id.clone())
        .collect::<HashSet<_>>();
    let mut port_owners = HashMap::new();
    for profile in &conversion_profiles {
        let port = crate::codex_proxy::global_proxy_port(&profile.id);
        if let Some(previous) = port_owners.insert(port, profile.id.clone()) {
            return Err(format!(
                "CodeX 协议转换 profile '{}' 与 '{}' 的稳定端口 {port} 冲突",
                previous, profile.id
            ));
        }
    }

    let global_path = global_config_path()?;
    if !global_path.exists() {
        let fingerprint = global_proxy_sync_fingerprint(&state, None, &[])?;
        if global_proxy_sync_is_reusable(&fingerprint) {
            #[cfg(debug_assertions)]
            eprintln!("[startup] codex-global-proxy-cache: hit");
            return Ok(());
        }
        crate::codex_proxy::stop_global_instances_except(&HashSet::new());
        remember_global_proxy_sync(fingerprint, Vec::new());
        return Ok(());
    }
    let mut raw = fs::read_to_string(&global_path)
        .map_err(|error| format!("无法读取全局 config.toml：{error}"))?;
    let original_raw = raw.clone();
    if state.global_profile_id.is_some() && !global_profile_is_recoverable_at_startup(&state, &raw)
    {
        // 全局文件已被外部修改或状态无法对应。保留文件，交给 UI
        // 让用户确认后重新同步，避免启动时静默覆盖手动配置。
        crate::codex_proxy::stop_global_instances_except(&HashSet::new());
        let fingerprint = global_proxy_sync_fingerprint(&state, Some(&raw), &[])?;
        remember_global_proxy_sync(fingerprint, Vec::new());
        return Ok(());
    }

    let repaired_missing_provider = if let Some(repaired) =
        restore_missing_managed_global_providers(&raw, &state)?
    {
        raw = repaired;
        true
    } else {
        false
    };

    let secret_fingerprints = resolve_conversion_secret_fingerprints(&conversion_profiles)?;
    let fingerprint =
        global_proxy_sync_fingerprint(&state, Some(&raw), &secret_fingerprints)?;
    if !repaired_missing_provider && global_proxy_sync_is_reusable(&fingerprint) {
        #[cfg(debug_assertions)]
        eprintln!("[startup] codex-global-proxy-cache: hit");
        return Ok(());
    }

    for profile in &conversion_profiles {
        let Some(local_url) = crate::codex_proxy::ensure_conversion_fixed_port(profile)? else {
            continue;
        };
        if let Some(rewritten) =
            rewrite_global_provider_base_url(&raw, &profile.provider_id, &local_url)?
        {
            raw = rewritten;
        }
    }

    for profile in state
        .profiles
        .iter()
        .filter(|profile| profile.auth_mode == CodexAuthMode::Custom && !profile.protocol_conversion)
    {
        if let Some(restored) = restore_deleted_global_proxy_base_url(&raw, profile)? {
            raw = restored;
        }
    }
    if raw != original_raw {
        write_toml_atomic(&global_path, raw.as_bytes())?;
    }
    crate::codex_proxy::stop_global_instances_except(&keep_profile_ids);
    let final_fingerprint = if raw == original_raw {
        fingerprint
    } else {
        global_proxy_sync_fingerprint(&state, Some(&raw), &secret_fingerprints)?
    };
    remember_global_proxy_sync(final_fingerprint, conversion_profiles);
    Ok(())
}

/// Restore the global config before process exit and stop every in-process
/// proxy. The runtime sync path intentionally keeps an enabled global proxy
/// active, so shutdown must use a separate operation.
pub fn restore_global_conversion_proxy() -> Result<(), String> {
    let _sync_guard = global_proxy_sync_lock()
        .lock()
        .map_err(|_| "CodeX 全局协议转换同步锁不可用".to_string())?;
    invalidate_global_proxy_sync_cache();
    let state = load_profile_state()?;
    let global_path = global_config_path()?;
    if global_path.exists() {
        let raw = fs::read_to_string(&global_path)
            .map_err(|error| format!("无法读取全局 config.toml：{error}"))?;
        let mut restored = raw.clone();
        for profile in state.profiles.iter().filter(|profile| {
            profile.protocol_conversion && profile.auth_mode == CodexAuthMode::Custom
        }) {
            if let Some(next) = restore_deleted_global_proxy_base_url(&restored, profile)? {
                restored = next;
            }
        }
        if restored != raw {
            write_toml_atomic(&global_path, restored.as_bytes())?;
        }
    }
    crate::codex_proxy::stop_all();
    invalidate_global_proxy_sync_cache();
    Ok(())
}

#[tauri::command]
pub fn resolve_codex_profile(profile_id: String) -> Result<CodexLaunchContext, String> {
    let mut state = load_profile_state()?;
    enrich_profiles(&mut state)?;
    let profile = state
        .profiles
        .iter()
        .find(|profile| profile.id == profile_id)
        .cloned()
        .ok_or_else(|| format!("CodeX 配置方案 '{profile_id}' 不存在"))?;
    let profile_path = managed_profile_path(&profile.id)?;
    if !profile_path.exists() {
        return Err(format!(
            "CodeX profile 文件不存在：{}",
            profile_path.display()
        ));
    }
    let existing_toml = fs::read_to_string(&profile_path)
        .map_err(|error| format!("无法读取 CodeX profile：{error}"))?;
    let profile_model_catalog_is_managed = profile.auth_mode == CodexAuthMode::Custom
        && profile.model_catalog.is_some();
    if let Some(previous) = state.managed_profile_model_catalogs.get(&profile.id) {
        validate_managed_profile_model_catalog_reference(
            Some(&existing_toml),
            &profile.id,
            previous,
            profile_model_catalog_is_managed,
        )?;
    }
    let profile_model_catalog_restore = if profile_model_catalog_is_managed {
        None
    } else if let Some(previous) = state.managed_profile_model_catalogs.get(&profile.id) {
        Some(previous.clone())
    } else {
        Some(model_catalog_json_value_from_raw(Some(&existing_toml))?)
    };
    // 协议转换：启用时确保本机转换代理在运行，并把 profile TOML 的 base_url
    // 临时改写为代理地址；未启用时保持现有逻辑（真实 base_url）完全不变。
    // 代理注册表仅存于内存，profile state 中的 base_url 始终是真实地址。
    let conversion_local_url = crate::codex_proxy::ensure_conversion(&profile)?;
    let mut render_profile = profile.clone();
    if let Some(local_url) = conversion_local_url.as_deref() {
        render_profile.base_url = local_url.to_string();
    }
    let rendered_toml = build_profile_toml_with_model_catalog_restore(
        Some(&existing_toml),
        managed_provider_id(&profile),
        &render_profile,
        profile_model_catalog_restore.as_ref(),
    )?;
    let mut env_vars = BTreeMap::new();
    let shared_home_text = codex_home()?.to_string_lossy().to_string();
    // Override a CODEX_HOME inherited by a launcher process that started before
    // the shared-home migration. New desktop/terminal processes converge on
    // the shared store immediately; no per-profile SQLite override is set.
    env_vars.insert(CODEX_HOME_ENV.to_string(), shared_home_text);
    if profile.auth_mode == CodexAuthMode::Custom && !uses_command_auth(&profile) {
        let api_key = resolve_profile_api_key(&profile)?;
        env_vars.insert(profile.env_key.clone(), api_key);
    }
    let model_provider = match profile.auth_mode {
        CodexAuthMode::Official => "openai".to_string(),
        CodexAuthMode::Custom => profile.provider_id.clone(),
    };

    let previous_model_catalog_state = state.managed_model_catalogs.clone();
    let desired_model_catalogs = desired_model_catalogs(
        &state,
        Some(profile.id.as_str()),
        state.global_profile_id.as_deref(),
        state.managed_global_provider_id.as_deref(),
        true,
    )?;
    let previous_model_catalog_files =
        model_catalog_paths_for_transition(&state, &desired_model_catalogs)?;
    let previous_metadata = fs::read(profiles_path()?).ok();
    let profile_needs_update = rendered_toml != existing_toml;
    let transaction: Result<(), String> = (|| {
        if profile_needs_update {
            write_toml_atomic(&profile_path, rendered_toml.as_bytes())?;
        }
        synchronize_model_catalogs(&mut state, &desired_model_catalogs)?;
        if state.managed_model_catalogs != previous_model_catalog_state {
            save_profile_state(&state)?;
        }
        Ok(())
    })();
    if let Err(error) = transaction {
        let mut rollback_errors = Vec::new();
        state.managed_model_catalogs = previous_model_catalog_state;
        if let Err(rollback) = restore_model_catalog_paths(&previous_model_catalog_files) {
            rollback_errors.push(rollback);
        }
        if profile_needs_update {
            if let Err(rollback) = restore_snapshot(
                &profile_path,
                Some(existing_toml.as_bytes()),
                SnapshotKind::Toml,
            ) {
                rollback_errors.push(rollback);
            }
        }
        if let Err(rollback) = restore_snapshot(
            &profiles_path()?,
            previous_metadata.as_deref(),
            SnapshotKind::Json,
        ) {
            rollback_errors.push(rollback);
        }
        if rollback_errors.is_empty() {
            return Err(format!("启动前 CodeX 配置准备失败，旧数据已恢复：{error}"));
        }
        return Err(format!(
            "启动前 CodeX 配置准备失败且回滚不完整：{error}；{}",
            rollback_errors.join("；")
        ));
    }

    Ok(CodexLaunchContext {
        managed_profile_name: profile.managed_profile_name,
        model_provider,
        env_vars,
    })
}

fn global_model_provider() -> Result<String, String> {
    let path = global_config_path()?;
    if !path.exists() {
        return Ok("openai".to_string());
    }
    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("无法读取全局 config.toml：{error}"))?;
    let document = DocumentMut::from_str(&raw)
        .map_err(|error| format!("全局 config.toml 无法解析：{error}"))?;
    Ok(document
        .get("model_provider")
        .and_then(Item::as_value)
        .and_then(|item| item.as_str())
        .filter(|provider| !provider.trim().is_empty())
        .unwrap_or("openai")
        .to_string())
}

pub fn resolve_codex_runtime_context(
    profile_id: Option<&str>,
) -> Result<CodexRuntimeContext, String> {
    if let Some(profile_id) = profile_id {
        let launch = resolve_codex_profile(profile_id.to_string())?;
        return Ok(CodexRuntimeContext {
            profile_name: Some(launch.managed_profile_name),
            model_provider: launch.model_provider,
            env_vars: launch.env_vars,
            cache_key: format!("profile:{profile_id}"),
        });
    }

    let model_provider = global_model_provider()?;
    let global_home = codex_home()?;
    fs::create_dir_all(&global_home)
        .map_err(|error| format!("无法创建 CodeX 全局数据目录：{error}"))?;
    let global_home_text = global_home.to_string_lossy().to_string();
    let env_vars = BTreeMap::from([(CODEX_HOME_ENV.to_string(), global_home_text.clone())]);
    Ok(CodexRuntimeContext {
        profile_name: None,
        model_provider: model_provider.clone(),
        env_vars,
        cache_key: format!("global:{model_provider}:{global_home_text}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct MemorySecretStore {
        values: RefCell<HashMap<String, String>>,
    }

    impl ProfileSecretStore for MemorySecretStore {
        fn read(&self, profile_id: &str) -> Result<Option<String>, String> {
            Ok(self.values.borrow().get(profile_id).cloned())
        }

        fn write(&self, profile_id: &str, secret: &str) -> Result<(), String> {
            self.values
                .borrow_mut()
                .insert(profile_id.to_string(), secret.to_string());
            Ok(())
        }

        fn delete(&self, profile_id: &str) -> Result<(), String> {
            self.values.borrow_mut().remove(profile_id);
            Ok(())
        }
    }

    fn official_profile() -> CodexProfile {
        CodexProfile {
            id: "profile-test".to_string(),
            name: "Official".to_string(),
            auth_mode: CodexAuthMode::Official,
            model: "gpt-5.6".to_string(),
            reasoning_effort: "high".to_string(),
            model_context_window: None,
            model_context_window_configured: false,
            model_auto_compact_ratio: None,
            model_auto_compact_ratio_configured: false,
            openai_base_url: String::new(),
            provider_id: String::new(),
            provider_name: String::new(),
            base_url: String::new(),
            wire_api: default_wire_api(),
            protocol_conversion: false,
            env_key: default_env_key(),
            has_stored_api_key: false,
            managed_profile_name: String::new(),
            model_catalog: None,
            chat_upstream_model: String::new(),
            prompt_cache_routing: default_prompt_cache_routing(),
            extra: Map::new(),
        }
    }

    fn custom_profile_with_catalog() -> CodexProfile {
        let mut profile = official_profile();
        profile.auth_mode = CodexAuthMode::Custom;
        profile.model = "deepseek-v4-flash".to_string();
        profile.provider_id = "deepseek".to_string();
        profile.provider_name = "DeepSeek".to_string();
        profile.base_url = "https://api.deepseek.com/".to_string();
        profile.model_catalog = Some(CodexModelCatalog {
            models: vec![CodexModelDefinition {
                slug: "deepseek-v4-flash".to_string(),
                display_name: "DeepSeek-V4-Flash".to_string(),
                input_modalities: vec!["text".to_string()],
                supports_image_detail_original: false,
                context_window: 1_048_576,
                max_context_window: 1_048_576,
                effective_context_window_percent: 95,
                truncation_policy: Some(CodexTruncationPolicy {
                    mode: "tokens".to_string(),
                    limit: 10_000,
                    extra: Map::new(),
                }),
                default_reasoning_level: "high".to_string(),
                supported_reasoning_levels: vec![
                    CodexReasoningLevel {
                        effort: "low".to_string(),
                        description: "Fast responses".to_string(),
                        extra: Map::new(),
                    },
                    CodexReasoningLevel {
                        effort: "high".to_string(),
                        description: "Deep reasoning".to_string(),
                        extra: Map::new(),
                    },
                ],
                extra: Map::new(),
            }],
            extra: Map::new(),
        });
        profile
    }

    #[test]
    fn global_proxy_sync_fingerprint_tracks_config_profile_and_secret_revisions() {
        let mut profile = custom_profile_with_catalog();
        profile.protocol_conversion = true;
        let mut state = CodexProfileState::default();
        state.profiles.push(profile);
        let secrets = vec![("profile-test".to_string(), [1_u8; 32])];
        let baseline = global_proxy_sync_fingerprint(
            &state,
            Some("model = 'first'"),
            &secrets,
        )
        .expect("baseline fingerprint");

        let changed_config = global_proxy_sync_fingerprint(
            &state,
            Some("model = 'second'"),
            &secrets,
        )
        .expect("config fingerprint");
        let changed_secret = global_proxy_sync_fingerprint(
            &state,
            Some("model = 'first'"),
            &[("profile-test".to_string(), [2_u8; 32])],
        )
        .expect("secret fingerprint");
        state.profiles[0].base_url = "https://changed.example.com/v1".to_string();
        let changed_profile = global_proxy_sync_fingerprint(
            &state,
            Some("model = 'first'"),
            &secrets,
        )
        .expect("profile fingerprint");

        assert_ne!(baseline, changed_config);
        assert_ne!(baseline, changed_secret);
        assert_ne!(baseline, changed_profile);
    }

    #[test]
    fn credential_store_contract_supports_profile_isolation_delete_and_rollback() {
        let store = MemorySecretStore::default();
        store.write("profile-a", "secret-a").expect("write a");
        store.write("profile-b", "secret-b").expect("write b");
        assert_eq!(
            store.read("profile-a").expect("read a").as_deref(),
            Some("secret-a")
        );
        assert_eq!(
            store.read("profile-b").expect("read b").as_deref(),
            Some("secret-b")
        );

        let snapshot = store.read("profile-a").expect("snapshot");
        store.write("profile-a", "replacement").expect("replace");
        restore_profile_secret_with(&store, "profile-a", snapshot.as_deref()).expect("rollback");
        assert_eq!(store.read("profile-a").expect("read restored"), snapshot);

        restore_profile_secret_with(&store, "profile-b", None).expect("delete");
        assert_eq!(store.read("profile-b").expect("read deleted"), None);
    }

    #[test]
    fn applied_profile_secret_guard_only_blocks_secret_removal() {
        let mut profile = official_profile();
        assert!(would_remove_profile_secret(&profile, false, Some("secret")));
        assert!(!would_remove_profile_secret(&profile, false, None));

        profile.auth_mode = CodexAuthMode::Custom;
        assert!(!would_remove_profile_secret(&profile, false, Some("secret")));
        assert!(would_remove_profile_secret(&profile, true, Some("secret")));
        assert!(!would_remove_profile_secret(&profile, true, None));
    }

    #[test]
    fn applied_profile_delete_guard_blocks_active_and_global_profiles() {
        let mut state = CodexProfileState::default();
        let mut index = ProfileIndexState {
            order: vec!["profile-active".to_string(), "profile-global".to_string()],
            profile_ids: BTreeMap::new(),
            active_profile_id: Some("profile-active".to_string()),
        };
        state.global_profile_id = Some("profile-global".to_string());

        assert!(profile_is_applied(&state, &index, "profile-active"));
        assert!(profile_is_applied(&state, &index, "profile-global"));
        assert!(!profile_is_applied(&state, &index, "profile-other"));

        index.active_profile_id = None;
        state.global_profile_id = None;
        assert!(!profile_is_applied(&state, &index, "profile-active"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_reports_plaintext_storage_and_disables_user_env_sync() {
        assert_eq!(secret_storage_kind(), "macos_plaintext");
        assert!(custom_global_sync_supported());
        assert!(!custom_global_key_sync_supported());
    }

    #[cfg(windows)]
    #[test]
    fn windows_dpapi_profile_uses_command_auth_without_plaintext_user_env() {
        let mut profile = official_profile();
        profile.auth_mode = CodexAuthMode::Custom;
        profile.name = "Company Proxy".to_string();
        profile.base_url = "https://proxy.example.com/v1".to_string();
        profile.has_stored_api_key = true;
        let profile = normalize_profile(profile).expect("valid profile");
        let rendered = build_profile_toml(None, None, &profile).expect("render profile");
        let document = DocumentMut::from_str(&rendered).expect("parse profile");
        let provider = document["model_providers"][profile.provider_id.as_str()]
            .as_table()
            .expect("provider table");
        assert!(provider.get("env_key").is_none());
        assert_eq!(
            provider["auth"]["command"].as_str(),
            Some("powershell.exe")
        );
        assert!(provider["auth"]["args"]
            .as_array()
            .expect("auth args")
            .iter()
            .filter_map(|value| value.as_str())
            .any(|value| value.contains("profile-test.bin")));
        assert!(!custom_global_key_sync_supported());
    }

    #[cfg(windows)]
    #[test]
    fn windows_dpapi_command_auth_round_trips_the_stored_secret() {
        let directory = std::env::temp_dir().join(format!("codex-command-auth-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("create command auth directory");
        let credential = directory.join("profile.bin");
        fs::write(
            &credential,
            protect_secret("test-command-secret").expect("protect test secret"),
        )
        .expect("write encrypted credential");
        let script = windows_dpapi_auth_script(&credential);
        let output = std::process::Command::new("powershell.exe")
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
            .arg(script)
            .output()
            .expect("run command auth");
        assert!(
            output.status.success(),
            "PowerShell command auth failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"test-command-secret");
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_plaintext_credentials_round_trip_in_private_json() {
        use std::os::unix::fs::PermissionsExt;

        let directory = std::env::temp_dir().join(format!("codex-credentials-{}", Uuid::new_v4()));
        let path = directory.join("credentials.json");
        let mut credentials = BTreeMap::new();
        credentials.insert("profile-a".to_string(), "secret-a".to_string());
        credentials.insert("profile-b".to_string(), "secret-b".to_string());

        save_plaintext_credentials_to(&path, &credentials).expect("save credentials");

        assert_eq!(
            load_plaintext_credentials_from(&path).expect("load credentials"),
            credentials
        );
        assert_eq!(
            fs::metadata(&path)
                .expect("credential metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&directory)
                .expect("credential directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let output = std::process::Command::new("/usr/bin/plutil")
            .args(["-extract", "profile-a", "raw"])
            .arg(&path)
            .output()
            .expect("run plutil");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "secret-a");
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_custom_global_sync_uses_plaintext_file_command_auth() {
        let mut profile = official_profile();
        profile.auth_mode = CodexAuthMode::Custom;
        profile.provider_id = "company_proxy".to_string();
        profile.provider_name = "Company Proxy".to_string();
        profile.base_url = "https://proxy.example.com/v1".to_string();
        profile.has_stored_api_key = true;
        let profile = normalize_profile(profile).expect("valid profile");

        let rendered = build_global_toml(None, None, &profile).expect("render global config");
        let document = DocumentMut::from_str(&rendered).expect("parse");
        let provider = document["model_providers"][profile.provider_id.as_str()]
            .as_table()
            .expect("provider table");
        assert!(provider.get("env_key").is_none());
        assert_eq!(
            provider["auth"]["command"].as_str(),
            Some("/usr/bin/plutil")
        );
        let arguments = provider["auth"]["args"]
            .as_array()
            .expect("auth arguments")
            .iter()
            .filter_map(TomlValue::as_str)
            .collect::<Vec<_>>();
        assert_eq!(&arguments[..3], ["-extract", profile.id.as_str(), "raw"]);
        assert!(arguments[3].ends_with("AgentsLauncher/codex/credentials.json"));
        assert!(!rendered.contains("sk-test-secret"));
    }

    #[test]
    fn official_profile_preserves_unknown_toml_tables() {
        let existing = "[features]\njs_repl = true\n";
        let profile = official_profile();
        let rendered = build_profile_toml(Some(existing), None, &profile).expect("render");
        let document = DocumentMut::from_str(&rendered).expect("parse");
        assert_eq!(document["model"].as_str(), Some("gpt-5.6"));
        assert_eq!(document["model_provider"].as_str(), Some("openai"));
        assert_eq!(document["features"]["js_repl"].as_bool(), Some(true));
    }

    #[test]
    fn official_profile_renders_model_context_window() {
        let mut profile = official_profile();
        profile.model_context_window = Some(1_050_000);
        let rendered = build_profile_toml(None, None, &profile).expect("render");
        let document = DocumentMut::from_str(&rendered).expect("parse");
        assert_eq!(
            document["model_context_window"].as_integer(),
            Some(1_050_000)
        );
    }

    #[test]
    fn official_profile_can_remove_model_context_window() {
        let existing = "model_context_window = 1000000\n";
        let profile = official_profile();
        let rendered = build_profile_toml(Some(existing), None, &profile).expect("render");
        let document = DocumentMut::from_str(&rendered).expect("parse");
        assert!(document.get("model_context_window").is_none());
    }

    #[test]
    fn official_profile_renders_model_auto_compact_token_limit_from_ratio() {
        let mut profile = official_profile();
        profile.model_context_window = Some(1_048_576);
        profile.model_auto_compact_ratio = Some(0.8);
        let profile = normalize_profile(profile).expect("valid profile");
        assert_eq!(profile.model_auto_compact_ratio, Some(0.8));
        let rendered = build_profile_toml(None, None, &profile).expect("render");
        let document = DocumentMut::from_str(&rendered).expect("parse");
        assert_eq!(
            document["model_auto_compact_token_limit"].as_integer(),
            Some(838_861)
        );
    }

    #[test]
    fn official_profile_removes_model_auto_compact_token_limit_when_ratio_is_empty() {
        let existing = "model_auto_compact_token_limit = 800000\n";
        let profile = official_profile();
        let rendered = build_profile_toml(Some(existing), None, &profile).expect("render");
        let document = DocumentMut::from_str(&rendered).expect("parse");
        assert!(document.get("model_auto_compact_token_limit").is_none());
    }

    #[test]
    fn model_auto_compact_ratio_is_ignored_without_context_window() {
        let mut profile = official_profile();
        profile.model_auto_compact_ratio = Some(0.8);
        let normalized = normalize_profile(profile).expect("valid profile");
        assert_eq!(normalized.model_auto_compact_ratio, None);
        assert!(!normalized.model_auto_compact_ratio_configured);

        let rendered = build_profile_toml(
            Some("model_auto_compact_token_limit = 800000\n"),
            None,
            &normalized,
        )
        .expect("render");
        let document = DocumentMut::from_str(&rendered).expect("parse");
        assert!(document.get("model_auto_compact_token_limit").is_none());
    }

    fn custom_profile_for_context_tests() -> CodexProfile {
        let mut profile = official_profile();
        profile.auth_mode = CodexAuthMode::Custom;
        profile.name = "Kimi".to_string();
        profile.provider_id = "kimi".to_string();
        profile.provider_name = "Kimi".to_string();
        profile.base_url = "https://api.moonshot.cn/v1/".to_string();
        profile.model = "k3-256k".to_string();
        profile
    }

    #[test]
    fn custom_profile_renders_model_context_window_and_auto_compact_token_limit() {
        let mut profile = custom_profile_for_context_tests();
        profile.model_context_window = Some(262_144);
        profile.model_auto_compact_ratio = Some(0.9);
        let profile = normalize_profile(profile).expect("valid profile");
        assert_eq!(profile.model_auto_compact_ratio, Some(0.9));
        assert!(profile.model_auto_compact_ratio_configured);

        let rendered = build_profile_toml(None, None, &profile).expect("render");
        let document = DocumentMut::from_str(&rendered).expect("parse");
        assert_eq!(
            document["model_context_window"].as_integer(),
            Some(262_144)
        );
        assert_eq!(
            document["model_auto_compact_token_limit"].as_integer(),
            Some(235_930)
        );
    }

    #[test]
    fn custom_profile_removes_context_settings_when_window_is_cleared() {
        let mut profile = custom_profile_for_context_tests();
        // 用户曾在 UI 里填过两项设置，随后清空上下文长度（configured 标志保留）。
        profile.model_context_window = None;
        profile.model_context_window_configured = true;
        profile.model_auto_compact_ratio = None;
        profile.model_auto_compact_ratio_configured = true;
        let profile = normalize_profile(profile).expect("valid profile");
        assert_eq!(profile.model_auto_compact_ratio, None);
        assert!(!profile.model_auto_compact_ratio_configured);

        let existing =
            "model_context_window = 262144\nmodel_auto_compact_token_limit = 235930\n";
        let rendered = build_profile_toml(Some(existing), None, &profile).expect("render");
        let document = DocumentMut::from_str(&rendered).expect("parse");
        assert!(document.get("model_context_window").is_none());
        assert!(document.get("model_auto_compact_token_limit").is_none());
    }

    #[test]
    fn model_auto_compact_ratio_must_be_between_zero_and_one() {
        for ratio in [-0.01, 1.01, f64::NAN] {
            let mut profile = official_profile();
            profile.model_context_window = Some(1_000_000);
            profile.model_auto_compact_ratio = Some(ratio);
            assert!(normalize_profile(profile).is_err());
        }
    }

    #[test]
    fn model_auto_compact_ratio_reverse_conversion_is_stable() {
        assert_eq!(
            model_auto_compact_ratio_from_token_limit(840_000, 1_050_000),
            Some(0.8)
        );
    }

    #[test]
    fn model_context_window_loader_accepts_positive_integers_only() {
        assert_eq!(
            model_context_window_from_raw("model_context_window = 1000000\n", "config.toml ")
                .expect("load context window"),
            Some(1_000_000)
        );
        assert_eq!(
            model_context_window_from_raw("model = \"gpt-5.6\"\n", "config.toml ")
                .expect("load absent context window"),
            None
        );
        assert!(model_context_window_from_raw("model_context_window = 0\n", "config.toml ")
            .is_err());
        assert!(model_context_window_from_raw(
            "model_context_window = \"1000000\"\n",
            "config.toml "
        )
        .is_err());
    }

    #[test]
    fn model_auto_compact_token_limit_loader_accepts_non_negative_integers_only() {
        assert_eq!(
            model_auto_compact_token_limit_from_raw(
                "model_auto_compact_token_limit = 800000\n",
                "config.toml "
            )
            .expect("load token limit"),
            Some(800_000)
        );
        assert_eq!(
            model_auto_compact_token_limit_from_raw(
                "model_auto_compact_token_limit = 0\n",
                "config.toml "
            )
            .expect("load zero token limit"),
            Some(0)
        );
        assert!(model_auto_compact_token_limit_from_raw(
            "model_auto_compact_token_limit = -1\n",
            "config.toml "
        )
        .is_err());
        assert!(model_auto_compact_token_limit_from_raw(
            "model_auto_compact_token_limit = 800000.0\n",
            "config.toml "
        )
        .is_err());
    }

    #[test]
    fn model_catalog_renders_context_and_official_field_names() {
        let profile = normalize_profile(custom_profile_with_catalog()).expect("valid profile");
        let bytes = render_model_catalog(&profile).expect("render models.json");
        let document: Value = serde_json::from_slice(&bytes).expect("parse models.json");
        let model = &document["models"][0];
        assert_eq!(model["slug"].as_str(), Some("deepseek-v4-flash"));
        assert_eq!(model["input_modalities"], serde_json::json!(["text"]));
        assert_eq!(model["supports_image_detail_original"].as_bool(), Some(false));
        assert_eq!(model["context_window"].as_u64(), Some(1_048_576));
        assert_eq!(model["max_context_window"].as_u64(), Some(1_048_576));
        assert_eq!(
            model["effective_context_window_percent"].as_u64(),
            Some(95)
        );
        assert_eq!(
            model["supported_reasoning_levels"][0]["effort"].as_str(),
            Some("low")
        );
        assert_eq!(model["prefer_websockets"].as_bool(), Some(false));
        assert_eq!(model["minimal_client_version"].as_str(), Some("0.144.0"));
        assert!(model["model_messages"].is_object());
        assert!(model["base_instructions"].is_string());
        assert!(model.get("supported_reasoning_efforts").is_none());

        let rendered = build_profile_toml(None, None, &profile).expect("render profile TOML");
        let toml = DocumentMut::from_str(&rendered).expect("parse profile TOML");
        assert_eq!(
            toml["model_catalog_json"].as_str(),
            Some(
                managed_model_catalog_path(&profile.id)
                    .expect("catalog path")
                    .to_string_lossy()
                    .as_ref()
            )
        );
    }

    #[test]
    fn model_catalog_supports_multiple_models_and_keeps_the_selected_default() {
        let mut profile = custom_profile_with_catalog();
        let mut second = profile
            .model_catalog
            .as_ref()
            .expect("catalog")
            .models[0]
            .clone();
        second.slug = "deepseek-v4-pro".to_string();
        second.display_name = "DeepSeek-V4-Pro".to_string();
        profile
            .model_catalog
            .as_mut()
            .expect("catalog")
            .models
            .push(second);
        profile.model = "deepseek-v4-pro".to_string();

        let profile = normalize_profile(profile).expect("valid multi-model profile");
        assert_eq!(profile.model, "deepseek-v4-pro");
        let bytes = render_model_catalog(&profile).expect("render multi-model catalog");
        let document: Value = serde_json::from_slice(&bytes).expect("parse models.json");
        assert_eq!(document["models"].as_array().map(Vec::len), Some(2));
        assert_eq!(document["models"][1]["slug"].as_str(), Some("deepseek-v4-pro"));
        assert_eq!(document["models"][1]["priority"].as_u64(), Some(2));
        assert_eq!(document["models"][1]["context_window"].as_u64(), Some(1_048_576));
    }

    /// 拖动排序必须落到 `priority` 上，而不只是数组顺序。
    ///
    /// Codex 按 `priority` 排序：`codex debug models` 只是照抄数组，而选择器读的
    /// app-server `model/list` 按 priority 升序返回，并把首位标成 `isDefault`
    /// （Codex 0.154.0 实测：数组 [p9, p1, p5] → 返回 p1, p5, p9）。所以这里故意
    /// 把 priority 写成与数组位置相反的值，断言落盘时按数组位置重排成 1..n——
    /// 只改数组顺序的话，Codex 内外看到的两份顺序仍然是旧的那一份。
    #[test]
    fn model_catalog_projects_model_order_onto_priority() {
        let mut profile = custom_profile_with_catalog();
        let base = profile
            .model_catalog
            .as_ref()
            .expect("catalog")
            .models[0]
            .clone();
        let ordered_model = |slug: &str, priority: u64| {
            let mut model = base.clone();
            model.slug = slug.to_string();
            model.display_name = slug.to_string();
            model
                .extra
                .insert("priority".to_string(), Value::Number(priority.into()));
            model
        };
        profile.model_catalog.as_mut().expect("catalog").models = vec![
            ordered_model("ordered-first", 9),
            ordered_model("ordered-second", 1),
            ordered_model("ordered-third", 5),
        ];
        // 默认模型由「默认」单选决定，与它在列表中的位置无关：这里选中间那个。
        profile.model = "ordered-second".to_string();

        let profile = normalize_profile(profile).expect("valid reordered profile");
        assert_eq!(profile.model, "ordered-second");
        let bytes = render_model_catalog(&profile).expect("render reordered models.json");
        let document: Value = serde_json::from_slice(&bytes).expect("parse models.json");
        let rendered: Vec<(String, u64)> = document["models"]
            .as_array()
            .expect("models array")
            .iter()
            .map(|model| {
                (
                    model["slug"].as_str().unwrap_or_default().to_string(),
                    model["priority"].as_u64().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            rendered,
            vec![
                ("ordered-first".to_string(), 1),
                ("ordered-second".to_string(), 2),
                ("ordered-third".to_string(), 3),
            ],
            "models.json 的 priority 必须按数组位置重排，Codex 才按这个顺序展示"
        );
    }

    #[test]
    fn model_catalog_normalizes_modalities_with_text_as_the_default() {
        let mut profile = custom_profile_with_catalog();
        profile
            .model_catalog
            .as_mut()
            .expect("catalog")
            .models[0]
            .input_modalities = vec!["IMAGE".to_string(), "image".to_string()];

        let normalized = normalize_profile(profile).expect("valid modality configuration");
        assert_eq!(
            normalized.model_catalog.expect("catalog").models[0].input_modalities,
            vec!["text".to_string(), "image".to_string()]
        );
    }

    #[test]
    fn model_catalog_renders_image_input_capability() {
        let mut profile = custom_profile_with_catalog();
        let model = &mut profile
            .model_catalog
            .as_mut()
            .expect("catalog")
            .models[0];
        model.input_modalities = vec!["text".to_string(), "image".to_string()];
        model.supports_image_detail_original = true;

        let normalized = normalize_profile(profile).expect("valid multimodal profile");
        let bytes = render_model_catalog(&normalized).expect("render multimodal catalog");
        let document: Value = serde_json::from_slice(&bytes).expect("parse models.json");
        assert_eq!(
            document["models"][0]["input_modalities"],
            serde_json::json!(["text", "image"])
        );
        assert_eq!(
            document["models"][0]["supports_image_detail_original"].as_bool(),
            Some(true)
        );
    }

    #[test]
    fn model_catalog_validation_rejects_invalid_context_ranges() {
        let mut profile = custom_profile_with_catalog();
        let catalog = profile.model_catalog.as_mut().expect("catalog");
        catalog.models[0].context_window = 0;
        assert!(normalize_profile(profile)
            .expect_err("zero context must fail")
            .contains("context_window"));

        let mut profile = custom_profile_with_catalog();
        let catalog = profile.model_catalog.as_mut().expect("catalog");
        catalog.models[0].max_context_window = 1;
        let normalized = normalize_profile(profile).expect("max context is derived from context");
        let model_catalog = normalized.model_catalog.expect("catalog");
        let model = &model_catalog.models[0];
        assert_eq!(
            model.max_context_window,
            model.context_window
        );
    }

    #[test]
    fn model_catalog_template_truncation_limit_fits_a_smaller_context_window() {
        let mut profile = custom_profile_with_catalog();
        let catalog = profile.model_catalog.as_mut().expect("catalog");
        catalog.models[0].context_window = 4096;
        catalog.models[0].max_context_window = 4096;
        let normalized = normalize_profile(profile).expect("smaller context is valid");
        assert_eq!(
            normalized.model_catalog.expect("catalog").models[0]
                .truncation_policy.as_ref().expect("policy").limit,
            3891
        );
    }

    #[test]
    fn official_profile_can_retain_third_party_catalog_without_using_its_model() {
        let mut profile = custom_profile_with_catalog();
        profile.auth_mode = CodexAuthMode::Official;
        profile.model = "gpt-5.6".to_string();
        let normalized = normalize_profile(profile).expect("official profile remains valid");
        assert_eq!(normalized.model, "gpt-5.6");
        assert!(normalized.model_catalog.is_some());
    }

    #[test]
    fn official_restore_can_restore_or_remove_model_catalog_json() {
        let existing = "model_catalog_json = \"C:/user/models.json\"\n";
        let profile = official_profile();
        let previous = Some(Some("C:/user/models.json".to_string()));
        let restored = build_profile_toml_with_model_catalog_restore(
            Some(existing),
            None,
            &profile,
            previous.as_ref(),
        )
        .expect("restore previous path");
        assert_eq!(
            DocumentMut::from_str(&restored).expect("parse restored") ["model_catalog_json"]
                .as_str(),
            Some("C:/user/models.json")
        );

        let absent = Some(None);
        let removed = build_profile_toml_with_model_catalog_restore(
            Some(existing),
            None,
            &profile,
            absent.as_ref(),
        )
        .expect("remove project path");
        assert!(DocumentMut::from_str(&removed)
            .expect("parse removed")
            .get("model_catalog_json")
            .is_none());
    }

    #[test]
    fn global_official_restore_preserves_the_preexisting_catalog_reference() {
        let existing = "model_provider = \"deepseek\"\nmodel_catalog_json = \"C:/user/models.json\"\n";
        let profile = official_profile();
        let previous = Some(Some("C:/user/models.json".to_string()));
        let restored = build_global_toml_with_model_catalog_restore(
            Some(existing),
            Some("deepseek"),
            &profile,
            previous.as_ref(),
        )
        .expect("restore global catalog reference");
        let document = DocumentMut::from_str(&restored).expect("parse global TOML");
        assert_eq!(
            document["model_catalog_json"].as_str(),
            Some("C:/user/models.json")
        );
        assert_eq!(document["model_provider"].as_str(), Some("openai"));
    }

    #[test]
    fn raw_model_catalog_snapshot_restores_bytes_or_removes_file() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-model-catalog-{}",
            Uuid::new_v4()
        ));
        let path = directory.join("models.json");
        let original = br#"{ "models": [{ "slug": "user-model" }] }"#;
        fs::create_dir_all(&directory).expect("create temp directory");
        fs::write(&path, br#"{ "models": [{ "slug": "managed-model" }] }"#)
            .expect("write managed file");
        restore_snapshot(&path, Some(original), SnapshotKind::Raw).expect("restore original");
        assert_eq!(fs::read(&path).expect("read restored"), original);
        restore_snapshot(&path, None, SnapshotKind::Raw).expect("remove projection");
        assert!(!path.exists());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn model_catalog_projection_is_removed_when_original_file_was_missing() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-model-catalog-missing-{}",
            Uuid::new_v4()
        ));
        let profile_id = "profile-missing";
        let mut profile = custom_profile_with_catalog();
        profile.id = profile_id.to_string();
        let content = render_model_catalog(&normalize_profile(profile).expect("valid profile"))
            .expect("render projection");
        let path = managed_model_catalog_path_at(&directory, profile_id);
        let desired = BTreeMap::from([(profile_id.to_string(), content.clone())]);
        let mut state = CodexProfileState::default();

        assert!(!path.exists());
        synchronize_model_catalogs_at(&mut state, &desired, &directory)
            .expect("create projection");
        assert_eq!(fs::read(&path).expect("read projection"), content);
        assert_eq!(
            state
                .managed_model_catalogs
                .get(profile_id)
                .and_then(|managed| managed.previous_bytes.as_deref()),
            None
        );

        synchronize_model_catalogs_at(&mut state, &BTreeMap::new(), &directory)
            .expect("remove projection");
        assert!(!path.exists());
        assert!(state.managed_model_catalogs.is_empty());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn model_catalog_projection_restores_original_bytes_after_official_switch() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-model-catalog-existing-{}",
            Uuid::new_v4()
        ));
        let profile_id = "profile-existing";
        let path = managed_model_catalog_path_at(&directory, profile_id);
        let original = br#"{\n  "models": [{"slug": "user-model"}],\n  "user_field": "preserve-me"\n}\n"#;
        fs::create_dir_all(&directory).expect("create catalog directory");
        fs::write(&path, original).expect("write original catalog");

        let mut profile = custom_profile_with_catalog();
        profile.id = profile_id.to_string();
        let content = render_model_catalog(&normalize_profile(profile).expect("valid profile"))
            .expect("render projection");
        let desired = BTreeMap::from([(profile_id.to_string(), content.clone())]);
        let mut state = CodexProfileState::default();

        synchronize_model_catalogs_at(&mut state, &desired, &directory)
            .expect("replace original catalog");
        assert_eq!(fs::read(&path).expect("read projection"), content);
        assert_eq!(
            state
                .managed_model_catalogs
                .get(profile_id)
                .and_then(|managed| managed.previous_bytes.as_deref()),
            Some(original.as_slice())
        );

        synchronize_model_catalogs_at(&mut state, &BTreeMap::new(), &directory)
            .expect("restore original catalog");
        assert_eq!(fs::read(&path).expect("read restored catalog"), original);
        assert!(state.managed_model_catalogs.is_empty());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn legacy_cc_launcher_profile_is_migrated_to_agents_launcher_name() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-profile-migration-{}",
            Uuid::new_v4()
        ));
        let profile_id = "profile-legacy";
        let legacy = legacy_managed_profile_path_at(&directory, profile_id);
        let target = directory.join(format!("{}.config.toml", managed_profile_name(profile_id)));
        let original = b"model = \"legacy-model\"\n[features]\njs_repl = true\n";
        fs::create_dir_all(&directory).expect("create profile directory");
        fs::write(&legacy, original).expect("write legacy profile");

        migrate_legacy_managed_profile_at(profile_id, &target, &directory)
            .expect("migrate legacy profile");
        assert_eq!(fs::read(&target).expect("read migrated profile"), original);
        assert!(!legacy.exists());
        assert!(DocumentMut::from_str(
            &String::from_utf8(fs::read(&target).expect("read migrated TOML"))
                .expect("UTF-8 migrated TOML")
        )
        .is_ok());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn desktop_project_state_migration_excludes_thread_and_prompt_state() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-project-state-isolation-{}",
            Uuid::new_v4()
        ));
        let shared = directory.join("shared");
        let isolated = directory.join("isolated");
        fs::create_dir_all(&shared).expect("create shared home");
        fs::create_dir_all(&isolated).expect("create isolated home");

        let source = serde_json::json!({
            "electron-saved-workspace-roots": ["D:/legacy"],
            "project-order": ["legacy-project"],
            "active-workspace-roots": ["D:/legacy"],
            "local-projects": {
                "legacy-project": {
                    "id": "legacy-project",
                    "name": "Legacy",
                    "rootPaths": ["D:/legacy"]
                }
            },
            "selected-project": {"type": "local", "id": "legacy-project"},
            "electron-workspace-root-labels": {"D:/legacy": "Legacy root"},
            "thread-project-assignments": {
                "legacy-thread": {"projectId": "legacy-project"}
            },
            "electron-persisted-atom-state": {
                "prompt-history": ["must not migrate"],
                "thread-client-id-v1:legacy-thread": "must-not-migrate"
            }
        });
        fs::write(
            isolated.join(CODEX_DESKTOP_STATE_FILE),
            serde_json::to_vec(&source).expect("serialize isolated state"),
        )
        .expect("write isolated state");

        let current = serde_json::json!({
            "electron-saved-workspace-roots": ["D:/current"],
            "project-order": ["current-project"],
            "local-projects": {
                "current-project": {
                    "id": "current-project",
                    "name": "Current",
                    "rootPaths": ["D:/current"]
                }
            },
            "unrelated-current-setting": true
        });
        let target_path = shared.join(CODEX_DESKTOP_STATE_FILE);
        fs::write(
            &target_path,
            serde_json::to_vec(&current).expect("serialize current state"),
        )
        .expect("write current state");

        merge_legacy_desktop_project_state(&isolated, &shared).expect("migrate project catalog");

        let migrated: Value = serde_json::from_slice(
            &fs::read(&target_path).expect("read migrated project state"),
        )
        .expect("parse migrated project state");
        assert_eq!(
            migrated["electron-saved-workspace-roots"],
            serde_json::json!(["D:/current", "D:/legacy"])
        );
        assert_eq!(
            migrated["project-order"],
            serde_json::json!(["current-project", "legacy-project"])
        );
        assert!(migrated["local-projects"]["current-project"].is_object());
        assert!(migrated["local-projects"]["legacy-project"].is_object());
        assert_eq!(
            migrated["selected-project"]["id"],
            Value::String("legacy-project".to_string())
        );
        assert_eq!(migrated["unrelated-current-setting"], Value::Bool(true));
        assert!(migrated.get("thread-project-assignments").is_none());
        assert!(migrated.get("electron-persisted-atom-state").is_none());
        let _ = fs::remove_dir_all(directory);
    }

    /// 写一个能被 `rollout_identity` 认出来的会话文件。
    fn write_rollout(path: &std::path::Path, id: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create rollout dir");
        }
        fs::write(
            path,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"model_provider\":\"company\"}}}}
"
            ),
        )
        .expect("write rollout");
    }

    /// 遗留 home 的会话迁移只做一次。
    ///
    /// 它挂在 `prepare_profiles_for_load` 上，也就是每次加载/保存/应用都会跑；
    /// 没有完成标记时，它每次都把两个目录树重走一遍（三个 2000 文件的遗留 home
    /// 实测 1.1 秒，全是在做已经做过的事）。标记落地之后必须不再读遗留 home——
    /// 这里往已迁完的遗留 home 里再塞一个会话，如果实现还在走目录树，它就会被
    /// 搬进共享端。
    #[test]
    fn legacy_home_migration_runs_once_and_then_leaves_the_legacy_home_alone() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-rollout-marker-{}",
            Uuid::new_v4()
        ));
        let isolated = directory.join("isolated");
        let shared = directory.join("shared");
        let marker = directory.join("migrations").join("profile-test.rollouts.json");
        let backups = directory.join("backups").join("profile-test");
        write_rollout(&isolated.join("sessions/2026/08/15/legacy-a.jsonl"), "legacy-a");
        write_rollout(&shared.join("sessions/2026/08/15/shared-b.jsonl"), "shared-b");

        let mut shared_thread_ids = None;
        migrate_legacy_home_rollouts(
            "profile-test",
            &isolated,
            &shared,
            &backups,
            &mut shared_thread_ids,
            &marker,
        )
        .expect("first pass migrates");

        assert!(shared.join("sessions/2026/08/15/legacy-a.jsonl").exists());
        assert!(marker.exists(), "迁完必须落下完成标记");
        assert_eq!(
            shared_thread_ids,
            Some(HashSet::from(["shared-b".to_string(), "legacy-a".to_string()])),
            "共享端线程集合按需算一次，并把本趟迁进来的线程并回去"
        );

        // 第二个会话是在迁移「完成」之后才出现的：有了标记就不该再被搬走。
        write_rollout(&isolated.join("sessions/2026/08/15/legacy-c.jsonl"), "legacy-c");
        migrate_legacy_home_rollouts(
            "profile-test",
            &isolated,
            &shared,
            &backups,
            &mut shared_thread_ids,
            &marker,
        )
        .expect("second pass is a no-op");

        assert!(
            !shared.join("sessions/2026/08/15/legacy-c.jsonl").exists(),
            "标记已存在时不应再遍历遗留 home"
        );
    }

    /// 两个 profile 各持同一线程的遗留副本时，第二个不该把它重新灌回共享端。
    ///
    /// 共享端已有该线程（任意文件）而目标路径缺失时 `migrate_rollout_tree` 会跳过，
    /// 依据是「共享端已存在的线程集合」。这份集合现在按需缓存一次、并把本趟迁进来的
    /// 线程并回去——如果只缓存不合并，第二个 profile 会拿着过时的快照把文件灌回来。
    #[test]
    fn cached_shared_thread_ids_keep_the_second_profile_from_reinjecting() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-rollout-cache-{}",
            Uuid::new_v4()
        ));
        let shared = directory.join("shared");
        write_rollout(&shared.join("sessions/2026/08/15/seed.jsonl"), "seed");
        let backups = directory.join("backups");

        // profile-a 的遗留副本被迁进共享端，线程 id 变成「共享端已存在」。
        let isolated_a = directory.join("isolated-a");
        write_rollout(&isolated_a.join("sessions/2026/08/15/moved.jsonl"), "moved");
        let mut shared_thread_ids = None;
        migrate_legacy_home_rollouts(
            "profile-a",
            &isolated_a,
            &shared,
            &backups.join("profile-a"),
            &mut shared_thread_ids,
            &directory.join("migrations").join("profile-a.rollouts.json"),
        )
        .expect("profile-a migrates");
        assert!(shared.join("sessions/2026/08/15/moved.jsonl").exists());

        // profile-b 也留着同一线程的副本，但在另一个内部路径下：目标路径不存在，
        // 而线程已在共享端——按规则不回灌。
        let isolated_b = directory.join("isolated-b");
        write_rollout(&isolated_b.join("sessions/2026/08/16/moved.jsonl"), "moved");
        migrate_legacy_home_rollouts(
            "profile-b",
            &isolated_b,
            &shared,
            &backups.join("profile-b"),
            &mut shared_thread_ids,
            &directory.join("migrations").join("profile-b.rollouts.json"),
        )
        .expect("profile-b migrates");

        assert!(
            !shared.join("sessions/2026/08/16/moved.jsonl").exists(),
            "共享端已有的线程不得从另一个 profile 的遗留 home 回灌"
        );
    }

    #[test]
    fn shared_home_migration_copies_all_provider_sessions_and_indexes() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-session-isolation-{}",
            Uuid::new_v4()
        ));
        let source = directory.join("isolated");
        let target = directory.join("shared");
        let source_sessions = source.join("sessions").join("2026").join("08").join("15");
        fs::create_dir_all(&source_sessions).expect("create shared sessions");
        fs::write(
            source_sessions.join("custom.jsonl"),
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"custom-id\",\"model_provider\":\"company\"}}\n",
                "{\"type\":\"event_msg\",\"payload\":{}}\n"
            ),
        )
        .expect("write custom rollout");
        fs::write(
            source_sessions.join("official.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"official-id\",\"model_provider\":\"openai\"}}\n",
        )
        .expect("write official rollout");
        fs::write(
            source.join("session_index.jsonl"),
            concat!(
                "{\"id\":\"custom-id\",\"thread_name\":\"custom\"}\n",
                "{\"id\":\"official-id\",\"thread_name\":\"official\"}\n"
            ),
        )
        .expect("write shared index");

        let mut ids = HashSet::new();
        migrate_rollout_tree(
            &source.join("sessions"),
            &target.join("sessions"),
            "profile-test",
            &directory.join("backups/profile-test"),
            "sessions",
            &mut ids,
            &HashSet::new(),
        )
        .expect("migrate all sessions");
        merge_jsonl(
            &source.join("session_index.jsonl"),
            &target.join("session_index.jsonl"),
            "test shared index",
        )
        .expect("migrate shared index");

        assert_eq!(
            ids,
            HashSet::from(["custom-id".to_string(), "official-id".to_string()])
        );
        assert!(target
            .join("sessions/2026/08/15/custom.jsonl")
            .exists());
        assert!(target
            .join("sessions/2026/08/15/official.jsonl")
            .exists());
        let index = fs::read_to_string(target.join("session_index.jsonl"))
            .expect("read isolated index");
        assert!(index.contains("custom-id"));
        assert!(index.contains("official-id"));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn divergent_rollout_migration_preserves_both_versions() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-session-conflict-{}",
            Uuid::new_v4()
        ));
        let source = directory.join("isolated/sessions/2026/08/15/thread.jsonl");
        let target = directory.join("shared/sessions/2026/08/15/thread.jsonl");
        let backup_root = directory.join("backups/profile-test");
        fs::create_dir_all(source.parent().expect("source parent")).expect("source parent");
        fs::create_dir_all(target.parent().expect("target parent")).expect("target parent");
        let target_bytes = concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-id\",\"model_provider\":\"openai\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"message\":\"shared\"}}\n"
        )
        .as_bytes();
        let source_bytes = concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-id\",\"model_provider\":\"openai\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"message\":\"isolated\"}}\n"
        )
        .as_bytes();
        fs::write(&target, target_bytes).expect("write shared version");
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&source, source_bytes).expect("write isolated version");

        merge_rollout_file(
            &source,
            &target,
            "profile-test",
            &backup_root,
            "sessions",
            Path::new("2026/08/15/thread.jsonl"),
        )
        .expect("merge divergent rollout");

        let canonical = fs::read(&target).expect("read canonical rollout");
        // 共享 home 是权威：分叉时共享侧保持不变，隔离侧进入备份。
        assert_eq!(canonical, target_bytes);
        let backup_path = migration_backup_path(
            &backup_root,
            "sessions",
            Path::new("2026/08/15/thread.jsonl"),
            "isolated.jsonl",
        )
        .expect("backup path");
        let backup = fs::read(backup_path).expect("read conflict backup");
        assert_eq!(backup, source_bytes);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn repaired_thread_files_are_not_reimported_from_legacy_home() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-session-reimport-{}",
            Uuid::new_v4()
        ));
        let thread_id = "01a03324-bad1-7c81-a458-4ca57d6b87ea";
        let source_sessions = directory.join("isolated/sessions/2026/08/24");
        let target_sessions = directory.join("shared/sessions/2026/08/24");
        fs::create_dir_all(&source_sessions).expect("create isolated sessions");
        fs::create_dir_all(&target_sessions).expect("create shared sessions");

        // 隔离 home 仍保存断链的两页原件。
        let legacy_parent = source_sessions.join(format!("rollout-2026-08-24T17-40-40-{thread_id}.jsonl"));
        let legacy_page = source_sessions.join(format!(
            "rollout-2026-08-24T20-14-25-{thread_id}_01a033b1-7d0c-7102-bb6c-748e4ccfbf10.jsonl"
        ));
        fs::write(
            &legacy_parent,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"01a03324-bad1-7c81-a458-4ca57d6b87ea\",\"model_provider\":\"openai\"}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"message\":\"legacy-tail\"}}\n"
            ),
        )
        .expect("write legacy parent");
        fs::write(
            &legacy_page,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"01a03324-bad1-7c81-a458-4ca57d6b87ea\",\"model_provider\":\"openai\"}}\n",
        )
        .expect("write legacy page");

        // 共享端是修复后的合并文件（与隔离父页分叉，且分页文件已被移除）。
        let shared_consolidated = target_sessions.join(format!("rollout-2026-08-24T17-40-40-{thread_id}.jsonl"));
        let consolidated_bytes = concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"01a03324-bad1-7c81-a458-4ca57d6b87ea\",\"model_provider\":\"openai\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"message\":\"consolidated\"}}\n"
        )
        .as_bytes();
        fs::write(&shared_consolidated, consolidated_bytes).expect("write consolidated");

        let mut ids = HashSet::new();
        let known = HashSet::from([thread_id.to_string()]);
        migrate_rollout_tree(
            &directory.join("isolated/sessions"),
            &directory.join("shared/sessions"),
            "profile-test",
            &directory.join("backups/profile-test"),
            "sessions",
            &mut ids,
            &known,
        )
        .expect("migrate with known threads");

        // 分页文件不得回灌；合并文件不得被隔离父页覆盖；隔离分叉版进备份。
        assert!(!target_sessions
            .join(format!(
                "rollout-2026-08-24T20-14-25-{thread_id}_01a033b1-7d0c-7102-bb6c-748e4ccfbf10.jsonl"
            ))
            .exists());
        assert_eq!(
            fs::read(&shared_consolidated).expect("read shared"),
            consolidated_bytes
        );
        let backup_path = migration_backup_path(
            &directory.join("backups/profile-test"),
            "sessions",
            Path::new(&format!("2026/08/24/rollout-2026-08-24T17-40-40-{thread_id}.jsonl")),
            "isolated.jsonl",
        )
        .expect("backup path");
        assert!(backup_path.exists());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn rendered_profile_removes_launcher_managed_sqlite_isolation() {
        let profile = official_profile();
        let existing = format!(
            "sqlite_home = {:?}\n",
            legacy_profile_home(&profile.id)
                .expect("legacy home")
                .to_string_lossy()
                .to_string()
        );
        let rendered = build_profile_toml(Some(&existing), None, &profile).expect("render profile");
        let document = DocumentMut::from_str(&rendered).expect("parse profile");
        assert!(document.get("sqlite_home").is_none());
    }

    #[test]
    fn custom_profile_uses_env_key_without_serializing_the_secret() {
        let mut profile = official_profile();
        profile.auth_mode = CodexAuthMode::Custom;
        profile.name = "Company Proxy".to_string();
        profile.base_url = "https://proxy.example.com/v1".to_string();
        profile.env_key = "COMPANY_CODEX_KEY".to_string();
        let profile = normalize_profile(profile).expect("valid profile");
        let rendered = build_profile_toml(None, None, &profile).expect("render");
        assert!(rendered.contains("env_key = \"COMPANY_CODEX_KEY\""));
        assert!(!rendered.contains("sk-test-secret"));
        assert_eq!(
            DocumentMut::from_str(&rendered).expect("parse")["model_providers"]["Company_Proxy"]
                ["wire_api"]
                .as_str(),
            Some("responses")
        );
        assert_eq!(profile.provider_id, "Company_Proxy");
        assert_eq!(profile.provider_name, "Company Proxy");
    }

    #[test]
    fn global_sync_keeps_the_previous_managed_provider_for_history() {
        let existing = concat!(
            "approval_policy = \"on-request\"\n",
            "model_provider = \"old_proxy\"\n",
            "[features]\n",
            "js_repl = true\n",
            "[model_providers.old_proxy]\n",
            "name = \"Old Proxy\"\n",
            "base_url = \"https://old.example.com/v1\"\n",
        );
        let mut profile = official_profile();
        profile.auth_mode = CodexAuthMode::Custom;
        profile.name = "New Proxy".to_string();
        profile.base_url = "https://new.example.com/v1".to_string();
        let profile = normalize_profile(profile).expect("valid profile");

        let rendered = build_global_toml(Some(existing), Some("old_proxy"), &profile)
            .expect("render global config");
        let document = DocumentMut::from_str(&rendered).expect("parse");
        assert_eq!(document["approval_policy"].as_str(), Some("on-request"));
        assert_eq!(document["features"]["js_repl"].as_bool(), Some(true));
        assert_eq!(
            document["model_providers"]["old_proxy"]["base_url"].as_str(),
            Some("https://old.example.com/v1")
        );
        assert_eq!(
            document["model_providers"]["New_Proxy"]["base_url"].as_str(),
            Some("https://new.example.com/v1")
        );
    }

    #[test]
    fn shared_provider_registry_keeps_every_profile_and_historical_provider() {
        let mut first = official_profile();
        first.id = "profile-first".to_string();
        first.name = "First Proxy".to_string();
        first.auth_mode = CodexAuthMode::Custom;
        first.base_url = "https://first.example.com/v1".to_string();
        let first = normalize_profile(first).expect("first profile");

        let mut second = official_profile();
        second.id = "profile-second".to_string();
        second.name = "Second Proxy".to_string();
        second.auth_mode = CodexAuthMode::Custom;
        second.base_url = "https://second.example.com/v1".to_string();
        second.protocol_conversion = true;
        let second = normalize_profile(second).expect("second profile");

        let mut state = CodexProfileState::default();
        state.profiles = vec![first.clone(), second.clone()];
        let raw = concat!(
            "model_provider = \"openai\"\n",
            "[model_providers.historical]\n",
            "name = \"Historical\"\n",
            "base_url = \"https://historical.example.com/v1\"\n",
        );
        let rendered = upsert_managed_provider_items(raw, &state, Some(&second.id))
            .expect("upsert registry")
            .expect("registry changes");
        let document = DocumentMut::from_str(&rendered).expect("parse registry");

        assert_eq!(
            document["model_providers"][first.provider_id.as_str()]["base_url"].as_str(),
            Some("https://first.example.com/v1")
        );
        assert_eq!(
            document["model_providers"][second.provider_id.as_str()]["base_url"].as_str(),
            Some(global_conversion_proxy_url(&second.id).as_str())
        );
        assert_eq!(
            document["model_providers"]["historical"]["base_url"].as_str(),
            Some("https://historical.example.com/v1")
        );
    }

    #[test]
    fn provider_identity_is_derived_from_profile_name() {
        assert_eq!(provider_id_from_profile_name("Kimi For Coding", "fallback"), "Kimi_For_Coding");
        assert_eq!(provider_id_from_profile_name("中文配置", "fallback"), "fallback");
        assert_eq!(provider_id_from_profile_name("OpenAI", "fallback"), "OpenAI_custom");

        let mut profile = official_profile();
        profile.auth_mode = CodexAuthMode::Custom;
        profile.name = "中文 配置".to_string();
        profile.provider_id = "user_supplied".to_string();
        profile.provider_name = "User supplied".to_string();
        profile.base_url = "https://proxy.example.com/v1".to_string();
        let normalized = normalize_profile(profile).expect("valid profile");
        assert_eq!(normalized.provider_id, "profile-test");
        assert_eq!(normalized.provider_name, "中文 配置");
    }

    #[test]
    fn applied_profile_cannot_be_renamed_but_unused_profile_can() {
        let previous = official_profile();
        let mut renamed = previous.clone();
        renamed.name = "Renamed".to_string();

        assert_eq!(
            profile_rename_block_reason(
                Some(&previous),
                &renamed,
                Some(previous.id.as_str()),
                Some(previous.id.as_str()),
            ),
            Some("启动器当前应用和 Codex 全局配置")
        );
        assert_eq!(
            profile_rename_block_reason(
                Some(&previous),
                &renamed,
                Some(previous.id.as_str()),
                None,
            ),
            Some("启动器当前应用")
        );
        assert_eq!(
            profile_rename_block_reason(
                Some(&previous),
                &renamed,
                None,
                Some(previous.id.as_str()),
            ),
            Some("Codex 全局配置")
        );
        assert_eq!(
            profile_rename_block_reason(Some(&previous), &renamed, None, None),
            None
        );
        assert_eq!(
            profile_rename_block_reason(
                Some(&previous),
                &previous,
                Some(previous.id.as_str()),
                None,
            ),
            None
        );
    }

    #[test]
    fn global_sync_status_ignores_official_model_fields_but_detects_other_changes() {
        let profile = official_profile();
        let rendered = build_global_toml(None, None, &profile).expect("render global config");
        let mut state = CodexProfileState::default();
        state.global_profile_id = Some(profile.id.clone());
        state.profiles.push(profile);

        assert!(global_profile_matches_document(&state, &rendered));

        let mut external = DocumentMut::from_str(&rendered).expect("parse global config");
        external["model"] = value("gpt-updated");
        external["model_reasoning_effort"] = value("xhigh");
        assert!(
            global_profile_matches_document(&state, &external.to_string()),
            "official model fields must not require a global re-sync",
        );

        external["model_context_window"] = value(123_456_i64);
        assert!(
            !global_profile_matches_document(&state, &external.to_string()),
            "official context window must require a global re-sync",
        );

        external["model_provider"] = value("unexpected-provider");
        assert!(
            !global_profile_matches_document(&state, &external.to_string()),
            "launcher-managed official config fields must still require a global re-sync",
        );
    }

    #[test]
    fn custom_global_sync_still_compares_model_fields() {
        let mut profile = official_profile();
        profile.auth_mode = CodexAuthMode::Custom;
        profile.provider_id = "custom".to_string();
        profile.provider_name = "Custom".to_string();
        profile.base_url = "https://proxy.example.com/v1".to_string();
        let rendered = build_global_toml(None, None, &profile).expect("render global config");
        let mut state = CodexProfileState::default();
        state.global_profile_id = Some(profile.id.clone());
        state.managed_global_provider_id = Some(profile.provider_id.clone());
        state.profiles.push(profile);

        let mut external = DocumentMut::from_str(&rendered).expect("parse global config");
        external["model"] = value("gpt-updated");
        assert!(
            !global_profile_matches_document(&state, &external.to_string()),
            "custom profile model fields remain launcher-managed",
        );
    }

    #[test]
    fn global_sync_detects_stale_managed_model_catalog_content() {
        let profile = custom_profile_with_catalog();
        let rendered = render_model_catalog(&profile).expect("render catalog");

        // 非第三方或没有模型目录的 profile 不参与目录文件比较。
        let mut no_catalog = profile.clone();
        no_catalog.model_catalog = None;
        assert!(global_model_catalog_content_in_sync(&no_catalog, None));

        // 文件内容与当前渲染结果一致视为已同步。
        assert!(global_model_catalog_content_in_sync(
            &profile,
            Some(rendered.as_slice()),
        ));

        // 保存更新上下文长度后，旧文件内容必须判定为未同步，
        // 否则 UI 会停留在“全局应用中”而无法直接重新同步。
        let stale = String::from_utf8(rendered.clone()).expect("utf8 catalog")
            .replace("1048576", "2097152")
            .into_bytes();
        assert_ne!(stale, rendered);
        assert!(!global_model_catalog_content_in_sync(&profile, Some(stale.as_slice())));

        // 托管模型目录文件缺失同样视为未同步。
        assert!(!global_model_catalog_content_in_sync(&profile, None));
    }

    #[test]
    fn global_sync_detects_stale_global_api_key_env() {
        // 有已存 Key：用户环境变量的实际值必须等于已存 Key。
        assert!(global_key_env_content_in_sync(
            true,
            Some("sk-test"),
            Some("sk-test"),
            None,
            "OPENAI_API_KEY",
        ));
        assert!(!global_key_env_content_in_sync(
            true,
            Some("sk-new"),
            Some("sk-old"),
            None,
            "OPENAI_API_KEY",
        ));
        assert!(!global_key_env_content_in_sync(
            true,
            None,
            Some("sk-old"),
            None,
            "OPENAI_API_KEY",
        ));
        assert!(!global_key_env_content_in_sync(
            true,
            Some("sk-old"),
            None,
            None,
            "OPENAI_API_KEY",
        ));

        // 无已存 Key：启动器托管的全局 Key 记录必须已被清理。
        let managed = ManagedGlobalEnv {
            key: "OPENAI_API_KEY".to_string(),
            applied_value: "sk-old".to_string(),
            previous_value: None,
        };
        let other = ManagedGlobalEnv {
            key: "OTHER_KEY".to_string(),
            applied_value: "sk-other".to_string(),
            previous_value: None,
        };
        assert!(global_key_env_content_in_sync(false, None, None, None, "OPENAI_API_KEY"));
        assert!(!global_key_env_content_in_sync(
            false,
            None,
            None,
            Some(&managed),
            "OPENAI_API_KEY",
        ));
        assert!(global_key_env_content_in_sync(
            false,
            None,
            None,
            Some(&other),
            "OPENAI_API_KEY",
        ));
    }

    #[test]
    fn global_sync_status_accepts_the_fixed_conversion_proxy_url() {
        let mut profile = official_profile();
        profile.auth_mode = CodexAuthMode::Custom;
        profile.protocol_conversion = true;
        profile.provider_id = "kimi".to_string();
        profile.provider_name = "Kimi".to_string();
        profile.base_url = "https://api.moonshot.cn/v1".to_string();
        let mut rendered_profile = profile.clone();
        rendered_profile.base_url = global_conversion_proxy_url(&profile.id);
        let rendered = build_global_toml(None, None, &rendered_profile).expect("render global config");

        let mut state = CodexProfileState::default();
        state.global_profile_id = Some(profile.id.clone());
        state.managed_global_provider_id = Some(profile.provider_id.clone());
        state.profiles.push(profile);

        assert!(global_profile_matches_document(&state, &rendered));
    }

    #[test]
    fn startup_global_sync_diagnostics_accept_proxy_or_real_base_url() {
        let mut profile = official_profile();
        profile.auth_mode = CodexAuthMode::Custom;
        profile.protocol_conversion = true;
        profile.provider_id = "kimi".to_string();
        profile.provider_name = "Kimi".to_string();
        profile.base_url = "https://api.moonshot.cn/v1".to_string();
        let mut state = CodexProfileState::default();
        state.global_profile_id = Some(profile.id.clone());
        state.managed_global_provider_id = Some(profile.provider_id.clone());
        state.profiles.push(profile.clone());

        let mut proxy_profile = profile.clone();
        proxy_profile.base_url = global_conversion_proxy_url(&profile.id);
        let proxy_raw = build_global_toml(None, None, &proxy_profile).expect("render proxy config");
        assert!(global_profile_is_recoverable_at_startup(&state, &proxy_raw));
        assert!(!global_sync_repair_required_for_raw(
            &state,
            true,
            Some(&proxy_raw),
        ));

        let real_raw = build_global_toml(None, None, &profile).expect("render real config");
        assert!(global_profile_is_recoverable_at_startup(&state, &real_raw));

        let mismatched = real_raw.replace("https://api.moonshot.cn/v1", "https://other.example/v1");
        assert!(!global_profile_is_recoverable_at_startup(&state, &mismatched));
        assert!(global_sync_repair_required_for_raw(
            &state,
            false,
            Some(&mismatched),
        ));
    }

    #[test]
    fn startup_global_sync_diagnostics_detect_orphaned_fixed_proxy() {
        let state = CodexProfileState::default();
        let raw = format!(
            "[model_providers.orphan]\nbase_url = \"{}\"\n",
            global_conversion_proxy_url("orphan")
        );
        assert!(global_config_contains_orphaned_conversion_proxy(&state, &raw));
        assert!(global_sync_repair_required_for_raw(&state, false, Some(&raw)));
    }

    #[test]
    fn shared_conversion_provider_is_not_orphaned_without_a_global_default() {
        let mut profile = official_profile();
        profile.auth_mode = CodexAuthMode::Custom;
        profile.protocol_conversion = true;
        profile.provider_id = "kimi".to_string();
        profile.provider_name = "Kimi".to_string();
        profile.base_url = "https://api.moonshot.cn/v1".to_string();
        let raw = format!(
            "[model_providers.kimi]\nbase_url = \"{}\"\n",
            global_conversion_proxy_url(&profile.id)
        );
        let mut state = CodexProfileState::default();
        state.profiles.push(profile);
        assert!(!global_config_contains_orphaned_conversion_proxy(&state, &raw));
        assert!(!global_sync_repair_required_for_raw(
            &state,
            false,
            Some(&raw)
        ));
    }

    #[test]
    fn official_global_sync_keeps_the_previous_managed_provider_for_history() {
        let existing = concat!(
            "model_provider = \"company_proxy\"\n",
            "[model_providers.company_proxy]\n",
            "name = \"Company Proxy\"\n",
            "base_url = \"https://proxy.example.com/v1\"\n",
        );
        let profile = official_profile();
        let rendered = build_global_toml(Some(existing), Some("company_proxy"), &profile)
            .expect("render official global config");
        let document = DocumentMut::from_str(&rendered).expect("parse");
        assert_eq!(document["model_provider"].as_str(), Some("openai"));
        assert_eq!(
            document["model_providers"]["company_proxy"]["base_url"].as_str(),
            Some("https://proxy.example.com/v1")
        );
    }

    #[test]
    fn missing_managed_provider_can_be_restored_without_overwriting_existing_config() {
        let mut target = DocumentMut::from_str(
            "model_provider = \"llama-cpp\"\n[model_providers.llama-cpp]\nbase_url = \"https://llama.example/v1\"\n",
        )
        .expect("parse target");
        let source = DocumentMut::from_str(
            "[model_providers.muskapis]\nname = \"muskapis\"\nbase_url = \"https://muskapis.example/v1\"\n",
        )
        .expect("parse source");

        assert!(merge_missing_provider_item(&mut target, &source, "muskapis").expect("merge"));
        assert!(!merge_missing_provider_item(&mut target, &source, "muskapis").expect("no-op"));
        assert_eq!(
            target["model_provider"].as_str(),
            Some("llama-cpp"),
            "restoring a historical provider must not change the active provider",
        );
        assert_eq!(
            target["model_providers"]["muskapis"]["base_url"].as_str(),
            Some("https://muskapis.example/v1")
        );
    }

    #[test]
    fn deleted_global_proxy_restores_only_the_fixed_proxy_url() {
        let mut profile = official_profile();
        profile.auth_mode = CodexAuthMode::Custom;
        profile.provider_id = "kimi".to_string();
        profile.base_url = "https://api.moonshot.cn/v1".to_string();

        let raw = format!(
            "[model_providers.kimi]\nbase_url = \"{}\"\n",
            global_conversion_proxy_url(&profile.id)
        );
        let restored = restore_deleted_global_proxy_base_url(&raw, &profile)
            .expect("restore")
            .expect("fixed proxy should restore");
        assert!(restored.contains("base_url = \"https://api.moonshot.cn/v1\""));

        let other_local = raw.replace(
            &global_conversion_proxy_url(&profile.id),
            "http://127.0.0.1:49152/v1",
        );
        assert!(restore_deleted_global_proxy_base_url(&other_local, &profile)
            .expect("restore")
            .is_none());
    }

    #[test]
    fn profile_index_does_not_auto_apply_an_editor_selection() {
        let profile = official_profile();
        let index = normalize_index(&[profile], vec!["profile-test".to_string()], None);
        assert_eq!(index.order, vec!["profile-test"]);
        assert_eq!(index.active_profile_id, None);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_secret_round_trip_does_not_store_plaintext() {
        let secret = "sk-test-secret";
        let encrypted = protect_secret(secret).expect("encrypt");
        assert!(!String::from_utf8_lossy(&encrypted).contains(secret));
        assert_eq!(unprotect_secret(&encrypted).expect("decrypt"), secret);
    }
}
