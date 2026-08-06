//! settings_manager.rs
//!
//! Reads and writes {home}/.claude/settings.json.
//!
//! Managed fields:
//!   - `skipDangerousModePermissionPrompt`: bool
//!   - `permissions.defaultMode`: "bypassPermissions" | "default"
//!   - `awaySummaryEnabled`: bool
//!   - `env`: { "VAR_NAME": "value", ... }
//!
//! All other fields in the file are preserved on write.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::file_transaction::{restore_json_backup_if_missing, write_json_atomic};

const DEFAULT_PERMISSION_MODE: &str = "auto";

fn normalize_permission_mode(mode: &str) -> Option<&'static str> {
    match mode.trim() {
        "bypassPermissions" => Some("bypassPermissions"),
        "auto" => Some("auto"),
        "default" | "manual" => Some("default"),
        "acceptEdits" => Some("acceptEdits"),
        "plan" => Some("plan"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Path helper
// ---------------------------------------------------------------------------

struct SettingsPaths {
    canonical: PathBuf,
    legacy: [PathBuf; 2],
}

fn settings_paths() -> Result<SettingsPaths, String> {
    let directory = dirs::home_dir()
        .map(|home| home.join(".claude"))
        .ok_or_else(|| "Could not determine home directory".to_string())?;
    Ok(SettingsPaths {
        canonical: directory.join("settings.json"),
        legacy: [directory.join("claude.json"), directory.join("config.json")],
    })
}

fn existing_settings_source(paths: &SettingsPaths) -> Option<(PathBuf, bool)> {
    if paths.canonical.exists() {
        return Some((paths.canonical.clone(), false));
    }
    paths
        .legacy
        .iter()
        .find(|path| path.exists())
        .cloned()
        .map(|path| (path, true))
}

fn read_settings_object(path: &PathBuf) -> Result<Map<String, Value>, String> {
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("无法读取 Claude Code 配置 {}：{error}", path.display()))?;
    let value: Value = serde_json::from_str(&raw).map_err(|error| {
        format!(
            "Claude Code 配置 JSON 无法解析（{}）：{error}",
            path.display()
        )
    })?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| format!("Claude Code 配置根节点必须是对象：{}", path.display()))
}

// ---------------------------------------------------------------------------
// Public data types
// ---------------------------------------------------------------------------

/// The subset of settings.json that this module manages.
/// Returned to / received from the frontend.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSettings {
    /// True when "跳过权限检查" is enabled.
    pub skip_permissions: bool,
    /// True when "关闭 away summary" checkbox is checked
    /// (i.e. `awaySummaryEnabled` is `false` in the file).
    pub away_summary_disabled: bool,
    /// Actual source selected during compatible reading.
    #[serde(default)]
    pub source_path: String,
    /// `settings`, `legacy`, or `missing`.
    #[serde(default)]
    pub source_kind: String,
    /// True when a historical `claude.json` / `config.json` supplied the values.
    #[serde(default)]
    pub using_legacy_path: bool,
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Read the managed fields from settings.json.
/// Returns defaults (both false) when the file does not exist or cannot be parsed.
#[tauri::command]
pub fn load_claude_settings() -> Result<ClaudeSettings, String> {
    load_claude_settings_from(&settings_paths()?)
}

fn load_claude_settings_from(paths: &SettingsPaths) -> Result<ClaudeSettings, String> {
    restore_json_backup_if_missing(&paths.canonical, "Claude Code settings.json")?;
    let Some((source_path, using_legacy_path)) = existing_settings_source(paths) else {
        return Ok(ClaudeSettings {
            skip_permissions: false,
            away_summary_disabled: false,
            source_path: paths.canonical.display().to_string(),
            source_kind: "missing".to_string(),
            using_legacy_path: false,
        });
    };

    let settings = Value::Object(read_settings_object(&source_path)?);

    // --- skip_permissions ---
    // True if permissions.defaultMode == "bypassPermissions"
    // OR if skipDangerousModePermissionPrompt is true.
    let skip_permissions = {
        let via_mode = settings
            .get("permissions")
            .and_then(|p| p.get("defaultMode"))
            .and_then(|m| m.as_str())
            == Some("bypassPermissions");

        let via_flag = settings
            .get("skipDangerousModePermissionPrompt")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        via_mode || via_flag
    };

    // --- away_summary_disabled ---
    // The checkbox "关闭 away summary" is checked when awaySummaryEnabled is false.
    let away_summary_disabled =
        settings.get("awaySummaryEnabled").and_then(|v| v.as_bool()) == Some(false);

    Ok(ClaudeSettings {
        skip_permissions,
        away_summary_disabled,
        source_path: source_path.display().to_string(),
        source_kind: if using_legacy_path {
            "legacy"
        } else {
            "settings"
        }
        .to_string(),
        using_legacy_path,
    })
}

