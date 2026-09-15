use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cli_capabilities::{
    parse_opencode_projects, parse_opencode_sessions, OpenCodeProject, OpenCodeSession,
};
use crate::cli_contract::{
    load_cli_contract, status_for_issue, CliIssueCode, CliKind, CliStatus, CliStatusState,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeDiscoveredProject {
    pub id: String,
    pub worktree: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeProjectDiscovery {
    pub projects: Vec<OpenCodeDiscoveredProject>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadSummary {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub preview: String,
    #[serde(default)]
    pub model_provider: Option<String>,
    pub cwd: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadList {
    pub threads: Vec<CodexThreadSummary>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexDiscoveredProject {
    pub worktree: String,
    pub updated_at: i64,
    pub session_count: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexProjectDiscovery {
    pub projects: Vec<CodexDiscoveredProject>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexWorkspaceSnapshot {
    pub discovery: CodexProjectDiscovery,
    pub threads: CodexThreadList,
}

#[derive(Debug, Deserialize)]
struct CodexSessionMetaEnvelope {
    #[serde(rename = "type")]
    kind: String,
    payload: CodexSessionMeta,
}

#[derive(Debug, Clone, Deserialize)]
struct CodexSessionMeta {
    #[serde(default)]
    id: Option<String>,
    cwd: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    model_provider: Option<String>,
}

#[derive(Debug, Clone)]
struct CodexRolloutRecord {
    path: PathBuf,
    meta: CodexSessionMeta,
    updated_at: i64,
    revision: Option<CodexRolloutFileRevision>,
}

#[derive(Debug, Clone)]
struct CodexRolloutIndex {
    records: Vec<CodexRolloutRecord>,
    skipped: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexRolloutFileRevision {
    len: u64,
    modified: Option<SystemTime>,
}

#[derive(Debug, Clone)]
struct CodexRolloutFileCacheEntry {
    revision: CodexRolloutFileRevision,
    meta: Option<CodexSessionMeta>,
    preview: Option<String>,
}

static CODEX_ROLLOUT_FILE_CACHE: OnceLock<
    Mutex<HashMap<PathBuf, CodexRolloutFileCacheEntry>>,
> = OnceLock::new();

fn codex_rollout_file_cache(
) -> &'static Mutex<HashMap<PathBuf, CodexRolloutFileCacheEntry>> {
    CODEX_ROLLOUT_FILE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Deserialize)]
struct CodexThreadListResponse {
    data: Vec<CodexThreadSummary>,
    #[serde(default, rename = "nextCursor", alias = "next_cursor")]
    next_cursor: Option<String>,
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

/// Human-facing name of a CLI. For dsh this is the package bin name, which is
/// **not** guaranteed to be on `PATH`; use [`cli_invocation`] to actually run it.
fn command_name(kind: CliKind) -> &'static str {
    match kind {
        CliKind::Claude => "claude",
        CliKind::Codex => "codex",
        CliKind::Opencode => "opencode",
        CliKind::Dsh => "dsh",
    }
}

/// Executable plus arguments used to run a CLI.
///
/// Three CLIs are single executables. dsh is distributed as the npm package
/// `@deepseek-ai/dsh` whose bin name is `dsh`; unless the user globally
/// installed it, `dsh` is not on `PATH`, so it must run through `npx`.
fn cli_invocation(kind: CliKind) -> Vec<String> {
    match kind {
        CliKind::Dsh => vec![
            "npx".to_string(),
            "--yes".to_string(),
            "@deepseek-ai/dsh".to_string(),
        ],
        other => vec![command_name(other).to_string()],
    }
}

/// The program `PATH` must contain for a CLI to be usable.
fn cli_launcher_name(kind: CliKind) -> &'static str {
    match kind {
        CliKind::Dsh => "npx",
        other => command_name(other),
    }
}

fn label(kind: CliKind) -> Result<String, String> {
    load_cli_contract()?
        .cli_descriptors
        .into_iter()
        .find(|descriptor| descriptor.kind == kind)
        .map(|descriptor| descriptor.label)
        .ok_or_else(|| format!("CLI 契约缺少 {kind:?} 描述"))
}

pub fn locate_cli(kind: CliKind) -> Option<PathBuf> {
    if kind == CliKind::Claude {
        if let Some(path) = crate::claude_launcher::locate_claude_executable() {
            return Some(PathBuf::from(path));
        }
    }
    crate::platform_env::locate_executable(cli_launcher_name(kind))
}

fn inspect_cli(kind: CliKind) -> CliStatus {
    // A running supervised dsh service is definitive proof that the CLI works
    // and already knows its version: entering the dsh workspace while the
    // service is up must not pay for the seconds-long npx probe below.
    if kind == CliKind::Dsh {
        if let Some((version, executable)) = crate::dsh_runtime::running_supervised_proof() {
            return CliStatus {
                kind,
                state: CliStatusState::Ready,
                issue_code: None,
                message: format!(
                    "{} 服务正在运行，已跳过版本探测。",
                    label(kind).unwrap_or_else(|_| command_name(kind).to_string())
                ),
                executable_path: Some(executable),
                version,
            };
        }
    }

    let Some(path) = locate_cli(kind) else {
        return status_for_issue(kind, CliIssueCode::ExecutableMissing).unwrap_or(CliStatus {
            kind,
            state: CliStatusState::Blocked,
            issue_code: Some(CliIssueCode::ExecutableMissing),
            message: format!("未检测到 {}。", command_name(kind)),
            executable_path: None,
            version: None,
        });
    };

    let invocation = cli_invocation(kind);
    let mut probe = hidden_command(&path);
    // dsh must be probed through npx, which resolves the npm dist-tag over the
    // network and is far slower than a local `--version`.
    if invocation.len() > 1 {
        probe.args(&invocation[1..]);
    }
    let version_flag = if kind == CliKind::Dsh { "-V" } else { "--version" };
    let output = match probe.arg(version_flag).output() {
        Ok(output) => output,
        Err(error) => {
            let mut status =
                status_for_issue(kind, CliIssueCode::VersionCommandFailed).unwrap_or(CliStatus {
                    kind,
                    state: CliStatusState::Blocked,
                    issue_code: Some(CliIssueCode::VersionCommandFailed),
                    message: format!("版本命令执行失败: {error}"),
                    executable_path: None,
                    version: None,
                });
            status.executable_path = Some(path.to_string_lossy().to_string());
            return status;
        }
    };

    if !output.status.success() {
        let mut status =
            status_for_issue(kind, CliIssueCode::VersionCommandFailed).unwrap_or(CliStatus {
                kind,
                state: CliStatusState::Blocked,
                issue_code: Some(CliIssueCode::VersionCommandFailed),
                message: "版本命令返回失败状态。".to_string(),
                executable_path: None,
                version: None,
            });
        status.executable_path = Some(path.to_string_lossy().to_string());
        return status;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let version = if stdout.is_empty() { stderr } else { stdout };
    let cli_label = label(kind).unwrap_or_else(|_| command_name(kind).to_string());

    if kind == CliKind::Codex && !codex_supports_phase_b(&path) {
        let mut status = status_for_issue(kind, CliIssueCode::VersionTooOld).unwrap_or(CliStatus {
            kind,
            state: CliStatusState::Blocked,
            issue_code: Some(CliIssueCode::VersionTooOld),
            message: "当前 CodeX 不支持项目目录或原生恢复能力。".to_string(),
            executable_path: None,
            version: None,
        });
        status.message = "CodeX 已通过版本检测，但未通过 -C / resume 帮助能力探测；工作区启动已停用。请升级或修复 Codex CLI 后重新检测。".to_string();
        status.executable_path = Some(path.to_string_lossy().to_string());
        status.version = (!version.is_empty()).then_some(version);
        return status;
    }

    CliStatus {
        kind,
        state: CliStatusState::Ready,
        issue_code: None,
        message: format!("{cli_label} 已就绪。"),
        executable_path: Some(path.to_string_lossy().to_string()),
        version: (!version.is_empty()).then_some(version),
    }
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn codex_supports_phase_b(path: &Path) -> bool {
    let Ok(help) = hidden_command(path).arg("--help").output() else {
        return false;
    };
    if !help.status.success() {
        return false;
    }
    let help_text = output_text(&help);
    if !codex_help_has_workspace_capabilities(&help_text) {
        return false;
    }

    hidden_command(path)
        .args(["resume", "--help"])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn codex_help_has_workspace_capabilities(help: &str) -> bool {
    help.contains("-C") && help.to_ascii_lowercase().contains("resume")
}

fn run_cli_output(kind: CliKind, args: &[&str], cwd: Option<&Path>) -> Result<Output, String> {
    let path = locate_cli(kind).ok_or_else(|| {
        status_for_issue(kind, CliIssueCode::ExecutableMissing)
            .map(|status| status.message)
            .unwrap_or_else(|_| format!("未检测到 {}。", command_name(kind)))
    })?;
    let mut command = hidden_command(&path);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
        .output()
        .map_err(|error| format!("无法执行 {}: {error}", command_name(kind)))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if message.is_empty() {
            format!("{} 命令返回失败状态。", command_name(kind))
        } else {
            message.chars().take(1000).collect()
        });
    }
    Ok(output)
}

#[tauri::command]
pub async fn check_cli(kind: CliKind) -> CliStatus {
    tokio::task::spawn_blocking(move || inspect_cli(kind))
        .await
        .unwrap_or_else(|error| CliStatus {
            kind,
            state: CliStatusState::Blocked,
            issue_code: Some(CliIssueCode::VersionCommandFailed),
            message: format!("CLI 检测任务异常结束: {error}"),
            executable_path: None,
            version: None,
        })
}

const CODEX_THREADS_CACHE_TTL: Duration = Duration::from_secs(20);

struct CodexThreadsCacheEntry {
    fetched_at: Instant,
    cache_key: String,
    threads: Vec<CodexThreadSummary>,
    complete: bool,
}

static CODEX_THREADS_CACHE: Mutex<Option<CodexThreadsCacheEntry>> = Mutex::new(None);

fn all_codex_threads(
    max_count: u32,
    force: bool,
    profile_id: Option<String>,
) -> Result<CodexThreadList, String> {
    let runtime = crate::codex_config::resolve_codex_runtime_context(profile_id.as_deref())?;
    let mut guard = CODEX_THREADS_CACHE
        .lock()
        .map_err(|_| "CodeX 会话缓存锁定失败。".to_string())?;
    if !force {
        if let Some(entry) = guard.as_ref() {
            if entry.cache_key == runtime.cache_key
                && entry.fetched_at.elapsed() < CODEX_THREADS_CACHE_TTL
            {
                return Ok(CodexThreadList {
                    threads: entry.threads.clone(),
                    complete: entry.complete,
                });
            }
        }
    }
    let result = match query_all_codex_threads(max_count, &runtime) {
        Ok(app_server) => {
            let rollout = codex_threads_from_rollouts(max_count, &runtime).ok();
            let threads = merge_codex_thread_lists(
                app_server.threads,
                rollout.as_ref().map(|result| result.threads.as_slice()),
                max_count,
            );
            let complete = rollout
                .as_ref()
                .map(|result| result.complete)
                // Without the rollout scan we cannot prove that the shared
                // app-server result covers every provider, even when its own
                // pagination is complete.
                .unwrap_or(false);
            CodexThreadList { threads, complete }
        }
        Err(_) => codex_threads_from_rollouts(max_count, &runtime)?,
    };
    *guard = Some(CodexThreadsCacheEntry {
        fetched_at: Instant::now(),
        cache_key: runtime.cache_key,
        threads: result.threads.clone(),
        complete: result.complete,
    });
    Ok(result)
}

#[tauri::command]
pub async fn list_all_codex_threads(
    max_count: Option<u32>,
    force: Option<bool>,
    profile_id: Option<String>,
) -> Result<CodexThreadList, String> {
    tokio::task::spawn_blocking(move || {
        all_codex_threads(
            max_count.unwrap_or(500),
            force.unwrap_or(false),
            profile_id,
        )
    })
    .await
    .map_err(|error| format!("CodeX 会话读取任务异常结束: {error}"))?
}

#[tauri::command]
pub async fn list_codex_threads(
    project_path: String,
    max_count: Option<u32>,
    force: Option<bool>,
    profile_id: Option<String>,
) -> Result<CodexThreadList, String> {
    tokio::task::spawn_blocking(move || {
        let cwd = PathBuf::from(&project_path);
        if !cwd.is_dir() {
            return Err(format!("CodeX 项目目录不存在: {project_path}"));
        }
        let max_count = max_count.unwrap_or(100);
        let normalized_target = normalize_path(&project_path);
        match all_codex_threads(max_count, force.unwrap_or(false), profile_id.clone()) {
            Ok(mut threads) => {
                threads
                    .threads
                    .retain(|thread| normalize_path(&thread.cwd) == normalized_target);
                Ok(threads)
            }
            Err(_) => {
                let runtime = crate::codex_config::resolve_codex_runtime_context(
                    profile_id.as_deref(),
                )?;
                let mut threads = query_codex_threads(&cwd, max_count, &runtime)?;
                threads
                    .threads
                    .retain(|thread| normalize_path(&thread.cwd) == normalized_target);
                Ok(threads)
            }
        }
    })
    .await
    .map_err(|error| format!("CodeX 会话读取任务异常结束: {error}"))?
}

#[tauri::command]
pub async fn discover_codex_projects(
    profile_id: Option<String>,
) -> Result<CodexProjectDiscovery, String> {
    tokio::task::spawn_blocking(move || {
        let runtime = crate::codex_config::resolve_codex_runtime_context(profile_id.as_deref())?;
        discover_codex_projects_from_session_meta(&runtime)
    })
    .await
    .map_err(|error| format!("CodeX 项目发现任务异常结束: {error}"))?
}

#[tauri::command]
pub async fn load_codex_workspace(
    max_count: Option<u32>,
    profile_id: Option<String>,
) -> Result<CodexWorkspaceSnapshot, String> {
    tokio::task::spawn_blocking(move || {
        let runtime = crate::codex_config::resolve_codex_runtime_context(profile_id.as_deref())?;
        let sessions_root = codex_sessions_root(&runtime)
            .ok_or_else(|| "无法确定 CodeX 数据目录。".to_string())?;
        let index = sessions_root
            .is_dir()
            .then(|| build_codex_rollout_index(&sessions_root));
        let discovery = index
            .as_ref()
            .map(discover_codex_projects_from_rollout_index)
            .unwrap_or_else(|| CodexProjectDiscovery {
                projects: Vec::new(),
                warning: Some(format!(
                    "未找到 CodeX 会话目录: {}",
                    sessions_root.display()
                )),
            });
        let max_count = max_count.unwrap_or(500);
        let app_server = query_all_codex_threads(max_count, &runtime);
        let rollout = index
            .as_ref()
            .map(|index| codex_threads_from_rollout_index(index, max_count));
        let threads = match app_server {
            Ok(app_server) => {
                let merged = merge_codex_thread_lists(
                    app_server.threads,
                    rollout.as_ref().map(|(threads, _)| threads.as_slice()),
                    max_count,
                );
                CodexThreadList {
                    threads: merged,
                    complete: rollout.as_ref().is_some_and(|(_, complete)| *complete),
                }
            }
            Err(_) => {
                let Some((threads, complete)) = rollout else {
                    return Err(format!(
                        "未找到 CodeX 会话目录: {}",
                        sessions_root.display()
                    ));
                };
                CodexThreadList { threads, complete }
            }
        };
        Ok(CodexWorkspaceSnapshot { discovery, threads })
    })
    .await
    .map_err(|error| format!("CodeX 工作区读取任务异常结束: {error}"))?
}

fn discover_codex_projects_from_session_meta(
    runtime: &crate::codex_config::CodexRuntimeContext,
) -> Result<CodexProjectDiscovery, String> {
    let sessions_root = codex_sessions_root(runtime)
        .ok_or_else(|| "无法确定 CodeX 数据目录。".to_string())?;
    if !sessions_root.is_dir() {
        return Ok(CodexProjectDiscovery {
            projects: Vec::new(),
            warning: Some(format!(
                "未找到 CodeX 会话目录: {}",
                sessions_root.display()
            )),
        });
    }

    Ok(discover_codex_projects_from_rollout_index(
        &build_codex_rollout_index(&sessions_root),
    ))
}

fn discover_codex_projects_from_rollout_index(
    index: &CodexRolloutIndex,
) -> CodexProjectDiscovery {
    let mut skipped = index.skipped;
    let mut by_path: HashMap<String, CodexDiscoveredProject> = HashMap::new();
    let mut validated_worktrees: HashMap<String, bool> = HashMap::new();

    for record in &index.records {
        let worktree = clean_codex_worktree(&record.meta.cwd);
        let worktree_exists = *validated_worktrees
            .entry(worktree.clone())
            .or_insert_with(|| !worktree.is_empty() && Path::new(&worktree).is_dir());
        if !worktree_exists {
            skipped = skipped.saturating_add(1);
            continue;
        }
        let key = normalize_path(&worktree);
        let updated_at = record
            .meta
            .timestamp
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.timestamp_millis())
            .unwrap_or(0);
        let entry = by_path
            .entry(key)
            .or_insert_with(|| CodexDiscoveredProject {
                worktree: worktree.clone(),
                updated_at,
                session_count: 0,
            });
        entry.session_count = entry.session_count.saturating_add(1);
        if updated_at > entry.updated_at {
            entry.updated_at = updated_at;
            entry.worktree = worktree;
        }
    }

    let mut projects: Vec<_> = by_path.into_values().collect();
    projects.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| normalize_path(&left.worktree).cmp(&normalize_path(&right.worktree)))
    });
    let warning = (skipped > 0)
        .then(|| format!("已跳过 {skipped} 个无法读取、格式不兼容或目录已不存在的 CodeX 会话记录"));
    CodexProjectDiscovery { projects, warning }
}

fn build_codex_rollout_index(root: &Path) -> CodexRolloutIndex {
    let mut files = Vec::new();
    let mut skipped = 0_u32;
    collect_codex_jsonl_files(root, &mut files, &mut skipped);
    let mut records = Vec::with_capacity(files.len());
    for path in files {
        let Some(record) = load_codex_rollout_record(&path) else {
            skipped = skipped.saturating_add(1);
            continue;
        };
        records.push(record);
    }
    CodexRolloutIndex { records, skipped }
}

fn codex_rollout_file_revision(path: &Path) -> Option<CodexRolloutFileRevision> {
    let metadata = fs::metadata(path).ok()?;
    Some(CodexRolloutFileRevision {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn load_codex_rollout_record(path: &Path) -> Option<CodexRolloutRecord> {
    let revision = codex_rollout_file_revision(path);
    if let Some(revision) = revision.as_ref() {
        let cache = codex_rollout_file_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entry) = cache
            .get(path)
            .filter(|entry| &entry.revision == revision)
        {
            return entry.meta.clone().map(|meta| CodexRolloutRecord {
                path: path.to_path_buf(),
                updated_at: rollout_updated_at(path, &meta),
                meta,
                revision: Some(revision.clone()),
            });
        }
    }

    let meta = read_codex_session_meta(path);
    if let Some(revision) = revision.as_ref() {
        codex_rollout_file_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                path.to_path_buf(),
                CodexRolloutFileCacheEntry {
                    revision: revision.clone(),
                    meta: meta.clone(),
                    preview: None,
                },
            );
    }
    meta.map(|meta| CodexRolloutRecord {
        path: path.to_path_buf(),
        updated_at: rollout_updated_at(path, &meta),
        meta,
        revision,
    })
}

fn rollout_updated_at(path: &Path, meta: &CodexSessionMeta) -> i64 {
    let created_at = meta
        .timestamp
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp())
        .unwrap_or(0);
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(created_at)
}

fn read_cached_codex_rollout_preview(record: &CodexRolloutRecord) -> String {
    if let Some(revision) = record.revision.as_ref() {
        let cache = codex_rollout_file_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(preview) = cache
            .get(&record.path)
            .filter(|entry| &entry.revision == revision)
            .and_then(|entry| entry.preview.clone())
        {
            return preview;
        }
    }

    let preview = read_codex_rollout_preview(&record.path);
    if let Some(revision) = record.revision.as_ref() {
        let mut cache = codex_rollout_file_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entry) = cache
            .get_mut(&record.path)
            .filter(|entry| &entry.revision == revision)
        {
            entry.preview = Some(preview.clone());
        }
    }
    preview
}

fn collect_codex_jsonl_files(root: &Path, files: &mut Vec<PathBuf>, skipped: &mut u32) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => {
            *skipped = skipped.saturating_add(1);
            return;
        }
    };
    for entry in entries {
        let Ok(entry) = entry else {
            *skipped = skipped.saturating_add(1);
            continue;
        };
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            *skipped = skipped.saturating_add(1);
            continue;
        };
        if file_type.is_dir() {
            collect_codex_jsonl_files(&path, files, skipped);
        } else if file_type.is_file() && path.extension().and_then(OsStr::to_str) == Some("jsonl") {
            files.push(path);
        }
    }
}

