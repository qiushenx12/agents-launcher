//! 跨平台环境变量应用器
//!
//! Windows: 转发到 registry.rs（注册表写入）
//! macOS:   写入 ~/.claude/settings.json 的 env 字段

use std::collections::HashMap;
use serde_json::{Map, Value};

#[tauri::command]
pub fn apply_env_vars(vars: HashMap<String, String>, scope: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        crate::registry::apply_env_vars_impl(vars, scope)
    }

    #[cfg(target_os = "macos")]
    {
        let _scope = scope;
        apply_env_vars_macos(vars)
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (vars, scope);
        Err("Unsupported platform".to_string())
    }
}

#[cfg(target_os = "macos")]
fn apply_env_vars_macos(vars: HashMap<String, String>) -> Result<(), String> {
    let settings_path = dirs::home_dir()
        .ok_or_else(|| "无法确定用户主目录".to_string())?
        .join(".claude")
        .join("settings.json");

    // 读取现有 settings.json
    let mut obj: Map<String, Value> = if settings_path.exists() {
        let raw = std::fs::read_to_string(&settings_path)
            .map_err(|e| format!("读取 settings.json 失败: {}", e))?;
        serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|v| match v {
                Value::Object(m) => Some(m),
                _ => None,
            })
            .unwrap_or_default()
    } else {
        Map::new()
    };

    // 构建 env 对象（空值不写入，避免覆盖 shell 中已设置的环境变量）
    let mut env_map = Map::new();
    for (k, v) in vars {
        if !v.is_empty() {
            env_map.insert(k, Value::String(v));
        }
    }
    obj.insert("env".to_string(), Value::Object(env_map));

    // 确保目录存在
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败: {}", e))?;
    }

    let json = serde_json::to_string_pretty(&Value::Object(obj))
        .map_err(|e| format!("序列化 settings.json 失败: {}", e))?;
    std::fs::write(&settings_path, json.as_bytes())
        .map_err(|e| format!("写入 settings.json 失败: {}", e))?;

    Ok(())
}
