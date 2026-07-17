# macOS 改写方案

> 基于完整代码调研，将 Windows-only Tauri 应用迁移至跨平台（保留 Windows + 新增 macOS 支持）。
> **状态：✅ 已完成** ｜ 上次更新：2026-06-30

---

## 实施进度总览

| 阶段 | 内容 | 状态 |
|------|------|------|
| 阶段一 | 构建环境与配置（tauri.macos.conf.json + app.icns） | ✅ 完成 |
| 阶段二 | Rust 后端适配（main.rs, lib.rs, claude_launcher.rs, utils.rs, pty/mod.rs, env_applier.rs, settings_manager.rs, registry.rs） | ✅ 完成 |
| 阶段三 | 前端适配（useDefaultShell.ts, usePlatform.ts, cmd.exe 替换, ConfigEditor.vue, claude.ts, ProjectTerminalArea.vue, App.vue） | ✅ 完成 |
| 阶段四 | Cargo 依赖确认 | ✅ 无需改动 |
| 编译验证 | cargo check 0E 0W + vue-tsc 0E | ✅ 通过 |
| 构建打包 | .app (16MB) + .dmg (6.6MB) | ✅ 通过 |
| 额外修复 | PTY PATH 修复（GUI 应用 PATH 不全）、emoji 终端渲染、APPDATA 错误消息修正 | ✅ 完成 |

### 构建产物

```
src-tauri/target/release/bundle/macos/ClaudeCode启动器.app (16MB)
src-tauri/target/release/bundle/dmg/ClaudeCode启动器_1.0.0_aarch64.dmg (6.6MB)
```

### 与原始方案的主要偏差

1. **env_applier.rs 替代 registry.rs**：原方案计划让 registry.rs 报错，实际创建了专门的 `env_applier.rs`，macOS 写入 `~/.claude/settings.json`
2. **settings_manager.rs 扩展**：新增 `load_claude_env` / `save_claude_env` 命令
3. **PTY PATH 修复**：macOS GUI 应用 PATH 不含 `/usr/local/bin`，`pty_create` 和 `launch_claude` 中显式追加
4. **usePlatform.ts 改用 navigator.platform**：`@tauri-apps/api/os` 在 Tauri v2 中不可用
5. **icon.png 占位**：Tauri v2 `generate_context!()` 在 macOS 编译时需要 `icons/icon.png` 作为 fallback

---

## 1. 现状概览

### 1.1 Windows 独占汇总

| 类别 | 文件 | 问题 |
|------|------|------|
| 打包配置 | `src-tauri/tauri.conf.json` | `bundle.targets: ["nsis"]`，仅生成 `.exe` 安装包 |
| 图标 | `src-tauri/icons/app.ico` | 仅有 Windows `.ico` 格式 |
| 构建文档 | `BUILD.md`, `dev.bat`, `build.bat` | 全部面向 Windows（MSVC、NSIS、`.bat` 脚本） |
| Rust: 条件依赖 | `src-tauri/Cargo.toml:34-41` | `[target.'cfg(windows)'.dependencies]` 含 `winreg` 和 `windows` crate |
| Rust: Claude 查找 | `src-tauri/src/claude_launcher.rs` | 硬编码 `claude.exe` / `claude.cmd` + `%LOCALAPPDATA%` 路径 |
| Rust: Claude 启动 | `src-tauri/src/claude_launcher.rs:50-75` | 使用 `std::os::windows::process::CommandExt` + `CREATE_NEW_CONSOLE` |
| Rust: 注册表 | `src-tauri/src/registry.rs` | 完全 Windows-only（`winreg` + `WM_SETTINGCHANGE`） |
| Rust: 工具函数 | `src-tauri/src/utils.rs:24-45` | `open_directory` 用 `explorer`，`open_env_vars_dialog` 用 `rundll32` |
| Rust: 进程树清理 | `src-tauri/src/pty/mod.rs:389-403` | `kill_process_tree` 用 `taskkill` + `CREATE_NO_WINDOW` |
| Rust: 窗口主题 | `src-tauri/src/lib.rs:13-36` | `set_titlebar_dark_mode` 通过 Windows `DwmSetWindowAttribute` |
| Rust: PTY 创建 | `src-tauri/src/pty/mod.rs:124-134` | Windows 分支处理 `cmd.exe` → `COMSPEC` |
| Rust: 入口属性 | `src-tauri/src/main.rs:2` | `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` |
| Rust: 路径文案 | `config_store.rs`, `persistent_state.rs`, `settings_manager.rs` 多处 | 注释和错误消息写 `%APPDATA%` / `%USERPROFILE%` |
| 前端: 默认 Shell | `src/stores/project.ts:809,956` | `['cmd.exe']` 硬编码 |
| 前端: 默认 Shell | `src/components/terminal/TerminalManager.vue:32` | `['cmd.exe']` 硬编码 |
| 前端: 默认 Shell | `src/components/orchestration/OrchestrationManager.vue:500` | `cmd: ['cmd.exe']` |
| 前端: 注册表 UI | `src/components/claude/ConfigEditor.vue:132-136` | "应用到环境变量"、"打开环境变量" 按钮（Windows 注册表功能） |
| 前端: 环境变量 | `src/stores/claude.ts:210-247` | `applyToRegistry()` 调用 Windows-only `apply_env_vars` |

