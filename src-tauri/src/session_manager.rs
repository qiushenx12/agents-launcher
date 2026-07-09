use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::BufRead;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Serialize, Deserialize, Clone)]
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

fn is_history_change(kind: &notify::EventKind) -> bool {
    matches!(
        kind,
        notify::EventKind::Create(_)
            | notify::EventKind::Modify(_)
            | notify::EventKind::Remove(_)
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
        let mut watcher = match notify::recommended_watcher(
            move |result: notify::Result<notify::Event>| {
                let Ok(event) = result else {
                    return;
                };

                if !is_history_change(&event.kind) {
                    return;
                }

                let matches_history = event.paths.iter().any(|path| {
                    path == &event_history
                        || path.file_name().map(|name| name == history_name).unwrap_or(false)
                });

                if matches_history {
                    let _ = event_app.emit(
                        "claude_history_changed",
                        ClaudeHistoryChangedPayload {
                            path: event_history.to_string_lossy().to_string(),
                        },
                    );
                }
            },
        ) {
            Ok(watcher) => watcher,
            Err(_) => return,
        };

        if notify::Watcher::watch(&mut watcher, &history_dir, notify::RecursiveMode::NonRecursive).is_err() {
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

#[tauri::command]
pub fn load_claude_sessions(target_dir: String) -> Result<Vec<SessionEntry>, String> {
    let path = history_path();

    // Map: session_id -> (ts, display)
    let mut seen: HashMap<String, (i64, String)> = HashMap::new();

    if path.exists() {
        let file = std::fs::File::open(&path)
            .map_err(|e| format!("Failed to open history file: {}", e))?;
        let reader = std::io::BufReader::new(file);

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            let entry: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let project = entry.get("project").and_then(|v| v.as_str()).unwrap_or("");
            let session_id = entry
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let ts = entry.get("timestamp").map(parse_ts).unwrap_or(0);
            let display = entry
                .get("display")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if project == target_dir && !session_id.is_empty() {
                seen.entry(session_id.to_string())
                    .and_modify(|entry| {
                        if ts >= entry.0 {
                            entry.0 = ts;
                            entry.1 = display.clone();
                        }
                    })
                    .or_insert((ts, display));
            }
        }
    }

    let mut sessions: Vec<SessionEntry> = seen
        .into_iter()
        .map(|(id, (ts, display))| SessionEntry { id, display, ts })
        .collect();

    sessions.sort_by(|a, b| b.ts.cmp(&a.ts));
    Ok(sessions)
}

#[tauri::command]
pub fn load_claude_recent_projects() -> Result<Vec<String>, String> {
    let path = history_path();
    let mut seen: HashMap<String, i64> = HashMap::new();

    if path.exists() {
        let file = std::fs::File::open(&path)
            .map_err(|e| format!("Failed to open history file: {}", e))?;
        let reader = std::io::BufReader::new(file);

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            let entry: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let project = entry.get("project").and_then(|v| v.as_str()).unwrap_or("");
            let ts = entry.get("timestamp").map(parse_ts).unwrap_or(0);

            if !project.is_empty() {
                seen.entry(project.to_string())
                    .and_modify(|existing| {
                        if ts > *existing {
                            *existing = ts;
                        }
                    })
                    .or_insert(ts);
            }
        }
    }

    let mut paths: Vec<(String, i64)> = seen.into_iter().collect();
    paths.sort_by(|a, b| b.1.cmp(&a.1));
    let result = paths.into_iter().take(10).map(|(p, _)| p).collect();
    Ok(result)
}
