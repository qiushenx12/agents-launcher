use std::collections::HashMap;
use std::ffi::OsStr;
use std::process::Command;

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCodeCheckResult {
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCodeInstallResult {
    pub message: String,
}

#[tauri::command]
pub fn find_claude_executable() -> Result<Option<String>, String> {
    Ok(locate_claude_executable())
}

#[tauri::command]
pub async fn check_claude_code_installed() -> ClaudeCodeCheckResult {
    tokio::task::spawn_blocking(inspect_claude_code)
        .await
        .unwrap_or_else(|error| ClaudeCodeCheckResult {
            installed: false,
            path: None,
            version: None,
            message: format!("Claude Code 检测任务异常结束: {error}"),
        })
}

#[tauri::command]
pub async fn install_claude_code_via_npm() -> Result<ClaudeCodeInstallResult, String> {
    tokio::task::spawn_blocking(run_npm_install)
        .await
        .map_err(|error| format!("Claude Code 安装任务异常结束: {error}"))?
}

pub(crate) fn locate_claude_executable() -> Option<String> {
    // Try PATH first
    if let Ok(path) = which::which("claude") {
        return Some(path.to_string_lossy().to_string());
    }

    // Platform-specific fallback paths
    #[cfg(target_os = "windows")]
    {
        let candidates = vec![
            expand_env(r"%LOCALAPPDATA%\Programs\claude\claude.exe"),
            expand_env(r"%LOCALAPPDATA%\claude\claude.exe"),
            expand_env(r"%ProgramFiles%\claude\claude.exe"),
            expand_env(r"%ProgramFiles(x86)%\claude\claude.exe"),
            expand_home("~/AppData/Local/Programs/claude/claude.exe"),
            expand_home("~/AppData/Roaming/npm/claude.cmd"),
            expand_home("~/AppData/Roaming/npm/claude"),
        ];

        for path in candidates {
            if std::path::Path::new(&path).is_file() {
                return Some(path);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let candidates = vec![
            "/opt/homebrew/bin/claude".to_string(),
            "/usr/local/bin/claude".to_string(),
        ];
        for path in &candidates {
            if std::path::Path::new(path).is_file() {
                return Some(path.clone());
            }
        }
        if let Some(home) = dirs::home_dir() {
            let npm_global = home.join(".npm-global").join("bin").join("claude");
            if npm_global.is_file() {
                return Some(npm_global.to_string_lossy().to_string());
            }
        }
    }

    None
}

fn inspect_claude_code() -> ClaudeCodeCheckResult {
    let Some(path) = locate_claude_executable() else {
        return ClaudeCodeCheckResult {
            installed: false,
            path: None,
            version: None,
            message: "未检测到 Claude Code。".to_string(),
        };
    };

    let output = match hidden_command(&path).arg("--version").output() {
        Ok(output) => output,
        Err(error) => {
            return ClaudeCodeCheckResult {
                installed: false,
                path: Some(path),
                version: None,
                message: format!("无法执行 Claude Code 版本检测: {error}"),
            };
        }
    };

    if !output.status.success() {
        return ClaudeCodeCheckResult {
            installed: false,
            path: Some(path),
            version: None,
            message: "Claude Code 无法正常执行版本检测。".to_string(),
        };
    }

    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    ClaudeCodeCheckResult {
        installed: true,
        path: Some(path),
        version: (!version.is_empty()).then_some(version),
        message: "Claude Code 已就绪。".to_string(),
    }
}

fn hidden_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn run_npm_install() -> Result<ClaudeCodeInstallResult, String> {
    let npm = which::which("npm")
        .or_else(|_| which::which("npm.cmd"))
        .map_err(|_| "未检测到 npm。请确认 Node.js 安装完整后重试。".to_string())?;

    let output = hidden_command(&npm)
        .args(["install", "-g", "@anthropic-ai/claude-code"])
        .output()
        .map_err(|error| format!("无法启动 npm: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "Claude Code 安装失败。{}",
            npm_error_message(&output.stdout, &output.stderr)
        ));
    }

    Ok(ClaudeCodeInstallResult {
        message: "Claude Code 已安装完成，请点击“重新检测”继续。".to_string(),
    })
}

fn npm_error_message(stdout: &[u8], stderr: &[u8]) -> String {
    let output = if stderr.is_empty() { stdout } else { stderr };
    let message = String::from_utf8_lossy(output)
        .trim()
        .replace(['\r', '\n'], " ");
    if message.is_empty() {
        "请检查网络、npm 权限或安装日志后重试。".to_string()
    } else {
        message.chars().take(500).collect()
    }
}

fn expand_env(template: &str) -> String {
    let mut result = template.to_string();
    for (key, val) in std::env::vars() {
        result = result.replace(&format!("%{}%", key), &val);
    }
    result
}

fn expand_home(template: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy().to_string();
        template.replacen('~', &home_str, 1)
    } else {
        template.to_string()
    }
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

    // Build env: start from current env, overlay with provided vars
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

    cmd.spawn()
        .map_err(|e| format!("Failed to launch claude: {}", e))?;

    Ok(())
}
