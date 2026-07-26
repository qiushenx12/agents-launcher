use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::cli_contract::CliKind;
use crate::cli_migration::migrate_project_store_value;

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
    #[serde(default)]
    pub active_project_ids: BTreeMap<CliKind, String>,
    #[serde(default)]
    pub active_session_ids: BTreeMap<CliKind, String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    #[serde(default)]
    pub cli_kind: CliKind,
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub order: i64,
    #[serde(default)]
    pub recent_items: Vec<RecentItem>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSession {
    #[serde(default)]
    pub cli_kind: CliKind,
    pub id: String,
    pub project_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_mode: Option<String>,
    #[serde(default)]
    pub shell: Option<Vec<String>>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    pub created_at: i64,
    pub updated_at: i64,
    pub order: i64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
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
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn inferred_session_kind(id: &str) -> Option<(CliKind, String)> {
    id.strip_prefix("session-codex-")
        .map(|native_id| (CliKind::Codex, native_id.to_string()))
        .or_else(|| {
            id.strip_prefix("session-opencode-")
                .map(|native_id| (CliKind::Opencode, native_id.to_string()))
        })
}

fn normalized_project_key(kind: CliKind, path: &str) -> String {
    #[cfg(windows)]
    let normalized = path
        .trim()
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_lowercase();
    #[cfg(not(windows))]
    let normalized = {
        if path == "/" {
            "/".to_string()
        } else {
            path.trim_end_matches('/').to_string()
        }
    };
    format!("{}:{normalized}", kind.as_str())
}

fn merge_project_record(canonical: &mut Project, duplicate: Project) {
    canonical.created_at = canonical.created_at.min(duplicate.created_at);
    canonical.updated_at = canonical.updated_at.max(duplicate.updated_at);
    canonical.order = canonical.order.min(duplicate.order);
    for item in duplicate.recent_items {
        let already_present = canonical.recent_items.iter().any(|existing| {
            existing.r#type == item.r#type
                && existing.name == item.name
                && existing.path == item.path
                && existing.url == item.url
        });
        if !already_present {
            canonical.recent_items.push(item);
        }
    }
    for (key, value) in duplicate.extra {
        canonical.extra.entry(key).or_insert(value);
    }
}

fn merge_session_record(canonical: &mut ProjectSession, duplicate: ProjectSession) {
    canonical.created_at = canonical.created_at.min(duplicate.created_at);
    canonical.updated_at = canonical.updated_at.max(duplicate.updated_at);
    canonical.order = canonical.order.min(duplicate.order);
    if canonical.claude_session_id.is_none() {
        canonical.claude_session_id = duplicate.claude_session_id;
    }
    if canonical.native_session_id.is_none() {
        canonical.native_session_id = duplicate.native_session_id;
    }
    if canonical.launch_mode.is_none() {
        canonical.launch_mode = duplicate.launch_mode;
    }
    if canonical.shell.is_none() {
        canonical.shell = duplicate.shell;
    }
    if canonical.cwd.is_none() {
        canonical.cwd = duplicate.cwd;
    }
    if canonical.env.is_none() {
        canonical.env = duplicate.env;
    }
    for (key, value) in duplicate.extra {
        canonical.extra.entry(key).or_insert(value);
    }
}

fn resolve_redirect(redirects: &HashMap<String, String>, id: &str) -> String {
    let mut current = id.to_string();
    for _ in 0..=redirects.len() {
        let Some(next) = redirects.get(&current) else {
            break;
        };
        if next == &current {
            break;
        }
        current = next.clone();
    }
    current
}

fn repair_project_store(data: &mut ProjectStoreFile) -> bool {
    let mut changed = false;
    let mut inferred_by_project: HashMap<String, BTreeSet<CliKind>> = HashMap::new();

    for session in &mut data.sessions {
        let Some((kind, native_id)) = inferred_session_kind(&session.id) else {
            continue;
        };
        inferred_by_project
            .entry(session.project_id.clone())
            .or_default()
            .insert(kind);
        if session.cli_kind == CliKind::Claude {
            session.cli_kind = kind;
            changed = true;
        }
        if session
            .native_session_id
            .as_deref()
            .unwrap_or_default()
            .is_empty()
        {
            session.native_session_id = Some(native_id);
            changed = true;
        }
        if session.launch_mode.is_none() {
            session.launch_mode = Some("resume".to_string());
            changed = true;
        }
    }

    let project_snapshots = data.projects.clone();
    let mut pure_claude_by_path = HashMap::new();
    for project in &project_snapshots {
        if project.cli_kind == CliKind::Claude && !inferred_by_project.contains_key(&project.id) {
            pure_claude_by_path
                .entry(normalized_project_key(CliKind::Claude, &project.path))
                .or_insert_with(|| project.id.clone());
        }
    }
    let mut known_project_ids: HashSet<String> = project_snapshots
        .iter()
        .map(|project| project.id.clone())
        .collect();
    let mut recovered_claude_projects = Vec::new();
    for project in &project_snapshots {
        if project.cli_kind != CliKind::Claude || !inferred_by_project.contains_key(&project.id) {
            continue;
        }
        let has_claude_sessions = data
            .sessions
            .iter()
            .any(|session| session.project_id == project.id && session.claude_session_id.is_some());
        if !has_claude_sessions {
            continue;
        }

        let path_key = normalized_project_key(CliKind::Claude, &project.path);
        let target_project_id = if let Some(existing) = pure_claude_by_path.get(&path_key) {
            existing.clone()
        } else {
            let base_id = format!("{}-claude-recovered", project.id);
            let mut recovered_id = base_id.clone();
            let mut suffix = 2;
            while !known_project_ids.insert(recovered_id.clone()) {
                recovered_id = format!("{base_id}-{suffix}");
                suffix += 1;
            }
            let mut recovered = project.clone();
            recovered.id = recovered_id.clone();
            recovered.cli_kind = CliKind::Claude;
            recovered_claude_projects.push(recovered);
            pure_claude_by_path.insert(path_key, recovered_id.clone());
            recovered_id
        };
        for session in &mut data.sessions {
            if session.project_id == project.id && session.claude_session_id.is_some() {
                session.project_id = target_project_id.clone();
                session.cli_kind = CliKind::Claude;
                changed = true;
            }
        }
    }
    data.projects.extend(recovered_claude_projects);

    for project in &mut data.projects {
        let inferred = inferred_by_project.get(&project.id);
        if project.cli_kind == CliKind::Claude && inferred.is_some_and(|kinds| kinds.len() == 1) {
            project.cli_kind = *inferred.and_then(|kinds| kinds.first()).unwrap();
            changed = true;
        }
    }

    let preferred_projects = data.active_project_ids.clone();
    let mut project_redirects = HashMap::new();
    let mut project_indexes: HashMap<String, usize> = HashMap::new();
    let mut repaired_projects: Vec<Project> = Vec::new();
    for project in std::mem::take(&mut data.projects) {
        let key = normalized_project_key(project.cli_kind, &project.path);
        if let Some(&index) = project_indexes.get(&key) {
            changed = true;
            let existing_id = repaired_projects[index].id.clone();
            let prefer_new = preferred_projects.get(&project.cli_kind) == Some(&project.id)
                && preferred_projects.get(&project.cli_kind) != Some(&existing_id);
            if prefer_new {
                let new_id = project.id.clone();
                let previous = std::mem::replace(&mut repaired_projects[index], project);
                project_redirects.insert(previous.id.clone(), new_id);
                merge_project_record(&mut repaired_projects[index], previous);
            } else {
                let duplicate_id = project.id.clone();
                merge_project_record(&mut repaired_projects[index], project);
                project_redirects.insert(duplicate_id, existing_id);
            }
        } else {
            project_indexes.insert(key, repaired_projects.len());
            repaired_projects.push(project);
        }
    }
    data.projects = repaired_projects;

    for session in &mut data.sessions {
        let next_project_id = resolve_redirect(&project_redirects, &session.project_id);
        if next_project_id != session.project_id {
            session.project_id = next_project_id;
            changed = true;
        }
    }
    for project_id in data.active_project_ids.values_mut() {
        *project_id = resolve_redirect(&project_redirects, project_id);
    }
    if let Some(project_id) = &mut data.active_project_id {
        *project_id = resolve_redirect(&project_redirects, project_id);
    }
    let mut seen_expanded = HashSet::new();
    data.expanded_project_ids = std::mem::take(&mut data.expanded_project_ids)
        .into_iter()
        .map(|id| resolve_redirect(&project_redirects, &id))
        .filter(|id| seen_expanded.insert(id.clone()))
        .collect();

    let project_kinds: HashMap<String, CliKind> = data
        .projects
        .iter()
        .map(|project| (project.id.clone(), project.cli_kind))
        .collect();
    for session in &mut data.sessions {
        if let Some(kind) = project_kinds.get(&session.project_id) {
            if session.cli_kind != *kind {
                session.cli_kind = *kind;
                changed = true;
            }
        }
    }

    let preferred_sessions = data.active_session_ids.clone();
    let mut session_redirects = HashMap::new();
    let mut session_indexes: HashMap<String, usize> = HashMap::new();
    let mut repaired_sessions: Vec<ProjectSession> = Vec::new();
    for session in std::mem::take(&mut data.sessions) {
        let identity = session
            .native_session_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .map(|id| format!("native:{id}"))
            .unwrap_or_else(|| format!("local:{}", session.id));
        let key = format!(
            "{}:{}:{identity}",
            session.cli_kind.as_str(),
            session.project_id
        );
        if let Some(&index) = session_indexes.get(&key) {
            changed = true;
            let existing_id = repaired_sessions[index].id.clone();
            let prefer_new = preferred_sessions.get(&session.cli_kind) == Some(&session.id)
                && preferred_sessions.get(&session.cli_kind) != Some(&existing_id);
            if prefer_new {
                let new_id = session.id.clone();
                let previous = std::mem::replace(&mut repaired_sessions[index], session);
                session_redirects.insert(previous.id.clone(), new_id);
                merge_session_record(&mut repaired_sessions[index], previous);
            } else {
                let duplicate_id = session.id.clone();
                merge_session_record(&mut repaired_sessions[index], session);
                session_redirects.insert(duplicate_id, existing_id);
            }
        } else {
            session_indexes.insert(key, repaired_sessions.len());
            repaired_sessions.push(session);
        }
    }
    data.sessions = repaired_sessions;

    for session_id in data.active_session_ids.values_mut() {
        *session_id = resolve_redirect(&session_redirects, session_id);
    }
    if let Some(session_id) = &mut data.active_session_id {
        *session_id = resolve_redirect(&session_redirects, session_id);
    }

    let final_project_kinds: HashMap<String, CliKind> = data
        .projects
        .iter()
        .map(|project| (project.id.clone(), project.cli_kind))
        .collect();
    data.active_project_ids
        .retain(|kind, id| final_project_kinds.get(id) == Some(kind));
    let final_session_kinds: HashMap<String, CliKind> = data
        .sessions
        .iter()
        .map(|session| (session.id.clone(), session.cli_kind))
        .collect();
    data.active_session_ids
        .retain(|kind, id| final_session_kinds.get(id) == Some(kind));
    if data
        .active_project_id
        .as_ref()
        .is_some_and(|id| !final_project_kinds.contains_key(id))
    {
        data.active_project_id = None;
        changed = true;
    }
    if data
        .active_session_id
        .as_ref()
        .is_some_and(|id| !final_session_kinds.contains_key(id))
    {
        data.active_session_id = None;
        changed = true;
    }

    changed
}

#[tauri::command]
pub fn load_projects() -> Result<ProjectStoreFile, String> {
    let path = projects_path()?;
    if !path.exists() {
        return Ok(ProjectStoreFile::default());
    }

    let raw =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read projects file: {e}"))?;
    let value: Value =
        serde_json::from_str(&raw).map_err(|e| format!("Failed to parse projects file: {e}"))?;
    let (migrated, _) = migrate_project_store_value(value)?;
    let mut data: ProjectStoreFile = serde_json::from_value(migrated)
        .map_err(|e| format!("Failed to decode projects file: {e}"))?;
    repair_project_store(&mut data);
    for session in &mut data.sessions {
        if session.native_session_id.is_none() && session.cli_kind == CliKind::Claude {
            session.native_session_id = session.claude_session_id.clone();
        }
    }
    Ok(data)
}

#[tauri::command]
pub fn save_projects(mut data: ProjectStoreFile) -> Result<(), String> {
    repair_project_store(&mut data);
    let path = projects_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create projects directory: {e}"))?;
    }
    let json = serde_json::to_string_pretty(&data)
        .map_err(|e| format!("Failed to serialise projects: {e}"))?;
    transactional_write(&path, json.as_bytes())
}

