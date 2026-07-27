use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::TcpListener as StdTcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::to_bytes;
use axum::extract::{Path as AxumPath, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use uuid::Uuid;

const MAX_EVENTS_IN_MEMORY: usize = 500;
const MAX_HOOK_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_STATUSLINE_BODY_BYTES: usize = 256 * 1024;
const MAX_STATUSLINE_OUTPUT_BYTES: usize = 256 * 1024;
const MAX_TRANSCRIPT_USAGE_TAIL_BYTES: u64 = 1024 * 1024;
const MAX_UI_VALUE_CHARS: usize = 256 * 1024;
const MAX_TERMINAL_LOG_BYTES: u64 = 5 * 1024 * 1024;
const MAX_HOOK_LOG_BYTES: u64 = 10 * 1024 * 1024;
const MAX_DIAGNOSTIC_LOG_BYTES: u64 = 1024 * 1024;
const MAX_FORENSIC_LOG_BYTES: u64 = 1024 * 1024;
const MAX_TRANSCRIPT_TAIL_BYTES: u64 = 8 * 1024 * 1024;
const MAX_HISTORY_EVENTS: usize = 200;
const MAX_HISTORY_TEXT_CHARS: usize = 64 * 1024;
const SCREEN_QUIET_WINDOW: Duration = Duration::from_millis(150);
const SCREEN_MAX_WINDOW: Duration = Duration::from_secs(1);
const SECRET_ASSIGNMENT_PATTERN: &str = r#"(?i)(["']?(?:(?:x[_-]?)?api[_-]?key|access[_-]?token|auth[_-]?token|token|authorization|cookie|set[_-]?cookie|secret|password|credential)["']?\s*[:=]\s*)(?:"[^"\r\n]*"|'[^'\r\n]*'|[^\r\n,}]+)"#;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAgentEvent {
    pub id: String,
    pub sequence: u64,
    pub capture_id: String,
    pub tab_id: Option<u32>,
    pub event_name: String,
    pub received_at: String,
    pub payload: Value,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeObserverStatus {
    pub tab_id: u32,
    pub status_revision: u64,
    pub capture_id: Option<String>,
    pub available: bool,
    pub active: bool,
    pub degraded_reason: Option<String>,
    pub log_dir: Option<String>,
    pub terminal_prompt: Option<ClaudeTerminalPrompt>,
    pub activity_status: Option<ClaudeActivityStatus>,
    pub current_model: Option<String>,
    pub permission_mode: Option<String>,
    pub context_usage: Option<ClaudeContextUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ClaudeTerminalPrompt {
    WorkspaceTrust { path: String },
    PluginInstall {
        plugin_name: String,
        prompt: String,
        options: Vec<String>,
    },
    ModelSwitchConfirm {
        prompt: String,
        options: Vec<String>,
        selected_index: usize,
    },
    PlanApproval {
        prompt: String,
        options: Vec<String>,
        selected_index: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeActivityStatus {
    pub label: String,
    pub elapsed: Option<String>,
    pub token_direction: Option<String>,
    pub token_count: Option<String>,
    pub phase: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeObserverSnapshot {
    pub tab_id: u32,
    pub status_revision: u64,
    pub capture_id: Option<String>,
    pub available: bool,
    pub active: bool,
    pub degraded_reason: Option<String>,
    pub log_dir: Option<String>,
    pub events: Vec<ClaudeAgentEvent>,
    pub terminal_log: String,
    pub terminal_prompt: Option<ClaudeTerminalPrompt>,
    pub activity_status: Option<ClaudeActivityStatus>,
    pub current_model: Option<String>,
    pub permission_mode: Option<String>,
    pub context_usage: Option<ClaudeContextUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeContextUsage {
    pub used_percentage: u8,
    pub remaining_percentage: u8,
    pub used_tokens: Option<u64>,
    pub context_window_size: Option<u64>,
    pub source: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudePromptSubmissionBaseline {
    pub capture_id: String,
    pub event_sequence: u64,
    pub transcript_len: Option<u64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeTerminalLogResult {
    pub text: String,
    pub log_dir: String,
    pub historical: bool,
}

pub struct PreparedCapture {
    pub capture_id: String,
    pub plugin_dir: PathBuf,
    pub settings_path: PathBuf,
    pub token: String,
}

struct HookEnvelope {
    capture_id: String,
    body: Value,
}

struct ScreenCapture {
    parser: vt100::Parser,
    forensic_parser: vte::Parser,
    forensic: ForensicTextCollector,
    dirty: bool,
    first_dirty_at: Option<Instant>,
    last_change_at: Option<Instant>,
    last_screen: Vec<String>,
    sequence: u64,
}

#[derive(Default)]
struct ForensicTextCollector {
    current: String,
    pending: VecDeque<String>,
    recent: VecDeque<String>,
    suppressed: usize,
    last_activity_signature: Option<String>,
}

impl ForensicTextCollector {
    fn finish_line(&mut self) {
        let line = self.current.trim().to_string();
        self.current.clear();
        if line.is_empty() {
            return;
        }
        if let Some(activity) = parse_claude_activity_status_line(&line) {
            let signature = claude_activity_signature(&activity);
            if self.last_activity_signature.as_deref() == Some(signature.as_str()) {
                return;
            }
            self.last_activity_signature = Some(signature);
        }
        if self.recent.iter().any(|recent| recent == &line) {
            self.suppressed += 1;
            return;
        }
        self.recent.push_back(line.clone());
        while self.recent.len() > 256 {
            self.recent.pop_front();
        }
        self.pending.push_back(line);
        while self.pending.len() > 1_024 {
            self.pending.pop_front();
        }
    }

    fn drain(&mut self, force: bool) -> Vec<String> {
        if force {
            self.finish_line();
        }
        let mut lines: Vec<String> = self.pending.drain(..).collect();
        if self.suppressed > 0 {
            lines.push(format!("[近期重复的终端文本已折叠 {} 次]", self.suppressed));
            self.suppressed = 0;
        }
        lines
    }

    fn reset_activity_dedup(&mut self) {
        self.last_activity_signature = None;
    }
}

impl vte::Perform for ForensicTextCollector {
    fn print(&mut self, character: char) {
        if self.current.len() < 8 * 1024 {
            self.current.push(character);
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | b'\r' => self.finish_line(),
            b'\t' => self.current.push('\t'),
            0x08 => {
                self.current.pop();
            }
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        _params: &vte::Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        if matches!(
            action,
            'A' | 'B' | 'E' | 'F' | 'G' | 'H' | 'J' | 'K' | 'd' | 'f'
        ) {
            self.finish_line();
        }
    }
}

impl ScreenCapture {
    fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows.max(1), cols.max(1), 2_000),
            forensic_parser: vte::Parser::new(),
            forensic: ForensicTextCollector::default(),
            dirty: false,
            first_dirty_at: None,
            last_change_at: None,
            last_screen: Vec::new(),
            sequence: 0,
        }
    }

    fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        for byte in bytes {
            self.forensic_parser.advance(&mut self.forensic, *byte);
        }
        let now = Instant::now();
        self.dirty = true;
        self.first_dirty_at.get_or_insert(now);
        self.last_change_at = Some(now);
    }

    fn take_forensic_lines(&mut self, force: bool) -> Vec<String> {
        self.forensic.drain(force)
    }

    fn reset_forensic_activity_dedup(&mut self) {
        self.forensic.reset_activity_dedup();
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.set_size(rows.max(1), cols.max(1));
        let now = Instant::now();
        self.dirty = true;
        self.first_dirty_at.get_or_insert(now);
        self.last_change_at = Some(now);
    }

    fn should_flush(&self, now: Instant, force: bool) -> bool {
        if !self.dirty {
            return false;
        }
        if force {
            return true;
        }
        let quiet = self
            .last_change_at
            .is_some_and(|last| now.duration_since(last) >= SCREEN_QUIET_WINDOW);
        let overdue = self
            .first_dirty_at
            .is_some_and(|first| now.duration_since(first) >= SCREEN_MAX_WINDOW);
        quiet || overdue
    }

    fn take_stable_update(&mut self, now: Instant, force: bool) -> Option<StableScreenUpdate> {
        if !self.should_flush(now, force) {
            return None;
        }

        self.dirty = false;
        self.first_dirty_at = None;
        self.last_change_at = None;

        let contents = normalize_screen(&self.parser.screen().contents());
        let current_lines: Vec<String> = if contents.is_empty() {
            Vec::new()
        } else {
            contents.lines().map(ToOwned::to_owned).collect()
        };
        let previous_counts = line_counts(&self.last_screen);
        let mut current_seen: HashMap<&str, usize> = HashMap::new();
        let max_rows = current_lines.len().max(self.last_screen.len());
        let mut changed_rows = Vec::new();
        for row in 0..max_rows {
            let current = current_lines.get(row).map(String::as_str).unwrap_or("");
            let previous = self.last_screen.get(row).map(String::as_str).unwrap_or("");
            if current == previous {
                if !current.is_empty() {
                    *current_seen.entry(current).or_default() += 1;
                }
                continue;
            }
            if !current.is_empty() {
                let seen = current_seen.entry(current).or_default();
                *seen += 1;
                let previous_occurrences =
                    previous_counts.get(current).copied().unwrap_or_default();
                // Scrolling and full-screen redraws move unchanged text between rows.
                // Only persist occurrences that are genuinely new to the stable screen.
                if *seen > previous_occurrences {
                    changed_rows.push(ScreenRowDiff {
                        row: row + 1,
                        text: current.to_string(),
                    });
                }
            } else if current_lines.is_empty()
                && !self.last_screen.is_empty()
                && changed_rows.is_empty()
            {
                changed_rows.push(ScreenRowDiff {
                    row: row + 1,
                    text: String::new(),
                });
            }
        }

        self.last_screen = current_lines;
        let diff = if changed_rows.is_empty() {
            None
        } else {
            self.sequence += 1;
            Some(ScreenDiff {
                sequence: self.sequence,
                changed_rows,
                latest_screen: contents.clone(),
            })
        };
        Some(StableScreenUpdate {
            diff,
            latest_screen: contents,
        })
    }

    #[cfg(test)]
    fn take_diff(&mut self, now: Instant, force: bool) -> Option<ScreenDiff> {
        self.take_stable_update(now, force)?.diff
    }
}

struct StableScreenUpdate {
    diff: Option<ScreenDiff>,
    latest_screen: String,
}

struct ScreenRowDiff {
    row: usize,
    text: String,
}

struct ScreenDiff {
    sequence: u64,
    changed_rows: Vec<ScreenRowDiff>,
    latest_screen: String,
}

struct CaptureState {
    capture_id: String,
    token: String,
    tab_id: Option<u32>,
    project_session_id: Option<String>,
    log_dir: PathBuf,
    plugin_dir: PathBuf,
    events: VecDeque<ClaudeAgentEvent>,
    next_event_sequence: u64,
    imported_transcripts: HashSet<String>,
    screen: ScreenCapture,
    secrets: Vec<String>,
    active: bool,
    log_error: Option<String>,
    terminal_prompt: Option<ClaudeTerminalPrompt>,
    activity_status: Option<ClaudeActivityStatus>,
    current_model: Option<String>,
    permission_mode: Option<String>,
    context_usage: Option<ClaudeContextUsage>,
    transcript_path: Option<PathBuf>,
    last_native_context_at: Option<Instant>,
    original_statusline_command: Option<String>,
    launch_env: HashMap<String, String>,
    launch_cwd: Option<PathBuf>,
    terminal_cols: u16,
    terminal_rows: u16,
    activity_expected: bool,
    last_screen_activity_signature: Option<String>,
    session_started: bool,
    status_revision: u64,
}

pub struct ClaudeObserverManager {
    app: AppHandle,
    endpoint: Option<String>,
    init_error: Option<String>,
    captures: Mutex<HashMap<String, CaptureState>>,
    tab_captures: Mutex<HashMap<u32, String>>,
    hook_tx: mpsc::Sender<HookEnvelope>,
    secret_assignment: Regex,
    bearer: Regex,
}

impl ClaudeObserverManager {
    pub fn start(app: AppHandle) -> Arc<Self> {
        let (hook_tx, hook_rx) = mpsc::channel::<HookEnvelope>(64);
        let listener = StdTcpListener::bind(("127.0.0.1", 0));
        let (listener, endpoint, init_error) = match listener {
            Ok(listener) => {
                let result = listener
                    .local_addr()
                    .map(|address| format!("http://127.0.0.1:{}", address.port()));
                match result {
                    Ok(endpoint) => (Some(listener), Some(endpoint), None),
                    Err(error) => (
                        None,
                        None,
                        Some(format!("无法读取 Claude Hook 监听地址：{error}")),
                    ),
                }
            }
            Err(error) => (
                None,
                None,
                Some(format!("无法启动 Claude Hook 本地监听：{error}")),
            ),
        };

        let manager = Arc::new(Self {
            app,
            endpoint,
            init_error,
            captures: Mutex::new(HashMap::new()),
            tab_captures: Mutex::new(HashMap::new()),
            hook_tx,
            secret_assignment: Regex::new(SECRET_ASSIGNMENT_PATTERN)
                .expect("valid secret redaction regex"),
            bearer: Regex::new(r"(?i)(bearer\s+)[A-Za-z0-9._~+/=-]+")
                .expect("valid bearer redaction regex"),
        });

        manager.spawn_hook_worker(hook_rx);
        manager.spawn_flush_worker();
        if let Some(listener) = listener {
            manager.spawn_http_server(listener);
        }
        manager
    }

    fn spawn_http_server(self: &Arc<Self>, listener: StdTcpListener) {
        let manager = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            if listener.set_nonblocking(true).is_err() {
                return;
            }
            let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                return;
            };
            let router = Router::new()
                .route("/hooks/{capture_id}", post(receive_hook))
                .route("/statusline/{capture_id}", post(receive_statusline))
                .with_state(manager);
            let _ = axum::serve(listener, router).await;
        });
    }

    fn spawn_hook_worker(self: &Arc<Self>, mut hook_rx: mpsc::Receiver<HookEnvelope>) {
        let manager = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            while let Some(envelope) = hook_rx.recv().await {
                manager.process_hook(envelope);
            }
        });
    }

    fn spawn_flush_worker(self: &Arc<Self>) {
        let manager = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            loop {
                interval.tick().await;
                manager.flush_screens(false);
            }
        });
    }

    pub fn prepare_capture(
        &self,
        cols: u16,
        rows: u16,
        project_session_id: Option<String>,
        cwd: Option<&str>,
        env: &HashMap<String, String>,
    ) -> Result<PreparedCapture, String> {
        let endpoint = self.endpoint.as_ref().ok_or_else(|| {
            self.init_error
                .clone()
                .unwrap_or_else(|| "Claude Hook 观察器不可用".into())
        })?;
        let capture_id = Uuid::new_v4().to_string();
        let token = Uuid::new_v4().to_string();
        let app_data_dir = app_data_base_dir()?;
        let log_dir = app_data_dir
            .join("terminal_logs")
            .join("claude")
            .join(&capture_id);
        let plugin_dir = app_data_dir
            .join("claude_observer")
            .join("runtime")
            .join(&capture_id);
        fs::create_dir_all(&log_dir)
            .map_err(|error| format!("创建 Claude 日志目录失败：{error}"))?;
        write_observer_plugin(&plugin_dir, endpoint, &capture_id)?;
        let settings_path = plugin_dir.join("observer-settings.json");
        write_statusline_settings(&settings_path, endpoint, &capture_id, &token)?;
        let original_statusline_command =
            resolve_existing_statusline_command(env, cwd).unwrap_or(None);

        let mut secrets = collect_sensitive_values(env);
        secrets.push(token.clone());
        let metadata = json!({
            "captureId": capture_id,
            "projectSessionId": project_session_id,
            "startedAt": Utc::now().to_rfc3339(),
            "formatVersion": 1,
            "terminalLogMode": "stable-screen-diff"
        });
        write_json_pretty(&log_dir.join("metadata.json"), &metadata)
            .map_err(|error| format!("写入 Claude 日志元数据失败：{error}"))?;

        let state = CaptureState {
            capture_id: capture_id.clone(),
            token: token.clone(),
            tab_id: None,
            project_session_id,
            log_dir,
            plugin_dir: plugin_dir.clone(),
            events: VecDeque::with_capacity(MAX_EVENTS_IN_MEMORY + 1),
            next_event_sequence: 1,
            imported_transcripts: HashSet::new(),
            screen: ScreenCapture::new(rows, cols),
            secrets,
            active: true,
            log_error: None,
            terminal_prompt: None,
            activity_status: None,
            current_model: None,
            permission_mode: Some("? for shortcuts".to_string()),
            context_usage: None,
            transcript_path: None,
            last_native_context_at: None,
            original_statusline_command,
            launch_env: env.clone(),
            launch_cwd: cwd.map(PathBuf::from),
            terminal_cols: cols,
            terminal_rows: rows,
            activity_expected: false,
            last_screen_activity_signature: None,
            session_started: false,
            status_revision: 0,
        };
        self.captures
            .lock()
            .map_err(|error| error.to_string())?
            .insert(capture_id.clone(), state);

        Ok(PreparedCapture {
            capture_id,
            plugin_dir,
            settings_path,
            token,
        })
    }

    pub fn bind_capture(&self, capture_id: &str, tab_id: u32) {
        let status = {
            let Ok(mut captures) = self.captures.lock() else {
                return;
            };
            let Some(capture) = captures.get_mut(capture_id) else {
                return;
            };
            capture.tab_id = Some(tab_id);
            if let Ok(mut tab_captures) = self.tab_captures.lock() {
                tab_captures.insert(tab_id, capture_id.to_string());
            }
            let events: Vec<_> = capture.events.iter().cloned().collect();
            for event in events {
                let mut event = event;
                event.tab_id = Some(tab_id);
                let _ = self.app.emit("claude_agent_event", event);
            }
            status_for_capture(capture, true)
        };
        let _ = self.app.emit("claude_observer_status", status);
    }

    pub fn emit_degraded(&self, tab_id: u32, reason: String) {
        let _ = self.app.emit(
            "claude_observer_status",
            ClaudeObserverStatus {
                tab_id,
                status_revision: 0,
                capture_id: None,
                available: false,
                active: true,
                degraded_reason: Some(reason),
                log_dir: None,
                terminal_prompt: None,
                activity_status: None,
                current_model: None,
                permission_mode: Some("? for shortcuts".to_string()),
                context_usage: None,
            },
        );
    }

    pub fn record_pty(&self, capture_id: &str, bytes: &[u8]) {
        let Ok(mut captures) = self.captures.lock() else {
            return;
        };
        if let Some(capture) = captures.get_mut(capture_id) {
            capture.screen.process(bytes);
        }
    }

    pub fn resize(&self, capture_id: &str, cols: u16, rows: u16) {
        let Ok(mut captures) = self.captures.lock() else {
            return;
        };
        if let Some(capture) = captures.get_mut(capture_id) {
            capture.screen.resize(rows, cols);
            capture.terminal_cols = cols;
            capture.terminal_rows = rows;
        }
    }

    pub fn finish_capture(&self, capture_id: &str) {
        self.flush_one(capture_id, true);
        let finish_context = {
            let Ok(mut captures) = self.captures.lock() else {
                return;
            };
            let Some(capture) = captures.get_mut(capture_id) else {
                return;
            };
            if !capture.active {
                return;
            }
            capture.active = false;
            capture.terminal_prompt = None;
            capture.activity_status = None;
            capture.activity_expected = false;
            capture.last_screen_activity_signature = None;
            capture.status_revision = capture.status_revision.saturating_add(1);
            let metadata = json!({
                "captureId": capture.capture_id,
                "projectSessionId": capture.project_session_id,
                "finishedAt": Utc::now().to_rfc3339(),
                "formatVersion": 1,
                "terminalLogMode": "stable-screen-diff",
                "logError": capture.log_error,
            });
            (
                capture.log_dir.clone(),
                capture.plugin_dir.clone(),
                metadata,
            )
        };
        let (log_dir, plugin_dir, mut metadata) = finish_context;
        if let Ok(existing) = fs::read_to_string(log_dir.join("metadata.json")) {
            if let Ok(Value::Object(existing)) = serde_json::from_str::<Value>(&existing) {
                if let Value::Object(target) = &mut metadata {
                    for (key, value) in existing {
                        target.entry(key).or_insert(value);
                    }
                }
            }
        }
        if let Err(error) = write_json_pretty(&log_dir.join("metadata.json"), &metadata) {
            self.record_log_error(capture_id, error.to_string());
        }
        let _ = fs::remove_dir_all(plugin_dir);
        let status = self.captures.lock().ok().and_then(|captures| {
            captures
                .get(capture_id)
                .map(|capture| status_for_capture(capture, true))
        });
        if let Some(status) = status {
            let _ = self.app.emit("claude_observer_status", status);
        }
    }

    pub fn abort_capture(&self, capture_id: &str) {
        let capture = self
            .captures
            .lock()
            .ok()
            .and_then(|mut captures| captures.remove(capture_id));
        if let Some(capture) = capture {
            let _ = fs::remove_dir_all(capture.plugin_dir);
        }
    }

    pub fn release_capture(&self, capture_id: &str, tab_id: u32) {
        if let Ok(mut tab_captures) = self.tab_captures.lock() {
            tab_captures.remove(&tab_id);
        }
        let capture = self
            .captures
            .lock()
            .ok()
            .and_then(|mut captures| captures.remove(capture_id));
        if let Some(capture) = capture {
            let _ = fs::remove_dir_all(capture.plugin_dir);
        }
    }

    fn process_hook(&self, envelope: HookEnvelope) {
        let capture_context = {
            let Ok(captures) = self.captures.lock() else {
                return;
            };
            let Some(capture) = captures.get(&envelope.capture_id) else {
                return;
            };
            (
                capture.capture_id.clone(),
                capture.log_dir.clone(),
                capture.secrets.clone(),
            )
        };
        let (capture_id, log_dir, secrets) = capture_context;
        let redacted = self.redact_value(envelope.body, &secrets);
        let event_name = redacted
            .get("hook_event_name")
            .or_else(|| redacted.get("event_name"))
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .to_string();
        if event_name == "SessionStart" {
            self.import_transcript_history(&capture_id, &redacted, &secrets);
        }
        let received_at = Utc::now().to_rfc3339();
        let mut event = ClaudeAgentEvent {
            id: Uuid::new_v4().to_string(),
            sequence: 0,
            capture_id: capture_id.clone(),
            tab_id: None,
            event_name,
            received_at,
            payload: trim_value_for_ui(redacted.clone()),
        };

        let observer_status = {
            let Ok(mut captures) = self.captures.lock() else {
                return;
            };
            let Some(capture) = captures.get_mut(&capture_id) else {
                return;
            };
            event.sequence = capture.next_event_sequence;
            capture.next_event_sequence += 1;
            event.tab_id = capture.tab_id;
            capture.events.push_back(event.clone());
            while capture.events.len() > MAX_EVENTS_IN_MEMORY {
                capture.events.pop_front();
            }
            let mut status_changed = false;
            if matches!(event.event_name.as_str(), "UserPromptSubmit" | "PreToolUse") {
                capture.activity_expected = true;
            } else if matches!(
                event.event_name.as_str(),
                "Stop" | "StopFailure" | "SessionStart" | "SessionEnd"
            ) {
                capture.activity_expected = false;
            }
            if event.event_name == "SessionStart" {
                capture.session_started = true;
                if capture.terminal_prompt.take().is_some() {
                    status_changed = true;
                }
                if capture.context_usage.take().is_some() {
                    capture.last_native_context_at = None;
                    status_changed = true;
                }
            }
            if matches!(
                event.event_name.as_str(),
                "UserPromptSubmit" | "SessionStart"
            ) {
                capture.screen.reset_forensic_activity_dedup();
                capture.last_screen_activity_signature = None;
            }
            if matches!(
                event.event_name.as_str(),
                "UserPromptSubmit" | "Stop" | "StopFailure" | "SessionStart" | "SessionEnd"
            ) && capture.activity_status.take().is_some()
            {
                status_changed = true;
            }
            if status_changed {
                capture.status_revision = capture.status_revision.saturating_add(1);
                capture.tab_id.map(|_| status_for_capture(capture, true))
            } else {
                None
            }
        };

        let observed_transcript_path = transcript_path_from_payload(&redacted);
        let log_record = json!({
            "id": event.id,
            "sequence": event.sequence,
            "captureId": event.capture_id,
            "tabId": event.tab_id,
            "eventName": event.event_name,
            "receivedAt": event.received_at,
            "payload": redacted,
        });
        if let Err(error) = append_json_line(
            &log_dir.join("hook-events.jsonl"),
            &log_record,
            MAX_HOOK_LOG_BYTES,
        ) {
            self.record_log_error(&capture_id, error.to_string());
        }
        if event.tab_id.is_some() {
            let _ = self.app.emit("claude_agent_event", event);
        }
        if let Some(status) = observer_status {
            let _ = self.app.emit("claude_observer_status", status);
        }
        self.refresh_context_usage_from_transcript(
            &capture_id,
            observed_transcript_path.as_deref(),
        );
    }

    fn update_context_usage(&self, capture_id: &str, usage: ClaudeContextUsage, native: bool) {
        let status = {
            let Ok(mut captures) = self.captures.lock() else {
                return;
            };
            let Some(capture) = captures.get_mut(capture_id) else {
                return;
            };
            if native {
                capture.last_native_context_at = Some(Instant::now());
            }
            let changed = capture
                .context_usage
                .as_ref()
                .is_none_or(|current| !same_context_usage(current, &usage));
            capture.context_usage = Some(usage);
            if !changed {
                return;
            }
            capture.status_revision = capture.status_revision.saturating_add(1);
            capture.tab_id.map(|_| status_for_capture(capture, true))
        };
        if let Some(status) = status {
            let _ = self.app.emit("claude_observer_status", status);
        }
    }

    fn refresh_context_usage_from_transcript(
        &self,
        capture_id: &str,
        observed_path: Option<&Path>,
    ) {
        let (transcript_path, context_window_size) = {
            let Ok(mut captures) = self.captures.lock() else {
                return;
            };
            let Some(capture) = captures.get_mut(capture_id) else {
                return;
            };
            if let Some(path) = observed_path.filter(|path| is_jsonl_path(path)) {
                capture.transcript_path = Some(path.to_path_buf());
            }
            if capture
                .last_native_context_at
                .is_some_and(|updated| updated.elapsed() < Duration::from_secs(5))
            {
                return;
            }
            let context_window_size = capture
                .context_usage
                .as_ref()
                .and_then(|usage| usage.context_window_size)
                .unwrap_or_else(|| infer_context_window_size(capture.current_model.as_deref()));
            (capture.transcript_path.clone(), context_window_size)
        };
        let Some(path) = transcript_path else {
            return;
        };
        let Ok(Some(usage)) = read_transcript_context_usage(&path, context_window_size) else {
            return;
        };
        self.update_context_usage(capture_id, usage, false);
    }

    fn apply_native_context_usage(&self, capture_id: &str, payload: &Value) {
        if let Some(path) = transcript_path_from_payload(payload) {
            if let Ok(mut captures) = self.captures.lock() {
                if let Some(capture) = captures.get_mut(capture_id) {
                    capture.transcript_path = Some(path);
                }
            }
        }
        if let Some(usage) = context_usage_from_statusline(payload) {
            self.update_context_usage(capture_id, usage, true);
        }
    }

    fn statusline_passthrough(
        &self,
        capture_id: &str,
    ) -> Option<(String, HashMap<String, String>, Option<PathBuf>)> {
        let captures = self.captures.lock().ok()?;
        let capture = captures.get(capture_id)?;
        let mut env = capture.launch_env.clone();
        env.insert("COLUMNS".to_string(), capture.terminal_cols.to_string());
        env.insert("LINES".to_string(), capture.terminal_rows.to_string());
        Some((
            capture.original_statusline_command.clone()?,
            env,
            capture.launch_cwd.clone(),
        ))
    }

    fn import_transcript_history(&self, capture_id: &str, payload: &Value, secrets: &[String]) {
        let source = payload.get("source").and_then(Value::as_str);
        if source != Some("resume") {
            return;
        }
        let Some(transcript_path) = payload
            .get("transcript_path")
            .or_else(|| payload.get("transcriptPath"))
            .and_then(Value::as_str)
        else {
            return;
        };
        let transcript_path = Path::new(transcript_path);
        if !transcript_path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
        {
            self.record_diagnostic(
                capture_id,
                "Claude resume transcript was not a JSONL file; history import was skipped.",
            );
            return;
        }
        let transcript_key = normalize_transcript_key(transcript_path);

        let raw_messages = match read_transcript_tail(transcript_path) {
            Ok(contents) => parse_transcript_messages(&contents),
            Err(error) => {
                self.record_diagnostic(
                    capture_id,
                    &format!("Unable to read Claude resume transcript history: {error}"),
                );
                return;
            }
        };
        if raw_messages.is_empty() {
            return;
        }

        let (imported, log_dir) = {
            let Ok(mut captures) = self.captures.lock() else {
                return;
            };
            let Some(capture) = captures.get_mut(capture_id) else {
                return;
            };
            if !capture.imported_transcripts.insert(transcript_key) {
                return;
            }
            let mut imported = Vec::with_capacity(raw_messages.len());
            for message in raw_messages {
                let payload = trim_value_for_ui(trim_history_value(
                    self.redact_value(message.payload, secrets),
                ));
                let event = ClaudeAgentEvent {
                    id: Uuid::new_v4().to_string(),
                    sequence: capture.next_event_sequence,
                    capture_id: capture_id.to_string(),
                    tab_id: capture.tab_id,
                    event_name: message.event_name.to_string(),
                    received_at: message.received_at,
                    payload,
                };
                capture.next_event_sequence += 1;
                capture.events.push_back(event.clone());
                while capture.events.len() > MAX_EVENTS_IN_MEMORY {
                    capture.events.pop_front();
                }
                imported.push(event);
            }
            (imported, capture.log_dir.clone())
        };
        for event in imported {
            let log_record = json!({
                "id": event.id,
                "sequence": event.sequence,
                "captureId": event.capture_id,
                "tabId": event.tab_id,
                "eventName": event.event_name,
                "receivedAt": event.received_at,
                "source": "transcript-import",
                "payload": event.payload,
            });
            if let Err(error) = append_json_line(
                &log_dir.join("history-events.jsonl"),
                &log_record,
                MAX_HOOK_LOG_BYTES,
            ) {
                self.record_log_error(capture_id, error.to_string());
            }
            if event.tab_id.is_some() {
                let _ = self.app.emit("claude_agent_event", event);
            }
        }
    }

    fn flush_screens(&self, force: bool) {
        let capture_ids = match self.captures.lock() {
            Ok(captures) => captures.keys().cloned().collect::<Vec<_>>(),
            Err(_) => return,
        };
        for capture_id in capture_ids {
            self.flush_one(&capture_id, force);
        }
    }

    fn flush_one(&self, capture_id: &str, force: bool) {
        let capture_context = {
            let Ok(mut captures) = self.captures.lock() else {
                return;
            };
            let Some(capture) = captures.get_mut(capture_id) else {
                return;
            };
            let screen_update = capture.screen.take_stable_update(Instant::now(), force);
            let forensic_lines = capture.screen.take_forensic_lines(force);
            if screen_update.is_none() && forensic_lines.is_empty() {
                return;
            }
            (
                screen_update,
                forensic_lines,
                capture.log_dir.clone(),
                capture.secrets.clone(),
            )
        };
        let (screen_update, forensic_lines, log_dir, secrets) = capture_context;
        let recorded_at = Utc::now().to_rfc3339();
        if !forensic_lines.is_empty() {
            let mut forensic_text = String::new();
            for line in forensic_lines {
                let line = self.redact_text(&line, &secrets);
                forensic_text.push_str(&format!("[{recorded_at}] {line}\n"));
            }
            if !forensic_text.is_empty() {
                if let Err(error) = append_limited(
                    &log_dir.join("pty-forensic-tail.txt"),
                    forensic_text.as_bytes(),
                    MAX_FORENSIC_LOG_BYTES,
                ) {
                    self.record_log_error(capture_id, error.to_string());
                }
            }
        }

        let Some(mut screen_update) = screen_update else {
            return;
        };
        screen_update.latest_screen = self.redact_text(&screen_update.latest_screen, &secrets);

        let detected_prompt = detect_terminal_prompt(&screen_update.latest_screen);
        let detected_activity = detect_claude_activity_status(&screen_update.latest_screen);
        let detected_current_model = detect_claude_current_model(&screen_update.latest_screen);
        let detected_permission_mode = detect_claude_permission_mode(&screen_update.latest_screen);
        let detected_activity_row = detected_activity.as_ref().map(|detected| detected.row);
        let detected_activity_status = detected_activity.map(|detected| detected.status);
        let (observer_status, suppress_activity_diff_row) = self
            .captures
            .lock()
            .ok()
            .and_then(|mut captures| {
                let capture = captures.get_mut(capture_id)?;
                let suppress_activity_diff_row = activity_diff_row_to_suppress(
                    &mut capture.last_screen_activity_signature,
                    detected_activity_row,
                    detected_activity_status.as_ref(),
                );
                let detected_activity_status = if capture.activity_expected
                    || detected_activity_status
                        .as_ref()
                        .is_some_and(is_claude_compaction_activity)
                {
                    detected_activity_status
                } else {
                    None
                };
                let detected_prompt = if capture.session_started {
                    detected_prompt.filter(|prompt| {
                        matches!(
                            prompt,
                            ClaudeTerminalPrompt::PluginInstall { .. }
                                | ClaudeTerminalPrompt::ModelSwitchConfirm { .. }
                                | ClaudeTerminalPrompt::PlanApproval { .. }
                        )
                    })
                } else {
                    detected_prompt
                };
                let prompt_changed = capture.terminal_prompt != detected_prompt;
                let activity_changed = capture.activity_status != detected_activity_status;
                let current_model_changed = detected_current_model
                    .as_ref()
                    .is_some_and(|model| capture.current_model.as_ref() != Some(model));
                let permission_mode_changed = detected_permission_mode
                    .as_ref()
                    .is_some_and(|mode| capture.permission_mode.as_ref() != Some(mode));
                let observer_status = if prompt_changed
                    || activity_changed
                    || current_model_changed
                    || permission_mode_changed
                {
                    capture.terminal_prompt = detected_prompt;
                    capture.activity_status = detected_activity_status;
                    if detected_current_model.is_some() {
                        capture.current_model = detected_current_model;
                    }
                    if detected_permission_mode.is_some() {
                        capture.permission_mode = detected_permission_mode;
                    }
                    capture.status_revision = capture.status_revision.saturating_add(1);
                    capture.tab_id.map(|_| status_for_capture(capture, true))
                } else {
                    None
                };
                Some((observer_status, suppress_activity_diff_row))
            })
            .unwrap_or((None, None));
        if let Some(status) = observer_status {
            let _ = self.app.emit("claude_observer_status", status);
        }

        if let Err(error) = fs::write(
            log_dir.join("terminal-latest.txt"),
            screen_update.latest_screen.as_bytes(),
        ) {
            self.record_log_error(capture_id, error.to_string());
        }

        let Some(mut diff) = screen_update.diff else {
            return;
        };
        diff.latest_screen = screen_update.latest_screen;
        for row in &mut diff.changed_rows {
            row.text = self.redact_text(&row.text, &secrets);
        }
        // Keep the first representative line for diagnostics, then fold only
        // refreshes from the detected bottom activity slot. Other terminal text
        // that merely resembles a status line is never filtered globally.
        if let Some(activity_row) = suppress_activity_diff_row {
            diff.changed_rows.retain(|row| row.row != activity_row);
        }
        if diff.changed_rows.is_empty() {
            return;
        }

        let changed_rows: Vec<Value> = diff
            .changed_rows
            .iter()
            .map(|row| json!({ "row": row.row, "text": row.text }))
            .collect();
        let record = json!({
            "kind": "screen_diff",
            "sequence": diff.sequence,
            "recordedAt": recorded_at,
            "changedRows": changed_rows,
        });
        if let Err(error) = append_json_line(
            &log_dir.join("terminal-output.jsonl"),
            &record,
            MAX_TERMINAL_LOG_BYTES,
        ) {
            self.record_log_error(capture_id, error.to_string());
        }

        let mut readable = format!("\n[{recorded_at}] 屏幕差分 #{}\n", diff.sequence);
        for row in &diff.changed_rows {
            if row.text.is_empty() {
                readable.push_str(&format!("  L{}: [已清空]\n", row.row));
            } else {
                readable.push_str(&format!("  L{}: {}\n", row.row, row.text));
            }
        }
        if let Err(error) = append_limited(
            &log_dir.join("terminal-output.txt"),
            readable.as_bytes(),
            MAX_TERMINAL_LOG_BYTES,
        ) {
            self.record_log_error(capture_id, error.to_string());
        }
    }

    fn record_log_error(&self, capture_id: &str, error: String) {
        let context = {
            let Ok(mut captures) = self.captures.lock() else {
                return;
            };
            let Some(capture) = captures.get_mut(capture_id) else {
                return;
            };
            if capture.log_error.as_deref() == Some(error.as_str()) {
                return;
            }
            capture.log_error = Some(error.clone());
            capture.status_revision = capture.status_revision.saturating_add(1);
            capture.tab_id.map(|_| status_for_capture(capture, true))
        };
        self.record_diagnostic(capture_id, &error);
        if let Some(status) = context {
            let _ = self.app.emit("claude_observer_status", status);
        }
    }

    fn record_diagnostic(&self, capture_id: &str, message: &str) {
        let log_dir = self.captures.lock().ok().and_then(|captures| {
            captures
                .get(capture_id)
                .map(|capture| capture.log_dir.clone())
        });
        let Some(log_dir) = log_dir else {
            return;
        };
        let diagnostic = json!({
            "recordedAt": Utc::now().to_rfc3339(),
            "level": "warning",
            "message": message,
        });
        let _ = append_json_line(
            &log_dir.join("diagnostics.jsonl"),
            &diagnostic,
            MAX_DIAGNOSTIC_LOG_BYTES,
        );
    }

    fn redact_text(&self, input: &str, secrets: &[String]) -> String {
        redact_text_with_patterns(input, secrets, &self.bearer, &self.secret_assignment)
    }

    fn redact_value(&self, value: Value, secrets: &[String]) -> Value {
        match value {
            Value::Object(object) => Value::Object(
                object
                    .into_iter()
                    .map(|(key, value)| {
                        if is_sensitive_container(&key) {
                            (key, Value::String("[REDACTED HEADERS]".to_string()))
                        } else if is_sensitive_key(&key) {
                            (key, Value::String("[REDACTED]".to_string()))
                        } else {
                            (key, self.redact_value(value, secrets))
                        }
                    })
                    .collect(),
            ),
            Value::Array(values) => Value::Array(
                values
                    .into_iter()
                    .map(|value| self.redact_value(value, secrets))
                    .collect(),
            ),
            Value::String(value) => Value::String(self.redact_text(&value, secrets)),
            other => other,
        }
    }

    fn authorize(&self, capture_id: &str, headers: &HeaderMap) -> bool {
        let Some(value) = headers.get(axum::http::header::AUTHORIZATION) else {
            return false;
        };
        let Ok(value) = value.to_str() else {
            return false;
        };
        let Ok(captures) = self.captures.lock() else {
            return false;
        };
        captures
            .get(capture_id)
            .is_some_and(|capture| value == format!("Bearer {}", capture.token))
    }

    fn capture_id_for_tab(&self, tab_id: u32) -> Option<String> {
        self.tab_captures
            .lock()
            .ok()
            .and_then(|tab_captures| tab_captures.get(&tab_id).cloned())
    }

    fn prompt_submission_baseline(
        &self,
        tab_id: u32,
    ) -> Result<ClaudePromptSubmissionBaseline, String> {
        let capture_id = self
            .capture_id_for_tab(tab_id)
            .ok_or_else(|| "当前终端没有 Claude 结构化观察数据".to_string())?;
        let captures = self.captures.lock().map_err(|error| error.to_string())?;
        let capture = captures
            .get(&capture_id)
            .ok_or_else(|| "Claude 观察会话已经清理".to_string())?;
        let transcript_len = latest_transcript_path(&capture.events)
            .map(|path| fs::metadata(path).map(|metadata| metadata.len()).unwrap_or(0));
        Ok(ClaudePromptSubmissionBaseline {
            capture_id,
            event_sequence: capture.next_event_sequence,
            transcript_len,
        })
    }

    fn prompt_submission_observed(
        &self,
        tab_id: u32,
        prompt: &str,
        baseline: &ClaudePromptSubmissionBaseline,
    ) -> Result<bool, String> {
        let capture_id = self
            .capture_id_for_tab(tab_id)
            .ok_or_else(|| "当前终端没有 Claude 结构化观察数据".to_string())?;
        if capture_id != baseline.capture_id {
            return Ok(false);
        }
        let (hook_observed, transcript_path) = {
            let captures = self.captures.lock().map_err(|error| error.to_string())?;
            let capture = captures
                .get(&capture_id)
                .ok_or_else(|| "Claude 观察会话已经清理".to_string())?;
            let expected = normalize_prompt_for_match(prompt);
            let hook_observed = capture.events.iter().any(|event| {
                event.sequence >= baseline.event_sequence
                    && event.event_name == "UserPromptSubmit"
                    && event_prompt_text(&event.payload)
                        .is_some_and(|value| normalize_prompt_for_match(value) == expected)
            });
            (hook_observed, latest_transcript_path(&capture.events))
        };
        if hook_observed {
            return Ok(true);
        }
        let (Some(transcript_path), Some(transcript_len)) =
            (transcript_path, baseline.transcript_len)
        else {
            return Ok(false);
        };
        let appended = read_file_from_offset(&transcript_path, transcript_len)?;
        Ok(transcript_contains_user_prompt(&appended, prompt))
    }

    fn snapshot(&self, tab_id: u32) -> ClaudeObserverSnapshot {
        let Some(capture_id) = self.capture_id_for_tab(tab_id) else {
            return ClaudeObserverSnapshot {
                tab_id,
                status_revision: 0,
                capture_id: None,
                available: false,
                active: false,
                degraded_reason: Some(
                    "当前终端没有 Claude 结构化观察数据，可继续使用原始终端。".into(),
                ),
                log_dir: None,
                events: Vec::new(),
                terminal_log: String::new(),
                terminal_prompt: None,
                activity_status: None,
                current_model: None,
                permission_mode: Some("? for shortcuts".to_string()),
                context_usage: None,
            };
        };
        // A user may return from the raw terminal immediately after cycling a mode.
        // Force the most recent terminal screen through the parser before exposing
        // the snapshot, so its footer cannot be replaced by an older observation.
        self.flush_one(&capture_id, true);
        let capture_context = {
            let Ok(captures) = self.captures.lock() else {
                return unavailable_snapshot(tab_id, "Claude 观察器状态暂时不可读。".into());
            };
            let Some(capture) = captures.get(&capture_id) else {
                return unavailable_snapshot(tab_id, "Claude 观察会话已经清理。".into());
            };
            (
                capture.capture_id.clone(),
                capture.status_revision,
                capture.active,
                capture.log_error.clone(),
                capture.log_dir.clone(),
                capture.events.iter().cloned().collect::<Vec<_>>(),
                capture.terminal_prompt.clone(),
                capture.activity_status.clone(),
                capture.current_model.clone(),
                capture.permission_mode.clone(),
                capture.context_usage.clone(),
            )
        };
        let (
            capture_id,
            status_revision,
            active,
            log_error,
            log_dir,
            events,
            terminal_prompt,
            activity_status,
            current_model,
            permission_mode,
            context_usage,
        ) = capture_context;
        let terminal_log =
            read_last_lines(&log_dir.join("terminal-output.txt"), 400).unwrap_or_default();
        ClaudeObserverSnapshot {
            tab_id,
            status_revision,
            capture_id: Some(capture_id),
            available: true,
            active,
            degraded_reason: log_error,
            log_dir: Some(log_dir.to_string_lossy().to_string()),
            events,
            terminal_log,
            terminal_prompt,
            activity_status,
            current_model,
            permission_mode,
            context_usage,
        }
    }
}

