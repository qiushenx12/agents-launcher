//! settings_manager.rs
//!
//! Reads and writes %USERPROFILE%\.claude\settings.json.
//!
//! Managed fields:
//!   - `skipDangerousModePermissionPrompt`: bool
//!   - `permissions.defaultMode`: "bypassPermissions" | "default"
//!   - `awaySummaryEnabled`: bool
//!
//! All other fields in the file are preserved on write.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

// ---------------------------------------------------------------------------
// Path helper
// ---------------------------------------------------------------------------

fn settings_path() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|h| h.join(".claude").join("settings.json"))
        .ok_or_else(|| "Could not determine %USERPROFILE% directory".to_string())
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
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Read the managed fields from settings.json.
/// Returns defaults (both false) when the file does not exist or cannot be parsed.
#[tauri::command]
pub fn load_claude_settings() -> Result<ClaudeSettings, String> {
    let path = settings_path()?;

    if !path.exists() {
        return Ok(ClaudeSettings {
            skip_permissions: false,
            away_summary_disabled: false,
        });
    }

    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read settings.json: {e}"))?;

    let settings: Value = serde_json::from_str(&raw).unwrap_or(Value::Object(Map::new()));

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
    let away_summary_disabled = settings
        .get("awaySummaryEnabled")
        .and_then(|v| v.as_bool())
        == Some(false);

    Ok(ClaudeSettings {
        skip_permissions,
        away_summary_disabled,
    })
}

/// Write the managed fields back to settings.json, preserving all other fields.
#[tauri::command]
pub fn save_claude_settings(settings: ClaudeSettings) -> Result<(), String> {
    let path = settings_path()?;

    // Load existing content (or start with an empty object).
    let mut obj: Map<String, Value> = if path.exists() {
        let raw = fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read settings.json: {e}"))?;
        serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|v| v.into_object())
            .unwrap_or_default()
    } else {
        Map::new()
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create .claude directory: {e}"))?;
    }

    let json = serde_json::to_string_pretty(&Value::Object(obj))
        .map_err(|e| format!("Failed to serialise settings: {e}"))?;

    fs::write(&path, json.as_bytes())
        .map_err(|e| format!("Failed to write settings.json: {e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Extension trait to convert Value → Map
// ---------------------------------------------------------------------------

trait IntoObject {
    fn into_object(self) -> Option<Map<String, Value>>;
}

impl IntoObject for Value {
    fn into_object(self) -> Option<Map<String, Value>> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }
}
