//! Codex 分页会话（paginated rollout）链完整性的检测与修复。
//!
//! Codex 0.149+ alpha 起，会话文件超过阈值后会翻页：新页文件名为
//! `rollout-<时间戳>-<线程uuid>_<页uuid>.jsonl`（双 UUID），首行 `session_meta`
//! 带 `history_base` 指向前一页的 `[0, end_byte_offset)` 字节范围。翻页点取在
//! 当前回合起点，新页会把父页尾部进行中的回合重录一遍。
//!
//! 当翻页方使用内存中的旧读位置作为 `history_base` 时（多窗口/重载场景），
//! 父页尾部会出现一段不在任何页链上的"孤立记录"，读取端顺链读取时表现为
//! 最近的聊天内容"丢失"。本模块负责检测这类断链并按已验证的合并算法修复。
//!
//! 修复原则：
//! - 只动传入的会话目录，不碰 sqlite 状态库；
//! - 修复前必须确认 Codex 桌面端 / VS Code 已退出（内存旧状态会覆盖修复）；
//! - 链上所有原始文件先复制到 `sessions/` 之外的备份目录；
//! - 合并结果写临时文件、校验通过后才原子替换回原路径。

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ISSUE_KIND_ORPHANED_TAIL: &str = "orphaned_tail";
pub const ISSUE_KIND_PARENT_MISSING: &str = "parent_missing";
pub const ISSUE_KIND_BASE_BEYOND_END: &str = "base_beyond_end";
pub const ISSUE_KIND_WRITER_NEWER: &str = "writer_newer_than_cli";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionIssue {
    pub thread_id: String,
    pub cwd: String,
    pub project_name: String,
    pub preview: String,
    pub kind: String,
    /// false 表示只能提示，不能自动修复（如父页缺失、base 越界、版本提示）。
    pub repairable: bool,
    pub orphaned_bytes: u64,
    pub orphaned_records: u64,
    pub orphaned_user_messages: u64,
    pub orphaned_first_at: Option<String>,
    pub orphaned_last_at: Option<String>,
    pub page_count: u32,
    pub writer_cli_version: Option<String>,
    pub local_cli_version: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairCodexSessionChainRequest {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionRepairResult {
    pub thread_id: String,
    pub strategy: String,
    pub backup_dir: String,
    pub page_count: u32,
    pub merged_bytes: u64,
    pub kept_records: u64,
    pub skipped_records: u64,
    /// 同步状态库 rollout_path 的结果说明（None = 状态库不存在/无此行/未改动）。
    pub rollout_path_note: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone)]
struct PageMeta {
    thread_id: Option<String>,
    cli_version: Option<String>,
    cwd: String,
    /// history_base = (父页文件名 uuid, end_ordinal_exclusive, end_byte_offset)。
    history_base: Option<(String, u64, u64)>,
}

#[derive(Debug, Clone)]
struct PageEntry {
    path: PathBuf,
    name: String,
    page_uuid: String,
    size: u64,
    meta: PageMeta,
}

/// 解析 rollout 文件名，返回 (页 uuid, 是否双 UUID 分页文件)。
fn parse_rollout_page_name(name: &str) -> Option<(String, bool)> {
    let core = name.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
    // 去掉前缀时间戳 "YYYY-MM-DDTHH-MM-SS-"（20 个字符）。
    let rest = core.get(20..)?;
    if let Some((first, second)) = rest.split_once('_') {
        if is_uuid_like(first) && is_uuid_like(second) {
            return Some((second.to_string(), true));
        }
        return None;
    }
    if is_uuid_like(rest) {
        return Some((rest.to_string(), false));
    }
    None
}

fn is_uuid_like(value: &str) -> bool {
    value.len() == 36
        && value.chars().enumerate().all(|(index, ch)| match index {
            8 | 13 | 18 | 23 => ch == '-',
            _ => ch.is_ascii_hexdigit(),
        })
}

fn read_page_meta(path: &Path) -> Option<PageMeta> {
    let file = File::open(path).ok()?;
    let first_line = BufReader::new(file).lines().next()?.ok()?;
    parse_page_meta(&first_line)
}

fn parse_page_meta(line: &str) -> Option<PageMeta> {
    let value: Value = serde_json::from_str(line.trim_start_matches('\u{feff}')).ok()?;
    (value.get("type")?.as_str()? == "session_meta").then_some(())?;
    let payload = value.get("payload")?;
    let history_base = payload.get("history_base").and_then(|base| {
        let parent = base.get("thread_id")?.as_str()?.to_string();
        let end_ordinal_exclusive = base.get("end_ordinal_exclusive")?.as_u64()?;
        let end_byte_offset = base.get("end_byte_offset")?.as_u64()?;
        Some((parent, end_ordinal_exclusive, end_byte_offset))
    });
    Some(PageMeta {
        thread_id: payload
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string),
        cli_version: payload
            .get("cli_version")
            .and_then(Value::as_str)
            .map(str::to_string),
        cwd: payload
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        history_base,
    })
}

fn collect_pages(root: &Path) -> Vec<PageEntry> {
    let mut files = Vec::new();
    collect_rollout_files(root, &mut files);
    let mut pages = Vec::with_capacity(files.len());
    for path in files {
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some((page_uuid, _double_uuid)) = parse_rollout_page_name(name) else {
            continue;
        };
        let name = name.to_string();
        let size = fs::metadata(&path).map(|metadata| metadata.len()).unwrap_or(0);
        let Some(meta) = read_page_meta(&path) else {
            continue;
        };
        pages.push(PageEntry {
            path,
            name,
            page_uuid,
            size,
            meta,
        });
    }
    pages
}

fn collect_rollout_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_rollout_files(&path, files);
        } else if file_type.is_file() {
            let is_rollout = path
                .file_name()
                .and_then(|value| value.to_str())
                .map(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
                .unwrap_or(false);
            if is_rollout {
                files.push(path);
            }
        }
    }
}

#[derive(Default)]
struct OrphanStats {
    bytes: u64,
    records: u64,
    user_messages: u64,
    first_at: Option<String>,
    last_at: Option<String>,
}