async fn receive_hook(
    State(manager): State<Arc<ClaudeObserverManager>>,
    AxumPath(capture_id): AxumPath<String>,
    request: Request,
) -> StatusCode {
    if !manager.authorize(&capture_id, request.headers()) {
        manager.record_log_error(
            &capture_id,
            "Claude Hook 本地认证失败，结构化事件未记录。".to_string(),
        );
        return StatusCode::NO_CONTENT;
    }
    let body = match to_bytes(request.into_body(), MAX_HOOK_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            manager.record_log_error(
                &capture_id,
                "Claude Hook 事件超过 2 MiB，已跳过并保留 PTY 兜底日志。".to_string(),
            );
            return StatusCode::NO_CONTENT;
        }
    };
    let body_length = body.len();
    let body = match parse_hook_body(&body) {
        Ok(body) => body,
        Err(error) => {
            manager.record_log_error(
                &capture_id,
                format!(
                    "Claude Hook 返回了无法解析的 JSON（{body_length} 字节；{error}），已跳过该事件。"
                ),
            );
            return StatusCode::NO_CONTENT;
        }
    };
    match manager.hook_tx.try_send(HookEnvelope {
        capture_id: capture_id.clone(),
        body,
    }) {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => {
            manager.record_log_error(
                &capture_id,
                "Claude Hook 事件队列已满，部分结构化事件可能缺失。".to_string(),
            );
            // Hook observation must never interrupt or visibly degrade Claude's turn.
            StatusCode::NO_CONTENT
        }
    }
}