fn transactional_write(path: &Path, content: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Projects file has no parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|e| format!("Failed to create projects directory: {e}"))?;

    let temp_path = path.with_extension("json.tmp");
    let backup_path = path.with_extension("json.bak");
    let mut temp = fs::File::create(&temp_path)
        .map_err(|e| format!("Failed to create temporary projects file: {e}"))?;
    temp.write_all(content)
        .map_err(|e| format!("Failed to write temporary projects file: {e}"))?;
    temp.sync_all()
        .map_err(|e| format!("Failed to flush temporary projects file: {e}"))?;
    drop(temp);

    let check = fs::read_to_string(&temp_path)
        .map_err(|e| format!("Failed to verify temporary projects file: {e}"))?;
    serde_json::from_str::<Value>(&check)
        .map_err(|e| format!("Temporary projects file is invalid JSON: {e}"))?;

    if path.exists() {
        if backup_path.exists() {
            fs::remove_file(&backup_path)
                .map_err(|e| format!("Failed to replace projects backup: {e}"))?;
        }
        fs::rename(path, &backup_path)
            .map_err(|e| format!("Failed to back up projects file: {e}"))?;
    }

    if let Err(error) = fs::rename(&temp_path, path) {
        if backup_path.exists() && !path.exists() {
            let _ = fs::rename(&backup_path, path);
        }
        return Err(format!("Failed to commit projects file: {error}"));
    }
    Ok(())
}