### 1.2 已跨平台的代码

以下模块使用 `dirs` crate 或标准 Rust API，本身跨平台、**无需修改**：

- `session_manager.rs` — history.jsonl 文件监听
- `settings_manager.rs` — `.claude/settings.json` 读写
- `config_store.rs` — 配置 JSON 读写
- `persistent_state.rs` — 应用状态持久化
- `project_manager.rs` — 项目管理
- `model_fetcher.rs` — HTTP 模型列表获取（基于 `reqwest`）
- `tab_cli.rs` — 跨标签通信协议（PTY 数据处理）
- `pty/session.rs` — 会话数据结构

---

## 2. 实施步骤

### 阶段一：构建环境与配置（非代码）

#### 2.1 Tauri macOS 配置文件

**新增** `src-tauri/tauri.macos.conf.json`：

```json
{
  "bundle": {
    "targets": ["app", "dmg"],
    "icon": ["icons/app.icns"],
    "macOS": {
      "minimumSystemVersion": "13.0",
      "exceptionDomain": "",
      "signing": {
        "identity": "-"
      }
    }
  }
}
```

Tauri v2 会自动合并此配置到 `tauri.conf.json` 之上（当 `cfg(target_os = "macos")` 时）。

#### 2.2 macOS 图标

从现有 `app.ico` 的源图生成：

```bash
# 需要 iconutil（Xcode 工具）
mkdir app.iconset
# 放入各尺寸 png（16, 32, 128, 256, 512 及其 @2x）
iconutil -c icns app.iconset -o src-tauri/icons/app.icns
```

#### 2.3 更新构建文档

重写 `BUILD.md`，增加 macOS 环境说明：

```markdown
## macOS 环境要求

- **macOS** >= 13 (Ventura)
- **Xcode Command Line Tools**: `xcode-select --install`
- **Node.js** >= 18
- **Rust** stable: `rustup update stable`
- 目标架构: `rustup target add aarch64-apple-darwin` (Apple Silicon)

## macOS 开发模式

```bash
npm run tauri dev
```

## macOS 生产构建

```bash
# Apple Silicon
npm run tauri build -- --target aarch64-apple-darwin --bundles app,dmg

# Universal (Apple Silicon + Intel)
npm run tauri build -- --target universal-apple-darwin --bundles app,dmg
```
```

删除或废弃 `dev.bat`、`build.bat`（保留但标记为 Windows-only）。

---

### 阶段二：Rust 后端适配（关键）

#### 2.4 `main.rs` —— 平台窗口属性

**文件**: `src-tauri/src/main.rs`
**行**: 2

```rust
// 原版（Windows-only）:
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// 改为跨平台:
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]
```

#### 2.5 `lib.rs` —— 窗口主题分离

**文件**: `src-tauri/src/lib.rs`
**行**: 13-36

```rust
mod window_theme {
    #[cfg(target_os = "windows")]
    fn set_titlebar_dark_mode(hwnd: windows::Win32::Foundation::HWND, dark: bool) {
        // ... 保持原有 Windows 实现 ...
    }

    #[cfg(target_os = "macos")]
    fn set_titlebar_dark_mode(dark: bool) {
        // macOS 使用 Tauri 自带的 theme API，无需额外操作
        // 可调用 NSAppearance 设置，但 Tauri v2 已处理
    }

    #[tauri::command]
    pub fn set_titlebar_theme(window: tauri::Window, dark: bool) {
        #[cfg(target_os = "windows")]
        {
            let hwnd = windows::Win32::Foundation::HWND(window.hwnd().unwrap().0 as _);
            set_titlebar_dark_mode(hwnd, dark);
        }
        #[cfg(target_os = "macos")]
        {
            set_titlebar_dark_mode(dark);
        }
    }
}
```

