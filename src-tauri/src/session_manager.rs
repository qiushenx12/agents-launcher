use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct SessionEntry {
    pub id: String,
    pub display: String,
    pub ts: i64,
    /// Official AI-generated session title from the session file
    /// (`{"type":"ai-title",...}` lines in `~/.claude/projects/<dir>/<id>.jsonl`).
    /// Falls back to `display` (last user prompt) when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

fn history_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".claude")
        .join("history.jsonl")
}

#[derive(Clone, Serialize)]
struct ClaudeHistoryChangedPayload {
    path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HistoryRevision {
    Missing,
    Present {
        len: u64,
        modified: Option<SystemTime>,
    },
    Unavailable,
}

#[derive(Debug, Default)]
pub(crate) struct ClaudeHistorySnapshot {
    pub recent_projects: Vec<String>,
    pub sessions_by_project: HashMap<String, Vec<SessionEntry>>,
    /// Session id → official AI title. Only populated for WSL snapshots (the
    /// distro reports all titles in one batch); local titles are read from
    /// session files on demand. Written on all platforms, but only the
    /// Windows WSL session loader reads it.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub titles: HashMap<String, String>,
}

#[derive(Debug)]
struct ClaudeHistoryCacheEntry {
    revision: HistoryRevision,
    snapshot: Arc<ClaudeHistorySnapshot>,
}

static HISTORY_CACHE: OnceLock<Mutex<Option<ClaudeHistoryCacheEntry>>> = OnceLock::new();

fn history_cache() -> &'static Mutex<Option<ClaudeHistoryCacheEntry>> {
    HISTORY_CACHE.get_or_init(|| Mutex::new(None))
}

fn invalidate_history_cache() {
    let mut cache = history_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *cache = None;
}

fn history_revision(path: &std::path::Path) -> HistoryRevision {
    match std::fs::metadata(path) {
        Ok(metadata) => HistoryRevision::Present {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => HistoryRevision::Missing,
        Err(_) => HistoryRevision::Unavailable,
    }
}

fn is_history_change(kind: &notify::EventKind) -> bool {
    matches!(
        kind,
        notify::EventKind::Create(_) | notify::EventKind::Modify(_) | notify::EventKind::Remove(_)
    )
}

pub fn start_history_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        let history = history_path();
        let Some(history_dir) = history.parent().map(PathBuf::from) else {
            return;
        };
        let Some(history_name) = history.file_name().map(|name| name.to_owned()) else {
            return;
        };

        if !history_dir.exists() {
            return;
        }

        let event_history = history.clone();
        let event_app = app.clone();
        let mut watcher =
            match notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                let Ok(event) = result else {
                    return;
                };

                if !is_history_change(&event.kind) {
                    return;
                }

                let matches_history = event.paths.iter().any(|path| {
                    path == &event_history
                        || path
                            .file_name()
                            .map(|name| name == history_name)
                            .unwrap_or(false)
                });

                if matches_history {
                    invalidate_history_cache();
                    let _ = event_app.emit(
                        "claude_history_changed",
                        ClaudeHistoryChangedPayload {
                            path: event_history.to_string_lossy().to_string(),
                        },
                    );
                }
            }) {
                Ok(watcher) => watcher,
                Err(_) => return,
            };

        if notify::Watcher::watch(
            &mut watcher,
            &history_dir,
            notify::RecursiveMode::NonRecursive,
        )
        .is_err()
        {
            return;
        }

        loop {
            std::thread::park();
        }
    });
}

fn parse_ts(value: &serde_json::Value) -> i64 {
    if let Some(n) = value.as_i64() {
        return n;
    }
    if let Some(n) = value.as_f64() {
        return n as i64;
    }
    if let Some(s) = value.as_str() {
        if let Ok(n) = s.parse::<f64>() {
            return n as i64;
        }
    }
    0
}

// ---------------------------------------------------------------------------
// Official session titles (`{"type":"ai-title",...}` lines in session files)
// ---------------------------------------------------------------------------

/// Bytes of a session file scanned from the end before falling back to a full
/// scan. Claude Code appends an `ai-title` line throughout the session, so the
/// latest title is normally within this window.
const TITLE_SCAN_WINDOW: u64 = 2 * 1024 * 1024;

/// Encodes a project path into Claude Code's `~/.claude/projects/<dir>` name:
/// every non-ASCII-alphanumeric character (including `/`, `\`, `:` and any
/// non-ASCII character) becomes `-`.
///
/// Known limitation: Claude Code truncates names longer than 200 chars and
/// appends a path hash; that form is not reproduced here (the read paths in
/// this module share the same limitation, so behaviour stays consistent).
pub(crate) fn encode_project_dir(project: &str) -> String {
    project
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Scans `path` starting at `offset` and returns the last `aiTitle` found in
/// `{"type":"ai-title",...}` lines (the file is append-only, so the last
/// occurrence is the current title).
fn scan_title_from(path: &std::path::Path, offset: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};

    let file = File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    if offset > 0 {
        reader.seek(SeekFrom::Start(offset)).ok()?;
    }

    let mut last: Option<String> = None;
    for line in reader.lines() {
        let Ok(raw) = line else {
            continue;
        };
        let raw = raw.trim();
        if raw.is_empty() || !raw.contains("ai-title") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            continue;
        };
        if value.get("type").and_then(serde_json::Value::as_str) != Some("ai-title") {
            continue;
        }
        if let Some(title) = value.get("aiTitle").and_then(serde_json::Value::as_str) {
            let title = title.trim();
            if !title.is_empty() {
                last = Some(title.to_string());
            }
        }
    }
    last
}

/// Latest official AI title of one Claude Code session file, if any.
fn read_session_title(path: &std::path::Path) -> Option<String> {
    let size = std::fs::metadata(path).ok()?.len();
    let offset = size.saturating_sub(TITLE_SCAN_WINDOW);
    if let Some(title) = scan_title_from(path, offset) {
        return Some(title);
    }
    if offset == 0 {
        return None;
    }
    // Title predates the tail window (e.g. one prompt, long output): rescan
    // the whole file.
    scan_title_from(path, 0)
}

