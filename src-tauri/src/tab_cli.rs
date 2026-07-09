//! Cross-tab agent communication module.
//!
//! Provides command parsing, permission checking, message routing,
//! and snapshot persistence for inter-tab CLI communication via `tab-*` prefix commands.

use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::State;

// ── Re-exports for convenience by callers ──────────────────────────────────────

pub use super::pty::PtyManager;
pub use super::pty::PtySession;

// ── Data Structures ───────────────────────────────────────────────────────────

/// Parsed `tab-*` command variants.
#[derive(Debug, Clone)]
pub enum TabCommand {
    Send { to: u32, message: String, wait_seconds: Option<u64> },
    List,
    Read { from: u32, lines: usize },
    Presence { to: u32 },
}

/// Per-tab permission configuration. Stored inline in PtySession.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TabPermission {
    pub enabled: bool,
    pub allowed_targets: Vec<u32>,
}

/// Pending reply for `--wait` mechanism.
#[derive(Debug, Clone)]
pub struct PendingReply {
    pub msg_id: String,
    pub deadline: std::time::Instant,
    pub caller_tab: u32,
    pub responder_tab: u32,
    pub original_message: String,
}

/// Result of executing a tab command. Contains immediate output and deferred actions.
#[derive(Debug, Default)]
pub struct CommandResult {
    /// Output to write to the caller's PTY immediately.
    pub immediate_output: Option<String>,
    /// Actions to apply to pending_replies after execute_command returns
    /// (add or remove entries). Applied by PtyManager.apply_pending_actions.
    pub pending_actions: Vec<PendingAction>,
}