fn read_codex_session_meta(path: &Path) -> Option<CodexSessionMeta> {
    let file = File::open(path).ok()?;
    let first_line = BufReader::new(file).lines().next()?.ok()?;
    parse_codex_session_meta(&first_line)
}

fn parse_codex_session_meta(line: &str) -> Option<CodexSessionMeta> {
    let envelope: CodexSessionMetaEnvelope =
        serde_json::from_str(line.trim_start_matches('\u{feff}')).ok()?;
    (envelope.kind == "session_meta").then_some(envelope.payload)
}

fn clean_codex_worktree(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['\\', '/']);
    if let Some(rest) = trimmed.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        trimmed.strip_prefix(r"\\?\").unwrap_or(trimmed).to_string()
    }
}

fn spawn_codex_app_server(
    cwd: Option<&Path>,
    runtime: &crate::codex_config::CodexRuntimeContext,
) -> Result<Child, String> {
    let path =
        locate_cli(CliKind::Codex).ok_or_else(|| "未检测到 CodeX，无法读取会话。".to_string())?;
    let mut command = hidden_command(&path);
    // All profiles use the same CODEX_HOME. Current Codex releases do not
    // accept --profile for app-server, so the shared home is the source of
    // truth and rollout metadata supplies the cross-provider catalog.
    command
        .args(codex_app_server_args())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command.envs(&runtime.env_vars);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command
        .spawn()
        .map_err(|error| format!("无法启动 CodeX App Server: {error}"))
}