fn extract_json_string_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    let needle = format!("\"{field}\":\"");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// 统计父页 `[end_byte_offset, 文件末尾)` 区间内的孤立记录。
fn scan_orphaned_tail(path: &Path, end_byte_offset: u64) -> OrphanStats {
    let mut stats = OrphanStats::default();
    let Ok(file) = File::open(path) else {
        return stats;
    };
    let file_size = fs::metadata(path).map(|metadata| metadata.len()).unwrap_or(0);
    if file_size <= end_byte_offset {
        return stats;
    }
    stats.bytes = file_size - end_byte_offset;
    let mut reader = BufReader::new(file);
    let mut position = 0_u64;
    let mut line = String::new();
    loop {
        line.clear();
        let Ok(read) = reader.read_line(&mut line) else {
            break;
        };
        if read == 0 {
            break;
        }
        let line_start = position;
        position = position.saturating_add(read as u64);
        if line_start < end_byte_offset {
            continue;
        }
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed.is_empty() {
            continue;
        }
        stats.records = stats.records.saturating_add(1);
        if trimmed.contains("\"role\":\"user\"") || trimmed.contains("\"type\":\"user_message\"") {
            stats.user_messages = stats.user_messages.saturating_add(1);
        }
        if let Some(timestamp) = extract_json_string_field(trimmed, "timestamp") {
            if stats.first_at.is_none() {
                stats.first_at = Some(timestamp.to_string());
            }
            stats.last_at = Some(timestamp.to_string());
        }
    }
    stats
}

