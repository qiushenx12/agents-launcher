use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    pub id: String,
    pub display: String,
    pub ts: i64,
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
struct ClaudeHistorySnapshot {
    recent_projects: Vec<String>,
    sessions_by_project: HashMap<String, Vec<SessionEntry>>,
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

fn parse_history(reader: impl BufRead) -> ClaudeHistorySnapshot {
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
                .map(|(id, (ts, display))| SessionEntry { id, display, ts })
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
    Ok(load_history_snapshot()?
        .sessions_by_project
        .get(&target_dir)
        .cloned()
        .unwrap_or_default())
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
                },
                SessionEntry {
                    id: "s3".to_string(),
                    display: "other".to_string(),
                    ts: 150,
                },
            ])
        );
        assert_eq!(
            snapshot.sessions_by_project.get(""),
            Some(&vec![SessionEntry {
                id: "empty-project".to_string(),
                display: String::new(),
                ts: 400,
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
}