async fn receive_statusline(
    State(manager): State<Arc<ClaudeObserverManager>>,
    AxumPath(capture_id): AxumPath<String>,
    request: Request,
) -> Response {
    if !manager.authorize(&capture_id, request.headers()) {
        return StatusCode::NO_CONTENT.into_response();
    }
    let body = match to_bytes(request.into_body(), MAX_STATUSLINE_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => return StatusCode::NO_CONTENT.into_response(),
    };
    let payload = match parse_hook_body(&body) {
        Ok(payload) => payload,
        Err(_) => return StatusCode::NO_CONTENT.into_response(),
    };
    manager.apply_native_context_usage(&capture_id, &payload);

    let Some((command, env, cwd)) = manager.statusline_passthrough(&capture_id) else {
        return String::new().into_response();
    };
    match run_statusline_command(&command, &env, cwd.as_deref(), &body).await {
        Some(output) => output.into_response(),
        None => String::new().into_response(),
    }
}

async fn run_statusline_command(
    command: &str,
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
    body: &[u8],
) -> Option<String> {
    #[cfg(windows)]
    let mut process = {
        if let Some(bash) = resolve_git_bash(env) {
            let mut process = tokio::process::Command::new(bash);
            process.args(["-lc", command]);
            process
        } else {
            let executable = env
                .get("COMSPEC")
                .cloned()
                .or_else(|| std::env::var("COMSPEC").ok())
                .unwrap_or_else(|| "C:\\Windows\\System32\\cmd.exe".to_string());
            let mut process = tokio::process::Command::new(executable);
            process.args(["/d", "/s", "/c", command]);
            process
        }
    };
    #[cfg(not(windows))]
    let mut process = {
        let executable = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut process = tokio::process::Command::new(executable);
        process.args(["-lc", command]);
        process
    };

    process.envs(env);
    if let Some(cwd) = cwd {
        process.current_dir(cwd);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        process
            .as_std_mut()
            .creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = process
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    if stdin.write_all(body).await.is_err() {
        return None;
    }
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(2), child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    let stdout = if output.stdout.len() > MAX_STATUSLINE_OUTPUT_BYTES {
        &output.stdout[..MAX_STATUSLINE_OUTPUT_BYTES]
    } else {
        &output.stdout
    };
    Some(String::from_utf8_lossy(stdout).into_owned())
}

#[cfg(windows)]
fn resolve_git_bash(env: &HashMap<String, String>) -> Option<PathBuf> {
    for key in ["CLAUDE_CODE_GIT_BASH_PATH", "GIT_BASH_PATH"] {
        if let Some(path) = env
            .get(key)
            .cloned()
            .or_else(|| std::env::var(key).ok())
            .map(PathBuf::from)
            .filter(|path| path.is_file())
        {
            return Some(path);
        }
    }
    if let Ok(git) = which::which("git.exe").or_else(|_| which::which("git")) {
        if let Some(root) = git.parent().and_then(Path::parent) {
            let bash = root.join("bin").join("bash.exe");
            if bash.is_file() {
                return Some(bash);
            }
        }
    }
    for variable in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        let Some(root) = std::env::var_os(variable).map(PathBuf::from) else {
            continue;
        };
        let bash = if variable == "LOCALAPPDATA" {
            root.join("Programs")
                .join("Git")
                .join("bin")
                .join("bash.exe")
        } else {
            root.join("Git").join("bin").join("bash.exe")
        };
        if bash.is_file() {
            return Some(bash);
        }
    }
    None
}

