use std::process::Command;
use std::collections::HashMap;

#[tauri::command]
pub fn get_current_env_vars(var_names: Vec<String>) -> HashMap<String, String> {
    let mut result = HashMap::new();
    for name in var_names {
        if let Ok(val) = std::env::var(&name) {
            result.insert(name, val);
        }
    }
    result
}

#[tauri::command]
pub fn get_claude_config_dir() -> Result<String, String> {
    let dir = dirs::data_dir()
        .ok_or("Cannot determine APPDATA")?
        .join("ClaudeEnvManager");
    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
pub fn open_directory(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("Failed to open directory: {}", e))?;
    }
    Ok(())
}

#[tauri::command]
pub fn open_env_vars_dialog() -> Result<(), String> {
    #[cfg(windows)]
    {
        Command::new("rundll32")
            .args(["sysdm.cpl,EditEnvironmentVariables"])
            .spawn()
            .map_err(|e| format!("Failed to open env vars dialog: {}", e))?;
    }
    Ok(())
}