fn codex_app_server_args() -> [&'static str; 1] {
    ["app-server"]
}

fn query_codex_threads(
    cwd: &Path,
    max_count: u32,
    runtime: &crate::codex_config::CodexRuntimeContext,
) -> Result<CodexThreadList, String> {
    let mut child = spawn_codex_app_server(Some(cwd), runtime)?;
    let result = exchange_codex_thread_list(&mut child, Some(cwd), max_count.clamp(1, 500));
    terminate_child(&mut child);
    result
}

fn query_all_codex_threads(
    max_count: u32,
    runtime: &crate::codex_config::CodexRuntimeContext,
) -> Result<CodexThreadList, String> {
    let mut child = spawn_codex_app_server(None, runtime)?;
    let result = exchange_codex_thread_list(&mut child, None, max_count.clamp(1, 2000));
    terminate_child(&mut child);
    result
}

fn codex_threads_from_rollouts(
    max_count: u32,
    runtime: &crate::codex_config::CodexRuntimeContext,
) -> Result<CodexThreadList, String> {
    let sessions_root = codex_sessions_root(runtime)
        .ok_or_else(|| "无法确定 CodeX 数据目录。".to_string())?;
    if !sessions_root.is_dir() {
        return Err(format!(
            "未找到 CodeX 会话目录: {}",
            sessions_root.display()
        ));
    }
    let (threads, complete) = codex_threads_from_rollout_root_with_completeness(
        &sessions_root,
        max_count,
    );
    Ok(CodexThreadList { threads, complete })
}