fn parse_hook_body(bytes: &[u8]) -> Result<Value, String> {
    if let Some(bytes) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        let text = std::str::from_utf8(bytes)
            .map_err(|error| format!("UTF-8 BOM 后的正文编码无效：{error}"))?;
        return serde_json::from_str(text).map_err(|error| format!("UTF-8 JSON 错误：{error}"));
    }

    let utf16 = if let Some(bytes) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        Some((bytes, true, "UTF-16LE"))
    } else if let Some(bytes) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        Some((bytes, false, "UTF-16BE"))
    } else if bytes.len() >= 2 && matches!(bytes[0], b'{' | b'[') && bytes[1] == 0 {
        Some((bytes, true, "UTF-16LE（无 BOM）"))
    } else if bytes.len() >= 2 && bytes[0] == 0 && matches!(bytes[1], b'{' | b'[') {
        Some((bytes, false, "UTF-16BE（无 BOM）"))
    } else {
        None
    };
    if let Some((bytes, little_endian, encoding)) = utf16 {
        if bytes.len() % 2 != 0 {
            return Err(format!("{encoding} 正文长度不是偶数"));
        }
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|chunk| {
                if little_endian {
                    u16::from_le_bytes([chunk[0], chunk[1]])
                } else {
                    u16::from_be_bytes([chunk[0], chunk[1]])
                }
            })
            .collect();
        let text = String::from_utf16(&units)
            .map_err(|error| format!("{encoding} 正文编码无效：{error}"))?;
        return serde_json::from_str(&text)
            .map_err(|error| format!("{encoding} JSON 错误：{error}"));
    }

    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("正文不是 UTF-8（首个无效字节偏移 {}）", error.valid_up_to()))?;
    serde_json::from_str(text).map_err(|error| format!("UTF-8 JSON 错误：{error}"))
}

#[tauri::command]
pub fn get_claude_observer_snapshot(
    tab_id: u32,
    observer: tauri::State<'_, Arc<ClaudeObserverManager>>,
) -> ClaudeObserverSnapshot {
    observer.snapshot(tab_id)
}

#[tauri::command]
pub fn begin_claude_prompt_submission(
    tab_id: u32,
    observer: tauri::State<'_, Arc<ClaudeObserverManager>>,
) -> Result<ClaudePromptSubmissionBaseline, String> {
    observer.prompt_submission_baseline(tab_id)
}

#[tauri::command]
pub fn confirm_claude_prompt_submission(
    tab_id: u32,
    prompt: String,
    baseline: ClaudePromptSubmissionBaseline,
    observer: tauri::State<'_, Arc<ClaudeObserverManager>>,
) -> Result<bool, String> {
    observer.prompt_submission_observed(tab_id, &prompt, &baseline)
}

#[tauri::command]
pub fn get_claude_terminal_log(
    tab_id: u32,
    max_lines: Option<usize>,
    project_session_id: Option<String>,
    observer: tauri::State<'_, Arc<ClaudeObserverManager>>,
) -> Result<ClaudeTerminalLogResult, String> {
    let (log_dir, historical) = resolve_claude_log_dir(
        observer.inner(),
        tab_id,
        project_session_id.as_deref(),
        true,
    )?;
    let text = read_last_lines(
        &log_dir.join("terminal-output.txt"),
        max_lines.unwrap_or(400).clamp(1, 2_000),
    )
    .map_err(|error| error.to_string())?;
    Ok(ClaudeTerminalLogResult {
        text,
        log_dir: log_dir.to_string_lossy().to_string(),
        historical,
    })
}

#[tauri::command]
pub fn open_claude_log_dir(
    tab_id: u32,
    project_session_id: Option<String>,
    observer: tauri::State<'_, Arc<ClaudeObserverManager>>,
) -> Result<(), String> {
    let (log_dir, _) = resolve_claude_log_dir(
        observer.inner(),
        tab_id,
        project_session_id.as_deref(),
        false,
    )?;
    crate::utils::open_directory(log_dir.to_string_lossy().to_string())
}

#[tauri::command]
pub fn report_claude_observer_timeout(
    tab_id: u32,
    submission_accepted: Option<bool>,
    observer: tauri::State<'_, Arc<ClaudeObserverManager>>,
) {
    if let Some(capture_id) = observer.capture_id_for_tab(tab_id) {
        let message = if submission_accepted == Some(true) {
            "发送消息已由 Claude transcript 确认接收，但 6 秒内未收到对应 Hook 事件；结构化输入已降级。"
        } else {
            "发送消息后 6 秒内既未收到对应 Claude Hook 事件，也未能从 transcript 确认接收；结构化输入已降级。"
        };
        observer.record_diagnostic(
            &capture_id,
            message,
        );
    }
}

fn resolve_claude_log_dir(
    observer: &ClaudeObserverManager,
    tab_id: u32,
    project_session_id: Option<&str>,
    require_terminal_output: bool,
) -> Result<(PathBuf, bool), String> {
    let current = observer.capture_id_for_tab(tab_id).and_then(|capture_id| {
        observer
            .captures
            .lock()
            .ok()
            .and_then(|captures| captures.get(&capture_id).map(|capture| capture.log_dir.clone()))
    });
    if let Some(log_dir) = current.as_ref() {
        let usable = if require_terminal_output {
            log_dir.join("terminal-output.txt").is_file()
        } else {
            has_useful_log_artifacts(log_dir)
        };
        if usable {
            return Ok((log_dir.clone(), false));
        }
    }
    if let Some(project_session_id) = project_session_id {
        let root = app_data_base_dir()?.join("terminal_logs").join("claude");
        if let Some(log_dir) =
            find_latest_project_log_dir(&root, project_session_id, require_terminal_output)
        {
            let historical = current.as_ref() != Some(&log_dir);
            return Ok((log_dir, historical));
        }
    }
    current
        .map(|log_dir| (log_dir, false))
        .ok_or_else(|| "当前终端没有 Claude 日志，且未找到该项目会话的历史日志".to_string())
}

fn find_latest_project_log_dir(
    root: &Path,
    project_session_id: &str,
    require_terminal_output: bool,
) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let metadata_path = path.join("metadata.json");
            let metadata: Value =
                serde_json::from_str(&fs::read_to_string(metadata_path).ok()?).ok()?;
            if metadata.get("projectSessionId").and_then(Value::as_str)
                != Some(project_session_id)
            {
                return None;
            }
            let usable = if require_terminal_output {
                path.join("terminal-output.txt").is_file()
            } else {
                has_useful_log_artifacts(&path)
            };
            if !usable {
                return None;
            }
            let started_at = metadata
                .get("startedAt")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Some((started_at, path))
        })
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, path)| path)
}

fn has_useful_log_artifacts(log_dir: &Path) -> bool {
    [
        "terminal-output.txt",
        "hook-events.jsonl",
        "diagnostics.jsonl",
        "pty-forensic-tail.txt",
    ]
    .into_iter()
    .any(|name| log_dir.join(name).is_file())
}

fn app_data_base_dir() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|path| path.join("ClaudeEnvManager"))
        .ok_or_else(|| "无法确定应用数据目录".to_string())
}

fn write_statusline_settings(
    settings_path: &Path,
    endpoint: &str,
    capture_id: &str,
    token: &str,
) -> Result<(), String> {
    #[cfg(windows)]
    let curl = std::env::var("SystemRoot")
        .ok()
        .map(|root| PathBuf::from(root).join("System32").join("curl.exe"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("curl.exe"));
    #[cfg(not(windows))]
    let curl = PathBuf::from("curl");
    let authorization = format!("Authorization: Bearer {token}");
    let command = format!(
        "\"{}\" --silent --max-time 2 --request POST --header \"{}\" --header \"Content-Type: application/json\" --data-binary \"@-\" \"{}/statusline/{}\"",
        curl.to_string_lossy(),
        authorization,
        endpoint,
        capture_id,
    );
    write_json_pretty(
        settings_path,
        &json!({
            "statusLine": {
                "type": "command",
                "command": command,
            }
        }),
    )
    .map_err(|error| format!("写入 Claude observer statusLine 配置失败：{error}"))
}

fn resolve_existing_statusline_command(
    env: &HashMap<String, String>,
    cwd: Option<&str>,
) -> Result<Option<String>, String> {
    let config_dir = env
        .get("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")));
    let mut paths = Vec::new();
    if let Some(config_dir) = config_dir {
        paths.push(config_dir.join("settings.json"));
    }
    if let Some(cwd) = cwd.filter(|value| !value.trim().is_empty()) {
        let project_config = PathBuf::from(cwd).join(".claude");
        paths.push(project_config.join("settings.json"));
        paths.push(project_config.join("settings.local.json"));
    }

    let mut command = None;
    for path in paths {
        if !path.is_file() {
            continue;
        }
        let raw = fs::read_to_string(&path).map_err(|error| {
            format!(
                "无法读取 Claude statusLine 配置 {}：{error}",
                path.display()
            )
        })?;
        let value: Value = serde_json::from_str(&raw).map_err(|error| {
            format!(
                "无法解析 Claude statusLine 配置 {}：{error}",
                path.display()
            )
        })?;
        if let Some(statusline) = value
            .as_object()
            .and_then(|object| object.get("statusLine"))
        {
            command = statusline
                .get("command")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
        }
    }
    Ok(command)
}

fn write_observer_plugin(
    plugin_dir: &Path,
    endpoint: &str,
    capture_id: &str,
) -> Result<(), String> {
    fs::create_dir_all(plugin_dir.join(".claude-plugin"))
        .map_err(|error| format!("创建 Claude observer 插件目录失败：{error}"))?;
    fs::create_dir_all(plugin_dir.join("hooks"))
        .map_err(|error| format!("创建 Claude observer hooks 目录失败：{error}"))?;
    fs::create_dir_all(plugin_dir.join("scripts"))
        .map_err(|error| format!("创建 Claude observer scripts 目录失败：{error}"))?;
    let manifest = json!({
        "name": format!("agents-launcher-observer-{capture_id}"),
        "version": "0.1.0",
        "description": "Agents Launcher local Claude Code observer",
        "author": { "name": "Agents Launcher" }
    });
    write_json_pretty(
        &plugin_dir.join(".claude-plugin").join("plugin.json"),
        &manifest,
    )
    .map_err(|error| format!("写入 Claude observer 插件清单失败：{error}"))?;

    let handler = json!({
        "type": "http",
        "url": format!("{endpoint}/hooks/{capture_id}"),
        "timeout": 5,
        "headers": {
            "Authorization": "Bearer ${AGENTS_LAUNCHER_HOOK_TOKEN}"
        },
        "allowedEnvVars": ["AGENTS_LAUNCHER_HOOK_TOKEN"]
    });
    let event_names = [
        "UserPromptSubmit",
        "MessageDisplay",
        "PreToolUse",
        "PostToolUse",
        "PostToolUseFailure",
        "PermissionRequest",
        "Notification",
        "SubagentStart",
        "SubagentStop",
        "TaskCreated",
        "TaskCompleted",
        "Stop",
        "StopFailure",
        "SessionEnd",
    ];
    let hooks: serde_json::Map<String, Value> = event_names
        .into_iter()
        .map(|event_name| {
            (
                event_name.to_string(),
                json!([{ "hooks": [handler.clone()] }]),
            )
        })
        .collect();
    let mut hooks = hooks;
    let session_start_script = format!(
        "$inputStream = [Console]::OpenStandardInput()\r\n\
         $memoryStream = New-Object System.IO.MemoryStream\r\n\
         $inputStream.CopyTo($memoryStream)\r\n\
         $bytes = $memoryStream.ToArray()\r\n\
         try {{\r\n\
           $headers = @{{ Authorization = \"Bearer $env:AGENTS_LAUNCHER_HOOK_TOKEN\" }}\r\n\
           Invoke-WebRequest -UseBasicParsing -Uri \"{endpoint}/hooks/{capture_id}\" -Method Post -ContentType \"application/json; charset=utf-8\" -Headers $headers -Body $bytes -TimeoutSec 3 | Out-Null\r\n\
         }} catch {{ }}\r\n\
         $memoryStream.Dispose()\r\n\
         exit 0\r\n"
    );
    fs::write(
        plugin_dir.join("scripts").join("session-start.ps1"),
        session_start_script,
    )
    .map_err(|error| format!("写入 Claude observer SessionStart 脚本失败：{error}"))?;
    hooks.insert(
        "SessionStart".to_string(),
        json!([{
            "hooks": [{
                "type": "command",
                "command": "powershell.exe",
                "args": [
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    "${CLAUDE_PLUGIN_ROOT}/scripts/session-start.ps1"
                ],
                "timeout": 5
            }]
        }]),
    );
    write_json_pretty(
        &plugin_dir.join("hooks").join("hooks.json"),
        &json!({ "hooks": hooks }),
    )
    .map_err(|error| format!("写入 Claude observer Hook 配置失败：{error}"))
}

struct HistoricalEvent {
    event_name: &'static str,
    received_at: String,
    payload: Value,
}

fn normalize_transcript_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase()
}

fn read_transcript_tail(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Ok(String::new());
    }
    let start = metadata.len().saturating_sub(MAX_TRANSCRIPT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((metadata.len() - start) as usize);
    file.read_to_end(&mut bytes)?;
    let mut contents = String::from_utf8_lossy(&bytes).into_owned();
    if start > 0 {
        if let Some(first_newline) = contents.find('\n') {
            contents.drain(..=first_newline);
        } else {
            contents.clear();
        }
    }
    Ok(contents)
}

fn latest_transcript_path(events: &VecDeque<ClaudeAgentEvent>) -> Option<PathBuf> {
    events.iter().rev().find_map(|event| {
        let value = event
            .payload
            .get("transcript_path")
            .or_else(|| event.payload.get("transcriptPath"))
            .and_then(Value::as_str)?;
        let path = PathBuf::from(value);
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
            .then_some(path)
    })
}

