//! config_store.rs
//!
//! Reads and writes named config profiles as JSON.
//!
//! Claude configs:  {data_dir}/ClaudeEnvManager/env_configs.json
//!
//! Format: { "config_name": { "VAR_NAME": "value", ... } }

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use crate::file_transaction::{restore_json_backup_if_missing, write_json_atomic};

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn app_data_dir() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|d| d.join("ClaudeEnvManager"))
        .ok_or_else(|| "Could not determine application data directory".to_string())
}

fn claude_config_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("env_configs.json"))
}

// ---------------------------------------------------------------------------
// Generic load / save helpers
// ---------------------------------------------------------------------------

/// Load a configs file.  Returns an empty map when the file does not exist.
fn load_configs_from(path: &PathBuf) -> Result<HashMap<String, HashMap<String, String>>, String> {
    restore_json_backup_if_missing(path, "Claude Code 配置方案")?;
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let raw = fs::read_to_string(path).map_err(|e| format!("Failed to read config file: {e}"))?;

    // Parse as generic JSON first so we can handle any value type gracefully.
    let json: Value =
        serde_json::from_str(&raw).map_err(|e| format!("Failed to parse config file: {e}"))?;

    let obj = json
        .as_object()
        .ok_or_else(|| "Config file root is not a JSON object".to_string())?;

    let mut result: HashMap<String, HashMap<String, String>> = HashMap::new();

    for (config_name, config_val) in obj {
        let inner = config_val
            .as_object()
            .ok_or_else(|| format!("Config entry '{config_name}' is not a JSON object"))?;

        let mut vars: HashMap<String, String> = HashMap::new();
        for (k, v) in inner {
            // Coerce every value to a string, matching Python's behaviour.
            let s = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            vars.insert(k.clone(), s);
        }
        result.insert(config_name.clone(), vars);
    }

    Ok(result)
}

/// Save a configs map to a file.
/// Uses `ensure_ascii=false, indent=2` to match the Python json.dump output.
fn save_configs_to(
    path: &PathBuf,
    configs: &HashMap<String, HashMap<String, String>>,
) -> Result<(), String> {
    // Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {e}"))?;
    }

    // Serialise with pretty-print (indent=2) to match Python output.
    let json = serde_json::to_string_pretty(configs)
        .map_err(|e| format!("Failed to serialise configs: {e}"))?;

    write_json_atomic(path, json.as_bytes(), "Claude Code 配置方案")
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn profile_round_trip_preserves_unknown_environment_fields() {
        let directory = std::env::temp_dir().join(format!("agents-launcher-{}", Uuid::new_v4()));
        let path = directory.join("env_configs.json");
        let mut vars = HashMap::new();
        vars.insert("ANTHROPIC_MODEL".to_string(), "known-model".to_string());
        vars.insert(
            "FUTURE_CLAUDE_OPTION".to_string(),
            "preserve-me".to_string(),
        );
        let mut configs = HashMap::new();
        configs.insert("profile".to_string(), vars);

        save_configs_to(&path, &configs).expect("save profiles");
        let loaded = load_configs_from(&path).expect("load profiles");

        assert_eq!(loaded, configs);
        let _ = fs::remove_dir_all(directory);
    }
}

// ---------------------------------------------------------------------------
// Tauri commands — Claude configs
// ---------------------------------------------------------------------------

/// Load all Claude Code environment-variable config profiles.
#[tauri::command]
pub fn load_claude_configs() -> Result<HashMap<String, HashMap<String, String>>, String> {
    let path = claude_config_path()?;
    load_configs_from(&path)
}

/// Persist all Claude Code environment-variable config profiles.
#[tauri::command]
pub fn save_claude_configs(
    configs: HashMap<String, HashMap<String, String>>,
) -> Result<(), String> {
    let path = claude_config_path()?;
    save_configs_to(&path, &configs)
}