fn codex_threads_from_rollout_root_with_completeness(
    sessions_root: &Path,
    max_count: u32,
) -> (Vec<CodexThreadSummary>, bool) {
    codex_threads_from_rollout_index(&build_codex_rollout_index(sessions_root), max_count)
}

fn codex_threads_from_rollout_index(
    index: &CodexRolloutIndex,
    max_count: u32,
) -> (Vec<CodexThreadSummary>, bool) {
    let mut threads = Vec::new();
    for indexed_record in &index.records {
        let Some(record) = load_codex_rollout_record(&indexed_record.path) else {
            continue;
        };
        let Some(id) = record.meta.id.as_ref().filter(|id| !id.is_empty()) else {
            continue;
        };
        let worktree = clean_codex_worktree(&record.meta.cwd);
        if worktree.is_empty() {
            continue;
        }
        let created_at = record
            .meta
            .timestamp
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.timestamp())
            .unwrap_or(0);
        threads.push(CodexThreadSummary {
            id: id.clone(),
            name: None,
            preview: read_cached_codex_rollout_preview(&record),
            model_provider: record.meta.model_provider.clone(),
            cwd: worktree,
            created_at,
            updated_at: record.updated_at,
        });
    }
    threads.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    let limit = max_count.clamp(1, 2000) as usize;
    let complete = threads.len() <= limit;
    threads.truncate(limit);
    (threads, complete)
}