fn transcript_path_from_payload(payload: &Value) -> Option<PathBuf> {
    let value = payload
        .get("transcript_path")
        .or_else(|| payload.get("transcriptPath"))
        .and_then(Value::as_str)?;
    let path = PathBuf::from(value);
    is_jsonl_path(&path).then_some(path)
}

fn is_jsonl_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
}

fn usage_token_value(usage: &Value, key: &str) -> u64 {
    usage
        .get(key)
        .and_then(|value| {
            value.as_u64().or_else(|| {
                value
                    .as_f64()
                    .filter(|number| *number >= 0.0)
                    .map(|number| number as u64)
            })
        })
        .unwrap_or(0)
}

fn context_used_tokens(usage: &Value) -> u64 {
    usage_token_value(usage, "input_tokens")
        .saturating_add(usage_token_value(usage, "cache_creation_input_tokens"))
        .saturating_add(usage_token_value(usage, "cache_read_input_tokens"))
}

fn bounded_percentage(value: f64) -> u8 {
    value.round().clamp(0.0, 100.0) as u8
}

fn context_usage_from_statusline(payload: &Value) -> Option<ClaudeContextUsage> {
    let context = payload.get("context_window")?;
    let current_usage = context.get("current_usage");
    let used_tokens = current_usage
        .map(context_used_tokens)
        .filter(|tokens| *tokens > 0);
    let context_window_size = context
        .get("context_window_size")
        .and_then(Value::as_u64)
        .filter(|size| *size > 0);
    let native_percentage = context
        .get("used_percentage")
        .and_then(Value::as_f64)
        .filter(|percentage| percentage.is_finite() && *percentage >= 0.0);
    let used_percentage = if native_percentage.is_some_and(|percentage| percentage > 0.0) {
        bounded_percentage(native_percentage.unwrap_or_default())
    } else if let (Some(tokens), Some(size)) = (used_tokens, context_window_size) {
        bounded_percentage(tokens as f64 / size as f64 * 100.0)
    } else {
        bounded_percentage(native_percentage?)
    };
    let remaining_percentage = context
        .get("remaining_percentage")
        .and_then(Value::as_f64)
        .filter(|percentage| percentage.is_finite() && *percentage >= 0.0)
        .map(bounded_percentage)
        .unwrap_or(100u8.saturating_sub(used_percentage));
    Some(ClaudeContextUsage {
        used_percentage,
        remaining_percentage,
        used_tokens,
        context_window_size,
        source: "native".to_string(),
        updated_at: Utc::now().to_rfc3339(),
    })
}

fn infer_context_window_size(current_model: Option<&str>) -> u64 {
    let model = current_model.unwrap_or_default().to_ascii_lowercase();
    if model.contains("[1m]") || model.contains("1m context") {
        1_000_000
    } else {
        200_000
    }
}

fn same_context_usage(left: &ClaudeContextUsage, right: &ClaudeContextUsage) -> bool {
    left.used_percentage == right.used_percentage
        && left.remaining_percentage == right.remaining_percentage
        && left.used_tokens == right.used_tokens
        && left.context_window_size == right.context_window_size
        && left.source == right.source
}

fn read_transcript_context_usage(
    path: &Path,
    context_window_size: u64,
) -> std::io::Result<Option<ClaudeContextUsage>> {
    let mut file = fs::File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(MAX_TRANSCRIPT_USAGE_TAIL_BYTES);
    if start > 0 {
        file.seek(SeekFrom::Start(start))?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let mut contents = String::from_utf8_lossy(&bytes).into_owned();
    if start > 0 {
        if let Some(first_newline) = contents.find('\n') {
            contents.drain(..=first_newline);
        } else {
            return Ok(None);
        }
    }
    for line in contents.lines().rev() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if record.get("type").and_then(Value::as_str) != Some("assistant")
            || record.get("isSidechain").and_then(Value::as_bool) == Some(true)
            || record.get("isMeta").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let Some(usage) = record
            .get("message")
            .and_then(|message| message.get("usage"))
        else {
            continue;
        };
        let used_tokens = context_used_tokens(usage);
        if used_tokens == 0 || context_window_size == 0 {
            continue;
        }
        let used_percentage =
            bounded_percentage(used_tokens as f64 / context_window_size as f64 * 100.0);
        return Ok(Some(ClaudeContextUsage {
            used_percentage,
            remaining_percentage: 100u8.saturating_sub(used_percentage),
            used_tokens: Some(used_tokens),
            context_window_size: Some(context_window_size),
            source: "transcript".to_string(),
            updated_at: Utc::now().to_rfc3339(),
        }));
    }
    Ok(None)
}

fn event_prompt_text(payload: &Value) -> Option<&str> {
    ["prompt", "text", "message"]
        .into_iter()
        .find_map(|key| payload.get(key).and_then(Value::as_str))
}

fn normalize_prompt_for_match(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|character| {
            let code = *character as u32;
            (!character.is_control() && code != 0x7f && !(0x80..=0x9f).contains(&code))
                || matches!(character, '\n' | '\t')
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn read_file_from_offset(path: &Path, offset: u64) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let length = file.metadata().map_err(|error| error.to_string())?.len();
    if length <= offset {
        return Ok(String::new());
    }
    let start = offset.max(length.saturating_sub(MAX_TRANSCRIPT_TAIL_BYTES));
    file.seek(SeekFrom::Start(start))
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let mut contents = String::from_utf8_lossy(&bytes).into_owned();
    if start > offset {
        if let Some(first_newline) = contents.find('\n') {
            contents.drain(..=first_newline);
        } else {
            contents.clear();
        }
    }
    Ok(contents)
}

fn transcript_contains_user_prompt(contents: &str, prompt: &str) -> bool {
    let expected = normalize_prompt_for_match(prompt);
    parse_transcript_messages(contents).into_iter().any(|event| {
        event.event_name == "HistoricalUserMessage"
            && event
                .payload
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|value| normalize_prompt_for_match(value) == expected)
    })
}

fn parse_transcript_messages(contents: &str) -> Vec<HistoricalEvent> {
    let mut events = VecDeque::with_capacity(MAX_HISTORY_EVENTS + 1);
    let mut tool_names = HashMap::<String, String>::new();
    for line in contents.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if record.get("isSidechain").and_then(Value::as_bool) == Some(true)
            || record.get("isMeta").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let Some(kind) = record.get("type").and_then(Value::as_str) else {
            continue;
        };
        let received_at = record
            .get("timestamp")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| Utc::now().to_rfc3339());

        if kind == "system"
            && record.get("subtype").and_then(Value::as_str) == Some("local_command")
        {
            if let Some(content) = record.get("content").and_then(Value::as_str) {
                if let Some(event) = historical_local_command_event(content, &received_at) {
                    events.push_back(event);
                    while events.len() > MAX_HISTORY_EVENTS {
                        events.pop_front();
                    }
                }
            }
            continue;
        }

        if !matches!(kind, "user" | "assistant") {
            continue;
        }
        let Some(message) = record.get("message") else {
            continue;
        };

        for event in transcript_record_events(kind, message, &received_at, &mut tool_names) {
            events.push_back(event);
            while events.len() > MAX_HISTORY_EVENTS {
                events.pop_front();
            }
        }
    }
    events.into_iter().collect()
}

fn transcript_record_events(
    kind: &str,
    message: &Value,
    received_at: &str,
    tool_names: &mut HashMap<String, String>,
) -> Vec<HistoricalEvent> {
    let Some(content) = message.get("content") else {
        return Vec::new();
    };
    if let Some(content) = content.as_str() {
        return historical_text_event(kind, content, received_at)
            .into_iter()
            .collect();
    }
    let Some(blocks) = content.as_array() else {
        return Vec::new();
    };

    let mut events = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    if let Some(event) = historical_text_event(kind, text, received_at) {
                        events.push(event);
                    }
                }
            }
            Some("tool_use") if kind == "assistant" => {
                let Some(tool_use_id) = block.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let tool_name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("工具调用");
                tool_names.insert(tool_use_id.to_string(), tool_name.to_string());
                events.push(HistoricalEvent {
                    event_name: "PreToolUse",
                    received_at: received_at.to_string(),
                    payload: json!({
                        "tool_use_id": tool_use_id,
                        "tool_name": tool_name,
                        "tool_input": block.get("input").cloned().unwrap_or(Value::Null),
                        "historical": true,
                    }),
                });
            }
            Some("tool_result") if kind == "user" => {
                let Some(tool_use_id) = block.get("tool_use_id").and_then(Value::as_str) else {
                    continue;
                };
                let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
                let tool_name = tool_names
                    .get(tool_use_id)
                    .cloned()
                    .unwrap_or_else(|| "工具调用".to_string());
                events.push(HistoricalEvent {
                    event_name: if failed {
                        "PostToolUseFailure"
                    } else {
                        "PostToolUse"
                    },
                    received_at: received_at.to_string(),
                    payload: json!({
                        "tool_use_id": tool_use_id,
                        "tool_name": tool_name,
                        "tool_response": transcript_tool_result(block.get("content")),
                        "historical": true,
                    }),
                });
            }
            // Thinking, signatures, images and other internal blocks are intentionally omitted.
            _ => {}
        }
    }
    events
}

fn historical_text_event(kind: &str, text: &str, received_at: &str) -> Option<HistoricalEvent> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let text: String = text.chars().take(MAX_HISTORY_TEXT_CHARS).collect();
    Some(HistoricalEvent {
        event_name: if kind == "user" {
            "HistoricalUserMessage"
        } else {
            "HistoricalAssistantMessage"
        },
        received_at: received_at.to_string(),
        payload: json!({ "text": text, "historical": true }),
    })
}

fn historical_local_command_event(text: &str, received_at: &str) -> Option<HistoricalEvent> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let text: String = text.chars().take(MAX_HISTORY_TEXT_CHARS).collect();
    Some(HistoricalEvent {
        event_name: "HistoricalLocalCommand",
        received_at: received_at.to_string(),
        payload: json!({ "text": text, "historical": true }),
    })
}

fn transcript_tool_result(content: Option<&Value>) -> Value {
    let Some(content) = content else {
        return Value::Null;
    };
    if content.is_string() || content.is_object() {
        return content.clone();
    }
    let Some(blocks) = content.as_array() else {
        return Value::Null;
    };
    let text = blocks
        .iter()
        .filter_map(|block| {
            if block.get("type").and_then(Value::as_str) == Some("text") {
                block.get("text").and_then(Value::as_str)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        json!({ "omittedNonTextContent": true })
    } else {
        Value::String(text)
    }
}

fn trim_history_value(value: Value) -> Value {
    match value {
        Value::String(value) if value.chars().count() > MAX_HISTORY_TEXT_CHARS => {
            let mut preview: String = value.chars().take(MAX_HISTORY_TEXT_CHARS).collect();
            preview.push_str("\n[历史工具内容已截断]");
            Value::String(preview)
        }
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .take(128)
                .map(trim_history_value)
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, trim_history_value(value)))
                .collect(),
        ),
        other => other,
    }
}

fn collect_sensitive_values(env: &HashMap<String, String>) -> Vec<String> {
    let mut values = Vec::new();
    for (key, value) in std::env::vars().chain(env.clone()) {
        if is_sensitive_key(&key) && value.len() >= 4 && !values.contains(&value) {
            values.push(value);
        }
    }
    values
}

fn redact_text_with_patterns(
    input: &str,
    secrets: &[String],
    bearer: &Regex,
    secret_assignment: &Regex,
) -> String {
    let mut output = input.to_string();
    for secret in secrets {
        if secret.len() >= 4 && output.contains(secret) {
            output = output.replace(secret, "[REDACTED]");
        }
    }
    output = bearer.replace_all(&output, "${1}[REDACTED]").into_owned();
    secret_assignment
        .replace_all(&output, "${1}[REDACTED]")
        .into_owned()
}

fn is_sensitive_key(key: &str) -> bool {
    let key = normalize_sensitive_key(key);
    key == "token"
        || key == "cookie"
        || key == "set_cookie"
        || key == "authorization"
        || key.contains("api_key")
        || key.contains("apikey")
        || key.contains("access_token")
        || key.contains("auth_token")
        || key.ends_with("_token")
        || key.contains("secret")
        || key.contains("password")
        || key.contains("credential")
        || key.contains("private_key")
        || key.contains("signing_key")
}

fn is_sensitive_container(key: &str) -> bool {
    matches!(
        normalize_sensitive_key(key).as_str(),
        "headers" | "http_headers" | "request_headers" | "response_headers"
    )
}

fn normalize_sensitive_key(key: &str) -> String {
    key.to_ascii_lowercase().replace(['-', '.', ' '], "_")
}

