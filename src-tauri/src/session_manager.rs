use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufRead;
use std::path::PathBuf;
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
    /// session files on demand.
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
fn encode_project_dir(project: &str) -> String {
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
}