fn project_name_from_cwd(cwd: &str) -> String {
    let trimmed = cwd.trim().trim_end_matches(['\\', '/']);
    trimmed
        .rsplit(['\\', '/'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

/// 组装线程的页链（根页在前，链尖在后）。
/// 选择文件名时间戳最新的"无人以它为父"的页作为链尖回溯。
fn build_chain(pages: &[&PageEntry]) -> Result<Vec<PageEntry>, String> {
    let by_uuid: HashMap<&str, &PageEntry> = pages
        .iter()
        .map(|page| (page.page_uuid.as_str(), *page))
        .collect();
    let parent_uuids: HashSet<&str> = pages
        .iter()
        .filter_map(|page| page.meta.history_base.as_ref().map(|base| base.0.as_str()))
        .collect();
    let tip = pages
        .iter()
        .filter(|page| !parent_uuids.contains(page.page_uuid.as_str()))
        .max_by(|left, right| left.name.cmp(&right.name))
        .ok_or_else(|| "会话页链为空或存在环".to_string())?;
    let mut chain: Vec<&PageEntry> = Vec::new();
    let mut visited = HashSet::new();
    let mut current: Option<&PageEntry> = Some(*tip);
    while let Some(page) = current {
        if !visited.insert(page.page_uuid.clone()) {
            return Err("会话页链存在环，无法自动修复".to_string());
        }
        chain.push(page);
        current = page
            .meta
            .history_base
            .as_ref()
            .and_then(|base| by_uuid.get(base.0.as_str()).copied());
    }
    chain.reverse();
    Ok(chain.into_iter().cloned().collect())
}

fn chain_root_dangling(chain: &[PageEntry]) -> bool {
    chain
        .first()
        .map(|page| page.meta.history_base.is_some())
        .unwrap_or(false)
}

/// 扫描会话目录，返回分页链完整性问题列表。
/// `local_cli_version` 用于生成"写入方版本高于本机 CLI"的提示。
pub fn scan_sessions_integrity(
    sessions_root: &Path,
    local_cli_version: Option<&str>,
) -> Vec<CodexSessionIssue> {
    let pages = collect_pages(sessions_root);
    let by_uuid: HashMap<&str, &PageEntry> = pages
        .iter()
        .map(|page| (page.page_uuid.as_str(), page))
        .collect();
    let mut by_thread: HashMap<String, Vec<&PageEntry>> = HashMap::new();
    for page in &pages {
        let Some(thread_id) = page.meta.thread_id.clone() else {
            continue;
        };
        by_thread.entry(thread_id).or_default().push(page);
    }

    let mut issues = Vec::new();
    for (thread_id, thread_pages) in &by_thread {
        let chain = build_chain(thread_pages).ok();
        let page_count = thread_pages.len() as u32;
        let root = chain.as_ref().and_then(|chain| chain.first());
        let cwd = root
            .map(|page| page.meta.cwd.clone())
            .or_else(|| thread_pages.first().map(|page| page.meta.cwd.clone()))
            .unwrap_or_default();
        let preview = root
            .map(|page| crate::cli_runtime::read_codex_rollout_preview(&page.path))
            .unwrap_or_default();

        for page in thread_pages {
            let Some((parent_uuid, _end_ordinal, end_byte_offset)) =
                page.meta.history_base.as_ref()
            else {
                continue;
            };
            let Some(parent) = by_uuid.get(parent_uuid.as_str()) else {
                issues.push(CodexSessionIssue {
                    thread_id: thread_id.clone(),
                    cwd: cwd.clone(),
                    project_name: project_name_from_cwd(&cwd),
                    preview: preview.clone(),
                    kind: ISSUE_KIND_PARENT_MISSING.to_string(),
                    repairable: false,
                    orphaned_bytes: 0,
                    orphaned_records: 0,
                    orphaned_user_messages: 0,
                    orphaned_first_at: None,
                    orphaned_last_at: None,
                    page_count,
                    writer_cli_version: page.meta.cli_version.clone(),
                    local_cli_version: local_cli_version.map(str::to_string),
                    message: format!(
                        "分页文件 {} 的父页 {} 缺失，链无法完整拼接。",
                        page.name, parent_uuid
                    ),
                });
                continue;
            };
            if *end_byte_offset > parent.size {
                issues.push(CodexSessionIssue {
                    thread_id: thread_id.clone(),
                    cwd: cwd.clone(),
                    project_name: project_name_from_cwd(&cwd),
                    preview: preview.clone(),
                    kind: ISSUE_KIND_BASE_BEYOND_END.to_string(),
                    repairable: false,
                    orphaned_bytes: 0,
                    orphaned_records: 0,
                    orphaned_user_messages: 0,
                    orphaned_first_at: None,
                    orphaned_last_at: None,
                    page_count,
                    writer_cli_version: page.meta.cli_version.clone(),
                    local_cli_version: local_cli_version.map(str::to_string),
                    message: format!(
                        "分页文件 {} 的挂接点（{} 字节）超出父页实际大小（{} 字节）。",
                        page.name, end_byte_offset, parent.size
                    ),
                });
                continue;
            }
            if parent.size > *end_byte_offset {
                let stats = scan_orphaned_tail(&parent.path, *end_byte_offset);
                if stats.records == 0 {
                    continue;
                }
                let repairable = chain
                    .as_ref()
                    .map(|chain| !chain_root_dangling(chain))
                    .unwrap_or(false);
                let time_range = match (&stats.first_at, &stats.last_at) {
                    (Some(first), Some(last)) => format!("{first} ~ {last}"),
                    _ => "未知时间".to_string(),
                };
                issues.push(CodexSessionIssue {
                    thread_id: thread_id.clone(),
                    cwd: cwd.clone(),
                    project_name: project_name_from_cwd(&cwd),
                    preview: preview.clone(),
                    kind: ISSUE_KIND_ORPHANED_TAIL.to_string(),
                    repairable,
                    orphaned_bytes: stats.bytes,
                    orphaned_records: stats.records,
                    orphaned_user_messages: stats.user_messages,
                    orphaned_first_at: stats.first_at.clone(),
                    orphaned_last_at: stats.last_at.clone(),
                    page_count,
                    writer_cli_version: page.meta.cli_version.clone(),
                    local_cli_version: local_cli_version.map(str::to_string),
                    message: format!(
                        "翻页挂接点早于父页末尾，{} 条记录（{} 字节，含 {} 条用户消息，{}）不在任何页链上，读取端不可见。",
                        stats.records, stats.bytes, stats.user_messages, time_range
                    ),
                });
            }
        }

        // 版本提示：以链尖（文件名时间戳最新）的写入方版本为准。
        if let Some(local) = local_cli_version {
            let tip = thread_pages
                .iter()
                .max_by(|left, right| left.name.cmp(&right.name));
            if let Some(tip) = tip {
                if let Some(writer) = tip.meta.cli_version.clone() {
                    if cmp_loose_version(&writer, local) == Ordering::Greater {
                        issues.push(CodexSessionIssue {
                            thread_id: thread_id.clone(),
                            cwd: cwd.clone(),
                            project_name: project_name_from_cwd(&cwd),
                            preview: preview.clone(),
                            kind: ISSUE_KIND_WRITER_NEWER.to_string(),
                            repairable: false,
                            orphaned_bytes: 0,
                            orphaned_records: 0,
                            orphaned_user_messages: 0,
                            orphaned_first_at: None,
                            orphaned_last_at: None,
                            page_count,
                            writer_cli_version: Some(writer.clone()),
                            local_cli_version: Some(local.to_string()),
                            message: format!(
                                "该会话由 codex {writer} 写入，本机 CLI（{local}）较旧，启动器内恢复会话可能读不全；请升级 codex CLI。"
                            ),
                        });
                    }
                }
            }
        }
    }

    issues.sort_by(|left, right| {
        right
            .orphaned_bytes
            .cmp(&left.orphaned_bytes)
            .then_with(|| left.thread_id.cmp(&right.thread_id))
    });
    issues
}

/// 宽松的语义化版本比较：先比数值三元组，再按"正式版 > 预发布"规则比较后缀。
pub fn cmp_loose_version(left: &str, right: &str) -> Ordering {
    fn split(version: &str) -> (Vec<u64>, String) {
        let (base, pre) = match version.split_once('-') {
            Some((base, pre)) => (base, pre),
            None => (version, ""),
        };
        let numbers = base
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect();
        (numbers, pre.to_string())
    }
    let (left_base, left_pre) = split(left);
    let (right_base, right_pre) = split(right);
    for index in 0..3 {
        let a = left_base.get(index).copied().unwrap_or(0);
        let b = right_base.get(index).copied().unwrap_or(0);
        match a.cmp(&b) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    match (left_pre.is_empty(), right_pre.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => left_pre.cmp(&right_pre),
    }
}

/// 探测本机 codex CLI 版本（进程启动有开销，模块内缓存一次）。
pub fn local_codex_cli_version() -> Option<String> {
    static CACHE: OnceLock<Mutex<Option<Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().ok()?;
    if let Some(cached) = guard.as_ref() {
        return cached.clone();
    }
    let probed = probe_codex_cli_version();
    *guard = Some(probed.clone());
    probed
}

fn probe_codex_cli_version() -> Option<String> {
    let executable = crate::cli_runtime::locate_cli(crate::cli_contract::CliKind::Codex)?;
    let mut command = std::process::Command::new(executable);
    command.arg("--version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // 输出形如 "codex-cli 0.147.0"。
    stdout
        .split_whitespace()
        .find(|token| token.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
        .map(str::to_string)
}

/// 检测 Codex 桌面端 / VS Code 是否在运行；在运行则拒绝修复。
pub fn ensure_codex_clients_not_running() -> Result<(), String> {
    let running = running_codex_client_processes();
    if running.is_empty() {
        return Ok(());
    }
    Err(format!(
        "检测到 {} 正在运行。修复会话前请完全退出 Codex 桌面端与 VS Code（含后台窗口），否则客户端退出时会用内存旧状态覆盖修复结果，然后重试。",
        running.join("、")
    ))
}

#[cfg(windows)]
fn running_codex_client_processes() -> Vec<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = std::process::Command::new("tasklist");
    command.args(["/FO", "CSV", "/NH"]);
    command.creation_flags(CREATE_NO_WINDOW);
    let Ok(output) = command.output() else {
        return Vec::new();
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let targets = ["code.exe", "codex.exe", "chatgpt.exe"];
    let mut found = Vec::new();
    for line in stdout.lines() {
        let name = line
            .trim_start_matches('"')
            .split('"')
            .next()
            .unwrap_or_default()
            .to_lowercase();
        if targets.contains(&name.as_str()) && !found.contains(&name) {
            found.push(name);
        }
    }
    found
}

#[cfg(not(windows))]
fn running_codex_client_processes() -> Vec<String> {
    let mut found = Vec::new();
    for name in ["Code", "Codex"] {
        let running = std::process::Command::new("pgrep")
            .args(["-x", name])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if running {
            found.push(name.to_string());
        }
    }
    found
}

fn extract_ordinal(line: &str) -> Option<u64> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    value.get("ordinal")?.as_u64()
}

/// 把线程的整条页链合并回填到根页原路径，得到自包含单文件。
///
/// 算法（已在本机真实数据上验证）：
/// - 保留根页 `[0, 第二页 history_base.end_byte_offset)` 字节；根页尾部被
///   孤立的回合由第二页头部的重录版本替代（原始执行细节保留在备份中）；
/// - 依次追加后续每一页去掉首行 meta 后的正文，跳过 ordinal 不超过已纳入
///   最大值的记录（即丢弃链尖分叉页对同一回合的重复重录）；
/// - 链尖页里 ordinal 超出父页范围的全新内容会保留。
pub fn repair_thread_chain(
    sessions_root: &Path,
    thread_id: &str,
    backup_root: &Path,
    codex_home: Option<&Path>,
) -> Result<CodexSessionRepairResult, String> {
    let pages = collect_pages(sessions_root);
    let thread_pages: Vec<&PageEntry> = pages
        .iter()
        .filter(|page| page.meta.thread_id.as_deref() == Some(thread_id))
        .collect();
    if thread_pages.is_empty() {
        return Err(format!("找不到会话 {thread_id} 的任何分页文件。"));
    }
    let chain = build_chain(&thread_pages)?;
    if chain_root_dangling(&chain) {
        return Err("该会话的根页仍指向缺失的父页，无法自动合并；原始文件未改动。".to_string());
    }
    if chain.len() < 2 {
        return Err("该会话不是分页会话，无需修复。".to_string());
    }
    let root = chain
        .first()
        .expect("chain has at least two pages")
        .clone();
    let cut = chain[1]
        .meta
        .history_base
        .as_ref()
        .map(|base| base.2)
        .ok_or_else(|| "第二页缺少 history_base，链结构异常。".to_string())?;

    let root_bytes = fs::read(&root.path)
        .map_err(|error| format!("无法读取根页 {}：{error}", root.path.display()))?;
    if cut > root_bytes.len() as u64 {
        return Err(format!(
            "挂接点（{cut} 字节）超出根页大小（{} 字节），无法自动合并。",
            root_bytes.len()
        ));
    }
    let cut = cut as usize;
    if cut > 0 && root_bytes.get(cut.wrapping_sub(1)) != Some(&b'\n') {
        return Err("挂接点不在行边界，为避免截断记录已放弃自动修复。".to_string());
    }

    // 1) 备份链上所有原始文件（先复制，替换成功前原件不动）。
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let short_id: String = thread_id.chars().take(8).collect();
    let backup_dir = backup_root.join(format!("{stamp}-{short_id}"));
    fs::create_dir_all(&backup_dir)
        .map_err(|error| format!("无法创建会话备份目录 {}：{error}", backup_dir.display()))?;
    for page in &chain {
        let backup_path = backup_dir.join(&page.name);
        fs::copy(&page.path, &backup_path)
            .map_err(|error| format!("无法备份 {}：{error}", page.path.display()))?;
    }

    // 2) 组装合并内容。
    let mut output = root_bytes[..cut].to_vec();
    let mut running_max: Option<u64> = None;
    if cut > 0 {
        let head_text = String::from_utf8_lossy(&root_bytes[..cut]);
        if let Some(last_line) = head_text.lines().last() {
            running_max = extract_ordinal(last_line);
        }
    }
    let mut kept_records = 0_u64;
    let mut skipped_records = 0_u64;
    for page in &chain[1..] {
        let file = File::open(&page.path)
            .map_err(|error| format!("无法读取分页 {}：{error}", page.path.display()))?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        let mut first_line = true;
        loop {
            line.clear();
            let read = reader
                .read_line(&mut line)
                .map_err(|error| format!("无法读取分页 {}：{error}", page.path.display()))?;
            if read == 0 {
                break;
            }
            if first_line {
                // 跳过每页的 session_meta。
                first_line = false;
                continue;
            }
            let trimmed = line.trim_end_matches(['\n', '\r']);
            if trimmed.is_empty() {
                continue;
            }
            let ordinal = extract_ordinal(trimmed);
            let keep = match (ordinal, running_max) {
                (Some(ordinal), Some(max)) => ordinal > max,
                _ => true,
            };
            if keep {
                output.extend_from_slice(trimmed.as_bytes());
                output.push(b'\n');
                kept_records = kept_records.saturating_add(1);
                if let Some(ordinal) = ordinal {
                    running_max = Some(running_max.map_or(ordinal, |max| max.max(ordinal)));
                }
            } else {
                skipped_records = skipped_records.saturating_add(1);
            }
        }
    }

    // 3) 写临时文件并校验。
    let temp_path = root.path.with_extension("consolidating");
    {
        let mut temp = File::create(&temp_path)
            .map_err(|error| format!("无法创建合并临时文件 {}：{error}", temp_path.display()))?;
        temp.write_all(&output)
            .map_err(|error| format!("无法写入合并临时文件：{error}"))?;
        temp.sync_all()
            .map_err(|error| format!("无法刷新合并临时文件：{error}"))?;
    }
    let validation = (|| -> Result<(), String> {
        let meta = read_page_meta(&temp_path)
            .ok_or_else(|| "合并结果首行不是有效的 session_meta。".to_string())?;
        if meta.thread_id.as_deref() != Some(thread_id) {
            return Err("合并结果的线程 id 与预期不一致。".to_string());
        }
        if meta.history_base.is_some() {
            return Err("合并结果仍带 history_base。".to_string());
        }
        if cut > 0 && !output.ends_with(b"\n") {
            return Err("合并结果未以换行结尾。".to_string());
        }
        Ok(())
    })();
    if let Err(error) = validation {
        let _ = fs::remove_file(&temp_path);
        return Err(format!("合并结果校验失败：{error}；原始文件未改动。"));
    }

    // 4) 原子替换：原件移入备份目录，临时文件改名到根页原路径。
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let move_result = (|| -> Result<(), String> {
        for page in &chain {
            let backup_path = backup_dir.join(&page.name);
            rename_or_copy(&page.path, &backup_path)?;
            moved.push((backup_path, page.path.clone()));
        }
        rename_or_copy(&temp_path, &root.path)?;
        Ok(())
    })();
    if let Err(error) = move_result {
        // 回滚：把已移走的原件放回原路径。
        for (backup_path, original_path) in moved.iter().rev() {
            let _ = rename_or_copy(backup_path, original_path);
        }
        let _ = fs::remove_file(&temp_path);
        return Err(format!("替换会话文件失败，已回滚：{error}"));
    }

    let page_count = chain.len() as u32;
    let merged_bytes = output.len() as u64;

    // 5) 同步 codex 状态库 threads.rollout_path：合并后只剩根页单文件，但状态库
    // 仍指着已被移走的链尖分页，resume 会 resolve 不到文件而报"恢复对话失败"。
    // 这是 best-effort：状态库不存在 / 无此行 / 已正确时静默跳过，不影响合并结果。
    let rollout_path_note = codex_home.and_then(|home| {
        match sync_state_rollout_path(home, thread_id, &root.path) {
            Ok(note) => note,
            Err(error) => {
                eprintln!("[codex_session_chain] 同步状态库 rollout_path 失败: {error}");
                Some(format!("状态库 rollout_path 同步失败（不影响会话内容）: {error}"))
            }
        }
    });

    let mut message = format!(
        "已把 {page_count} 个分页文件合并为单文件（{merged_bytes} 字节，保留 {kept_records} 条后续记录，丢弃 {skipped_records} 条分叉重录）。原始文件已备份到 {}。",
        backup_dir.display()
    );
    if let Some(note) = &rollout_path_note {
        message.push(' ');
        message.push_str(note);
    }

    Ok(CodexSessionRepairResult {
        thread_id: thread_id.to_string(),
        strategy: "consolidate".to_string(),
        backup_dir: backup_dir.display().to_string(),
        page_count,
        merged_bytes,
        kept_records,
        skipped_records,
        rollout_path_note,
        message,
    })
}

/// 把 threads 表里该线程的 rollout_path 更新为合并后的根页路径。
/// 返回 Ok(Some(说明)) 表示发生/确认了改动，Ok(None) 表示状态库或行不存在、或已正确。
fn sync_state_rollout_path(
    codex_home: &Path,
    thread_id: &str,
    root_path: &Path,
) -> Result<Option<String>, String> {
    // 兼容 codex 状态库的多版本文件名（state_N.sqlite，取最高版本号）。
    let db_path = latest_state_db_path(codex_home).ok_or_else(|| {
        format!(
            "未在 {} 找到 codex 状态库（state_N.sqlite）",
            codex_home.display()
        )
    })?;

    // codex 在 Windows 上给 rollout_path 加扩展长路径前缀（\\?\）；写入值要
    // 跟现有格式一致，否则读取端按字符串原样 resolve。按 codex 的约定补前缀：
    // 已是 \\?\ 或相对/其它形式不强行改，仅对普通绝对盘符路径补前缀。
    let root_text = root_path.to_string_lossy().to_string();
    let canonical = canonical_rollout_path_for_state(&root_text);

    let connection = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .map_err(|error| format!("无法打开 codex 状态库 {}: {error}", db_path.display()))?;
    connection
        .busy_timeout(Duration::from_millis(1500))
        .map_err(|error| format!("无法配置 codex 状态库: {error}"))?;

    let current: Option<String> = connection
        .query_row(
            "SELECT rollout_path FROM threads WHERE id = ?1",
            [thread_id],
            |row| row.get(0),
        )
        .ok();

    let Some(current) = current else {
        // 状态库里没有这一行（例如新线程还没入库），无需同步。
        return Ok(None);
    };
    if paths_refer_to_same_file(&current, &canonical) {
        return Ok(None);
    }

    let changed = connection
        .execute(
            "UPDATE threads SET rollout_path = ?1 WHERE id = ?2",
            rusqlite::params![canonical, thread_id],
        )
        .map_err(|error| format!("无法更新 codex 状态库 rollout_path: {error}"))?;
    if changed == 0 {
        return Ok(None);
    }
    Ok(Some(format!(
        "已同步状态库 rollout_path → {}",
        root_path.display()
    )))
}

/// 读状态库里该线程的 rollout_path（无此行/无库时返回 None）。
fn read_state_rollout_path(codex_home: &Path, thread_id: &str) -> Option<String> {
    let db_path = latest_state_db_path(codex_home)?;
    let connection = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    connection
        .busy_timeout(Duration::from_millis(500))
        .ok()?;
    connection
        .query_row(
            "SELECT rollout_path FROM threads WHERE id = ?1",
            [thread_id],
            |row| row.get(0),
        )
        .ok()
}

/// 状态库 rollout_path 指向的文件是否真实存在（忽略 \\?\ 前缀差异）。
fn state_rollout_target_exists(rollout_path: &str) -> bool {
    let normalized = rollout_path.trim_start_matches(r"\\?\");
    Path::new(normalized).is_file()
}

/// 切换配置预检时的轻量校正（区别于 repair_thread_chain 的合并重手术）：
/// 对每个磁盘上仍有存活页的线程，若状态库 rollout_path 指向已不存在的文件
/// （典型：之前合并把链尖分页移走，但状态库指针没跟上；或客户端翻页后又指向了
/// 新分页），直接把状态库校正到该线程的存活链尖。**不删/合并任何会话文件，
/// 也不要求退出 Codex 客户端**——WAL 模式下写入与客户端读不冲突，客户端下次
/// resume 即读到正确路径。
///
/// 返回 (校正的线程数, 无法校正的线程 id 列表——这些仍需走合并修复)。
pub fn reconcile_state_rollout_paths(
    sessions_root: &Path,
    codex_home: &Path,
) -> (u32, Vec<String>) {
    let pages = collect_pages(sessions_root);
    // 按线程分组，取每个线程的存活链尖（文件名时间戳最新的页）。
    let mut tip_by_thread: HashMap<String, &PageEntry> = HashMap::new();
    for page in &pages {
        let Some(thread_id) = page.meta.thread_id.clone() else {
            continue;
        };
        let replace = match tip_by_thread.get(&thread_id) {
            Some(existing) => page.name > existing.name,
            None => true,
        };
        if replace {
            tip_by_thread.insert(thread_id, page);
        }
    }

    let mut corrected = 0_u32;
    let mut unrecoverable = Vec::new();
    for (thread_id, tip) in &tip_by_thread {
        let Some(current) = read_state_rollout_path(codex_home, thread_id) else {
            continue; // 状态库无此行，无需处理
        };
        if state_rollout_target_exists(&current) {
            continue; // 状态库指向的文件仍在，正常
        }
        // 状态库指向已丢失的文件：校正到磁盘上的存活链尖。
        match sync_state_rollout_path(codex_home, thread_id, &tip.path) {
            Ok(Some(_)) => corrected = corrected.saturating_add(1),
            Ok(None) => {}
            Err(_) => unrecoverable.push(thread_id.clone()),
        }
    }
    (corrected, unrecoverable)
}

/// 取 codex_home 下版本号最高的 state_N.sqlite。
fn latest_state_db_path(codex_home: &Path) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in fs::read_dir(codex_home).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(stem) = name.strip_prefix("state_") else { continue };
        let Some(version) = stem.strip_suffix(".sqlite") else { continue };
        let Ok(version) = version.parse::<u64>() else { continue };
        let path = entry.path();
        let better = match &best {
            Some((best_version, _)) => version > *best_version,
            None => true,
        };
        if better {
            best = Some((version, path));
        }
    }
    best.map(|(_, path)| path)
}

/// 按 codex 在 Windows 上的存储约定给绝对路径补 \\?\ 前缀（其它平台原样返回）。
fn canonical_rollout_path_for_state(path: &str) -> String {
    #[cfg(windows)]
    {
        let already_prefixed = path.starts_with(r"\\?\");
        let is_absolute = {
            let bytes = path.as_bytes();
            bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && (bytes[2] == b'\\' || bytes[2] == b'/')
        };
        if !already_prefixed && is_absolute {
            return format!(r"\\?\{}", path.replace('/', "\\"));
        }
    }
    path.to_string()
}

/// 判断两个 rollout_path 字符串是否指向同一文件（忽略 \\?\ 前缀与斜杠差异）。
fn paths_refer_to_same_file(left: &str, right: &str) -> bool {
    normalize_rollout_path(left) == normalize_rollout_path(right)
}

#[cfg(windows)]
fn normalize_rollout_path(path: &str) -> String {
    let trimmed = path.trim_start_matches(r"\\?\");
    trimmed.replace('/', "\\").to_lowercase()
}

#[cfg(not(windows))]
fn normalize_rollout_path(path: &str) -> String {
    path.trim_start_matches(r"\\?\").to_string()
}

fn rename_or_copy(source: &Path, target: &Path) -> Result<(), String> {
    match fs::rename(source, target) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(source, target).map_err(|error| {
                format!(
                    "无法移动 {} 到 {}：{error}",
                    source.display(),
                    target.display()
                )
            })?;
            fs::remove_file(source)
                .map_err(|error| format!("无法删除 {}：{error}", source.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_page(path: &Path, lines: &[String]) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        let mut content = String::new();
        for line in lines {
            content.push_str(line);
            content.push('\n');
        }
        fs::write(path, content).expect("write page");
    }

    fn meta_line(thread_id: &str, ordinal: u64, history_base: Option<(&str, u64, u64)>) -> String {
        let base = history_base
            .map(|(parent, ord, off)| {
                format!(
                    ",\"history_base\":{{\"thread_id\":\"{parent}\",\"end_ordinal_exclusive\":{ord},\"end_byte_offset\":{off}}}"
                )
            })
            .unwrap_or_default();
        format!(
            "{{\"timestamp\":\"2026-09-16T10:00:00.000Z\",\"ordinal\":{ordinal},\"type\":\"session_meta\",\"payload\":{{\"id\":\"{thread_id}\",\"timestamp\":\"2026-09-16T10:00:00.000Z\",\"cwd\":\"D:\\\\work\\\\demo\",\"cli_version\":\"0.154.0-alpha.6.2\",\"history_mode\":\"paginated\"{base}}}}}"
        )
    }

    fn record_line(ordinal: u64, kind: &str) -> String {
        format!(
            "{{\"timestamp\":\"2026-09-16T10:00:{ordinal:02}.000Z\",\"ordinal\":{ordinal},\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"{kind}\"}}}}"
        )
    }

    #[test]
    fn parse_rollout_page_name_supports_single_and_double_uuid() {
        let single = parse_rollout_page_name(
            "rollout-2026-09-14T19-23-36-01a09fa8-8359-7e60-b622-b36d1031928b.jsonl",
        );
        assert_eq!(
            single,
            Some(("01a09fa8-8359-7e60-b622-b36d1031928b".to_string(), false))
        );
        let double = parse_rollout_page_name(
            "rollout-2026-09-15T12-40-02-01a09fa8-8359-7e60-b622-b36d1031928b_01a0a35d-656e-7271-ac9a-2187070e0080.jsonl",
        );
        assert_eq!(
            double,
            Some(("01a0a35d-656e-7271-ac9a-2187070e0080".to_string(), true))
        );
        assert!(parse_rollout_page_name("rollout-not-a-uuid.jsonl").is_none());
        assert!(parse_rollout_page_name("other-file.jsonl").is_none());
    }

    #[test]
    fn cmp_loose_version_orders_alpha_below_stable() {
        assert_eq!(
            cmp_loose_version("0.154.0-alpha.6.2", "0.147.0"),
            Ordering::Greater
        );
        assert_eq!(
            cmp_loose_version("0.154.0-alpha.6.2", "0.154.0"),
            Ordering::Less
        );
        assert_eq!(cmp_loose_version("0.154.0", "0.154.0"), Ordering::Equal);
        assert_eq!(cmp_loose_version("0.9.0", "0.10.0"), Ordering::Less);
    }

    /// 构造与本机气球线程一致的三页分叉夹具：根页尾部回合被第二页重录，
    /// 第三页又是对第二页尾部回合的重录（fork）。
    fn build_forked_fixture(root: &Path) -> (String, PathBuf) {
        let thread_id = "11111111-2222-3333-4444-555555555555";
        let page2_uuid = "aaaaaaaa-1111-2222-3333-444444444444";
        let page3_uuid = "bbbbbbbb-1111-2222-3333-444444444444";

        // 根页：ordinal 0..4，其中 2..4 是被第二页重录的尾部回合。
        let page1_lines = vec![
            meta_line(thread_id, 0, None),
            record_line(1, "user"),
            record_line(2, "user"),
            record_line(3, "assistant"),
            record_line(4, "assistant"),
        ];
        let page1_path = root.join(format!(
            "2026/09/14/rollout-2026-09-14T19-23-36-{thread_id}.jsonl"
        ));
        write_page(&page1_path, &page1_lines);
        let page1_size = fs::metadata(&page1_path).expect("size").len();
        // 挂接点取在尾部回合起点（ordinal 2 那行的起始字节）。
        let cut = page1_lines[0].len() as u64 + 1 + page1_lines[1].len() as u64 + 1;
        assert!(cut < page1_size);

        // 第二页：meta 占用 ordinal 2，正文 3..8（重录尾部回合 + 新内容）。
        let page2_lines = vec![
            meta_line(thread_id, 2, Some((thread_id, 2, cut))),
            record_line(3, "user"),
            record_line(4, "assistant"),
            record_line(5, "user"),
            record_line(6, "assistant"),
            record_line(7, "user"),
            record_line(8, "assistant"),
        ];
        let page2_path = root.join(format!(
            "2026/09/15/rollout-2026-09-15T12-40-02-{thread_id}_{page2_uuid}.jsonl"
        ));
        write_page(&page2_path, &page2_lines);

        // 第三页（fork）：对第二页 ordinal 5..8 回合的 5 秒重录。
        let page3_lines = vec![
            meta_line(thread_id, 5, Some((page2_uuid, 5, 0))),
            record_line(6, "user"),
            record_line(7, "assistant"),
        ];
        let page3_path = root.join(format!(
            "2026/09/16/rollout-2026-09-16T20-03-31-{thread_id}_{page3_uuid}.jsonl"
        ));
        write_page(&page3_path, &page3_lines);

        (thread_id.to_string(), page1_path)
    }

    #[test]
    fn scan_detects_orphaned_tail_and_writer_version() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-chain-scan-{}",
            uuid::Uuid::new_v4()
        ));
        let sessions = directory.join("sessions");
        let (thread_id, _) = build_forked_fixture(&sessions);

        let issues = scan_sessions_integrity(&sessions, Some("0.147.0"));
        let orphaned: Vec<_> = issues
            .iter()
            .filter(|issue| issue.kind == ISSUE_KIND_ORPHANED_TAIL)
            .collect();
        assert_eq!(orphaned.len(), 2, "两处挂接点都应检测出孤立段");
        assert!(orphaned.iter().all(|issue| issue.repairable));
        assert!(orphaned.iter().all(|issue| issue.thread_id == thread_id));
        // 根页孤立段 = 3 条记录（ordinal 2..4），含 1 条用户消息。
        let root_issue = orphaned
            .iter()
            .find(|issue| issue.orphaned_records == 3)
            .expect("root orphan issue");
        assert_eq!(root_issue.orphaned_user_messages, 1);

        let writer: Vec<_> = issues
            .iter()
            .filter(|issue| issue.kind == ISSUE_KIND_WRITER_NEWER)
            .collect();
        assert_eq!(writer.len(), 1);
        assert_eq!(writer[0].writer_cli_version.as_deref(), Some("0.154.0-alpha.6.2"));

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn repair_consolidates_forked_chain_into_single_file() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-chain-repair-{}",
            uuid::Uuid::new_v4()
        ));
        let sessions = directory.join("sessions");
        let backup_root = directory.join("backups");
        let (thread_id, page1_path) = build_forked_fixture(&sessions);

        let result = repair_thread_chain(&sessions, &thread_id, &backup_root, None)
            .expect("repair forked chain");
        assert_eq!(result.page_count, 3);
        // 保留：根页头部(ordinal 0..1) + 第二页正文(3..8) = 2 + 6 = 8 行；
        // 第三页正文 ordinal 6..7 均 <= 8，全部丢弃。
        assert_eq!(result.kept_records, 6);
        assert_eq!(result.skipped_records, 2);

        let merged = fs::read_to_string(&page1_path).expect("read merged file");
        let lines: Vec<&str> = merged.lines().collect();
        assert_eq!(lines.len(), 8);
        let meta = parse_page_meta(lines[0]).expect("merged meta");
        assert_eq!(meta.thread_id.as_deref(), Some(thread_id.as_str()));
        assert!(meta.history_base.is_none(), "合并结果不得再带 history_base");
        let ordinals: Vec<u64> = lines
            .iter()
            .map(|line| extract_ordinal(line).expect("ordinal"))
            .collect();
        assert_eq!(ordinals, vec![0, 1, 3, 4, 5, 6, 7, 8]);

        // 其余分页文件已被移走，备份目录里有全部 3 个原件。
        let remaining: Vec<_> = collect_pages(&sessions);
        assert_eq!(remaining.len(), 1);
        let backup_files: Vec<_> = fs::read_dir(&result.backup_dir)
            .expect("backup dir")
            .flatten()
            .collect();
        assert_eq!(backup_files.len(), 3);

        // 修复后再次扫描：不再有任何断链问题。
        let issues = scan_sessions_integrity(&sessions, None);
        assert!(issues.is_empty(), "修复后应无剩余问题: {issues:?}");

        let _ = fs::remove_dir_all(directory);
    }

    /// 状态库同步：threads 表里指着链尖分页的 rollout_path 必须改指合并后的根页，
    /// 否则 resume 时报"恢复对话失败: file does not exist"。
    #[test]
    fn repair_syncs_state_db_rollout_path() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-chain-state-{}",
            uuid::Uuid::new_v4()
        ));
        let sessions = directory.join("sessions");
        let backup_root = directory.join("backups");
        let codex_home = directory.join("codex-home");
        fs::create_dir_all(&codex_home).expect("codex home");
        let (thread_id, page1_path) = build_forked_fixture(&sessions);

        // 状态库 threads 表指向会被移走的链尖分页（第三页）。
        let page3_name = concat!(
            "rollout-2026-09-16T20-03-31-11111111-2222-3333-4444-555555555555_",
            "bbbbbbbb-1111-2222-3333-444444444444.jsonl"
        );
        let stale_path = sessions
            .join("2026/09/16")
            .join(page3_name)
            .to_string_lossy()
            .to_string();
        let db_path = codex_home.join("state_5.sqlite");
        {
            let connection = rusqlite::Connection::open(&db_path).expect("open state db");
            connection
                .execute(
                    "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL)",
                    [],
                )
                .expect("create threads table");
            connection
                .execute(
                    "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
                    rusqlite::params![thread_id, stale_path],
                )
                .expect("insert stale rollout_path");
        }

        let result = repair_thread_chain(
            &sessions,
            &thread_id,
            &backup_root,
            Some(&codex_home),
        )
        .expect("repair with state db");

        let connection = rusqlite::Connection::open(&db_path).expect("reopen state db");
        let stored: String = connection
            .query_row(
                "SELECT rollout_path FROM threads WHERE id = ?1",
                [thread_id],
                |row| row.get(0),
            )
            .expect("read rollout_path");
        let expected =
            canonical_rollout_path_for_state(&page1_path.to_string_lossy().to_string());
        assert_eq!(
            stored, expected,
            "rollout_path 应指向合并后的根页"
        );
        assert!(result.rollout_path_note.is_some(), "应记录同步说明");
        assert!(std::path::Path::new(&stored).exists() || cfg!(windows));

        let _ = fs::remove_dir_all(directory);
    }

    /// 切换预检的轻量校正：状态库指向已丢失的分页，但磁盘上有存活页时，
    /// 只把 rollout_path 改指存活链尖，不动任何会话文件。
    #[test]
    fn reconcile_fixes_stale_state_pointer_without_touching_files() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-chain-reconcile-{}",
            uuid::Uuid::new_v4()
        ));
        let sessions = directory.join("sessions");
        let codex_home = directory.join("codex-home");
        fs::create_dir_all(&codex_home).expect("codex home");
        let (thread_id, page1_path) = build_forked_fixture(&sessions);

        // 状态库指向一个已被移走的分页（磁盘上不存在）。
        let stale_path = sessions
            .join("2026/09/20")
            .join("rollout-2026-09-20T18-40-50-11111111-2222-3333-4444-555555555555_bbbbbbbb-1111-2222-3333-444444444444.jsonl")
            .to_string_lossy()
            .to_string();
        let db_path = codex_home.join("state_5.sqlite");
        {
            let connection = rusqlite::Connection::open(&db_path).expect("open state db");
            connection
                .execute(
                    "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL)",
                    [],
                )
                .expect("create threads table");
            connection
                .execute(
                    "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
                    rusqlite::params![thread_id, stale_path],
                )
                .expect("insert stale rollout_path");
        }

        // 记录修复前会话目录的文件集合与根页字节，用于验证"不动会话文件"。
        let root_bytes_before = fs::read(&page1_path).expect("read root before");
        let files_before = collect_rollout_files_under(&sessions);

        let (corrected, unrecoverable) = reconcile_state_rollout_paths(&sessions, &codex_home);

        assert_eq!(corrected, 1, "应校正该线程的状态库指针");
        assert!(unrecoverable.is_empty());
        // 会话文件未被改动。
        let root_bytes_after = fs::read(&page1_path).expect("read root after");
        assert_eq!(root_bytes_before, root_bytes_after, "根页内容不得变化");
        assert_eq!(files_before, collect_rollout_files_under(&sessions), "会话文件集合不得变化");
        // 状态库已指向存活链尖（链上最新的第三页）。
        let connection = rusqlite::Connection::open(&db_path).expect("reopen state db");
        let stored: String = connection
            .query_row(
                "SELECT rollout_path FROM threads WHERE id = ?1",
                [thread_id],
                |row| row.get(0),
            )
            .expect("read rollout_path");
        assert!(
            state_rollout_target_exists(&stored),
            "校正后的 rollout_path 必须指向真实存在的文件: {stored}"
        );

        let _ = fs::remove_dir_all(directory);
    }

    /// 状态库指向的文件仍存在时，轻量校正不应改动任何指针。
    #[test]
    fn reconcile_leaves_valid_state_pointer_alone() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-chain-reconcile-ok-{}",
            uuid::Uuid::new_v4()
        ));
        let sessions = directory.join("sessions");
        let codex_home = directory.join("codex-home");
        fs::create_dir_all(&codex_home).expect("codex home");
        let (thread_id, page1_path) = build_forked_fixture(&sessions);

        // 状态库已指向真实存在的根页。
        let valid_path = page1_path.to_string_lossy().to_string();
        let db_path = codex_home.join("state_5.sqlite");
        {
            let connection = rusqlite::Connection::open(&db_path).expect("open state db");
            connection
                .execute(
                    "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL)",
                    [],
                )
                .expect("create threads table");
            connection
                .execute(
                    "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
                    rusqlite::params![thread_id, valid_path],
                )
                .expect("insert valid rollout_path");
        }

        let (corrected, unrecoverable) = reconcile_state_rollout_paths(&sessions, &codex_home);
        assert_eq!(corrected, 0, "指针已有效时不应校正");
        assert!(unrecoverable.is_empty());

        let _ = fs::remove_dir_all(directory);
    }

    /// 递归收集目录下所有 rollout 文件的相对路径，用于断言"会话文件未被改动"。
    fn collect_rollout_files_under(root: &Path) -> Vec<String> {
        let mut files = Vec::new();
        collect_rollout_files(root, &mut files);
        let mut names: Vec<String> = files
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn repair_rejects_dangling_root() {
        let directory = std::env::temp_dir().join(format!(
            "agents-launcher-chain-dangling-{}",
            uuid::Uuid::new_v4()
        ));
        let sessions = directory.join("sessions");
        let thread_id = "99999999-8888-7777-6666-555555555555";
        // 唯一的页指向不存在的父页。
        let page_lines = vec![
            meta_line(
                thread_id,
                2,
                Some(("00000000-0000-0000-0000-000000000000", 2, 128)),
            ),
            record_line(3, "user"),
        ];
        write_page(
            &sessions.join(format!(
                "2026/09/16/rollout-2026-09-16T20-03-31-{thread_id}_aaaaaaaa-1111-2222-3333-444444444444.jsonl"
            )),
            &page_lines,
        );
        let error = repair_thread_chain(&sessions, thread_id, &directory.join("backups"), None)
            .expect_err("dangling root must fail");
        assert!(error.contains("父页") || error.contains("链"), "{error}");
        let _ = fs::remove_dir_all(directory);
    }
}