/// Fills `SessionEntry::title` for local Claude sessions by reading
/// `<projects_root>/<encoded project>/<session id>.jsonl`.
pub(crate) fn enrich_claude_session_titles(
    sessions: &[SessionEntry],
    project: &str,
    projects_root: &std::path::Path,
) -> Vec<SessionEntry> {
    let project_dir = projects_root.join(encode_project_dir(project));
    sessions
        .iter()
        .map(|entry| {
            let mut entry = entry.clone();
            entry.title = read_session_title(&project_dir.join(format!("{}.jsonl", entry.id)));
            entry
        })
        .collect()
}

pub(crate) fn parse_history(reader: impl BufRead) -> ClaudeHistorySnapshot {
    let mut recent_projects: HashMap<String, i64> = HashMap::new();
    let mut sessions_by_project: HashMap<String, HashMap<String, (i64, String)>> = HashMap::new();

    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => continue,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let project = entry
            .get("project")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let session_id = entry
            .get("sessionId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let ts = entry.get("timestamp").map(parse_ts).unwrap_or(0);
        let display = entry
            .get("display")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();

        if !project.is_empty() {
            recent_projects
                .entry(project.to_string())
                .and_modify(|existing| {
                    if ts > *existing {
                        *existing = ts;
                    }
                })
                .or_insert(ts);
        }

        if !session_id.is_empty() {
            sessions_by_project
                .entry(project.to_string())
                .or_default()
                .entry(session_id.to_string())
                .and_modify(|existing| {
                    if ts >= existing.0 {
                        existing.0 = ts;
                        existing.1 = display.clone();
                    }
                })
                .or_insert((ts, display));
        }
    }

    let mut recent_projects: Vec<(String, i64)> = recent_projects.into_iter().collect();
    recent_projects.sort_by(|left, right| right.1.cmp(&left.1));

    let sessions_by_project = sessions_by_project
        .into_iter()
        .map(|(project, sessions)| {
            let mut sessions = sessions
                .into_iter()
                .map(|(id, (ts, display))| SessionEntry {
                    id,
                    display,
                    ts,
                    title: None,
                })
                .collect::<Vec<_>>();
            sessions.sort_by(|left, right| right.ts.cmp(&left.ts));
            (project, sessions)
        })
        .collect();

    ClaudeHistorySnapshot {
        recent_projects: recent_projects
            .into_iter()
            .take(10)
            .map(|(project, _)| project)
            .collect(),
        sessions_by_project,
        titles: HashMap::new(),
    }
}

fn parse_history_file(path: &std::path::Path) -> Result<ClaudeHistorySnapshot, String> {
    if !path.exists() {
        return Ok(ClaudeHistorySnapshot::default());
    }
    let file = File::open(path).map_err(|error| format!("Failed to open history file: {error}"))?;
    Ok(parse_history(std::io::BufReader::new(file)))
}

fn load_history_snapshot() -> Result<Arc<ClaudeHistorySnapshot>, String> {
    let path = history_path();
    load_history_snapshot_from_path(&path, history_cache())
}

fn load_history_snapshot_from_path(
    path: &std::path::Path,
    cache: &Mutex<Option<ClaudeHistoryCacheEntry>>,
) -> Result<Arc<ClaudeHistorySnapshot>, String> {
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    for _ in 0..2 {
        let revision = history_revision(path);
        if let Some(cached) = cache.as_ref().filter(|cached| cached.revision == revision) {
            return Ok(cached.snapshot.clone());
        }

        let snapshot = Arc::new(parse_history_file(path)?);
        let revision_after = history_revision(path);
        if revision == revision_after && revision != HistoryRevision::Unavailable {
            *cache = Some(ClaudeHistoryCacheEntry {
                revision,
                snapshot: snapshot.clone(),
            });
            return Ok(snapshot);
        }
    }

    Ok(Arc::new(parse_history_file(path)?))
}

#[tauri::command]
pub fn load_claude_sessions(target_dir: String) -> Result<Vec<SessionEntry>, String> {
    let sessions = load_history_snapshot()?
        .sessions_by_project
        .get(&target_dir)
        .cloned()
        .unwrap_or_default();
    let Some(projects_root) = dirs::home_dir().map(|home| home.join(".claude").join("projects")) else {
        return Ok(sessions);
    };
    Ok(enrich_claude_session_titles(&sessions, &target_dir, &projects_root))
}

#[tauri::command]
pub fn load_claude_recent_projects() -> Result<Vec<String>, String> {
    Ok(load_history_snapshot()?.recent_projects.clone())
}

#[tauri::command]
pub fn invalidate_claude_history_cache() {
    invalidate_history_cache();
}

// ---------------------------------------------------------------------------
// Native deletion of Claude Code project/session data
// ---------------------------------------------------------------------------
//
// Mirrors the official `claude project purge` deletion plan (verified with
// `--dry-run` against a real project): transcripts dir, per-session
// file-history, the `projects["<path>"]` entry in ~/.claude.json, and the
// project's lines in history.jsonl. `shell-snapshots/`, `backups/` and
// `session-env/` are deliberately left alone, matching the official command.
//
// Deletion order is fail-safe: files first, history.jsonl second, .claude.json
// last. A hard file error aborts before history.jsonl is touched, so the
// sidebar list (driven by history.jsonl) stays unchanged and the user can
// retry.

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NativeDeleteReport {
    pub deleted: Vec<String>,
    pub warnings: Vec<String>,
}

fn claude_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".claude")
}

fn claude_json_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".claude.json")
}

/// Session ids may be interpolated into filesystem paths; accept only the
/// UUID-shaped ids Claude Code actually issues.
pub(crate) fn validate_native_session_id(session_id: &str) -> Result<(), String> {
    let valid = (8..=64).contains(&session_id.len())
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(format!("非法的会话 ID：{session_id:?}"))
    }
}

pub(crate) fn validate_native_project_path(project_path: &str) -> Result<String, String> {
    let trimmed = project_path.trim();
    if trimmed.is_empty() || trimmed.chars().all(|c| c == '/' || c == '\\') {
        return Err("项目路径为空或无效".to_string());
    }
    Ok(trimmed.to_string())
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as i32, 0) };
    if result == 0 {
        return true;
    }
    // EPERM: the process exists but belongs to another user.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn pid_is_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                true
            }
            Err(_) => false,
        }
    }
}