#### 2.6 `claude_launcher.rs` —— 查找与启动

**文件**: `src-tauri/src/claude_launcher.rs`

**`find_claude_executable`** 改造（第 4-28 行）：

```rust
#[tauri::command]
pub fn find_claude_executable() -> Result<Option<String>, String> {
    // Try PATH first（跨平台）
    if let Ok(path) = which::which("claude") {
        return Ok(Some(path.to_string_lossy().to_string()));
    }

    #[cfg(target_os = "windows")]
    {
        // Windows fallback 路径（原有）
        let candidates = vec![
            r"%LOCALAPPDATA%\Programs\claude\claude.exe",
            r"%LOCALAPPDATA%\claude\claude.exe",
            r"%ProgramFiles%\claude\claude.exe",
            r"%ProgramFiles(x86)%\claude\claude.exe",
            r"~\AppData\Local\Programs\claude\claude.exe",
            r"~\AppData\Roaming\npm\claude.cmd",
            r"~\AppData\Roaming\npm\claude",
        ];
        for path in candidates {
            let expanded = expand_env(path);
            if std::path::Path::new(&expanded).is_file() {
                return Ok(Some(expanded));
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        // macOS fallback 路径
        let candidates = vec![
            "/opt/homebrew/bin/claude",
            "/usr/local/bin/claude",
            "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js",
            "/usr/local/lib/node_modules/@anthropic-ai/claude-code/cli.js",
        ];
        for path in candidates {
            if std::path::Path::new(path).is_file() {
                return Ok(Some(path.to_string()));
            }
        }
        // 检查 ~/.npm-global/bin/claude
        if let Some(home) = dirs::home_dir() {
            let npm_global = home.join(".npm-global").join("bin").join("claude");
            if npm_global.is_file() {
                return Ok(Some(npm_global.to_string_lossy().to_string()));
            }
        }
    }

    Ok(None)
}
```

**`launch_claude`** 改造（第 43-75 行）：

```rust
#[tauri::command]
pub fn launch_claude(
    exe: String,
    env_vars: HashMap<String, String>,
    args: Vec<String>,
    cwd: Option<String>,
) -> Result<(), String> {
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(&args);

    let mut env: HashMap<String, String> = std::env::vars().collect();
    for (k, v) in &env_vars {
        env.insert(k.clone(), v.clone());
    }
    cmd.envs(&env);

    if let Some(dir) = &cwd {
        if !dir.is_empty() {
            cmd.current_dir(dir);
        }
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x00000010;
        cmd.creation_flags(CREATE_NEW_CONSOLE);
    }

    #[cfg(target_os = "macos")]
    {
        // macOS 无需特殊 flag，直接 spawn
    }

    cmd.spawn()
        .map_err(|e| format!("Failed to launch claude: {}", e))?;

    Ok(())
}
```

移除 Windows-only 的 `expand_env` 辅助函数（改为平台分支内使用）或保留但改为跨平台实现。

#### 2.7 `utils.rs` —— 目录打开

**文件**: `src-tauri/src/utils.rs`
**行**: 24-45

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
        // macOS 没有等价系统面板；返回提示信息
        Err("macOS 不支持直接打开环境变量面板，请通过终端设置环境变量。".to_string())
    }
}
```

#### 2.8 `pty/mod.rs` —— 进程树清理 + PTY 创建

**文件**: `src-tauri/src/pty/mod.rs`

**`kill_process_tree`**（第 389-403 行）：

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
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        let _ = cmd.output();
    }
    #[cfg(target_os = "macos")]
    {
        // macOS: 使用 kill -TERM 发送到进程组
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &format!("-{}", pid)])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        // 如果进程还在，稍后 SIGKILL
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{}", pid)])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
    }
}
```

**`pty_create` 中 cmd.exe 处理**（第 124-134 行）：

```rust
// 原 Windows 分支处理：
#[cfg(target_os = "windows")]
let exe = if cmd[0].eq_ignore_ascii_case("cmd.exe") || cmd[0].eq_ignore_ascii_case("cmd") {
    env.get("COMSPEC")
        .cloned()
        .or_else(|| std::env::var("COMSPEC").ok())
        .unwrap_or_else(|| "C:\\Windows\\System32\\cmd.exe".to_string())
} else {
    cmd[0].clone()
};

// 保持不变，但在 macOS 上需要额外处理 sh/zsh 检测：
#[cfg(not(target_os = "windows"))]
let exe = {
    if cmd[0] == "cmd.exe" || cmd[0] == "cmd" {
        // 当前端发来 cmd.exe（硬编码）时，替换为 macOS 默认 shell
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
    } else {
        cmd[0].clone()
    }
};
```

