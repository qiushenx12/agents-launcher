//! DeepSeek Harness (dsh) runtime supervisor.
//!
//! Unlike Claude Code / CodeX / OpenCode, dsh is not a foreground TUI driven by
//! a PTY: `npx --yes @deepseek-ai/dsh web` starts a long-lived HTTP server that
//! serves its own browser UI. This module owns that external process:
//!
//! 1. generate the `--patch` overlay that decides host + port,
//! 2. spawn `npx --yes @deepseek-ai/dsh web --patch <overlay> --port <n> --no-open`,
//! 3. read stdout until the ready line appears,
//! 4. keep the child alive until an explicit stop or application exit.
//!
//! Everything version- or format-sensitive lives in this file on purpose: the
//! ready-line grammar, the overlay template and the npx invocation are the
//! three things an unpinned dsh release can move out from under us.

use std::collections::VecDeque;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// Ready-line prefix printed by `dsh web` once the loader tree has settled.
const READY_PREFIX: &str = "dsh web: ";
const READY_LAN_MARKER: &str = " (LAN: ";

/// First start may have to download the whole package closure through npx.
const READY_TIMEOUT: Duration = Duration::from_secs(180);
/// How often the first-download sampler reads the npm cache directory.
const PROGRESS_SAMPLE_INTERVAL: Duration = Duration::from_millis(500);
/// Below this many bytes the npm cache is considered untouched, i.e. a hit.
const PROGRESS_ACTIVE_THRESHOLD_BYTES: u64 = 64 * 1024;
const STDERR_TAIL_LINES: usize = 20;
const STDERR_BUFFER_LINES: usize = 200;
/// Time allowed for the process tree to die and the port to come back.
const STOP_GRACE: Duration = Duration::from_secs(5);

const DEFAULT_PORT: u16 = 3080;

/// `npx` is always used: dsh's bin name is not on `PATH` unless the user
/// installed the package globally, and this feature must work without that.
const DSH_PACKAGE: &str = "@deepseek-ai/dsh";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DshAccess {
    Local,
    Remote,
}

impl DshAccess {
    fn parse(value: &str) -> Self {
        if value == "remote" {
            Self::Remote
        } else {
            Self::Local
        }
    }

    /// Address dsh binds to. `0.0.0.0` cannot be requested through the CLI:
    /// the dsh launcher rejects `--host 0.0.0.0` for safety, so the overlay is
    /// the only lever for "局域网可访问".
    fn bind_host(self) -> &'static str {
        match self {
            Self::Local => "127.0.0.1",
            Self::Remote => "0.0.0.0",
        }
    }

    /// Address used to *probe* the port. In remote mode the server itself binds
    /// `0.0.0.0`, so probing only loopback would miss an occupant bound to a
    /// single interface.
    fn probe_host(self) -> &'static str {
        self.bind_host()
    }

    fn overlay_host(self) -> &'static str {
        self.bind_host()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DshPhase {
    Stopped,
    Preparing,
    Starting,
    Running,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshRuntimeStatus {
    pub phase: DshPhase,
    pub access: DshAccess,
    pub port: u16,
    /// Result of `npx --yes @deepseek-ai/dsh -V`; `None` before the first probe.
    pub version: Option<String>,
    /// Resolved `npx` path, for diagnostics only.
    pub executable: Option<String>,
    /// User-facing Chinese description, rendered verbatim.
    pub message: String,
    /// Short machine-readable code for the failed phase:
    /// `port_in_use` / `npx_missing` / `spawn_failed` / `ready_timeout` / `exited`.
    pub issue: Option<String>,
    /// Sanitized tail of stderr, populated for failed starts only.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshPortStatus {
    pub available: bool,
    /// Identity of the listening process when the port is taken, as a neutral
    /// noun phrase (`node.exe（PID 21436）`) so every surface can phrase it the
    /// same way: «当前占用进程为 …».
    pub occupant: Option<String>,
    /// True when the occupant answers like a dsh web server.
    pub occupant_is_dsh: bool,
    /// True when the occupant is the dsh service this launcher supervises.
    pub occupant_is_supervised: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshRuntimeUrls {
    pub local_url: String,
    pub remote_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DshInstallProgress {
    bytes: u64,
    bytes_per_second: u64,
    elapsed_ms: u64,
    /// The cache is not growing, so the package came from the local cache.
    cached: bool,
}

// ---------------------------------------------------------------------------
// Process registry
// ---------------------------------------------------------------------------

struct DshServer {
    child: Child,
    access: DshAccess,
    port: u16,
    local_url: String,
    lan_url: Option<String>,
    version: Option<String>,
    executable: String,
    started_at: Instant,
}

#[derive(Default)]
struct DshRegistry {
    server: Option<DshServer>,
    /// Last failed status, rendered as the error card once the process is gone.
    failure: Option<DshRuntimeStatus>,
    /// Last requested configuration, so a stopped status can report the
    /// settings the user actually chose instead of hardcoded defaults.
    last_access: Option<DshAccess>,
    last_port: Option<u16>,
    /// Serialises start/stop so double-clicking 启动 cannot race two servers.
    busy: bool,
}

fn registry() -> &'static Mutex<DshRegistry> {
    static REGISTRY: OnceLock<Mutex<DshRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(DshRegistry::default()))
}

fn lock_registry() -> Result<MutexGuard<'static, DshRegistry>, String> {
    registry()
        .lock()
        .map_err(|_| "dsh 运行时注册表不可用".to_string())
}

fn app_data_dir() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|path| path.join("ClaudeEnvManager"))
        .ok_or_else(|| "无法确定应用数据目录".to_string())
}

fn runtime_dir() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("dsh"))
}

fn overlay_path() -> Result<PathBuf, String> {
    Ok(runtime_dir()?.join("runtime.overlay.yml"))
}

// ---------------------------------------------------------------------------
// Overlay
// ---------------------------------------------------------------------------