#[tauri::command]
pub fn path_kind(path: String) -> Result<String, String> {
    let path_buf = PathBuf::from(&path);
    let metadata = fs::metadata(&path_buf).map_err(|e| format!("Failed to inspect path: {e}"))?;
    if metadata.is_dir() {
        Ok("directory".to_string())
    } else if metadata.is_file() {
        Ok("file".to_string())
    } else {
        Ok("missing".to_string())
    }
}

fn is_supported_text_path(path: &Path) -> bool {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
    {
        Some(ext) => matches!(
            ext.as_str(),
            "md" | "markdown"
                | "txt"
                | "log"
                | "env"
                | "json"
                | "yaml"
                | "yml"
                | "js"
                | "ts"
                | "vue"
                | "html"
                | "css"
                | "rs"
                | "py"
                | "toml"
        ),
        None => false,
    }
}

#[tauri::command]
pub fn read_image_base64(path: String) -> Result<String, String> {
    use std::io::Read;
    let path_buf = std::path::PathBuf::from(&path);
    let ext = path_buf
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        _ => "application/octet-stream",
    };
    let mut file = std::fs::File::open(&path_buf).map_err(|e| format!("无法打开文件：{e}"))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| format!("无法读取文件：{e}"))?;
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}

const MAX_PASTED_IMAGE_BYTES: usize = 20 * 1024 * 1024;