fn normalize_screen(contents: &str) -> String {
    let mut lines: Vec<String> = contents
        .replace('\0', "")
        .lines()
        .map(|line| line.trim_end().to_string())
        .collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

fn detect_terminal_prompt(screen: &str) -> Option<ClaudeTerminalPrompt> {
    let lines: Vec<&str> = screen
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    detect_workspace_trust_prompt(&lines)
        .or_else(|| detect_model_switch_confirm_prompt(&lines))
        .or_else(|| detect_plan_approval_prompt(&lines))
        .or_else(|| detect_plugin_install_prompt(&lines))
}

fn detect_model_switch_confirm_prompt(lines: &[&str]) -> Option<ClaudeTerminalPrompt> {
    let title_index = lines
        .iter()
        .rposition(|line| contains_ascii_case_insensitive(line, "switch model?"))?;
    if lines.len().saturating_sub(title_index) > 8 {
        return None;
    }
    if !lines[title_index..]
        .iter()
        .any(|line| contains_ascii_case_insensitive(line, "conversation is cached"))
    {
        return None;
    }

    let option_end = if lines.last().is_some_and(|line| is_terminal_selection_footer(line)) {
        lines.len().saturating_sub(1)
    } else {
        lines.len()
    };
    if option_end <= title_index
        || collect_numbered_options(lines[option_end - 1])
            .last()
            .map(|(number, _, _)| *number)
            != Some(2)
    {
        return None;
    }

    let option_start = lines
        .iter()
        .enumerate()
        .skip(title_index)
        .take(option_end.saturating_sub(title_index))
        .find_map(|(index, line)| {
            collect_numbered_options(line)
                .iter()
                .any(|(number, _, _)| *number == 1)
                .then_some(index)
        })?;
    let mut options = collect_numbered_options(&lines[option_start..option_end].join(" "));
    if options.len() != 2 || options[0].0 != 1 || options[1].0 != 2 {
        return None;
    }
    let selected_index = options.iter().position(|(_, _, selected)| *selected)?;
    let options = options
        .drain(..)
        .map(|(_, label, _)| label)
        .collect::<Vec<_>>();
    let option_line_prefix = find_numbered_option_start(lines[option_start], 1)
        .map(|offset| strip_terminal_selection_marker(&lines[option_start][..offset]))
        .unwrap_or_default();
    let mut prompt_parts = lines[title_index..option_start].to_vec();
    if !option_line_prefix.is_empty() {
        prompt_parts.push(&option_line_prefix);
    }
    let prompt = prompt_parts
        .join(" ")
        .chars()
        .take(360)
        .collect::<String>();

    Some(ClaudeTerminalPrompt::ModelSwitchConfirm {
        prompt,
        options,
        selected_index,
    })
}

fn detect_plan_approval_prompt(lines: &[&str]) -> Option<ClaudeTerminalPrompt> {
    let footer_index = lines
        .iter()
        .rposition(|line| is_terminal_selection_footer(line))?;
    if footer_index + 1 != lines.len() || footer_index < 2 {
        return None;
    }

    for option_start in (0..footer_index).rev() {
        let Some((first_number, first_label)) = parse_terminal_option(lines[option_start]) else {
            continue;
        };
        if first_number != 1 {
            continue;
        }

        let mut options = vec![first_label];
        let mut next_number = 2;
        let mut index = option_start + 1;
        while index < footer_index {
            let Some((number, label)) = parse_terminal_option(lines[index]) else {
                break;
            };
            if number != next_number {
                break;
            }
            options.push(label);
            next_number += 1;
            index += 1;
        }
        if options.len() < 2 || index != footer_index {
            continue;
        }

        let context_start = option_start.saturating_sub(10);
        let context = &lines[context_start..option_start];
        let context_text = context.join(" ");
        let is_plan_approval = contains_ascii_case_insensitive(&context_text, "plan")
            && (contains_ascii_case_insensitive(&context_text, "ready to code")
                || contains_ascii_case_insensitive(&context_text, "start coding")
                || contains_ascii_case_insensitive(&context_text, "proceed")
                || contains_ascii_case_insensitive(&context_text, "implement"));
        if !is_plan_approval {
            continue;
        }

        let selected_index = lines[option_start..footer_index]
            .iter()
            .position(|line| has_terminal_selection_marker(line))
            .unwrap_or(0);
        if selected_index >= options.len() {
            continue;
        }
        let prompt = context
            .join(" ")
            .chars()
            .take(360)
            .collect::<String>();
        return Some(ClaudeTerminalPrompt::PlanApproval {
            prompt,
            options,
            selected_index,
        });
    }
    None
}

fn detect_workspace_trust_prompt(lines: &[&str]) -> Option<ClaudeTerminalPrompt> {
    let workspace_index = lines
        .iter()
        .position(|line| *line == "Accessing workspace:")?;
    let quick_check_index = lines
        .iter()
        .enumerate()
        .skip(workspace_index + 1)
        .find_map(|(index, line)| line.starts_with("Quick safety check:").then_some(index))?;
    if quick_check_index <= workspace_index + 1 {
        return None;
    }

    let path = lines[workspace_index + 1..quick_check_index].concat();
    if !looks_like_windows_workspace_path(&path) {
        return None;
    }

    let trust_index = lines
        .iter()
        .enumerate()
        .skip(quick_check_index + 1)
        .find_map(|(index, line)| (*line == "❯ 1. Yes, I trust this folder").then_some(index))?;
    if lines.get(trust_index + 1).copied() != Some("2. No, exit")
        || lines.get(trust_index + 2).copied() != Some("Enter to confirm · Esc to cancel")
        || trust_index + 2 != lines.len() - 1
    {
        return None;
    }

    Some(ClaudeTerminalPrompt::WorkspaceTrust { path })
}

fn detect_plugin_install_prompt(lines: &[&str]) -> Option<ClaudeTerminalPrompt> {
    let footer_index = lines
        .iter()
        .rposition(|line| is_terminal_selection_footer(line))?;
    if footer_index + 1 != lines.len() {
        return None;
    }
    let option_end = footer_index;
    if option_end < 2 {
        return None;
    }

    for option_start in (0..option_end).rev() {
        let Some((first_number, first_label)) = parse_terminal_option(lines[option_start]) else {
            continue;
        };
        if first_number != 1 {
            continue;
        }

        let mut options = vec![first_label];
        let mut next_number = 2;
        let mut index = option_start + 1;
        while index < option_end {
            let Some((number, label)) = parse_terminal_option(lines[index]) else {
                break;
            };
            if number != next_number {
                break;
            }
            options.push(label);
            next_number += 1;
            index += 1;
        }

        if options.len() < 2 || index != option_end {
            continue;
        }
        let context_start = option_start.saturating_sub(8);
        let context = &lines[context_start..option_start];
        let context_text = context.join(" ");
        if !contains_ascii_case_insensitive(&context_text, "plugin")
            || !contains_ascii_case_insensitive(&context_text, "install")
        {
            continue;
        }

        let prompt = context
            .iter()
            .rev()
            .find(|line| {
                contains_ascii_case_insensitive(line, "plugin")
                    || contains_ascii_case_insensitive(line, "install")
            })
            .copied()
            .unwrap_or("Install plugin?")
            .chars()
            .take(240)
            .collect::<String>();
        let plugin_name = extract_plugin_name(&prompt)
            .unwrap_or_else(|| "Claude Code plugin".to_string());

        return Some(ClaudeTerminalPrompt::PluginInstall {
            plugin_name,
            prompt,
            options,
        });
    }

    None
}

fn collect_numbered_options(text: &str) -> Vec<(usize, String, bool)> {
    let mut starts = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let Some(relative_digit_start) = text[index..]
            .find(|character: char| character.is_ascii_digit())
        else {
            break;
        };
        let digit_start = index + relative_digit_start;
        let number_end = text[digit_start..]
            .find(|character: char| !character.is_ascii_digit())
            .map(|offset| digit_start + offset)
            .unwrap_or(text.len());
        if text[number_end..].starts_with('.')
            && text[number_end + 1..]
                .chars()
                .next()
                .is_some_and(|character| character.is_whitespace())
            && (digit_start == 0
                || !text[..digit_start]
                    .chars()
                    .next_back()
                    .is_some_and(|character| character.is_ascii_alphanumeric()))
        {
            if let Ok(number) = text[digit_start..number_end].parse::<usize>() {
                let selected = has_terminal_selection_marker(&text[..digit_start]);
                starts.push((digit_start, number, selected));
            }
        }
        index = number_end.saturating_add(1);
    }

    starts
        .iter()
        .enumerate()
        .map(|(index, (start, number, selected))| {
            let label_start = start
                + text[*start..]
                    .find('.')
                    .map(|offset| offset + 1)
                    .unwrap_or(0);
            let label_end = starts
                .get(index + 1)
                .map(|(next_start, _, _)| *next_start)
                .unwrap_or(text.len());
            (
                *number,
                strip_terminal_selection_marker(&text[label_start..label_end]),
                *selected,
            )
        })
        .collect()
}

fn find_numbered_option_start(text: &str, target_number: usize) -> Option<usize> {
    let needle = format!("{target_number}.");
    text.match_indices(&needle).find_map(|(start, _)| {
        let before_is_valid = start == 0
            || !text[..start]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_ascii_alphanumeric());
        let after_is_valid = text[start + needle.len()..]
            .chars()
            .next()
            .is_some_and(|character| character.is_whitespace());
        (before_is_valid && after_is_valid).then_some(start)
    })
}

fn has_terminal_selection_marker(prefix: &str) -> bool {
    let prefix = prefix.trim_end();
    prefix.ends_with('❯') || prefix.ends_with('>') || prefix.ends_with("鉂?")
}

fn strip_terminal_selection_marker(label: &str) -> String {
    let label = label.trim();
    for marker in ['❯', '>'] {
        if let Some(without_marker) = label.strip_suffix(marker) {
            return without_marker.trim().to_string();
        }
    }
    label
        .strip_suffix("鉂?")
        .map(str::trim)
        .unwrap_or(label)
        .to_string()
}

fn parse_terminal_option(line: &str) -> Option<(usize, String)> {
    parse_terminal_option_with_selection(line)
        .map(|(number, label, _)| (number, label))
}

fn parse_terminal_option_with_selection(line: &str) -> Option<(usize, String, bool)> {
    let line = line.trim();
    let digit_start = line.find(|character: char| character.is_ascii_digit())?;
    let prefix = line[..digit_start].trim();
    if prefix.chars().any(|character| character.is_ascii_alphanumeric()) {
        return None;
    }

    let number_end = line[digit_start..]
        .find(|character: char| !character.is_ascii_digit())
        .map(|offset| digit_start + offset)
        .unwrap_or(line.len());
    let number = line[digit_start..number_end].parse().ok()?;
    let remainder = line[number_end..].trim_start();
    let remainder = remainder.strip_prefix('.').or_else(|| remainder.strip_prefix(')'))?;
    let label = remainder.trim_start();
    if label.is_empty() {
        return None;
    }
    Some((number, label.to_string(), !prefix.is_empty()))
}

fn is_terminal_selection_footer(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("enter")
        && lower.contains("esc")
        && (lower.contains("cancel") || lower.contains("back"))
}

fn contains_ascii_case_insensitive(text: &str, needle: &str) -> bool {
    text.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
}

fn extract_plugin_name(prompt: &str) -> Option<String> {
    let lower = prompt.to_ascii_lowercase();
    let plugin_index = lower.find("plugin")?;
    let suffix = prompt[plugin_index + "plugin".len()..]
        .trim_matches(|character: char| {
            character.is_whitespace()
                || matches!(character, ':' | '=' | '-' | '_' | '`' | '\"' | '\'' | '?' | '.')
        })
        .strip_prefix("named")
        .unwrap_or_else(|| {
            prompt[plugin_index + "plugin".len()..]
                .trim_matches(|character: char| {
                    character.is_whitespace()
                        || matches!(character, ':' | '=' | '-' | '_' | '`' | '\"' | '\'' | '?' | '.')
                })
        })
        .trim_matches(|character: char| {
            character.is_whitespace()
                || matches!(character, ':' | '=' | '-' | '_' | '`' | '\"' | '\'' | '?' | '.')
        });
    let candidate = suffix
        .split(['?', ':'])
        .next()
        .unwrap_or_default()
        .trim();
    if candidate.is_empty() {
        return None;
    }
    Some(candidate.chars().take(120).collect())
}

struct DetectedClaudeActivityStatus {
    row: usize,
    status: ClaudeActivityStatus,
}

fn detect_claude_activity_status(screen: &str) -> Option<DetectedClaudeActivityStatus> {
    let lines: Vec<&str> = screen.lines().collect();
    lines
        .iter()
        .enumerate()
        .rev()
        .take(12)
        .find_map(|(index, line)| {
            parse_claude_activity_status_line(line).map(|status| DetectedClaudeActivityStatus {
                row: index + 1,
                status,
            })
        })
}

fn is_claude_compaction_activity(status: &ClaudeActivityStatus) -> bool {
    status.label.eq_ignore_ascii_case("Compacting conversation")
}

fn detect_claude_current_model(screen: &str) -> Option<String> {
    screen
        .lines()
        .rev()
        .take(12)
        .find_map(parse_claude_current_model_line)
}

fn detect_claude_permission_mode(screen: &str) -> Option<String> {
    screen.lines().rev().take(12).find_map(|line| {
        let normalized = line.trim().to_ascii_lowercase();
        if !(normalized.contains("shift+tab") || normalized.contains("shift + tab"))
            || !normalized.contains("cycle")
        {
            return None;
        }

        if normalized.contains("bypass permissions") {
            terminal_permission_mode_label(line)
        } else if normalized.contains("don't ask") || normalized.contains("dont ask") {
            terminal_permission_mode_label(line)
        } else if normalized.contains("auto-accept edits") || normalized.contains("accept edits") {
            terminal_permission_mode_label(line)
        } else if normalized.contains("plan mode") {
            terminal_permission_mode_label(line)
        } else if normalized.contains("auto mode") {
            terminal_permission_mode_label(line)
        } else if normalized.contains("manual mode")
            || normalized.contains("default mode")
            || normalized.contains("ask permissions")
        {
            terminal_permission_mode_label(line)
        } else {
            None
        }
    })
}

fn terminal_permission_mode_label(line: &str) -> Option<String> {
    let label = line
        .trim()
        .split_once("(shift+tab")
        .or_else(|| line.trim().split_once("(shift + tab"))
        .map(|(label, _)| label.trim())?;
    let label = label
        .strip_suffix(" on")
        .or_else(|| label.strip_suffix(" ON"))
        .unwrap_or(label)
        .trim();
    (!label.is_empty()).then(|| label.to_string())
}

fn parse_claude_current_model_line(line: &str) -> Option<String> {
    let line = line.trim();
    let model_end = line.strip_prefix('[')?.find(']')?;
    let model = line[1..model_end + 1].trim();
    let suffix = line[model_end + 2..].trim_start();
    if !suffix.starts_with('|')
        || model.is_empty()
        || model.chars().count() > 80
        || model.chars().any(char::is_control)
    {
        return None;
    }
    Some(model.to_string())
}

fn parse_claude_activity_status_line(line: &str) -> Option<ClaudeActivityStatus> {
    let line = line.trim();
    let (mut label, details) = if let Some(without_close) = line.strip_suffix(')') {
        let open_index = without_close.rfind('(')?;
        (
            without_close[..open_index].trim(),
            Some(without_close[open_index + 1..].trim()),
        )
    } else {
        (line, None)
    };

    let spinner = label.chars().next()?;
    if !matches!(spinner, '·' | '*' | '✢' | '✳' | '✶' | '✻' | '✽') {
        return None;
    }
    label = label[spinner.len_utf8()..].trim_start();
    label = label
        .strip_suffix('…')
        .or_else(|| label.strip_suffix("..."))?
        .trim_end();
    if label.is_empty() || label.chars().count() > 48 {
        return None;
    }

    let mut elapsed = None;
    let mut token_direction = None;
    let mut token_count = None;
    let mut phase = None;
    let Some(details) = details else {
        return Some(ClaudeActivityStatus {
            label: label.to_string(),
            elapsed,
            token_direction,
            token_count,
            phase,
        });
    };
    let parts: Vec<&str> = details.split('·').map(str::trim).collect();
    let elapsed_text = parts.first().copied().unwrap_or_default();
    if !is_claude_activity_elapsed(elapsed_text) {
        return None;
    }
    elapsed = Some(elapsed_text.to_string());

    if let Some(second) = parts.get(1).copied() {
        if matches!(second.chars().next(), Some('↑' | '↓')) {
            let (direction, count) = parse_claude_activity_tokens(second)?;
            token_direction = Some(direction);
            token_count = Some(count);
            phase = (parts.len() > 2)
                .then(|| parts[2..].join(" · "))
                .filter(|value| !value.is_empty() && value.chars().count() <= 48);
        } else {
            phase = Some(parts[1..].join(" · "))
                .filter(|value| !value.is_empty() && value.chars().count() <= 48);
        }
    }

    Some(ClaudeActivityStatus {
        label: label.to_string(),
        elapsed,
        token_direction,
        token_count,
        phase,
    })
}

fn is_claude_activity_elapsed(elapsed: &str) -> bool {
    if elapsed.is_empty()
        || elapsed.chars().count() > 24
        || !elapsed.chars().any(|character| character.is_ascii_digit())
        || !elapsed.chars().all(|character| {
            character.is_ascii_digit()
                || character.is_ascii_whitespace()
                || matches!(character, 'h' | 'm' | 's')
        })
    {
        return false;
    }
    true
}

fn parse_claude_activity_tokens(token_part: &str) -> Option<(String, String)> {
    let mut token_characters = token_part.chars();
    let direction = token_characters.next()?;
    if !matches!(direction, '↑' | '↓') {
        return None;
    }
    let token_text = token_characters.as_str().trim();
    let token_count = token_text
        .strip_suffix("tokens")
        .or_else(|| token_text.strip_suffix("token"))?
        .trim();
    if token_count.is_empty()
        || token_count.chars().count() > 16
        || !token_count
            .chars()
            .any(|character| character.is_ascii_digit())
        || !token_count.chars().all(|character| {
            character.is_ascii_digit() || matches!(character, ',' | '.' | 'k' | 'K' | 'm' | 'M')
        })
    {
        return None;
    }
    Some((direction.to_string(), token_count.to_string()))
}

fn claude_activity_signature(status: &ClaudeActivityStatus) -> String {
    format!(
        "{}\0{}\0{}",
        status.label,
        status.token_direction.as_deref().unwrap_or_default(),
        status.phase.as_deref().unwrap_or_default(),
    )
}