/// Whole-row replacement of the `webserver` config.
///
/// `port` keeps its `!!js` expression so the launcher's `--port` flag still
/// wins when the user changes it. The patch is the highest-precedence layer
/// (bundle → profile patch → home patch → `--patch`), which is exactly why
/// "本地 / 远程" can only be implemented by the launcher.
fn overlay_contents(access: DshAccess, port: u16) -> String {
    format!(
        "# Generated by Agents Launcher for this start; do not edit by hand.\n\
         # Mirrors the `webserver` row of the dsh web bundle patch. dsh replaces\n\
         # the whole `config` block, so new upstream fields must be added here.\n\
         - id: webserver\n\
         \x20 config:\n\
         \x20   host: '{host}'\n\
         \x20   port: !!js ctx.webStartup.port ?? {port}\n\
         \x20   compression: gzip\n\
         \x20   compressionLevel: 1\n\
         \x20   compressionThresholdBytes: 1024\n",
        host = access.overlay_host(),
        port = port,
    )
}

fn write_overlay(access: DshAccess, port: u16) -> Result<PathBuf, String> {
    let path = overlay_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建 dsh 运行时目录: {error}"))?;
    }
    std::fs::write(&path, overlay_contents(access, port))
        .map_err(|error| format!("无法写入 dsh 启动配置: {error}"))?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Ready line
// ---------------------------------------------------------------------------

/// Parse the single stdout line `dsh web` prints once it is serving.
///
/// Grammar: `dsh web: <local-url> (LAN: <lan-url>)` — the LAN segment is
/// absent in "本地" mode. Returns `(local_url, lan_url)`.
pub fn parse_ready_line(line: &str, expected_port: u16) -> Option<(String, Option<String>)> {
    let rest = line.strip_prefix(READY_PREFIX)?;
    let (local_raw, lan_raw) = match rest.split_once(READY_LAN_MARKER) {
        Some((local, lan)) => (local.trim(), Some(lan.trim().trim_end_matches(')').trim())),
        None => (rest.trim(), None),
    };

    let local = normalize_ready_url(local_raw, expected_port)?;
    let lan = match lan_raw {
        Some(raw) => Some(normalize_ready_url(raw, expected_port)?),
        None => None,
    };
    Some((local, lan))
}

/// Validate one URL from the ready line: it must parse, carry a `token` query
/// parameter, and report the port we asked for. A mismatching port means some
/// other instance answered, which must fail loudly rather than be trusted.
fn normalize_ready_url(raw: &str, expected_port: u16) -> Option<String> {
    let trimmed = raw.trim();
    let (scheme, without_scheme) = if let Some(rest) = trimmed.strip_prefix("http://") {
        ("http", rest)
    } else if let Some(rest) = trimmed.strip_prefix("https://") {
        ("https", rest)
    } else {
        return None;
    };
    let (authority, query) = match without_scheme.split_once('?') {
        Some((authority, query)) => (authority.trim_end_matches('/'), Some(query)),
        None => (without_scheme.trim_end_matches('/'), None),
    };
    if authority.is_empty() {
        return None;
    }
    if parse_authority_port(authority)? != expected_port {
        return None;
    }
    let token = query?
        .split('&')
        .find_map(|pair| pair.strip_prefix("token="))?;
    if token.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{authority}/?token={token}"))
}

fn parse_authority_port(authority: &str) -> Option<u16> {
    // IPv6 literals are bracketed: `[::1]:3080`.
    if let Some(rest) = authority.strip_prefix('[') {
        let (_, tail) = rest.split_once(']')?;
        return tail.strip_prefix(':')?.parse().ok();
    }
    authority.rsplit_once(':')?.1.parse().ok()
}

/// Replace every `token=…` value before diagnostics leave the process: the
/// token grants full access to this machine's agent and shell.
pub fn sanitize_diagnostic(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find("token=") {
        out.push_str(&rest[..index]);
        out.push_str("token=***");
        let tail = &rest[index + "token=".len()..];
        let end = tail
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '&' | ')' | '"' | '\'' | ',')
            })
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// npx resolution and command construction
// ---------------------------------------------------------------------------

fn locate_npx() -> Option<PathBuf> {
    crate::platform_env::locate_executable("npx")
}

fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
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

/// Arguments for `npx`. Launcher flags must precede app arguments: dsh's
/// launcher parses its own flags and hands *everything after the first unknown
/// token* to the app, so `--patch` after `--port` would reach the web app and
/// be rejected with `unknown option '--patch'`.
fn dsh_arguments(overlay: &Path, port: u16) -> Vec<String> {
    vec![
        "--yes".to_string(),
        DSH_PACKAGE.to_string(),
        "web".to_string(),
        "--patch".to_string(),
        overlay.to_string_lossy().to_string(),
        "--port".to_string(),
        port.to_string(),
        "--no-open".to_string(),
    ]
}

// ---------------------------------------------------------------------------
// Port probing
// ---------------------------------------------------------------------------

/// Connect-test used to detect a listener on `address`.
///
/// Binding cannot be used for this: Windows lets a wildcard socket
/// (`0.0.0.0:port`) and a concrete-address socket (`127.0.0.1:port`) coexist, so
/// a successful bind does not mean the port is actually free. A refused
/// connection means nothing is listening.
fn is_listening(address: SocketAddr) -> bool {
    TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_ok()
}

/// True when a listener would still accept a connection on `host:port`.
///
/// "本地" only needs loopback. "远程" binds `0.0.0.0`, which also serves the
/// loopback address, so both are probed: probing only loopback would miss an
/// occupant bound to a concrete interface address.
fn port_is_occupied(host: &str, port: u16) -> bool {
    if is_listening(SocketAddr::from(([127, 0, 0, 1], port))) {
        return true;
    }
    if host == "127.0.0.1" {
        return false;
    }
    // Sense the wildcard bind as well, without claiming the port for a caller
    // that forgets to drop the listener.
    let Ok(addresses) = (host, port).to_socket_addrs() else {
        return false;
    };
    !addresses
        .into_iter()
        .any(|address| TcpListener::bind(address).is_ok())
}

