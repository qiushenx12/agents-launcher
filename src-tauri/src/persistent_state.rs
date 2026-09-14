//! persistent_state.rs
//!
//! Manages all UI state in a single file: {data_dir}/ClaudeEnvManager/app_state.json

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::cli_migration::normalize_main_tab;
use crate::file_transaction::{restore_json_backup_if_missing, write_json_atomic};

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn app_data_dir() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|d| d.join("ClaudeEnvManager"))
        .ok_or_else(|| "Could not determine application data directory".to_string())
}

fn app_state_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("app_state.json"))
}

// ---------------------------------------------------------------------------
// AppState structure
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq)]
pub struct WindowState {
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToolState {
    #[serde(default)]
    pub config_order: Vec<String>,
    #[serde(default)]
    pub launch_dir: String,
    #[serde(default)]
    pub use_builtin_terminal: bool,
    #[serde(default = "default_drop_path_mode")]
    pub project_drop_path_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_busy_input_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_startup_view: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_log_output_enabled: Option<bool>,
    #[serde(default = "default_pane_width")]
    pub pane_width: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_sizes: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_id: Option<String>,
    #[serde(default)]
    pub profile_ids: BTreeMap<String, String>,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileIndexState {
    pub order: Vec<String>,
    pub profile_ids: BTreeMap<String, String>,
    pub active_profile_id: Option<String>,
}

fn default_pane_width() -> f64 {
    280.0
}

fn default_font_size() -> f64 {
    10.0
}

fn default_drop_path_mode() -> String {
    "relative".to_string()
}

fn default_claude_busy_input_mode() -> String {
    "native".to_string()
}

fn normalize_claude_busy_input_mode(value: &str) -> String {
    match value {
        "after-stop" => "after-stop".to_string(),
        _ => default_claude_busy_input_mode(),
    }
}

fn default_claude_startup_view() -> String {
    "terminal".to_string()
}

fn normalize_claude_startup_view(value: &str) -> String {
    match value {
        "conversation" => "conversation".to_string(),
        _ => default_claude_startup_view(),
    }
}

impl Default for ToolState {
    fn default() -> Self {
        Self {
            config_order: Vec::new(),
            launch_dir: String::new(),
            use_builtin_terminal: false,
            project_drop_path_mode: default_drop_path_mode(),
            claude_busy_input_mode: None,
            claude_startup_view: None,
            claude_log_output_enabled: None,
            pane_width: default_pane_width(),
            pane_sizes: None,
            active_profile_id: None,
            profile_ids: BTreeMap::new(),
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TerminalState {
    #[serde(default = "default_font_size")]
    pub font_size: f64,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            font_size: default_font_size(),
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DshRuntimeConfig {
    /// `"local"` (loopback only) or `"remote"` (`0.0.0.0`, LAN reachable).
    #[serde(default = "default_dsh_access")]
    pub access: String,
    #[serde(default = "default_dsh_port")]
    pub port: u16,
    /// The dsh version future starts are pinned to (`npx @deepseek-ai/dsh@<v>`).
    ///
    /// Recorded by the backend after every successful start, so the next start
    /// reuses exactly the version that worked instead of re-resolving `latest`
    /// on every launch. `None` only before the first recorded start; the
    /// frontend never writes this field (see `save_dsh_runtime_config`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_version: Option<String>,
}

impl Default for DshRuntimeConfig {
    fn default() -> Self {
        Self {
            access: default_dsh_access(),
            port: default_dsh_port(),
            pinned_version: None,
        }
    }
}

impl DshRuntimeConfig {
    /// Normalize untrusted values coming from the frontend or an edited file.
    pub fn normalized(mut self) -> Self {
        if self.access != "remote" {
            self.access = "local".to_string();
        }
        if self.port == 0 {
            self.port = default_dsh_port();
        }
        self.pinned_version = normalize_dsh_pinned_version(self.pinned_version);
        self
    }
}

fn default_dsh_access() -> String {
    "local".to_string()
}

fn default_dsh_port() -> u16 {
    3080
}

/// A blank stored pin is no pin at all. Charset validation happens in
/// `dsh_runtime`, which owns everything version-sensitive (the value ends up
/// inside an npx package spec).
fn normalize_dsh_pinned_version(version: Option<String>) -> Option<String> {
    version
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Merge an incoming config with the stored one: the frontend save carries
/// only `access`/`port`, so a missing pin must preserve the stored one rather
/// than wiping the version the backend recorded.
fn merge_dsh_runtime_config(
    incoming: DshRuntimeConfig,
    stored: &DshRuntimeConfig,
) -> DshRuntimeConfig {
    if incoming.pinned_version.is_some() {
        incoming
    } else {
        DshRuntimeConfig {
            pinned_version: stored.pinned_version.clone(),
            ..incoming
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct AppState {
    #[serde(default)]
    pub window: WindowState,
    #[serde(default)]
    pub minimize_to_tray: bool,
    #[serde(default)]
    pub claude: ToolState,
    #[serde(default)]
    pub codex: ToolState,
    #[serde(default)]
    pub opencode: ToolState,
    #[serde(default)]
    pub dsh: ToolState,
    #[serde(default)]
    pub dsh_runtime: DshRuntimeConfig,
    #[serde(default)]
    pub terminal: TerminalState,
    #[serde(default = "default_last_active_main_tab")]
    pub last_active_main_tab: String,
    #[serde(default = "default_top_bar_order")]
    pub top_bar_order: Vec<String>,
    #[serde(default)]
    pub top_bar_hidden: Vec<String>,
    #[serde(default)]
    pub pane_widths: BTreeMap<String, f64>,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

fn default_last_active_main_tab() -> String {
    "config".to_string()
}

fn default_top_bar_order() -> Vec<String> {
    ["config", "claude", "codex", "opencode", "dsh"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn normalize_top_bar_order(order: &[String]) -> Vec<String> {
    let defaults = default_top_bar_order();
    let mut normalized = Vec::with_capacity(defaults.len());
    for item in order.iter().chain(defaults.iter()) {
        if defaults.contains(item) && !normalized.contains(item) {
            normalized.push(item.clone());
        }
    }
    normalized
}

fn normalize_top_bar_hidden(hidden: &[String]) -> Vec<String> {
    default_top_bar_order()
        .into_iter()
        .filter(|item| item != "config" && hidden.contains(item))
        .collect()
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TopBarLayout {
    pub order: Vec<String>,
    pub hidden: Vec<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StartupBootstrapState {
    pub claude_startup_view: String,
    pub claude_log_output_enabled: bool,
    pub claude_busy_input_mode: String,
    pub claude_launch_dir: String,
    pub claude_project_drop_path_mode: String,
    pub top_bar_layout: TopBarLayout,
    pub minimize_to_tray: bool,
    pub terminal_font_size: f64,
    pub window_state: WindowState,
    pub last_active_main_tab: String,
}

// ---------------------------------------------------------------------------
// Core read/write
// ---------------------------------------------------------------------------

fn load_state() -> Result<AppState, String> {
    let path = app_state_path()?;
    restore_json_backup_if_missing(&path, "应用状态")?;
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(migrate_legacy().unwrap_or_default());
        }
        Err(error) => return Err(format!("Failed to read state file: {error}")),
    };
    serde_json::from_str(&raw).map_err(|error| {
        format!("Failed to parse app_state.json; the file was not changed: {error}")
    })
}

fn save_state(state: &AppState) -> Result<(), String> {
    let path = app_state_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create state directory: {e}"))?;
    }
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| format!("Failed to serialise state: {e}"))?;
    write_json_atomic(&path, json.as_bytes(), "应用状态")
}

/// Read-modify-write helper
fn update_state<F: FnOnce(&mut AppState)>(f: F) -> Result<(), String> {
    let mut state = load_state()?;
    f(&mut state);
    save_state(&state)
}

fn validated_window_state(state: &AppState) -> WindowState {
    let window = state.window.clone();
    let invalid = window.width.is_some_and(|value| value <= 50.0)
        || window.height.is_some_and(|value| value <= 50.0)
        || window.x.is_some_and(|value| value < -10000.0)
        || window.y.is_some_and(|value| value < -10000.0);
    if invalid {
        WindowState::default()
    } else {
        window
    }
}

fn claude_busy_input_mode_from_state(state: &AppState) -> String {
    normalize_claude_busy_input_mode(
        state
            .claude
            .claude_busy_input_mode
            .as_deref()
            .unwrap_or("native"),
    )
}

fn claude_startup_view_from_state(state: &AppState) -> String {
    normalize_claude_startup_view(
        state
            .claude
            .claude_startup_view
            .as_deref()
            .unwrap_or("conversation"),
    )
}

fn top_bar_layout_from_state(state: &AppState) -> TopBarLayout {
    TopBarLayout {
        order: normalize_top_bar_order(&state.top_bar_order),
        hidden: normalize_top_bar_hidden(&state.top_bar_hidden),
    }
}

fn last_active_main_tab_from_state(state: &AppState) -> String {
    normalize_main_tab(&state.last_active_main_tab)
        .as_str()
        .to_string()
}

fn startup_bootstrap_from_state(state: &AppState) -> StartupBootstrapState {
    StartupBootstrapState {
        claude_startup_view: claude_startup_view_from_state(state),
        claude_log_output_enabled: state.claude.claude_log_output_enabled.unwrap_or(false),
        claude_busy_input_mode: claude_busy_input_mode_from_state(state),
        claude_launch_dir: state.claude.launch_dir.clone(),
        claude_project_drop_path_mode: state.claude.project_drop_path_mode.clone(),
        top_bar_layout: top_bar_layout_from_state(state),
        minimize_to_tray: state.minimize_to_tray,
        terminal_font_size: state.terminal.font_size,
        window_state: validated_window_state(state),
        last_active_main_tab: last_active_main_tab_from_state(state),
    }
}

fn pane_width_from_state(state: &AppState, key: &str) -> Result<f64, String> {
    if let Ok(tool) = tool_state_ref(state, key) {
        return Ok(tool.pane_width);
    }
    state
        .pane_widths
        .get(key)
        .copied()
        .ok_or_else(|| format!("No saved pane width for key: {key}"))
}

fn pane_widths_from_state(state: &AppState, keys: Vec<String>) -> BTreeMap<String, f64> {
    keys.into_iter()
        .filter_map(|key| {
            pane_width_from_state(state, &key)
                .ok()
                .map(|width| (key, width))
        })
        .collect()
}

// PLACEHOLDER_MIGRATION

// ---------------------------------------------------------------------------
// Legacy migration — reads old individual JSON files into AppState
// ---------------------------------------------------------------------------

fn legacy_path(filename: &str) -> Option<PathBuf> {
    app_data_dir().ok().map(|d| d.join(filename))
}

fn read_legacy<T: for<'de> Deserialize<'de>>(filename: &str) -> Option<T> {
    let path = legacy_path(filename)?;
    let raw = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

#[derive(Deserialize)]
struct LegacyWindowState {
    width: Option<f64>,
    height: Option<f64>,
    x: Option<f64>,
    y: Option<f64>,
}

#[derive(Deserialize)]
struct LegacyLaunchDir {
    dir: Option<String>,
}

#[derive(Deserialize)]
struct LegacyPaneWidth {
    width: Option<f64>,
}

#[derive(Deserialize)]
struct LegacyTerminalSettings {
    font_size: Option<f64>,
}

#[derive(Deserialize)]
struct LegacyConfigOrder {
    order: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct LegacyUseBuiltinTerminal {
    value: Option<bool>,
}

fn migrate_legacy() -> Option<AppState> {
    let dir = app_data_dir().ok()?;
    if !dir.exists() {
        return None;
    }

    let mut state = AppState::default();
    state.last_active_main_tab = default_last_active_main_tab();

    // Window
    if let Some(w) = read_legacy::<LegacyWindowState>("window_size.json") {
        state.window = WindowState {
            width: w.width,
            height: w.height,
            x: w.x,
            y: w.y,
            extra: Map::new(),
        };
    }

    // Claude
    if let Some(ld) = read_legacy::<LegacyLaunchDir>("launch_dir.json") {
        state.claude.launch_dir = ld.dir.unwrap_or_default();
    }
    if let Some(pw) = read_legacy::<LegacyPaneWidth>("pane_width_claude-panel.json") {
        state.claude.pane_width = pw.width.unwrap_or(default_pane_width());
    }
    if let Some(co) = read_legacy::<LegacyConfigOrder>("config_order_claude.json") {
        state.claude.config_order = co.order.unwrap_or_default();
    }
    if let Some(bt) = read_legacy::<LegacyUseBuiltinTerminal>("use_builtin_terminal_claude.json") {
        state.claude.use_builtin_terminal = bt.value.unwrap_or(false);
    }

    // Terminal
    if let Some(ts) = read_legacy::<LegacyTerminalSettings>("terminal_settings.json") {
        state.terminal.font_size = ts.font_size.unwrap_or(default_font_size());
    }

    // Write the migrated state
    let _ = save_state(&state);
    Some(state)
}

// ---------------------------------------------------------------------------
// Tauri commands — dsh runtime settings
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_dsh_runtime_config() -> Result<DshRuntimeConfig, String> {
    Ok(load_state()?.dsh_runtime.normalized())
}

#[tauri::command]
pub fn save_dsh_runtime_config(config: DshRuntimeConfig) -> Result<DshRuntimeConfig, String> {
    let incoming = config.normalized();
    let mut merged: Option<DshRuntimeConfig> = None;
    update_state(|state| {
        let next = merge_dsh_runtime_config(incoming, &state.dsh_runtime);
        state.dsh_runtime = next.clone();
        merged = Some(next);
    })?;
    merged.ok_or_else(|| "保存 dsh 运行设置失败：状态更新未执行。".to_string())
}

/// The version future dsh starts are pinned to. `None` before the first
/// recorded start. Read by `dsh_runtime` when it builds the npx invocation.
pub(crate) fn load_dsh_pinned_version() -> Option<String> {
    load_state()
        .ok()
        .and_then(|state| normalize_dsh_pinned_version(state.dsh_runtime.pinned_version))
}

/// Record the dsh version that just started successfully. A write failure is
/// reported to the caller but must never fail the launch itself.
pub(crate) fn save_dsh_pinned_version(version: &str) -> Result<(), String> {
    let normalized = normalize_dsh_pinned_version(Some(version.to_string()))
        .ok_or_else(|| "dsh 版本号为空，无法记录。".to_string())?;
    update_state(|state| state.dsh_runtime.pinned_version = Some(normalized))
}

// PLACEHOLDER_COMMANDS

// ---------------------------------------------------------------------------
// Key → tool field mapping
// ---------------------------------------------------------------------------

fn tool_state_mut<'a>(state: &'a mut AppState, key: &str) -> Result<&'a mut ToolState, String> {
    match key {
        "claude" | "claude-panel" => Ok(&mut state.claude),
        "codex" | "codex-panel" | "codex-config-panel" => Ok(&mut state.codex),
        "opencode" | "opencode-panel" | "opencode-config-panel" => Ok(&mut state.opencode),
        "dsh" | "dsh-panel" | "dsh-config-panel" => Ok(&mut state.dsh),
        _ => Err(format!("Unknown CLI state key: {key}")),
    }
}

fn tool_state_ref<'a>(state: &'a AppState, key: &str) -> Result<&'a ToolState, String> {
    match key {
        "claude" | "claude-panel" => Ok(&state.claude),
        "codex" | "codex-panel" | "codex-config-panel" => Ok(&state.codex),
        "opencode" | "opencode-panel" | "opencode-config-panel" => Ok(&state.opencode),
        "dsh" | "dsh-panel" | "dsh-config-panel" => Ok(&state.dsh),
        _ => Err(format!("Unknown CLI state key: {key}")),
    }
}

fn update_tool_state<F: FnOnce(&mut ToolState)>(key: &str, update: F) -> Result<(), String> {
    let mut state = load_state()?;
    update(tool_state_mut(&mut state, key)?);
    save_state(&state)
}

pub(crate) fn load_profile_index_state(key: &str) -> Result<ProfileIndexState, String> {
    let state = load_state()?;
    let tool = tool_state_ref(&state, key)?;
    Ok(ProfileIndexState {
        order: tool.config_order.clone(),
        profile_ids: tool.profile_ids.clone(),
        active_profile_id: tool.active_profile_id.clone(),
    })
}

pub(crate) fn save_profile_index_state(
    key: &str,
    profile_index: &ProfileIndexState,
) -> Result<(), String> {
    update_tool_state(key, |tool| {
        tool.config_order = profile_index.order.clone();
        tool.profile_ids = profile_index.profile_ids.clone();
        tool.active_profile_id = profile_index.active_profile_id.clone();
    })
}

// ---------------------------------------------------------------------------
// Tauri commands — Window
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_startup_bootstrap() -> Result<StartupBootstrapState, String> {
    Ok(startup_bootstrap_from_state(&load_state()?))
}

#[tauri::command]
pub fn load_window_state() -> Result<WindowState, String> {
    Ok(validated_window_state(&load_state()?))
}

#[tauri::command]
pub fn save_window_state(mut state: WindowState) -> Result<(), String> {
    // Don't persist zero-size or wildly off-screen positions.
    let invalid = state.width.map_or(false, |v| v <= 0.0)
        || state.height.map_or(false, |v| v <= 0.0)
        || state.x.map_or(false, |v| v < -10000.0)
        || state.y.map_or(false, |v| v < -10000.0);
    if invalid {
        return Ok(());
    }
    update_state(|current| {
        for (key, value) in std::mem::take(&mut current.window.extra) {
            state.extra.entry(key).or_insert(value);
        }
        current.window = state;
    })
}

#[tauri::command]
pub fn load_minimize_to_tray() -> Result<bool, String> {
    Ok(load_state()?.minimize_to_tray)
}

#[tauri::command]
pub fn save_minimize_to_tray(enabled: bool) -> Result<(), String> {
    update_state(|state| state.minimize_to_tray = enabled)
}

// ---------------------------------------------------------------------------
// Tauri commands — Launch directory
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_launch_dir(key: String) -> Result<String, String> {
    let state = load_state()?;
    Ok(tool_state_ref(&state, &key)?.launch_dir.clone())
}

#[tauri::command]
pub fn save_launch_dir(key: String, dir: String) -> Result<(), String> {
    update_tool_state(&key, |tool| tool.launch_dir = dir)
}

// ---------------------------------------------------------------------------
// Tauri commands — Pane width
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_pane_width(key: String) -> Result<f64, String> {
    let state = load_state()?;
    pane_width_from_state(&state, &key)
}

#[tauri::command]
pub fn load_pane_widths(keys: Vec<String>) -> Result<BTreeMap<String, f64>, String> {
    let state = load_state()?;
    Ok(pane_widths_from_state(&state, keys))
}

#[tauri::command]
pub fn save_pane_width(key: String, width: f64) -> Result<(), String> {
    let mut state = load_state()?;
    match tool_state_mut(&mut state, &key) {
        Ok(tool) => tool.pane_width = width,
        Err(_) => {
            state.pane_widths.insert(key, width);
        }
    }
    save_state(&state)
}

// ---------------------------------------------------------------------------
// Tauri commands — Terminal font size
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_terminal_font_size() -> Result<f64, String> {
    Ok(load_state()?.terminal.font_size)
}

#[tauri::command]
pub fn save_terminal_font_size(font_size: f64) -> Result<(), String> {
    update_state(|s| s.terminal.font_size = font_size)
}

// ---------------------------------------------------------------------------
// Tauri commands — Config order
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_config_order(key: String) -> Result<Vec<String>, String> {
    let state = load_state()?;
    Ok(tool_state_ref(&state, &key)?.config_order.clone())
}

#[tauri::command]
pub fn save_config_order(key: String, order: Vec<String>) -> Result<(), String> {
    update_tool_state(&key, |tool| tool.config_order = order)
}

// ---------------------------------------------------------------------------
// Tauri commands — Active profile selection
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_active_profile_id(key: String) -> Result<Option<String>, String> {
    let state = load_state()?;
    Ok(tool_state_ref(&state, &key)?.active_profile_id.clone())
}

#[tauri::command]
pub fn save_active_profile_id(key: String, profile_id: Option<String>) -> Result<(), String> {
    update_tool_state(&key, |tool| tool.active_profile_id = profile_id)
}

#[tauri::command]
pub fn load_profile_ids(key: String) -> Result<BTreeMap<String, String>, String> {
    let state = load_state()?;
    Ok(tool_state_ref(&state, &key)?.profile_ids.clone())
}

#[tauri::command]
pub fn save_profile_index(
    key: String,
    order: Vec<String>,
    profile_ids: BTreeMap<String, String>,
    active_profile_id: Option<String>,
) -> Result<(), String> {
    save_profile_index_state(
        &key,
        &ProfileIndexState {
            order,
            profile_ids,
            active_profile_id,
        },
    )
}

// ---------------------------------------------------------------------------
// Tauri commands — Use builtin terminal
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_use_builtin_terminal(key: String) -> Result<bool, String> {
    let state = load_state()?;
    Ok(tool_state_ref(&state, &key)?.use_builtin_terminal)
}

#[tauri::command]
pub fn save_use_builtin_terminal(key: String, value: bool) -> Result<(), String> {
    update_tool_state(&key, |tool| tool.use_builtin_terminal = value)
}

// ---------------------------------------------------------------------------
// Tauri commands — Project drop path mode
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_project_drop_path_mode(key: String) -> Result<String, String> {
    let state = load_state()?;
    Ok(tool_state_ref(&state, &key)?.project_drop_path_mode.clone())
}

#[tauri::command]
pub fn save_project_drop_path_mode(key: String, value: String) -> Result<(), String> {
    update_tool_state(&key, |tool| tool.project_drop_path_mode = value)
}

// ---------------------------------------------------------------------------
// Tauri commands — Claude busy input mode
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_claude_busy_input_mode() -> Result<String, String> {
    Ok(claude_busy_input_mode_from_state(&load_state()?))
}

#[tauri::command]
pub fn save_claude_busy_input_mode(mode: String) -> Result<(), String> {
    let normalized = normalize_claude_busy_input_mode(&mode);
    update_tool_state("claude", |tool| tool.claude_busy_input_mode = Some(normalized))
}

// ---------------------------------------------------------------------------
// Tauri commands — Claude startup view
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_claude_startup_view() -> Result<String, String> {
    Ok(claude_startup_view_from_state(&load_state()?))
}

#[tauri::command]
pub fn save_claude_startup_view(view: String) -> Result<(), String> {
    let normalized = normalize_claude_startup_view(&view);
    update_tool_state("claude", |tool| tool.claude_startup_view = Some(normalized))
}

// ---------------------------------------------------------------------------
// Tauri commands — Claude log output
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_claude_log_output_enabled() -> Result<bool, String> {
    Ok(load_state()?
        .claude
        .claude_log_output_enabled
        .unwrap_or(false))
}

pub fn save_claude_log_output_enabled(enabled: bool) -> Result<(), String> {
    update_tool_state("claude", |tool| {
        tool.claude_log_output_enabled = Some(enabled)
    })
}

// ---------------------------------------------------------------------------
// Tauri commands — last active main tab
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_last_active_main_tab() -> Result<String, String> {
    Ok(last_active_main_tab_from_state(&load_state()?))
}

#[tauri::command]
pub fn save_last_active_main_tab(tab: String) -> Result<(), String> {
    let normalized = normalize_main_tab(&tab).as_str().to_string();
    update_state(|s| s.last_active_main_tab = normalized)
}

// ---------------------------------------------------------------------------
// Tauri commands — top bar layout
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_top_bar_layout() -> Result<TopBarLayout, String> {
    Ok(top_bar_layout_from_state(&load_state()?))
}

#[tauri::command]
pub fn save_top_bar_layout(layout: TopBarLayout) -> Result<(), String> {
    let order = normalize_top_bar_order(&layout.order);
    let hidden = normalize_top_bar_hidden(&layout.hidden);
    update_state(|state| {
        state.top_bar_order = order;
        state.top_bar_hidden = hidden;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_profile_ids_remain_scoped_by_cli_kind() {
        let mut state = AppState::default();
        tool_state_mut(&mut state, "claude")
            .expect("claude state")
            .active_profile_id = Some("default".to_string());
        tool_state_mut(&mut state, "codex")
            .expect("codex state")
            .active_profile_id = Some("default".to_string());

        assert_eq!(
            tool_state_ref(&state, "claude")
                .expect("claude state")
                .active_profile_id
                .as_deref(),
            Some("default"),
        );
        assert_eq!(
            tool_state_ref(&state, "codex")
                .expect("codex state")
                .active_profile_id
                .as_deref(),
            Some("default"),
        );
        assert!(tool_state_ref(&state, "opencode")
            .expect("opencode state")
            .active_profile_id
            .is_none());

        tool_state_mut(&mut state, "claude")
            .expect("claude state")
            .profile_ids
            .insert("方案".to_string(), "default".to_string());
        assert!(tool_state_ref(&state, "codex")
            .expect("codex state")
            .profile_ids
            .is_empty());
    }

    #[test]
    fn unknown_state_fields_survive_a_serde_round_trip() {
        let input = serde_json::json!({
            "claude": { "futureToolField": "keep" },
            "futureRootField": { "enabled": true }
        });
        let state: AppState = serde_json::from_value(input).expect("decode state");
        let output = serde_json::to_value(state).expect("encode state");

        assert_eq!(output["claude"]["futureToolField"], "keep");
        assert_eq!(output["futureRootField"]["enabled"], Value::Bool(true));
    }

    #[test]
    fn minimize_to_tray_defaults_to_disabled_and_round_trips() {
        assert!(!AppState::default().minimize_to_tray);

        let state: AppState = serde_json::from_value(serde_json::json!({
            "minimize_to_tray": true
        }))
        .expect("decode minimize-to-tray setting");
        assert!(state.minimize_to_tray);

        let output = serde_json::to_value(state).expect("encode minimize-to-tray setting");
        assert_eq!(output["minimize_to_tray"], Value::Bool(true));
    }

    #[test]
    fn generic_pane_widths_round_trip_and_do_not_leak_into_cli_state() {
        let input = serde_json::json!({
            "pane_widths": { "project-left-sidebar": 260.0 }
        });
        let mut state: AppState = serde_json::from_value(input).expect("decode state");
        assert_eq!(state.pane_widths.get("project-left-sidebar"), Some(&260.0));
        assert!(tool_state_ref(&state, "project-left-sidebar").is_err());

        state
            .pane_widths
            .insert("project-right-sidebar".to_string(), 340.0);
        let output = serde_json::to_value(&state).expect("encode state");
        assert_eq!(output["pane_widths"]["project-right-sidebar"], 340.0);
        assert_eq!(
            tool_state_ref(&state, "claude")
                .expect("claude state")
                .pane_width,
            default_pane_width(),
        );
    }

    #[test]
    fn top_bar_order_deduplicates_filters_and_appends_missing_items() {
        let order = vec![
            "codex".to_string(),
            "unknown".to_string(),
            "config".to_string(),
            "codex".to_string(),
        ];
        assert_eq!(
            normalize_top_bar_order(&order),
            vec!["codex", "config", "claude", "opencode", "dsh"]
        );
    }

    #[test]
    fn top_bar_order_keeps_dsh_when_persisted() {
        let order = vec!["dsh".to_string(), "config".to_string()];
        assert_eq!(
            normalize_top_bar_order(&order),
            vec!["dsh", "config", "claude", "codex", "opencode"]
        );
        assert_eq!(
            normalize_top_bar_hidden(&["dsh".to_string()]),
            vec!["dsh"]
        );
    }

    #[test]
    fn dsh_tool_state_keys_are_known() {
        let mut state = AppState::default();
        assert!(tool_state_mut(&mut state, "dsh").is_ok());
        assert!(tool_state_mut(&mut state, "dsh-panel").is_ok());
        assert!(tool_state_mut(&mut state, "dsh-config-panel").is_ok());
        assert!(tool_state_ref(&state, "dsh").is_ok());
    }
    #[test]
    fn dsh_runtime_config_defaults_and_normalizes() {
        let state: AppState = serde_json::from_str("{}").expect("empty state should decode");
        assert_eq!(state.dsh_runtime.access, "local");
        assert_eq!(state.dsh_runtime.port, 3080);

        // AppState keeps snake_case keys, so the stored key is `dsh_runtime`.
        let decoded: AppState =
            serde_json::from_str(r#"{"dsh_runtime":{"access":"remote","port":3199}}"#)
                .expect("state should decode");
        assert_eq!(decoded.dsh_runtime.access, "remote");
        assert_eq!(decoded.dsh_runtime.port, 3199);

        let fallback = DshRuntimeConfig {
            access: "bogus".to_string(),
            port: 0,
            pinned_version: None,
        }
        .normalized();
        assert_eq!(fallback.access, "local");
        assert_eq!(fallback.port, 3080);

        let padded = DshRuntimeConfig {
            access: "local".to_string(),
            port: 3080,
            pinned_version: Some("  0.1.0  ".to_string()),
        }
        .normalized();
        assert_eq!(pinned_version_of(&padded), Some("0.1.0"));

        let blank = DshRuntimeConfig {
            access: "local".to_string(),
            port: 3080,
            pinned_version: Some("   ".to_string()),
        }
        .normalized();
        assert_eq!(blank.pinned_version, None, "a blank pin is no pin");
    }

    #[test]
    fn app_state_without_dsh_runtime_still_starts() {
        // Older app_state.json files have no dsh_runtime key at all; deleting
        // the key by hand must not break startup either.
        let stored = serde_json::json!({
            "minimize_to_tray": true,
            "last_active_main_tab": "codex"
        });
        let mut state: AppState = serde_json::from_value(stored).expect("legacy state should decode");
        assert_eq!(state.dsh_runtime.access, "local");
        assert_eq!(state.dsh_runtime.port, 3080);

        state.dsh_runtime = DshRuntimeConfig::default();
        let output = serde_json::to_value(&state).expect("encode state");
        assert_eq!(output["dsh_runtime"]["access"], "local");
        assert_eq!(output["dsh_runtime"]["port"], 3080);
    }

    #[test]
    fn dsh_runtime_config_serializes_with_the_documented_keys() {
        let value = serde_json::to_value(DshRuntimeConfig::default()).expect("encode config");
        assert_eq!(value["access"], "local");
        assert_eq!(value["port"], 3080);
        // No recorded version yet: the key is omitted, so pre-pin state files
        // and post-pin ones are distinguishable.
        assert!(value.get("pinnedVersion").is_none());

        let pinned = DshRuntimeConfig {
            pinned_version: Some("0.1.0".to_string()),
            ..DshRuntimeConfig::default()
        };
        let value = serde_json::to_value(pinned).expect("encode pinned config");
        assert_eq!(value["pinnedVersion"], "0.1.0");
    }

    /// The frontend save carries only access/port. It must never wipe the
    /// version the backend recorded after a start; an explicit pin in the
    /// payload (a future API) still wins.
    #[test]
    fn saving_dsh_runtime_config_preserves_the_pinned_version() {
        let stored = DshRuntimeConfig {
            pinned_version: Some("0.1.0".to_string()),
            ..DshRuntimeConfig::default()
        };
        let incoming = DshRuntimeConfig {
            access: "remote".to_string(),
            port: 3199,
            pinned_version: None,
        };
        let merged = merge_dsh_runtime_config(incoming, &stored);
        assert_eq!(merged.access, "remote");
        assert_eq!(merged.port, 3199);
        assert_eq!(pinned_version_of(&merged), Some("0.1.0"));

        let explicit = DshRuntimeConfig {
            pinned_version: Some("0.2.0".to_string()),
            ..DshRuntimeConfig::default()
        };
        let merged = merge_dsh_runtime_config(explicit, &stored);
        assert_eq!(pinned_version_of(&merged), Some("0.2.0"));

        let merged = merge_dsh_runtime_config(DshRuntimeConfig::default(), &DshRuntimeConfig::default());
        assert_eq!(merged.pinned_version, None);
    }

    /// Decode helper so the assertions above read as strings.
    fn pinned_version_of(config: &DshRuntimeConfig) -> Option<&str> {
        config.pinned_version.as_deref()
    }

    #[test]
    fn top_bar_hidden_filters_config_unknown_and_duplicate_items() {
        let hidden = vec![
            "config".to_string(),
            "codex".to_string(),
            "unknown".to_string(),
            "codex".to_string(),
            "claude".to_string(),
        ];
        assert_eq!(normalize_top_bar_hidden(&hidden), vec!["claude", "codex"]);
    }

    #[test]
    fn claude_busy_input_mode_defaults_and_rejects_unknown_values() {
        assert_eq!(default_claude_busy_input_mode(), "native");
        assert_eq!(normalize_claude_busy_input_mode("after-stop"), "after-stop");
        assert_eq!(normalize_claude_busy_input_mode("future-mode"), "native");
    }

    #[test]
    fn claude_startup_view_defaults_and_rejects_unknown_values() {
        assert_eq!(default_claude_startup_view(), "terminal");
        assert_eq!(
            normalize_claude_startup_view("conversation"),
            "conversation"
        );
        assert_eq!(normalize_claude_startup_view("terminal"), "terminal");
        assert_eq!(normalize_claude_startup_view("log"), "terminal");
        assert_eq!(
            normalize_claude_startup_view("future-view"),
            "terminal"
        );
    }

    #[test]
    fn claude_log_output_defaults_to_disabled() {
        let state = ToolState::default();
        assert_eq!(state.claude_log_output_enabled, None);
    }

    #[test]
    fn startup_bootstrap_matches_individual_state_normalizers() {
        let mut state = AppState {
            minimize_to_tray: true,
            last_active_main_tab: "codex".to_string(),
            top_bar_order: vec!["codex".to_string(), "config".to_string()],
            top_bar_hidden: vec!["claude".to_string(), "unknown".to_string()],
            window: WindowState {
                width: Some(1280.0),
                height: Some(800.0),
                x: Some(120.0),
                y: Some(80.0),
                extra: Map::new(),
            },
            ..AppState::default()
        };
        state.claude.claude_startup_view = Some("conversation".to_string());
        state.claude.claude_log_output_enabled = Some(true);
        state.claude.claude_busy_input_mode = Some("after-stop".to_string());
        state.claude.launch_dir = "D:/workspace".to_string();
        state.claude.project_drop_path_mode = "filename".to_string();
        state.terminal.font_size = 14.0;

        let bootstrap = startup_bootstrap_from_state(&state);

        assert_eq!(
            bootstrap.claude_startup_view,
            claude_startup_view_from_state(&state)
        );
        assert_eq!(
            bootstrap.claude_log_output_enabled,
            state.claude.claude_log_output_enabled.unwrap_or(false)
        );
        assert_eq!(
            bootstrap.claude_busy_input_mode,
            claude_busy_input_mode_from_state(&state)
        );
        assert_eq!(bootstrap.claude_launch_dir, state.claude.launch_dir);
        assert_eq!(
            bootstrap.claude_project_drop_path_mode,
            state.claude.project_drop_path_mode
        );
        assert_eq!(bootstrap.top_bar_layout, top_bar_layout_from_state(&state));
        assert_eq!(bootstrap.minimize_to_tray, state.minimize_to_tray);
        assert_eq!(bootstrap.terminal_font_size, state.terminal.font_size);
        assert_eq!(bootstrap.window_state, validated_window_state(&state));
        assert_eq!(
            bootstrap.last_active_main_tab,
            last_active_main_tab_from_state(&state)
        );
    }

    #[test]
    fn startup_bootstrap_preserves_invalid_window_fallback() {
        let state = AppState {
            window: WindowState {
                width: Some(40.0),
                height: Some(800.0),
                x: Some(0.0),
                y: Some(0.0),
                extra: Map::new(),
            },
            ..AppState::default()
        };

        assert_eq!(
            startup_bootstrap_from_state(&state).window_state,
            WindowState::default()
        );
    }

    #[test]
    fn startup_bootstrap_serializes_with_frontend_field_names() {
        let value = serde_json::to_value(startup_bootstrap_from_state(&AppState::default()))
            .expect("serialize startup bootstrap");

        assert!(value.get("claudeStartupView").is_some());
        assert!(value.get("claudeLogOutputEnabled").is_some());
        assert!(value.get("claudeBusyInputMode").is_some());
        assert!(value.get("claudeLaunchDir").is_some());
        assert!(value.get("claudeProjectDropPathMode").is_some());
        assert!(value.get("topBarLayout").is_some());
        assert!(value.get("minimizeToTray").is_some());
        assert!(value.get("terminalFontSize").is_some());
        assert!(value.get("windowState").is_some());
        assert!(value.get("lastActiveMainTab").is_some());
    }

    #[test]
    fn pane_width_snapshot_matches_individual_reads_and_omits_missing_keys() {
        let mut state = AppState::default();
        state.claude.pane_width = 315.0;
        state
            .pane_widths
            .insert("project-right-sidebar".to_string(), 340.0);
        let keys = vec![
            "claude".to_string(),
            "project-right-sidebar".to_string(),
            "missing".to_string(),
        ];

        let snapshot = pane_widths_from_state(&state, keys);

        assert_eq!(
            snapshot.get("claude").copied(),
            Some(pane_width_from_state(&state, "claude").expect("claude width"))
        );
        assert_eq!(
            snapshot.get("project-right-sidebar").copied(),
            Some(pane_width_from_state(&state, "project-right-sidebar").expect("project width"))
        );
        assert!(!snapshot.contains_key("missing"));
    }
}