fn pasted_image_extension(extension: &str) -> Result<&'static str, String> {
    match extension.to_lowercase().as_str() {
        "png" => Ok("png"),
        "jpg" | "jpeg" => Ok("jpg"),
        "gif" => Ok("gif"),
        "webp" => Ok("webp"),
        "bmp" => Ok("bmp"),
        _ => Err(format!("不支持的粘贴图片格式：{extension}")),
    }
}

fn save_pasted_image_to(dir: &Path, data_base64: &str, extension: &str) -> Result<String, String> {
    use base64::Engine as _;

    let ext = pasted_image_extension(extension)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64.as_bytes())
        .map_err(|e| format!("粘贴图片数据解码失败：{e}"))?;
    if bytes.len() > MAX_PASTED_IMAGE_BYTES {
        return Err(format!(
            "粘贴图片过大（{} 字节），超过 {} 字节上限",
            bytes.len(),
            MAX_PASTED_IMAGE_BYTES
        ));
    }

    fs::create_dir_all(dir).map_err(|e| format!("创建粘贴图片目录失败：{e}"))?;
    let file_path = dir.join(format!("{}.{}", uuid::Uuid::new_v4(), ext));
    fs::write(&file_path, &bytes).map_err(|e| format!("写入粘贴图片失败：{e}"))?;
    Ok(file_path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn save_pasted_image(data_base64: String, extension: String) -> Result<String, String> {
    let dir = app_data_dir()?.join("pasted_images");
    save_pasted_image_to(&dir, &data_base64, &extension)
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
    fs::read_to_string(&path_buf).map_err(|e| format!("Failed to read file: {e}"))
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
    fs::write(&path_buf, content.as_bytes()).map_err(|e| format!("Failed to write file: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use uuid::Uuid;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("cc-launcher-pasted-images-{}", Uuid::new_v4()))
    }

    #[test]
    fn save_pasted_image_writes_file_and_returns_existing_path() {
        let dir = temp_dir();
        let data = base64::engine::general_purpose::STANDARD.encode(b"fake-png-bytes");

        let path = save_pasted_image_to(&dir, &data, "png").expect("should save image");

        assert!(Path::new(&path).exists());
        assert!(path.ends_with(".png"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_pasted_image_rejects_unsupported_extension() {
        let dir = temp_dir();
        let data = base64::engine::general_purpose::STANDARD.encode(b"fake-bytes");

        let error = save_pasted_image_to(&dir, &data, "exe").expect_err("must reject extension");

        assert!(error.contains("不支持的粘贴图片格式"));
        assert!(!dir.exists());
    }

    #[test]
    fn save_pasted_image_rejects_oversized_payload() {
        let dir = temp_dir();
        let oversized = vec![0u8; MAX_PASTED_IMAGE_BYTES + 1];
        let data = base64::engine::general_purpose::STANDARD.encode(&oversized);

        let error = save_pasted_image_to(&dir, &data, "png").expect_err("must reject size");

        assert!(error.contains("超过"));
        assert!(!dir.exists());
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_project_keys_preserve_case_spaces_and_unicode() {
        assert_ne!(
            normalized_project_key(CliKind::Opencode, "/Users/Demo/项目"),
            normalized_project_key(CliKind::Opencode, "/Users/demo/项目"),
        );
        assert_eq!(
            normalized_project_key(CliKind::Opencode, "/Users/me/My Project/"),
            "opencode:/Users/me/My Project",
        );
        assert_eq!(
            normalized_project_key(CliKind::Opencode, "/Users/me/trailing "),
            "opencode:/Users/me/trailing ",
        );
        assert_eq!(normalized_project_key(CliKind::Opencode, "/"), "opencode:/",);
    }

    #[test]
    fn repairs_misclassified_cli_projects_without_cross_talk() {
        let mut data: ProjectStoreFile = serde_json::from_str(include_str!(
            "../tests/fixtures/migration/misclassified-cli-projects.json"
        ))
        .expect("fixture should parse");

        assert!(repair_project_store(&mut data));
        assert_eq!(data.projects.len(), 3);
        assert_eq!(
            data.projects
                .iter()
                .filter(|project| project.cli_kind == CliKind::Claude)
                .count(),
            1
        );

        let codex = data
            .projects
            .iter()
            .find(|project| project.cli_kind == CliKind::Codex)
            .expect("Codex project should survive");
        assert_eq!(codex.id, "project-codex-current");
        assert_eq!(
            codex.extra.get("staleProjectField"),
            Some(&Value::String("preserve-me".to_string()))
        );
        let codex_sessions: Vec<_> = data
            .sessions
            .iter()
            .filter(|session| session.project_id == codex.id)
            .collect();
        assert!(codex_sessions
            .iter()
            .all(|session| session.cli_kind == CliKind::Codex));
        assert_eq!(
            codex_sessions
                .iter()
                .filter(|session| session.native_session_id.as_deref() == Some("thread-1"))
                .count(),
            1
        );
        assert!(codex_sessions
            .iter()
            .any(|session| session.id == "session-codex-current"));
        assert!(codex_sessions
            .iter()
            .all(|session| session.claude_session_id.is_none()));

        let claude = data
            .projects
            .iter()
            .find(|project| project.cli_kind == CliKind::Claude)
            .expect("Claude project should survive beside CodeX at the same path");
        assert_eq!(claude.path, codex.path);
        assert!(data.sessions.iter().any(|session| {
            session.project_id == claude.id
                && session.claude_session_id.as_deref() == Some("claude-shared-native")
                && session.cli_kind == CliKind::Claude
        }));

        assert_eq!(
            data.active_project_ids
                .get(&CliKind::Codex)
                .map(String::as_str),
            Some("project-codex-current")
        );
        assert!(!data.active_project_ids.contains_key(&CliKind::Claude));
        assert_eq!(
            data.active_session_ids
                .get(&CliKind::Codex)
                .map(String::as_str),
            Some("session-codex-current")
        );
        assert!(!data.active_session_ids.contains_key(&CliKind::Claude));
        assert!(!repair_project_store(&mut data));
    }
}