/// Ask `127.0.0.1:<port>` whether it is a dsh web server. A dsh server answers
/// the index route with 401 and a fixed sentence; anything else is a different
/// program, which deserves a different message.
fn port_occupant_is_dsh(port: u16) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    if TcpStream::connect_timeout(&address, Duration::from_millis(600)).is_err() {
        return false;
    }
    tauri::async_runtime::block_on(async move {
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_millis(1500))
            .build()
        else {
            return false;
        };
        match client.get(format!("http://127.0.0.1:{port}/")).send().await {
            Ok(response) => {
                if response.status().as_u16() != 401 {
                    return false;
                }
                response
                    .text()
                    .await
                    .map(|body| body.contains("dsh web authentication required"))
                    .unwrap_or(false)
            }
            Err(_) => false,
        }
    })
}

/// What is known about the process holding a port.
struct PortOccupant {
    /// Names the process: `node.exe（PID 21436）`.
    label: String,
    /// True when it answers like a dsh web server.
    is_dsh: bool,
    /// True when it is the dsh service this launcher started.
    is_supervised: bool,
}

impl PortOccupant {
    /// Sentence form, for the messages the backend owns (start failures).
    fn sentence(&self) -> String {
        format!("当前占用进程为 {}。", self.label)
    }
}

/// Describe the occupant without claiming where it came from.
///
/// The copy used to read «被另一个 dsh 实例占用» even when the occupant was the
/// service this very launcher had started — an application restart loses the
/// child handle, so `isRunning` is false while our own dsh still holds the port.
/// One neutral phrasing («当前占用进程为 …») plus the process identity is both
/// accurate and more actionable than guessing whether it is "another" instance.
fn describe_occupant(port: u16) -> PortOccupant {
    PortOccupant {
        label: occupant_label(&listening_processes(port)),
        is_dsh: port_occupant_is_dsh(port),
        is_supervised: supervised_listens_on(port),
    }
}

/// Neutral noun phrase for the first process listening on the port.
fn occupant_label(processes: &[DshPortProcess]) -> String {
    processes
        .first()
        .map(|process| format!("{}（PID {}）", process.name, process.pid))
        .unwrap_or_else(|| "未知进程".to_string())
}

// ---------------------------------------------------------------------------
// Occupant identification and release ("清理占用")
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshPortProcess {
    pub pid: u32,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshPortReleaseReport {
    /// True when nothing listens on the port any more.
    pub released: bool,
    pub killed: Vec<DshPortProcess>,
    /// Processes that could not be terminated, with the reason.
    pub failed: Vec<String>,
    /// Set when the request was refused because the occupant is this very
    /// process (or an ancestor), i.e. the app the user is clicking in.
    pub self_protected: bool,
    pub message: String,
}

/// PIDs listening on `port`, taken from the OS rather than guessed.
///
/// `netstat -ano` is available on every Windows install without extra tooling;
/// the POSIX branch uses `lsof`. Both are parsed defensively: an unexpected
/// line is skipped rather than aborting the lookup.
fn listening_processes(port: u16) -> Vec<DshPortProcess> {
    listening_pids(port)
        .into_iter()
        .map(|pid| DshPortProcess {
            pid,
            name: process_name(pid),
        })
        .collect()
}

fn listening_pids(port: u16) -> Vec<u32> {
    #[cfg(windows)]
    {
        let mut pids: Vec<u32> = Vec::new();
        let Ok(output) = hidden_command("netstat").args(["-ano"]).output() else {
            return pids;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            // Proto, Local Address, Foreign Address, State, PID
            if fields.len() < 5 || !fields[0].eq_ignore_ascii_case("tcp") {
                continue;
            }
            if !fields[3].eq_ignore_ascii_case("listening") {
                continue;
            }
            if authority_port(fields[1]) != Some(port) {
                continue;
            }
            if let Ok(pid) = fields[4].parse::<u32>() {
                if pid != 0 && !pids.contains(&pid) {
                    pids.push(pid);
                }
            }
        }
        pids
    }

    #[cfg(not(windows))]
    {
        let mut pids: Vec<u32> = Vec::new();
        let Ok(output) = hidden_command("lsof")
            .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-t"])
            .output()
        else {
            return pids;
        };
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Ok(pid) = line.trim().parse::<u32>() {
                if pid != 0 && !pids.contains(&pid) {
                    pids.push(pid);
                }
            }
        }
        pids
    }
}

/// `host:port` or `[::]:port` → port. Bracketed IPv6 is handled explicitly
/// because it also contains colons.
fn authority_port(authority: &str) -> Option<u16> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (_, tail) = rest.split_once(']')?;
        return tail.strip_prefix(':')?.parse().ok();
    }
    authority.rsplit_once(':')?.1.parse().ok()
}

