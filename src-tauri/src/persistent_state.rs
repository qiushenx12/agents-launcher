//! persistent_state.rs
//!
//! Manages all UI state in a single file: %APPDATA%\ClaudeEnvManager\app_state.json

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn app_data_dir() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|d| d.join("ClaudeEnvManager"))
        .ok_or_else(|| "Could not determine %APPDATA% directory".to_string())
}

fn app_state_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("app_state.json"))
}

// ---------------------------------------------------------------------------
// AppState structure
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct WindowState {
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub x: Option<f64>,
    pub y: Option<f64>,
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
    #[serde(default = "default_pane_width")]
    pub pane_width: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_sizes: Option<[f64; 2]>,
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

impl Default for ToolState {
    fn default() -> Self {
        Self {
            config_order: Vec::new(),
            launch_dir: String::new(),
            use_builtin_terminal: false,
            project_drop_path_mode: default_drop_path_mode(),
            pane_width: default_pane_width(),
            pane_sizes: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TerminalState {
    #[serde(default = "default_font_size")]
    pub font_size: f64,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self { font_size: default_font_size() }
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct AppState {
    #[serde(default)]
    pub window: WindowState,
    #[serde(default)]
    pub claude: ToolState,
    #[serde(default)]
    pub terminal: TerminalState,
    #[serde(default = "default_last_active_main_tab")]
    pub last_active_main_tab: String,
}

fn default_last_active_main_tab() -> String {
    "config".to_string()
}

// ---------------------------------------------------------------------------
// Core read/write
// ---------------------------------------------------------------------------

fn load_state() -> AppState {
    let path = match app_state_path() {
        Ok(p) => p,
        Err(_) => return AppState::default(),
    };
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return migrate_legacy().unwrap_or_default(),
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn save_state(state: &AppState) -> Result<(), String> {
    let path = app_state_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create state directory: {e}"))?;
    }
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| format!("Failed to serialise state: {e}"))?;
    fs::write(&path, json.as_bytes())
        .map_err(|e| format!("Failed to write state file: {e}"))?;
    Ok(())
}

/// Read-modify-write helper
fn update_state<F: FnOnce(&mut AppState)>(f: F) -> Result<(), String> {
    let mut state = load_state();
    f(&mut state);
    save_state(&state)
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

// PLACEHOLDER_COMMANDS

// ---------------------------------------------------------------------------
// Key → tool field mapping
// ---------------------------------------------------------------------------

fn tool_state_mut<'a>(state: &'a mut AppState, _key: &str) -> &'a mut ToolState {
    &mut state.claude
}

fn tool_state_ref<'a>(state: &'a AppState, _key: &str) -> &'a ToolState {
    &state.claude
}

// ---------------------------------------------------------------------------
// Tauri commands — Window
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_window_state() -> Result<WindowState, String> {
    let state = load_state().window;
    // Guard against corrupted state (zero size or way off-screen).
    let invalid = state.width.map_or(false, |v| v <= 50.0)
        || state.height.map_or(false, |v| v <= 50.0)
        || state.x.map_or(false, |v| v < -10000.0)
        || state.y.map_or(false, |v| v < -10000.0);
    if invalid {
        return Ok(WindowState::default());
    }
    Ok(state)
}

#[tauri::command]
pub fn save_window_state(state: WindowState) -> Result<(), String> {
    // Don't persist zero-size or wildly off-screen positions.
    let invalid = state.width.map_or(false, |v| v <= 0.0)
        || state.height.map_or(false, |v| v <= 0.0)
        || state.x.map_or(false, |v| v < -10000.0)
        || state.y.map_or(false, |v| v < -10000.0);
    if invalid {
        return Ok(());
    }
    update_state(|s| s.window = state)
}

// ---------------------------------------------------------------------------
// Tauri commands — Launch directory
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_launch_dir() -> Result<String, String> {
    Ok(load_state().claude.launch_dir)
}

#[tauri::command]
pub fn save_launch_dir(dir: String) -> Result<(), String> {
    update_state(|s| s.claude.launch_dir = dir)
}

// ---------------------------------------------------------------------------
// Tauri commands — Pane width
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_pane_width(key: String) -> Result<f64, String> {
    Ok(tool_state_ref(&load_state(), &key).pane_width)
}

#[tauri::command]
pub fn save_pane_width(key: String, width: f64) -> Result<(), String> {
    update_state(|s| tool_state_mut(s, &key).pane_width = width)
}

// ---------------------------------------------------------------------------
// Tauri commands — Terminal font size
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_terminal_font_size() -> Result<f64, String> {
    Ok(load_state().terminal.font_size)
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
    Ok(tool_state_ref(&load_state(), &key).config_order.clone())
}

#[tauri::command]
pub fn save_config_order(key: String, order: Vec<String>) -> Result<(), String> {
    update_state(|s| tool_state_mut(s, &key).config_order = order)
}

// ---------------------------------------------------------------------------
// Tauri commands — Use builtin terminal
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_use_builtin_terminal(key: String) -> Result<bool, String> {
    Ok(tool_state_ref(&load_state(), &key).use_builtin_terminal)
}

#[tauri::command]
pub fn save_use_builtin_terminal(key: String, value: bool) -> Result<(), String> {
    update_state(|s| tool_state_mut(s, &key).use_builtin_terminal = value)
}

// ---------------------------------------------------------------------------
// Tauri commands — Project drop path mode
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_project_drop_path_mode(key: String) -> Result<String, String> {
    Ok(tool_state_ref(&load_state(), &key).project_drop_path_mode.clone())
}

#[tauri::command]
pub fn save_project_drop_path_mode(key: String, value: String) -> Result<(), String> {
    update_state(|s| tool_state_mut(s, &key).project_drop_path_mode = value)
}

// ---------------------------------------------------------------------------
// Tauri commands — last active main tab
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn load_last_active_main_tab() -> Result<String, String> {
    Ok(load_state().last_active_main_tab)
}

#[tauri::command]
pub fn save_last_active_main_tab(tab: String) -> Result<(), String> {
    let normalized = match tab.as_str() {
        "config" | "terminal" | "project" | "orchestration" => tab,
        _ => "config".to_string(),
    };
    update_state(|s| s.last_active_main_tab = normalized)
}