fn activity_diff_row_to_suppress(
    previous_signature: &mut Option<String>,
    detected_row: Option<usize>,
    detected_status: Option<&ClaudeActivityStatus>,
) -> Option<usize> {
    let current_signature = detected_status.map(claude_activity_signature);
    let suppress_row = (current_signature.is_some()
        && previous_signature.as_deref() == current_signature.as_deref())
    .then_some(detected_row)
    .flatten();
    *previous_signature = current_signature;
    suppress_row
}

fn looks_like_windows_workspace_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/'))
        || path.starts_with(r"\\")
}

fn line_counts(lines: &[String]) -> HashMap<&str, usize> {
    let mut counts = HashMap::new();
    for line in lines {
        if !line.is_empty() {
            *counts.entry(line.as_str()).or_default() += 1;
        }
    }
    counts
}

fn trim_value_for_ui(value: Value) -> Value {
    let Ok(serialized) = serde_json::to_string(&value) else {
        return json!({ "truncated": true });
    };
    if serialized.chars().count() <= MAX_UI_VALUE_CHARS {
        return value;
    }
    json!({
        "truncated": true,
        "preview": serialized.chars().take(MAX_UI_VALUE_CHARS).collect::<String>()
    })
}

fn status_for_capture(capture: &CaptureState, available: bool) -> ClaudeObserverStatus {
    ClaudeObserverStatus {
        tab_id: capture.tab_id.unwrap_or_default(),
        status_revision: capture.status_revision,
        capture_id: Some(capture.capture_id.clone()),
        available,
        active: capture.active,
        degraded_reason: capture.log_error.clone(),
        log_dir: Some(capture.log_dir.to_string_lossy().to_string()),
        terminal_prompt: capture.terminal_prompt.clone(),
        activity_status: capture.activity_status.clone(),
        current_model: capture.current_model.clone(),
        permission_mode: capture.permission_mode.clone(),
        context_usage: capture.context_usage.clone(),
    }
}

fn unavailable_snapshot(tab_id: u32, reason: String) -> ClaudeObserverSnapshot {
    ClaudeObserverSnapshot {
        tab_id,
        status_revision: 0,
        capture_id: None,
        available: false,
        active: false,
        degraded_reason: Some(reason),
        log_dir: None,
        events: Vec::new(),
        terminal_log: String::new(),
        terminal_prompt: None,
        activity_status: None,
        current_model: None,
        permission_mode: Some("? for shortcuts".to_string()),
        context_usage: None,
    }
}

fn write_json_pretty(path: &Path, value: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    fs::write(path, bytes)
}

fn append_json_line(path: &Path, value: &Value, max_bytes: u64) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    append_limited(path, &bytes, max_bytes)
}