fn read_codex_rollout_preview(path: &Path) -> String {
    let Ok(file) = File::open(path) else {
        return String::new();
    };
    for line in BufReader::new(file).lines().take(400) {
        let Ok(line) = line else {
            break;
        };
        if line.len() > 256 * 1024 {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(line.trim_start_matches('\u{feff}')) else {
            continue;
        };
        let Some(payload) = message.get("payload") else {
            continue;
        };
        if payload.get("type").and_then(Value::as_str) != Some("message")
            || payload.get("role").and_then(Value::as_str) != Some("user")
        {
            continue;
        }
        let Some(content) = payload.get("content").and_then(Value::as_array) else {
            continue;
        };
        for part in content {
            let Some(text) = part.get("text").and_then(Value::as_str) else {
                continue;
            };
            let trimmed = text.trim();
            if trimmed.is_empty() || is_codex_instruction_text(trimmed) {
                continue;
            }
            return trimmed.chars().take(200).collect();
        }
    }
    String::new()
}

fn is_codex_instruction_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("# AGENTS.md") || trimmed.starts_with('<')
}

fn enrich_codex_thread_titles(
    mut threads: Vec<CodexThreadSummary>,
    rollout_threads: &[CodexThreadSummary],
) -> Vec<CodexThreadSummary> {
    let by_id = rollout_threads
        .iter()
        .map(|thread| (thread.id.as_str(), thread))
        .collect::<HashMap<_, _>>();

    for thread in &mut threads {
        let Some(rollout) = by_id.get(thread.id.as_str()) else {
            continue;
        };
        if thread
            .model_provider
            .as_deref()
            .is_none_or(|provider| provider.is_empty())
        {
            thread.model_provider = rollout.model_provider.clone();
        }
        if thread
            .name
            .as_deref()
            .is_some_and(is_codex_instruction_text)
        {
            thread.name = None;
        }
        if (thread.preview.trim().is_empty() || is_codex_instruction_text(&thread.preview))
            && !rollout.preview.trim().is_empty()
        {
            thread.preview = rollout.preview.clone();
        }
    }
    threads
}

fn merge_codex_thread_lists(
    app_server_threads: Vec<CodexThreadSummary>,
    rollout_threads: Option<&[CodexThreadSummary]>,
    max_count: u32,
) -> Vec<CodexThreadSummary> {
    let rollout_threads = rollout_threads.unwrap_or(&[]);
    let mut by_id: HashMap<String, CodexThreadSummary> = HashMap::new();

    for thread in rollout_threads {
        match by_id.get(&thread.id) {
            Some(existing) if existing.updated_at >= thread.updated_at => {}
            _ => {
                by_id.insert(thread.id.clone(), thread.clone());
            }
        }
    }

    for thread in enrich_codex_thread_titles(app_server_threads, rollout_threads) {
        by_id.insert(thread.id.clone(), thread);
    }

    let mut merged: Vec<_> = by_id.into_values().collect();
    merged.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    merged.truncate(max_count.clamp(1, 2000) as usize);
    merged
}

fn codex_sessions_root(runtime: &crate::codex_config::CodexRuntimeContext) -> Option<PathBuf> {
    runtime
        .env_vars
        .get("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
        .map(|home| home.join("sessions"))
}

fn exchange_codex_thread_list(
    child: &mut Child,
    cwd: Option<&Path>,
    max_count: u32,
) -> Result<CodexThreadList, String> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "CodeX App Server 未提供标准输出。".to_string())?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "CodeX App Server 未提供标准输入。".to_string())?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    write_codex_app_server_message(
        &mut stdin,
        &json!({
            "method": "initialize",
            "id": 0,
            "params": {
                "clientInfo": {
                    "name": "agents-launcher",
                    "title": "Agents Launcher",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
    )?;
    write_codex_app_server_message(&mut stdin, &json!({ "method": "initialized", "params": {} }))?;

    let limit = max_count.clamp(1, 2000) as usize;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut request_id = 1_i64;
    let mut cursor: Option<String> = None;
    let mut seen_cursors = HashSet::new();
    let mut seen_ids = HashSet::new();
    let mut threads = Vec::new();

    loop {
        let mut params = json!({
            "limit": max_count,
            "archived": false,
            "sourceKinds": ["cli", "vscode", "appServer"],
            "sortKey": "updated_at",
            "sortDirection": "desc"
        });
        if let Some(cursor) = cursor.as_deref() {
            params["cursor"] = Value::String(cursor.to_string());
        }
        if let Some(cwd) = cwd {
            params["cwd"] = Value::String(cwd.to_string_lossy().to_string());
        }

        write_codex_app_server_message(
            &mut stdin,
            &json!({
                "method": "thread/list",
                "id": request_id,
                "params": params
            }),
        )?;
        let response = receive_codex_thread_list_page(
            &receiver,
            request_id,
            deadline,
        )?;
        for thread in response.data {
            if seen_ids.insert(thread.id.clone()) {
                threads.push(thread);
            }
        }

        let next_cursor = response
            .next_cursor
            .filter(|cursor| !cursor.trim().is_empty());
        if threads.len() >= limit {
            threads.truncate(limit);
            return Ok(CodexThreadList {
                threads,
                complete: next_cursor.is_none(),
            });
        }
        let Some(next_cursor) = next_cursor else {
            return Ok(CodexThreadList {
                threads,
                complete: true,
            });
        };
        if !seen_cursors.insert(next_cursor.clone()) {
            return Err("CodeX App Server 会话列表分页游标重复。".to_string());
        }
        cursor = Some(next_cursor);
        request_id += 1;
    }
}

fn write_codex_app_server_message<W: Write>(
    stdin: &mut W,
    message: &Value,
) -> Result<(), String> {
    serde_json::to_writer(&mut *stdin, message)
        .map_err(|error| format!("CodeX App Server 请求序列化失败: {error}"))?;
    stdin
        .write_all(b"\n")
        .map_err(|error| format!("CodeX App Server 请求写入失败: {error}"))?;
    stdin
        .flush()
        .map_err(|error| format!("CodeX App Server 请求刷新失败: {error}"))
}

fn receive_codex_thread_list_page(
    receiver: &mpsc::Receiver<String>,
    request_id: i64,
    deadline: Instant,
) -> Result<CodexThreadListResponse, String> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("CodeX App Server 会话列表请求超时。".to_string());
        }
        let line = receiver
            .recv_timeout(remaining)
            .map_err(|_| "CodeX App Server 会话列表请求超时或连接关闭。".to_string())?;
        if let Some(response) = parse_codex_thread_list_message_for_id(&line, request_id)? {
            return Ok(response);
        }
    }
}