/// Write the managed fields back to settings.json, preserving all other fields.
#[tauri::command]
pub fn save_claude_settings(settings: ClaudeSettings) -> Result<(), String> {
    save_claude_settings_to(&settings_paths()?, &settings)
}

fn save_claude_settings_to(paths: &SettingsPaths, settings: &ClaudeSettings) -> Result<(), String> {
    restore_json_backup_if_missing(&paths.canonical, "Claude Code settings.json")?;
    // A malformed canonical or legacy source is a hard failure. Never turn a
    // parse error into an empty object that can overwrite the user's file.
    let mut obj = match existing_settings_source(paths) {
        Some((source_path, _)) => read_settings_object(&source_path)?,
        None => Map::new(),
    };

    // --- skipDangerousModePermissionPrompt ---
    obj.insert(
        "skipDangerousModePermissionPrompt".to_string(),
        Value::Bool(settings.skip_permissions),
    );

    // --- permissions.defaultMode ---
    let mode = if settings.skip_permissions {
        "bypassPermissions"
    } else {
        "default"
    };
    let permissions = obj
        .entry("permissions")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| "permissions field is not an object".to_string())?;
    permissions.insert("defaultMode".to_string(), Value::String(mode.to_string()));

    // --- awaySummaryEnabled ---
    // Checkbox checked  → away_summary_disabled = true  → awaySummaryEnabled = false
    // Checkbox unchecked → away_summary_disabled = false → awaySummaryEnabled = true
    obj.insert(
        "awaySummaryEnabled".to_string(),
        Value::Bool(!settings.away_summary_disabled),
    );

    // Ensure parent directory exists.
    if let Some(parent) = paths.canonical.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create .claude directory: {e}"))?;
    }

    let json = serde_json::to_string_pretty(&Value::Object(obj))
        .map_err(|e| format!("Failed to serialise settings: {e}"))?;

    write_json_atomic(
        &paths.canonical,
        json.as_bytes(),
        "Claude Code settings.json",
    )
}

/// Read the permission mode used before a Claude session is started.
/// Missing or unsupported values follow Claude Code's auto mode default.
#[tauri::command]
pub fn load_claude_default_permission_mode() -> Result<String, String> {
    load_claude_default_permission_mode_from(&settings_paths()?)
}

fn load_claude_default_permission_mode_from(paths: &SettingsPaths) -> Result<String, String> {
    restore_json_backup_if_missing(&paths.canonical, "Claude Code settings.json")?;
    let Some((source_path, _)) = existing_settings_source(paths) else {
        return Ok(DEFAULT_PERMISSION_MODE.to_string());
    };
    let settings = read_settings_object(&source_path)?;
    Ok(settings
        .get("permissions")
        .and_then(|permissions| permissions.get("defaultMode"))
        .and_then(Value::as_str)
        .and_then(normalize_permission_mode)
        .unwrap_or(DEFAULT_PERMISSION_MODE)
        .to_string())
}

/// Persist the pre-session Shift+Tab selection without changing unrelated settings.
#[tauri::command]
pub fn save_claude_default_permission_mode(mode: String) -> Result<(), String> {
    save_claude_default_permission_mode_to(&settings_paths()?, &mode)
}

fn save_claude_default_permission_mode_to(paths: &SettingsPaths, mode: &str) -> Result<(), String> {
    let mode = normalize_permission_mode(mode)
        .ok_or_else(|| format!("Unsupported Claude permission mode: {mode}"))?;
    restore_json_backup_if_missing(&paths.canonical, "Claude Code settings.json")?;
    let mut settings = match existing_settings_source(paths) {
        Some((source_path, _)) => read_settings_object(&source_path)?,
        None => Map::new(),
    };
    let permissions = settings
        .entry("permissions")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| "permissions field is not an object".to_string())?;
    permissions.insert("defaultMode".to_string(), Value::String(mode.to_string()));

    if let Some(parent) = paths.canonical.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create .claude directory: {error}"))?;
    }
    let json = serde_json::to_string_pretty(&Value::Object(settings))
        .map_err(|error| format!("Failed to serialise settings: {error}"))?;
    write_json_atomic(
        &paths.canonical,
        json.as_bytes(),
        "Claude Code settings.json",
    )
}