#### 2.9 `registry.rs` —— 已有条件编译

**文件**: `src-tauri/src/registry.rs`

已有 `#[cfg(windows)]` 和 `#[cfg(not(windows))]` 分支，无需修改代码。但前端应隐藏 macOS 上的注册表相关 UI。

#### 2.10 路径提示文案修正

全局搜索以下模式并修正：

| 文件 | 原文 | 改为 |
|------|------|------|
| `config_store.rs:22` | `%APPDATA%` | `data directory` (注释) |
| `config_store.rs:23` | `%APPDATA% directory` | `data directory` |
| `persistent_state.rs:17` | `%APPDATA%` | `data directory` |
| `session_manager.rs:25` | `%USERPROFILE%` | `home directory` |
| `settings_manager.rs:25` | `%USERPROFILE% directory` | `home directory` |

---

### 阶段三：前端适配

#### 2.11 默认 Shell 检测

新增 `src/composables/useDefaultShell.ts`：

```typescript
// 检测平台默认 shell
export function getDefaultShell(): string[] {
  // 检测平台
  if (navigator.userAgent.includes('Windows NT') || navigator.userAgent.includes('Windows')) {
    return ['cmd.exe'];
  }
  // macOS / Linux: 使用 SHELL 环境变量或回退到 /bin/zsh
  return ['/bin/zsh'];
}
```

修改以下位置，替换硬编码 `['cmd.exe']`：

| 文件 | 行 | 替换为 |
|------|-----|--------|
| `src/stores/project.ts` | 809 | `getDefaultShell()` |
| `src/stores/project.ts` | 956 | `getDefaultShell()` |
| `src/components/terminal/TerminalManager.vue` | 32 | `getDefaultShell()` |
| `src/components/orchestration/OrchestrationManager.vue` | 500 | `getDefaultShell()` |

#### 2.12 注册表 UI 隐藏

**文件**: `src/components/claude/ConfigEditor.vue`
**行**: 132-136

包裹注册表相关按钮，在非 Windows 平台隐藏：

```vue
<template v-if="isWindows">
  <button class="btn btn-primary" @click="store.applyToRegistry()">应用到环境变量</button>
  <button class="btn btn-secondary" @click="openEnvVars()">打开环境变量</button>
</template>
```

其中 `isWindows` 来自：

```typescript
import { platform } from '@tauri-apps/api/os';
const isWindows = ref(false);
platform().then(p => { isWindows.value = p === 'win32'; });
```

#### 2.13 环境变量应用 UI 适配

**文件**: `src/stores/claude.ts`
**行**: 210-247

`applyToRegistry()` 在 macOS 应禁用/提示不可用：

```typescript
async function applyToRegistry() {
  // 非 Windows 平台提示不可用
  const osPlatform = await platform();
  if (osPlatform !== 'win32') {
    statusMessage.value = '环境变量注册表功能仅支持 Windows 平台';
    return;
  }
  // ... 原有逻辑 ...
}
```

---

### 阶段四：Cargo 依赖更新

#### 2.14 `Cargo.toml` —— 移除/隔离 Windows 依赖

**文件**: `src-tauri/Cargo.toml`

保持 `[target.'cfg(windows)'.dependencies]` 不变（条件编译已在 rustc 层面处理）。
macOS 构建时不会编译 `winreg` 和 `windows` crate。

需要确认 macOS 构建无额外依赖缺失。当前最小依赖列表：

```toml
[dependencies]
tauri = { version = "2", features = [] }
tauri-plugin-dialog = "2"
tauri-plugin-shell = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
portable-pty = "0.8"          # ✅ 跨平台
toml_edit = "0.22"
url = "2"
dirs = "5"                    # ✅ 跨平台
uuid = { version = "1", features = ["v4"] }
base64 = "0.22"
which = "7"                   # ✅ 跨平台
sha2 = "0.10"
chrono = { version = "0.4", features = ["serde"] }
notify = "6"                  # ✅ 跨平台
```

**注意**: `portable-pty` 在 macOS 上依赖系统 PTY 设备（`/dev/ptmx`），需要确保 macOS 编译环境有正确的系统头文件（Xcode CLT 已包含）。

---

## 3. 编译与验证

### 3.1 分步验证流程

