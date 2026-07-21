use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use toml_edit::{value, DocumentMut, Item, Table};
#[cfg(target_os = "macos")]
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
const MANAGED_PROFILE_PREFIX: &str = "cc-launcher-";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodexAuthMode {
    Official,
    Custom,
}

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
    #[serde(default = "default_env_key")]
    pub env_key: String,
    #[serde(default)]
    pub has_stored_api_key: bool,
    #[serde(default)]
    pub managed_profile_name: String,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

fn default_wire_api() -> String {
    "responses".to_string()
}

fn default_env_key() -> String {
    "OPENAI_API_KEY".to_string()
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
    #[serde(default, flatten)]
    extra: Map<String, Value>,
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
    pub profiles_path: String,
    pub global_config_path: String,
    pub auth_path: String,
    pub global_config_error: Option<String>,
    pub auth_status: CodexAuthStatus,
    pub custom_global_sync_supported: bool,
    pub custom_global_key_sync_supported: bool,
    pub secret_storage_kind: &'static str,
    pub platform: &'static str,
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
    pub env_vars: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyCodexProfileRequest {
    pub profile_id: String,
    #[serde(default)]
    pub apply_to_global: bool,
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

fn app_data_dir() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|path| path.join("ClaudeEnvManager"))
        .ok_or_else(|| "无法确定 %APPDATA% 目录".to_string())
}

fn codex_data_dir() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("codex"))
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

fn codex_home() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("CODEX_HOME").map(PathBuf::from) {
        if !path.as_os_str().is_empty() {
            return Ok(path);
        }
    }
    dirs::home_dir()
        .map(|path| path.join(".codex"))
        .ok_or_else(|| "无法确定 CODEX_HOME".to_string())
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

fn managed_profile_path(profile_id: &str) -> Result<PathBuf, String> {
    Ok(codex_home()?.join(format!("{}.config.toml", managed_profile_name(profile_id))))
}

#[cfg(windows)]
fn credential_path(profile_id: &str) -> Result<PathBuf, String> {
    Ok(credentials_dir()?.join(format!("{profile_id}.bin")))
}

fn custom_global_sync_supported() -> bool {
    cfg!(any(windows, target_os = "macos"))
}

