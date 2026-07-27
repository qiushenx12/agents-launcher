pub mod session;

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use base64::Engine;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

pub use session::PtySession;

use crate::cli_contract::CliKind;
use crate::tab_cli::TabPermission;

// ── Event payloads ────────────────────────────────────────────────────────────

#[derive(Clone, Serialize)]
struct PtyOutputPayload {
    tab_id: u32,
    cli_kind: CliKind,
    data: String,
}

#[derive(Clone, Serialize)]
struct PtyStatusPayload {
    tab_id: u32,
    cli_kind: CliKind,
    alive: bool,
}

#[derive(Clone, Serialize)]
struct PtyTitlePayload {
    tab_id: u32,
    cli_kind: CliKind,
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
                if data[end] == 0x07 {
                    break;
                }
                if data[end] == 0x1b && end + 1 < data.len() && data[end + 1] == b'\\' {
                    break;
                }
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
    let trimmed = buf
        .iter()
        .position(|&b| b != b' ' && b != b'\t' && b != b'\r' && b != b'\n');
    match trimmed {
        Some(start) if start + prefix.len() <= buf.len() => {
            &buf[start..start + prefix.len()] == prefix
        }
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
    mut cmd: Vec<String>,
    mut env: HashMap<String, String>,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
    session_id: Option<String>,
    cli_kind: Option<CliKind>,
    observe_claude: Option<bool>,
    project_session_id: Option<String>,
    app: AppHandle,
    state: State<'_, Mutex<PtyManager>>,
    observer: State<'_, Arc<crate::claude_observer::ClaudeObserverManager>>,
) -> Result<u32, String> {
    if cmd.is_empty() {
        return Err("cmd must not be empty".into());
    }

    let cli_kind = cli_kind.unwrap_or_default();
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty failed: {e}"))?;

    let mut observer_error = None;
    let prepared_capture = if cli_kind == CliKind::Claude && observe_claude.unwrap_or(false) {
        match observer.prepare_capture(cols, rows, project_session_id, cwd.as_deref(), &env) {
            Ok(capture) => {
                cmd.insert(1, capture.settings_path.to_string_lossy().to_string());
                cmd.insert(1, "--settings".to_string());
                cmd.insert(1, capture.plugin_dir.to_string_lossy().to_string());
                cmd.insert(1, "--plugin-dir".to_string());
                env.insert(
                    "AGENTS_LAUNCHER_HOOK_TOKEN".to_string(),
                    capture.token.clone(),
                );
                Some(capture)
            }
            Err(error) => {
                observer_error = Some(error);
                None
            }
        }
    } else {
        None
    };

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
    let exe = if cmd[0].eq_ignore_ascii_case("cmd.exe") || cmd[0].eq_ignore_ascii_case("cmd") {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
    } else {
        cmd[0].clone()
    };

    let mut cmd_builder = CommandBuilder::new(&exe);
    for arg in &cmd[1..] {
        cmd_builder.arg(arg);
    }
    for (k, v) in std::env::vars() {
        cmd_builder.env(&k, &v);
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = crate::platform_env::effective_path() {
        cmd_builder.env("PATH", path);
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

    let child = match pair.slave.spawn_command(cmd_builder) {
        Ok(child) => child,
        Err(error) => {
            if let Some(capture) = prepared_capture.as_ref() {
                observer.abort_capture(&capture.capture_id);
            }
            return Err(format!("spawn failed: {error}"));
        }
    };
    let child_pid = child.process_id();

    let master = pair.master;
    let writer = master.take_writer().map_err(|e| {
        if let Some(capture) = prepared_capture.as_ref() {
            observer.abort_capture(&capture.capture_id);
        }
        format!("take_writer failed: {e}")
    })?;
    let mut reader = master.try_clone_reader().map_err(|e| {
        if let Some(capture) = prepared_capture.as_ref() {
            observer.abort_capture(&capture.capture_id);
        }
        format!("clone reader failed: {e}")
    })?;

    let output_lines: Arc<std::sync::Mutex<VecDeque<String>>> =
        Arc::new(std::sync::Mutex::new(VecDeque::with_capacity(501)));
    let reader_output_lines = output_lines.clone();

    let title: Arc<std::sync::Mutex<String>> = Arc::new(std::sync::Mutex::new(String::new()));
    let reader_title = title.clone();

    let tab_id = {
        let mut mgr = state.lock().map_err(|e| e.to_string())?;
        let id = mgr.next_id;
        mgr.next_id += 1;
        mgr.sessions.insert(
            id,
            PtySession {
                cli_kind,
                master,
                writer,
                child,
                child_pid,
                output_lines,
                permission: TabPermission::default(),
                title,
                session_id,
                capture_id: prepared_capture
                    .as_ref()
                    .map(|capture| capture.capture_id.clone()),
            },
        );
        mgr.line_buffers.insert(id, Vec::new());
        id
    };

    if let Some(capture) = prepared_capture.as_ref() {
        observer.bind_capture(&capture.capture_id, tab_id);
    } else if let Some(error) = observer_error {
        observer.emit_degraded(tab_id, error);
    }

    let app_clone = app.clone();
    let observer = Arc::clone(observer.inner());
    let capture_id = prepared_capture.map(|capture| capture.capture_id);
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Some(capture_id) = capture_id.as_deref() {
                        observer.record_pty(capture_id, &buf[..n]);
                    }
                    if let Some(title_str) = extract_osc0_title(&buf[..n]) {
                        // Update session title for backend use (e.g. tab-list)
                        if let Ok(mut t) = reader_title.lock() {
                            *t = title_str.clone();
                        }
                        let spinner = is_claude_working(&title_str);
                        let _ = app_clone.emit(
                            "pty_title",
                            PtyTitlePayload {
                                tab_id,
                                cli_kind,
                                title: title_str,
                                has_spinner: spinner,
                            },
                        );
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
                    let _ = app_clone.emit(
                        "pty_output",
                        PtyOutputPayload {
                            tab_id,
                            cli_kind,
                            data: encoded,
                        },
                    );
                }
                Err(_) => break,
            }
        }
        if let Some(capture_id) = capture_id.as_deref() {
            observer.finish_capture(capture_id);
        }
        let _ = app_clone.emit(
            "pty_status",
            PtyStatusPayload {
                tab_id,
                cli_kind,
                alive: false,
            },
        );
    });

    Ok(tab_id)
}