fn parse_codex_thread_list_message_for_id(
    line: &str,
    request_id: i64,
) -> Result<Option<CodexThreadListResponse>, String> {
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(_) => return Ok(None),
    };
    if message.get("id").and_then(Value::as_i64) != Some(request_id) {
        return Ok(None);
    }
    if let Some(error) = message.get("error") {
        return Err(format!("CodeX App Server 返回错误: {error}"));
    }
    let result = message
        .get("result")
        .cloned()
        .ok_or_else(|| "CodeX App Server 会话列表缺少 result。".to_string())?;
    let response: CodexThreadListResponse = serde_json::from_value(result)
        .map_err(|error| format!("CodeX App Server 会话列表格式不兼容: {error}"))?;
    Ok(Some(response))
}

fn terminate_child(child: &mut Child) {
    for _ in 0..10 {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[tauri::command]
pub async fn discover_opencode_projects() -> Result<OpenCodeProjectDiscovery, String> {
    tokio::task::spawn_blocking(|| {
        if let Ok(discovery) = discover_opencode_projects_from_db() {
            return Ok(discovery);
        }
        discover_opencode_projects_from_cli()
    })
    .await
    .map_err(|error| format!("OpenCode 项目发现任务异常结束: {error}"))?
}

fn clean_opencode_worktree(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches(['\\', '/']);
    #[cfg(windows)]
    {
        let stripped = if let Some(rest) = trimmed.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{rest}")
        } else {
            trimmed.strip_prefix(r"\\?\").unwrap_or(trimmed).to_string()
        };
        stripped.replace('/', "\\")
    }
    #[cfg(not(windows))]
    {
        trimmed.to_string()
    }
}

fn discover_opencode_projects_from_db() -> Result<OpenCodeProjectDiscovery, String> {
    let snapshot = crate::opencode_db::query_workspace(2000)?;
    let projects = snapshot.projects;
    let sessions = snapshot.sessions;
    let mut seen_paths = HashSet::new();
    let mut discovered = Vec::new();
    let mut skipped = 0_u32;

    for (id, worktree) in projects.iter().filter(|(_, worktree)| worktree != "/") {
        let worktree = clean_opencode_worktree(worktree);
        let normalized = normalize_path(&worktree);
        if normalized.is_empty() || !seen_paths.insert(normalized) {
            continue;
        }
        if !Path::new(&worktree).is_dir() {
            skipped = skipped.saturating_add(1);
            continue;
        }
        discovered.push(OpenCodeDiscoveredProject {
            id: id.clone(),
            worktree,
        });
    }

    for session in &sessions {
        let directory = clean_opencode_worktree(&session.directory);
        let normalized = normalize_path(&directory);
        if normalized.is_empty() || !seen_paths.insert(normalized.clone()) {
            continue;
        }
        if !Path::new(&directory).is_dir() {
            skipped = skipped.saturating_add(1);
            continue;
        }
        discovered.push(OpenCodeDiscoveredProject {
            id: format!("global:{normalized}"),
            worktree: directory,
        });
    }

    let warning =
        (skipped > 0).then(|| format!("已跳过 {skipped} 个目录已不存在的 OpenCode 项目记录"));
    Ok(OpenCodeProjectDiscovery {
        projects: discovered,
        warning,
    })
}

fn discover_opencode_projects_from_cli() -> Result<OpenCodeProjectDiscovery, String> {
    let output = run_cli_output(CliKind::Opencode, &["debug", "scrap"], None)?;
    let projects = parse_opencode_projects(&String::from_utf8_lossy(&output.stdout))?;
    let has_global_project = projects.iter().any(|project| project.worktree == "/");
    let mut global_sessions = Vec::new();
    let mut warning = None;

    if has_global_project {
        match opencode_global_probe_dir() {
            Some(cwd) => match query_opencode_sessions(&cwd, 500) {
                Ok(sessions) => {
                    global_sessions = sessions
                        .into_iter()
                        .filter(|session| Path::new(&session.directory).is_dir())
                        .collect();
                }
                Err(error) => {
                    warning = Some(format!("OpenCode 全局项目会话读取失败: {error}"));
                }
            },
            None => {
                warning = Some("找不到可用于读取 OpenCode 全局项目的系统根目录。".to_string());
            }
        }
    }

    Ok(OpenCodeProjectDiscovery {
        projects: merge_opencode_discovered_projects(&projects, &global_sessions),
        warning,
    })
}

#[tauri::command]
pub async fn list_all_opencode_sessions(
    max_count: Option<u32>,
) -> Result<Vec<OpenCodeSession>, String> {
    tokio::task::spawn_blocking(move || {
        let max_count = max_count.unwrap_or(500);
        if let Ok(sessions) = crate::opencode_db::query_sessions(max_count) {
            return Ok(sessions);
        }
        let cwd = opencode_global_probe_dir()
            .ok_or_else(|| "找不到可用于读取 OpenCode 会话的目录。".to_string())?;
        query_opencode_sessions(&cwd, max_count)
    })
    .await
    .map_err(|error| format!("OpenCode 会话读取任务异常结束: {error}"))?
}

#[tauri::command]
pub async fn list_opencode_sessions(
    project_path: String,
    max_count: Option<u32>,
) -> Result<Vec<OpenCodeSession>, String> {
    tokio::task::spawn_blocking(move || {
        let cwd = PathBuf::from(&project_path);
        if !cwd.is_dir() {
            return Err(format!("OpenCode 项目目录不存在: {project_path}"));
        }
        let max_count = max_count.unwrap_or(100);
        let normalized_target = normalize_path(&project_path);
        if let Ok(sessions) = crate::opencode_db::query_sessions(max_count) {
            return Ok(sessions
                .into_iter()
                .filter(|session| normalize_path(&session.directory) == normalized_target)
                .collect());
        }
        let sessions = query_opencode_sessions(&cwd, max_count)?;
        Ok(sessions
            .into_iter()
            .filter(|session| normalize_path(&session.directory) == normalized_target)
            .collect())
    })
    .await
    .map_err(|error| format!("OpenCode 会话读取任务异常结束: {error}"))?
}

fn query_opencode_sessions(cwd: &Path, max_count: u32) -> Result<Vec<OpenCodeSession>, String> {
    let count = max_count.clamp(1, 500).to_string();
    let output = run_cli_output(
        CliKind::Opencode,
        &["session", "list", "--format", "json", "--max-count", &count],
        Some(cwd),
    )?;
    parse_opencode_sessions(&String::from_utf8_lossy(&output.stdout))
}

fn opencode_global_probe_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        return std::env::var_os("SystemDrive")
            .map(PathBuf::from)
            .map(|drive| PathBuf::from(format!("{}\\", drive.display())))
            .filter(|path| path.is_dir())
            .or_else(|| dirs::home_dir().filter(|path| path.is_dir()));
    }

    #[cfg(not(windows))]
    {
        let root = PathBuf::from("/");
        root.is_dir().then_some(root)
    }
}