fn custom_global_key_sync_supported() -> bool {
    cfg!(windows)
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
    profile.managed_profile_name = managed_profile_name(&profile.id);
    profile.model = profile.model.trim().to_string();
    profile.reasoning_effort = profile.reasoning_effort.trim().to_string();
    profile.openai_base_url = profile.openai_base_url.trim().to_string();
    profile.provider_id = profile.provider_id.trim().to_string();
    profile.provider_name = profile.provider_name.trim().to_string();
    profile.base_url = profile.base_url.trim().to_string();
    profile.env_key = profile.env_key.trim().to_string();
    profile.wire_api = profile.wire_api.trim().to_string();

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

fn set_optional_string(document: &mut DocumentMut, key: &str, value_text: &str) {
    if value_text.is_empty() {
        document.as_table_mut().remove(key);
    } else {
        document[key] = value(value_text);
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

fn uses_plaintext_command_auth(profile: &CodexProfile) -> bool {
    cfg!(target_os = "macos")
        && profile.auth_mode == CodexAuthMode::Custom
        && profile.has_stored_api_key
}

fn configure_provider_credentials(
    provider: &mut Table,
    profile: &CodexProfile,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if uses_plaintext_command_auth(profile) {
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

    provider["env_key"] = value(&profile.env_key);
    provider.remove("auth");
    Ok(())
}

fn build_profile_toml(
    existing: Option<&str>,
    previous_managed_provider_id: Option<&str>,
    profile: &CodexProfile,
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

    if let Some(previous_provider_id) = previous_managed_provider_id {
        if profile.auth_mode != CodexAuthMode::Custom || previous_provider_id != profile.provider_id
        {
            remove_provider(&mut document, previous_provider_id);
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

fn build_global_toml(
    existing: Option<&str>,
    previous_managed_provider_id: Option<&str>,
    profile: &CodexProfile,
) -> Result<String, String> {
    build_profile_toml(existing, previous_managed_provider_id, profile)
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
        },
        None => remove_if_exists(path),
    }
}

#[derive(Clone, Copy)]
enum SnapshotKind {
    Json,
    Toml,
    Credential,
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
        let path = global_env_path()?;
        if !path.exists() {
            return Ok(None);
        }
        let encrypted =
            fs::read(&path).map_err(|error| format!("无法读取 CodeX 全局环境变量记录：{error}"))?;
        let json = unprotect_secret(&encrypted)?;
        serde_json::from_str(&json)
            .map(Some)
            .map_err(|error| format!("CodeX 全局环境变量记录无法解析：{error}"))
    }
}

#[cfg(windows)]
fn save_managed_global_env(record: &ManagedGlobalEnv) -> Result<(), String> {
    let json = serde_json::to_string(record)
        .map_err(|error| format!("无法序列化 CodeX 全局环境变量记录：{error}"))?;
    let encrypted = protect_secret(&json)?;
    write_credential_atomic(&global_env_path()?, &encrypted)
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
    #[cfg(not(windows))]
    {
        let _ = previous;
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
                save_managed_global_env(&ManagedGlobalEnv {
                    key: next_key.to_string(),
                    applied_value: next_value.to_string(),
                    previous_value,
                })?;
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
                    save_managed_global_env(&ManagedGlobalEnv {
                        key: next_key.to_string(),
                        applied_value: next_value.to_string(),
                        previous_value,
                    })?;
                } else {
                    remove_if_exists(&global_env_path()?)?;
                    remove_transaction_sidecars(&global_env_path()?)?;
                }
            }
        }
        Ok(())
    }
}

fn resolve_profile_api_key(profile: &CodexProfile) -> Result<String, String> {
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

fn enrich_profiles(state: &mut CodexProfileState) -> Result<(), String> {
    for profile in &mut state.profiles {
        profile.managed_profile_name = managed_profile_name(&profile.id);
        profile.has_stored_api_key = profile_secret_exists(&profile.id)?;
        let profile_path = managed_profile_path(&profile.id)?;
        if profile_path.exists() {
            let raw = fs::read_to_string(&profile_path).map_err(|error| {
                format!("无法读取 CodeX profile {}：{error}", profile_path.display())
            })?;
            DocumentMut::from_str(&raw).map_err(|error| {
                format!("CodeX profile {} 无法解析：{error}", profile_path.display())
            })?;
        }
    }
    Ok(())
}

fn global_profile_matches_document(state: &CodexProfileState, raw: &str) -> bool {
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
    build_global_toml(
        Some(raw),
        state.managed_global_provider_id.as_deref(),
        profile,
    )
    .is_ok_and(|expected| expected == raw)
}

fn global_profile_in_sync(state: &CodexProfileState) -> Result<bool, String> {
    if state.global_profile_id.is_none() {
        return Ok(false);
    }
    let path = global_config_path()?;
    if !path.exists() {
        return Ok(false);
    }
    let raw =
        fs::read_to_string(&path).map_err(|error| format!("无法读取全局 config.toml：{error}"))?;
    Ok(global_profile_matches_document(state, &raw))
}

fn load_payload() -> Result<CodexProfilesPayload, String> {
    let mut state = load_profile_state()?;
    enrich_profiles(&mut state)?;
    let global_profile_in_sync = global_profile_in_sync(&state)?;
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
        profiles_path: profiles_path()?.display().to_string(),
        global_config_path: global_config_path()?.display().to_string(),
        auth_path: auth_path()?.display().to_string(),
        global_config_error: global_config_error()?,
        auth_status: auth_status()?,
        custom_global_sync_supported: custom_global_sync_supported(),
        custom_global_key_sync_supported: custom_global_key_sync_supported(),
        secret_storage_kind: secret_storage_kind(),
        platform: platform_name(),
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
    let metadata_path = profiles_path()?;
    let profile_path = managed_profile_path(&profile.id)?;
    let mut state = load_profile_state()?;
    let previous_profile = state
        .profiles
        .iter()
        .find(|item| item.id == profile.id)
        .cloned();
    if state
        .profiles
        .iter()
        .any(|item| item.id != profile.id && item.name == profile.name)
    {
        return Err(format!("CodeX 配置名称 '{}' 已存在", profile.name));
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
    let rendered = build_profile_toml(
        existing_toml.as_deref(),
        previous_managed_provider_id,
        &stored_profile,
    )?;

    let previous_metadata = fs::read(&metadata_path).ok();
    let previous_toml = fs::read(&profile_path).ok();
    let previous_index = load_profile_index_state(CODEX_STATE_KEY)?;

    let transaction = (|| {
        write_toml_atomic(&profile_path, rendered.as_bytes())?;

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
    if request.apply_to_global && !custom_global_sync_supported() {
        return Err("当前平台不支持同步 CodeX 全局配置".to_string());
    }
    let profile_path = managed_profile_path(&profile.id)?;
    let existing_profile_toml = fs::read_to_string(&profile_path)
        .map_err(|error| format!("无法读取 CodeX profile：{error}"))?;
    let rendered_profile_toml = build_profile_toml(
        Some(&existing_profile_toml),
        managed_provider_id(&profile),
        &profile,
    )?;
    let profile_needs_update = rendered_profile_toml != existing_profile_toml;
    let previous_index = load_profile_index_state(CODEX_STATE_KEY)?;
    let previous_metadata = fs::read(profiles_path()?).ok();
    let global_path = global_config_path()?;
    let previous_global = fs::read(&global_path).ok();
    let global_env_record_path = global_env_path()?;
    let previous_global_env_file = fs::read(&global_env_record_path).ok();
    let previous_managed_env = load_managed_global_env()?;

    let rendered_global = if request.apply_to_global {
        let existing = if global_path.exists() {
            Some(
                fs::read_to_string(&global_path)
                    .map_err(|error| format!("无法读取全局 config.toml：{error}"))?,
            )
        } else {
            None
        };
        Some(build_global_toml(
            existing.as_deref(),
            state.managed_global_provider_id.as_deref(),
            &profile,
        )?)
    } else {
        None
    };
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
    if next_api_key.is_some() {
        env_keys.insert(profile.env_key.clone());
    }
    let env_snapshots = env_keys
        .into_iter()
        .map(|key| read_user_env_var(&key).map(|value| (key, value)))
        .collect::<Result<HashMap<_, _>, _>>()?;

    let transaction = (|| {
        if profile_needs_update {
            write_toml_atomic(&profile_path, rendered_profile_toml.as_bytes())?;
        }
        if let Some(rendered) = rendered_global.as_ref() {
            write_toml_atomic(&global_path, rendered.as_bytes())?;
            let next_env = next_api_key
                .as_deref()
                .map(|api_key| (profile.env_key.as_str(), api_key));
            transition_managed_global_env(next_env, previous_managed_env.as_ref())?;
            state.global_profile_id = Some(profile.id.clone());
            state.managed_global_provider_id = managed_provider_id(&profile).map(str::to_string);
            save_profile_state(&state)?;
        }

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
        if request.apply_to_global {
            if let Err(rollback) = restore_snapshot(
                &profiles_path()?,
                previous_metadata.as_deref(),
                SnapshotKind::Json,
            ) {
                rollback_errors.push(rollback);
            }
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
            if let Err(rollback) = restore_user_env_snapshots(&env_snapshots) {
                rollback_errors.push(rollback);
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

    load_payload()
}

#[tauri::command]
pub fn delete_codex_profile(
    request: DeleteCodexProfileRequest,
) -> Result<CodexProfilesPayload, String> {
    let metadata_path = profiles_path()?;
    let profile_path = managed_profile_path(&request.profile_id)?;
    let mut state = load_profile_state()?;
    if !state
        .profiles
        .iter()
        .any(|profile| profile.id == request.profile_id)
    {
        return Err("要删除的 CodeX 配置不存在".to_string());
    }

    let previous_metadata = fs::read(&metadata_path).ok();
    let previous_toml = fs::read(&profile_path).ok();
    let previous_secret = read_profile_secret(&request.profile_id)?;
    let previous_index = load_profile_index_state(CODEX_STATE_KEY)?;
    state
        .profiles
        .retain(|profile| profile.id != request.profile_id);
    if state.global_profile_id.as_deref() == Some(request.profile_id.as_str()) {
        state.global_profile_id = None;
    }

    let transaction = (|| {
        remove_if_exists(&profile_path)?;
        remove_transaction_sidecars(&profile_path)?;
        delete_profile_secret(&request.profile_id)?;
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
        if rollback_errors.is_empty() {
            return Err(format!("删除 CodeX 配置失败，旧数据已恢复：{error}"));
        }
        return Err(format!(
            "删除 CodeX 配置失败且回滚不完整：{error}；{}",
            rollback_errors.join("；")
        ));
    }

    load_payload()
}

#[tauri::command]
pub fn resolve_codex_profile(profile_id: String) -> Result<CodexLaunchContext, String> {
    let mut state = load_profile_state()?;
    enrich_profiles(&mut state)?;
    let profile = state
        .profiles
        .into_iter()
        .find(|profile| profile.id == profile_id)
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
    let rendered_toml = build_profile_toml(
        Some(&existing_toml),
        managed_provider_id(&profile),
        &profile,
    )?;
    if rendered_toml != existing_toml {
        write_toml_atomic(&profile_path, rendered_toml.as_bytes())?;
    }
    let mut env_vars = BTreeMap::new();
    if profile.auth_mode == CodexAuthMode::Custom && !uses_plaintext_command_auth(&profile) {
        let api_key = resolve_profile_api_key(&profile)?;
        env_vars.insert(profile.env_key.clone(), api_key);
    }
    Ok(CodexLaunchContext {
        managed_profile_name: profile.managed_profile_name,
        env_vars,
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
            openai_base_url: String::new(),
            provider_id: String::new(),
            provider_name: String::new(),
            base_url: String::new(),
            wire_api: default_wire_api(),
            env_key: default_env_key(),
            has_stored_api_key: false,
            managed_profile_name: String::new(),
            extra: Map::new(),
        }
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

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_reports_plaintext_storage_and_disables_user_env_sync() {
        assert_eq!(secret_storage_kind(), "macos_plaintext");
        assert!(custom_global_sync_supported());
        assert!(!custom_global_key_sync_supported());
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
        let provider = document["model_providers"]["company_proxy"]
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
        assert!(arguments[3].ends_with("ClaudeEnvManager/codex/credentials.json"));
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
    fn custom_profile_uses_env_key_without_serializing_the_secret() {
        let mut profile = official_profile();
        profile.auth_mode = CodexAuthMode::Custom;
        profile.provider_id = "company_proxy".to_string();
        profile.provider_name = "Company Proxy".to_string();
        profile.base_url = "https://proxy.example.com/v1".to_string();
        profile.env_key = "COMPANY_CODEX_KEY".to_string();
        let profile = normalize_profile(profile).expect("valid profile");
        let rendered = build_profile_toml(None, None, &profile).expect("render");
        assert!(rendered.contains("env_key = \"COMPANY_CODEX_KEY\""));
        assert!(!rendered.contains("sk-test-secret"));
        assert_eq!(
            DocumentMut::from_str(&rendered).expect("parse")["model_providers"]["company_proxy"]
                ["wire_api"]
                .as_str(),
            Some("responses")
        );
    }

    #[test]
    fn global_sync_replaces_only_the_previous_managed_provider() {
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
        profile.provider_id = "new_proxy".to_string();
        profile.provider_name = "New Proxy".to_string();
        profile.base_url = "https://new.example.com/v1".to_string();
        let profile = normalize_profile(profile).expect("valid profile");

        let rendered = build_global_toml(Some(existing), Some("old_proxy"), &profile)
            .expect("render global config");
        let document = DocumentMut::from_str(&rendered).expect("parse");
        assert_eq!(document["approval_policy"].as_str(), Some("on-request"));
        assert_eq!(document["features"]["js_repl"].as_bool(), Some(true));
        assert!(document["model_providers"].get("old_proxy").is_none());
        assert_eq!(
            document["model_providers"]["new_proxy"]["base_url"].as_str(),
            Some("https://new.example.com/v1")
        );
    }

    #[test]
    fn global_sync_status_detects_saved_profile_changes() {
        let profile = official_profile();
        let rendered = build_global_toml(None, None, &profile).expect("render global config");
        let mut state = CodexProfileState::default();
        state.global_profile_id = Some(profile.id.clone());
        state.profiles.push(profile);

        assert!(global_profile_matches_document(&state, &rendered));

        state.profiles[0].model = "gpt-updated".to_string();
        assert!(
            !global_profile_matches_document(&state, &rendered),
            "saving the same profile with new values must require a global re-sync",
        );
    }

    #[test]
    fn official_global_sync_removes_the_previous_managed_provider() {
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
        assert!(document.get("model_providers").is_none());
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