fn append_limited(path: &Path, bytes: &[u8], max_bytes: u64) -> std::io::Result<()> {
    if path
        .metadata()
        .is_ok_and(|metadata| metadata.len() >= max_bytes)
    {
        let rotated = path.with_extension(format!(
            "{}.1",
            path.extension()
                .and_then(|value| value.to_str())
                .unwrap_or("log")
        ));
        let _ = fs::remove_file(&rotated);
        fs::rename(path, rotated)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(bytes)
}

fn read_last_lines(path: &Path, max_lines: usize) -> std::io::Result<String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error),
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    Ok(lines[start..].join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_prompt_serializes_struct_variant_fields_as_camel_case() {
        let plugin_prompt = ClaudeTerminalPrompt::PluginInstall {
            plugin_name: "example-plugin".to_string(),
            prompt: "Install plugin?".to_string(),
            options: vec!["Yes".to_string(), "No".to_string()],
        };
        let plugin_json = serde_json::to_value(plugin_prompt).expect("plugin prompt json");
        assert_eq!(plugin_json["pluginName"], "example-plugin");
        assert!(plugin_json.get("plugin_name").is_none());

        let confirmation = ClaudeTerminalPrompt::ModelSwitchConfirm {
            prompt: "Switch model?".to_string(),
            options: vec!["Yes".to_string(), "No".to_string()],
            selected_index: 0,
        };
        let confirmation_json = serde_json::to_value(confirmation).expect("confirmation json");
        assert_eq!(confirmation_json["kind"], "modelSwitchConfirm");
        assert_eq!(confirmation_json["selectedIndex"], 0);
    }

    #[test]
    fn stable_screen_diff_drops_identical_redraws() {
        let mut capture = ScreenCapture::new(4, 40);
        capture.process(b"hello");
        let first = capture.take_diff(Instant::now(), true).expect("first diff");
        assert_eq!(first.changed_rows.len(), 1);
        assert_eq!(first.changed_rows[0].text, "hello");

        capture.process(b"\rhello");
        assert!(capture.take_diff(Instant::now(), true).is_none());
    }

    #[test]
    fn screen_diff_keeps_real_repeated_lines_on_distinct_rows() {
        let mut capture = ScreenCapture::new(4, 40);
        capture.process(b"same\r\nsame");
        let diff = capture
            .take_diff(Instant::now(), true)
            .expect("screen diff");
        assert_eq!(diff.changed_rows.len(), 2);
        assert_eq!(diff.changed_rows[0].text, "same");
        assert_eq!(diff.changed_rows[1].text, "same");
    }

    #[test]
    fn screen_diff_does_not_relog_lines_that_only_scrolled() {
        let mut capture = ScreenCapture::new(3, 40);
        capture.process(b"one\r\ntwo\r\nthree");
        capture
            .take_diff(Instant::now(), true)
            .expect("initial diff");

        capture.process(b"\r\nfour");
        let diff = capture
            .take_diff(Instant::now(), true)
            .expect("scroll diff");
        assert_eq!(diff.changed_rows.len(), 1);
        assert_eq!(diff.changed_rows[0].text, "four");
    }

    #[test]
    fn stable_screen_update_survives_a_deletion_only_change() {
        let mut capture = ScreenCapture::new(3, 40);
        capture.process(b"one\r\ntwo");
        capture
            .take_stable_update(Instant::now(), true)
            .expect("initial stable update");

        capture.process(b"\x1b[2J\x1b[Hone");
        let update = capture
            .take_stable_update(Instant::now(), true)
            .expect("deletion-only stable update");

        assert!(update.diff.is_none());
        assert_eq!(update.latest_screen, "one");
    }

    #[test]
    fn forensic_tail_keeps_fast_output_that_left_the_visible_screen() {
        let mut capture = ScreenCapture::new(3, 40);
        let output = (0..100)
            .map(|index| format!("forensic-line-{index}\r\n"))
            .collect::<String>();
        capture.process(output.as_bytes());
        let lines = capture.take_forensic_lines(true);

        assert!(lines.iter().any(|line| line == "forensic-line-0"));
        assert!(lines.iter().any(|line| line == "forensic-line-99"));
    }

    #[test]
    fn forensic_tail_collapses_recent_repeated_redraw_text() {
        let mut capture = ScreenCapture::new(3, 40);
        capture.process(b"same\r\nsame\r\nsame\r\n");
        let lines = capture.take_forensic_lines(true);

        assert_eq!(
            lines.iter().filter(|line| line.as_str() == "same").count(),
            1
        );
        assert!(lines.iter().any(|line| line.contains('2')));
    }

    #[test]
    fn sensitive_key_detection_is_narrow_enough_for_known_credentials() {
        assert!(is_sensitive_key("ANTHROPIC_API_KEY"));
        assert!(is_sensitive_key("access_token"));
        assert!(is_sensitive_key("x-api-key"));
        assert!(is_sensitive_key("token"));
        assert!(is_sensitive_key("Cookie"));
        assert!(is_sensitive_key("private-key"));
        assert!(is_sensitive_container("headers"));
        assert!(is_sensitive_container("request.headers"));
        assert!(!is_sensitive_key("token_budget"));
        assert!(!is_sensitive_key("model"));
    }

    #[test]
    fn text_redaction_removes_quoted_json_credentials_not_present_in_env() {
        let bearer = Regex::new(r"(?i)(bearer\s+)[A-Za-z0-9._~+/=-]+").expect("bearer regex");
        let assignment = Regex::new(SECRET_ASSIGNMENT_PATTERN).expect("assignment regex");
        let input = r#"{"headers":{"x-api-key":"sk-live-abc","token":"tok-value","Authorization":"Basic abc def"}}"#;

        let redacted = redact_text_with_patterns(input, &[], &bearer, &assignment);

        assert!(!redacted.contains("sk-live-abc"));
        assert!(!redacted.contains("tok-value"));
        assert!(!redacted.contains("Basic abc def"));
        assert_eq!(redacted.matches("[REDACTED]").count(), 3);
    }

    #[test]
    fn transcript_deduplication_key_is_windows_path_insensitive() {
        assert_eq!(
            normalize_transcript_key(Path::new(r"C:\Users\Alice\.claude\SESSION.JSONL")),
            normalize_transcript_key(Path::new("c:/users/alice/.claude/session.jsonl")),
        );
    }

    #[test]
    fn transcript_parser_keeps_text_and_omits_internal_blocks() {
        let transcript = concat!(
            r#"{"type":"user","timestamp":"2026-07-21T00:00:00Z","message":{"content":"hello"}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-07-21T00:00:01Z","message":{"content":[{"type":"thinking","thinking":"private reasoning"},{"type":"text","text":"answer"},{"type":"tool_use","name":"Bash"}]}}"#,
            "\n",
            r#"{"type":"user","isSidechain":true,"message":{"content":"subagent"}}"#,
        );
        let messages = parse_transcript_messages(transcript);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].event_name, "HistoricalUserMessage");
        assert_eq!(messages[0].payload["text"], "hello");
        assert_eq!(messages[1].event_name, "HistoricalAssistantMessage");
        assert_eq!(messages[1].payload["text"], "answer");
    }

    #[test]
    fn transcript_parser_includes_local_command_output() {
        let transcript = concat!(
            r#"{"type":"user","timestamp":"2026-07-21T00:00:00Z","message":{"content":"/compact"}}"#,
            "\n",
            r#"{"type":"system","subtype":"local_command","timestamp":"2026-07-21T00:00:01Z","content":"<local-command-stdout>Not enough messages to compact.</local-command-stdout>"}"#,
        );
        let events = parse_transcript_messages(transcript);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_name, "HistoricalUserMessage");
        assert_eq!(events[1].event_name, "HistoricalLocalCommand");
        assert_eq!(
            events[1].payload["text"],
            "<local-command-stdout>Not enough messages to compact.</local-command-stdout>"
        );
    }

    #[test]
    fn transcript_submission_confirmation_only_matches_appended_user_prompt() {
        let appended = concat!(
            r#"{"type":"user","timestamp":"2026-07-21T00:00:02Z","message":{"content":"  hello\r\nworld  "}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-07-21T00:00:03Z","message":{"content":"answer"}}"#,
        );

        assert!(transcript_contains_user_prompt(appended, "hello\nworld"));
        assert!(!transcript_contains_user_prompt(appended, "older prompt"));
    }

    #[test]
    fn statusline_context_prefers_native_percentage_and_reports_token_breakdown() {
        let usage = context_usage_from_statusline(&json!({
            "context_window": {
                "used_percentage": 42.4,
                "remaining_percentage": 57.6,
                "context_window_size": 200000,
                "current_usage": {
                    "input_tokens": 12000,
                    "cache_creation_input_tokens": 3000,
                    "cache_read_input_tokens": 69000,
                    "output_tokens": 99999
                }
            }
        }))
        .expect("context usage");

        assert_eq!(usage.used_percentage, 42);
        assert_eq!(usage.remaining_percentage, 58);
        assert_eq!(usage.used_tokens, Some(84_000));
        assert_eq!(usage.context_window_size, Some(200_000));
        assert_eq!(usage.source, "native");
    }

    #[test]
    fn statusline_context_calculates_initial_zero_frame_from_tokens() {
        let usage = context_usage_from_statusline(&json!({
            "context_window": {
                "used_percentage": 0,
                "context_window_size": 200000,
                "current_usage": {
                    "input_tokens": 10000,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 30000
                }
            }
        }))
        .expect("context usage");

        assert_eq!(usage.used_percentage, 20);
        assert_eq!(usage.remaining_percentage, 80);
    }

    #[test]
    fn transcript_context_uses_latest_main_assistant_usage() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("session.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"type":"assistant","message":{"usage":{"input_tokens":1000,"cache_creation_input_tokens":2000,"cache_read_input_tokens":3000,"output_tokens":90000}}}"#,
                "\n",
                r#"{"type":"assistant","isSidechain":true,"message":{"usage":{"input_tokens":190000}}}"#,
                "\n",
                r#"{"type":"assistant","message":{"usage":{"input_tokens":25000,"cache_creation_input_tokens":5000,"cache_read_input_tokens":70000,"output_tokens":50000}}}"#,
            ),
        )
        .expect("write transcript");

        let usage = read_transcript_context_usage(&path, 200_000)
            .expect("read transcript")
            .expect("context usage");
        assert_eq!(usage.used_tokens, Some(100_000));
        assert_eq!(usage.used_percentage, 50);
        assert_eq!(usage.source, "transcript");
    }

    #[test]
    fn statusline_resolution_uses_local_project_override() {
        let directory = tempfile::tempdir().expect("temp directory");
        let config_dir = directory.path().join("config");
        let project_dir = directory.path().join("project");
        fs::create_dir_all(&config_dir).expect("config directory");
        fs::create_dir_all(project_dir.join(".claude")).expect("project config directory");
        fs::write(
            config_dir.join("settings.json"),
            r#"{"statusLine":{"type":"command","command":"user-hud"}}"#,
        )
        .expect("user settings");
        fs::write(
            project_dir.join(".claude").join("settings.local.json"),
            r#"{"statusLine":{"type":"command","command":"project-hud"}}"#,
        )
        .expect("project settings");
        let env = HashMap::from([(
            "CLAUDE_CONFIG_DIR".to_string(),
            config_dir.to_string_lossy().to_string(),
        )]);

        assert_eq!(
            resolve_existing_statusline_command(
                &env,
                Some(project_dir.to_string_lossy().as_ref()),
            )
            .expect("resolve statusline"),
            Some("project-hud".to_string()),
        );
    }

    #[test]
    fn historical_log_lookup_skips_newer_metadata_only_capture() {
        let root =
            std::env::temp_dir().join(format!("agents-launcher-log-test-{}", Uuid::new_v4()));
        let older = root.join("older");
        let newer = root.join("newer");
        fs::create_dir_all(&older).expect("older log dir");
        fs::create_dir_all(&newer).expect("newer log dir");
        write_json_pretty(
            &older.join("metadata.json"),
            &json!({
                "projectSessionId": "session-1",
                "startedAt": "2026-07-27T01:34:52Z",
            }),
        )
        .expect("older metadata");
        fs::write(older.join("terminal-output.txt"), "captured output")
            .expect("older terminal log");
        write_json_pretty(
            &newer.join("metadata.json"),
            &json!({
                "projectSessionId": "session-1",
                "startedAt": "2026-07-27T01:43:50Z",
            }),
        )
        .expect("newer metadata");

        assert_eq!(
            find_latest_project_log_dir(&root, "session-1", true),
            Some(older)
        );
        fs::remove_dir_all(root).expect("remove log fixture");
    }

    #[test]
    fn transcript_parser_reconstructs_historical_tool_execution_in_order() {
        let transcript = concat!(
            r#"{"type":"assistant","timestamp":"2026-07-21T00:00:00Z","message":{"content":[{"type":"text","text":"checking"}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-07-21T00:00:01Z","message":{"content":[{"type":"thinking","thinking":"private"},{"type":"tool_use","id":"tool-1","name":"Grep","input":{"pattern":"public camera","path":"docs"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-07-21T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"docs/plan.html:77: public camera"}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-07-21T00:00:03Z","message":{"content":[{"type":"tool_use","id":"tool-2","name":"Edit","input":{"file_path":"docs/plan.html","old_string":"before","new_string":"after"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-07-21T00:00:04Z","message":{"content":[{"type":"tool_result","tool_use_id":"tool-2","content":"updated"}]}}"#,
        );
        let events = parse_transcript_messages(transcript);

        assert_eq!(
            events
                .iter()
                .map(|event| event.event_name)
                .collect::<Vec<_>>(),
            vec![
                "HistoricalAssistantMessage",
                "PreToolUse",
                "PostToolUse",
                "PreToolUse",
                "PostToolUse",
            ]
        );
        assert_eq!(events[1].payload["tool_name"], "Grep");
        assert_eq!(events[1].payload["tool_input"]["pattern"], "public camera");
        assert_eq!(events[2].payload["tool_name"], "Grep");
        assert_eq!(
            events[2].payload["tool_response"],
            "docs/plan.html:77: public camera"
        );
        assert_eq!(events[3].payload["tool_name"], "Edit");
        assert_eq!(events[4].payload["tool_response"], "updated");
    }

    #[test]
    fn hook_body_parser_accepts_utf8_bom_and_utf16_with_chinese_paths() {
        let value = json!({
            "hook_event_name": "SessionStart",
            "cwd": r"D:\work\体感游戏",
        });
        let text = serde_json::to_string(&value).expect("hook json");

        let mut utf8_bom = vec![0xEF, 0xBB, 0xBF];
        utf8_bom.extend_from_slice(text.as_bytes());
        assert_eq!(parse_hook_body(&utf8_bom).expect("UTF-8 BOM"), value);

        let mut utf16_le = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            utf16_le.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(parse_hook_body(&utf16_le).expect("UTF-16LE"), value);
    }

    #[test]
    fn hook_body_parser_reports_encoding_without_echoing_invalid_content() {
        let invalid = b"{\"cwd\":\"\xD6\xD0-secret-value\"}";
        let error = parse_hook_body(invalid).expect_err("invalid UTF-8");

        assert!(error.contains("不是 UTF-8"));
        assert!(!error.contains("secret-value"));
    }

    #[test]
    fn normalize_screen_removes_blank_tail_not_duplicate_content() {
        assert_eq!(normalize_screen("one\none\n\n"), "one\none");
    }

    #[test]
    fn detects_current_model_from_claude_footer() {
        let screen = concat!(
            "Set model to Haiku 4.5 and saved as your default for new sessions\n",
            "[Sonnet 5] | 30919\n",
            "Context 22%\n",
            "bypass permissions on",
        );

        assert_eq!(detect_claude_current_model(screen).as_deref(), Some("Sonnet 5"));
    }

    #[test]
    fn current_model_detection_ignores_model_command_history() {
        let screen = concat!(
            "[Haiku 4.5] historical text without footer separator\n",
            "Set model to Sonnet 5 and saved as your default for new sessions",
        );

        assert_eq!(detect_claude_current_model(screen), None);
    }

    #[test]
    fn detects_permission_mode_from_claude_footer() {
        assert_eq!(
            detect_claude_permission_mode("⏵⏵ plan mode on (shift+tab to cycle)").as_deref(),
            Some("⏵⏵ plan mode")
        );
        assert_eq!(
            detect_claude_permission_mode("⏵⏵ auto-accept edits on (shift + tab to cycle)")
                .as_deref(),
            Some("⏵⏵ auto-accept edits")
        );
        assert_eq!(
            detect_claude_permission_mode("⏵⏵ bypass permissions on (shift+tab to cycle)")
                .as_deref(),
            Some("⏵⏵ bypass permissions")
        );
        assert_eq!(detect_claude_permission_mode("plan mode on"), None);
    }

    #[test]
    fn detects_plan_approval_options_from_terminal() {
        let screen = concat!(
            "Plan complete. Ready to code?\n",
            "❯ 1. Start coding\n",
            "  2. Keep planning\n",
            "Enter to confirm · Esc to cancel",
        );
        assert_eq!(
            detect_terminal_prompt(screen),
            Some(ClaudeTerminalPrompt::PlanApproval {
                prompt: "Plan complete. Ready to code?".to_string(),
                options: vec!["Start coding".to_string(), "Keep planning".to_string()],
                selected_index: 0,
            })
        );
    }

    #[test]
    fn parses_classic_claude_activity_status() {
        let status =
            parse_claude_activity_status_line("  ✻ Actioning… (7s · ↓ 200 tokens · thinking)  ")
                .expect("Claude activity status");

        assert_eq!(
            status,
            ClaudeActivityStatus {
                label: "Actioning".to_string(),
                elapsed: Some("7s".to_string()),
                token_direction: Some("↓".to_string()),
                token_count: Some("200".to_string()),
                phase: Some("thinking".to_string()),
            }
        );
    }

    #[test]
    fn recognizes_context_compaction_activity_without_hook_activity() {
        let status = parse_claude_activity_status_line("* Compacting conversation...")
            .expect("compaction activity status");

        assert!(is_claude_compaction_activity(&status));
    }

    #[test]
    fn parses_activity_status_with_long_duration_and_compact_token_count() {
        let status = parse_claude_activity_status_line(
            "✽ Thinking... (1m 13s · ↑ 1.2k tokens · responding)",
        )
        .expect("Claude activity status");

        assert_eq!(status.label, "Thinking");
        assert_eq!(status.elapsed.as_deref(), Some("1m 13s"));
        assert_eq!(status.token_direction.as_deref(), Some("↑"));
        assert_eq!(status.token_count.as_deref(), Some("1.2k"));
        assert_eq!(status.phase.as_deref(), Some("responding"));
    }

    #[test]
    fn parses_progressive_activity_formats_from_real_terminal_output() {
        let bare = parse_claude_activity_status_line("· Misting…").expect("bare activity");
        assert_eq!(bare.label, "Misting");
        assert_eq!(bare.elapsed, None);
        assert_eq!(bare.token_count, None);

        let elapsed =
            parse_claude_activity_status_line("* Misting… (23s)").expect("elapsed activity");
        assert_eq!(elapsed.elapsed.as_deref(), Some("23s"));
        assert_eq!(elapsed.phase, None);

        let phase = parse_claude_activity_status_line("✻ Transfiguring… (29s · thinking)")
            .expect("phase activity");
        assert_eq!(phase.elapsed.as_deref(), Some("29s"));
        assert_eq!(phase.phase.as_deref(), Some("thinking"));
        assert_eq!(phase.token_count, None);

        let complete = parse_claude_activity_status_line(
            "✶ Transfiguring… (32s · ↓ 72 tokens · thought for 2s)",
        )
        .expect("complete activity");
        assert_eq!(complete.token_direction.as_deref(), Some("↓"));
        assert_eq!(complete.token_count.as_deref(), Some("72"));
        assert_eq!(complete.phase.as_deref(), Some("thought for 2s"));
    }

    #[test]
    fn activity_status_detection_ignores_completed_thought_summaries() {
        let screen = concat!(
            "Thought for 20s, ran 1 shell command\n",
            "✻ Cogitated for 25s\n",
            "normal response text",
        );

        assert!(detect_claude_activity_status(screen).is_none());
        assert_eq!(
            parse_claude_activity_status_line("Actioning… (7s · ↓ 200 tokens · thinking)"),
            None,
            "ordinary transcript text without Claude's spinner must not become live status",
        );
        assert!(
            parse_claude_activity_status_line("- Actioning… (7s · ↓ 200 tokens · thinking)")
                .is_none(),
        );
        assert!(
            parse_claude_activity_status_line("> Actioning… (7s · ↓ 200 tokens · thinking)")
                .is_none(),
        );
    }

    #[test]
    fn activity_status_detection_is_limited_to_the_bottom_terminal_region() {
        let mut lines = vec!["✻ Actioning… (7s · ↓ 200 tokens · thinking)".to_string()];
        lines.extend((0..12).map(|index| format!("ordinary terminal line {index}")));

        assert!(detect_claude_activity_status(&lines.join("\n")).is_none());
    }

    #[test]
    fn forensic_tail_keeps_one_activity_sample_and_folds_refreshes() {
        let mut capture = ScreenCapture::new(6, 80);
        capture.process(
            concat!(
                "✻ Actioning… (7s · ↓ 200 tokens · thinking)\r\n",
                "✽ Actioning… (8s · ↓ 220 tokens · thinking)\r\n",
            )
            .as_bytes(),
        );
        let lines = capture.take_forensic_lines(true);

        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("Actioning"))
                .count(),
            1,
        );
    }

    #[test]
    fn screen_diff_folds_activity_refreshes_without_hook_state() {
        let first =
            parse_claude_activity_status_line("✻ Actioning… (7s · ↓ 200 tokens · thinking)")
                .expect("first activity");
        let second =
            parse_claude_activity_status_line("✽ Actioning… (8s · ↓ 220 tokens · thinking)")
                .expect("second activity");
        let mut previous_signature = None;

        assert_eq!(
            activity_diff_row_to_suppress(&mut previous_signature, Some(29), Some(&first)),
            None,
            "the first representative status line stays in the fallback log",
        );
        assert_eq!(
            activity_diff_row_to_suppress(&mut previous_signature, Some(29), Some(&second)),
            Some(29),
            "later redraws fold even when no Hook marked the turn as active",
        );
    }

    #[test]
    fn detects_workspace_trust_prompt_and_extracts_path() {
        let screen = concat!(
            "Accessing workspace:\n\n",
            " C:\\Users\\30919\n\n",
            " Quick safety check: Is this a project you created or one you trust?\n\n",
            " ❯ 1. Yes, I trust this folder\n",
            "   2. No, exit\n\n",
            " Enter to confirm · Esc to cancel",
        );

        let prompt = detect_terminal_prompt(screen);
        assert_eq!(
            prompt,
            Some(ClaudeTerminalPrompt::WorkspaceTrust {
                path: r"C:\Users\30919".to_string(),
            })
        );
        assert_eq!(
            serde_json::to_value(prompt).expect("serialize prompt")["kind"],
            "workspaceTrust"
        );
    }

    #[test]
    fn workspace_trust_detection_rejects_partial_terminal_text() {
        let screen = concat!(
            "Accessing workspace:\n\n",
            " C:\\Users\\30919\n\n",
            "Quick safety check:\n",
            "Yes, I trust this folder\n",
            "No, exit",
        );

        assert_eq!(detect_terminal_prompt(screen), None);
    }

    #[test]
    fn workspace_trust_detection_rejects_reordered_or_quoted_text() {
        let reordered = concat!(
            "Accessing workspace:\n",
            " C:\\Users\\30919\n",
            " Quick safety check:\n",
            " 2. No, exit\n",
            " ❯ 1. Yes, I trust this folder\n",
            " Enter to confirm · Esc to cancel",
        );
        assert_eq!(detect_terminal_prompt(reordered), None);

        let quoted = concat!(
            "Accessing workspace:\n",
            " C:\\Users\\30919\n",
            " Quick safety check:\n",
            " ❯ 1. Yes, I trust this folder\n",
            " 2. No, exit\n",
            " Enter to confirm · Esc to cancel\n",
            "Claude: this is only an example copied into the conversation",
        );
        assert_eq!(detect_terminal_prompt(quoted), None);
    }

    #[test]
    fn detects_plugin_install_prompt_and_extracts_options() {
        let screen = concat!(
            "Install plugin `demo-tools`?\n",
            "\n",
            " ❯ 1. Install now\n",
            "   2. Cancel\n",
            "\n",
            " Enter to select · Esc to cancel",
        );

        let detected = detect_terminal_prompt(screen);
        assert_eq!(
            detected,
            Some(ClaudeTerminalPrompt::PluginInstall {
                plugin_name: "demo-tools".to_string(),
                prompt: "Install plugin `demo-tools`?".to_string(),
                options: vec!["Install now".to_string(), "Cancel".to_string()],
            })
        );
    }

    #[test]
    fn plugin_install_detection_rejects_unrelated_numbered_text() {
        let screen = concat!(
            "The conversation mentions a plugin earlier.\n",
            "1. First item\n",
            "2. Second item\n",
            "Enter to select · Esc to cancel",
        );

        assert_eq!(detect_terminal_prompt(screen), None);
    }

    #[test]
    fn plugin_install_detection_clears_when_terminal_content_continues() {
        let screen = concat!(
            "Install plugin `demo-tools`?\n",
            " ❯ 1. Install now\n",
            "   2. Cancel\n",
            " Enter to select · Esc to cancel\n",
            "Claude is continuing the current task...",
        );

        assert_eq!(detect_terminal_prompt(screen), None);
    }

    #[test]
    fn ignores_model_select_prompt_in_structured_observer() {
        let screen = concat!(
            "Select model\n",
            "Switch between Claude models.\n",
            "1. Default model\n",
            "2. claude-opus-4-8 Custom Opus model\n",
            "❯ 3. claude-sonnet-5 Custom Sonnet model\n",
            "Enter to set as default · s to use this session only · Esc to cancel",
        );

        assert_eq!(detect_terminal_prompt(screen), None);
    }

    #[test]
    fn detects_model_switch_confirmation_from_wrapped_terminal_layout() {
        let screen = concat!(
            "Set model to Sonnet 5 and saved as your default for new sessions\n",
            "Switch model? Your next response will be slower and use more tokens\n",
            "This conversation is cached for the current model. Switching to Sonnet 5 means the full history gets re-read on your next message. > 1. Yes, switch to Sonnet 5\n",
            "2. No, go back",
        );

        assert_eq!(
            detect_terminal_prompt(screen),
            Some(ClaudeTerminalPrompt::ModelSwitchConfirm {
                prompt: "Switch model? Your next response will be slower and use more tokens This conversation is cached for the current model. Switching to Sonnet 5 means the full history gets re-read on your next message.".to_string(),
                options: vec![
                    "Yes, switch to Sonnet 5".to_string(),
                    "No, go back".to_string(),
                ],
                selected_index: 0,
            })
        );
    }

    #[test]
    fn model_switch_confirmation_clears_when_terminal_output_continues() {
        let screen = concat!(
            "Switch model?\n",
            "This conversation is cached for the current model.\n",
            "> 1. Yes, switch to Sonnet 5\n",
            "2. No, go back\n",
            "Model switch completed",
        );

        assert_eq!(detect_terminal_prompt(screen), None);
    }

    #[test]
    fn old_model_select_text_does_not_create_terminal_prompt() {
        let screen = concat!(
            "Select model\n",
            "1. Default\n",
            "2. Sonnet\n",
            "● High effort (default) ←/→ to adjust\n",
            "Enter to set as default · s to use this session only · Esc to cancel\n",
            "Claude is continuing the current task...",
        );

        assert_eq!(detect_terminal_prompt(screen), None);
    }

    #[test]
    fn observer_plugin_uses_supported_session_start_transport() {
        let root =
            std::env::temp_dir().join(format!("agents-launcher-observer-test-{}", Uuid::new_v4()));
        write_observer_plugin(&root, "http://127.0.0.1:43210", "capture-test")
            .expect("plugin files");

        let hooks: Value = serde_json::from_str(
            &fs::read_to_string(root.join("hooks").join("hooks.json")).expect("hooks file"),
        )
        .expect("valid hooks json");
        assert_eq!(
            hooks["hooks"]["SessionStart"][0]["hooks"][0]["type"],
            "command"
        );
        assert_eq!(
            hooks["hooks"]["MessageDisplay"][0]["hooks"][0]["type"],
            "http"
        );
        assert!(root.join("scripts").join("session-start.ps1").is_file());
        let session_start = fs::read_to_string(root.join("scripts").join("session-start.ps1"))
            .expect("session start script");
        assert!(session_start.contains("[Console]::OpenStandardInput()"));
        assert!(session_start.contains("$inputStream.CopyTo($memoryStream)"));
        assert!(session_start.contains("-Body $bytes"));

        fs::remove_dir_all(root).expect("remove test plugin");
    }
}