/// Session ids with a live process, from `~/.claude/sessions/<pid>.json`.
/// Unreadable entries are skipped: on Windows a genuinely in-use transcript
/// still fails the delete with a sharing violation, which surfaces as Err.
fn live_session_ids(sessions_dir: &Path) -> HashSet<String> {
    let mut live = HashSet::new();
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return live;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
            continue;
        };
        let (Some(session_id), Some(pid)) = (
            value.get("sessionId").and_then(|v| v.as_str()),
            value.get("pid").and_then(|v| v.as_u64()),
        ) else {
            continue;
        };
        if pid_is_alive(pid as u32) {
            live.insert(session_id.to_string());
        }
    }
    live
}

/// Refuse deletion while any target session is still running. Retried briefly
/// because the frontend kills the app-hosted PTY right before invoking the
/// command, and the Claude process may need a moment to exit.
fn guard_no_live_sessions(claude_dir: &Path, targets: &HashSet<String>) -> Result<(), String> {
    if targets.is_empty() {
        return Ok(());
    }
    let sessions_dir = claude_dir.join("sessions");
    for attempt in 0..3 {
        let live = live_session_ids(&sessions_dir);
        let blocking: Vec<&String> = targets.iter().filter(|id| live.contains(*id)).collect();
        if blocking.is_empty() {
            return Ok(());
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(300));
            continue;
        }
        let shown = blocking
            .iter()
            .take(3)
            .map(|id| id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "会话仍在运行中（{shown}），请先关闭对应的 Claude 进程再删除"
        ));
    }
    Ok(())
}

/// Deletes a file or directory. Missing paths are skipped silently (the
/// official cleanup may have removed them already); a real failure aborts.
fn remove_native_path(path: &Path, report: &mut NativeDeleteReport) -> Result<(), String> {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let result = if metadata.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
            result.map_err(|error| format!("删除 {} 失败：{error}", path.display()))?;
            report.deleted.push(path.display().to_string());
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("读取 {} 状态失败：{error}", path.display())),
    }
}

/// Rewrites history.jsonl keeping every line for which `drop_line` is false.
/// Unparseable lines are always kept (they are not ours to delete). Returns
/// the number of dropped lines; the file is left untouched when nothing
/// matched. The read-filter-write window races with the CLI's own appends,
/// same as the official purge; the atomic write keeps it as small as possible.
fn filter_history_lines(
    path: &Path,
    drop_line: impl Fn(&serde_json::Value) -> bool,
) -> Result<usize, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("读取 history.jsonl 失败：{error}")),
    };
    let mut kept = String::with_capacity(content.len());
    let mut dropped = 0usize;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let drop = serde_json::from_str::<serde_json::Value>(line)
            .map(|value| drop_line(&value))
            .unwrap_or(false);
        if drop {
            dropped += 1;
        } else {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    if dropped == 0 {
        return Ok(0);
    }
    // history.jsonl is 0600 on unix; the private writer preserves that.
    crate::file_transaction::write_private_text_atomic(
        path,
        kept.as_bytes(),
        "Claude 历史 history.jsonl",
    )?;
    Ok(dropped)
}

/// Applies `edit` to ~/.claude.json, preserving every field it does not touch.
/// Returns true when the file changed.
fn edit_claude_json(
    path: &Path,
    edit: impl FnOnce(&mut serde_json::Value) -> bool,
) -> Result<bool, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("读取 .claude.json 失败：{error}")),
    };
    let mut value: serde_json::Value =
        serde_json::from_str(&content).map_err(|error| format!(".claude.json 解析失败：{error}"))?;
    if !edit(&mut value) {
        return Ok(false);
    }
    let serialized = serde_json::to_string(&value)
        .map_err(|error| format!(".claude.json 序列化失败：{error}"))?;
    crate::file_transaction::write_private_json_atomic(
        path,
        serialized.as_bytes(),
        ".claude.json",
    )?;
    Ok(true)
}

/// Session ids attributed to a project: history.jsonl lines plus the
/// transcript file names under `projects/<encoded>/` (the two sources can
/// diverge after the official 30-day cleanup).
fn collect_project_session_ids(claude_dir: &Path, project_path: &str) -> HashSet<String> {
    let mut ids = HashSet::new();
    if let Ok(content) = std::fs::read_to_string(claude_dir.join("history.jsonl")) {
        for line in content.lines() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if value.get("project").and_then(|p| p.as_str()) != Some(project_path) {
                continue;
            }
            if let Some(id) = value.get("sessionId").and_then(|s| s.as_str()) {
                if !id.is_empty() {
                    ids.insert(id.to_string());
                }
            }
        }
    }
    let project_dir = claude_dir
        .join("projects")
        .join(encode_project_dir(project_path));
    if let Ok(entries) = std::fs::read_dir(&project_dir) {
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if let Some(id) = name.strip_suffix(".jsonl") {
                ids.insert(id.to_string());
            }
        }
    }
    ids
}

fn delete_project_native_at(
    claude_dir: &Path,
    claude_json: &Path,
    project_path: &str,
) -> Result<NativeDeleteReport, String> {
    let mut report = NativeDeleteReport::default();
    let session_ids = collect_project_session_ids(claude_dir, project_path);
    guard_no_live_sessions(claude_dir, &session_ids)?;

    remove_native_path(
        &claude_dir
            .join("projects")
            .join(encode_project_dir(project_path)),
        &mut report,
    )?;
    for session_id in &session_ids {
        remove_native_path(&claude_dir.join("file-history").join(session_id), &mut report)?;
    }

    let dropped = filter_history_lines(&claude_dir.join("history.jsonl"), |value| {
        value.get("project").and_then(|p| p.as_str()) == Some(project_path)
    })?;
    if dropped > 0 {
        report
            .deleted
            .push(format!("history.jsonl 中的 {dropped} 行 prompt 历史"));
    }

    if edit_claude_json(claude_json, |value| {
        value
            .get_mut("projects")
            .and_then(|projects| projects.as_object_mut())
            .map(|projects| projects.remove(project_path).is_some())
            .unwrap_or(false)
    })? {
        report
            .deleted
            .push(format!(".claude.json 项目条目 projects[\"{project_path}\"]"));
    }
    Ok(report)
}