/// 读取 settings.json 中的 env 字段（忽略空值）。
#[tauri::command]
pub fn load_claude_env() -> Result<HashMap<String, String>, String> {
    let paths = settings_paths()?;
    let path = &paths.canonical;
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("读取 settings.json 失败: {}", e))?;
    let json: Value = serde_json::from_str(&raw).unwrap_or(Value::Object(Map::new()));
    let mut result = HashMap::new();
    if let Some(env_obj) = json.get("env").and_then(|v| v.as_object()) {
        for (k, v) in env_obj {
            if let Some(s) = v.as_str() {
                if !s.is_empty() {
                    result.insert(k.clone(), s.to_string());
                }
            }
        }
    }
    Ok(result)
}

/// 写入 settings.json 中的 env 字段（覆盖整个 env 对象，保留其他字段）。
/// 空值会被过滤掉，避免覆盖 shell 中已设置的环境变量。
#[tauri::command]
pub fn save_claude_env(env: HashMap<String, String>) -> Result<(), String> {
    let paths = settings_paths()?;
    let path = &paths.canonical;
    let mut obj: Map<String, Value> = if path.exists() {
        let raw =
            std::fs::read_to_string(path).map_err(|e| format!("读取 settings.json 失败: {}", e))?;
        serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    } else {
        Map::new()
    };
    let env_map: Map<String, Value> = env
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k, Value::String(v)))
        .collect();
    obj.insert("env".to_string(), Value::Object(env_map));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
    }
    let json = serde_json::to_string_pretty(&Value::Object(obj))
        .map_err(|e| format!("序列化 settings.json 失败: {}", e))?;
    std::fs::write(path, json.as_bytes()).map_err(|e| format!("写入 settings.json 失败: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_paths() -> (PathBuf, SettingsPaths) {
        let directory = std::env::temp_dir().join(format!("agents-launcher-{}", Uuid::new_v4()));
        let paths = SettingsPaths {
            canonical: directory.join("settings.json"),
            legacy: [directory.join("claude.json"), directory.join("config.json")],
        };
        (directory, paths)
    }

    fn settings(skip_permissions: bool) -> ClaudeSettings {
        ClaudeSettings {
            skip_permissions,
            away_summary_disabled: false,
            source_path: String::new(),
            source_kind: String::new(),
            using_legacy_path: false,
        }
    }

    #[test]
    fn malformed_settings_are_never_replaced() {
        let (directory, paths) = temp_paths();
        fs::create_dir_all(&directory).expect("create temp directory");
        fs::write(&paths.canonical, "{broken").expect("write malformed settings");

        let error = save_claude_settings_to(&paths, &settings(true))
            .expect_err("malformed source must fail");

        assert!(error.contains("无法解析"));
        assert_eq!(
            fs::read_to_string(&paths.canonical).expect("read source"),
            "{broken"
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn legacy_source_is_read_and_unknown_fields_move_to_canonical_file() {
        let (directory, paths) = temp_paths();
        fs::create_dir_all(&directory).expect("create temp directory");
        fs::write(
            &paths.legacy[0],
            br#"{"futureSetting":{"keep":true},"awaySummaryEnabled":false}"#,
        )
        .expect("write legacy settings");

        let loaded = load_claude_settings_from(&paths).expect("load legacy settings");
        assert!(loaded.using_legacy_path);
        assert!(loaded.away_summary_disabled);

        save_claude_settings_to(&paths, &settings(true)).expect("save canonical settings");
        let saved: Value = serde_json::from_str(
            &fs::read_to_string(&paths.canonical).expect("read canonical settings"),
        )
        .expect("parse canonical settings");
        assert_eq!(saved["futureSetting"]["keep"], Value::Bool(true));
        assert_eq!(saved["permissions"]["defaultMode"], "bypassPermissions");
        assert!(
            paths.legacy[0].exists(),
            "legacy file must not be modified or removed"
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn default_permission_mode_uses_auto_and_preserves_unrelated_settings() {
        let (directory, paths) = temp_paths();
        fs::create_dir_all(&directory).expect("create temp directory");
        assert_eq!(
            load_claude_default_permission_mode_from(&paths).expect("load missing settings"),
            "auto"
        );

        fs::write(
            &paths.canonical,
            br#"{"futureSetting":{"keep":true},"permissions":{"defaultMode":"bypassPermissions"}}"#,
        )
        .expect("write settings");
        assert_eq!(
            load_claude_default_permission_mode_from(&paths).expect("load configured mode"),
            "bypassPermissions"
        );

        save_claude_default_permission_mode_to(&paths, "plan").expect("save plan mode");
        let saved: Value = serde_json::from_str(
            &fs::read_to_string(&paths.canonical).expect("read settings"),
        )
        .expect("parse settings");
        assert_eq!(saved["permissions"]["defaultMode"], "plan");
        assert_eq!(saved["futureSetting"]["keep"], Value::Bool(true));
        let _ = fs::remove_dir_all(directory);
    }
}
