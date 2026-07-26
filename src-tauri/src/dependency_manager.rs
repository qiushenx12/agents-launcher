use serde::Serialize;
use std::ffi::OsStr;
use std::process::Command;

const MIN_NODE_MAJOR: u64 = 18;

#[derive(Clone, Copy)]
enum DependencyKind {
    Node,
    Git,
}

impl DependencyKind {
    fn key(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Git => "git",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Node => "Node.js",
            Self::Git => "Git",
        }
    }

    fn executable(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Git => "git",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyStatus {
    Installed,
    Missing,
    Unsupported,
    Error,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyCheckResult {
    pub dependency: &'static str,
    pub status: DependencyStatus,
    pub path: Option<String>,
    pub version: Option<String>,
    pub message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyInstallResult {
    pub dependency: String,
    pub display_name: String,
    pub message: String,
}

#[tauri::command]
pub async fn check_node_dependency() -> DependencyCheckResult {
    check_dependency(DependencyKind::Node).await
}

#[tauri::command]
pub async fn check_git_dependency() -> DependencyCheckResult {
    check_dependency(DependencyKind::Git).await
}

#[tauri::command]
pub async fn install_dependency_via_winget(
    dependency: String,
) -> Result<DependencyInstallResult, String> {
    #[cfg(windows)]
    {
        let (package_id, display_name) = match dependency.as_str() {
            "node" => ("OpenJS.NodeJS.LTS", "Node.js LTS"),
            "git" => ("Git.Git", "Git"),
            _ => return Err("不支持安装此依赖。".to_string()),
        };

        tokio::task::spawn_blocking(move || {
            run_winget_install(&dependency, package_id, display_name)
        })
        .await
        .map_err(|error| format!("安装任务异常结束: {error}"))?
    }
    #[cfg(not(windows))]
    {
        let _ = dependency;
        Err("依赖安装仅支持 Windows（winget）。macOS 请使用 Homebrew 或官网安装。".to_string())
    }
}

async fn check_dependency(kind: DependencyKind) -> DependencyCheckResult {
    tokio::task::spawn_blocking(move || inspect_dependency(kind))
        .await
        .unwrap_or_else(|error| DependencyCheckResult {
            dependency: kind.key(),
            status: DependencyStatus::Error,
            path: None,
            version: None,
            message: format!("{} 检测任务异常结束: {error}", kind.display_name()),
        })
}

fn inspect_dependency(kind: DependencyKind) -> DependencyCheckResult {
    let path = match crate::platform_env::locate_executable(kind.executable()) {
        Some(path) => path,
        None => {
            return DependencyCheckResult {
                dependency: kind.key(),
                status: DependencyStatus::Missing,
                path: None,
                version: None,
                message: format!("未检测到 {}。", kind.display_name()),
            };
        }
    };

    let output = match hidden_command(&path).arg("--version").output() {
        Ok(output) => output,
        Err(error) => {
            return DependencyCheckResult {
                dependency: kind.key(),
                status: DependencyStatus::Error,
                path: Some(path.to_string_lossy().to_string()),
                version: None,
                message: format!("无法执行 {} 版本检测: {error}", kind.display_name()),
            };
        }
    };

    if !output.status.success() {
        return DependencyCheckResult {
            dependency: kind.key(),
            status: DependencyStatus::Error,
            path: Some(path.to_string_lossy().to_string()),
            version: None,
            message: format!("{} 无法正常执行版本检测。", kind.display_name()),
        };
    }

    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version.is_empty() {
        return DependencyCheckResult {
            dependency: kind.key(),
            status: DependencyStatus::Error,
            path: Some(path.to_string_lossy().to_string()),
            version: None,
            message: format!("{} 未返回可识别的版本信息。", kind.display_name()),
        };
    }

    if matches!(kind, DependencyKind::Node) {
        match parse_node_major_version(&version) {
            Some(major) if major >= MIN_NODE_MAJOR => {}
            Some(_) => {
                return DependencyCheckResult {
                    dependency: kind.key(),
                    status: DependencyStatus::Unsupported,
                    path: Some(path.to_string_lossy().to_string()),
                    version: Some(version),
                    message: format!(
                        "检测到 Node.js 版本低于要求。Claude Code 需要 Node.js {MIN_NODE_MAJOR}+。"
                    ),
                };
            }
            None => {
                return DependencyCheckResult {
                    dependency: kind.key(),
                    status: DependencyStatus::Error,
                    path: Some(path.to_string_lossy().to_string()),
                    version: Some(version),
                    message: "无法识别 Node.js 版本信息。".to_string(),
                };
            }
        }
    }

    DependencyCheckResult {
        dependency: kind.key(),
        status: DependencyStatus::Installed,
        path: Some(path.to_string_lossy().to_string()),
        version: Some(version),
        message: format!("{} 已就绪。", kind.display_name()),
    }
}

#[cfg(windows)]
fn run_winget_install(
    dependency: &str,
    package_id: &str,
    display_name: &str,
) -> Result<DependencyInstallResult, String> {
    let winget_check = hidden_command("winget")
        .arg("--version")
        .output()
        .map_err(|_| "未检测到 winget。请使用官网下载并安装所需依赖。".to_string())?;

    if !winget_check.status.success() {
        return Err("winget 无法正常运行。请使用官网下载并安装所需依赖。".to_string());
    }

    let output = hidden_command("winget")
        .args([
            "install",
            "--id",
            package_id,
            "--exact",
            "--source",
            "winget",
            "--accept-package-agreements",
            "--accept-source-agreements",
            "--disable-interactivity",
        ])
        .output()
        .map_err(|error| format!("无法启动 winget: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "{display_name} 安装失败。{}",
            winget_error_message(&output.stdout, &output.stderr)
        ));
    }

    Ok(DependencyInstallResult {
        dependency: dependency.to_string(),
        display_name: display_name.to_string(),
        message: format!("{display_name} 已安装完成。请关闭并重新打开应用以继续。"),
    })
}

#[allow(unused_mut)]
fn hidden_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    crate::platform_env::apply_effective_path(&mut command);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn parse_node_major_version(version: &str) -> Option<u64> {
    version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

#[cfg(windows)]
fn winget_error_message(stdout: &[u8], stderr: &[u8]) -> String {
    let output = if stderr.is_empty() { stdout } else { stderr };
    let message = String::from_utf8_lossy(output)
        .trim()
        .replace(['\r', '\n'], " ");
    if message.is_empty() {
        "请检查网络、权限或安装程序日志后重试。".to_string()
    } else {
        let preview: String = message.chars().take(500).collect();
        format!("{preview}")
    }
}

#[cfg(test)]
mod tests {
    use super::parse_node_major_version;

    #[test]
    fn parses_node_major_version() {
        assert_eq!(parse_node_major_version("v18.20.8"), Some(18));
        assert_eq!(parse_node_major_version("24.13.0"), Some(24));
        assert_eq!(parse_node_major_version("not-a-version"), None);
    }
}
