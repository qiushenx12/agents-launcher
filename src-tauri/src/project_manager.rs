use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

fn app_data_dir() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|d| d.join("ClaudeEnvManager"))
        .ok_or_else(|| "Could not determine %APPDATA% directory".to_string())
}

fn projects_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("projects.json"))
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStoreFile {
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub sessions: Vec<ProjectSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_session_id: Option<String>,
    #[serde(default)]
    pub expanded_project_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_sort_mode: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub order: i64,
    #[serde(default)]
    pub recent_items: Vec<RecentItem>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSession {
    pub id: String,
    pub project_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
    #[serde(default)]
    pub shell: Option<Vec<String>>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: Option<std::collections::HashMap<String, String>>,
    pub created_at: i64,
    pub updated_at: i64,
    pub order: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RecentItem {
    pub r#type: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub opened_at: i64,
}

#[tauri::command]
pub fn load_projects() -> Result<ProjectStoreFile, String> {
    let path = projects_path()?;
    if !path.exists() {
        return Ok(ProjectStoreFile::default());
    }

    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read projects file: {e}"))?;
    serde_json::from_str(&raw)
        .map_err(|e| format!("Failed to parse projects file: {e}"))
}

#[tauri::command]
pub fn save_projects(data: ProjectStoreFile) -> Result<(), String> {
    let path = projects_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create projects directory: {e}"))?;
    }
    let json = serde_json::to_string_pretty(&data)
        .map_err(|e| format!("Failed to serialise projects: {e}"))?;
    fs::write(&path, json.as_bytes())
        .map_err(|e| format!("Failed to write projects file: {e}"))
}

#[tauri::command]
pub fn path_kind(path: String) -> Result<String, String> {
    let path_buf = PathBuf::from(&path);
    let metadata = fs::metadata(&path_buf)
        .map_err(|e| format!("Failed to inspect path: {e}"))?;
    if metadata.is_dir() {
        Ok("directory".to_string())
    } else if metadata.is_file() {
        Ok("file".to_string())
    } else {
        Ok("missing".to_string())
    }
}

fn is_supported_text_path(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()) {
        Some(ext) => matches!(
            ext.as_str(),
            "md" | "markdown" | "txt" | "log" | "env" | "json" | "yaml" | "yml"
                | "js" | "ts" | "vue" | "html" | "css" | "rs" | "py" | "toml"
        ),
        None => false,
    }
}

#[tauri::command]
pub fn read_text_file(path: String) -> Result<String, String> {
    let path_buf = PathBuf::from(&path);
    if !is_supported_text_path(&path_buf) {
        let ext = path_buf
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("unknown");
        return Err(format!("无法打开该文件类型：{ext}"));
    }
    fs::read_to_string(&path_buf)
        .map_err(|e| format!("Failed to read file: {e}"))
}

#[tauri::command]
pub fn save_text_file(path: String, content: String) -> Result<(), String> {
    let path_buf = PathBuf::from(&path);
    if !is_supported_text_path(&path_buf) {
        let ext = path_buf
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("unknown");
        return Err(format!("无法打开该文件类型：{ext}"));
    }
    fs::write(&path_buf, content.as_bytes())
        .map_err(|e| format!("Failed to write file: {e}"))
}