fn delete_session_native_at(
    claude_dir: &Path,
    claude_json: &Path,
    project_path: &str,
    session_id: &str,
) -> Result<NativeDeleteReport, String> {
    let mut report = NativeDeleteReport::default();
    guard_no_live_sessions(claude_dir, &HashSet::from([session_id.to_string()]))?;

    let project_dir = claude_dir
        .join("projects")
        .join(encode_project_dir(project_path));
    remove_native_path(
        &project_dir.join(format!("{session_id}.jsonl")),
        &mut report,
    )?;
    remove_native_path(&project_dir.join(session_id), &mut report)?;
    remove_native_path(
        &claude_dir.join("file-history").join(session_id),
        &mut report,
    )?;

    let dropped = filter_history_lines(&claude_dir.join("history.jsonl"), |value| {
        value.get("sessionId").and_then(|s| s.as_str()) == Some(session_id)
    })?;
    if dropped > 0 {
        report
            .deleted
            .push(format!("history.jsonl 中的 {dropped} 行 prompt 历史"));
    }

    // A `lastSessionId` pointing at the deleted session would make
    // `claude --continue` in that directory fail; clear it.
    edit_claude_json(claude_json, |value| {
        let Some(project) = value
            .get_mut("projects")
            .and_then(|projects| projects.get_mut(project_path))
        else {
            return false;
        };
        if project.get("lastSessionId").and_then(|v| v.as_str()) != Some(session_id) {
            return false;
        }
        project
            .as_object_mut()
            .map(|project| project.remove("lastSessionId").is_some())
            .unwrap_or(false)
    })?;
    Ok(report)
}

// ---------------------------------------------------------------------------
// Relocating a Claude Code project to a new directory
// ---------------------------------------------------------------------------
//
// Claude Code keys every piece of project data by the absolute path: the
// transcript directory `projects/<encoded path>/`, the `project` field on each
// history.jsonl line, and the `projects["<path>"]` entry in ~/.claude.json.
// When the folder is moved or renamed, all three keep pointing at the stale
// path, so `claude -r <id>`, the launcher's session sync, and PTY launches
// against the new location all fail to find anything.
//
// `relocate_claude_project_native` rewrites all three to the new path (merging
// with whatever already exists at the destination), guarded by the same
// live-session check as deletion. Fail-safe order mirrors deletion: transcripts
// first, history.jsonl second, .claude.json last. `file-history/<session id>`
// is keyed by session id, so it needs no changes.

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NativeRelocateReport {
    pub migrated: Vec<String>,
    pub warnings: Vec<String>,
}

/// Moves (or merges) the transcript directory from the old encoded path to the
/// new one. Missing source directories are fine — the old path may simply have
/// never run a session that survived the official cleanup.
fn relocate_transcripts_dir(
    claude_dir: &Path,
    old_project: &str,
    new_project: &str,
    report: &mut NativeRelocateReport,
) -> Result<(), String> {
    let src = claude_dir
        .join("projects")
        .join(encode_project_dir(old_project));
    let dst = claude_dir
        .join("projects")
        .join(encode_project_dir(new_project));
    let Ok(src_metadata) = std::fs::metadata(&src) else {
        return Ok(());
    };
    if !src_metadata.is_dir() {
        return Err(format!("转写目录 {} 不是目录", src.display()));
    }
    if dst.exists() {
        let dst_metadata =
            std::fs::metadata(&dst).map_err(|e| format!("读取 {} 失败：{e}", dst.display()))?;
        if !dst_metadata.is_dir() {
            return Err(format!("目标转写目录 {} 不是目录", dst.display()));
        }
        // Merge: the destination keeps its own copies (same name = same
        // session id), the source copy is dropped after it is superseded.
        let mut conflicts = 0usize;
        for entry in std::fs::read_dir(&src).map_err(|e| format!("读取转写目录失败：{e}"))? {
            let Ok(entry) = entry else { continue };
            let target = dst.join(entry.file_name());
            if target.exists() {
                conflicts += 1;
                let removed = std::fs::remove_file(&entry.path())
                    .or_else(|_| std::fs::remove_dir_all(&entry.path()));
                if let Err(e) = removed {
                    report.warnings.push(format!(
                        "同名转写条目 {} 清理失败：{e}",
                        entry.path().display()
                    ));
                }
                continue;
            }
            std::fs::rename(entry.path(), &target)
                .map_err(|e| format!("移动转写条目失败：{e}"))?;
        }
        let _ = std::fs::remove_dir(&src);
        report.migrated.push(format!(
            "转写目录并入 {}（{conflicts} 个同名条目保留目标位置的版本）",
            dst.display()
        ));
        return Ok(());
    }
    std::fs::rename(&src, &dst).map_err(|e| format!("移动转写目录失败：{e}"))?;
    report
        .migrated
        .push(format!("转写目录 {}", dst.display()));
    Ok(())
}

/// Rewrites the `project` field of history.jsonl lines matching `old_project`
/// to `new_project`. Unparseable lines are kept untouched, same rule as the
/// history filter used by deletion. The file is left alone when nothing
/// matched.
fn rewrite_history_project(
    path: &Path,
    old_project: &str,
    new_project: &str,
) -> Result<usize, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("读取 history.jsonl 失败：{error}")),
    };
    let mut kept = String::with_capacity(content.len());
    let mut rewritten = 0usize;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut rewritten_line = false;
        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(line) {
            if value.get("project").and_then(|p| p.as_str()) == Some(old_project) {
                value["project"] = serde_json::Value::String(new_project.to_string());
                kept.push_str(&value.to_string());
                kept.push('\n');
                rewritten += 1;
                rewritten_line = true;
            }
        }
        if !rewritten_line {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    if rewritten == 0 {
        return Ok(0);
    }
    crate::file_transaction::write_private_text_atomic(
        path,
        kept.as_bytes(),
        "Claude 历史 history.jsonl",
    )?;
    Ok(rewritten)
}

/// Moves the `projects["<old>"]` entry of ~/.claude.json to
/// `projects["<new>"]`. When the destination entry already exists, fields are
/// merged with the destination winning, and the stale old key is removed.
fn relocate_claude_json_entry(
    path: &Path,
    old_project: &str,
    new_project: &str,
) -> Result<bool, String> {
    edit_claude_json(path, |value| {
        let Some(projects) = value
            .get_mut("projects")
            .and_then(|projects| projects.as_object_mut())
        else {
            return false;
        };
        let Some(entry) = projects.remove(old_project) else {
            return false;
        };
        match projects.get_mut(new_project) {
            Some(existing) => {
                if let (Some(existing_obj), Some(entry_obj)) =
                    (existing.as_object_mut(), entry.as_object())
                {
                    for (key, entry_value) in entry_obj {
                        existing_obj
                            .entry(key.clone())
                            .or_insert(entry_value.clone());
                    }
                }
                true
            }
            None => {
                projects.insert(new_project.to_string(), entry);
                true
            }
        }
    })
}