fn process_name(pid: u32) -> String {
    #[cfg(windows)]
    {
        // `tasklist` is the dependency-free way to map a PID to an image name.
        let Ok(output) = hidden_command("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
        else {
            return "未知进程".to_string();
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let Some(first) = text.lines().find(|line| line.starts_with('"')) else {
            return "未知进程".to_string();
        };
        return first
            .split("\",\"")
            .next()
            .map(|name| name.trim_start_matches('"').to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "未知进程".to_string());
    }
    #[cfg(not(windows))]
    {
        let Ok(output) = hidden_command("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
        else {
            return "未知进程".to_string();
        };
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if name.is_empty() {
            "未知进程".to_string()
        } else {
            name
        }
    }
}

/// Terminate a process tree by PID (`taskkill /T /F` on Windows, a process
/// group SIGTERM → SIGKILL escalation on Unix). `npx` spawns `cmd.exe` → `node`,
/// so killing only the owner PID would leave a live listener behind.
fn terminate_pid(pid: u32) -> Result<(), String> {
    #[cfg(windows)]
    {
        let output = hidden_command("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .output()
            .map_err(|error| format!("无法结束 PID {pid}: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        Err(if detail.is_empty() {
            format!("无法结束 PID {pid}（可能需要管理员权限）。")
        } else {
            format!("无法结束 PID {pid}：{detail}")
        })
    }
    #[cfg(not(windows))]
    {
        let group = pid as i32;
        unsafe {
            libc::kill(-group, libc::SIGTERM);
        }
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(25));
            if unsafe { libc::kill(-group, 0) } != 0 {
                return Ok(());
            }
        }
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
        Ok(())
    }
}

/// This process and every ancestor of it.
///
/// The occupant can legitimately be the app the user is clicking in — a second
/// dsh instance may have been started from an Agents Launcher terminal, and the
/// process running this command must never be terminated.
fn self_process_chain() -> Vec<u32> {
    #[cfg(windows)]
    {
        let mut chain = vec![std::process::id()];
        let Ok(output) = hidden_command("wmic")
            .args(["process", "get", "ProcessId,ParentProcessId", "/format:csv"])
            .output()
        else {
            return chain;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let mut parents: Vec<(u32, u32)> = Vec::new();
        for line in text.lines().skip(1) {
            let fields: Vec<&str> = line.split(',').collect();
            // Node, ParentProcessId, ProcessId
            if fields.len() < 3 {
                continue;
            }
            if let (Ok(parent), Ok(pid)) =
                (fields[1].trim().parse::<u32>(), fields[2].trim().parse::<u32>())
            {
                parents.push((pid, parent));
            }
        }
        let mut current = std::process::id();
        for _ in 0..32 {
            let Some((_, parent)) = parents.iter().find(|(pid, _)| *pid == current) else {
                break;
            };
            if *parent == 0 || chain.contains(parent) {
                break;
            }
            chain.push(*parent);
            current = *parent;
        }
        chain
    }
    #[cfg(not(windows))]
    {
        let mut chain = vec![std::process::id()];
        let mut current = std::process::id();
        for _ in 0..32 {
            let Ok(output) = hidden_command("ps")
                .args(["-p", &current.to_string(), "-o", "ppid="])
                .output()
            else {
                break;
            };
            let Ok(parent) = String::from_utf8_lossy(&output.stdout).trim().parse::<u32>() else {
                break;
            };
            if parent == 0 || parent == 1 || chain.contains(&parent) {
                break;
            }
            chain.push(parent);
            current = parent;
        }
        chain
    }
}

/// Kill whatever holds `port` so a managed `dsh web` can bind it.
///
/// Refuses to touch this process or any ancestor: the occupant may be the app
/// the request came from, and the user is talking to the agent through it.
fn release_port(port: u16) -> DshPortReleaseReport {
    if port_is_occupied(DshAccess::Local.probe_host(), port)
        && crate::dsh_runtime::supervised_listens_on(port)
    {
        // Managed by this process: `dsh_runtime_stop` is the correct action, and
        // killing the tracked child directly would desynchronise the registry.
        return DshPortReleaseReport {
            released: false,
            killed: Vec::new(),
            failed: Vec::new(),
            self_protected: false,
            message: "该端口正由启动器托管的 dsh 服务占用，请点击「关闭」而不是清理。".to_string(),
        };
    }

    let processes = listening_processes(port);
    if processes.is_empty() {
        return DshPortReleaseReport {
            released: true,
            killed: Vec::new(),
            failed: Vec::new(),
            self_protected: false,
            message: format!("端口 {port} 当前没有被占用。"),
        };
    }

    let protected = self_process_chain();
    let mut killed = Vec::new();
    let mut failed = Vec::new();
    let mut self_protected = false;

    for process in processes {
        if protected.contains(&process.pid) {
            self_protected = true;
            failed.push(format!(
                "{}（PID {}）是启动器自身进程或它的父进程，已跳过。",
                process.name, process.pid
            ));
            continue;
        }
        match terminate_pid(process.pid) {
            Ok(()) => killed.push(process),
            Err(error) => failed.push(error),
        }
    }

    // Killing is not the same as releasing: verify instead of claiming success.
    let deadline = Instant::now() + STOP_GRACE;
    let mut released = false;
    while Instant::now() < deadline {
        if !port_is_occupied(DshAccess::Local.probe_host(), port) {
            released = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let message = if released && failed.is_empty() {
        let names = killed
            .iter()
            .map(|process| format!("{}（PID {}）", process.name, process.pid))
            .collect::<Vec<_>>()
            .join("、");
        format!("已结束占用端口 {port} 的进程：{names}。端口已释放。")
    } else if released {
        format!(
            "端口 {port} 已释放，但有 {} 项未能处理：{}",
            failed.len(),
            failed.join("；")
        )
    } else if self_protected {
        format!(
            "已跳过启动器自身进程，端口 {port} 仍被占用。请手动关闭占用它的程序。"
        )
    } else {
        format!(
            "端口 {port} 仍被占用。{}",
            if failed.is_empty() {
                "占用者可能被其它程序自动重启。".to_string()
            } else {
                failed.join("；")
            }
        )
    };

    DshPortReleaseReport {
        released,
        killed,
        failed,
        self_protected,
        message,
    }
}

/// True when the supervised child already listens on `port`.
fn supervised_listens_on(port: u16) -> bool {
    lock_registry()
        .map(|guard| guard.server.as_ref().is_some_and(|server| server.port == port))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// npm cache sampling (first-download progress)
// ---------------------------------------------------------------------------

fn npm_cache_dir() -> Option<PathBuf> {
    if let Ok(output) = hidden_command("npm")
        .args(["config", "get", "cache"])
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() && path != "undefined" {
                return Some(PathBuf::from(path));
            }
        }
    }
    dirs::cache_dir().map(|path| path.join("npm"))
}

fn directory_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total += directory_size(&entry.path());
        } else {
            total += metadata.len();
        }
    }
    total
}

/// Emit `dsh_install_progress` from the moment npx starts until the ready line
/// arrives (or the sampler is switched off again).
fn spawn_progress_sampler(app: AppHandle, cache_dir: PathBuf, active: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let started_at = Instant::now();
        let baseline = directory_size(&cache_dir);
        let mut previous_bytes = 0_u64;
        let mut cached = false;
        while active.load(Ordering::Relaxed) {
            std::thread::sleep(PROGRESS_SAMPLE_INTERVAL);
            if !active.load(Ordering::Relaxed) {
                return;
            }
            let bytes = directory_size(&cache_dir).saturating_sub(baseline);
            let growth = bytes.saturating_sub(previous_bytes);
            previous_bytes = bytes;
            if growth >= PROGRESS_ACTIVE_THRESHOLD_BYTES {
                cached = false;
            } else if bytes < PROGRESS_ACTIVE_THRESHOLD_BYTES
                && started_at.elapsed() > Duration::from_secs(3)
            {
                cached = true;
            }
            let _ = app.emit(
                "dsh_install_progress",
                DshInstallProgress {
                    bytes,
                    bytes_per_second: growth * (1000 / PROGRESS_SAMPLE_INTERVAL.as_millis() as u64),
                    elapsed_ms: started_at.elapsed().as_millis() as u64,
                    cached,
                },
            );
        }
    });
}

