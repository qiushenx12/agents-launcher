# macOS 完整改写方案

> 将 Windows-only Claude Code 启动器完整迁移至 macOS。
> 不仅解决"能否编译"，而是实现完整功能对等——每个 Windows 特性都有 macOS 等价实现。
>
> 上次更新：2026-06-30 ｜ 状态：✅ 全部 Task 完成

---

## 目录

1. [项目全景拆解](#1-项目全景拆解)
2. [macOS 配置体系差异（核心认知）](#2-macos-配置体系差异核心认知)
3. [模块化适配矩阵](#3-模块化适配矩阵)
4. [详细改造方案](#4-详细改造方案)
5. [实施路线图](#5-实施路线图)

---

## 1. 项目全景拆解

### 1.1 总览

项目是一个 **Claude Code 图形化管理器**，核心功能：管理 Claude Code 的环境变量配置、启动 Claude Code、管理项目和会话、提供内置 PTY 终端、支持跨终端标签页通信和编排。

```
┌─────────────────────────────────────────────────────┐
│                    App.vue                           │
│  自定义标题栏 + 4 个主面板 + 状态栏 + 设置弹窗        │
├──────────┬──────────┬──────────┬─────────────────────┤
│  Config   │  Project  │ Terminal │  Orchestration      │
│  配置管理   │  项目管理  │ 终端管理   │  编排/多Agent      │
├──────────┴──────────┴──────────┴─────────────────────┤
│                Pinia Stores (状态层)                  │
│  claude.ts │ project.ts │ terminal.ts │ tabComm.ts    │
├──────────────────────────────────────────────────────┤
│           Tauri invoke() 桥接层 (IPC)                 │
├──────────────────────────────────────────────────────┤
│              Rust 后端 (tauri commands)               │
│  config_store  settings_manager  persistent_state     │
│  claude_launcher  registry  pty  session_manager     │
│  project_manager  model_fetcher  tab_cli  utils       │
└──────────────────────────────────────────────────────┘
```

### 1.2 四个面板详解

#### 面板 A：配置管理（Config）

| 层级 | 关键文件 | 职责 |
|------|---------|------|
| UI | `ClaudePanel.vue` | 左右分栏布局（配置列表 + 配置编辑） |
| UI | `ConfigList.vue` | 配置项列表，可拖拽排序、选择、删除 |
| UI | `ConfigEditor.vue` | 编辑环境变量键值对、"应用到环境变量"按钮、scope 选择 |
| UI | `LaunchOptions.vue` | 启动目录、away summary、skip permissions 选项、启动按钮 |
| UI | `SessionList.vue` | 显示最近的 Claude Code 会话历史 |
| UI | `ModelField.vue` | 模型选择下拉框 |
| UI | `TokenField.vue` | 令牌输入（密码模式） |
| Store | `stores/claude.ts` | 配置 CRUD、applyToRegistry()、模型获取、会话加载、设置管理 |
| Rust | `config_store.rs` | 持久化配置 JSON (`%APPDATA%/ClaudeEnvManager/env_configs.json`) |
| Rust | `registry.rs` | **Windows 注册表写入**（HKCU/HKLM + WM_SETTINGCHANGE） |
| Rust | `settings_manager.rs` | 读写 `~/.claude/settings.json`（permissions/awaySummary） |
| Rust | `session_manager.rs` | 解析 `~/.claude/history.jsonl` 获取会话列表 |
| Rust | `persistent_state.rs` | 窗口状态、面板宽度等持久化 |

**数据流：**
```
用户编辑配置 → claude.ts editingConfig
  → saveConfig() → config_store.rs → env_configs.json (持久化配置定义)
  → applyToRegistry() → registry.rs → Windows 注册表 (实际生效)
```

#### 面板 B：项目管理（Project）

| 层级 | 关键文件 | 职责 |
|------|---------|------|
| UI | `ProjectPanel.vue` | 项目布局总容器 |
| UI | `ProjectSidebar.vue` | 项目侧边栏（文件树/工具/终端等） |
| UI | `ProjectTerminalArea.vue` | 项目关联的终端区域 |
| UI | `RightSidebar.vue` | 右侧边栏（工具、文件预览、浏览器等） |
| UI | `ModuleToolbar.vue` | 模块工具栏 |
| Store | `stores/project.ts` | 项目 CRUD、会话管理、文件操作、Claude 启动（内置终端模式） |
| Rust | `project_manager.rs` | 项目/会话/最近文件 JSON 持久化 |

**数据流：**
```
项目面板 → project.ts createSession()
  → terminalStore.createTab() → claude_launcher.rs / pty/mod.rs
```

#### 面板 C：终端管理（Terminal）

| 层级 | 关键文件 | 职责 |
|------|---------|------|
| UI | `TerminalManager.vue` | 终端标签页管理器 |
| UI | `TerminalPane.vue` | xterm.js 终端实例 + PTY 桥接 |
| UI | `TerminalTab.vue` | 单个终端标签页 UI |
| UI | `SnapshotManager.vue` | 终端快照保存/恢复 |
| UI | `TabPermissionModal.vue` | 跨标签权限配置 |
| Store | `stores/terminal.ts` | 终端标签 CRUD、PTY 事件监听、xterm writer 注册 |
| Store | `stores/tabComm.ts` | 跨标签通信状态 |
| Rust | `pty/mod.rs` | PTY 创建/写入/Resize/终止（`portable-pty`） |
| Rust | `pty/session.rs` | PTY 会话结构 |
| Rust | `tab_cli.rs` | `tab-*` 命令解析、权限、快照、预设 |

**数据流：**
```
createTab() → pty_create (Rust) → portable_pty → shell
  → pty_output 事件 (Tauri event) → terminal.ts → xterm.js
xterm.js 输入 → pty_write (Rust) → PTY master → shell
```

#### 面板 D：编排管理（Orchestration）

| 层级 | 关键文件 | 职责 |
|------|---------|------|
| UI | `OrchestrationManager.vue` | 编排画布 + Agent 管理 |
| UI | `AgentRoleModal.vue` | Agent 角色/提示词编辑 |
| UI | `PresetManager.vue` | 编排预设管理 |
| Store | `stores/tabComm.ts` | 快照、预设数据 |
| Rust | `tab_cli.rs` | 预设持久化、快照持久化 |

### 1.3 Rust 后端模块完整清单

| 模块 | 跨平台? | 说明 |
|------|---------|------|
| `main.rs` | ❌ | `windows_subsystem = "windows"` 属性 |
| `lib.rs` | ❌ | `window_theme` 使用 Windows Dwm API |
| `claude_launcher.rs` | ❌ | 路径硬编码 + `CommandExt` |
| `utils.rs` | ❌ | `explorer` / `rundll32` |
| `registry.rs` | ❌ | 纯 Windows 注册表操作 |
| `pty/mod.rs` | ⚠️ | `kill_process_tree` 用 `taskkill`；`cmd.exe` 处理 |
| `pty/session.rs` | ✅ | 纯数据结构 |
| `config_store.rs` | ✅ | 使用 `dirs::data_dir()`（跨平台） |
| `settings_manager.rs` | ✅ | 使用 `dirs::home_dir()`（跨平台） |
| `persistent_state.rs` | ✅ | 使用 `dirs::data_dir()`（跨平台） |
| `session_manager.rs` | ✅ | 使用 `dirs::home_dir()`（跨平台） |
| `project_manager.rs` | ✅ | 使用 `dirs::data_dir()`（跨平台） |
| `model_fetcher.rs` | ✅ | 纯 HTTP（`reqwest`） |
| `tab_cli.rs` | ✅ | 纯数据结构 + `sha2` |

---

## 2. macOS 配置体系差异（核心认知）

### 2.1 Windows vs macOS 对比

| 能力 | Windows | macOS |
|------|---------|-------|
| **Claude Code 配置来源** | 系统环境变量 | `~/.claude/settings.json` 的 `env` 字段 |
| **环境变量持久化** | Windows 注册表 (HKCU/HKLM) | 无注册表；通过 shell profile (`.zshrc`) 或 launchd |
| **"应用到系统"** | 写入 HKLM（需管理员） | 无等价概念 |
| **"应用到用户"** | 写入 HKCU（无需提权） | 写入 `~/.claude/settings.json` 或 shell profile |
| **WM_SETTINGCHANGE** | 广播通知所有进程重启生效 | 无此机制；需重启 Claude Code |

### 2.2 对你当前环境的验证

你的 `~/.claude/settings.json`：

```json
{
  "env": {
    "ANTHROPIC_BASE_URL": "https://api.deepseek.com/anthropic",
    "ANTHROPIC_AUTH_TOKEN": "sk-...",
    "ANTHROPIC_MODEL": "deepseek-v4-flash",
    "ANTHROPIC_DEFAULT_OPUS_MODEL": "deepseek-v4-pro[1m]",
    ...
  }
}
```

Claude Code 进程启动时读取此文件，不是从系统环境变量读取（你 `env | grep ANTHROPIC` 看到的值是 Claude Code 启动后 export 的）。

### 2.3 架构影响

**Windows 版架构：**
```
配置 Profile (env_configs.json) → "应用到环境变量" → 注册表 → 系统 env vars → Claude Code 启动时读取
```

**macOS 版架构（需要改为）：**
```
配置 Profile (env_configs.json) → "应用设置" → ~/.claude/settings.json 的 env 字段 → Claude Code 启动时读取
```

这意味着：
1. **`registry.rs` 整个模块在 macOS 无意义** → 需要新增 `macos_env_applier.rs`（写入 `settings.json`）
2. **`settings_manager.rs` 需要扩展** → 当前只管理 `skipPermissions` 和 `awaySummaryDisabled`，需增加 `env` 字段管理
3. **前端 `ConfigEditor.vue` 的 scope 和注册表按钮需要改造** → macOS 没有"用户/系统"范围选择

---

## 3. 模块化适配矩阵

### 3.1 各功能模块适配策略

| # | 功能 | Windows 实现 | macOS 实现 | 改动量 |
|---|------|-------------|-----------|--------|
| 1 | 配置编辑/保存 | `env_configs.json` 读写 | **保持不变**（跨平台） | 无 |
| 2 | **"应用到环境变量"** | 写入注册表 | **改写为写入 `~/.claude/settings.json` 的 `env` 字段** | **大** |
| 3 | **设置管理** | 管理 `settings.json` 的权限/away 字段 | **扩展到管理 `env` 字段** | **中** |
| 4 | "打开环境变量面板" | `rundll32 sysdm.cpl` | 隐藏（macOS 无等价功能） | 小 |
| 5 | 选择用户/系统 scope | 注册表两种键路径 | 隐藏（macOS 无概念） | 小 |
| 6 | 查找 Claude 可执行文件 | `which claude` → `%LOCALAPPDATA%` fallbacks | `which claude` → Homebrew/NPM fallbacks | 中 |
| 7 | 启动 Claude | `CommandExt + CREATE_NEW_CONSOLE` | 普通 `Command::new` | 小 |
| 8 | 默认 Shell | `cmd.exe` | `$SHELL` → `/bin/zsh` | 小 |
| 9 | 目录打开 | `explorer` | `open` | 小 |
| 10 | PTY 进程终止 | `taskkill /T /F` | `kill -TERM/-KILL` 进程组 | 小 |
| 11 | 窗口主题 | `DwmSetWindowAttribute` | Tauri 原生支持 (macOS 自动适配) | 小 |
| 12 | PTY 创建 shell 类型 | `cmd.exe` → `COMSPEC` 检测 | `cmd.exe` → 替换为 `$SHELL` | 小 |
| 13 | 窗口属性 | `windows_subsystem = "windows"` | 无（macOS 不需要） | 小 |
| 14 | 构建目标 | NSIS (`.exe`) | `.app` / `.dmg` | 配置改动 |
| 15 | 图标 | `app.ico` | `app.icns` | 资源文件 |

### 3.2 改造优先级分组

```
P0（必须——否则不启动）
  └─ 构建配置 (14) + 窗口属性 (13) + cargo check 通过

P1（核心功能——用户可正常使用）
  ├─ 查找/启动 Claude (6, 7)
  ├─ 默认 Shell (8)
  └─ PTY 相关 (10, 12)

P2（配置管理功能完整对等）
  ├─ "应用到环境变量" → "写入 settings.json" (2, 3)
  └─ scope 文字调整/隐藏 (5)

P3（边缘功能）
  ├─ 目录打开 (9)
  ├─ 窗口主题 (11)
  ├─ 环境变量面板隐藏 (4)
  └─ 图标 (15)
```

---

## 4. 详细改造方案

### 4.0 准备工作：环境搭建

#### 4.0.1 构建配置

**文件**: `src-tauri/tauri.conf.json` → 新增 `src-tauri/tauri.macos.conf.json`

```json
{
  "bundle": {
    "targets": ["app", "dmg"],
    "icon": ["icons/app.icns"],
    "macOS": {
      "minimumSystemVersion": "13.0"
    }
  }
}
```

#### 4.0.2 图标

```bash
# 从源 PNG 生成 app.icns
mkdir app.iconset
cp icon_16.png app.iconset/icon_16x16.png
cp icon_32.png app.iconset/icon_16x16@2x.png
cp icon_32.png app.iconset/icon_32x32.png
cp icon_64.png app.iconset/icon_32x32@2x.png
cp icon_128.png app.iconset/icon_128x128.png
cp icon_256.png app.iconset/icon_128x128@2x.png
cp icon_256.png app.iconset/icon_256x256.png
cp icon_512.png app.iconset/icon_256x256@2x.png
cp icon_512.png app.iconset/icon_512x512.png
cp icon_1024.png app.iconset/icon_512x512@2x.png
iconutil -c icns app.iconset -o src-tauri/icons/app.icns
```

#### 4.0.3 `CLAUDE.md` 更新

标注为跨平台项目，移除 "Windows only" 描述。

---

### 4.1 `main.rs` —— 窗口入口属性

**文件**: `src-tauri/src/main.rs`

```rust
// 原：整行无条件启用 windows_subsystem
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// 改：仅 Windows 平台启用
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]
```

---

### 4.2 `lib.rs` —— 窗口主题

**文件**: `src-tauri/src/lib.rs:13-36`

```rust
mod window_theme {
    #[cfg(target_os = "windows")]
    fn set_titlebar_dark_mode(hwnd: windows::Win32::Foundation::HWND, dark: bool) {
        use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
        let value: i32 = if dark { 1 } else { 0 };
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd, DWMWINDOWATTRIBUTE(20),
                &value as *const _ as *const _, std::mem::size_of::<i32>() as u32,
            );
        }
    }

    #[tauri::command]
    pub fn set_titlebar_theme(window: tauri::Window, dark: bool) {
        #[cfg(target_os = "windows")]
        {
            let raw = window.hwnd().unwrap().0 as _;
            set_titlebar_dark_mode(windows::Win32::Foundation::HWND(raw), dark);
        }
        #[cfg(target_os = "macos")]
        {
            // macOS：Tauri v2 自动跟随系统主题。如需手动控制：
            // 使用 tauri::theme::Theme 或调用 NSAppearance
            let _ = dark;
        }
    }
}
```

---

### 4.3 `claude_launcher.rs` —— 查找与启动 Claude（核心功能）

**文件**: `src-tauri/src/claude_launcher.rs`

#### 完整重写：

```rust
use std::collections::HashMap;

#[tauri::command]
pub fn find_claude_executable() -> Result<Option<String>, String> {
    // 跨平台：优先 PATH
    if let Ok(path) = which::which("claude") {
        return Ok(Some(path.to_string_lossy().to_string()));
    }

    // Windows fallback 路径
    #[cfg(target_os = "windows")]
    {
        let win_candidates = vec![
            /* 保留原有 Windows 路径 */
        ];
        // ... 原有逻辑 ...
    }

    // macOS fallback 路径
    #[cfg(target_os = "macos")]
    {
        let mac_candidates = vec![
            "/opt/homebrew/bin/claude".to_string(),
            "/usr/local/bin/claude".to_string(),
        ];
        for path in mac_candidates {
            if std::path::Path::new(&path).is_file() {
                return Ok(Some(path));
            }
        }
        // npm global
        if let Some(home) = dirs::home_dir() {
            let npm = home.join(".npm-global").join("bin").join("claude");
            if npm.is_file() {
                return Ok(Some(npm.to_string_lossy().to_string()));
            }
        }
    }

    Ok(None)
}

#[tauri::command]
pub fn launch_claude(
    exe: String,
    env_vars: HashMap<String, String>,
    args: Vec<String>,
    cwd: Option<String>,
) -> Result<(), String> {
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(&args);

    // 合并环境变量
    let mut env: HashMap<String, String> = std::env::vars().collect();
    for (k, v) in &env_vars {
        env.insert(k.clone(), v.clone());
    }
    cmd.envs(&env);

    if let Some(dir) = &cwd {
        if !dir.is_empty() { cmd.current_dir(dir); }
    }

    // Windows：创建新控制台窗口
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x00000010); // CREATE_NEW_CONSOLE
    }

    cmd.spawn()
        .map_err(|e| format!("Failed to launch claude: {}", e))?;
    Ok(())
}
```

---

### 4.4 ~~`registry.rs`~~ → 新增 `env_applier.rs`（核心功能改写）

**Windows 注册表写入在 macOS 上被替换为 `~/.claude/settings.json` env 字段写入。**

这是整个项目最核心的架构差异——Windows 版把环境变量写到注册表让系统读取，macOS 应该把配置写到 `~/.claude/settings.json` 让 Claude Code 读取。

#### 新增 `src-tauri/src/env_applier.rs`：

```rust
//! 跨平台环境变量应用器
//!
//! Windows: 写入注册表（保留现有 registry.rs 行为）
//! macOS:   写入 ~/.claude/settings.json 的 env 字段

use std::collections::HashMap;
use serde_json::{Map, Value};

#[tauri::command]
pub fn apply_env_vars(vars: HashMap<String, String>, scope: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        // 转发到原有 registry.rs 逻辑
        crate::registry::apply_env_vars_impl(vars, scope)
    }

    #[cfg(target_os = "macos")]
    {
        // macOS：将 env vars 写入 ~/.claude/settings.json
        apply_env_vars_macos(vars)
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (vars, scope);
        Err("Unsupported platform".to_string())
    }
}

#[cfg(target_os = "macos")]
fn apply_env_vars_macos(vars: HashMap<String, String>) -> Result<(), String> {
    let settings_path = dirs::home_dir()
        .ok_or_else(|| "无法确定用户主目录".to_string())?
        .join(".claude")
        .join("settings.json");

    // 读取现有 settings.json
    let mut obj: Map<String, Value> = if settings_path.exists() {
        let raw = std::fs::read_to_string(&settings_path)
            .map_err(|e| format!("读取 settings.json 失败: {}", e))?;
        serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|v| v.into_object())
            .unwrap_or_default()
    } else {
        Map::new()
    };

    // 构建 env 对象
    let mut env_map = Map::new();
    for (k, v) in &vars {
        if !v.is_empty() {
            env_map.insert(k.clone(), Value::String(v.clone()));
        }
    }

    // 写入 env 字段（保留其他现有字段如 permissions）
    obj.insert("env".to_string(), Value::Object(env_map));

    // 确保目录存在
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败: {}", e))?;
    }

    // 写回文件
    let json = serde_json::to_string_pretty(&Value::Object(obj))
        .map_err(|e| format!("序列化 settings.json 失败: {}", e))?;
    std::fs::write(&settings_path, json.as_bytes())
        .map_err(|e| format!("写入 settings.json 失败: {}", e))?;

    Ok(())
}
```

> 说明：`registry.rs` 保留，但提取出一个 `apply_env_vars_impl` 供 Windows 调用。
> macOS 不走注册表，直接修改 `settings.json`。

**同时修改 `lib.rs` 注册命令：**

```rust
// 替换 registry::apply_env_vars 为 env_applier::apply_env_vars
```

---

### 4.5 `settings_manager.rs` —— 扩展管理 env 字段

**文件**: `src-tauri/src/settings_manager.rs`

当前只管理 `skipPermissions` 和 `awaySummaryDisabled`。需要新增 **读取/写入 env 字段** 的功能，配合前端"应用设置"功能。

**新增命令：**

```rust
/// 读取 settings.json 中的 env 字段
#[tauri::command]
pub fn load_claude_env() -> Result<HashMap<String, String>, String> {
    let path = settings_path()?;
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取 settings.json 失败: {}", e))?;
    let json: Value = serde_json::from_str(&raw).unwrap_or(Value::Object(Map::new()));
    let env = json.get("env").and_then(|v| v.as_object());
    let mut result = HashMap::new();
    if let Some(env_map) = env {
        for (k, v) in env_map {
            if let Some(s) = v.as_str() {
                result.insert(k.clone(), s.to_string());
            }
        }
    }
    Ok(result)
}

/// 写入 settings.json 中的 env 字段（覆盖整个 env 对象）
#[tauri::command]
pub fn save_claude_env(env: HashMap<String, String>) -> Result<(), String> {
    let path = settings_path()?;
    let mut obj: Map<String, Value> = if path.exists() {
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("读取 settings.json 失败: {}", e))?;
        serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|v| v.into_object())
            .unwrap_or_default()
    } else {
        Map::new()
    };
    // 构建 env 对象
    let env_map: Map<String, Value> = env.into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect();
    obj.insert("env".to_string(), Value::Object(env_map));
    // 写回
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败: {}", e))?;
    }
    let json = serde_json::to_string_pretty(&Value::Object(obj))
        .map_err(|e| format!("序列化 settings.json 失败: {}", e))?;
    std::fs::write(&path, json.as_bytes())
        .map_err(|e| format!("写入 settings.json 失败: {}", e))?;
    Ok(())
}
```

---

### 4.6 `utils.rs` —— 平台工具函数

**文件**: `src-tauri/src/utils.rs`

```rust
#[tauri::command]
pub fn open_directory(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("Failed to open directory: {}", e))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("Failed to open directory: {}", e))?;
    }
    Ok(())
}

#[tauri::command]
pub fn open_env_vars_dialog() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("rundll32")
            .args(["sysdm.cpl,EditEnvironmentVariables"])
            .spawn()
            .map_err(|e| format!("Failed to open env vars dialog: {}", e))?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        // macOS 无等价系统面板
        Err("macOS 不支持直接打开环境变量面板。请通过 ~/.claude/settings.json 或在终端中设置。".to_string())
    }
}
```

---

### 4.7 `pty/mod.rs` —— PTY 进程清理 + Shell 检测

**文件**: `src-tauri/src/pty/mod.rs:389-403` `kill_process_tree`：

```rust
fn kill_process_tree(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/T", "/F", "/PID", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
        let _ = cmd.output();
    }
    #[cfg(target_os = "macos")]
    {
        // macOS：发送到进程组，先 SIGTERM 后 SIGKILL
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &format!("-{}", pid)])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        // 等 100ms 后强制 kill
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{}", pid)])
            .output();
    }
}
```

**`pty_create` 中 cmd.exe 处理（第 124-134 行）：**

```rust
// 保留 Windows 分支：
#[cfg(target_os = "windows")]
let exe = if cmd[0].eq_ignore_ascii_case("cmd.exe") || cmd[0].eq_ignore_ascii_case("cmd") {
    env.get("COMSPEC")
        .cloned()
        .or_else(|| std::env::var("COMSPEC").ok())
        .unwrap_or_else(|| "C:\\Windows\\System32\\cmd.exe".to_string())
} else {
    cmd[0].clone()
};

// macOS/Linux：当前端传 cmd.exe 时，替换为用户 shell
#[cfg(not(target_os = "windows"))]
let exe = {
    if cmd[0] == "cmd.exe" || cmd[0] == "cmd" {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
    } else {
        cmd[0].clone()
    }
};
```

---

### 4.8 前端改造

#### 4.8.1 新增 `useDefaultShell` composable（统一默认 Shell）

**新增**: `src/composables/useDefaultShell.ts`

```typescript
export function getDefaultShell(): string[] {
  // Tauri v2: 通过 platform() 检测
  // 也可以使用 navigator.userAgent 或 @tauri-apps/api/os
  return ['/bin/zsh']; // 先返回 zsh，启动时 PTY 自动替换为 SHELL
}
```

**修改以下文件：**

| 文件 | 行 | 原代码 | 替换为 |
|------|-----|--------|--------|
| `src/stores/project.ts` | 809 | `['cmd.exe']` | `getDefaultShell()` |
| `src/stores/project.ts` | 956 | `['cmd.exe']` | `getDefaultShell()` |
| `src/components/terminal/TerminalManager.vue` | 32 | `['cmd.exe']` | `getDefaultShell()` |
| `src/components/orchestration/OrchestrationManager.vue` | 500 | `cmd: ['cmd.exe']` | `cmd: getDefaultShell()` |

#### 4.8.2 `ConfigEditor.vue` —— 平台适配 UI

**文件**: `src/components/claude/ConfigEditor.vue`

```vue
<template>
  <!-- ... 其他字段保持不变 ... -->

  <!-- 应用范围：仅 Windows 显示 -->
  <div v-if="isWindows" class="scope-row">
    <span class="scope-label">应用范围</span>
    <label class="radio-label">
      <input type="radio" v-model="store.scope" value="user" /> 当前用户
    </label>
    <label class="radio-label">
      <input type="radio" v-model="store.scope" value="system" /> 系统（所有用户）
    </label>
    <span class="scope-hint">修改系统变量需要管理员权限</span>
  </div>

  <!-- Action buttons -->
  <div class="action-row">
    <button class="btn btn-primary" @click="store.saveConfig()">保存配置</button>
    <button class="btn btn-primary" @click="store.applyEnv()">
      {{ isWindows ? '应用到环境变量' : '应用到 Claude 配置' }}
    </button>
  </div>
  <div class="action-row action-row--tools">
    <button class="btn btn-secondary" @click="openClaudeSettings()">
      打开 Claude 配置文件
    </button>
    <button class="btn btn-secondary" @click="openClaudePath()">打开 Claude Code 路径</button>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { platform } from '@tauri-apps/api/os'

const isWindows = ref(false)

onMounted(async () => {
  try {
    const p = await platform()
    isWindows.value = p === 'win32'
  } catch { /* dev browser fallback */ }
})
</script>
```

#### 4.8.3 `claude.ts` store —— 拆分 applyToRegistry 为 applyEnv

**文件**: `src/stores/claude.ts`

```typescript
async function applyEnv() {
  const vars = editingConfig.value.vars
  const nonEmpty = Object.entries(vars).filter(([, v]) => v)

  // 检测平台
  const { platform } = await import('@tauri-apps/api/os')
  const os = await platform()

  if (os === 'win32') {
    // Windows：原有注册表逻辑
    await applyToRegistryWindows(vars, nonEmpty)
  } else {
    // macOS：写入 ~/.claude/settings.json
    await applyToSettingsJson(vars, nonEmpty)
  }
}

async function applyToSettingsJson(
  vars: Record<string, string>,
  nonEmpty: [string, string][]
) {
  if (nonEmpty.length === 0) {
    statusMessage.value = '没有需要应用的环境变量'
    return
  }
  const confirmed = await confirm(
    `将以下 ${nonEmpty.length} 个环境变量写入 ~/.claude/settings.json:\n\n` +
      nonEmpty.map(([k, v]) => `  ${k}=${v}`).join('\n'),
    { title: '确认应用配置', kind: 'warning' }
  )
  if (!confirmed) return

  try {
    // 构建完整的 env 对象：已知 key 全部写入，空值也写入（清空）
    const fullVars: Record<string, string> = {}
    for (const k of KNOWN_ENV_KEYS) {
      fullVars[k] = vars[k] ?? ''
    }
    // 写入 settings.json
    await invoke('save_claude_env', { env: fullVars })
    statusMessage.value = '已应用到 Claude Code 配置 (~/.claude/settings.json)'
  } catch (e) {
    statusMessage.value = `应用失败: ${e}`
  }
}

// 暴露新方法
return {
  // ... 原有 ...
  applyEnv,    // 替换 applyToRegistry
}
```

#### 4.8.4 环境变量面板按钮处理

在 `ConfigEditor.vue` 中：

```vue
<!-- macOS 上用"打开 Claude 配置文件"替代 -->
<button class="btn btn-secondary" @click="openClaudeSettings()">
  打开 Claude 配置文件
</button>

<script>
async function openClaudeSettings() {
  try {
    const homedir = await invoke<string>('get_claude_config_dir')
    const settingsPath = homedir.replace(/\/[^/]+$/, '') + '/.claude/settings.json'
    await invoke('open_directory', { path: settingsPath })
  } catch {
    store.statusMessage = '打开失败'
  }
}
</script>
```

---

### 4.9 Cargo.toml 确认

**文件**: `src-tauri/Cargo.toml`

Windows 条件依赖保持不变。macOS 构建不编译 `winreg`/`windows` crate。

新增 `env_applier.rs` 模块需要添加为 `mod env_applier` 并在 `lib.rs` 中注册。

---

## 5. 实施路线图

### 第一阶段：编译通过（优先级 P0）

```
Step 1: 环境准备
  ├── macOS 上 npm install
  ├── rustup update stable
  └── cargo check（预期失败）

Step 2: 修复编译错误
  ├── main.rs ─── windows_subsystem 加 cfg
  ├── lib.rs ──── window_theme 加 cfg(macos) 空分支
  ├── claude_launcher.rs ─── CommandExt 放入 cfg(windows)
  ├── utils.rs ─── explorer/rundll32 放入 cfg(windows)
  └── pty/mod.rs ── taskkill 放入 cfg(windows)

Step 3: cargo check 通过
  └── npm run tauri build（预期先只出 .app）
```

### 第二阶段：核心功能（优先级 P1）

```
Step 4: Claude Code 查找/启动
  ├── claude_launcher.rs macOS fallback 路径
  └── 验证：which claude、Homebrew 安装检测

Step 5: PTY 终端
  ├── pty/mod.rs macOS shell 检测（cmd.exe → $SHELL）
  ├── pty/mod.rs macOS kill_process_tree（kill -TERM）
  └── 验证：终端可启动 zsh、输入命令

Step 6: 前端默认 Shell
  ├── 新增 composables/useDefaultShell.ts
  ├── project.ts 替换 cmd.exe
  ├── TerminalManager.vue 替换 cmd.exe
  └── OrchestrationManager.vue 替换 cmd.exe
```

### 第三阶段：配置管理对等（优先级 P2）

```
Step 7: 新增 env_applier.rs
  ├── Windows → 转发 registry.rs
  ├── macOS → 写入 ~/.claude/settings.json env 字段
  └── 注册命令到 lib.rs

Step 8: settings_manager.rs 扩展
  ├── load_claude_env / save_claude_env 命令
  └── 注册到 lib.rs

Step 9: 前端改造
  ├── claude.ts: applyToRegistry → applyEnv 分支逻辑
  ├── ConfigEditor.vue: scope 隐藏 + 按钮文案
  └── 验证：编辑 → 应用 → 检查 settings.json → 启动 Claude Code
```

### 第四阶段：打磨（优先级 P3）

```
Step 10: 边缘功能
  ├── 图标 app.icns
  ├── tauri.macos.conf.json
  ├── 构建文档更新
  └── 无边框窗口验证
```

---

## 附录 A：`dirs` crate 在各平台解析的路径

| API | Windows (示例) | macOS (示例) |
|-----|---------------|-------------|
| `dirs::data_dir()` | `C:\Users\<user>\AppData\Roaming` | `~/Library/Application Support` |
| `dirs::home_dir()` | `C:\Users\<user>` | `/Users/<user>` |
| `dirs::config_dir()` | `C:\Users\<user>\AppData\Roaming` | `~/Library/Application Support` |

所以 `config_store.rs` 中 `app_data_dir()` 返回：
- Windows: `%APPDATA%/ClaudeEnvManager`
- macOS: `~/Library/Application Support/ClaudeEnvManager`

## 附录 B：macOS 上 Claude Code 常用查找路径

| 安装方式 | 路径 |
|---------|------|
| Homebrew | `/opt/homebrew/bin/claude` |
| npm global | `/usr/local/bin/claude` 或 `~/.npm-global/bin/claude` |
| 用户自定义 | `which claude` |

## 附录 C：文件改动汇总清单

| 文件 | 操作 | 性质 |
|------|------|------|
| `src-tauri/tauri.macos.conf.json` | **新增** | 构建配置 |
| `src-tauri/icons/app.icns` | **新增** | 资源 |
| `src-tauri/src/env_applier.rs` | **新增** | Rust 模块 |
| `src/composables/useDefaultShell.ts` | **新增** | Vue composable |
| `src-tauri/src/main.rs:2` | 修改 | 1 行 |
| `src-tauri/src/lib.rs:13-36` | 修改 | window_theme 加 macOS 分支 |
| `src-tauri/src/lib.rs:38-111` | 修改 | 注册新命令 |
| `src-tauri/src/claude_launcher.rs` | 修改 | 全模块 |
| `src-tauri/src/utils.rs:24-45` | 修改 | 加 macOS 分支 |
| `src-tauri/src/pty/mod.rs:124-134` | 修改 | cmd.exe 处理 |
| `src-tauri/src/pty/mod.rs:389-403` | 修改 | kill_process_tree |
| `src-tauri/src/settings_manager.rs` | **扩展** | 新增 load/save_claude_env |
| `src/stores/claude.ts:210-247` | 修改 | applyToRegistry → applyEnv |
| `src/components/claude/ConfigEditor.vue` | 修改 | scope 隐藏 + 按钮文案 |
| `src/stores/project.ts:809,956` | 修改 | cmd.exe → getDefaultShell() |
| `src/components/terminal/TerminalManager.vue:32` | 修改 | 同上 |
| `src/components/orchestration/OrchestrationManager.vue:500` | 修改 | 同上 |
| `BUILD.md` | 修改 | 增加 macOS 环境说明 |
| `CLAUDE.md` | 修改 | 移除 "Windows only" |