fn relocate_project_native_at(
    claude_dir: &Path,
    claude_json: &Path,
    old_project: &str,
    new_project: &str,
) -> Result<NativeRelocateReport, String> {
    let session_ids = collect_project_session_ids(claude_dir, old_project);
    guard_no_live_sessions(claude_dir, &session_ids)?;

    let mut report = NativeRelocateReport::default();
    relocate_transcripts_dir(claude_dir, old_project, new_project, &mut report)?;

    let rewritten = rewrite_history_project(
        &claude_dir.join("history.jsonl"),
        old_project,
        new_project,
    )?;
    if rewritten > 0 {
        report
            .migrated
            .push(format!("history.jsonl 中的 {rewritten} 行 prompt 历史"));
    }

    if relocate_claude_json_entry(claude_json, old_project, new_project)? {
        report
            .migrated
            .push(format!(".claude.json 项目条目 → projects[\"{new_project}\"]"));
    }

    if report.migrated.is_empty() {
        report.warnings.push(
            "Claude 原生数据里没有旧路径的条目（可能已被清理），仅更新了应用内的项目记录"
                .to_string(),
        );
    }
    Ok(report)
}

#[tauri::command]
pub fn relocate_claude_project_native(
    old_project_path: String,
    new_project_path: String,
) -> Result<NativeRelocateReport, String> {
    let old_project = validate_native_project_path(&old_project_path)?;
    let new_project = validate_native_project_path(&new_project_path)?;
    if new_project == old_project {
        return Ok(NativeRelocateReport::default());
    }
    let new_metadata = std::fs::metadata(&new_project)
        .map_err(|_| format!("新路径不存在或无法访问：{new_project}"))?;
    if !new_metadata.is_dir() {
        return Err(format!("新路径不是目录：{new_project}"));
    }

    let report = relocate_project_native_at(
        &claude_dir(),
        &claude_json_path(),
        &old_project,
        &new_project,
    )?;
    invalidate_history_cache();
    Ok(report)
}

#[tauri::command]
pub fn delete_claude_project_native(project_path: String) -> Result<NativeDeleteReport, String> {
    let project_path = validate_native_project_path(&project_path)?;
    let report = delete_project_native_at(&claude_dir(), &claude_json_path(), &project_path)?;
    invalidate_history_cache();
    Ok(report)
}

