pub mod session;

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use base64::Engine;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

pub use session::PtySession;

use crate::tab_cli::TabPermission;

// ── Event payloads ────────────────────────────────────────────────────────────

#[derive(Clone, Serialize)]
struct PtyOutputPayload {
    tab_id: u32,
    data: String,
}

#[derive(Clone, Serialize)]
struct PtyStatusPayload {
    tab_id: u32,
    alive: bool,
}

#[derive(Clone, Serialize)]
struct PtyTitlePayload {
    tab_id: u32,
    title: String,
    has_spinner: bool,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn extract_osc0_title(data: &[u8]) -> Option<String> {
    let prefix = b"\x1b]0;";
    let mut last_title = None;
    let mut i = 0;
    while i < data.len().saturating_sub(prefix.len()) {
        if data[i..].starts_with(prefix) {
            let start = i + prefix.len();
            let mut end = start;
            while end < data.len() {
                if data[end] == 0x07 { break; }
                if data[end] == 0x1b && end + 1 < data.len() && data[end + 1] == b'\\' { break; }
                end += 1;
            }
            if end > start && end < data.len() {
                if let Ok(title) = String::from_utf8(data[start..end].to_vec()) {
                    last_title = Some(title);
                }
            }
            i = end + 1;
        } else {
            i += 1;
        }
    }
    last_title
}

fn is_claude_working(title: &str) -> bool {
    let first_char = title.chars().next();
    matches!(first_char, Some('⠂') | Some('⠐'))
}

/// Check if the buffer starts with "tab-" (ignoring leading whitespace/control chars).
fn is_tab_command_prefix(buf: &[u8]) -> bool {
    let prefix = b"tab-";
    let trimmed = buf.iter().position(|&b| b != b' ' && b != b'\t' && b != b'\r' && b != b'\n');
    match trimmed {
        Some(start) if start + prefix.len() <= buf.len() => &buf[start..start + prefix.len()] == prefix,
        _ => false,
    }
}

// ── Manager ───────────────────────────────────────────────────────────────────

pub struct PtyManager {
    pub sessions: HashMap<u32, PtySession>,
    pub next_id: u32,
    /// Pending --wait replies keyed by caller tab ID.
    pub pending_replies: HashMap<u32, Vec<crate::tab_cli::PendingReply>>,
    /// Per-tab line buffer for tab-* command detection (separate to avoid borrow conflicts).
    pub line_buffers: HashMap<u32, Vec<u8>>,
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            next_id: 1,
            pending_replies: HashMap::new(),
            line_buffers: HashMap::new(),
        }
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn pty_create(
    cmd: Vec<String>,
    env: HashMap<String, String>,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
    session_id: Option<String>,
    app: AppHandle,
    state: State<'_, Mutex<PtyManager>>,
) -> Result<u32, String> {
    if cmd.is_empty() {
        return Err("cmd must not be empty".into());
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("openpty failed: {e}"))?;

    #[cfg(windows)]
    let exe = if cmd[0].eq_ignore_ascii_case("cmd.exe") || cmd[0].eq_ignore_ascii_case("cmd") {
        env.get("COMSPEC")
            .cloned()
            .or_else(|| std::env::var("COMSPEC").ok())
            .unwrap_or_else(|| "C:\\Windows\\System32\\cmd.exe".to_string())
    } else {
        cmd[0].clone()
    };
    #[cfg(not(windows))]
    let exe = cmd[0].clone();

    let mut cmd_builder = CommandBuilder::new(&exe);
    for arg in &cmd[1..] {
        cmd_builder.arg(arg);
    }
    for (k, v) in std::env::vars() {
        cmd_builder.env(&k, &v);
    }
    for (k, v) in &env {
        cmd_builder.env(k, v);
    }
    cmd_builder.env("TERM", "xterm-256color");
    cmd_builder.env("COLORTERM", "truecolor");
    cmd_builder.env_remove("WT_SESSION");
    cmd_builder.env_remove("WT_PROFILE_ID");

    if let Some(ref dir) = cwd {
        cmd_builder.cwd(dir);
    }

    let child = pair.slave.spawn_command(cmd_builder)
        .map_err(|e| format!("spawn failed: {e}"))?;
    let child_pid = child.process_id();

    let master = pair.master;
    let writer = master.take_writer()
        .map_err(|e| format!("take_writer failed: {e}"))?;
    let mut reader = master.try_clone_reader()
        .map_err(|e| format!("clone reader failed: {e}"))?;

    let output_lines: Arc<std::sync::Mutex<VecDeque<String>>> =
        Arc::new(std::sync::Mutex::new(VecDeque::with_capacity(501)));
    let reader_output_lines = output_lines.clone();

    let title: Arc<std::sync::Mutex<String>> = Arc::new(std::sync::Mutex::new(String::new()));
    let reader_title = title.clone();

    let tab_id = {
        let mut mgr = state.lock().map_err(|e| e.to_string())?;
        let id = mgr.next_id;
        mgr.next_id += 1;
        mgr.sessions.insert(id, PtySession {
            master,
            writer,
            child,
            child_pid,
            output_lines,
            permission: TabPermission::default(),
            title,
            session_id,
        });
        mgr.line_buffers.insert(id, Vec::new());
        id
    };

    let app_clone = app.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Some(title_str) = extract_osc0_title(&buf[..n]) {
                        // Update session title for backend use (e.g. tab-list)
                        if let Ok(mut t) = reader_title.lock() {
                            *t = title_str.clone();
                        }
                        let spinner = is_claude_working(&title_str);
                        let _ = app_clone.emit("pty_title", PtyTitlePayload { tab_id, title: title_str, has_spinner: spinner });
                    }

                    // Populate output_lines for tab-read command
                    let text = String::from_utf8_lossy(&buf[..n]);
                    for line in text.split(|c| c == '\n') {
                        let trimmed = line.trim_matches(|c: char| c == '\r');
                        if !trimmed.is_empty() {
                            if let Ok(mut deque) = reader_output_lines.lock() {
                                deque.push_back(trimmed.to_string());
                                while deque.len() > 500 {
                                    deque.pop_front();
                                }
                            }
                        }
                    }

                    let encoded = base64::engine::general_purpose::STANDARD.encode(&buf[..n]);
                    let _ = app_clone.emit("pty_output", PtyOutputPayload { tab_id, data: encoded });
                }
                Err(_) => break,
            }
        }
        let _ = app_clone.emit("pty_status", PtyStatusPayload { tab_id, alive: false });
    });

    Ok(tab_id)
}