// ---------------------------------------------------------------------------
// Spawn and wait for readiness
// ---------------------------------------------------------------------------

struct SpawnOutcome {
    child: Child,
    local_url: String,
    lan_url: Option<String>,
}

type SpawnFailure = (String, String, Option<String>);

fn spawn_and_wait(
    app: &AppHandle,
    npx: &Path,
    overlay: &Path,
    port: u16,
) -> Result<SpawnOutcome, SpawnFailure> {
    let mut command = hidden_command(npx);
    command
        .args(dsh_arguments(overlay, port))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|error| {
        (
            "spawn_failed".to_string(),
            format!("无法启动 dsh: {error}"),
            None,
        )
    })?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let (line_tx, line_rx): (mpsc::Sender<String>, Receiver<String>) = mpsc::channel();
    let stderr_tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));

    if let Some(stdout) = stdout {
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line_tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    if let Some(stderr) = stderr {
        let tail = stderr_tail.clone();
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if let Ok(mut guard) = tail.lock() {
                    guard.push_back(line);
                    while guard.len() > STDERR_BUFFER_LINES {
                        guard.pop_front();
                    }
                }
            }
        });
    }

    let progress_active = Arc::new(AtomicBool::new(true));
    if let Some(cache_dir) = npm_cache_dir() {
        spawn_progress_sampler(app.clone(), cache_dir, progress_active.clone());
    }

    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            progress_active.store(false, Ordering::Relaxed);
            let detail = stderr_summary(&stderr_tail);
            terminate_child(&mut child);
            return Err((
                "ready_timeout".to_string(),
                format!(
                    "启动 dsh 超时（{} 秒内未就绪）。首次运行需要下载依赖，请检查网络与代理后重试。",
                    READY_TIMEOUT.as_secs()
                ),
                detail,
            ));
        }

        match line_rx.recv_timeout(remaining.min(Duration::from_millis(500))) {
            Ok(line) => {
                if let Some((local_url, lan_url)) = parse_ready_line(&line, port) {
                    progress_active.store(false, Ordering::Relaxed);
                    return Ok(SpawnOutcome {
                        child,
                        local_url,
                        lan_url,
                    });
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                progress_active.store(false, Ordering::Relaxed);
                let detail = stderr_summary(&stderr_tail);
                let reason = exit_reason(&mut child);
                return Err((
                    "exited".to_string(),
                    format!("dsh 进程在就绪前退出（{reason}）。"),
                    detail,
                ));
            }
        }

        if let Ok(Some(status)) = child.try_wait() {
            progress_active.store(false, Ordering::Relaxed);
            let detail = stderr_summary(&stderr_tail);
            return Err((
                "exited".to_string(),
                format!("dsh 进程在就绪前退出（{}）。", describe_exit_status(&status)),
                detail,
            ));
        }
    }
}

fn describe_exit_status(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("退出码 {code}"),
        None => "被信号终止".to_string(),
    }
}

fn exit_reason(child: &mut Child) -> String {
    match child.try_wait() {
        Ok(Some(status)) => describe_exit_status(&status),
        _ => "输出流已关闭".to_string(),
    }
}

fn stderr_summary(tail: &Arc<Mutex<VecDeque<String>>>) -> Option<String> {
    let guard = tail.lock().ok()?;
    if guard.is_empty() {
        return None;
    }
    let lines: Vec<String> = guard
        .iter()
        .rev()
        .take(STDERR_TAIL_LINES)
        .rev()
        .map(|line| sanitize_diagnostic(line))
        .collect();
    Some(lines.join("\n"))
}

/// Terminate the whole process tree. npx spawns `cmd.exe` → `node` → dsh's own
/// children, so killing only the direct child leaves a live listener behind.
fn terminate_child(child: &mut Child) {
    let pid = child.id();
    #[cfg(windows)]
    {
        let mut command = hidden_command("taskkill");
        command
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = command.output();
    }
    #[cfg(unix)]
    {
        let group = pid as i32;
        // `npx` is the direct child and the process-group leader here, so the
        // whole tree can be signalled at once.
        unsafe {
            libc::kill(-group, libc::SIGTERM);
        }
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(25));
            if unsafe { libc::kill(-group, 0) } != 0 {
                break;
            }
        }
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn read_version(npx: &Path) -> Option<String> {
    let output = hidden_command(npx)
        .args(["--yes", DSH_PACKAGE, "-V"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    text.lines()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && line
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_ascii_digit())
        })
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Start / stop / status
// ---------------------------------------------------------------------------