#[tauri::command]
pub fn delete_claude_session_native(
    project_path: String,
    session_id: String,
) -> Result<NativeDeleteReport, String> {
    let project_path = validate_native_project_path(&project_path)?;
    validate_native_session_id(&session_id)?;
    let report =
        delete_session_native_at(&claude_dir(), &claude_json_path(), &project_path, &session_id)?;
    invalidate_history_cache();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn shared_history_parser_preserves_project_and_session_rules() {
        let input = concat!(
            "{\"project\":\"D:/alpha\",\"sessionId\":\"s1\",\"timestamp\":100,\"display\":\"old\"}\n",
            "not-json\n",
            "{\"project\":\"D:/beta\",\"sessionId\":\"s2\",\"timestamp\":\"300.9\",\"display\":\"beta\"}\n",
            "{\"project\":\"D:/alpha\",\"sessionId\":\"s1\",\"timestamp\":200.8,\"display\":\"new\"}\n",
            "{\"project\":\"D:/alpha\",\"sessionId\":\"s3\",\"timestamp\":150,\"display\":\"other\"}\n",
            "{\"project\":\"\",\"sessionId\":\"empty-project\",\"timestamp\":400}\n",
            "{\"project\":\"D:/ignored\",\"sessionId\":\"\",\"timestamp\":50}\n",
        );

        let snapshot = parse_history(Cursor::new(input));

        assert_eq!(
            snapshot.recent_projects,
            vec!["D:/beta", "D:/alpha", "D:/ignored"]
        );
        assert_eq!(
            snapshot.sessions_by_project.get("D:/alpha"),
            Some(&vec![
                SessionEntry {
                    id: "s1".to_string(),
                    display: "new".to_string(),
                    ts: 200,
                    title: None,
                },
                SessionEntry {
                    id: "s3".to_string(),
                    display: "other".to_string(),
                    ts: 150,
                    title: None,
                },
            ])
        );
        assert_eq!(
            snapshot.sessions_by_project.get(""),
            Some(&vec![SessionEntry {
                id: "empty-project".to_string(),
                display: String::new(),
                ts: 400,
                title: None,
            }])
        );
    }

    #[test]
    fn equal_timestamp_keeps_the_later_display_value() {
        let input = concat!(
            "{\"project\":\"p\",\"sessionId\":\"s\",\"timestamp\":10,\"display\":\"first\"}\n",
            "{\"project\":\"p\",\"sessionId\":\"s\",\"timestamp\":10,\"display\":\"second\"}\n",
        );

        let snapshot = parse_history(Cursor::new(input));

        assert_eq!(snapshot.sessions_by_project["p"][0].display, "second");
    }

    #[test]
    fn history_cache_reloads_when_file_revision_changes() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("history.jsonl");
        let cache = Mutex::new(None);
        std::fs::write(
            &path,
            "{\"project\":\"first\",\"sessionId\":\"s1\",\"timestamp\":1}\n",
        )
        .expect("write first history");

        let first = load_history_snapshot_from_path(&path, &cache).expect("first snapshot");
        std::fs::write(
            &path,
            "{\"project\":\"second-longer\",\"sessionId\":\"s2\",\"timestamp\":2}\n",
        )
        .expect("write second history");
        let second = load_history_snapshot_from_path(&path, &cache).expect("second snapshot");

        assert_eq!(first.recent_projects, vec!["first"]);
        assert_eq!(second.recent_projects, vec!["second-longer"]);
    }

    #[test]
    fn encode_project_dir_replaces_non_alphanumerics() {
        assert_eq!(encode_project_dir("D:\\project\\cc-launcher"), "D--project-cc-launcher");
        assert_eq!(encode_project_dir("C:\\Users\\30919"), "C--Users-30919");
        assert_eq!(encode_project_dir("/home/paul/foo"), "-home-paul-foo");
        // Non-ASCII characters map one-to-one to dashes (verified against the
        // directories Claude Code actually creates).
        assert_eq!(encode_project_dir("D:\\work\\体感游戏调研"), "D--work-------");
    }

    #[test]
    fn read_session_title_picks_last_ai_title_line() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("s1.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"user\",\"sessionId\":\"s1\"}\n",
                "{\"type\":\"ai-title\",\"aiTitle\":\"old title\",\"sessionId\":\"s1\"}\n",
                "not-json\n",
                "{\"type\":\"ai-title\",\"aiTitle\":\"new title\",\"sessionId\":\"s1\"}\n",
                "{\"type\":\"ai-title\",\"aiTitle\":\"  \",\"sessionId\":\"s1\"}\n",
                "{\"type\":\"summary\",\"summary\":\"nope\",\"sessionId\":\"s1\"}\n",
            ),
        )
        .expect("write session file");

        assert_eq!(read_session_title(&path).as_deref(), Some("new title"));
    }

    #[test]
    fn read_session_title_missing_or_titleless_file_is_none() {
        let directory = tempfile::tempdir().expect("temp dir");
        assert_eq!(read_session_title(&directory.path().join("absent.jsonl")), None);

        let path = directory.path().join("plain.jsonl");
        std::fs::write(&path, "{\"type\":\"user\",\"sessionId\":\"s2\"}\n").expect("write file");
        assert_eq!(read_session_title(&path), None);
    }

    #[test]
    fn read_session_title_falls_back_to_full_scan() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("big.jsonl");
        // Title in the first half, padding beyond the tail window afterwards.
        let mut content = String::from("{\"type\":\"ai-title\",\"aiTitle\":\"early\",\"sessionId\":\"s3\"}\n");
        content.push_str(&"x".repeat(TITLE_SCAN_WINDOW as usize + 64));
        content.push('\n');
        std::fs::write(&path, content).expect("write padded file");

        assert_eq!(read_session_title(&path).as_deref(), Some("early"));
    }

    #[test]
    fn enrich_titles_reads_per_session_files() {
        let directory = tempfile::tempdir().expect("temp dir");
        let project_dir = directory.path().join("D--project-foo");
        std::fs::create_dir_all(&project_dir).expect("create project dir");
        std::fs::write(
            project_dir.join("s1.jsonl"),
            "{\"type\":\"ai-title\",\"aiTitle\":\"官方标题\",\"sessionId\":\"s1\"}\n",
        )
        .expect("write s1");

        let sessions = vec![
            SessionEntry {
                id: "s1".to_string(),
                display: "prompt one".to_string(),
                ts: 1,
                title: None,
            },
            SessionEntry {
                id: "missing".to_string(),
                display: "prompt two".to_string(),
                ts: 2,
                title: None,
            },
        ];

        let enriched = enrich_claude_session_titles(&sessions, "D:\\project\\foo", directory.path());
        assert_eq!(enriched[0].title.as_deref(), Some("官方标题"));
        assert_eq!(enriched[0].display, "prompt one");
        assert_eq!(enriched[1].title, None);
    }

    // -----------------------------------------------------------------------
    // Native deletion
    // -----------------------------------------------------------------------

    fn write_native_delete_fixture(claude: &std::path::Path) {
        let encoded = encode_project_dir("D:\\work\\demo");
        let other_encoded = encode_project_dir("D:\\work\\other");
        std::fs::create_dir_all(claude.join("projects").join(&encoded)).expect("project dir");
        std::fs::create_dir_all(claude.join("projects").join(&other_encoded)).expect("other dir");
        std::fs::create_dir_all(claude.join("file-history").join("sid-aaa")).expect("fh aaa");
        std::fs::create_dir_all(claude.join("file-history").join("sid-bbb")).expect("fh bbb");
        std::fs::create_dir_all(claude.join("sessions")).expect("sessions dir");
        std::fs::write(
            claude.join("projects").join(&encoded).join("sid-aaa.jsonl"),
            "{\"type\":\"ai-title\",\"aiTitle\":\"t\",\"sessionId\":\"sid-aaa\"}\n",
        )
        .expect("transcript aaa");
        std::fs::write(
            claude.join("projects").join(&encoded).join("sid-bbb.jsonl"),
            "{\"type\":\"ai-title\",\"aiTitle\":\"t\",\"sessionId\":\"sid-bbb\"}\n",
        )
        .expect("transcript bbb");
        std::fs::write(
            claude
                .join("projects")
                .join(&other_encoded)
                .join("sid-ccc.jsonl"),
            "{\"type\":\"user\",\"sessionId\":\"sid-ccc\"}\n",
        )
        .expect("transcript ccc");
        std::fs::write(
            claude.join("history.jsonl"),
            concat!(
                "{\"project\":\"D:\\\\work\\\\demo\",\"sessionId\":\"sid-aaa\",\"timestamp\":1}\n",
                "not-json\n",
                "{\"project\":\"D:\\\\work\\\\other\",\"sessionId\":\"sid-ccc\",\"timestamp\":2}\n",
                "{\"project\":\"D:\\\\work\\\\demo\",\"sessionId\":\"sid-bbb\",\"timestamp\":3}\n",
                "{\"project\":\"D:\\\\work\\\\demo\",\"sessionId\":\"sid-aaa\",\"timestamp\":4}\n",
            ),
        )
        .expect("history");
        std::fs::write(
            claude.join(".claude.json"),
            concat!(
                "{\"userID\":\"u1\",",
                "\"projects\":{",
                "\"D:\\\\work\\\\demo\":{\"lastSessionId\":\"sid-aaa\",\"allowedTools\":[\"Bash\"]},",
                "\"D:\\\\work\\\\other\":{\"lastSessionId\":\"sid-ccc\"}",
                "}}",
            ),
        )
        .expect("claude.json");
    }

    #[test]
    fn filter_history_lines_drops_matching_and_keeps_bad_lines() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("history.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"project\":\"a\",\"sessionId\":\"s1\"}\n",
                "not-json\n",
                "{\"project\":\"b\",\"sessionId\":\"s2\"}\n",
            ),
        )
        .expect("write history");

        let dropped = filter_history_lines(&path, |value| {
            value.get("project").and_then(|p| p.as_str()) == Some("a")
        })
        .expect("filter");

        assert_eq!(dropped, 1);
        let rewritten = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(rewritten, "not-json\n{\"project\":\"b\",\"sessionId\":\"s2\"}\n");
    }

    #[test]
    fn filter_history_lines_no_match_does_not_rewrite() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("history.jsonl");
        let original = "{\"project\":\"a\"}\n";
        std::fs::write(&path, original).expect("write history");

        let dropped = filter_history_lines(&path, |_| false).expect("filter");
        assert_eq!(dropped, 0);
        assert_eq!(std::fs::read_to_string(&path).expect("read back"), original);
        // No .bak sidecar when nothing was written.
        assert!(!directory.path().join("history.jsonl.bak").exists());
    }

    #[test]
    fn validate_native_session_id_rejects_traversal() {
        assert!(validate_native_session_id("92d153c1-9855-4c08-b2ce-c9f6b67a88df").is_ok());
        assert!(validate_native_session_id("../etc").is_err());
        assert!(validate_native_session_id("abc/def").is_err());
        assert!(validate_native_session_id("abc\\def").is_err());
        assert!(validate_native_session_id("short").is_err());
        assert!(validate_native_session_id("").is_err());
    }

    #[test]
    fn validate_native_project_path_rejects_empty_and_root() {
        assert!(validate_native_project_path("  ").is_err());
        assert!(validate_native_project_path("/").is_err());
        assert!(validate_native_project_path("\\\\").is_err());
        assert_eq!(
            validate_native_project_path("  D:\\work\\demo  ").expect("valid"),
            "D:\\work\\demo"
        );
    }

    #[test]
    fn delete_project_native_removes_all_official_purge_items() {
        let directory = tempfile::tempdir().expect("temp dir");
        let claude = directory.path().join(".claude");
        write_native_delete_fixture(&claude);
        let claude_json = claude.join(".claude.json");

        let report = delete_project_native_at(&claude, &claude_json, "D:\\work\\demo")
            .expect("delete project");

        // Transcripts dir and per-session file-history are gone.
        assert!(!claude.join("projects").join(encode_project_dir("D:\\work\\demo")).exists());
        assert!(!claude.join("file-history").join("sid-aaa").exists());
        assert!(!claude.join("file-history").join("sid-bbb").exists());
        // history.jsonl keeps other projects and unparseable lines.
        let history = std::fs::read_to_string(claude.join("history.jsonl")).expect("history");
        assert_eq!(
            history,
            "not-json\n{\"project\":\"D:\\\\work\\\\other\",\"sessionId\":\"sid-ccc\",\"timestamp\":2}\n"
        );
        // .claude.json drops only the deleted project's entry.
        let config: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&claude_json).expect("claude.json"),
        )
        .expect("parse claude.json");
        assert_eq!(config.get("userID").and_then(|v| v.as_str()), Some("u1"));
        let projects = config.get("projects").expect("projects");
        assert!(projects.get("D:\\work\\demo").is_none());
        assert!(projects.get("D:\\work\\other").is_some());
        // Other project's transcript survives.
        assert!(claude
            .join("projects")
            .join(encode_project_dir("D:\\work\\other"))
            .join("sid-ccc.jsonl")
            .exists());
        assert!(!report.deleted.is_empty());
    }

    #[test]
    fn delete_session_native_removes_transcript_history_and_last_session_id() {
        let directory = tempfile::tempdir().expect("temp dir");
        let claude = directory.path().join(".claude");
        write_native_delete_fixture(&claude);
        let claude_json = claude.join(".claude.json");

        delete_session_native_at(&claude, &claude_json, "D:\\work\\demo", "sid-aaa")
            .expect("delete session");

        let project_dir = claude.join("projects").join(encode_project_dir("D:\\work\\demo"));
        assert!(!project_dir.join("sid-aaa.jsonl").exists());
        // The sibling session of the same project survives.
        assert!(project_dir.join("sid-bbb.jsonl").exists());
        assert!(!claude.join("file-history").join("sid-aaa").exists());
        assert!(claude.join("file-history").join("sid-bbb").exists());

        let history = std::fs::read_to_string(claude.join("history.jsonl")).expect("history");
        assert!(!history.contains("sid-aaa"));
        assert!(history.contains("sid-bbb"));
        assert!(history.contains("not-json"));

        let config: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&claude_json).expect("claude.json"),
        )
        .expect("parse claude.json");
        let demo = config
            .get("projects")
            .and_then(|p| p.get("D:\\work\\demo"))
            .expect("demo entry");
        assert!(demo.get("lastSessionId").is_none());
        // Unknown fields of the same entry are preserved.
        assert!(demo.get("allowedTools").is_some());
    }

    #[test]
    fn delete_refused_while_session_process_is_alive() {
        let directory = tempfile::tempdir().expect("temp dir");
        let claude = directory.path().join(".claude");
        write_native_delete_fixture(&claude);
        std::fs::write(
            claude.join("sessions").join("12345.json"),
            format!(
                "{{\"sessionId\":\"sid-aaa\",\"pid\":{}}}",
                std::process::id()
            ),
        )
        .expect("live session marker");

        let error = delete_session_native_at(
            &claude,
            &claude.join(".claude.json"),
            "D:\\work\\demo",
            "sid-aaa",
        )
        .expect_err("live session must block deletion");
        assert!(error.contains("仍在运行中"), "unexpected error: {error}");
        // Nothing was deleted.
        assert!(claude
            .join("projects")
            .join(encode_project_dir("D:\\work\\demo"))
            .join("sid-aaa.jsonl")
            .exists());
        assert!(std::fs::read_to_string(claude.join("history.jsonl"))
            .expect("history")
            .contains("sid-aaa"));
    }

    #[test]
    fn delete_proceeds_when_session_marker_pid_is_dead() {
        let directory = tempfile::tempdir().expect("temp dir");
        let claude = directory.path().join(".claude");
        write_native_delete_fixture(&claude);
        std::fs::write(
            claude.join("sessions").join("99999.json"),
            "{\"sessionId\":\"sid-aaa\",\"pid\":4294967294}",
        )
        .expect("stale session marker");

        delete_session_native_at(
            &claude,
            &claude.join(".claude.json"),
            "D:\\work\\demo",
            "sid-aaa",
        )
        .expect("stale marker must not block");
        assert!(!claude
            .join("projects")
            .join(encode_project_dir("D:\\work\\demo"))
            .join("sid-aaa.jsonl")
            .exists());
    }

    // -----------------------------------------------------------------------
    // Native relocation
    // -----------------------------------------------------------------------

    #[test]
    fn relocate_project_moves_transcripts_history_and_claude_json_entry() {
        let directory = tempfile::tempdir().expect("temp dir");
        let claude = directory.path().join(".claude");
        write_native_delete_fixture(&claude);
        let claude_json = claude.join(".claude.json");

        let report = relocate_project_native_at(
            &claude,
            &claude_json,
            "D:\\work\\demo",
            "D:\\work\\demo-renamed",
        )
        .expect("relocate project");

        // Transcripts moved to the new encoded directory, sibling project untouched.
        let new_dir = claude
            .join("projects")
            .join(encode_project_dir("D:\\work\\demo-renamed"));
        assert!(new_dir.join("sid-aaa.jsonl").exists());
        assert!(new_dir.join("sid-bbb.jsonl").exists());
        assert!(!claude
            .join("projects")
            .join(encode_project_dir("D:\\work\\demo"))
            .exists());
        assert!(claude
            .join("projects")
            .join(encode_project_dir("D:\\work\\other"))
            .join("sid-ccc.jsonl")
            .exists());
        // file-history is session-keyed and stays put.
        assert!(claude.join("file-history").join("sid-aaa").exists());

        // history.jsonl lines were rewritten, other projects kept.
        let history = std::fs::read_to_string(claude.join("history.jsonl")).expect("history");
        assert!(history.contains("D:\\\\work\\\\demo-renamed"));
        assert!(!history.contains("\"project\":\"D:\\\\work\\\\demo\""));
        assert!(history.contains("sid-ccc"));
        assert!(history.contains("not-json"));

        // .claude.json entry moved with its fields.
        let config: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&claude_json).expect("claude.json"),
        )
        .expect("parse claude.json");
        assert!(config
            .get("projects")
            .and_then(|p| p.get("D:\\work\\demo"))
            .is_none());
        let moved = config
            .get("projects")
            .and_then(|p| p.get("D:\\work\\demo-renamed"))
            .expect("moved entry");
        assert_eq!(moved.get("lastSessionId").and_then(|v| v.as_str()), Some("sid-aaa"));
        assert!(moved.get("allowedTools").is_some());
        assert!(!report.migrated.is_empty());
    }

    #[test]
    fn relocate_project_merges_into_existing_destination() {
        let directory = tempfile::tempdir().expect("temp dir");
        let claude = directory.path().join(".claude");
        write_native_delete_fixture(&claude);
        let claude_json = claude.join(".claude.json");
        // The destination already has its own transcript dir and .claude.json
        // entry (e.g. a session was already run at the new path).
        let dst_dir = claude
            .join("projects")
            .join(encode_project_dir("D:\\work\\demo-renamed"));
        std::fs::create_dir_all(&dst_dir).expect("dst dir");
        std::fs::write(
            dst_dir.join("sid-bbb.jsonl"),
            "{\"type\":\"ai-title\",\"aiTitle\":\"new copy\",\"sessionId\":\"sid-bbb\"}\n",
        )
        .expect("dst transcript");
        std::fs::write(
            dst_dir.join("sid-ddd.jsonl"),
            "{\"type\":\"user\",\"sessionId\":\"sid-ddd\"}\n",
        )
        .expect("dst-only transcript");
        edit_claude_json(&claude_json, |value| {
            value["projects"]["D:\\work\\demo-renamed"] =
                serde_json::json!({"lastSessionId": "sid-ddd", "customNew": true});
            true
        })
        .expect("seed dst entry");

        relocate_project_native_at(
            &claude,
            &claude_json,
            "D:\\work\\demo",
            "D:\\work\\demo-renamed",
        )
        .expect("relocate onto existing destination");

        // Destination keeps its own copy of the conflicting sid-bbb.
        let content =
            std::fs::read_to_string(dst_dir.join("sid-bbb.jsonl")).expect("dst sid-bbb");
        assert!(content.contains("new copy"));
        // Non-conflicting transcript arrives from the source.
        assert!(dst_dir.join("sid-aaa.jsonl").exists());
        // Destination-only transcript survives.
        assert!(dst_dir.join("sid-ddd.jsonl").exists());

        let config: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&claude_json).expect("claude.json"),
        )
        .expect("parse claude.json");
        let merged = config
            .get("projects")
            .and_then(|p| p.get("D:\\work\\demo-renamed"))
            .expect("merged entry");
        // Destination wins on conflicting keys, source fills the gaps.
        assert_eq!(merged.get("lastSessionId").and_then(|v| v.as_str()), Some("sid-ddd"));
        assert_eq!(merged.get("customNew").and_then(|v| v.as_bool()), Some(true));
        assert!(merged.get("allowedTools").is_some());
        assert!(config
            .get("projects")
            .and_then(|p| p.get("D:\\work\\demo"))
            .is_none());
    }

    #[test]
    fn relocate_refused_while_session_process_is_alive() {
        let directory = tempfile::tempdir().expect("temp dir");
        let claude = directory.path().join(".claude");
        write_native_delete_fixture(&claude);
        std::fs::write(
            claude.join("sessions").join("12345.json"),
            format!(
                "{{\"sessionId\":\"sid-aaa\",\"pid\":{}}}",
                std::process::id()
            ),
        )
        .expect("live session marker");

        let error = relocate_project_native_at(
            &claude,
            &claude.join(".claude.json"),
            "D:\\work\\demo",
            "D:\\work\\demo-renamed",
        )
        .expect_err("live session must block relocation");
        assert!(error.contains("仍在运行中"), "unexpected error: {error}");
        assert!(claude
            .join("projects")
            .join(encode_project_dir("D:\\work\\demo"))
            .join("sid-aaa.jsonl")
            .exists());
    }

    #[test]
    fn relocate_without_native_data_succeeds_with_warning() {
        let directory = tempfile::tempdir().expect("temp dir");
        let claude = directory.path().join(".claude");
        std::fs::create_dir_all(&claude).expect("claude dir");
        let claude_json = claude.join(".claude.json");

        let report =
            relocate_project_native_at(&claude, &claude_json, "D:\\gone", "D:\\fresh")
                .expect("relocate with nothing to move");
        assert!(report.migrated.is_empty());
        assert_eq!(report.warnings.len(), 1);
    }
}
