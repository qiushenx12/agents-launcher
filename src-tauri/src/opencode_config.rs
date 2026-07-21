use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use jsonc_parser::parse_to_serde_value;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use url::Url;
use uuid::Uuid;

use crate::cli_contract::CliKind;
use crate::file_transaction::{
    restore_json_backup_if_missing, write_json_atomic, write_private_json_atomic,
};
use crate::persistent_state::{
    load_profile_index_state, save_profile_index_state, ProfileIndexState,
};
use crate::{cli_runtime, model_fetcher};

const STATE_VERSION: u32 = 1;
const STATE_KEY: &str = "opencode";
const SCHEMA_URL: &str = "https://opencode.ai/config.json";
#[cfg(target_os = "macos")]
const WRITABLE_TEST_PATH: &str = "/bin/test";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpencodeProviderAuthMode {
    Existing,
    Managed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpencodeApiType {
    ChatCompletions,
    Responses,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpencodeProviderKind {
    Builtin,
    Custom,
}

fn default_provider_kind() -> OpencodeProviderKind {
    OpencodeProviderKind::Custom
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub context_limit: Option<u64>,
    #[serde(default)]
    pub output_limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeProvider {
    #[serde(default)]
    pub credential_id: String,
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_provider_kind")]
    pub provider_kind: OpencodeProviderKind,
    pub auth_mode: OpencodeProviderAuthMode,
    pub api_type: OpencodeApiType,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub env_key: String,
    #[serde(default)]
    pub models: Vec<OpencodeModel>,
    #[serde(default)]
    pub headers: Vec<OpencodeHeader>,
    #[serde(default)]
    pub has_stored_api_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeProfile {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub providers: Vec<OpencodeProvider>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub small_model: String,
    #[serde(default)]
    pub managed_config_path: String,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct OpencodeProfileState {
    #[serde(default = "default_state_version")]
    version: u32,
    #[serde(default)]
    profiles: Vec<OpencodeProfile>,
    #[serde(default, flatten)]
    extra: Map<String, Value>,
}

impl Default for OpencodeProfileState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            profiles: Vec::new(),
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeProfilesPayload {
    pub profiles: Vec<OpencodeProfile>,
    pub order: Vec<String>,
    pub active_profile_id: Option<String>,
    pub profiles_path: String,
    pub global_config_path: String,
    pub auth_path: String,
    pub model_state_path: String,
    pub connected_provider_ids: Vec<String>,
    pub provider_status_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSecretInput {
    pub credential_id: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOpencodeProfileRequest {
    pub profile: OpencodeProfile,
    #[serde(default)]
    pub secrets: Vec<ProviderSecretInput>,
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub active_profile_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyOpencodeProfileRequest {
    pub profile_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOpencodeProfileRequest {
    pub profile_id: String,
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub active_profile_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchOpencodeModelsRequest {
    #[serde(default)]
    pub profile_id: String,
    pub provider: OpencodeProvider,
    #[serde(default)]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeLaunchContext {
    pub config_path: String,
    pub env_vars: BTreeMap<String, String>,
    pub configured_model: String,
    pub model: String,
    pub small_model: String,
    pub provider_ids: Vec<String>,
    pub model_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeGlobalModel {
    #[serde(default)]
    pub original_id: String,
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub context_limit: Option<u64>,
    #[serde(default)]
    pub output_limit: Option<u64>,
    #[serde(default = "default_true")]
    pub input_text: bool,
    #[serde(default)]
    pub input_image: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeGlobalProvider {
    #[serde(default)]
    pub original_id: String,
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub npm: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub models: Vec<OpencodeGlobalModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeGlobalConfigPayload {
    pub config_path: String,
    pub revision: String,
    pub auth_path: String,
    pub auth_revision: String,
    pub connected_provider_ids: Vec<String>,
    pub disabled_provider_ids: Vec<String>,
    pub connection_keys: BTreeMap<String, String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub small_model: String,
    #[serde(default)]
    pub providers: Vec<OpencodeGlobalProvider>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOpencodeGlobalConfigRequest {
    pub config: OpencodeGlobalConfigPayload,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteOpencodeGlobalProviderRequest {
    pub provider: OpencodeGlobalProvider,
    pub revision: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOpencodeGlobalProviderRequest {
    pub provider_id: String,
    pub revision: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOpencodeGlobalOptionsRequest {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub small_model: String,
    pub revision: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchOpencodeGlobalModelsRequest {
    pub provider: OpencodeGlobalProvider,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOpencodeProviderConnectionRequest {
    pub provider_id: String,
    pub auth_revision: String,
    pub config_revision: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOpencodeProviderKeyRequest {
    pub provider_id: String,
    pub api_key: String,
    pub auth_revision: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisconnectOpencodeProviderRequest {
    pub provider_id: String,
    pub auth_revision: String,
    pub config_revision: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpencodeConnectionStatusPayload {
    pub auth_path: String,
    pub auth_revision: String,
    pub connected_provider_ids: Vec<String>,
    pub config_revision: String,
    pub disabled_provider_ids: Vec<String>,
    pub connection_keys: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpencodePermissionStatus {
    pub supported: bool,
    pub requires_repair: bool,
    pub directories: Vec<String>,
    pub blocked_directories: Vec<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct StoredModelRef {
    #[serde(rename = "providerID")]
    provider_id: String,
    #[serde(rename = "modelID")]
    model_id: String,
}

#[derive(Debug, Default, Deserialize)]
struct OpencodeModelState {
    #[serde(default)]
    recent: Vec<StoredModelRef>,
}

fn default_state_version() -> u32 {
    STATE_VERSION
}

fn app_data_dir() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|path| path.join("ClaudeEnvManager"))
        .ok_or_else(|| "无法确定 %APPDATA% 目录".to_string())
}

fn data_dir() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("opencode"))
}

fn profiles_path() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("profiles.json"))
}

fn managed_profiles_dir() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("profiles"))
}

fn credentials_dir() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("credentials"))
}

fn managed_config_path(profile_id: &str) -> Result<PathBuf, String> {
    Ok(managed_profiles_dir()?.join(format!("{profile_id}.jsonc")))
}

fn credential_path(profile_id: &str, credential_id: &str) -> Result<PathBuf, String> {
    Ok(credentials_dir()?
        .join(profile_id)
        .join(format!("{credential_id}.bin")))
}

fn home_dir() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "无法确定用户主目录".to_string())
}

fn global_config_path() -> Result<PathBuf, String> {
    let directory = home_dir()?.join(".config").join("opencode");
    let jsonc = directory.join("opencode.jsonc");
    let json = directory.join("opencode.json");
    if jsonc.exists() || !json.exists() {
        Ok(jsonc)
    } else {
        Ok(json)
    }
}

fn auth_path() -> Result<PathBuf, String> {
    Ok(home_dir()?
        .join(".local")
        .join("share")
        .join("opencode")
        .join("auth.json"))
}

fn model_state_path() -> Result<PathBuf, String> {
    Ok(home_dir()?
        .join(".local")
        .join("state")
        .join("opencode")
        .join("model.json"))
}

#[cfg(any(target_os = "macos", test))]
fn opencode_permission_directories(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".config").join("opencode"),
        home.join(".local").join("share").join("opencode"),
        home.join(".local").join("state").join("opencode"),
    ]
}

#[cfg(target_os = "macos")]
fn nearest_existing_ancestor(path: &Path) -> Option<&Path> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if candidate.exists() {
            return Some(candidate);
        }
        current = candidate.parent();
    }
    None
}

#[cfg(target_os = "macos")]
fn path_is_writable(path: &Path) -> bool {
    Command::new(WRITABLE_TEST_PATH)
        .arg("-w")
        .arg(path)
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "macos")]
fn opencode_permission_status_impl() -> Result<OpencodePermissionStatus, String> {
    use std::os::unix::fs::MetadataExt;

    let home = home_dir()?;
    let current_uid = fs::metadata(&home)
        .map_err(|error| format!("无法读取用户主目录权限：{error}"))?
        .uid();
    let directories = opencode_permission_directories(&home);
    let mut blocked = Vec::new();

    for directory in &directories {
        let needs_repair = if directory.exists() {
            let link_metadata = fs::symlink_metadata(directory)
                .map_err(|error| format!("无法读取 OpenCode 目录权限：{error}"))?;
            if link_metadata.file_type().is_symlink() || !link_metadata.is_dir() {
                true
            } else {
                link_metadata.uid() != current_uid || !path_is_writable(directory)
            }
        } else {
            nearest_existing_ancestor(directory)
                .is_none_or(|ancestor| !ancestor.is_dir() || !path_is_writable(ancestor))
        };
        if needs_repair {
            blocked.push(directory.display().to_string());
        }
    }

    Ok(OpencodePermissionStatus {
        supported: true,
        requires_repair: !blocked.is_empty(),
        directories: directories
            .into_iter()
            .map(|path| path.display().to_string())
            .collect(),
        blocked_directories: blocked,
    })
}

#[cfg(target_os = "macos")]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(target_os = "macos")]
fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('\"', "\\\""))
}

#[cfg(target_os = "macos")]
fn opencode_permission_repair_command(directories: &[PathBuf], owner: &str) -> String {
    let quoted_paths = directories
        .iter()
        .map(|path| shell_quote(&path.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "/bin/mkdir -p {quoted_paths} && /usr/sbin/chown -R -P {owner} {quoted_paths} && /bin/chmod 700 {quoted_paths}"
    )
}

#[tauri::command]
pub fn check_opencode_permissions() -> Result<OpencodePermissionStatus, String> {
    #[cfg(target_os = "macos")]
    {
        opencode_permission_status_impl()
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(OpencodePermissionStatus {
            supported: false,
            requires_repair: false,
            directories: Vec::new(),
            blocked_directories: Vec::new(),
        })
    }
}

#[tauri::command]
pub async fn repair_opencode_permissions() -> Result<OpencodePermissionStatus, String> {
    #[cfg(target_os = "macos")]
    {
        return tokio::task::spawn_blocking(|| {
            use std::os::unix::fs::MetadataExt;

            let current_status = opencode_permission_status_impl()?;
            if !current_status.requires_repair {
                return Ok(current_status);
            }
            let home = home_dir()?;
            let home_metadata =
                fs::metadata(&home).map_err(|error| format!("无法读取用户主目录权限：{error}"))?;
            let owner = format!("{}:{}", home_metadata.uid(), home_metadata.gid());
            let directories = opencode_permission_directories(&home);

            for directory in &directories {
                if directory.exists() {
                    let metadata = fs::symlink_metadata(directory)
                        .map_err(|error| format!("无法检查 OpenCode 目录：{error}"))?;
                    if metadata.file_type().is_symlink() || !metadata.is_dir() {
                        return Err(format!(
                            "拒绝自动修复非普通目录：{}。请先手动检查该路径。",
                            directory.display()
                        ));
                    }
                }
            }

            let shell_command = opencode_permission_repair_command(&directories, &owner);
            let apple_script = format!(
                "do shell script {} with administrator privileges",
                applescript_string(&shell_command)
            );
            let output = Command::new("/usr/bin/osascript")
                .args(["-e", &apple_script])
                .output()
                .map_err(|error| format!("无法打开 macOS 管理员授权：{error}"))?;
            if !output.status.success() {
                let detail = String::from_utf8_lossy(&output.stderr);
                if detail.contains("-128") || detail.to_ascii_lowercase().contains("canceled") {
                    return Err("已取消 macOS 管理员授权，目录权限未修改".to_string());
                }
                return Err(format!(
                    "macOS 管理员授权未完成：{}",
                    detail.trim().replace('\n', " ")
                ));
            }

            let status = opencode_permission_status_impl()?;
            if status.requires_repair {
                return Err(format!(
                    "授权完成，但以下目录仍不可写：{}",
                    status.blocked_directories.join("、")
                ));
            }
            Ok(status)
        })
        .await
        .map_err(|error| format!("OpenCode 权限修复任务异常结束：{error}"))?;
    }

    #[cfg(not(target_os = "macos"))]
    Err("一键修复 OpenCode 目录权限仅支持 macOS".to_string())
}

fn valid_stable_id(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

fn valid_env_key(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn validate_url(value: &str) -> Result<(), String> {
    let parsed = Url::parse(value).map_err(|error| format!("Base URL 无效：{error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Base URL 必须使用 http 或 https".to_string());
    }
    Ok(())
}

fn normalize_profile(mut profile: OpencodeProfile) -> Result<OpencodeProfile, String> {
    profile.name = profile.name.trim().to_string();
    if profile.name.is_empty() {
        return Err("请输入 OpenCode 配置名称".to_string());
    }
    if profile.id.trim().is_empty() {
        profile.id = format!("profile-{}", Uuid::new_v4());
    }
    if !valid_stable_id(&profile.id) {
        return Err("OpenCode profile ID 含有不支持的字符".to_string());
    }
    profile.model = profile.model.trim().to_string();
    profile.small_model = profile.small_model.trim().to_string();
    for (label, value) in [
        ("默认模型", &profile.model),
        ("Small model", &profile.small_model),
    ] {
        if !value.is_empty()
            && (!value.contains('/') || value.starts_with('/') || value.ends_with('/'))
        {
            return Err(format!("{label} 必须使用 provider_id/model_id 格式"));
        }
    }

    let mut provider_ids = HashSet::new();
    let mut credential_ids = HashSet::new();
    let mut managed_env_keys = HashSet::new();
    for provider in &mut profile.providers {
        provider.id = provider.id.trim().to_string();
        provider.name = provider.name.trim().to_string();
        provider.base_url = provider.base_url.trim().to_string();
        provider.env_key = provider.env_key.trim().to_string();
        if provider.credential_id.trim().is_empty() {
            provider.credential_id = format!("provider-{}", Uuid::new_v4());
        }
        if !valid_stable_id(&provider.credential_id) {
            return Err("OpenCode provider 凭据 ID 含有不支持的字符".to_string());
        }
        if !valid_stable_id(&provider.id) {
            return Err("provider ID 只能包含字母、数字、短横线和下划线".to_string());
        }
        if !provider_ids.insert(provider.id.clone()) {
            return Err(format!("provider ID '{}' 重复", provider.id));
        }
        if !credential_ids.insert(provider.credential_id.clone()) {
            return Err("provider 稳定凭据 ID 重复".to_string());
        }
        if provider.name.is_empty() {
            provider.name = provider.id.clone();
        }
        if provider.provider_kind == OpencodeProviderKind::Custom && provider.base_url.is_empty() {
            return Err(format!("自定义 provider '{}' 需要 Base URL", provider.id));
        }
        if !provider.base_url.is_empty() {
            validate_url(&provider.base_url)?;
        }
        if provider.auth_mode == OpencodeProviderAuthMode::Managed {
            if !valid_env_key(&provider.env_key) {
                return Err(format!("provider '{}' 的 Key 环境变量名无效", provider.id));
            }
            if !managed_env_keys.insert(provider.env_key.clone()) {
                return Err(format!(
                    "环境变量 '{}' 被多个 provider 重复使用",
                    provider.env_key
                ));
            }
        }
        let mut model_ids = HashSet::new();
        for model in &mut provider.models {
            model.id = model.id.trim().to_string();
            model.name = model.name.trim().to_string();
            if model.id.is_empty() {
                return Err(format!("provider '{}' 存在空模型 ID", provider.id));
            }
            if !model_ids.insert(model.id.clone()) {
                return Err(format!(
                    "provider '{}' 的模型 '{}' 重复",
                    provider.id, model.id
                ));
            }
            if model.name.is_empty() {
                model.name = model.id.clone();
            }
            if model.context_limit.is_some() != model.output_limit.is_some() {
                return Err(format!(
                    "provider '{}' 的模型 '{}' 必须同时填写 Context 与 Output limit",
                    provider.id, model.id
                ));
            }
        }
        for header in &mut provider.headers {
            header.name = header.name.trim().to_string();
            header.value = header.value.trim().to_string();
            if header.name.is_empty() {
                return Err(format!("provider '{}' 存在空 Header 名称", provider.id));
            }
            let normalized = header.name.to_ascii_lowercase();
            let sensitive = [
                "authorization",
                "api-key",
                "apikey",
                "token",
                "cookie",
                "secret",
            ]
            .iter()
            .any(|part| normalized.contains(part));
            if sensitive
                && !(header.value.starts_with("{env:") || header.value.starts_with("{file:"))
            {
                return Err(format!(
                    "provider '{}' 的敏感 Header '{}' 必须使用 {{env:...}} 或 {{file:...}} 占位符",
                    provider.id, header.name
                ));
            }
        }
    }
    profile.managed_config_path = managed_config_path(&profile.id)?.display().to_string();
    Ok(profile)
}

fn parse_jsonc(raw: &str, label: &str) -> Result<Value, String> {
    parse_to_serde_value(raw, &Default::default())
        .map_err(|error| format!("{label} 无法解析：{error}"))
}

fn object_mut(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    value.as_object_mut().expect("value normalized to object")
}

fn set_optional_string(map: &mut Map<String, Value>, key: &str, value: &str) {
    if value.is_empty() {
        map.remove(key);
    } else {
        map.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn npm_package(api_type: &OpencodeApiType) -> &'static str {
    match api_type {
        OpencodeApiType::ChatCompletions => "@ai-sdk/openai-compatible",
        OpencodeApiType::Responses => "@ai-sdk/openai",
    }
}

fn build_managed_config(
    existing: Option<&str>,
    previous: Option<&OpencodeProfile>,
    profile: &OpencodeProfile,
) -> Result<String, String> {
    let mut root = match existing {
        Some(raw) if !raw.trim().is_empty() => parse_jsonc(raw, "现有 OpenCode 受管配置")?,
        _ => Value::Object(Map::new()),
    };
    let root_map = object_mut(&mut root);
    root_map.insert("$schema".to_string(), Value::String(SCHEMA_URL.to_string()));
    set_optional_string(root_map, "model", &profile.model);
    set_optional_string(root_map, "small_model", &profile.small_model);

    let provider_value = root_map
        .entry("provider".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let provider_map = object_mut(provider_value);
    let current_ids = profile
        .providers
        .iter()
        .map(|provider| provider.id.as_str())
        .collect::<HashSet<_>>();
    if let Some(previous) = previous {
        for old in &previous.providers {
            if !current_ids.contains(old.id.as_str()) {
                provider_map.remove(&old.id);
            }
        }
    }

    for provider in &profile.providers {
        let entry = provider_map
            .entry(provider.id.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        let provider_object = object_mut(entry);
        set_optional_string(provider_object, "name", &provider.name);
        if provider.provider_kind == OpencodeProviderKind::Custom {
            provider_object.insert(
                "npm".to_string(),
                Value::String(npm_package(&provider.api_type).to_string()),
            );
        } else {
            provider_object.remove("npm");
        }

        let options_value = provider_object
            .entry("options".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let options = object_mut(options_value);
        set_optional_string(options, "baseURL", &provider.base_url);
        if provider.auth_mode == OpencodeProviderAuthMode::Managed {
            options.insert(
                "apiKey".to_string(),
                Value::String(format!("{{env:{}}}", provider.env_key)),
            );
        } else {
            options.remove("apiKey");
        }
        if provider.headers.is_empty() {
            options.remove("headers");
        } else {
            options.insert(
                "headers".to_string(),
                Value::Object(
                    provider
                        .headers
                        .iter()
                        .map(|header| (header.name.clone(), Value::String(header.value.clone())))
                        .collect(),
                ),
            );
        }
        if options.is_empty() {
            provider_object.remove("options");
        }

        let models_value = provider_object
            .entry("models".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let models = object_mut(models_value);
        if let Some(old_provider) =
            previous.and_then(|item| item.providers.iter().find(|old| old.id == provider.id))
        {
            let current_model_ids = provider
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<HashSet<_>>();
            for old in &old_provider.models {
                if !current_model_ids.contains(old.id.as_str()) {
                    models.remove(&old.id);
                }
            }
        }
        for model in &provider.models {
            let model_value = models
                .entry(model.id.clone())
                .or_insert_with(|| Value::Object(Map::new()));
            let model_object = object_mut(model_value);
            set_optional_string(model_object, "name", &model.name);
            if model.context_limit.is_some() || model.output_limit.is_some() {
                let mut limit = Map::new();
                if let Some(context) = model.context_limit {
                    limit.insert("context".to_string(), json!(context));
                }
                if let Some(output) = model.output_limit {
                    limit.insert("output".to_string(), json!(output));
                }
                model_object.insert("limit".to_string(), Value::Object(limit));
            } else {
                model_object.remove("limit");
            }
        }
        if models.is_empty() {
            provider_object.remove("models");
        }
    }

    if provider_map.is_empty() {
        root_map.remove("provider");
    }
    let rendered = serde_json::to_string_pretty(&root)
        .map_err(|error| format!("无法生成 OpenCode JSONC：{error}"))?;
    validate_managed_config(&rendered)?;
    Ok(format!("{rendered}\n"))
}

fn validate_managed_config(raw: &str) -> Result<(), String> {
    let value = parse_jsonc(raw, "OpenCode JSONC")?;
    let root = value
        .as_object()
        .ok_or_else(|| "OpenCode JSONC 顶层必须是对象".to_string())?;
    if let Some(providers) = root.get("provider") {
        if !providers.is_object() {
            return Err("OpenCode provider 必须是对象映射".to_string());
        }
    }
    for key in ["model", "small_model"] {
        if root.get(key).is_some_and(|value| !value.is_string()) {
            return Err(format!("OpenCode {key} 必须是字符串"));
        }
    }
    Ok(())
}

fn document_revision(raw: &str) -> String {
    let mut hasher = DefaultHasher::new();
    raw.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn is_custom_provider(value: &Value) -> bool {
    value
        .get("npm")
        .and_then(Value::as_str)
        .is_some_and(|npm| !npm.trim().is_empty())
}

fn read_global_config_document() -> Result<(PathBuf, String, Value), String> {
    let path = global_config_path()?;
    if !path.exists() {
        return Ok((path, String::new(), json!({ "$schema": SCHEMA_URL })));
    }
    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("无法读取 OpenCode 全局配置 {}：{error}", path.display()))?;
    let value = parse_jsonc(&raw, "OpenCode 全局配置")?;
    if !value.is_object() {
        return Err("OpenCode 全局配置顶层必须是对象".to_string());
    }
    Ok((path, raw, value))
}

fn global_model_from_value(id: &str, value: &Value) -> OpencodeGlobalModel {
    let input = value
        .get("modalities")
        .and_then(|modalities| modalities.get("input"))
        .and_then(Value::as_array);
    let context_limit = value
        .get("limit")
        .and_then(|limit| limit.get("context"))
        .and_then(Value::as_u64);
    let output_limit = value
        .get("limit")
        .and_then(|limit| limit.get("output"))
        .and_then(Value::as_u64);
    OpencodeGlobalModel {
        original_id: id.to_string(),
        id: id.to_string(),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_string(),
        context_limit,
        output_limit,
        input_text: input
            .map(|items| items.iter().any(|item| item.as_str() == Some("text")))
            .unwrap_or(true),
        input_image: input
            .is_some_and(|items| items.iter().any(|item| item.as_str() == Some("image"))),
    }
}

fn global_provider_from_value(id: &str, value: &Value) -> OpencodeGlobalProvider {
    let options = value.get("options").and_then(Value::as_object);
    let models = value
        .get("models")
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .map(|(model_id, model)| global_model_from_value(model_id, model))
                .collect()
        })
        .unwrap_or_default();
    OpencodeGlobalProvider {
        original_id: id.to_string(),
        id: id.to_string(),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_string(),
        npm: value
            .get("npm")
            .and_then(Value::as_str)
            .unwrap_or("@ai-sdk/openai-compatible")
            .to_string(),
        base_url: options
            .and_then(|items| items.get("baseURL"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        api_key: options
            .and_then(|items| items.get("apiKey"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        models,
    }
}

fn payload_from_global_document(
    path: &Path,
    raw: &str,
    value: &Value,
) -> Result<OpencodeGlobalConfigPayload, String> {
    let providers = value
        .get("provider")
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .filter(|(_, provider)| is_custom_provider(provider))
                .map(|(id, provider)| global_provider_from_value(id, provider))
                .collect()
        })
        .unwrap_or_default();
    Ok(OpencodeGlobalConfigPayload {
        config_path: path.display().to_string(),
        revision: document_revision(raw),
        auth_path: String::new(),
        auth_revision: String::new(),
        connected_provider_ids: Vec::new(),
        disabled_provider_ids: disabled_provider_ids(value)?,
        connection_keys: BTreeMap::new(),
        model: value
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        small_model: value
            .get("small_model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        providers,
    })
}

fn disabled_provider_ids(value: &Value) -> Result<Vec<String>, String> {
    let Some(disabled) = value.get("disabled_providers") else {
        return Ok(Vec::new());
    };
    let items = disabled
        .as_array()
        .ok_or_else(|| "OpenCode disabled_providers 必须是数组".to_string())?;
    let mut ids = items
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn read_auth_document() -> Result<(PathBuf, String, Value), String> {
    let path = auth_path()?;
    if !path.exists() {
        return Ok((path, String::new(), Value::Object(Map::new())));
    }
    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("无法读取 OpenCode 连接配置 {}：{error}", path.display()))?;
    let value = if raw.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str::<Value>(&raw)
            .map_err(|error| format!("OpenCode auth.json 无法解析：{error}"))?
    };
    if !value.is_object() {
        return Err("OpenCode auth.json 顶层必须是对象".to_string());
    }
    Ok((path, raw, value))
}

fn connection_status_from_document(
    path: &Path,
    raw: &str,
    value: &Value,
) -> OpencodeConnectionStatusPayload {
    let mut connected_provider_ids = value
        .as_object()
        .map(|items| items.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    connected_provider_ids.sort();
    let connection_keys = value
        .as_object()
        .map(|items| {
            items
                .iter()
                .filter_map(|(provider_id, credential)| {
                    (credential.get("type").and_then(Value::as_str) == Some("api"))
                        .then(|| {
                            credential
                                .get("key")
                                .and_then(Value::as_str)
                                .map(|key| (provider_id.clone(), key.to_string()))
                        })
                        .flatten()
                })
                .collect()
        })
        .unwrap_or_default();
    OpencodeConnectionStatusPayload {
        auth_path: path.display().to_string(),
        auth_revision: document_revision(raw),
        connected_provider_ids,
        config_revision: String::new(),
        disabled_provider_ids: Vec::new(),
        connection_keys,
    }
}

fn load_connection_status() -> Result<OpencodeConnectionStatusPayload, String> {
    let (path, raw, value) = read_auth_document()?;
    let mut status = connection_status_from_document(&path, &raw, &value);
    let (_, config_raw, config) = read_global_config_document()?;
    status.config_revision = document_revision(&config_raw);
    status.disabled_provider_ids = disabled_provider_ids(&config)?;
    Ok(status)
}

fn enrich_global_connection_status(
    mut payload: OpencodeGlobalConfigPayload,
) -> Result<OpencodeGlobalConfigPayload, String> {
    let status = load_connection_status()?;
    payload.auth_path = status.auth_path;
    payload.auth_revision = status.auth_revision;
    payload.connected_provider_ids = status.connected_provider_ids;
    payload.revision = status.config_revision;
    payload.disabled_provider_ids = status.disabled_provider_ids;
    payload.connection_keys = status.connection_keys;
    Ok(payload)
}

fn validate_auth_revision(raw: &str, expected: &str) -> Result<(), String> {
    if document_revision(raw) != expected {
        return Err("OpenCode 连接状态已被其它程序修改，请先重新读取".to_string());
    }
    Ok(())
}

fn ensure_api_credential_or_missing(
    auth: &Map<String, Value>,
    provider_id: &str,
) -> Result<(), String> {
    let Some(existing) = auth.get(provider_id) else {
        return Ok(());
    };
    if existing.get("type").and_then(Value::as_str) == Some("api") {
        return Ok(());
    }
    Err(format!(
        "Provider '{provider_id}' 使用的不是 API Key 连接；为避免影响内置/OAuth 凭据，启动器不会修改它"
    ))
}

fn set_api_credential(value: &mut Value, provider_id: &str, api_key: &str) -> Result<(), String> {
    let auth = object_mut(value);
    ensure_api_credential_or_missing(auth, provider_id)?;
    auth.insert(
        provider_id.to_string(),
        json!({ "type": "api", "key": api_key }),
    );
    Ok(())
}

fn set_provider_disabled(
    config: &mut Value,
    provider_id: &str,
    disabled: bool,
) -> Result<(), String> {
    let root = object_mut(config);
    let disabled_value = root
        .entry("disabled_providers".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let items = disabled_value
        .as_array_mut()
        .ok_or_else(|| "OpenCode disabled_providers 必须是数组".to_string())?;
    items.retain(|item| item.as_str() != Some(provider_id));
    if disabled {
        items.push(Value::String(provider_id.to_string()));
    }
    Ok(())
}

fn normalize_global_config(
    mut config: OpencodeGlobalConfigPayload,
) -> Result<OpencodeGlobalConfigPayload, String> {
    config.model = config.model.trim().to_string();
    config.small_model = config.small_model.trim().to_string();
    let mut provider_ids = HashSet::new();
    for provider in &mut config.providers {
        *provider = normalize_global_provider(provider.clone())?;
        if !provider_ids.insert(provider.id.clone()) {
            return Err(format!("Provider ID '{}' 重复", provider.id));
        }
    }
    Ok(config)
}

fn normalize_global_provider(
    mut provider: OpencodeGlobalProvider,
) -> Result<OpencodeGlobalProvider, String> {
    provider.original_id = provider.original_id.trim().to_string();
    provider.id = provider.id.trim().to_string();
    provider.name = provider.name.trim().to_string();
    provider.npm = provider.npm.trim().to_string();
    provider.base_url = provider.base_url.trim().to_string();
    provider.api_key = provider.api_key.trim().to_string();
    if !valid_stable_id(&provider.id) {
        return Err("Provider ID 只能包含字母、数字、短横线和下划线".to_string());
    }
    if !provider.original_id.is_empty() && !valid_stable_id(&provider.original_id) {
        return Err("原 Provider ID 含有不支持的字符".to_string());
    }
    if provider.name.is_empty() {
        provider.name = provider.id.clone();
    }
    if provider.npm.is_empty() {
        provider.npm = "@ai-sdk/openai-compatible".to_string();
    }
    if provider.base_url.is_empty() {
        return Err(format!("Provider '{}' 需要 API 地址", provider.id));
    }
    validate_url(&provider.base_url)?;
    let mut model_ids = HashSet::new();
    for model in &mut provider.models {
        model.original_id = model.original_id.trim().to_string();
        model.id = model.id.trim().to_string();
        model.name = model.name.trim().to_string();
        if model.id.is_empty() {
            return Err(format!("Provider '{}' 存在空模型 ID", provider.id));
        }
        if !model_ids.insert(model.id.clone()) {
            return Err(format!("模型 ID '{}' 重复", model.id));
        }
        if model.name.is_empty() {
            model.name = model.id.clone();
        }
        if !model.input_text && !model.input_image {
            return Err(format!(
                "模型 '{}' 至少选择 Text 或 Image 一种输入能力",
                model.id
            ));
        }
        if model.context_limit.is_some() != model.output_limit.is_some() {
            return Err(format!(
                "模型 '{}' 必须同时填写上下文长度与输出上限",
                model.id
            ));
        }
    }
    Ok(provider)
}

fn apply_global_model(model: &OpencodeGlobalModel, value: &mut Value) {
    let model_object = object_mut(value);
    set_optional_string(model_object, "name", &model.name);
    if model.context_limit.is_some() || model.output_limit.is_some() {
        let limit = model_object
            .entry("limit".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let limit_object = object_mut(limit);
        if let Some(context_limit) = model.context_limit {
            limit_object.insert("context".to_string(), json!(context_limit));
        } else {
            limit_object.remove("context");
        }
        if let Some(output_limit) = model.output_limit {
            limit_object.insert("output".to_string(), json!(output_limit));
        } else {
            limit_object.remove("output");
        }
    } else {
        let remove_limit =
            if let Some(limit) = model_object.get_mut("limit").and_then(Value::as_object_mut) {
                limit.remove("context");
                limit.remove("output");
                limit.is_empty()
            } else {
                false
            };
        if remove_limit {
            model_object.remove("limit");
        }
    }
    let modalities = model_object
        .entry("modalities".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let modalities_object = object_mut(modalities);
    let mut input = Vec::new();
    if model.input_text {
        input.push(Value::String("text".to_string()));
    }
    if model.input_image {
        input.push(Value::String("image".to_string()));
    }
    modalities_object.insert("input".to_string(), Value::Array(input));
}

fn apply_global_provider(
    provider: &OpencodeGlobalProvider,
    value: &mut Value,
) -> Result<(), String> {
    let provider_object = object_mut(value);
    set_optional_string(provider_object, "name", &provider.name);
    provider_object.insert("npm".to_string(), Value::String(provider.npm.clone()));
    let options = provider_object
        .entry("options".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let options_object = object_mut(options);
    set_optional_string(options_object, "baseURL", &provider.base_url);
    set_optional_string(options_object, "apiKey", &provider.api_key);

    let models = provider_object
        .entry("models".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let model_map = object_mut(models);
    let retained_original_ids = provider
        .models
        .iter()
        .filter_map(|model| (!model.original_id.is_empty()).then_some(model.original_id.as_str()))
        .collect::<HashSet<_>>();
    model_map.retain(|id, _| retained_original_ids.contains(id.as_str()));
    for model in &provider.models {
        if model.id != model.original_id && model_map.contains_key(&model.id) {
            return Err(format!("模型 ID '{}' 已存在，无法重命名", model.id));
        }
        let mut model_value = if model.original_id.is_empty() {
            Value::Object(Map::new())
        } else {
            model_map
                .remove(&model.original_id)
                .unwrap_or_else(|| Value::Object(Map::new()))
        };
        apply_global_model(model, &mut model_value);
        model_map.insert(model.id.clone(), model_value);
    }
    Ok(())
}

fn build_global_config(
    mut root: Value,
    config: &OpencodeGlobalConfigPayload,
) -> Result<String, String> {
    let root_object = object_mut(&mut root);
    root_object
        .entry("$schema".to_string())
        .or_insert_with(|| Value::String(SCHEMA_URL.to_string()));
    set_optional_string(root_object, "model", &config.model);
    set_optional_string(root_object, "small_model", &config.small_model);
    let providers = root_object
        .entry("provider".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let provider_map = object_mut(providers);

    for provider in &config.providers {
        if provider_map.contains_key(&provider.id) && provider.id != provider.original_id {
            return Err(format!(
                "Provider ID '{}' 已被现有配置使用，无法重命名",
                provider.id
            ));
        }
    }
    let retained_custom_ids = config
        .providers
        .iter()
        .filter_map(|provider| {
            (!provider.original_id.is_empty()).then_some(provider.original_id.as_str())
        })
        .collect::<HashSet<_>>();
    provider_map.retain(|id, value| {
        !is_custom_provider(value) || retained_custom_ids.contains(id.as_str())
    });
    for provider in &config.providers {
        let mut provider_value = if provider.original_id.is_empty() {
            Value::Object(Map::new())
        } else {
            provider_map
                .remove(&provider.original_id)
                .unwrap_or_else(|| Value::Object(Map::new()))
        };
        apply_global_provider(provider, &mut provider_value)?;
        provider_map.insert(provider.id.clone(), provider_value);
    }
    if provider_map.is_empty() {
        root_object.remove("provider");
    }
    render_global_config(&root)
}

fn render_global_config(root: &Value) -> Result<String, String> {
    let rendered = serde_json::to_string_pretty(&root)
        .map_err(|error| format!("无法生成 OpenCode 全局配置：{error}"))?;
    validate_managed_config(&rendered)?;
    Ok(format!("{rendered}\n"))
}

fn build_global_provider_update(
    mut root: Value,
    provider: &OpencodeGlobalProvider,
) -> Result<String, String> {
    let root_object = object_mut(&mut root);
    root_object
        .entry("$schema".to_string())
        .or_insert_with(|| Value::String(SCHEMA_URL.to_string()));
    let providers = root_object
        .entry("provider".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let provider_map = object_mut(providers);

    if provider.original_id.is_empty() {
        if provider_map.contains_key(&provider.id) {
            return Err(format!(
                "Provider ID '{}' 已被现有配置使用，无法写入",
                provider.id
            ));
        }
    } else {
        let existing = provider_map.get(&provider.original_id).ok_or_else(|| {
            format!(
                "Provider '{}' 已不在目标 JSON 中，请重新读取",
                provider.original_id
            )
        })?;
        if !is_custom_provider(existing) {
            return Err(format!(
                "Provider '{}' 不是启动器可管理的自定义供应商",
                provider.original_id
            ));
        }
        if provider.id != provider.original_id && provider_map.contains_key(&provider.id) {
            return Err(format!(
                "Provider ID '{}' 已被现有配置使用，无法重命名",
                provider.id
            ));
        }
    }

    let mut provider_value = if provider.original_id.is_empty() {
        Value::Object(Map::new())
    } else {
        provider_map
            .remove(&provider.original_id)
            .unwrap_or_else(|| Value::Object(Map::new()))
    };
    apply_global_provider(provider, &mut provider_value)?;
    provider_map.insert(provider.id.clone(), provider_value);

    if !provider.original_id.is_empty() && provider.original_id != provider.id {
        if let Some(disabled) = root_object
            .get_mut("disabled_providers")
            .and_then(Value::as_array_mut)
        {
            for item in disabled {
                if item.as_str() == Some(provider.original_id.as_str()) {
                    *item = Value::String(provider.id.clone());
                }
            }
        }
    }
    render_global_config(&root)
}

fn build_global_provider_delete(mut root: Value, provider_id: &str) -> Result<String, String> {
    let root_object = object_mut(&mut root);
    let remove_provider_map = {
        let provider_map = root_object
            .get_mut("provider")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| format!("Provider '{provider_id}' 不存在于目标 JSON"))?;
        let existing = provider_map
            .get(provider_id)
            .ok_or_else(|| format!("Provider '{provider_id}' 不存在于目标 JSON"))?;
        if !is_custom_provider(existing) {
            return Err(format!(
                "Provider '{provider_id}' 不是启动器可管理的自定义供应商"
            ));
        }
        provider_map.remove(provider_id);
        provider_map.is_empty()
    };
    if remove_provider_map {
        root_object.remove("provider");
    }

    let remove_disabled_list = if let Some(disabled) = root_object
        .get_mut("disabled_providers")
        .and_then(Value::as_array_mut)
    {
        disabled.retain(|item| item.as_str() != Some(provider_id));
        disabled.is_empty()
    } else {
        false
    };
    if remove_disabled_list {
        root_object.remove("disabled_providers");
    }
    render_global_config(&root)
}

fn build_global_options_update(
    mut root: Value,
    model: &str,
    small_model: &str,
) -> Result<String, String> {
    let root_object = object_mut(&mut root);
    root_object
        .entry("$schema".to_string())
        .or_insert_with(|| Value::String(SCHEMA_URL.to_string()));
    set_optional_string(root_object, "model", model.trim());
    set_optional_string(root_object, "small_model", small_model.trim());
    render_global_config(&root)
}

fn has_plaintext_api_key(value: &Value) -> bool {
    value
        .get("provider")
        .and_then(Value::as_object)
        .is_some_and(|providers| {
            providers.values().any(|provider| {
                provider
                    .get("options")
                    .and_then(|options| options.get("apiKey"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .is_some_and(|key| {
                        !key.is_empty()
                            && !(key.starts_with("{env:") && key.ends_with('}'))
                            && !(key.starts_with("{file:") && key.ends_with('}'))
                    })
            })
        })
}

fn write_global_config_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    let value: Value = serde_json::from_slice(content)
        .map_err(|error| format!("OpenCode 全局配置不是有效 JSON：{error}"))?;
    // The backup contains the old document, so it must stay private even when
    // this write is the one that removes the last plaintext key.
    let existing_has_plaintext_api_key = match fs::read_to_string(path) {
        Ok(raw) => has_plaintext_api_key(&parse_jsonc(&raw, "现有 OpenCode 全局配置")?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(format!("无法读取现有 OpenCode 全局配置：{error}")),
    };
    if has_plaintext_api_key(&value) || existing_has_plaintext_api_key {
        write_private_json_atomic(path, content, "OpenCode 全局配置")
    } else {
        write_json_atomic(path, content, "OpenCode 全局配置")
    }
}

#[tauri::command]
pub fn load_opencode_global_config() -> Result<OpencodeGlobalConfigPayload, String> {
    let (path, raw, value) = read_global_config_document()?;
    enrich_global_connection_status(payload_from_global_document(&path, &raw, &value)?)
}

#[tauri::command]
pub fn save_opencode_global_config(
    request: SaveOpencodeGlobalConfigRequest,
) -> Result<OpencodeGlobalConfigPayload, String> {
    let config = normalize_global_config(request.config)?;
    let (path, raw, value) = read_global_config_document()?;
    if document_revision(&raw) != config.revision {
        return Err("opencode.jsonc 已被其它程序修改，请先刷新后再保存".to_string());
    }
    let rendered = build_global_config(value, &config)?;
    write_global_config_atomic(&path, rendered.as_bytes())?;
    load_opencode_global_config()
}

#[tauri::command]
pub fn write_opencode_global_provider(
    request: WriteOpencodeGlobalProviderRequest,
) -> Result<OpencodeGlobalConfigPayload, String> {
    let provider = normalize_global_provider(request.provider)?;
    let (path, raw, value) = read_global_config_document()?;
    if document_revision(&raw) != request.revision {
        return Err("opencode.jsonc 已被其它程序修改，请先刷新后再写入".to_string());
    }
    let rendered = build_global_provider_update(value, &provider)?;
    write_global_config_atomic(&path, rendered.as_bytes())?;
    load_opencode_global_config()
}

#[tauri::command]
pub fn delete_opencode_global_provider(
    request: DeleteOpencodeGlobalProviderRequest,
) -> Result<OpencodeGlobalConfigPayload, String> {
    let provider_id = request.provider_id.trim();
    if !valid_stable_id(provider_id) {
        return Err("Provider ID 无效，无法从目标 JSON 删除".to_string());
    }
    let (path, raw, value) = read_global_config_document()?;
    if document_revision(&raw) != request.revision {
        return Err("opencode.jsonc 已被其它程序修改，请先刷新后再删除".to_string());
    }
    let rendered = build_global_provider_delete(value, provider_id)?;
    write_global_config_atomic(&path, rendered.as_bytes())?;
    load_opencode_global_config()
}

#[tauri::command]
pub fn save_opencode_global_options(
    request: SaveOpencodeGlobalOptionsRequest,
) -> Result<OpencodeGlobalConfigPayload, String> {
    let (path, raw, value) = read_global_config_document()?;
    if document_revision(&raw) != request.revision {
        return Err("opencode.jsonc 已被其它程序修改，请先刷新后再保存".to_string());
    }
    let rendered = build_global_options_update(value, &request.model, &request.small_model)?;
    write_global_config_atomic(&path, rendered.as_bytes())?;
    load_opencode_global_config()
}

#[tauri::command]
pub fn save_opencode_provider_connection(
    request: SaveOpencodeProviderConnectionRequest,
) -> Result<OpencodeConnectionStatusPayload, String> {
    let provider_id = request.provider_id.trim();
    if !valid_stable_id(provider_id) {
        return Err("Provider ID 无效，无法保存连接".to_string());
    }
    let (config_path, config_raw, mut config) = read_global_config_document()?;
    if document_revision(&config_raw) != request.config_revision {
        return Err("opencode.jsonc 已被其它程序修改，请先重新读取".to_string());
    }
    let provider = config
        .get("provider")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get(provider_id))
        .filter(|provider| is_custom_provider(provider))
        .ok_or_else(|| format!("自定义 Provider '{provider_id}' 不存在于 opencode.jsonc"))?;
    let config_has_key = provider
        .get("options")
        .and_then(|options| options.get("apiKey"))
        .and_then(Value::as_str)
        .is_some_and(|key| !key.trim().is_empty());
    let (_, auth_raw, auth) = read_auth_document()?;
    validate_auth_revision(&auth_raw, &request.auth_revision)?;
    let auth_has_key = auth
        .get(provider_id)
        .filter(|credential| credential.get("type").and_then(Value::as_str) == Some("api"))
        .and_then(|credential| credential.get("key"))
        .and_then(Value::as_str)
        .is_some_and(|key| !key.is_empty());
    if !config_has_key && !auth_has_key {
        return Err("该 Provider 尚未保存 Key，请先使用独立的“保存 Key”操作".to_string());
    }
    set_provider_disabled(&mut config, provider_id, false)?;
    let rendered = serde_json::to_vec_pretty(&config)
        .map_err(|error| format!("无法生成 OpenCode 全局配置：{error}"))?;
    write_global_config_atomic(&config_path, &rendered)?;
    load_connection_status()
}

#[tauri::command]
pub fn save_opencode_provider_key(
    request: SaveOpencodeProviderKeyRequest,
) -> Result<OpencodeConnectionStatusPayload, String> {
    let provider_id = request.provider_id.trim();
    if !valid_stable_id(provider_id) {
        return Err("Provider ID 无效，无法保存 Key".to_string());
    }
    let api_key = request.api_key.trim();
    if api_key.is_empty() {
        return Err("Key 不能为空".to_string());
    }
    let (_, _, config) = read_global_config_document()?;
    let provider_exists = config
        .get("provider")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get(provider_id))
        .is_some_and(is_custom_provider);
    if !provider_exists {
        return Err(format!(
            "自定义 Provider '{provider_id}' 不存在于 opencode.jsonc"
        ));
    }
    let (auth_path, auth_raw, mut auth) = read_auth_document()?;
    validate_auth_revision(&auth_raw, &request.auth_revision)?;
    set_api_credential(&mut auth, provider_id, api_key)?;
    let rendered = serde_json::to_vec_pretty(&auth)
        .map_err(|error| format!("无法生成 OpenCode 连接配置：{error}"))?;
    write_private_json_atomic(&auth_path, &rendered, "OpenCode 连接 Key")?;
    load_connection_status()
}

#[tauri::command]
pub fn disconnect_opencode_provider(
    request: DisconnectOpencodeProviderRequest,
) -> Result<OpencodeConnectionStatusPayload, String> {
    let provider_id = request.provider_id.trim();
    let (config_path, config_raw, mut config) = read_global_config_document()?;
    if document_revision(&config_raw) != request.config_revision {
        return Err("opencode.jsonc 已被其它程序修改，请先重新读取".to_string());
    }
    let provider_exists = config
        .get("provider")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get(provider_id))
        .is_some_and(is_custom_provider);
    if !provider_exists {
        return Err(format!(
            "自定义 Provider '{provider_id}' 不存在于 opencode.jsonc"
        ));
    }
    let (_, auth_raw, _) = read_auth_document()?;
    validate_auth_revision(&auth_raw, &request.auth_revision)?;
    set_provider_disabled(&mut config, provider_id, true)?;
    let rendered = serde_json::to_vec_pretty(&config)
        .map_err(|error| format!("无法生成 OpenCode 全局配置：{error}"))?;
    write_global_config_atomic(&config_path, &rendered)?;
    load_connection_status()
}

fn resolve_global_api_key(value: &str) -> Result<String, String> {
    let value = value.trim();
    if let Some(name) = value
        .strip_prefix("{env:")
        .and_then(|item| item.strip_suffix('}'))
    {
        return std::env::var(name).map_err(|_| format!("环境变量 '{name}' 不存在，无法获取模型"));
    }
    if value.starts_with("{file:") {
        return Err(
            "获取模型暂不读取 {file:...} 凭据，请在 OpenCode 中验证该 Provider".to_string(),
        );
    }
    Ok(value.to_string())
}

#[tauri::command]
pub async fn fetch_opencode_global_models(
    request: FetchOpencodeGlobalModelsRequest,
) -> Result<Vec<String>, String> {
    let provider = request.provider;
    validate_url(provider.base_url.trim())?;
    let api_key = resolve_global_api_key(&provider.api_key)?;
    model_fetcher::fetch_openai_compatible_models(provider.base_url.trim(), &api_key).await
}

fn load_state() -> Result<OpencodeProfileState, String> {
    let path = profiles_path()?;
    restore_json_backup_if_missing(&path, "OpenCode 方案索引")?;
    if !path.exists() {
        return Ok(OpencodeProfileState::default());
    }
    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("无法读取 OpenCode 方案索引：{error}"))?;
    serde_json::from_str(&raw).map_err(|error| format!("OpenCode 方案索引无法解析：{error}"))
}

fn save_state(state: &OpencodeProfileState) -> Result<(), String> {
    let content = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("无法序列化 OpenCode 方案索引：{error}"))?;
    write_json_atomic(&profiles_path()?, &content, "OpenCode 方案索引")
}

fn normalize_index(
    profiles: &[OpencodeProfile],
    requested_order: Vec<String>,
    requested_active: Option<String>,
) -> ProfileIndexState {
    let ids = profiles
        .iter()
        .map(|profile| profile.id.clone())
        .collect::<BTreeSet<_>>();
    let mut seen = HashSet::new();
    let mut order = requested_order
        .into_iter()
        .filter(|id| ids.contains(id) && seen.insert(id.clone()))
        .collect::<Vec<_>>();
    for id in &ids {
        if seen.insert(id.clone()) {
            order.push(id.clone());
        }
    }
    let active_profile_id = requested_active.filter(|id| ids.contains(id));
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

fn enrich_profiles(state: &mut OpencodeProfileState) -> Result<(), String> {
    for profile in &mut state.profiles {
        profile.managed_config_path = managed_config_path(&profile.id)?.display().to_string();
        for provider in &mut profile.providers {
            provider.has_stored_api_key =
                credential_path(&profile.id, &provider.credential_id)?.exists();
        }
        let config_path = managed_config_path(&profile.id)?;
        if config_path.exists() {
            let raw = fs::read_to_string(&config_path)
                .map_err(|error| format!("无法读取 OpenCode 受管配置：{error}"))?;
            validate_managed_config(&raw)?;
        }
    }
    Ok(())
}

#[allow(unused_mut)]
fn hidden_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    crate::platform_env::apply_effective_path(&mut command);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn strip_ansi(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        } else {
            output.push(character);
        }
    }
    output
}

fn connected_provider_ids() -> Result<Vec<String>, String> {
    let executable = cli_runtime::locate_cli(CliKind::Opencode)
        .ok_or_else(|| "未检测到 OpenCode，无法读取提供商登录状态".to_string())?;
    let output = hidden_command(executable)
        .args(["providers", "list", "--pure"])
        .output()
        .map_err(|error| format!("无法执行 opencode providers list：{error}"))?;
    if !output.status.success() {
        return Err(strip_ansi(&String::from_utf8_lossy(&output.stderr))
            .trim()
            .chars()
            .take(1000)
            .collect());
    }
    let clean = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    let mut ids = clean
        .lines()
        .filter_map(|line| line.trim().strip_prefix('•'))
        .filter_map(|line| line.split_whitespace().next())
        .filter(|id| valid_stable_id(id))
        .map(str::to_string)
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn load_payload() -> Result<OpencodeProfilesPayload, String> {
    let mut state = load_state()?;
    enrich_profiles(&mut state)?;
    let stored = load_profile_index_state(STATE_KEY)?;
    let index = normalize_index(&state.profiles, stored.order, stored.active_profile_id);
    let (connected_provider_ids, provider_status_error) = match connected_provider_ids() {
        Ok(ids) => (ids, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    Ok(OpencodeProfilesPayload {
        profiles: state.profiles,
        order: index.order,
        active_profile_id: index.active_profile_id,
        profiles_path: profiles_path()?.display().to_string(),
        global_config_path: global_config_path()?.display().to_string(),
        auth_path: auth_path()?.display().to_string(),
        model_state_path: model_state_path()?.display().to_string(),
        connected_provider_ids,
        provider_status_error,
    })
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!("{value}.{suffix}"))
        .unwrap_or_else(|| suffix.to_string());
    path.with_extension(extension)
}

fn write_bytes_atomic(path: &Path, content: &[u8], label: &str) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| format!("{label} 没有父目录"))?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建 {label} 目录：{error}"))?;
    let temp = sidecar_path(path, "tmp");
    let backup = sidecar_path(path, "bak");
    let mut file =
        fs::File::create(&temp).map_err(|error| format!("无法创建 {label} 临时文件：{error}"))?;
    file.write_all(content)
        .map_err(|error| format!("无法写入 {label} 临时文件：{error}"))?;
    file.sync_all()
        .map_err(|error| format!("无法刷新 {label} 临时文件：{error}"))?;
    drop(file);
    if path.exists() {
        if backup.exists() {
            fs::remove_file(&backup).map_err(|error| format!("无法替换 {label} 备份：{error}"))?;
        }
        fs::rename(path, &backup).map_err(|error| format!("无法备份 {label}：{error}"))?;
    }
    if let Err(error) = fs::rename(&temp, path) {
        if backup.exists() && !path.exists() {
            let _ = fs::rename(&backup, path);
        }
        return Err(format!("无法提交 {label}：{error}"));
    }
    Ok(())
}

fn remove_file(path: &Path) -> Result<(), String> {
    if path.exists() {
        fs::remove_file(path).map_err(|error| format!("无法删除 {}：{error}", path.display()))?;
    }
    for suffix in ["tmp", "bak"] {
        let sidecar = sidecar_path(path, suffix);
        if sidecar.exists() {
            fs::remove_file(&sidecar)
                .map_err(|error| format!("无法删除 {}：{error}", sidecar.display()))?;
        }
    }
    Ok(())
}

fn restore_snapshot(path: &Path, content: Option<&[u8]>) -> Result<(), String> {
    match content {
        Some(content) => write_bytes_atomic(path, content, "OpenCode 回滚文件"),
        None => remove_file(path),
    }
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
            w!("Agents Launcher OpenCode Provider API Key"),
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
        String::from_utf8(decrypted).map_err(|error| format!("OpenCode 凭据不是 UTF-8：{error}"))
    }
}

#[cfg(not(windows))]
fn protect_secret(_secret: &str) -> Result<Vec<u8>, String> {
    Err("OpenCode 凭据加密仅支持 Windows".to_string())
}

#[cfg(not(windows))]
fn unprotect_secret(_encrypted: &[u8]) -> Result<String, String> {
    Err("OpenCode 凭据解密仅支持 Windows".to_string())
}

fn resolve_provider_key(profile_id: &str, provider: &OpencodeProvider) -> Result<String, String> {
    let path = credential_path(profile_id, &provider.credential_id)?;
    if path.exists() {
        return unprotect_secret(
            &fs::read(&path).map_err(|error| format!("无法读取 OpenCode 加密凭据：{error}"))?,
        );
    }
    std::env::var(&provider.env_key).map_err(|_| {
        format!(
            "provider '{}' 没有已保存的 API Key，环境变量 '{}' 也不存在",
            provider.id, provider.env_key
        )
    })
}

#[tauri::command]
pub fn load_opencode_profiles() -> Result<OpencodeProfilesPayload, String> {
    load_payload()
}

#[tauri::command]
pub fn save_opencode_profile(
    request: SaveOpencodeProfileRequest,
) -> Result<OpencodeProfilesPayload, String> {
    let profile = normalize_profile(request.profile)?;
    let metadata_path = profiles_path()?;
    let config_path = managed_config_path(&profile.id)?;
    let mut state = load_state()?;
    if state
        .profiles
        .iter()
        .any(|item| item.id != profile.id && item.name == profile.name)
    {
        return Err(format!("OpenCode 配置名称 '{}' 已存在", profile.name));
    }
    let previous_profile = state
        .profiles
        .iter()
        .find(|item| item.id == profile.id)
        .cloned();
    let existing_config = fs::read_to_string(&config_path).ok();
    let rendered = build_managed_config(
        existing_config.as_deref(),
        previous_profile.as_ref(),
        &profile,
    )?;
    let previous_metadata = fs::read(&metadata_path).ok();
    let previous_config = fs::read(&config_path).ok();
    let previous_index = load_profile_index_state(STATE_KEY)?;
    let secret_inputs = request
        .secrets
        .into_iter()
        .map(|secret| (secret.credential_id.clone(), secret))
        .collect::<HashMap<_, _>>();
    let previous_credential_ids = previous_profile
        .as_ref()
        .map(|item| {
            item.providers
                .iter()
                .map(|provider| provider.credential_id.clone())
                .collect()
        })
        .unwrap_or_else(Vec::new);
    let affected_credentials = previous_credential_ids
        .iter()
        .cloned()
        .chain(
            profile
                .providers
                .iter()
                .map(|provider| provider.credential_id.clone()),
        )
        .collect::<HashSet<_>>();
    let credential_snapshots = affected_credentials
        .iter()
        .map(|id| {
            let path = credential_path(&profile.id, id)?;
            Ok((path.clone(), fs::read(path).ok()))
        })
        .collect::<Result<Vec<_>, String>>()?;

    let mut stored_profile = profile.clone();
    let transaction = (|| {
        write_json_atomic(&config_path, rendered.as_bytes(), "OpenCode 受管 JSONC")?;
        let current_credential_ids = stored_profile
            .providers
            .iter()
            .map(|provider| provider.credential_id.clone())
            .collect::<HashSet<_>>();
        for old in previous_credential_ids {
            if !current_credential_ids.contains(&old) {
                remove_file(&credential_path(&stored_profile.id, &old)?)?;
            }
        }
        for provider in &mut stored_profile.providers {
            let path = credential_path(&stored_profile.id, &provider.credential_id)?;
            if provider.auth_mode == OpencodeProviderAuthMode::Existing {
                remove_file(&path)?;
                provider.has_stored_api_key = false;
                continue;
            }
            if let Some(input) = secret_inputs.get(&provider.credential_id) {
                if input.clear_api_key {
                    remove_file(&path)?;
                } else if let Some(api_key) = input
                    .api_key
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    write_bytes_atomic(&path, &protect_secret(api_key)?, "OpenCode DPAPI 凭据")?;
                }
            }
            provider.has_stored_api_key = path.exists();
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
        save_state(&state)?;
        let index = normalize_index(
            &state.profiles,
            request.order.clone(),
            request.active_profile_id.clone(),
        );
        save_profile_index_state(STATE_KEY, &index)?;
        if load_state()? != state || load_profile_index_state(STATE_KEY)? != index {
            return Err("OpenCode 方案写入后回读不一致".to_string());
        }
        validate_managed_config(
            &fs::read_to_string(&config_path)
                .map_err(|error| format!("无法回读 OpenCode JSONC：{error}"))?,
        )?;
        for provider in &stored_profile.providers {
            if provider.has_stored_api_key {
                let path = credential_path(&stored_profile.id, &provider.credential_id)?;
                unprotect_secret(
                    &fs::read(path).map_err(|error| format!("无法回读 OpenCode 凭据：{error}"))?,
                )?;
            }
        }
        Ok(())
    })();
    if let Err(error) = transaction {
        let mut rollback_errors = Vec::new();
        if let Err(rollback) = save_profile_index_state(STATE_KEY, &previous_index) {
            rollback_errors.push(rollback);
        }
        if let Err(rollback) = restore_snapshot(&metadata_path, previous_metadata.as_deref()) {
            rollback_errors.push(rollback);
        }
        if let Err(rollback) = restore_snapshot(&config_path, previous_config.as_deref()) {
            rollback_errors.push(rollback);
        }
        for (path, snapshot) in credential_snapshots {
            if let Err(rollback) = restore_snapshot(&path, snapshot.as_deref()) {
                rollback_errors.push(rollback);
            }
        }
        return Err(if rollback_errors.is_empty() {
            format!("保存 OpenCode 配置失败，旧数据已恢复：{error}")
        } else {
            format!(
                "保存 OpenCode 配置失败且回滚不完整：{error}；{}",
                rollback_errors.join("；")
            )
        });
    }
    load_payload()
}

#[tauri::command]
pub fn apply_opencode_profile(
    request: ApplyOpencodeProfileRequest,
) -> Result<OpencodeProfilesPayload, String> {
    let mut state = load_state()?;
    enrich_profiles(&mut state)?;
    let profile = state
        .profiles
        .iter()
        .find(|profile| profile.id == request.profile_id)
        .ok_or_else(|| format!("OpenCode 配置方案 '{}' 不存在", request.profile_id))?;
    validate_managed_config(
        &fs::read_to_string(managed_config_path(&profile.id)?)
            .map_err(|error| format!("无法读取 OpenCode 受管配置：{error}"))?,
    )?;
    for provider in &profile.providers {
        if provider.auth_mode == OpencodeProviderAuthMode::Managed {
            resolve_provider_key(&profile.id, provider)?;
        }
    }
    let previous = load_profile_index_state(STATE_KEY)?;
    let index = normalize_index(
        &state.profiles,
        previous.order.clone(),
        Some(profile.id.clone()),
    );
    save_profile_index_state(STATE_KEY, &index)?;
    if load_profile_index_state(STATE_KEY)? != index {
        let _ = save_profile_index_state(STATE_KEY, &previous);
        return Err("OpenCode 活动方案写入后回读不一致，旧状态已恢复".to_string());
    }
    load_payload()
}

#[tauri::command]
pub fn delete_opencode_profile(
    request: DeleteOpencodeProfileRequest,
) -> Result<OpencodeProfilesPayload, String> {
    let metadata_path = profiles_path()?;
    let config_path = managed_config_path(&request.profile_id)?;
    let mut state = load_state()?;
    let profile = state
        .profiles
        .iter()
        .find(|profile| profile.id == request.profile_id)
        .cloned()
        .ok_or_else(|| "要删除的 OpenCode 配置不存在".to_string())?;
    let previous_metadata = fs::read(&metadata_path).ok();
    let previous_config = fs::read(&config_path).ok();
    let previous_index = load_profile_index_state(STATE_KEY)?;
    let credential_snapshots = profile
        .providers
        .iter()
        .map(|provider| {
            let path = credential_path(&profile.id, &provider.credential_id)?;
            Ok((path.clone(), fs::read(path).ok()))
        })
        .collect::<Result<Vec<_>, String>>()?;
    state.profiles.retain(|item| item.id != profile.id);
    let transaction = (|| {
        remove_file(&config_path)?;
        for provider in &profile.providers {
            remove_file(&credential_path(&profile.id, &provider.credential_id)?)?;
        }
        let credential_directory = credentials_dir()?.join(&profile.id);
        if credential_directory.exists() {
            fs::remove_dir(&credential_directory)
                .map_err(|error| format!("无法删除 OpenCode 凭据目录：{error}"))?;
        }
        save_state(&state)?;
        let index = normalize_index(
            &state.profiles,
            request.order.clone(),
            request.active_profile_id.clone(),
        );
        save_profile_index_state(STATE_KEY, &index)?;
        if load_state()? != state || load_profile_index_state(STATE_KEY)? != index {
            return Err("删除 OpenCode 配置后回读不一致".to_string());
        }
        Ok(())
    })();
    if let Err(error) = transaction {
        let mut rollback_errors = Vec::new();
        if let Err(rollback) = save_profile_index_state(STATE_KEY, &previous_index) {
            rollback_errors.push(rollback);
        }
        if let Err(rollback) = restore_snapshot(&metadata_path, previous_metadata.as_deref()) {
            rollback_errors.push(rollback);
        }
        if let Err(rollback) = restore_snapshot(&config_path, previous_config.as_deref()) {
            rollback_errors.push(rollback);
        }
        for (path, snapshot) in credential_snapshots {
            if let Err(rollback) = restore_snapshot(&path, snapshot.as_deref()) {
                rollback_errors.push(rollback);
            }
        }
        return Err(if rollback_errors.is_empty() {
            format!("删除 OpenCode 配置失败，旧数据已恢复：{error}")
        } else {
            format!(
                "删除 OpenCode 配置失败且回滚不完整：{error}；{}",
                rollback_errors.join("；")
            )
        });
    }
    load_payload()
}

fn resolve_context(profile_id: &str) -> Result<OpencodeLaunchContext, String> {
    let mut state = load_state()?;
    enrich_profiles(&mut state)?;
    let profile = state
        .profiles
        .into_iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| format!("OpenCode 配置方案 '{profile_id}' 不存在"))?;
    let path = managed_config_path(&profile.id)?;
    let mut env_vars = BTreeMap::new();
    for provider in &profile.providers {
        if provider.auth_mode == OpencodeProviderAuthMode::Managed {
            env_vars.insert(
                provider.env_key.clone(),
                resolve_provider_key(&profile.id, provider)?,
            );
        }
    }
    env_vars.insert("OPENCODE_CONFIG".to_string(), path.display().to_string());
    Ok(OpencodeLaunchContext {
        config_path: path.display().to_string(),
        env_vars,
        configured_model: profile.model.clone(),
        model: profile.model,
        small_model: profile.small_model,
        provider_ids: Vec::new(),
        model_source: "none".to_string(),
    })
}

fn empty_launch_context() -> OpencodeLaunchContext {
    OpencodeLaunchContext {
        config_path: String::new(),
        env_vars: BTreeMap::new(),
        configured_model: String::new(),
        model: String::new(),
        small_model: String::new(),
        provider_ids: Vec::new(),
        model_source: "none".to_string(),
    }
}

fn run_resolved_config(
    context: &OpencodeLaunchContext,
    project_path: Option<&str>,
) -> Result<Value, String> {
    let executable = cli_runtime::locate_cli(CliKind::Opencode)
        .ok_or_else(|| "未检测到 OpenCode，无法读取当前配置".to_string())?;
    let mut command = hidden_command(executable);
    command.args(["debug", "config", "--pure"]);
    let secret_values = context
        .env_vars
        .iter()
        .filter(|(key, _)| key.as_str() != "OPENCODE_CONFIG")
        .map(|(_, value)| value.clone())
        .collect::<Vec<_>>();
    for (key, value) in &context.env_vars {
        command.env(key, value);
    }
    if let Some(path) = project_path
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let directory = Path::new(path);
        if !directory.is_dir() {
            return Err(format!("OpenCode 项目目录不存在或不可访问：{path}"));
        }
        command.current_dir(directory);
    }
    let output = command
        .output()
        .map_err(|error| format!("无法执行 opencode debug config：{error}"))?;
    if !output.status.success() {
        let mut message = strip_ansi(&String::from_utf8_lossy(&output.stderr));
        for secret in secret_values {
            if !secret.is_empty() {
                message = message.replace(&secret, "***");
            }
        }
        let message: String = message.trim().chars().take(1000).collect();
        return Err(if message.is_empty() {
            format!("OpenCode 当前配置读取失败（退出状态：{}）", output.status)
        } else {
            message
        });
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("OpenCode 当前配置不是有效 JSON：{error}"))
}

fn model_ref_exists(config: &Value, model_ref: &StoredModelRef) -> bool {
    let Some(provider) = config
        .get("provider")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get(&model_ref.provider_id))
        .and_then(Value::as_object)
    else {
        return false;
    };
    match provider.get("models").and_then(Value::as_object) {
        Some(models) if !models.is_empty() => models.contains_key(&model_ref.model_id),
        _ => true,
    }
}

fn resolve_effective_model(
    config: &Value,
    recent: &[StoredModelRef],
) -> (String, String, String, Vec<String>) {
    let configured_model = config
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let small_model = config
        .get("small_model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let providers = config.get("provider").and_then(Value::as_object);
    let mut provider_ids = providers
        .map(|items| items.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    provider_ids.sort();

    if !configured_model.is_empty() {
        return (
            configured_model.clone(),
            "config".to_string(),
            small_model,
            provider_ids,
        );
    }
    if let Some(model_ref) = recent.iter().find(|item| model_ref_exists(config, item)) {
        return (
            format!("{}/{}", model_ref.provider_id, model_ref.model_id),
            "recent".to_string(),
            small_model,
            provider_ids,
        );
    }
    if let Some((provider_id, model_id)) = providers.and_then(|items| {
        items.iter().find_map(|(provider_id, provider)| {
            provider
                .get("models")
                .and_then(Value::as_object)
                .and_then(|models| models.keys().next())
                .map(|model_id| (provider_id.clone(), model_id.clone()))
        })
    }) {
        return (
            format!("{provider_id}/{model_id}"),
            "provider_default".to_string(),
            small_model,
            provider_ids,
        );
    }
    (String::new(), "none".to_string(), small_model, provider_ids)
}

fn load_recent_models() -> Vec<StoredModelRef> {
    model_state_path()
        .ok()
        .and_then(|path| fs::read(path).ok())
        .and_then(|content| serde_json::from_slice::<OpencodeModelState>(&content).ok())
        .map(|state| state.recent)
        .unwrap_or_default()
}

#[tauri::command]
pub fn resolve_opencode_profile(
    profile_id: Option<String>,
    project_path: String,
) -> Result<OpencodeLaunchContext, String> {
    let mut context = match profile_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(profile_id) => resolve_context(profile_id)?,
        None => empty_launch_context(),
    };
    let config = run_resolved_config(&context, Some(&project_path))?;
    let configured_model = config
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let (model, model_source, small_model, provider_ids) =
        resolve_effective_model(&config, &load_recent_models());
    context.configured_model = configured_model;
    context.model = model;
    context.small_model = small_model;
    context.provider_ids = provider_ids;
    context.model_source = model_source;
    Ok(context)
}

#[tauri::command]
pub fn resolve_opencode_current_config(
    project_path: String,
) -> Result<OpencodeLaunchContext, String> {
    let mut context = empty_launch_context();
    let config = run_resolved_config(&context, Some(&project_path))?;
    let configured_model = config
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let (model, model_source, small_model, provider_ids) =
        resolve_effective_model(&config, &load_recent_models());
    context.configured_model = configured_model;
    context.model = model;
    context.small_model = small_model;
    context.provider_ids = provider_ids;
    context.model_source = model_source;
    Ok(context)
}

#[tauri::command]
pub async fn fetch_opencode_provider_models(
    request: FetchOpencodeModelsRequest,
) -> Result<Vec<String>, String> {
    let provider = request.provider;
    if provider.auth_mode == OpencodeProviderAuthMode::Managed
        && !provider.base_url.trim().is_empty()
    {
        validate_url(provider.base_url.trim())?;
        let provided = request
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let stored = if provided.is_none()
            && !request.profile_id.is_empty()
            && valid_stable_id(&request.profile_id)
            && valid_stable_id(&provider.credential_id)
        {
            let path = credential_path(&request.profile_id, &provider.credential_id)?;
            path.exists()
                .then(|| fs::read(path))
                .transpose()
                .map_err(|error| format!("无法读取 OpenCode 加密凭据：{error}"))?
                .map(|encrypted| unprotect_secret(&encrypted))
                .transpose()?
        } else {
            None
        };
        let environment = if provided.is_none() && stored.is_none() && !provider.env_key.is_empty()
        {
            std::env::var(&provider.env_key).ok()
        } else {
            None
        };
        return model_fetcher::fetch_openai_compatible_models(
            provider.base_url.trim(),
            &provided.or(stored).or(environment).unwrap_or_default(),
        )
        .await;
    }

    let executable = cli_runtime::locate_cli(CliKind::Opencode)
        .ok_or_else(|| "未检测到 OpenCode，无法读取模型".to_string())?;
    let mut command = hidden_command(executable);
    command.args(["models", provider.id.as_str(), "--pure"]);
    let mut provider_content = Map::new();
    provider_content.insert("name".to_string(), Value::String(provider.name.clone()));
    if provider.provider_kind == OpencodeProviderKind::Custom {
        provider_content.insert(
            "npm".to_string(),
            Value::String(npm_package(&provider.api_type).to_string()),
        );
    }
    if !provider.base_url.is_empty() {
        provider_content.insert(
            "options".to_string(),
            json!({ "baseURL": provider.base_url }),
        );
    }
    provider_content.insert(
        "models".to_string(),
        Value::Object(
            provider
                .models
                .iter()
                .map(|model| (model.id.clone(), json!({ "name": model.name })))
                .collect(),
        ),
    );
    let inline_provider = json!({
        "provider": Value::Object(
            [(provider.id.clone(), Value::Object(provider_content))]
                .into_iter()
                .collect()
        )
    });
    command.env("OPENCODE_CONFIG_CONTENT", inline_provider.to_string());
    if !request.profile_id.is_empty() {
        if let Ok(context) = resolve_context(&request.profile_id) {
            for (key, value) in context.env_vars {
                command.env(key, value);
            }
        }
    }
    let output = command
        .output()
        .map_err(|error| format!("无法执行 opencode models：{error}"))?;
    if !output.status.success() {
        return Err(strip_ansi(&String::from_utf8_lossy(&output.stderr))
            .trim()
            .chars()
            .take(1000)
            .collect());
    }
    let prefix = format!("{}/", provider.id);
    let mut models = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().strip_prefix(&prefix))
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    if models.is_empty() {
        return Err(format!(
            "OpenCode 没有返回 provider '{}' 的模型",
            provider.id
        ));
    }
    Ok(models)
}

fn redact_value(value: &mut Value, parent_key: Option<&str>) {
    const SENSITIVE: [&str; 7] = [
        "apikey",
        "authorization",
        "token",
        "secret",
        "password",
        "credential",
        "cookie",
    ];
    fn sensitive_key(key: &str) -> bool {
        let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
        SENSITIVE.iter().any(|item| normalized.contains(item))
    }
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if sensitive_key(key) {
                    *child = Value::String("***".to_string());
                } else {
                    redact_value(child, Some(key));
                }
            }
        }
        Value::Array(values) => {
            for child in values {
                redact_value(child, parent_key);
            }
        }
        Value::String(text) if parent_key.is_some_and(sensitive_key) => {
            *text = "***".to_string();
        }
        _ => {}
    }
}

#[tauri::command]
pub fn preview_opencode_profile(
    profile_id: String,
    project_path: Option<String>,
) -> Result<Value, String> {
    let context = resolve_context(&profile_id)?;
    let mut value = run_resolved_config(&context, project_path.as_deref())?;
    redact_value(&mut value, None);
    Ok(value)
}

#[tauri::command]
pub fn preview_opencode_current_config(project_path: Option<String>) -> Result<Value, String> {
    let context = empty_launch_context();
    let mut value = run_resolved_config(&context, project_path.as_deref())?;
    redact_value(&mut value, None);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_targets_are_limited_to_opencode_directories() {
        let home = Path::new("/Users/Test User");
        assert_eq!(
            opencode_permission_directories(home),
            vec![
                home.join(".config/opencode"),
                home.join(".local/share/opencode"),
                home.join(".local/state/opencode"),
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn permission_repair_command_quotes_paths_and_never_targets_the_home_directory() {
        let home = Path::new("/Users/O'Neil Test");
        let directories = opencode_permission_directories(home);
        let command = opencode_permission_repair_command(&directories, "501:20");

        assert!(command.contains("'/Users/O'\\''Neil Test/.config/opencode'"));
        assert!(command.contains("/usr/sbin/chown -R -P 501:20"));
        assert!(!command.contains("501:20 '/Users/O'\\''Neil Test' "));
        assert!(applescript_string(&command).starts_with('"'));
        assert!(Path::new(WRITABLE_TEST_PATH).is_file());
    }

    #[test]
    fn plaintext_api_keys_are_distinguished_from_opencode_references() {
        let plaintext = json!({
            "provider": { "proxy": { "options": { "apiKey": "sk-private" } } }
        });
        let environment = json!({
            "provider": { "proxy": { "options": { "apiKey": "{env:PROXY_KEY}" } } }
        });
        let file = json!({
            "provider": { "proxy": { "options": { "apiKey": "{file:~/.secret}" } } }
        });
        assert!(has_plaintext_api_key(&plaintext));
        assert!(!has_plaintext_api_key(&environment));
        assert!(!has_plaintext_api_key(&file));
    }

    #[cfg(unix)]
    #[test]
    fn removing_plaintext_key_keeps_the_secret_backup_private() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "agents-launcher-opencode-private-{}",
            Uuid::new_v4()
        ));
        let path = root.join("opencode.jsonc");
        fs::create_dir_all(&root).expect("create temp directory");
        fs::write(
            &path,
            r#"{
                // Existing JSONC may contain comments.
                "provider": { "proxy": { "options": { "apiKey": "sk-private" } } }
            }"#,
        )
        .expect("write existing config");

        write_global_config_atomic(&path, br#"{"provider":{}}"#).expect("remove plaintext key");

        let backup = path.with_extension("jsonc.bak");
        assert_eq!(
            fs::metadata(backup)
                .expect("backup metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600,
        );
        let _ = fs::remove_dir_all(root);
    }

    fn provider(id: &str, model: &str) -> OpencodeProvider {
        OpencodeProvider {
            credential_id: format!("credential-{id}"),
            id: id.to_string(),
            name: id.to_string(),
            provider_kind: OpencodeProviderKind::Custom,
            auth_mode: OpencodeProviderAuthMode::Managed,
            api_type: OpencodeApiType::ChatCompletions,
            base_url: "https://example.com/v1".to_string(),
            env_key: format!("OPENCODE_{}_KEY", id.to_ascii_uppercase()),
            models: vec![OpencodeModel {
                id: model.to_string(),
                name: model.to_string(),
                context_limit: Some(128_000),
                output_limit: Some(8_192),
            }],
            headers: Vec::new(),
            has_stored_api_key: false,
        }
    }

    fn profile() -> OpencodeProfile {
        OpencodeProfile {
            id: "profile-test".to_string(),
            name: "Test".to_string(),
            providers: vec![provider("alpha", "model-a"), provider("beta", "model-b")],
            model: "beta/model-b".to_string(),
            small_model: "alpha/model-a".to_string(),
            managed_config_path: String::new(),
            extra: Map::new(),
        }
    }

    #[test]
    fn managed_config_supports_multiple_providers_and_cross_provider_models() {
        let rendered = build_managed_config(None, None, &profile()).expect("render");
        let value: Value = serde_json::from_str(&rendered).expect("json");
        assert_eq!(value["model"], "beta/model-b");
        assert_eq!(value["small_model"], "alpha/model-a");
        assert!(value["provider"]["alpha"].is_object());
        assert!(value["provider"]["beta"].is_object());
        assert_eq!(
            value["provider"]["alpha"]["options"]["apiKey"],
            "{env:OPENCODE_ALPHA_KEY}"
        );
        assert!(!rendered.contains("sk-secret"));
    }

    #[test]
    fn renderer_preserves_unknown_fields_and_removes_deleted_managed_models() {
        let mut previous = profile();
        previous.providers[0].models.push(OpencodeModel {
            id: "removed".to_string(),
            name: "Removed".to_string(),
            context_limit: None,
            output_limit: None,
        });
        let existing = r#"{
          // user comment is accepted on read
          "unknownTop": true,
          "provider": {
            "alpha": {
              "unknownProvider": 1,
              "models": {
                "removed": { "name": "old" },
                "manual": { "name": "keep" }
              }
            }
          }
        }"#;
        let rendered =
            build_managed_config(Some(existing), Some(&previous), &profile()).expect("render");
        let value: Value = serde_json::from_str(&rendered).expect("json");
        assert_eq!(value["unknownTop"], true);
        assert_eq!(value["provider"]["alpha"]["unknownProvider"], 1);
        assert!(value["provider"]["alpha"]["models"]["manual"].is_object());
        assert!(value["provider"]["alpha"]["models"]
            .get("removed")
            .is_none());
    }

    #[test]
    fn diagnostics_redact_provider_secrets_and_headers() {
        let mut value = json!({
            "provider": { "a": { "options": { "apiKey": "secret", "headers": { "Authorization": "Bearer secret" } } } },
            "model": "a/model",
            "keybinds": { "leader": "ctrl+x" }
        });
        redact_value(&mut value, None);
        assert_eq!(value["provider"]["a"]["options"]["apiKey"], "***");
        assert_eq!(
            value["provider"]["a"]["options"]["headers"]["Authorization"],
            "***"
        );
        assert_eq!(value["model"], "a/model");
        assert_eq!(value["keybinds"]["leader"], "ctrl+x");
    }

    #[test]
    fn jsonc_parser_accepts_comments_and_trailing_commas() {
        let value = parse_jsonc(
            r#"{
              // OpenCode supports JSONC
              "model": "alpha/model-a",
            }"#,
            "test",
        )
        .expect("parse jsonc");
        assert_eq!(value["model"], "alpha/model-a");
    }

    #[test]
    fn builtin_provider_does_not_override_the_native_sdk_package() {
        let mut profile = profile();
        profile.providers[0].provider_kind = OpencodeProviderKind::Builtin;
        let rendered = build_managed_config(None, None, &profile).expect("render");
        let value: Value = serde_json::from_str(&rendered).expect("json");
        assert!(value["provider"]["alpha"].get("npm").is_none());
        assert_eq!(
            value["provider"]["beta"]["npm"],
            "@ai-sdk/openai-compatible"
        );
    }

    #[test]
    fn launch_preflight_prefers_the_merged_config_model() {
        let config = json!({
            "model": "beta/model-b",
            "small_model": "alpha/model-a",
            "provider": {
                "alpha": { "models": { "model-a": {} } },
                "beta": { "models": { "model-b": {} } }
            }
        });
        let recent = vec![StoredModelRef {
            provider_id: "alpha".to_string(),
            model_id: "model-a".to_string(),
        }];
        let (model, source, small_model, provider_ids) = resolve_effective_model(&config, &recent);
        assert_eq!(model, "beta/model-b");
        assert_eq!(source, "config");
        assert_eq!(small_model, "alpha/model-a");
        assert_eq!(provider_ids, ["alpha", "beta"]);
    }

    #[test]
    fn launch_preflight_uses_recent_model_when_config_has_no_model() {
        let config = json!({
            "provider": {
                "alpha": { "models": { "model-a": {}, "model-a2": {} } }
            }
        });
        let recent = vec![StoredModelRef {
            provider_id: "alpha".to_string(),
            model_id: "model-a2".to_string(),
        }];
        let (model, source, _, _) = resolve_effective_model(&config, &recent);
        assert_eq!(model, "alpha/model-a2");
        assert_eq!(source, "recent");
    }

    #[test]
    fn launch_preflight_falls_back_to_a_configured_provider_model() {
        let config = json!({
            "provider": {
                "alpha": { "models": { "model-a": {} } }
            }
        });
        let (model, source, _, _) = resolve_effective_model(&config, &[]);
        assert_eq!(model, "alpha/model-a");
        assert_eq!(source, "provider_default");
    }

    #[test]
    fn global_sync_only_edits_custom_providers_and_preserves_hidden_fields() {
        let existing = json!({
            "$schema": SCHEMA_URL,
            "shell": "powershell",
            "disabled_providers": ["example"],
            "provider": {
                "native": {
                    "name": "OpenCode native override",
                    "options": { "timeout": 1234 }
                },
                "custom": {
                    "name": "Custom",
                    "npm": "@ai-sdk/openai-compatible",
                    "options": { "baseURL": "https://old.example/v1", "timeout": 9000 },
                    "models": {
                        "vision": {
                            "name": "Vision",
                            "limit": { "context": 200000, "output": 65536 },
                            "modalities": { "input": ["text", "image"], "output": ["text"] }
                        }
                    }
                }
            }
        });
        let mut payload =
            payload_from_global_document(Path::new("opencode.jsonc"), "original", &existing)
                .expect("read payload");
        assert_eq!(payload.providers.len(), 1);
        assert_eq!(payload.providers[0].id, "custom");
        assert!(payload.providers[0].models[0].input_text);
        assert!(payload.providers[0].models[0].input_image);
        assert_eq!(payload.providers[0].models[0].context_limit, Some(200000));
        assert_eq!(payload.providers[0].models[0].output_limit, Some(65536));

        payload.providers[0].base_url = "https://new.example/v1".to_string();
        payload.providers[0].models[0].input_text = false;
        payload.providers[0].models[0].context_limit = Some(262144);
        payload.providers[0].models[0].output_limit = Some(32768);
        let rendered = build_global_config(existing, &payload).expect("build global config");
        let value: Value = serde_json::from_str(&rendered).expect("strict JSON output");

        assert_eq!(value["shell"], "powershell");
        assert_eq!(value["disabled_providers"][0], "example");
        assert_eq!(value["provider"]["native"]["options"]["timeout"], 1234);
        assert_eq!(
            value["provider"]["custom"]["options"]["baseURL"],
            "https://new.example/v1"
        );
        assert_eq!(value["provider"]["custom"]["options"]["timeout"], 9000);
        assert_eq!(
            value["provider"]["custom"]["models"]["vision"]["limit"]["context"],
            262144
        );
        assert_eq!(
            value["provider"]["custom"]["models"]["vision"]["limit"]["output"],
            32768
        );
        assert_eq!(
            value["provider"]["custom"]["models"]["vision"]["modalities"]["input"],
            json!(["image"])
        );
        assert_eq!(
            value["provider"]["custom"]["models"]["vision"]["modalities"]["output"],
            json!(["text"])
        );
    }

    #[test]
    fn writing_one_global_provider_preserves_other_providers() {
        let existing = json!({
            "$schema": SCHEMA_URL,
            "shell": "powershell",
            "provider": {
                "first": {
                    "name": "First",
                    "npm": "@ai-sdk/openai-compatible",
                    "options": { "baseURL": "https://first.example/v1" }
                },
                "native": {
                    "name": "Built in override",
                    "options": { "timeout": 1234 }
                }
            }
        });
        let provider = OpencodeGlobalProvider {
            original_id: String::new(),
            id: "second".to_string(),
            name: "Second".to_string(),
            npm: "@ai-sdk/openai-compatible".to_string(),
            base_url: "https://second.example/v1".to_string(),
            api_key: String::new(),
            models: Vec::new(),
        };

        let rendered = build_global_provider_update(existing, &provider).expect("write provider");
        let value: Value = serde_json::from_str(&rendered).expect("strict JSON output");

        assert_eq!(
            value["provider"]["first"]["options"]["baseURL"],
            "https://first.example/v1"
        );
        assert_eq!(
            value["provider"]["second"]["options"]["baseURL"],
            "https://second.example/v1"
        );
        assert_eq!(value["provider"]["native"]["options"]["timeout"], 1234);
        assert_eq!(value["shell"], "powershell");
    }

    #[test]
    fn deleting_one_global_provider_preserves_the_others() {
        let existing = json!({
            "$schema": SCHEMA_URL,
            "disabled_providers": ["first", "second"],
            "provider": {
                "first": {
                    "npm": "@ai-sdk/openai-compatible",
                    "options": { "baseURL": "https://first.example/v1" }
                },
                "second": {
                    "npm": "@ai-sdk/openai-compatible",
                    "options": { "baseURL": "https://second.example/v1" }
                }
            }
        });

        let rendered = build_global_provider_delete(existing, "first").expect("delete provider");
        let value: Value = serde_json::from_str(&rendered).expect("strict JSON output");

        assert!(value["provider"].get("first").is_none());
        assert_eq!(
            value["provider"]["second"]["options"]["baseURL"],
            "https://second.example/v1"
        );
        assert_eq!(value["disabled_providers"], json!(["second"]));
    }

    #[test]
    fn saving_global_options_does_not_rewrite_providers() {
        let existing = json!({
            "$schema": SCHEMA_URL,
            "model": "first/old-model",
            "provider": {
                "first": {
                    "npm": "@ai-sdk/openai-compatible",
                    "options": {
                        "baseURL": "https://first.example/v1",
                        "timeout": 9000
                    }
                }
            }
        });

        let rendered = build_global_options_update(existing, "first/new-model", "")
            .expect("save global options");
        let value: Value = serde_json::from_str(&rendered).expect("strict JSON output");

        assert_eq!(value["model"], "first/new-model");
        assert!(value.get("small_model").is_none());
        assert_eq!(value["provider"]["first"]["options"]["timeout"], 9000);
    }

    #[test]
    fn key_updates_and_provider_toggles_preserve_credentials() {
        let mut auth = json!({
            "builtin-oauth": {
                "type": "oauth",
                "access": "oauth-access",
                "refresh": "oauth-refresh"
            },
            "custom": {
                "type": "api",
                "key": "old-key"
            }
        });

        set_api_credential(&mut auth, "custom", "new-key").expect("update custom key");
        assert_eq!(auth["custom"]["type"], "api");
        assert_eq!(auth["custom"]["key"], "new-key");
        assert_eq!(auth["builtin-oauth"]["access"], "oauth-access");
        assert!(set_api_credential(&mut auth, "builtin-oauth", "bad").is_err());

        assert_eq!(auth["builtin-oauth"]["refresh"], "oauth-refresh");

        let mut config = json!({
            "disabled_providers": ["llama-cpp", "keep-disabled"],
            "provider": { "llama-cpp": { "npm": "@ai-sdk/openai-compatible" } }
        });
        set_provider_disabled(&mut config, "llama-cpp", false).expect("enable provider");
        assert_eq!(config["disabled_providers"], json!(["keep-disabled"]));
        set_provider_disabled(&mut config, "llama-cpp", true).expect("disable provider");
        assert_eq!(
            config["disabled_providers"],
            json!(["keep-disabled", "llama-cpp"])
        );
        assert_eq!(auth["custom"]["key"], "new-key");
        assert_eq!(auth["builtin-oauth"]["refresh"], "oauth-refresh");

        let status = connection_status_from_document(Path::new("auth.json"), "raw", &auth);
        assert_eq!(
            status.connection_keys.get("custom"),
            Some(&"new-key".to_string())
        );
        assert!(!status.connection_keys.contains_key("builtin-oauth"));
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_secret_round_trip_does_not_store_plaintext() {
        let secret = "opencode-test-secret";
        let encrypted = protect_secret(secret).expect("protect");
        assert!(!encrypted
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()));
        assert_eq!(unprotect_secret(&encrypted).expect("unprotect"), secret);
    }
}