fn failure_status(
    access: DshAccess,
    port: u16,
    issue: &str,
    message: &str,
    detail: Option<String>,
) -> DshRuntimeStatus {
    DshRuntimeStatus {
        phase: DshPhase::Failed,
        access,
        port,
        version: None,
        executable: locate_npx().map(|path| path.to_string_lossy().to_string()),
        message: message.to_string(),
        issue: Some(issue.to_string()),
        detail,
    }
}

fn stopped_status(access: DshAccess, port: u16) -> DshRuntimeStatus {
    DshRuntimeStatus {
        phase: DshPhase::Stopped,
        access,
        port,
        version: None,
        executable: locate_npx().map(|path| path.to_string_lossy().to_string()),
        message: "DeepSeek Harness 未启动。".to_string(),
        issue: None,
        detail: None,
    }
}

/// Start (or reuse) the managed server. Runs while holding no registry lock, so
/// it must be called with `busy` already set.
fn start_server(app: &AppHandle, access: DshAccess, port: u16) -> DshRuntimeStatus {
    let Some(npx) = locate_npx() else {
        return failure_status(
            access,
            port,
            "npx_missing",
            "未检测到 npx。请先安装 Node.js（含 npm/npx）后重试。",
            None,
        );
    };
    let executable = npx.to_string_lossy().to_string();

    if port_is_occupied(access.probe_host(), port) {
        let occupant = describe_occupant(port);
        let message = format!(
            "端口 {port} 已被占用：{}可改用其它端口，或在配置页点「一键清理占用」。",
            occupant.sentence()
        );
        return failure_status(access, port, "port_in_use", &message, None);
    }

    let overlay = match write_overlay(access, port) {
        Ok(path) => path,
        Err(error) => return failure_status(access, port, "spawn_failed", &error, None),
    };

    let outcome = match spawn_and_wait(app, &npx, &overlay, port) {
        Ok(outcome) => outcome,
        Err((issue, message, detail)) => {
            return failure_status(access, port, &issue, &message, detail)
        }
    };

    let version = read_version(&npx);
    let SpawnOutcome {
        child,
        local_url,
        lan_url,
    } = outcome;

    let mut guard = match lock_registry() {
        Ok(guard) => guard,
        Err(error) => return failure_status(access, port, "spawn_failed", &error, None),
    };
    guard.failure = None;
    guard.server = Some(DshServer {
        child,
        access,
        port,
        local_url,
        lan_url: lan_url.clone(),
        version: version.clone(),
        executable: executable.clone(),
        started_at: Instant::now(),
    });
    DshRuntimeStatus {
        phase: DshPhase::Running,
        access,
        port,
        version,
        executable: Some(executable),
        message: match lan_url {
            Some(_) => "DeepSeek Harness 正在运行，局域网内其它设备也可以访问。".to_string(),
            None => "DeepSeek Harness 正在运行，仅本机可访问。".to_string(),
        },
        issue: None,
        detail: None,
    }
}

/// Stop the managed server, then verify the port was actually released instead
/// of claiming success.
fn stop_server() -> DshRuntimeStatus {
    let mut guard = match lock_registry() {
        Ok(guard) => guard,
        Err(error) => return failure_status(DshAccess::Local, DEFAULT_PORT, "spawn_failed", &error, None),
    };
    let Some(mut server) = guard.server.take() else {
        let access = guard.last_access.unwrap_or(DshAccess::Local);
        let port = guard.last_port.unwrap_or(DEFAULT_PORT);
        guard.failure = None;
        return stopped_status(access, port);
    };
    let access = server.access;
    let port = server.port;
    terminate_child(&mut server.child);

    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        if !port_is_occupied(access.probe_host(), port) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    guard.failure = None;
    stopped_status(access, port)
}

fn live_status(guard: &mut DshRegistry) -> Option<DshRuntimeStatus> {
    let server = guard.server.as_mut()?;
    let (access, port) = (server.access, server.port);
    match server.child.try_wait() {
        Ok(None) => Some(DshRuntimeStatus {
            phase: DshPhase::Running,
            access,
            port,
            version: server.version.clone(),
            executable: Some(server.executable.clone()),
            message: format!(
                "DeepSeek Harness 正在运行（已运行 {} 秒）。",
                server.started_at.elapsed().as_secs()
            ),
            issue: None,
            detail: None,
        }),
        Ok(Some(status)) => {
            guard.server = None;
            let status = DshRuntimeStatus {
                phase: DshPhase::Failed,
                access,
                port,
                version: None,
                executable: None,
                message: format!("dsh 服务已意外退出（{}）。", describe_exit_status(&status)),
                issue: Some("exited".to_string()),
                detail: None,
            };
            guard.failure = Some(status.clone());
            Some(status)
        }
        Err(error) => Some(DshRuntimeStatus {
            phase: DshPhase::Failed,
            access,
            port,
            version: None,
            executable: None,
            message: format!("无法查询 dsh 进程状态: {error}"),
            issue: Some("exited".to_string()),
            detail: None,
        }),
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn dsh_runtime_status() -> Result<DshRuntimeStatus, String> {
    let mut guard = lock_registry()?;
    if let Some(status) = live_status(&mut guard) {
        return Ok(status);
    }
    if let Some(failure) = guard.failure.clone() {
        return Ok(failure);
    }
    Ok(stopped_status(
        guard.last_access.unwrap_or(DshAccess::Local),
        guard.last_port.unwrap_or(DEFAULT_PORT),
    ))
}

/// URLs are served separately from the polled status so the token-bearing URLs
/// are only fetched when the UI actually needs them.
#[tauri::command]
pub fn dsh_runtime_urls() -> Result<Option<DshRuntimeUrls>, String> {
    let guard = lock_registry()?;
    Ok(guard.server.as_ref().map(|server| DshRuntimeUrls {
        local_url: server.local_url.clone(),
        remote_url: server.lan_url.clone(),
    }))
}

#[tauri::command]
pub async fn dsh_runtime_start(
    app: AppHandle,
    access: String,
    port: u16,
) -> Result<DshRuntimeStatus, String> {
    if port == 0 {
        return Err("端口必须在 1–65535 之间。".to_string());
    }
    let access = DshAccess::parse(&access);
    tauri::async_runtime::spawn_blocking(move || {
        {
            let mut guard = lock_registry()?;
            if guard.busy {
                return Err("dsh 正在启动或停止，请稍候。".to_string());
            }
            if let Some(server) = guard.server.as_ref() {
                if server.access == access && server.port == port {
                    if let Some(status) = live_status(&mut guard) {
                        if status.phase == DshPhase::Running {
                            return Ok(status);
                        }
                    }
                }
            }
            guard.busy = true;
            guard.last_access = Some(access);
            guard.last_port = Some(port);
        }
        // The registry lock must NOT be held while the child starts: the start
        // can wait up to 180s for the first npx download, and the UI thread
        // calls into the registry (`dsh_embed_show`) while it runs. Only the
        // `busy` flag is needed to keep a second start out.
        let status = start_server(&app, access, port);
        finish_start(status)
    })
    .await
    .map_err(|error| format!("dsh 启动任务异常结束: {error}"))?
}

/// Clear the `busy` flag after a start attempt and remember a failure so the
/// next `dsh_runtime_status` still renders the error card.
fn finish_start(status: DshRuntimeStatus) -> Result<DshRuntimeStatus, String> {
    if let Ok(mut guard) = lock_registry() {
        guard.busy = false;
        if status.phase == DshPhase::Failed {
            guard.failure = Some(status.clone());
        }
    }
    Ok(status)
}

#[tauri::command]
pub async fn dsh_runtime_stop() -> Result<DshRuntimeStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        {
            let mut guard = lock_registry()?;
            if guard.busy {
                return Err("dsh 正在启动或停止，请稍候。".to_string());
            }
            guard.busy = true;
        }
        // Same reasoning as `dsh_runtime_start`: the lock is released before the
        // process tree is killed and the port is re-probed.
        let status = stop_server();
        if let Ok(mut guard) = lock_registry() {
            guard.busy = false;
        }
        Ok(status)
    })
    .await
    .map_err(|error| format!("dsh 停止任务异常结束: {error}"))?
}