/// Action to modify the pending_replies map after command execution.
#[derive(Debug, Clone)]
pub enum PendingAction {
    /// Remove a pending reply entry at the given index for the given caller tab.
    RemoveReply { caller_tab: u32, index: usize },
    /// Add a new pending reply entry.
    AddReply { caller_tab: u32, entry: PendingReply },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentRole {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct CanvasSnapshot {
    pub items: Vec<CanvasItemSnapshot>,
    pub connections: Vec<CanvasConnection>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CanvasItemSnapshot {
    pub tab_id: u32,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CanvasConnection {
    pub from: u32,
    pub to: u32,
}

/// Full snapshot of all terminal tabs for a project.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TerminalSnapshot {
    pub project_path: String,
    pub timestamp: String,
    pub tabs: Vec<SnapshotTabEntry>,
    pub canvas: CanvasSnapshot,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SnapshotTabEntry {
    pub tab_id: u32,
    pub title: String,
    pub session_id: Option<String>,
    pub permission: TabPermission,
    pub role: Option<AgentRole>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SnapshotEntry {
    pub id: String,
    pub project_path: String,
    pub timestamp: String,
}

// ── Command Parsing ───────────────────────────────────────────────────────────

/// Parse a line that starts with `tab-`. Returns `None` if not a tab command.
pub fn parse_tab_command(line: &str) -> Option<TabCommand> {
    let trimmed = line.trim();
    if trimmed.starts_with("tab-send") { return parse_tab_send(trimmed); }
    if trimmed == "tab-list" { return Some(TabCommand::List); }
    if trimmed.starts_with("tab-read") { return parse_tab_read(trimmed); }
    if trimmed.starts_with("tab-presence") { return parse_tab_presence(trimmed); }
    None
}

fn parse_tab_send(line: &str) -> Option<TabCommand> {
    let rest = line.strip_prefix("tab-send")?.trim();
    if rest.is_empty() { return None; }
    let to: u32 = extract_flag_u32(rest, "--to")?;
    let (message, _) = extract_quoted_message(rest)?;
    let wait_seconds = extract_flag_u64(rest, "--wait");
    Some(TabCommand::Send { to, message, wait_seconds })
}

fn extract_quoted_message(s: &str) -> Option<(String, String)> {
    let first = s.find('"')?;
    let last = s.rfind('"')?;
    if last <= first { return None; }
    Some((s[first + 1..last].to_string(), s[last + 1..].to_string()))
}

fn parse_tab_read(line: &str) -> Option<TabCommand> {
    let rest = line.strip_prefix("tab-read")?.trim();
    if rest.is_empty() { return None; }
    let from = extract_flag_u32(rest, "--from")?;
    let lines = extract_flag_usize(rest, "--lines")?;
    Some(TabCommand::Read { from, lines })
}

fn parse_tab_presence(line: &str) -> Option<TabCommand> {
    let rest = line.strip_prefix("tab-presence")?.trim();
    if rest.is_empty() { return None; }
    let to = extract_flag_u32(rest, "--to")?;
    Some(TabCommand::Presence { to })
}

fn extract_flag_u32(s: &str, flag: &str) -> Option<u32> {
    let idx = s.find(flag)?;
    s[idx + flag.len()..].trim().split_whitespace().next()?.parse().ok()
}

fn extract_flag_u64(s: &str, flag: &str) -> Option<u64> {
    let idx = s.find(flag)?;
    s[idx + flag.len()..].trim().split_whitespace().next()?.parse().ok()
}

fn extract_flag_usize(s: &str, flag: &str) -> Option<usize> {
    let idx = s.find(flag)?;
    s[idx + flag.len()..].trim().split_whitespace().next()?.parse().ok()
}

// ── Permission Checking ───────────────────────────────────────────────────────

/// Check the caller's inline permission (stored in PtySession) for an operation.
fn check_permission_inline(session: &PtySession, target_id: Option<u32>) -> Result<(), String> {
    if !session.permission.enabled {
        return Err("Permission denied".to_string());
    }
    if !session.permission.allowed_targets.is_empty() {
        if let Some(tid) = target_id {
            if !session.permission.allowed_targets.contains(&tid) {
                return Err("Target not in allowed list".to_string());
            }
        }
    }
    Ok(())
}

/// Legacy: check permission using a HashMap (for tests and external callers).
pub fn check_permission(
    permissions: &HashMap<u32, TabPermission>,
    caller_id: u32,
    target_id: Option<u32>,
) -> Result<(), String> {
    let perm = permissions.get(&caller_id).ok_or_else(|| "Permission denied".to_string())?;
    if !perm.enabled { return Err("Permission denied".to_string()); }
    if !perm.allowed_targets.is_empty() {
        if let Some(tid) = target_id {
            if !perm.allowed_targets.contains(&tid) {
                return Err("Target not in allowed list".to_string());
            }
        }
    }
    Ok(())
}

// ── Output Formatting ────────────────────────────────────────────────────────

pub fn format_result(cmd: &TabCommand, result: &str) -> String {
    let cmd_name = match cmd {
        TabCommand::Send { .. } => "send",
        TabCommand::List => "list",
        TabCommand::Read { .. } => "read",
        TabCommand::Presence { .. } => "presence",
    };
    format!("[TAB-CMD] {}: {}\r\n", cmd_name, result)
}

pub fn format_message(msg: &str) -> String {
    format!("[TAB-CMD] {}\r\n", msg)
}

// ── Command Execution ────────────────────────────────────────────────────────

/// Execute a tab command.
///
/// Takes `&mut PtyManager` to avoid cross-field borrow issues (sessions and
/// pending_replies are separate HashMap fields that need sequential, not simultaneous, access).
///
/// Returns a CommandResult with:
/// - `immediate_output`: text to write to caller's PTY (or None for --wait mode)
/// - `pending_actions`: modifications to pending_replies to apply after this returns
pub fn execute_command(
    cmd: &TabCommand,
    caller_id: u32,
    mgr: &mut PtyManager,
) -> Result<CommandResult, String> {
    match cmd {
        TabCommand::Send { to, message, wait_seconds } => {
            // Permission check
            {
                let caller_session = mgr.sessions.get_mut(&caller_id)
                    .ok_or_else(|| format!("Caller tab {} not found", caller_id))?;
                check_permission_inline(caller_session, Some(*to))?;
            }

            // Check target exists and is alive
            let target_alive = {
                let target = mgr.sessions.get_mut(to)
                    .ok_or_else(|| format!("Target tab {} not found", to))?;
                if target.child.try_wait().map_err(|e| format!("try_wait failed: {}", e))?.is_some() {
                    return Err(format!("Target tab {} is not alive", to));
                }
                true
            };
            let _ = target_alive;

            // Write message to target PTY
            {
                let target = mgr.sessions.get_mut(to)
                    .ok_or_else(|| format!("Target tab {} not found", to))?;
                use std::io::Write;
                let payload = format!("{}\r\n", message);
                target.writer.write_all(payload.as_bytes())
                    .map_err(|e| format!("Write to target PTY failed: {}", e))?;
            }

            // Check if target has pending replies waiting for response from this caller
            check_and_fire_pending_replies(caller_id, *to, message, mgr);

            if let Some(wait_secs) = wait_seconds {
                // --wait mode: register pending reply
                let msg_id = uuid::Uuid::new_v4().to_string();
                let mut result = CommandResult::default();
                result.pending_actions.push(PendingAction::AddReply {
                    caller_tab: caller_id,
                    entry: PendingReply {
                        msg_id,
                        deadline: std::time::Instant::now() + std::time::Duration::from_secs(*wait_secs),
                        caller_tab: caller_id,
                        responder_tab: *to,
                        original_message: message.clone(),
                    },
                });
                result.immediate_output = Some(format_message(&format!(
                    "send: Message sent to tab {} (waiting for reply, timeout: {}s)", to, wait_secs
                )));
                Ok(result)
            } else {
                // Immediate mode
                Ok(CommandResult {
                    immediate_output: Some(format_result(
                        cmd, &format!("Message sent to tab {} ({} bytes)", to, message.len()),
                    )),
                    pending_actions: Vec::new(),
                })
            }
        }

        TabCommand::List => {
            {
                let caller_session = mgr.sessions.get_mut(&caller_id)
                    .ok_or_else(|| format!("Caller tab {} not found", caller_id))?;
                check_permission_inline(caller_session, None)?;
            }

            let mut lines = Vec::new();
            let tab_ids: Vec<u32> = mgr.sessions.keys().copied().collect();
            for tid in tab_ids {
                if let Some(session) = mgr.sessions.get_mut(&tid) {
                    let alive = session.child.try_wait().map_or(false, |r| r.is_none());
                    let title = session.title.lock().map_or_else(
                        |_| "Terminal".to_string(),
                        |t| if t.is_empty() { "Terminal".to_string() } else { t.clone() },
                    );
                    lines.push(format!(
                        "Tab {} | {} | {}",
                        tid, title, if alive { "alive" } else { "dead" },
                    ));
                }
            }

            if lines.is_empty() {
                return Ok(CommandResult {
                    immediate_output: Some(format_result(&TabCommand::List, "No active sessions")),
                    ..Default::default()
                });
            }
            Ok(CommandResult {
                immediate_output: Some(format_result(&TabCommand::List, &lines.join("\r\n"))),
                ..Default::default()
            })
        }

        TabCommand::Read { from, lines: n } => {
            {
                let caller_session = mgr.sessions.get_mut(&caller_id)
                    .ok_or_else(|| format!("Caller tab {} not found", caller_id))?;
                check_permission_inline(caller_session, Some(*from))?;
            }

            let output = {
                let target = mgr.sessions.get(from)
                    .ok_or_else(|| format!("Target tab {} not found", from))?;
                let deque = target.output_lines.lock().map_err(|e| e.to_string())?;
                let captured: Vec<String> = deque.iter().rev().take(*n).cloned().collect();
                let mut ordered = captured;
                ordered.reverse();
                if ordered.is_empty() {
                    format_result(cmd, &format!("No recent output from tab {}", from))
                } else {
                    format_result(cmd, &ordered.join("\r\n"))
                }
            };

            Ok(CommandResult {
                immediate_output: Some(output),
                ..Default::default()
            })
        }

        TabCommand::Presence { to } => {
            {
                let caller_session = mgr.sessions.get_mut(&caller_id)
                    .ok_or_else(|| format!("Caller tab {} not found", caller_id))?;
                check_permission_inline(caller_session, None)?;
            }

            let output = match mgr.sessions.get_mut(to) {
                Some(session) => {
                    let alive = session.child.try_wait().map_or(false, |r| r.is_none());
                    if alive {
                        format_result(cmd, &format!("Tab {} is alive", to))
                    } else {
                        format_result(cmd, &format!("Tab {} exists but is not alive", to))
                    }
                }
                None => format_result(cmd, &format!("Tab {} does not exist", to)),
            };

            Ok(CommandResult {
                immediate_output: Some(output),
                ..Default::default()
            })
        }
    }
}

/// When a `tab-send` is processed, check if the target has any pending replies
/// waiting for a response from the current sender. If found, write reply notification
/// to the original caller's PTY and return actions to remove matched entries.
fn check_and_fire_pending_replies(
    caller_id: u32,
    target_id: u32,
    message: &str,
    mgr: &mut PtyManager,
) {
    if let Some(waiting_list) = mgr.pending_replies.get_mut(&target_id) {
        let mut i = waiting_list.len();
        while i > 0 {
            i -= 1;
            let entry = &waiting_list[i];
            if entry.caller_tab == target_id && entry.responder_tab == caller_id {
                // Write reply notification to the original caller's PTY
                if let Some(session) = mgr.sessions.get_mut(&target_id) {
                    use std::io::Write;
                    let notification = format_message(&format!(
                        "send: Reply received from tab {}: {}", caller_id, message
                    ));
                    let _ = session.writer.write_all(notification.as_bytes());
                }
                waiting_list.remove(i);
            }
        }
        if waiting_list.is_empty() {
            mgr.pending_replies.remove(&target_id);
        }
    }
}

// ── Tauri Commands: Permission Management ─────────────────────────────────────

#[tauri::command]
pub fn set_tab_permission(
    tab_id: u32,
    enabled: bool,
    allowed_targets: Vec<u32>,
    state: State<'_, StdMutex<PtyManager>>,
) -> Result<(), String> {
    let mut mgr = state.lock().map_err(|e| e.to_string())?;
    if let Some(session) = mgr.sessions.get_mut(&tab_id) {
        session.permission = TabPermission { enabled, allowed_targets };
        Ok(())
    } else {
        Err(format!("Tab {} does not exist", tab_id))
    }
}

#[tauri::command]
pub fn get_tab_permission(
    tab_id: u32,
    state: State<'_, StdMutex<PtyManager>>,
) -> Result<TabPermission, String> {
    let mgr = state.lock().map_err(|e| e.to_string())?;
    if let Some(session) = mgr.sessions.get(&tab_id) {
        Ok(session.permission.clone())
    } else {
        Err(format!("No permission config for tab {}", tab_id))
    }
}

// ── Snapshot Persistence ─────────────────────────────────────────────────────

fn snapshot_dir() -> Result<std::path::PathBuf, String> {
    let base = dirs::config_dir()
        .ok_or_else(|| "Could not determine config directory".to_string())?
        .join("ClaudeEnvManager")
        .join("terminal_snapshots");
    std::fs::create_dir_all(&base).map_err(|e| format!("Failed to create snapshot dir: {}", e))?;
    Ok(base)
}

fn path_to_id(path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.as_bytes());
    let result = hasher.finalize();
    bytes_to_hex(&result[..8])
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[tauri::command]
pub fn save_terminal_snapshot(
    project_path: String,
    canvas: Option<CanvasSnapshot>,
    roles: Option<HashMap<u32, AgentRole>>,
    state: State<'_, StdMutex<PtyManager>>,
) -> Result<(), String> {
    let mgr = state.lock().map_err(|e| e.to_string())?;
    let snapshot = TerminalSnapshot {
        project_path: project_path.clone(),
        timestamp: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        tabs: mgr.sessions.iter().map(|(&tab_id, session)| {
            let title = session.title.lock().map_or_else(|_| format!("Tab {}", tab_id), |t| {
                if t.is_empty() { format!("Tab {}", tab_id) } else { t.clone() }
            });
            SnapshotTabEntry {
                tab_id,
                title,
                session_id: session.session_id.clone(),
                permission: session.permission.clone(),
                role: roles.as_ref().and_then(|r| r.get(&tab_id)).cloned(),
            }
        }).collect(),
        canvas: canvas.unwrap_or_default(),
    };

    let snapshot_path = snapshot_dir()?;
    let file_name = format!("{}.json", path_to_id(&project_path));
    let full_path = snapshot_path.join(&file_name);
    let json = serde_json::to_string_pretty(&snapshot)
        .map_err(|e| format!("Failed to serialize snapshot: {}", e))?;
    std::fs::write(&full_path, json)
        .map_err(|e| format!("Failed to write snapshot file {}: {}", full_path.display(), e))?;
    Ok(())
}

#[tauri::command]
pub fn load_terminal_snapshot(
    project_path: String,
) -> Result<Option<TerminalSnapshot>, String> {
    let snapshot_path = snapshot_dir()?;
    let file_name = format!("{}.json", path_to_id(&project_path));
    let full_path = snapshot_path.join(&file_name);
    if !full_path.exists() { return Ok(None); }
    let content = std::fs::read_to_string(&full_path)
        .map_err(|e| format!("Failed to read snapshot file {}: {}", full_path.display(), e))?;
    let snapshot: TerminalSnapshot = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse snapshot JSON: {}", e))?;
    Ok(Some(snapshot))
}

#[tauri::command]
pub fn list_terminal_snapshots() -> Result<Vec<SnapshotEntry>, String> {
    let snapshot_path = snapshot_dir()?;
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&snapshot_path)
        .map_err(|e| format!("Failed to read snapshot directory: {}", e))?
    {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") { continue; }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read file {}: {}", path.display(), e))?;
        if let Ok(snapshot) = serde_json::from_str::<TerminalSnapshot>(&content) {
            entries.push(SnapshotEntry {
                id: path_to_id(&snapshot.project_path),
                project_path: snapshot.project_path,
                timestamp: snapshot.timestamp,
            });
        }
    }
    entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(entries)
}

// ── Orchestration Presets ─────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OrchestrationPreset {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub agents: Vec<VirtualAgent>,
    pub connections: Vec<VirtualConnection>,
    pub layout: HashMap<String, Position>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VirtualAgent {
    pub id: String,
    pub name: String,
    pub role: AgentRole,
    pub launch_config: LaunchConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LaunchConfig {
    pub agent_type: String,  // "claude" | "terminal"
    pub cmd: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VirtualConnection {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PresetEntry {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn preset_dir() -> Result<std::path::PathBuf, String> {
    let base = dirs::config_dir()
        .ok_or_else(|| "Could not determine config directory".to_string())?
        .join("ClaudeEnvManager")
        .join("orchestration_presets");
    std::fs::create_dir_all(&base).map_err(|e| format!("Failed to create preset dir: {}", e))?;
    Ok(base)
}

#[tauri::command]
pub fn list_presets() -> Result<Vec<PresetEntry>, String> {
    let dir = preset_dir()?;
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .map_err(|e| format!("Failed to read preset directory: {}", e))?
    {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") { continue; }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read file {}: {}", path.display(), e))?;
        if let Ok(preset) = serde_json::from_str::<OrchestrationPreset>(&content) {
            entries.push(PresetEntry {
                id: preset.id,
                name: preset.name,
                description: preset.description,
                created_at: preset.created_at,
                updated_at: preset.updated_at,
            });
        }
    }
    entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(entries)
}

#[tauri::command]
pub fn save_preset(preset: OrchestrationPreset) -> Result<(), String> {
    let dir = preset_dir()?;
    let file_name = format!("{}.json", preset.id);
    let full_path = dir.join(&file_name);
    let json = serde_json::to_string_pretty(&preset)
        .map_err(|e| format!("Failed to serialize preset: {}", e))?;
    std::fs::write(&full_path, json)
        .map_err(|e| format!("Failed to write preset file {}: {}", full_path.display(), e))?;
    Ok(())
}

#[tauri::command]
pub fn delete_preset(id: String) -> Result<(), String> {
    let dir = preset_dir()?;
    let file_name = format!("{}.json", id);
    let full_path = dir.join(&file_name);
    if full_path.exists() {
        std::fs::remove_file(&full_path)
            .map_err(|e| format!("Failed to delete preset file {}: {}", full_path.display(), e))?;
    }
    Ok(())
}

#[tauri::command]
pub fn load_preset(id: String) -> Result<Option<OrchestrationPreset>, String> {
    let dir = preset_dir()?;
    let file_name = format!("{}.json", id);
    let full_path = dir.join(&file_name);
    if !full_path.exists() { return Ok(None); }
    let content = std::fs::read_to_string(&full_path)
        .map_err(|e| format!("Failed to read preset file {}: {}", full_path.display(), e))?;
    let preset: OrchestrationPreset = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse preset JSON: {}", e))?;
    Ok(Some(preset))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tab_send() {
        let cmd = parse_tab_command(r#"tab-send --to 2 "hello world""#).unwrap();
        match cmd {
            TabCommand::Send { to, message, wait_seconds } => {
                assert_eq!(to, 2);
                assert_eq!(message, "hello world");
                assert_eq!(wait_seconds, None);
            }
            _ => panic!("Expected Send"),
        }
    }

    #[test]
    fn test_parse_tab_send_with_wait() {
        let cmd = parse_tab_command(r#"tab-send --to 3 "run tests" --wait 30"#).unwrap();
        match cmd {
            TabCommand::Send { to, message, wait_seconds } => {
                assert_eq!(to, 3);
                assert_eq!(message, "run tests");
                assert_eq!(wait_seconds, Some(30));
            }
            _ => panic!("Expected Send"),
        }
    }

    #[test]
    fn test_parse_tab_list() {
        assert!(matches!(parse_tab_command("tab-list").unwrap(), TabCommand::List));
    }

    #[test]
    fn test_parse_tab_read() {
        let cmd = parse_tab_command("tab-read --from 1 --lines 50").unwrap();
        match cmd {
            TabCommand::Read { from, lines } => { assert_eq!(from, 1); assert_eq!(lines, 50); }
            _ => panic!("Expected Read"),
        }
    }

    #[test]
    fn test_parse_tab_presence() {
        let cmd = parse_tab_command("tab-presence --to 4").unwrap();
        match cmd { TabCommand::Presence { to } => assert_eq!(to, 4), _ => panic!("Expected Presence") }
    }

    #[test]
    fn test_non_tab_command_returns_none() {
        assert!(parse_tab_command("echo hello").is_none());
        assert!(parse_tab_command("dir /s").is_none());
        assert!(parse_tab_command("").is_none());
    }

    #[test]
    fn test_permission_denied_when_disabled() {
        let mut perms = HashMap::new();
        perms.insert(1, TabPermission { enabled: false, allowed_targets: vec![2, 3] });
        assert!(check_permission(&perms, 1, Some(2)).is_err());
    }

    #[test]
    fn test_permission_denied_when_unknown_caller() {
        let perms: HashMap<u32, TabPermission> = HashMap::new();
        assert!(check_permission(&perms, 99, None).is_err());
    }

    #[test]
    fn test_permission_target_not_allowed() {
        let mut perms = HashMap::new();
        perms.insert(1, TabPermission { enabled: true, allowed_targets: vec![2] });
        assert!(check_permission(&perms, 1, Some(3)).is_err());
        assert!(check_permission(&perms, 1, Some(2)).is_ok());
    }

    #[test]
    fn test_permission_empty_allowed_targets_allows_any() {
        let mut perms = HashMap::new();
        perms.insert(1, TabPermission { enabled: true, allowed_targets: vec![] });
        assert!(check_permission(&perms, 1, Some(99)).is_ok());
    }

    #[test]
    fn test_format_result() {
        assert_eq!(format_result(&TabCommand::List, "3 tabs"), "[TAB-CMD] list: 3 tabs\r\n");
    }

    #[test]
    fn test_path_to_id_deterministic() {
        assert_eq!(path_to_id("/a"), path_to_id("/a"));
    }

    #[test]
    fn test_path_to_id_different() {
        assert_ne!(path_to_id("/a"), path_to_id("/b"));
    }

    #[test]
    fn test_parse_send_wait_before_to() {
        let cmd = parse_tab_command(r#"tab-send --wait 10 --to 5 "msg""#).unwrap();
        match cmd {
            TabCommand::Send { to, message, wait_seconds } => {
                assert_eq!(to, 5); assert_eq!(message, "msg"); assert_eq!(wait_seconds, Some(10));
            }
            _ => panic!("Expected Send"),
        }
    }

    #[test]
    fn test_parse_send_message_with_quotes_inside() {
        let cmd = parse_tab_command(r#"tab-send --to 1 "it's a \"test\"""#).unwrap();
        match cmd {
            TabCommand::Send { to, message, .. } => { assert_eq!(to, 1); assert!(message.contains("test")); }
            _ => panic!("Expected Send"),
        }
    }
}