#[tauri::command]
pub fn pty_set_session_id(
    tab_id: u32,
    session_id: String,
    state: State<'_, Mutex<PtyManager>>,
) -> Result<(), String> {
    let mut mgr = state.lock().map_err(|error| error.to_string())?;
    let session = mgr
        .sessions
        .get_mut(&tab_id)
        .ok_or_else(|| format!("PTY tab {tab_id} not found"))?;
    session.session_id = Some(session_id);
    Ok(())
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
            mgr.line_buffers.insert(tab_id, line_buffer);
            let session = mgr
                .sessions
                .get_mut(&tab_id)
                .ok_or_else(|| format!("PTY tab {tab_id} not found"))?;
            session
                .writer
                .write_all(&bytes_to_send)
                .map_err(|error| format!("PTY tab {tab_id} write failed: {error}"))?;
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
            let line_clean = line_str
                .trim_matches(|c: char| c == '\r' || c == '\n')
                .to_string();

            if line_clean.starts_with("tab-") {
                if let Some(cmd) = crate::tab_cli::parse_tab_command(&line_clean) {
                    match crate::tab_cli::execute_command(&cmd, tab_id, &mut mgr) {
                        Ok(result) => {
                            // Write immediate output to caller's PTY (if any, None for --wait mode with deferred reply)
                            if let Some(output) = result.immediate_output {
                                let session = mgr
                                    .sessions
                                    .get_mut(&tab_id)
                                    .ok_or_else(|| format!("PTY tab {tab_id} not found"))?;
                                session
                                    .writer
                                    .write_all(output.as_bytes())
                                    .map_err(|error| {
                                        format!("PTY tab {tab_id} write failed: {error}")
                                    })?;
                            }
                            // Apply pending actions (add/remove pending replies) -- critical for --wait mode
                            for action in result.pending_actions {
                                match action {
                                    crate::tab_cli::PendingAction::AddReply {
                                        caller_tab,
                                        entry,
                                    } => {
                                        mgr.pending_replies
                                            .entry(caller_tab)
                                            .or_default()
                                            .push(entry);
                                    }
                                    crate::tab_cli::PendingAction::RemoveReply {
                                        caller_tab,
                                        index,
                                    } => {
                                        if let Some(list) = mgr.pending_replies.get_mut(&caller_tab)
                                        {
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
                            let session = mgr
                                .sessions
                                .get_mut(&tab_id)
                                .ok_or_else(|| format!("PTY tab {tab_id} not found"))?;
                            let err_msg = crate::tab_cli::format_message(&format!("Error: {}", e));
                            session
                                .writer
                                .write_all(err_msg.as_bytes())
                                .map_err(|error| {
                                    format!("PTY tab {tab_id} write failed: {error}")
                                })?;
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

        // Restore the (possibly partially consumed) line buffer before writing,
        // so a failed PTY write does not also lose parser state.
        mgr.line_buffers.insert(tab_id, line_buffer);

        // Write all passthrough bytes to PTY
        for bytes in passthrough_bytes {
            let session = mgr
                .sessions
                .get_mut(&tab_id)
                .ok_or_else(|| format!("PTY tab {tab_id} not found"))?;
            session
                .writer
                .write_all(&bytes)
                .map_err(|error| format!("PTY tab {tab_id} write failed: {error}"))?;
        }
    }

    Ok(())
}

#[tauri::command]
pub fn pty_resize(
    tab_id: u32,
    cols: u16,
    rows: u16,
    state: State<'_, Mutex<PtyManager>>,
    observer: State<'_, Arc<crate::claude_observer::ClaudeObserverManager>>,
) -> Result<(), String> {
    let mgr = state.lock().map_err(|e| e.to_string())?;
    let session = mgr
        .sessions
        .get(&tab_id)
        .ok_or_else(|| format!("no session for tab_id {tab_id}"))?;
    session
        .master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("resize failed: {e}"))?;
    if let Some(capture_id) = session.capture_id.as_deref() {
        observer.resize(capture_id, cols, rows);
    }
    Ok(())
}

#[cfg(windows)]
fn kill_process_tree(pid: u32) {
    let mut cmd = std::process::Command::new("taskkill");
    cmd.args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let _ = cmd.output();
}

#[cfg(unix)]
fn signal_process_group(process_group_id: i32, signal: i32) -> std::io::Result<()> {
    let result = unsafe { libc::kill(-process_group_id, signal) };
    if result == 0 {
        return Ok(());
    }

    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        // The process group has already exited, which is the desired state.
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(unix)]
fn process_group_exists(process_group_id: i32) -> bool {
    let result = unsafe { libc::kill(-process_group_id, 0) };
    if result == 0 {
        return true;
    }

    // EPERM means the group exists but cannot be signalled by this process.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
fn kill_process_tree(pid: u32) {
    let Ok(process_group_id) = i32::try_from(pid) else {
        return;
    };

    // portable-pty calls setsid() before exec on Unix, making the direct child
    // the leader of an isolated process group. Signal that whole group so CLI
    // helpers such as Node processes and MCP servers do not outlive the tab.
    if signal_process_group(process_group_id, libc::SIGTERM).is_err() {
        return;
    }

    for _ in 0..10 {
        if !process_group_exists(process_group_id) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    let _ = signal_process_group(process_group_id, libc::SIGKILL);
}

#[tauri::command]
pub fn pty_kill(
    tab_id: u32,
    state: State<'_, Mutex<PtyManager>>,
    observer: State<'_, Arc<crate::claude_observer::ClaudeObserverManager>>,
) -> Result<(), String> {
    let session = {
        let mut mgr = state.lock().map_err(|e| e.to_string())?;
        take_session(&mut mgr, tab_id)
    };
    if let Some(session) = session {
        let capture_id = session.capture_id.clone();
        if let Some(capture_id) = capture_id.as_deref() {
            observer.finish_capture(capture_id);
        }
        terminate_session(session);
        if let Some(capture_id) = capture_id.as_deref() {
            observer.release_capture(capture_id, tab_id);
        }
    }
    Ok(())
}

fn take_session(mgr: &mut PtyManager, tab_id: u32) -> Option<PtySession> {
    mgr.line_buffers.remove(&tab_id);
    mgr.sessions.remove(&tab_id)
}

fn terminate_session(mut session: PtySession) {
    // Drop the writer first to send EOF to the slave side.
    drop(session.writer);

    // Cascade before killing the direct child: once the parent exits, detached
    // descendants can be re-parented and become harder to identify on Windows.
    if let Some(pid) = session.child_pid {
        kill_process_tree(pid);
    }

    // Keep a direct-child fallback and reap it even if process-tree cleanup was
    // unavailable or only partially successful.
    let _ = session.child.kill();
    let _ = session.child.wait();
}

pub fn cleanup_all_sessions(app: &tauri::AppHandle) {
    let state = app.state::<Mutex<PtyManager>>();
    let observer = app.state::<Arc<crate::claude_observer::ClaudeObserverManager>>();
    let Ok(mgr) = state.lock() else { return };
    let tab_ids: Vec<u32> = mgr.sessions.keys().copied().collect();
    drop(mgr);
    for tab_id in tab_ids {
        let session = {
            let Ok(mut mgr) = state.lock() else { return };
            take_session(&mut mgr, tab_id)
        };
        if let Some(session) = session {
            let capture_id = session.capture_id.clone();
            if let Some(capture_id) = capture_id.as_deref() {
                observer.finish_capture(capture_id);
            }
            terminate_session(session);
            if let Some(capture_id) = capture_id.as_deref() {
                observer.release_capture(capture_id, tab_id);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod process_cleanup_tests {
    use super::*;

    #[test]
    fn unix_cleanup_terminates_the_full_pty_process_group() {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open test PTY");

        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        // Both the shell and its background child ignore graceful signals so
        // the test also exercises the process-group SIGKILL fallback.
        command.arg("trap '' HUP TERM; sleep 30 & wait");
        let mut child = pair
            .slave
            .spawn_command(command)
            .expect("spawn isolated PTY process group");
        let pid = child.process_id().expect("test child PID");
        let process_group_id = i32::try_from(pid).expect("PID fits in i32");

        assert_ne!(process_group_id, unsafe { libc::getpgrp() });
        assert!(process_group_exists(process_group_id));

        kill_process_tree(pid);
        let _ = child.kill();
        let _ = child.wait();

        let mut cleaned = false;
        for _ in 0..20 {
            if !process_group_exists(process_group_id) {
                cleaned = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }

        // Ensure a failing assertion can never leave the test process running.
        if !cleaned {
            let _ = signal_process_group(process_group_id, libc::SIGKILL);
        }

        assert!(cleaned, "PTY process group survived cleanup");
    }
}