#[tauri::command]
pub async fn dsh_check_port(access: Option<String>, port: u16) -> Result<DshPortStatus, String> {
    let access = DshAccess::parse(access.as_deref().unwrap_or("local"));
    tauri::async_runtime::spawn_blocking(move || {
        if !port_is_occupied(access.probe_host(), port) {
            return DshPortStatus {
                available: true,
                occupant: None,
                occupant_is_dsh: false,
                occupant_is_supervised: false,
            };
        }
        let occupant = describe_occupant(port);
        DshPortStatus {
            available: false,
            occupant: Some(occupant.label),
            occupant_is_dsh: occupant.is_dsh,
            occupant_is_supervised: occupant.is_supervised,
        }
    })
    .await
    .map_err(|error| format!("dsh 端口探测异常结束: {error}"))
}

/// One-click cleanup for a port another process holds.
///
/// Destructive by design, so the frontend must confirm first. Two refusals are
/// built in: a port served by the supervised child is stopped through
/// `dsh_runtime_stop` instead, and this process (or any ancestor) is never
/// killed.
#[tauri::command]
pub async fn dsh_release_port(port: u16) -> Result<DshPortReleaseReport, String> {
    tauri::async_runtime::spawn_blocking(move || release_port(port))
        .await
        .map_err(|error| format!("清理端口任务异常结束: {error}"))
}