#[tauri::command]
pub fn pty_write(
    tab_id: u32,
    data: String,
    _app: AppHandle,
    state: State<'_, Mutex<PtyManager>>,
) -> Result<(), String> {
    // Phase 1: extract line buffer as owned data (no borrows on mgr held)
    let mut line_buffer: Vec<u8>;
    {
        let mut mgr = state.lock().map_err(|e| e.to_string())?;
        line_buffer = mgr.line_buffers.remove(&tab_id).unwrap_or_default();
    }

    // Append incoming data (outside any lock)
    line_buffer.extend(data.as_bytes());

    // Phase 2: process with mgr lock — line_buffer is owned, independent of mgr
    {
        let mut mgr = state.lock().map_err(|e| e.to_string())?;

        // Step A: cleanup expired pending replies (two-phase to avoid closure borrows)
        let now = std::time::Instant::now();
        let expired_entries: Vec<(u32, crate::tab_cli::PendingReply)> = {
            let mut entries = Vec::new();
            let keys: Vec<u32> = mgr.pending_replies.keys().copied().collect();
            for caller_tab in keys {
                if let Some(list) = mgr.pending_replies.get_mut(&caller_tab) {
                    let mut i = 0;
                    while i < list.len() {
                        if now > list[i].deadline {
                            entries.push((caller_tab, list.remove(i)));
                        } else {
                            i += 1;
                        }
                    }
                    if list.is_empty() {
                        mgr.pending_replies.remove(&caller_tab);
                    }
                }
            }
            entries
        };
        // Write timeout notifications for expired entries
        for (caller_tab, entry) in expired_entries {
            if let Some(session) = mgr.sessions.get_mut(&caller_tab) {
                let timeout_msg = crate::tab_cli::format_message(&format!(
                    "send: Timeout waiting for reply from tab {} (message: {})",
                    entry.responder_tab, entry.original_message
                ));
                let _ = session.writer.write_all(timeout_msg.as_bytes());
            }
        }

        // Step B: early flush — if buffer doesn't start with "tab-", pass through immediately
        if !is_tab_command_prefix(&line_buffer) {
            let bytes_to_send = std::mem::take(&mut line_buffer);
            if let Some(session) = mgr.sessions.get_mut(&tab_id) {
                let _ = session.writer.write_all(&bytes_to_send);
            }
            mgr.line_buffers.insert(tab_id, line_buffer);
            return Ok(());
        }

        // Step C: buffer starts with "tab-" — process complete lines
        let mut passthrough_bytes: Vec<Vec<u8>> = Vec::new();

        loop {
            let newline_pos = match line_buffer.iter().position(|&b| b == b'\n' || b == b'\r') {
                Some(pos) => pos,
                None => break,
            };

            let line_bytes: Vec<u8> = line_buffer.drain(..=newline_pos).collect();
            let line_str = String::from_utf8_lossy(&line_bytes);
            let line_clean = line_str.trim_matches(|c: char| c == '\r' || c == '\n').to_string();

            if line_clean.starts_with("tab-") {
                if let Some(cmd) = crate::tab_cli::parse_tab_command(&line_clean) {
                    match crate::tab_cli::execute_command(&cmd, tab_id, &mut mgr) {
                        Ok(result) => {
                            // Write immediate output to caller's PTY (if any, None for --wait mode with deferred reply)
                            if let Some(output) = result.immediate_output {
                                if let Some(session) = mgr.sessions.get_mut(&tab_id) {
                                    let _ = session.writer.write_all(output.as_bytes());
                                }
                            }
                            // Apply pending actions (add/remove pending replies) -- critical for --wait mode
                            for action in result.pending_actions {
                                match action {
                                    crate::tab_cli::PendingAction::AddReply { caller_tab, entry } => {
                                        mgr.pending_replies.entry(caller_tab).or_default().push(entry);
                                    }
                                    crate::tab_cli::PendingAction::RemoveReply { caller_tab, index } => {
                                        if let Some(list) = mgr.pending_replies.get_mut(&caller_tab) {
                                            if index < list.len() {
                                                list.remove(index);
                                                if list.is_empty() {
                                                    mgr.pending_replies.remove(&caller_tab);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            if let Some(session) = mgr.sessions.get_mut(&tab_id) {
                                use std::io::Write;
                                let err_msg = crate::tab_cli::format_message(&format!("Error: {}", e));
                                let _ = session.writer.write_all(err_msg.as_bytes());
                            }
                        }
                    }
                    continue;
                }
            }

            // Not a tab command — collect for passthrough
            passthrough_bytes.push(line_bytes);
        }

        // Flush non-tab remaining buffer to passthrough
        if !is_tab_command_prefix(&line_buffer) {
            let remaining = std::mem::take(&mut line_buffer);
            if !remaining.is_empty() {
                passthrough_bytes.push(remaining);
            }
        }

        // Write all passthrough bytes to PTY
        for bytes in passthrough_bytes {
            if let Some(session) = mgr.sessions.get_mut(&tab_id) {
                let _ = session.writer.write_all(&bytes);
            }
        }

        // Restore the (possibly partially consumed) line buffer
        mgr.line_buffers.insert(tab_id, line_buffer);
    }

    Ok(())
}

#[tauri::command]
pub fn pty_resize(
    tab_id: u32, cols: u16, rows: u16,
    state: State<'_, Mutex<PtyManager>>,
) -> Result<(), String> {
    let mgr = state.lock().map_err(|e| e.to_string())?;
    let session = mgr.sessions.get(&tab_id)
        .ok_or_else(|| format!("no session for tab_id {tab_id}"))?;
    session.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("resize failed: {e}"))?;
    Ok(())
}

fn kill_process_tree(pid: u32) {
    // ponytail: taskkill covers the common Windows process-tree cleanup;
    // replace with CreateToolhelp32Snapshot if taskkill ever proves unreliable.
    let mut cmd = std::process::Command::new("taskkill");
    cmd.args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let _ = cmd.output();
}

#[tauri::command]
pub fn pty_kill(
    tab_id: u32,
    state: State<'_, Mutex<PtyManager>>,
) -> Result<(), String> {
    let mut mgr = state.lock().map_err(|e| e.to_string())?;
    kill_session(&mut mgr, tab_id);
    Ok(())
}

fn kill_session(mgr: &mut PtyManager, tab_id: u32) {
    if let Some(mut session) = mgr.sessions.remove(&tab_id) {
        // Drop the writer first to send EOF to the slave side.
        drop(session.writer);
        // Kill the direct child, then cascade to any grandchildren spawned
        // inside the PTY (e.g. node/python processes started by claude.exe).
        let _ = session.child.kill();
        if let Some(pid) = session.child_pid {
            kill_process_tree(pid);
        }
        // ponytail: drop the master immediately instead of sleeping while
        // holding the PTY manager lock; portable_pty cleans up on drop.
    }
    mgr.line_buffers.remove(&tab_id);
}

pub fn cleanup_all_sessions(app: &tauri::AppHandle) {
    let state = app.state::<Mutex<PtyManager>>();
    let Ok(mgr) = state.lock() else { return };
    let tab_ids: Vec<u32> = mgr.sessions.keys().copied().collect();
    drop(mgr);
    for tab_id in tab_ids {
        let Ok(mut mgr) = state.lock() else { return };
        kill_session(&mut mgr, tab_id);
    }
}