fn merge_opencode_discovered_projects(
    projects: &[OpenCodeProject],
    global_sessions: &[OpenCodeSession],
) -> Vec<OpenCodeDiscoveredProject> {
    let mut seen_paths = HashSet::new();
    let mut discovered = Vec::new();

    for project in projects.iter().filter(|project| project.worktree != "/") {
        let normalized = normalize_path(&project.worktree);
        if normalized.is_empty() || !seen_paths.insert(normalized) {
            continue;
        }
        discovered.push(OpenCodeDiscoveredProject {
            id: project.id.clone(),
            worktree: project.worktree.clone(),
        });
    }

    for session in global_sessions {
        let normalized = normalize_path(&session.directory);
        if normalized.is_empty() || !seen_paths.insert(normalized.clone()) {
            continue;
        }
        discovered.push(OpenCodeDiscoveredProject {
            id: format!("global:{normalized}"),
            worktree: session.directory.clone(),
        });
    }

    discovered
}

fn normalize_path(path: &str) -> String {
    #[cfg(windows)]
    {
        return path
            .trim()
            .trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_lowercase();
    }

    #[cfg(not(windows))]
    {
        if path == "/" {
            "/".to_string()
        } else {
            path.trim_end_matches('/').to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn path_comparison_is_case_and_separator_insensitive_on_windows() {
        assert_eq!(
            normalize_path(r"D:\Work\Demo\\"),
            normalize_path("d:/work/demo")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_path_comparison_preserves_case_unicode_and_root() {
        assert_ne!(normalize_path("/Users/Demo"), normalize_path("/Users/demo"));
        assert_eq!(normalize_path("/Users/项目 space/"), "/Users/项目 space");
        assert_eq!(
            normalize_path("/Users/ leading /trailing "),
            "/Users/ leading /trailing "
        );
        assert_eq!(normalize_path("/"), "/");
    }

    #[cfg(not(windows))]
    #[test]
    fn opencode_global_probe_uses_unix_root() {
        assert_eq!(opencode_global_probe_dir(), Some(PathBuf::from("/")));
    }

    #[test]
    fn codex_help_must_advertise_project_directory_and_resume() {
        assert!(codex_help_has_workspace_capabilities(
            "-C, --cd <DIR>  Set working directory\nresume  Resume a previous session"
        ));
        assert!(!codex_help_has_workspace_capabilities(
            "--cd <DIR>  Set working directory"
        ));
        assert!(!codex_help_has_workspace_capabilities(
            "-C <DIR>  Set working directory"
        ));
    }

    #[test]
    fn opencode_global_project_expands_to_unique_session_directories() {
        let projects = parse_opencode_projects(include_str!(
            "../tests/fixtures/cli/opencode-debug-scrap-global.sample.json"
        ))
        .expect("project sample should parse");
        let sessions = parse_opencode_sessions(include_str!(
            "../tests/fixtures/cli/opencode-global-session-list.sample.json"
        ))
        .expect("session sample should parse");

        let discovered = merge_opencode_discovered_projects(&projects, &sessions);
        #[cfg(windows)]
        assert_eq!(discovered.len(), 3);
        #[cfg(not(windows))]
        assert_eq!(discovered.len(), 4);
        assert!(discovered.iter().all(|project| project.worktree != "/"));
        #[cfg(windows)]
        assert_eq!(
            discovered
                .iter()
                .filter(|project| {
                    normalize_path(&project.worktree) == normalize_path(r"D:\work\global-one")
                })
                .count(),
            1
        );
        #[cfg(not(windows))]
        assert_eq!(
            discovered
                .iter()
                .filter(|project| project.worktree.to_ascii_lowercase().contains("global-one"))
                .count(),
            2,
            "Unix must not merge differently-cased native paths",
        );
    }

    #[test]
    fn codex_app_server_thread_list_sample_maps_desktop_and_cli_threads() {
        let sample = include_str!("../tests/fixtures/cli/codex-app-server-thread-list.sample.json");
        let threads = parse_codex_thread_list_message_for_id(sample, 1)
            .expect("sample should parse")
            .expect("sample should be a thread/list response");
        assert_eq!(threads.data.len(), 2);
        assert_eq!(threads.data[0].id, "019f5f28-5a4d-71b2-8d69-5f7d8b2c9da1");
        assert_eq!(threads.data[0].cwd, r"D:\project\cc-launcher");
        assert_eq!(threads.data[0].name.as_deref(), Some("Desktop task"));
        assert_eq!(threads.data[0].model_provider.as_deref(), Some("openai"));
        assert_eq!(threads.data[1].preview, "CLI task prompt");
        assert!(threads.next_cursor.is_none());
    }

    #[test]
    fn codex_app_server_uses_the_shared_home_without_a_profile_flag() {
        assert_eq!(
            codex_app_server_args(),
            ["app-server"]
        );
    }

    #[test]
    fn codex_thread_list_parser_preserves_the_next_cursor() {
        let page = parse_codex_thread_list_message_for_id(
            r#"{"id":1,"result":{"data":[],"nextCursor":"2026-07-07T01:28:04.235Z"}}"#,
            1,
        )
        .expect("page should parse")
        .expect("page should be a thread/list response");
        assert_eq!(page.next_cursor.as_deref(), Some("2026-07-07T01:28:04.235Z"));
    }

    #[test]
    fn codex_thread_title_enrichment_replaces_injected_instruction_text() {
        let app_server = CodexThreadSummary {
            id: "thread-1".to_string(),
            name: Some("# AGENTS.md instructions for D:\\project\\cc-launcher".to_string()),
            preview: "# AGENTS.md instructions".to_string(),
            model_provider: Some("company_proxy".to_string()),
            cwd: r"D:\project\cc-launcher".to_string(),
            created_at: 1,
            updated_at: 2,
        };
        let rollout = CodexThreadSummary {
            id: "thread-1".to_string(),
            name: None,
            preview: "真实用户问题".to_string(),
            model_provider: Some("company_proxy".to_string()),
            cwd: r"D:\project\cc-launcher".to_string(),
            created_at: 1,
            updated_at: 2,
        };

        let enriched = enrich_codex_thread_titles(vec![app_server], &[rollout]);
        assert_eq!(enriched[0].name, None);
        assert_eq!(enriched[0].preview, "真实用户问题");
    }

    #[test]
    fn codex_thread_merge_keeps_rollouts_missing_from_the_app_server_page() {
        let app_server = CodexThreadSummary {
            id: "current".to_string(),
            name: Some("Current".to_string()),
            preview: String::new(),
            model_provider: Some("openai".to_string()),
            cwd: r"D:\project\cc-launcher".to_string(),
            created_at: 2,
            updated_at: 2,
        };
        let older_rollout = CodexThreadSummary {
            id: "older".to_string(),
            name: None,
            preview: "历史会话".to_string(),
            model_provider: Some("openai".to_string()),
            cwd: r"D:\project\cc-launcher".to_string(),
            created_at: 1,
            updated_at: 1,
        };

        let merged = merge_codex_thread_lists(vec![app_server], Some(&[older_rollout]), 100);
        assert_eq!(
            merged.iter().map(|thread| thread.id.as_str()).collect::<Vec<_>>(),
            vec!["current", "older"]
        );
    }

    #[test]
    fn codex_rollout_fallback_builds_threads_from_session_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let nested = dir.path().join("2026").join("07").join("17");
        fs::create_dir_all(&nested).expect("create nested");
        let rollout =
            nested.join("rollout-2026-07-17T01-57-47-019f6c13-d886-7521-a0de-90cfe0a99c67.jsonl");
        fs::write(
            &rollout,
            concat!(
                "{\"timestamp\":\"2026-07-17T01:57:47.000Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"019f6c13-d886-7521-a0de-90cfe0a99c67\",\"timestamp\":\"2026-07-17T01:57:47.000Z\",\"cwd\":\"D:\\\\Project\\\\demo\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"<environment_context>\\n  <cwd>D:\\\\Project\\\\demo</cwd>\\n</environment_context>\"}]}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"把当前项目的git更新到最新\"}]}}\n"
            ),
        )
        .expect("write rollout");

        let (threads, complete) = codex_threads_from_rollout_root_with_completeness(dir.path(), 100);
        assert_eq!(threads.len(), 1);
        assert!(complete);
        assert_eq!(threads[0].id, "019f6c13-d886-7521-a0de-90cfe0a99c67");
        assert_eq!(threads[0].cwd, r"D:\Project\demo");
        assert_eq!(threads[0].created_at, 1784253467);
        assert!(threads[0].updated_at >= threads[0].created_at);
        assert_eq!(threads[0].preview, "把当前项目的git更新到最新");
    }

    #[test]
    fn codex_rollout_index_feeds_discovery_and_threads_without_reenumeration() {
        let dir = tempfile::tempdir().expect("temp dir");
        let sessions_root = dir.path().join("sessions");
        let worktree = dir.path().join("worktree");
        fs::create_dir_all(&sessions_root).expect("create sessions");
        fs::create_dir_all(&worktree).expect("create worktree");
        let rollout = sessions_root.join("rollout.jsonl");
        let meta = json!({
            "type": "session_meta",
            "payload": {
                "id": "thread-1",
                "timestamp": "2026-08-31T01:02:03Z",
                "cwd": worktree.to_string_lossy(),
                "model_provider": "provider-a"
            }
        });
        let message = json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "真实问题" }]
            }
        });
        fs::write(&rollout, format!("{meta}\n{message}\n")).expect("write rollout");

        let index = build_codex_rollout_index(&sessions_root);
        let discovery = discover_codex_projects_from_rollout_index(&index);
        let (threads, complete) = codex_threads_from_rollout_index(&index, 100);

        assert_eq!(index.records.len(), 1);
        assert_eq!(discovery.projects.len(), 1);
        assert_eq!(discovery.projects[0].session_count, 1);
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id, "thread-1");
        assert_eq!(threads[0].preview, "真实问题");
        assert_eq!(threads[0].model_provider.as_deref(), Some("provider-a"));
        assert!(complete);

        let updated_meta = json!({
            "type": "session_meta",
            "payload": {
                "id": "thread-1",
                "timestamp": "2026-08-31T01:02:04Z",
                "cwd": worktree.to_string_lossy(),
                "model_provider": "provider-b"
            }
        });
        let updated_message = json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "更新后的真实问题" }]
            }
        });
        fs::write(
            &rollout,
            format!("{updated_meta}\n{updated_message}\n"),
        )
        .expect("update rollout");

        let (updated_threads, _) = codex_threads_from_rollout_index(&index, 100);
        assert_eq!(updated_threads[0].preview, "更新后的真实问题");
        assert_eq!(
            updated_threads[0].model_provider.as_deref(),
            Some("provider-b")
        );
    }

    #[test]
    fn codex_rollout_preview_skips_non_user_and_wrapped_lines() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rollout = dir.path().join("rollout.jsonl");
        fs::write(
            &rollout,
            concat!(
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}}\n",
                "not json\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"# AGENTS.md instructions for D:\\\\project\\\\cc-launcher\"}]}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"\"}]}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"真实问题\"}]}}\n"
            ),
        )
        .expect("write rollout");
        assert_eq!(read_codex_rollout_preview(&rollout), "真实问题");
    }

    #[test]
    fn opencode_db_worktrees_normalize_to_native_display_form() {
        let cleaned = clean_opencode_worktree("D:/Project/demo/");
        #[cfg(windows)]
        assert_eq!(cleaned, r"D:\Project\demo");
        #[cfg(not(windows))]
        assert_eq!(cleaned, "D:/Project/demo");
        #[cfg(windows)]
        assert_eq!(
            clean_opencode_worktree(r"\\?\D:\Project\demo\"),
            r"D:\Project\demo"
        );
    }

    #[test]
    fn codex_jsonl_project_discovery_reads_only_session_meta() {
        let sample = include_str!("../tests/fixtures/cli/codex-session-meta.sample.jsonl");
        let first_line = sample.lines().next().expect("fixture should have metadata");
        let meta = parse_codex_session_meta(first_line).expect("session_meta should parse");
        assert_eq!(meta.cwd, r"D:\project\cc-launcher");
        assert_eq!(meta.timestamp.as_deref(), Some("2026-07-14T05:45:12.933Z"));
        assert!(parse_codex_session_meta(
            sample
                .lines()
                .nth(1)
                .expect("fixture should have a body line")
        )
        .is_none());
        assert_eq!(
            clean_codex_worktree(r"\\?\D:\project\cc-launcher\"),
            r"D:\project\cc-launcher"
        );
    }
}