/// Stop the managed server during application shutdown.
pub fn stop() {
    let Ok(mut guard) = registry().lock() else {
        return;
    };
    if let Some(mut server) = guard.server.take() {
        terminate_child(&mut server.child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_switches_host_without_losing_the_port_expression() {
        let local = overlay_contents(DshAccess::Local, 3080);
        assert!(local.contains("host: '127.0.0.1'"));
        assert!(local.contains("port: !!js ctx.webStartup.port ?? 3080"));
        assert!(local.contains("- id: webserver"));

        let remote = overlay_contents(DshAccess::Remote, 3199);
        assert!(remote.contains("host: '0.0.0.0'"));
        assert!(remote.contains("port: !!js ctx.webStartup.port ?? 3199"));
    }

    #[test]
    fn overlay_replaces_the_whole_webserver_config_block() {
        let overlay = overlay_contents(DshAccess::Local, 3080);
        for key in [
            "compression: gzip",
            "compressionLevel: 1",
            "compressionThresholdBytes: 1024",
        ] {
            assert!(overlay.contains(key), "overlay should keep {key}");
        }
    }

    #[test]
    fn ready_line_parses_local_only() {
        let line = "dsh web: http://127.0.0.1:3080/?token=abc.def-ghi";
        let (local, lan) = parse_ready_line(line, 3080).expect("ready line should parse");
        assert_eq!(local, "http://127.0.0.1:3080/?token=abc.def-ghi");
        assert!(lan.is_none());
    }

    #[test]
    fn ready_line_parses_lan_segment() {
        let line = "dsh web: http://127.0.0.1:3080/?token=t1 (LAN: http://192.168.1.5:3080/?token=t1)";
        let (local, lan) = parse_ready_line(line, 3080).expect("ready line should parse");
        assert_eq!(local, "http://127.0.0.1:3080/?token=t1");
        assert_eq!(lan.as_deref(), Some("http://192.168.1.5:3080/?token=t1"));
    }

    #[test]
    fn ready_line_rejects_other_stdout_lines() {
        for line in [
            "dsh web: opening the default browser…",
            "loader ready",
            "dsh web: http://127.0.0.1:3080/",
            "dsh web: http://127.0.0.1:3080/?token=",
            "",
        ] {
            assert!(
                parse_ready_line(line, 3080).is_none(),
                "{line:?} must not be treated as ready"
            );
        }
    }

    #[test]
    fn ready_line_rejects_a_port_mismatch() {
        let line = "dsh web: http://127.0.0.1:3199/?token=abc";
        assert!(parse_ready_line(line, 3080).is_none());
    }

    #[test]
    fn ready_line_normalizes_a_missing_path() {
        let line = "dsh web: http://127.0.0.1:3080?token=abc";
        let (local, _) = parse_ready_line(line, 3080).expect("ready line should parse");
        assert_eq!(local, "http://127.0.0.1:3080/?token=abc");
    }

    #[test]
    fn diagnostics_never_carry_a_token() {
        let raw = "visit http://127.0.0.1:3080/?token=super.secret-value&x=1 or (LAN: http://10.0.0.2:3080/?token=other)";
        let sanitized = sanitize_diagnostic(raw);
        assert!(!sanitized.contains("super.secret-value"));
        assert!(!sanitized.contains("other"));
        assert!(sanitized.contains("token=***"));
    }

    #[test]
    fn dsh_arguments_put_launcher_flags_before_app_flags() {
        let args = dsh_arguments(Path::new("/tmp/overlay.yml"), 3199);
        assert_eq!(
            args,
            vec![
                "--yes",
                "@deepseek-ai/dsh",
                "web",
                "--patch",
                "/tmp/overlay.yml",
                "--port",
                "3199",
                "--no-open",
            ]
        );
    }

    #[test]
    fn access_maps_to_bind_and_probe_hosts() {
        assert_eq!(DshAccess::parse("local").bind_host(), "127.0.0.1");
        assert_eq!(DshAccess::parse("remote").bind_host(), "0.0.0.0");
        assert_eq!(DshAccess::parse("remote").probe_host(), "0.0.0.0");
        assert_eq!(DshAccess::Local.overlay_host(), "127.0.0.1");
        // Anything other than the literal "remote" must fall back to the safe,
        // loopback-only default.
        assert_eq!(DshAccess::parse("bogus"), DshAccess::Local);
        assert_eq!(DshAccess::parse("REMOTE"), DshAccess::Local);
    }

    #[test]
    fn a_free_port_reports_available() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener);
        assert!(!port_is_occupied("127.0.0.1", port));
        assert!(!port_is_occupied("0.0.0.0", port));
    }

    #[test]
    fn a_loopback_listener_is_detected_in_both_modes() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        assert!(port_is_occupied("127.0.0.1", port));
        // 远程 mode must still refuse to start when something already serves
        // that port on loopback.
        assert!(port_is_occupied("0.0.0.0", port));
    }

    #[test]
    fn a_wildcard_listener_is_detected() {
        let listener = TcpListener::bind("0.0.0.0:0").expect("bind ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        assert!(port_is_occupied("0.0.0.0", port));
        assert!(port_is_occupied("127.0.0.1", port));
    }

    #[test]
    fn netstat_local_address_ports_are_parsed() {
        assert_eq!(authority_port("127.0.0.1:3080"), Some(3080));
        assert_eq!(authority_port("0.0.0.0:3080"), Some(3080));
        assert_eq!(authority_port("[::]:3080"), Some(3080));
        assert_eq!(authority_port("[::1]:3199"), Some(3199));
        assert_eq!(authority_port("127.0.0.1:"), None);
        assert_eq!(authority_port("garbage"), None);
        assert_eq!(authority_port(""), None);
    }

    /// The occupant lookup must return the process actually listening on the
    /// tested port, so "清理占用" kills the right thing.
    #[test]
    fn the_listening_process_of_a_test_listener_is_identified() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        let processes = listening_processes(port);
        // The lookup shells out to netstat/tasklist; on a restricted host it can
        // legitimately come back empty, but when it answers it must be us.
        if let Some(found) = processes.first() {
            assert_eq!(
                found.pid,
                std::process::id(),
                "the owner of a test listener is this test binary"
            );
            assert!(!found.name.is_empty());
        } else {
            eprintln!("[dsh] occupant lookup unavailable on this host; skipping assertion");
        }
    }

    /// The occupancy copy must not guess where the occupant came from.
    ///
    /// It used to read «被另一个 dsh 实例占用» even when the occupant was the
    /// service this launcher had started (an app restart loses the child handle,
    /// so `isRunning` is false while our own dsh still holds the port). The
    /// description is now just the process, and every surface phrases it as
    /// «当前占用进程为 …».
    #[test]
    fn the_occupant_description_names_the_process_without_guessing_its_origin() {
        let label = occupant_label(&[DshPortProcess {
            pid: 21436,
            name: "node.exe".to_string(),
        }]);
        assert_eq!(label, "node.exe（PID 21436）");
        assert!(!label.contains("另一个"));
        assert!(!label.contains("其它"));
        assert_eq!(occupant_label(&[]), "未知进程");
        assert_eq!(
            PortOccupant {
                label,
                is_dsh: true,
                is_supervised: true,
            }
            .sentence(),
            "当前占用进程为 node.exe（PID 21436）。"
        );
    }

    /// The occupant of a free port must not be reported at all.
    #[test]
    fn a_free_port_has_no_listening_process() {        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener);
        assert!(listening_processes(port).is_empty());
    }

    /// The app must never terminate itself: the process chain always contains
    /// this process, and a self-held port is refused rather than killed.
    #[test]
    fn the_self_process_chain_protects_this_process() {
        let chain = self_process_chain();
        assert!(chain.contains(&std::process::id()));

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        let report = release_port(port);
        assert!(!report.released, "the port is still held by this process");
        assert!(report.self_protected, "self-protection must trip");
        assert!(report.killed.is_empty(), "nothing may be killed");
        // The listener must still work after the refused release.
        assert!(TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(500)
        )
        .is_ok());
    }

    #[test]
    fn releasing_an_unoccupied_port_reports_success_without_killing() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener);
        let report = release_port(port);
        assert!(report.released);
        assert!(report.killed.is_empty());
        assert!(report.failed.is_empty());
        assert!(!report.self_protected);
        assert!(report.message.contains(&port.to_string()));
    }
}