```bash
# 1. 环境检查
rustup show
xcode-select -p
node --version

# 2. 安装依赖
npm install

# 3. 前端构建（验证 JS/TS 编译）
npm run build

# 4. Rust 语法检查
cd src-tauri
cargo check --target aarch64-apple-darwin

# 5. 完整构建
cd ..
npm run tauri build -- --target aarch64-apple-darwin --bundles app,dmg
```

### 3.2 预期编译问题清单

| 问题 | 原因 | 修复参考 |
|------|------|----------|
| `windows_subsystem` 不支持 macOS | `main.rs:2` | 加 `cfg(target_os = "windows")` |
| `std::os::windows::process::CommandExt` 找不到 | `claude_launcher.rs:50` | 放入 `#[cfg(windows)]` 块 |
| `HWND` 类型未定义 | `lib.rs:15` | lib.rs 的 theme 模块已有 `#[cfg(target_os = "windows")]` |
| `winreg` / `windows` crate 解析失败 | `Cargo.toml:34-41` | 已条件编译，无影响 |
| `taskkill` 命令找不到 | `pty/mod.rs:392` | 放入 `#[cfg(windows)]` 块 |
| `explorer` / `rundll32` 找不到 | `utils.rs` | 放入 `#[cfg(windows)]` 块 |

### 3.3 功能验证清单

| 功能 | 验证方法 |
|------|----------|
| 应用启动 | 双击 `.app` 或从终端运行 |
| 无边框窗口 | 标题栏、窗口按钮、拖动正常 |
| 配置读写 | 新增/编辑/删除配置，关闭重开后持久化 |
| 项目管理 | 添加项目目录、查看文件树 |
| 内置终端 | 启动默认 shell（zsh）、输入命令 |
| Claude 查找 | 检查 `which claude` + fallback 路径 |
| Claude 启动 | 从配置页启动 Claude CLI |
| 历史刷新 | 监听 `~/.claude/history.jsonl` |
| 目录打开 | 右键打开文件所在目录 |
| 环境变量配置 | 编辑保存 ANTHROPIC_BASE_URL 等 |
| 跨标签通信 | `tab-send`, `tab-list`, `tab-read` |
| 快照/预设 | 保存和恢复终端布局 |

---

## 4. 架构决策记录

### 4.1 平台检测策略

| 层级 | 机制 | 用途 |
|------|------|------|
| Rust 编译期 | `#[cfg(target_os = "...")]` | 条件编译（首选） |
| Rust 运行时 | `std::env::consts::OS` | 运行时分支选择 |
| 前端 | `@tauri-apps/api/os` 的 `platform()` | UI 适配 |

### 4.2 改造原则

1. **不破坏 Windows 构建**：所有平台差异用 `#[cfg()]` 隔离
2. **前端优先使用能力检测而非平台检测**：如 `open_directory` 只需告诉后端"打开"，后端处理平台差异
3. **硬编码 `cmd.exe` 必须消除**：前端通过 `getDefaultShell()` 统一获取
4. **注册表相关 UI 在 macOS 隐藏**：但不删除代码，保留未来可能性

---

## 5. 非功能性需求

### 5.1 代码签名与公证

首次适配阶段可跳过签名，确保应用能编译和运行后再接入：

```bash
# 签名（需要 Apple Developer 证书）
codesign --deep --force --verify --verbose \
  --options runtime \
  --sign "Developer ID Application: Your Name (TEAMID)" \
  src-tauri/target/release/bundle/macos/ClaudeCode启动器.app

# 公证
xcrun notarytool submit \
  --apple-id your@email.com \
  --team-id TEAMID \
  --password @keychain:AC_PASSWORD \
  --wait \
  src-tauri/target/release/bundle/dmg/ClaudeCode启动器_1.0.0_aarch64.dmg
```

### 5.2 自动更新

当前无自动更新机制。macOS 版本如需自动更新，建议使用 Tauri 的 `updater` 插件 + 自建更新服务器或 GitHub Releases。

---

## 6. 实施顺序（全部完成）

```
✅ 第 1 步: 环境准备 + Cargo.toml 确认
✅ 第 2 步: Rust 编译错误修复 (main.rs → lib.rs → claude_launcher.rs → utils.rs → pty/mod.rs)
✅ 第 3 步: tauri.macos.conf.json + app.icns → tauri build 通过
✅ 第 4 步: 前端适配 (默认 shell → 注册表 UI → 环境变量 → 路径兼容)
✅ 第 5 步: 功能验证 (内置终端、Claude 启动、配置应用)
✅ 第 6 步: Bug 修复 (PTY PATH、emoji 渲染、APPDATA 文案)
```
