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

use std::collections::{BTreeMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// Ready-line prefix printed by `dsh web` once the loader tree has settled.
const READY_PREFIX: &str = "dsh web: ";
const READY_LAN_MARKER: &str = " (LAN: ";

/// First start may have to download the whole package closure through npx.
///
/// This bounds *staleness*, not the whole attempt: as long as the download is
/// demonstrably still growing the wait goes on (see [`ready_deadline`]).
/// Interrupting a healthy download at 180s throws away everything already
/// transferred and leaves the user to start over, which is worse than waiting.
const READY_TIMEOUT: Duration = Duration::from_secs(180);
/// A cache hit downloads nothing, so there is no progress to wait for: dsh
/// either prints its ready line quickly or something is wedged (a damaged
/// `_npx` entry is the known case). Give it far less rope than a real
/// download — the sooner this surfaces, the sooner the corrupt-cache
/// self-repair can clean the entry and retry.
const READY_TIMEOUT_CACHED: Duration = Duration::from_secs(30);
/// Absolute ceiling for one start attempt, however briskly it is downloading.
/// Without it a connection that trickles for hours would hold the `busy` flag
/// and the 「正在启动」 state forever.
const READY_TIMEOUT_CEILING: Duration = Duration::from_secs(30 * 60);
/// How often the first-download sampler reads the npm cache directory.
const PROGRESS_SAMPLE_INTERVAL: Duration = Duration::from_millis(500);
/// Below this many bytes the npm cache is considered untouched, i.e. a hit.
///
/// The same amount of *accumulated* growth is what counts as one observation of
/// progress. Accumulating rather than judging single samples keeps a slow but
/// steady download (a few tens of KB per 500ms) from being read as stalled.
const PROGRESS_ACTIVE_THRESHOLD_BYTES: u64 = 64 * 1024;
const STDERR_TAIL_LINES: usize = 20;
const STDERR_BUFFER_LINES: usize = 200;
/// Time allowed for the process tree to die and the port to come back.
const STOP_GRACE: Duration = Duration::from_secs(5);

const DEFAULT_PORT: u16 = 3080;

/// The npm registry the dsh download is pointed at. The default
/// `registry.npmjs.org` is reachable but extremely slow from the target
/// network (a metadata fetch alone takes ~12s, so the package closure never
/// lands inside the readiness window), while the npmmirror.com mirror answers
/// the same fetch in ~4s. Injecting `NPM_CONFIG_REGISTRY` into the child only
/// — never the user's global config — is the least invasive way to make the
/// download actually finish.
const DSH_NPM_REGISTRY: &str = "https://registry.npmmirror.com";

/// `npx` is always used: dsh's bin name is not on `PATH` unless the user
/// installed the package globally, and this feature must work without that.
const DSH_PACKAGE: &str = "@deepseek-ai/dsh";

/// Package spec handed to npx. A recorded version pins the start to exactly
/// that release, which npx serves from its cache without re-resolving `latest`;
/// without a record the bare spec keeps the previous "resolve latest" behaviour
/// for the first-ever start.
///
/// The pin is an **exact** version, and that is load-bearing rather than
/// cosmetic. dsh's transient dependencies are released in waves that do not
/// land atomically, and every intra-release range in the closure is a caret
/// range over a prerelease (`^0.1.5-rc.1`). Caret ranges treat the whole
/// `0.1.5` prerelease ladder as one compatible series, so `^0.1.5-rc.1`
/// happily resolves to `rc.2` or `rc.3`. Asking npx for
/// `@deepseek-ai/dsh@0.1.5-rc.1` therefore does **not** pin the closure: npm
/// rewrites `dsh-base@^0.1.5-rc.1` to the newest matching prerelease, and the
/// whole tree drifts up to the newest wave. When that newest wave is the one
/// whose siblings have not been published yet, the install fails with ETARGET
/// on a package the user never asked for — the observed failure where pinning
/// `rc.1` still died on `dsh-client-ui-sidebar-documentpreview@^0.1.5-rc.3`.
///
/// npm has no global "exact" switch for a dependency subtree, so the launcher
/// pins what it can control: the top-level spec stays exact (it always was),
/// and the resolved closure is *verified* after the fact instead of being
/// trusted — see [`closure_drifted_from_pin`]. A drifted closure is reported as
/// the same upstream publishing gap rather than being silently accepted.
fn dsh_package_spec(version: Option<&str>) -> String {
    match version {
        Some(version) => format!("{DSH_PACKAGE}@{version}"),
        None => DSH_PACKAGE.to_string(),
    }
}

/// A recorded version becomes part of a command line, so accept only the
/// charset a semver/npm dist-tag can actually use. Anything else means the
/// state file was edited by hand; the start then falls back to the bare spec
/// and the post-start version record heals the stored value.
fn is_valid_pinned_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '+'))
}

/// The pinned version a start should use. A corrupted entry is dropped here
/// so it can never reach the npx command line.
fn start_pinned_version() -> Option<String> {
    crate::persistent_state::load_dsh_pinned_version()
        .filter(|version| is_valid_pinned_version(version))
}

/// Whether a failed install was caused by the pinned release's closure
/// resolving *past* the pin, and if so which package npm named.
///
/// [`is_unresolvable_version_failure`] says npm could not satisfy *some*
/// range. This narrows it to the case that matters for a pinned start: the
/// range npm failed on belongs to a *newer* wave than the one requested. The
/// `^0.1.5-rc.3` in the message versus a `0.1.5-rc.1` pin is exactly that
/// signature, and it is worth separating because the remedy differs — a pin
/// that cannot hold its closure is best answered by choosing a version whose
/// wave is complete, not merely by waiting.
///
/// Returns the offending version when the pin's prerelease ladder was walked
/// forward, or `None` when the message carries no version to compare — in
/// which case the generic upstream-gap wording stays accurate.
fn closure_drifted_from_pin(detail: &str, pinned: Option<&str>) -> Option<String> {
    let pinned = pinned?;
    let mut found = None;
    for line in detail.lines() {
        // The npm error names the range it wanted: `…for <pkg>@<range>.`
        let Some((_, tail)) = line.split_once("for ") else {
            continue;
        };
        let range = tail.trim().trim_end_matches('.').rsplit_once('@')?.1;
        let digits = range.trim_start_matches(['^', '~', '>', '=', '<', ' ']);
        // A drifted closure mentions a version with the same release core but a
        // later prerelease than the pin (`0.1.5-rc.3` vs `0.1.5-rc.1`).
        if digits == pinned || !digits.starts_with(prerelease_core(pinned)) {
            continue;
        }
        found = Some(digits.to_string());
    }
    found
}

/// The `major.minor.patch` prefix of a prerelease version, used to tell
/// "another prerelease of the same wave" from an unrelated package version.
fn prerelease_core(version: &str) -> &str {
    match version.split_once('-') {
        Some((core, _)) => core,
        None => version,
    }
}

/// Whether a key of npm's `time` object is a version rather than one of the
/// metadata keys npm mixes in (`created`, `modified`).
///
/// The charset check alone is not enough: `created` is all-alphanumeric and
/// would pass [`is_valid_pinned_version`]. A version key always starts with a
/// digit and carries a dot.
fn is_version_key(key: &str) -> bool {
    key.chars().next().is_some_and(|c| c.is_ascii_digit())
        && key.contains('.')
        && is_valid_pinned_version(key)
}

/// When each published version of the dsh package landed, oldest first.
///
/// Same read-only shape as [`query_available_versions`] (registry metadata, no
/// download). An entry whose timestamp will not parse is dropped instead of
/// failing the read: a missing entry only makes the cutoff less precise, while
/// failing here would silently disable closure locking altogether.
fn query_release_times(npm: &Path) -> Result<Vec<(String, DateTime<Utc>)>, String> {
    let output = hidden_command(npm)
        .args(["view", DSH_PACKAGE, "time", "--json"])
        .output()
        .map_err(|error| format!("无法运行 npm: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail: Vec<&str> = stderr
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(3)
            .collect();
        return Err(format!(
            "查询 dsh 发布时间表失败（{}）。",
            if detail.is_empty() {
                format!("npm 退出码 {}", output.status.code().unwrap_or(-1))
            } else {
                detail.join("；")
            }
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|_| "npm 返回的 dsh 发布时间表无法解析。".to_string())?;
    let Some(object) = parsed.as_object() else {
        return Err("npm 返回的 dsh 发布时间表格式异常。".to_string());
    };
    let mut times: Vec<(String, DateTime<Utc>)> = object
        .iter()
        .filter(|entry| is_version_key(entry.0))
        .filter_map(|(version, value)| {
            let time = DateTime::parse_from_rfc3339(value.as_str()?).ok()?;
            Some((version.clone(), time.with_timezone(&Utc)))
        })
        .collect();
    times.sort_by_key(|entry| entry.1);
    if times.is_empty() {
        return Err("npm 返回的 dsh 发布时间表为空。".to_string());
    }
    Ok(times)
}

/// The instant npm should be told to resolve *no later than*, so a prerelease
/// pin keeps the closure belonging to its own release wave.
///
/// The value is the **midpoint** between the pinned version's publish time and
/// the next version in the same prerelease ladder. Midpoint rather than "next
/// minus one second", because the measured shape of a dsh release makes the
/// edge rule unsafe:
///
/// * a wave lands package by package over roughly 16 minutes (measured across
///   `0.1.5-rc.2` and `0.1.5-rc.3`), and
/// * the top-level `@deepseek-ai/dsh` package lands **last** in its wave
///   (`rc.3`: siblings from 05:39, the top package at 05:55).
///
/// So `next_time - 1s` would still admit every sibling of the next wave
/// published before the top package — nearly all of them, and exactly the
/// `^0.1.5-rc.3` ranges that break an `rc.1` pin. The midpoint maximises the
/// distance from *both* waves, the right hedge when neither wave's extent is
/// knowable from the top package's timeline alone.
///
/// `None` when the pin has no later sibling in its ladder: the newest wave
/// cannot be isolated because there is nothing to cut it off from. That is the
/// one case `--before` cannot help with, and the caller starts normally.
fn release_cutoff(times: &[(String, DateTime<Utc>)], pinned: &str) -> Option<String> {
    let ladder = prerelease_core(pinned);
    let pin_time = times
        .iter()
        .find(|entry| entry.0.as_str() == pinned)
        .map(|entry| entry.1)?;
    // The earliest later publish wins, not the next version number: waves have
    // landed out of order before, and the point is to clear the whole wave.
    let next_time = times
        .iter()
        .map(|entry| (entry.0.as_str(), entry.1))
        .filter(|(version, time)| prerelease_core(version) == ladder && *time > pin_time)
        .map(|(_, time)| time)
        .min()?;
    let midpoint = pin_time + (next_time - pin_time) / 2;
    Some(midpoint.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// The `--before` value for a start that is about to resolve against the
/// registry, or `None` when there is nothing to lock.
///
/// Best-effort by construction: a pinned start must not fail because the
/// timeline could not be read, and every failure here degrades to exactly the
/// earlier behaviour — resolve normally, then report drift if it happens.
fn resolve_start_cutoff(version: Option<&str>) -> Option<String> {
    let version = version?;
    let npm = locate_npm()?;
    match query_release_times(&npm) {
        Ok(times) => release_cutoff(&times, version),
        Err(error) => {
            eprintln!("未能获取 dsh 发布时间表，本次不锁定依赖闭包：{error}");
            None
        }
    }
}

/// Persist the version that just started, unless the stored pin moved while
/// this start was in flight (the user clicked 「更新版本」 during the download):
/// the newer record must survive a slower start of the older pin.
fn record_started_version(started_with: Option<&str>, version: &str) {
    if !is_valid_pinned_version(version) || started_with == Some(version) {
        return;
    }
    let current = crate::persistent_state::load_dsh_pinned_version();
    if current.as_deref() != started_with {
        return;
    }
    if let Err(error) = crate::persistent_state::save_dsh_pinned_version(version) {
        eprintln!("未能记录 dsh 版本（{version}）：{error}");
    }
}

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
    /// Seconds since the supervised service started; `Some` only while Running.
    /// The frontend turns this into a live-ticking「已运行 …」label, so the
    /// backend reports a plain number instead of baking a stale value into
    /// `message`.
    pub uptime_secs: Option<u64>,
    /// Short machine-readable code for the failed phase:
    /// `port_in_use` / `npx_missing` / `spawn_failed` / `ready_timeout` /
    /// `exited` / `stop_failed` / `version_unavailable`.
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
    /// Which networks the occupant serves, so the runtime panel can compare a
    /// resident dsh instance with the saved 访问范围. `None` when the port is
    /// free (there is no occupant to describe).
    pub occupant_listen_scope: Option<DshListenScope>,
}

/// Whether the listener holding a port answers beyond the loopback interface.
///
/// The answer comes from connect probes against the machine's own addresses,
/// not from parsing socket tables: a wildcard bind (`0.0.0.0`) and a bind to
/// one concrete interface are indistinguishable from the client side, and for
/// the panel's «运行配置与保存配置是否一致» check both mean the same thing —
/// the network can reach this service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DshListenScope {
    /// Only loopback answers: reachable from this machine only.
    Local,
    /// At least one non-loopback address answers: the network can reach it.
    Remote,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshRuntimeUrls {
    pub local_url: String,
    pub remote_url: Option<String>,
    /// Every interface address the running service answers on, each with its own
    /// token-bearing URL, ordered 本地 → 局域网 → Tailscale → 其它.
    ///
    /// One `remote_url` is not enough: `dsh web` picks a single non-loopback
    /// address for its `(LAN: …)` segment and that pick can be the Tailscale
    /// one, which is useless to a phone on the same Wi-Fi. The panel lists what
    /// the machine actually has instead of guessing.
    pub addresses: Vec<DshAccessAddress>,
}

/// Which network an entry point belongs to. The panel groups and labels the
/// access list by this; it is a stable identifier, not display copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DshAddressKind {
    /// `127.0.0.1`: reachable from this machine only.
    Loopback,
    /// A private-range address (`10/8`, `172.16/12`, `192.168/16`) of a local
    /// interface: the address another device on the same network can open.
    Lan,
    /// A Tailscale address. Tailscale hands out `100.64.0.0/10` (CGNAT), which
    /// is a different audience from the LAN: only devices in the same tailnet.
    Tailscale,
    /// Anything else the host answers on (a public or corporate address).
    Other,
}

impl DshAddressKind {
    /// Display order rank. Stable so the list does not reshuffle between reads.
    fn rank(self) -> u8 {
        match self {
            Self::Loopback => 0,
            Self::Lan => 1,
            Self::Tailscale => 2,
            Self::Other => 3,
        }
    }
}

/// One way into the running service: the address plus the URL that carries this
/// process's access token.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshAccessAddress {
    pub kind: DshAddressKind,
    /// Bare IPv4 address, e.g. `192.168.1.5` or `100.101.102.103`.
    pub address: String,
    /// Interface name when the OS reports one (`WLAN`, `Tailscale`, `en0`); it
    /// is what makes a list of four look-alike numbers readable.
    pub interface: Option<String>,
    /// Token-bearing URL. Never logged, never persisted, never in diagnostics.
    pub url: String,
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

/// Outcome of 「检查更新」's read-only half: what the registry's `latest` is
/// and whether pinning to it would change what the next start runs. Nothing
/// is written here — the update itself is a separate, confirmed command.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshVersionCheck {
    /// The registry's current `latest`.
    pub latest: String,
    /// The version future starts are pinned to, when one has been recorded.
    pub pinned: Option<String>,
    /// True when the pin differs from `latest`, i.e. there is something the
    /// confirmation dialog can offer to apply.
    pub update_available: bool,
    /// User-facing Chinese summary, rendered verbatim by the panel.
    pub message: String,
}

/// Outcome of the confirmed 「更新版本」: `version` has been recorded as the
/// pinned version future starts use.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshVersionUpdate {
    /// The version future starts are now pinned to.
    pub version: String,
    /// The pin this update replaced, when there was one.
    pub previous: Option<String>,
    /// False when the recorded version was already the pinned one.
    pub changed: bool,
    /// User-facing Chinese summary, rendered verbatim by the panel.
    pub message: String,
}

/// The version picker's data: every version the registry offers, plus which one
/// is currently pinned so the dialog can mark that row.
///
/// This replaces the old "is there a newer `latest`?" report. The question a
/// user actually has is "which version should I run", and the answer is not
/// always the newest: when a release wave is published out of order, the newest
/// version is exactly the one that cannot install. Showing the full list lets
/// them step back to a version whose closure is complete, which the
/// `latest`-only report could never express.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshVersionList {
    /// Every published version, newest first.
    pub versions: Vec<String>,
    /// The pin future starts use, when one has been recorded. `None` before the
    /// first recorded start, or when the stored value failed validation.
    pub pinned: Option<String>,
    /// The registry's `latest` dist-tag, when it could be read. Used to label
    /// the newest row; a failure here does not fail the whole listing because
    /// the version list itself is still usable.
    pub latest: Option<String>,
}

/// A runnable dsh release found in npm's `_npx` cache. One release can have
/// several entries when npm used different resolution options.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshCachedVersion {
    pub version: String,
    pub entries: usize,
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

static DSH_SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

fn shutdown_requested() -> bool {
    DSH_SHUTTING_DOWN.load(Ordering::Acquire)
}

fn request_shutdown() {
    // This flag is process-lifetime state: once application exit begins, no
    // later dsh start may publish a child after the exit hook looked for one.
    DSH_SHUTTING_DOWN.store(true, Ordering::Release);
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
// Access addresses (本机 / 局域网 / Tailscale)
// ---------------------------------------------------------------------------

/// One IPv4 address an interface of this machine currently carries.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalAddress {
    ip: Ipv4Addr,
    /// Interface name as the OS reports it (`WLAN`, `Tailscale`, `en0`).
    interface: Option<String>,
    /// True when the owning adapter has a default gateway. Such an adapter is a
    /// real network rather than a virtual switch a hypervisor created, so its
    /// address is the better default for «复制链接».
    has_gateway: bool,
}

/// `100.64.0.0/10` — the CGNAT range Tailscale assigns.
fn is_tailscale_range(ip: Ipv4Addr) -> bool {
    let [first, second, ..] = ip.octets();
    first == 100 && (64..128).contains(&second)
}

/// RFC 1918 private space, i.e. the addresses a LAN peer normally sees.
fn is_private_range(ip: Ipv4Addr) -> bool {
    let [first, second, ..] = ip.octets();
    first == 10
        || (first == 172 && (16..32).contains(&second))
        || (first == 192 && second == 168)
}

/// Classify one address, or reject it when it cannot be opened by anyone.
///
/// The interface name is only a second signal: Tailscale's address range is the
/// reliable marker, because a renamed adapter would otherwise fall into the LAN
/// group and be handed out as if a Wi-Fi peer could reach it.
fn classify_address(ip: Ipv4Addr, interface: Option<&str>) -> Option<DshAddressKind> {
    if ip.is_loopback() {
        return Some(DshAddressKind::Loopback);
    }
    // 169.254/16 is what an interface reports while it is still asking for a
    // lease: listing it would offer an address nothing can reach.
    if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() || ip.is_link_local() {
        return None;
    }
    let named_tailscale = interface
        .is_some_and(|name| name.to_ascii_lowercase().contains("tailscale"));
    if named_tailscale || is_tailscale_range(ip) {
        return Some(DshAddressKind::Tailscale);
    }
    if is_private_range(ip) {
        return Some(DshAddressKind::Lan);
    }
    Some(DshAddressKind::Other)
}

/// IPv4 addresses of every interface that is up, straight from the OS.
///
/// Deliberately not derived from the ready line: `dsh web` reports one address
/// and on a machine that also runs Tailscale that one can be the tailnet
/// address, which is exactly the case this list exists to fix.
#[cfg(windows)]
fn local_ipv4_addresses() -> Vec<LocalAddress> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
        GAA_FLAG_SKIP_MULTICAST, IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::Networking::WinSock::{AF_INET, AF_UNSPEC, SOCKADDR_IN};

    /// `ERROR_BUFFER_OVERFLOW`: the table grew, `size` now holds what it needs.
    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    const MAX_ATTEMPTS: usize = 4;

    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    // One call to learn the size, one to fill the buffer. The first is still
    // made through the same loop: passing a buffer that happens to be large
    // enough turns this into a single call.
    //
    // The buffer is a `Vec<u64>`, not a `Vec<u8>`: the API writes
    // `IP_ADAPTER_ADDRESSES_LH` (and its linked lists) into it, and reading those
    // out of a byte buffer would be a misaligned reference.
    let mut size: u32 = 15 * 1024;
    let mut buffer: Vec<u64> = Vec::new();
    let mut filled = false;
    for _ in 0..MAX_ATTEMPTS {
        buffer.resize((size as usize).div_ceil(std::mem::size_of::<u64>()), 0);
        let result = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC.0),
                flags,
                None,
                Some(buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>()),
                &mut size,
            )
        };
        if result == 0 {
            filled = true;
            break;
        }
        if result != ERROR_BUFFER_OVERFLOW {
            return Vec::new();
        }
    }
    if !filled {
        // A table that will not fit after four attempts is not worth reporting
        // half of: the caller falls back to the address dsh itself printed.
        return Vec::new();
    }

    let mut found = Vec::new();
    let mut adapter = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    while let Some(current) = unsafe { adapter.as_ref() } {
        // A down adapter only carries stale addresses, and the software loopback
        // adapter only carries `127.0.0.1`, which is added explicitly later.
        if current.OperStatus == IfOperStatusUp && current.IfType != IF_TYPE_SOFTWARE_LOOPBACK {
            let interface = unsafe { current.FriendlyName.to_string() }
                .ok()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty());
            let has_gateway = !current.FirstGatewayAddress.is_null();
            let mut unicast = current.FirstUnicastAddress;
            while let Some(entry) = unsafe { unicast.as_ref() } {
                let socket = entry.Address;
                if !socket.lpSockaddr.is_null()
                    && socket.iSockaddrLength as usize >= std::mem::size_of::<SOCKADDR_IN>()
                {
                    let raw = unsafe { &*socket.lpSockaddr.cast::<SOCKADDR_IN>() };
                    if raw.sin_family == AF_INET {
                        // `sin_addr` is in network byte order.
                        let ip =
                            Ipv4Addr::from(u32::from_be(unsafe { raw.sin_addr.S_un.S_addr }));
                        found.push(LocalAddress {
                            ip,
                            interface: interface.clone(),
                            has_gateway,
                        });
                    }
                }
                unicast = entry.Next;
            }
        }
        adapter = current.Next;
    }
    found
}

/// POSIX branch: `ifconfig -a`, whose output is not localized and which groups
/// addresses under a `name: flags=…` header line.
#[cfg(not(windows))]
fn local_ipv4_addresses() -> Vec<LocalAddress> {
    let Ok(output) = hidden_command("ifconfig").arg("-a").output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut found = Vec::new();
    let mut interface: Option<String> = None;
    for line in text.lines() {
        // A header line starts in column zero; every address line is indented.
        if !line.starts_with([' ', '\t']) {
            interface = line
                .split(':')
                .next()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty());
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        // `inet 192.168.1.5 netmask 0xffffff00 broadcast 192.168.1.255`
        if fields.len() >= 2 && fields[0] == "inet" {
            if let Ok(ip) = fields[1].parse::<Ipv4Addr>() {
                found.push(LocalAddress {
                    ip,
                    interface: interface.clone(),
                    // `ifconfig` reports no gateway, and inventing one would make
                    // the ordering claim something it cannot know.
                    has_gateway: false,
                });
            }
        }
    }
    found
}

/// `http://127.0.0.1:3080/?token=abc` → `http`.
fn url_scheme(url: &str) -> Option<&str> {
    url.split_once("://").map(|(scheme, _)| scheme)
}

/// `http://192.168.1.5:3080/?token=abc` → `192.168.1.5`.
fn url_authority_host(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() {
        return None;
    }
    if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, _) = bracketed.split_once(']')?;
        return Some(host.to_string());
    }
    let host = authority
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(authority);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// The raw `token` query value of a ready-line URL. Never decoded and never
/// re-encoded: the value is copied verbatim into the URLs we hand out, so a
/// token that contains `%` keeps working.
fn url_token(url: &str) -> Option<String> {
    let (_, query) = url.split_once('?')?;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("token="))
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

/// Build the access list from the addresses the OS reports.
///
/// Split out from {@link access_addresses} so the ordering, the de-duplication
/// and the URL construction are testable without a running child process.
fn build_access_addresses(
    access: DshAccess,
    port: u16,
    local_url: &str,
    lan_url: Option<&str>,
    enumerated: Vec<LocalAddress>,
) -> Vec<DshAccessAddress> {
    // 本地 mode binds loopback only: every other address would be a dead link,
    // and offering it as "the way in" is worse than not listing it.
    if access == DshAccess::Local {
        return vec![DshAccessAddress {
            kind: DshAddressKind::Loopback,
            address: url_authority_host(local_url).unwrap_or_else(|| "127.0.0.1".to_string()),
            interface: None,
            url: local_url.to_string(),
        }];
    }

    let scheme = url_scheme(local_url).unwrap_or("http");
    let Some(token) = url_token(local_url) else {
        // Without the token every URL would land on dsh's 401 page, so there is
        // nothing worth listing.
        return Vec::new();
    };

    let mut candidates: Vec<(DshAddressKind, Ipv4Addr, Option<String>, bool)> = Vec::new();
    {
        let mut push = |ip: Ipv4Addr, interface: Option<String>, has_gateway: bool| {
            if candidates.iter().any(|(_, existing, _, _)| *existing == ip) {
                return;
            }
            if let Some(kind) = classify_address(ip, interface.as_deref()) {
                candidates.push((kind, ip, interface, has_gateway));
            }
        };

        for entry in enumerated {
            push(entry.ip, entry.interface, entry.has_gateway);
        }
        // The ready line is ground truth for "dsh considers this address its
        // LAN one", so it is kept even when the enumeration missed it. It is
        // added last and without a gateway claim, so a real interface address
        // still wins the «默认» slot.
        if let Some(host) = lan_url.and_then(url_authority_host) {
            if let Ok(ip) = host.parse::<Ipv4Addr>() {
                push(ip, None, false);
            }
        }
        push(Ipv4Addr::LOCALHOST, None, false);
    }

    // Stable sort: within one group the OS enumeration order is kept, and
    // gateway-bearing adapters come first because they lead somewhere.
    candidates.sort_by_key(|(kind, _, _, has_gateway)| {
        (kind.rank(), if *has_gateway { 0 } else { 1 })
    });

    candidates
        .into_iter()
        .map(|(kind, ip, interface, _)| DshAccessAddress {
            kind,
            address: ip.to_string(),
            interface,
            url: format!("{scheme}://{ip}:{port}/?token={token}"),
        })
        .collect()
}

/// The access list of a running server.
fn access_addresses(server: &DshServer) -> Vec<DshAccessAddress> {
    build_access_addresses(
        server.access,
        server.port,
        &server.local_url,
        server.lan_url.as_deref(),
        local_ipv4_addresses(),
    )
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

/// Put the launched command at the root of an independently terminable process
/// tree. Windows uses `taskkill /T` and therefore needs no spawn flag; on Unix
/// the negative-PID signal used by `terminate_pid` only works when the child is
/// actually a process-group leader.
fn configure_process_tree(_command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        _command.process_group(0);
    }
}

/// Arguments for `npx`. Launcher flags must precede app arguments: dsh's
/// launcher parses its own flags and hands *everything after the first unknown
/// token* to the app, so `--patch` after `--port` would reach the web app and
/// be rejected with `unknown option '--patch'`.
/// Arguments for `npx`. Launcher flags must precede app arguments: dsh's
/// launcher parses its own flags and hands *everything after the first unknown
/// token* to the app, so `--patch` after `--port` would reach the web app and
/// be rejected with `unknown option '--patch'`.
///
/// `offline` adds `--offline`, which tells npm to satisfy the spec from its
/// cache alone (`cache mode: only-if-cached`) instead of re-resolving the
/// dependency closure against the registry. This is the difference between a
/// start that can run with no network at all and one that fails whenever the
/// registry's view of the pinned release is incomplete.
///
/// `before` adds `--before=<instant>`, npm's "only consider versions published
/// on or before this moment" switch. It is the answer to the one hole an exact
/// pin cannot cover: pinning `@deepseek-ai/dsh@0.1.5-rc.1` fixes the top-level
/// package but not the closure, because every intra-release range in that
/// closure is a caret range over a prerelease (`^0.1.5-rc.1`), and npm reads
/// those as "any rc of 0.1.5, newest wins" — see [`release_cutoff`] for how the
/// instant is chosen.
///
/// It is deliberately **skipped when `offline` is set**. The switch only
/// changes what npm is allowed to *resolve*; an offline attempt reads the
/// cached entry instead of resolving at all, so carrying it there would be
/// noise. Passing it on the networked attempts is what makes it effective.
fn dsh_arguments(
    overlay: &Path,
    port: u16,
    version: Option<&str>,
    offline: bool,
    before: Option<&str>,
) -> Vec<String> {
    let mut args = Vec::with_capacity(12);
    if offline {
        args.push("--offline".to_string());
    } else if let Some(cutoff) = before {
        args.push(format!("--before={cutoff}"));
    }
    args.extend([
        "--yes".to_string(),
        dsh_package_spec(version),
        "web".to_string(),
        "--patch".to_string(),
        overlay.to_string_lossy().to_string(),
        "--port".to_string(),
        port.to_string(),
        "--no-open".to_string(),
    ]);
    args
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

/// Which networks the port's listener answers on, probed from the client side.
///
/// Connecting to each of the machine's own non-loopback addresses works for a
/// wildcard bind and for a concrete-interface bind alike, and it needs no
/// platform-specific socket-table parsing.
fn occupant_listen_scope(port: u16) -> DshListenScope {
    let serves_network = local_ipv4_addresses()
        .into_iter()
        .filter(|entry| !entry.ip.is_loopback())
        .any(|entry| is_listening(SocketAddr::new(std::net::IpAddr::V4(entry.ip), port)));
    if serves_network {
        DshListenScope::Remote
    } else {
        DshListenScope::Local
    }
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
            if parse_authority_port(fields[1]) != Some(port) {
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
        let term_result = unsafe { libc::kill(-group, libc::SIGTERM) };
        if term_result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(format!("无法结束进程组 {group}: {error}"));
            }
            return Ok(());
        }
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(25));
            if unsafe { libc::kill(-group, 0) } != 0 {
                return Ok(());
            }
        }
        let kill_result = unsafe { libc::kill(-group, libc::SIGKILL) };
        if kill_result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(format!("无法强制结束进程组 {group}: {error}"));
            }
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
    // Resolve npm the same way the dsh launch resolves npx — by full path via
    // `locate_executable`, which honours PATHEXT (`npm.cmd`). `Command::new("npm")`
    // alone fails in a GUI process whose PATH does not resolve bare names.
    let npm = crate::platform_env::locate_executable("npm");
    if let Some(path) = npm.as_ref() {
        if let Ok(output) = hidden_command(path)
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
    }
    // The default cache location differs by platform. On Windows it is
    // `%LOCALAPPDATA%\npm-cache`; on Unix it is `~/.npm`. Falling back to a
    // directory that never exists would silently disable the corrupt-`_npx`
    // scan — the exact failure the Windows side of this fallback exists to
    // prevent — so each platform must name its own default.
    #[cfg(windows)]
    {
        dirs::cache_dir().map(|path| path.join("npm-cache"))
    }
    #[cfg(not(windows))]
    {
        dirs::home_dir().map(|path| path.join(".npm"))
    }
}

/// Only count complete, top-level dsh installs. npm also caches unrelated
/// packages under `_npx`, and a failed install may leave an empty entry behind.
fn cached_dsh_version(entry: &Path) -> Option<String> {
    for path in [
        entry.to_path_buf(),
        entry.join("node_modules"),
        entry.join("node_modules").join("@deepseek-ai"),
        entry.join("node_modules").join("@deepseek-ai").join("dsh"),
    ] {
        let kind = std::fs::symlink_metadata(path).ok()?.file_type();
        if !kind.is_dir() || kind.is_symlink() {
            return None;
        }
    }
    let root: serde_json::Value =
        serde_json::from_slice(&std::fs::read(entry.join("package.json")).ok()?).ok()?;
    root.get("dependencies")?.get(DSH_PACKAGE)?.as_str()?;
    let manifest = entry
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh")
        .join("package.json");
    let package: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest).ok()?).ok()?;
    if package.get("name")?.as_str()? != DSH_PACKAGE {
        return None;
    }
    let version = package.get("version")?.as_str()?;
    is_version_key(version).then(|| version.to_string())
}

fn cached_dsh_entries(cache_dir: &Path) -> Result<BTreeMap<String, Vec<PathBuf>>, String> {
    let root = cache_dir.join("_npx");
    let metadata = match std::fs::symlink_metadata(&root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("无法读取 npx 缓存目录：{error}")),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("npx 缓存目录不是普通目录，已拒绝访问。".to_string());
    }
    let mut versions = BTreeMap::<String, Vec<PathBuf>>::new();
    for item in std::fs::read_dir(&root).map_err(|error| format!("无法列出 npx 缓存：{error}"))? {
        let item = item.map_err(|error| format!("无法读取 npx 缓存条目：{error}"))?;
        if let Some(version) = cached_dsh_version(&item.path()) {
            versions.entry(version).or_default().push(item.path());
        }
    }
    Ok(versions)
}

fn delete_cached_dsh_version_from(cache_dir: &Path, version: &str) -> Result<usize, String> {
    let root = cache_dir.join("_npx");
    let entries = cached_dsh_entries(cache_dir)?;
    let targets = entries
        .get(version)
        .ok_or_else(|| format!("本机缓存中已没有 v{version}。"))?;
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("无法核对 npx 缓存目录：{error}"))?;
    // Recheck every target immediately before deletion. The version is
    // supplied by the UI, never used to construct a filesystem path.
    for target in targets {
        let canonical = target
            .canonicalize()
            .map_err(|error| format!("无法核对缓存条目：{error}"))?;
        if canonical.parent() != Some(canonical_root.as_path())
            || cached_dsh_version(target).as_deref() != Some(version)
        {
            return Err("缓存条目已变化，已取消删除；请重新打开版本列表。".to_string());
        }
    }
    for target in targets {
        std::fs::remove_dir_all(target)
            .map_err(|error| format!("删除缓存条目 {} 失败：{error}", target.display()))?;
    }
    Ok(targets.len())
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

/// The first-download sampler's state, shared with the readiness wait.
///
/// The sampler owns the npm cache measurement; the wait loop needs only two
/// facts from it — how much has arrived and when the last real growth was — so
/// that a healthy download keeps a first start alive past [`READY_TIMEOUT`].
#[derive(Debug)]
struct DownloadProgress {
    /// Cache size before the download, measured by the sampler on its first
    /// pass. `None` until then, so a reading can never be compared against an
    /// unset baseline and reported as a huge burst of growth.
    baseline: Option<u64>,
    /// Bytes added since `baseline`.
    bytes: u64,
    /// Bytes accumulated since growth was last recorded. Kept because the
    /// threshold has to be reached by *summing* samples: a download that gains
    /// 20 KB per 500ms is making steady progress, but no single sample crosses
    /// the threshold on its own.
    bytes_since_growth: u64,
    /// When the cache last grew by a whole threshold. Set at construction, and
    /// a cache hit never moves it — which is exactly why a start that downloads
    /// nothing is bounded by [`READY_TIMEOUT_CACHED`] instead of the download
    /// window.
    last_growth: Instant,
    /// True once any threshold-sized growth has been observed.
    grew: bool,
}

impl DownloadProgress {
    fn new() -> Self {
        Self {
            baseline: None,
            bytes: 0,
            bytes_since_growth: 0,
            last_growth: Instant::now(),
            grew: false,
        }
    }

    /// Fold one cache measurement in, returning `(bytes so far, growth since the
    /// previous reading)`. The first call only establishes the baseline.
    fn observe(&mut self, total_bytes: u64) -> (u64, u64) {
        let Some(baseline) = self.baseline else {
            self.baseline = Some(total_bytes);
            return (0, 0);
        };
        let bytes = total_bytes.saturating_sub(baseline);
        let growth = bytes.saturating_sub(self.bytes);
        self.bytes = bytes;
        self.bytes_since_growth = self.bytes_since_growth.saturating_add(growth);
        if self.bytes_since_growth >= PROGRESS_ACTIVE_THRESHOLD_BYTES {
            self.bytes_since_growth = 0;
            self.last_growth = Instant::now();
            self.grew = true;
        }
        (bytes, growth)
    }
}

/// A consistent read of the shared sampler state for the wait loop.
#[derive(Debug, Clone, Copy)]
struct ProgressSnapshot {
    bytes: u64,
    last_growth: Instant,
    grew: bool,
}

/// Snapshot the sampler, or `None` when there is no sampler (no npm cache
/// directory could be resolved) or the lock is poisoned. `None` means "fall back
/// to the plain absolute timeout" — never "wait forever".
fn progress_snapshot(shared: &Option<Arc<Mutex<DownloadProgress>>>) -> Option<ProgressSnapshot> {
    let guard = shared.as_ref()?.lock().ok()?;
    Some(ProgressSnapshot {
        bytes: guard.bytes,
        last_growth: guard.last_growth,
        grew: guard.grew,
    })
}

/// The moment the readiness wait gives up.
///
/// [`READY_TIMEOUT`] bounds *staleness*, not the whole attempt: every fresh
/// observation of cache growth slides the deadline forward, so a first start
/// that is still pulling the package closure down is not interrupted halfway
/// through. Two things keep that from becoming an unbounded wait — the deadline
/// only moves while the cache is demonstrably growing (a cached start, or a
/// process wedged before it printed anything, keeps its own shorter
/// [`READY_TIMEOUT_CACHED`]), and it can never pass
/// `started_at + READY_TIMEOUT_CEILING`.
///
/// `fresh_download` skips the cache-hit window entirely: the self-repair retry
/// has just emptied the cache, so it is by definition downloading and must not
/// be cut off by the 30s window that fired the timeout it is recovering from.
fn ready_deadline(
    snapshot: Option<ProgressSnapshot>,
    started_at: Instant,
    fresh_download: bool,
) -> Instant {
    match snapshot {
        Some(snapshot) if snapshot.grew => {
            (snapshot.last_growth + READY_TIMEOUT).min(started_at + READY_TIMEOUT_CEILING)
        }
        Some(_) if fresh_download => started_at + READY_TIMEOUT,
        Some(_) => started_at + READY_TIMEOUT_CACHED,
        None => started_at + READY_TIMEOUT,
    }
}

/// `13.4 MiB`. The timeout message reports what actually arrived, so a download
/// cut off at the ceiling is visibly non-zero rather than silently discarded.
fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.0} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Emit `dsh_install_progress` from the moment npx starts until the ready line
/// arrives (or the sampler is switched off again), and keep `progress` current
/// so the readiness wait can tell "still downloading" from "nothing is
/// happening".
fn spawn_progress_sampler(
    app: AppHandle,
    cache_dir: PathBuf,
    active: Arc<AtomicBool>,
    progress: Arc<Mutex<DownloadProgress>>,
) {
    std::thread::spawn(move || {
        let started_at = Instant::now();
        let mut cached = false;
        while active.load(Ordering::Relaxed) {
            std::thread::sleep(PROGRESS_SAMPLE_INTERVAL);
            if !active.load(Ordering::Relaxed) {
                return;
            }
            // A poisoned lock only costs the wait loop its progress signal; the
            // event traffic below is still worth emitting.
            let (bytes, growth) = match progress.lock() {
                Ok(mut progress) => progress.observe(directory_size(&cache_dir)),
                Err(_) => (0, 0),
            };
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

/// Why one spawn attempt failed, in the vocabulary `SpawnFailure.issue` uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnIssue {
    /// `READY_TIMEOUT` / ceiling hit: the process is alive but never printed
    /// the ready line. This is the one failure a cache-hit npx leaves on the
    /// table when its `_npx` entry came from an interrupted install.
    ReadyTimeout,
    /// Anything else (spawn, early exit, stream close).
    Other,
}

type SpawnFailure = (String, String, Option<String>);

fn spawn_failure_issue(failure: &SpawnFailure) -> SpawnIssue {
    if failure.0 == "ready_timeout" {
        SpawnIssue::ReadyTimeout
    } else {
        SpawnIssue::Other
    }
}

/// An install can stop before readiness either by timing out or by exiting
/// after npm has written only part of its `_npx` entry. A spawn failure never
/// started npm and cannot have created one.
fn failure_may_leave_incomplete_npx_entry(failure: &SpawnFailure) -> bool {
    matches!(failure.0.as_str(), "ready_timeout" | "exited")
}

fn spawn_and_wait(
    app: &AppHandle,
    npx: &Path,
    overlay: &Path,
    port: u16,
    version: Option<&str>,
    // `true` on the self-repair retry: the cache was just purged, so this
    // attempt *is* a fresh download and must get the sliding download window,
    // not the 30s cache-hit window that fired the first timeout.
    fresh_download: bool,
    // `true` for the offline-first attempt: npm is told to use its cache only.
    // A pinned start has everything it needs on disk already, so this is the
    // normal path; it only falls back to a networked attempt when the cache
    // genuinely cannot satisfy the spec.
    offline: bool,
    // The `--before` instant that keeps a networked resolution inside the
    // pinned release's own wave, from [`resolve_start_cutoff`]. `None` for an
    // offline attempt (where it would be meaningless) and for the newest wave
    // (where there is nothing to cut it off from).
    before: Option<&str>,
) -> Result<SpawnOutcome, SpawnFailure> {
    let mut command = hidden_command(npx);
    command
        .args(dsh_arguments(overlay, port, version, offline, before))
        // Route only this download through the fast mirror; the user's global
        // npm config is untouched. See `DSH_NPM_REGISTRY`.
        .env("NPM_CONFIG_REGISTRY", DSH_NPM_REGISTRY)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_process_tree(&mut command);

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

    let started_at = Instant::now();
    let progress_active = Arc::new(AtomicBool::new(true));
    // Only sampled when the npm cache location could be resolved; without it the
    // wait has no growth signal and falls back to the plain absolute timeout.
    let progress = npm_cache_dir().map(|cache_dir| {
        let shared = Arc::new(Mutex::new(DownloadProgress::new()));
        spawn_progress_sampler(
            app.clone(),
            cache_dir,
            progress_active.clone(),
            shared.clone(),
        );
        shared
    });

    loop {
        if shutdown_requested() {
            progress_active.store(false, Ordering::Relaxed);
            let detail = with_shutdown_error(
                stderr_summary(&stderr_tail),
                terminate_managed_process(&mut child, DshAccess::Local, port),
            );
            return Err((
                "exited".to_string(),
                "应用正在退出，已取消 dsh 启动。".to_string(),
                detail,
            ));
        }
        // Re-derived every pass: a first-download sampler that is still seeing
        // cache growth keeps pushing this forward, so only a *stalled* start
        // ever reaches the deadline. A self-repair retry has just emptied the
        // cache, so it is forced onto the download window rather than the
        // cache-hit window that fired the timeout being repaired.
        let snapshot = progress_snapshot(&progress);
        let deadline = ready_deadline(snapshot, started_at, fresh_download);
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            progress_active.store(false, Ordering::Relaxed);
            let detail = stderr_summary(&stderr_tail);
            let detail = with_shutdown_error(
                detail,
                terminate_managed_process(&mut child, DshAccess::Local, port),
            );
            // The message names the cause. A download still moving at the
            // ceiling, and one that never got going, are the two download
            // cases. A *cache hit* that never became ready is the corrupt
            // `_npx` entry case — but only a first attempt can claim that: the
            // self-repair retry just emptied the cache, so it is downloading
            // even before any growth has been observed, and must not be
            // mislabelled as a cache hit.
            let message = match snapshot {
                Some(snapshot) if snapshot.grew => format!(
                    "启动 dsh 超过 {} 分钟上限仍未就绪（已下载 {}）。下载一直在进行，但耗时过长，请检查网络与代理后重试。",
                    READY_TIMEOUT_CEILING.as_secs() / 60,
                    human_bytes(snapshot.bytes)
                ),
                Some(_) if !fresh_download => format!(
                    "启动 dsh 超时（使用本地缓存，{} 秒内未就绪）。本地缓存可能已损坏，正在自动清理并重试；若仍失败请重试。",
                    READY_TIMEOUT_CACHED.as_secs()
                ),
                _ => format!(
                    "启动 dsh 超时（{} 秒内未就绪）。首次运行需要下载依赖，请检查网络与代理后重试。",
                    READY_TIMEOUT.as_secs()
                ),
            };
            return Err(("ready_timeout".to_string(), message, detail));
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
                let detail = with_shutdown_error(
                    detail,
                    terminate_managed_process(&mut child, DshAccess::Local, port),
                );
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
            let detail = with_shutdown_error(
                detail,
                terminate_managed_process(&mut child, DshAccess::Local, port),
            );
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

// ---------------------------------------------------------------------------
// npx cache self-repair
// ---------------------------------------------------------------------------

/// The corrupt npx cache entry named by an npm ENOENT failure, if this stderr
/// output is that failure.
///
/// An interrupted `npx` install leaves `<npm-cache>/_npx/<key>/` behind with a
/// populated `node_modules` but no `package.json`. Every later start of the
/// same spec then dies before dsh itself runs, because npx only *reads* the
/// existing entry and never reinstalls it:
///
/// ```text
/// npm error enoent Could not read package.json: Error: ENOENT: no such file or directory, open 'C:\…\npm-cache\_npx\1da1392061ab1944\package.json'
/// ```
///
/// Both spellings of the path are accepted — the quoted `open '…'` clause and
/// the bare `npm error path …` line — and the result is only ever the entry
/// directory exactly one level below `_npx`, so the `_npx` root itself can
/// never be returned.
fn corrupt_npx_cache_dir(detail: &str) -> Option<PathBuf> {
    if !detail.contains("ENOENT") {
        return None;
    }
    for line in detail.lines() {
        let candidate = line
            .split_once("open '")
            .and_then(|(_, tail)| tail.split_once('\''))
            .map(|(path, _)| path.trim())
            .or_else(|| line.trim().strip_prefix("npm error path ").map(str::trim));
        let Some(candidate) = candidate else { continue };
        let path = Path::new(candidate);
        if path.file_name().and_then(|name| name.to_str()) != Some("package.json") {
            continue;
        }
        let Some(entry) = path.parent() else { continue };
        let under_npx = entry
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            == Some("_npx");
        if under_npx {
            return Some(entry.to_path_buf());
        }
    }
    None
}

/// Whether this stderr is npm refusing an offline start because its cache
/// cannot satisfy the spec — `ENOTCACHED`.
///
/// ```text
/// npm error code ENOTCACHED
/// npm error request to https://registry.npmjs.org/left-pad failed: cache mode is 'only-if-cached' but no cached response is available.
/// ```
///
/// Used for **logging only**, not as the gate on the networked retry. The gate
/// is deliberately broader — see [`offline_failure_justifies_download`] — and
/// this narrower check exists so the log line can say which of the two reasons
/// sent the launcher back online.
fn is_offline_cache_miss(detail: &str) -> bool {
    detail.contains("ENOTCACHED") || detail.contains("only-if-cached")
}

/// Whether a failed *offline* attempt should be retried online.
///
/// Offline resolution and online resolution fail in ways that are **not**
/// distinguishable from each other by their error text, which is the subtlety
/// this whole fallback turns on. Measured behaviour, all with `--offline`:
///
/// | cache state | error |
/// |---|---|
/// | package absent entirely | `ENOTCACHED` |
/// | metadata cached, tarball absent | `ENOTCACHED` |
/// | metadata cached, closure unsatisfiable | `ETARGET` |
/// | entry present but damaged | `ENOENT` |
///
/// The third row is the problem: a *partially* cached closure reports the same
/// `ETARGET` an upstream publishing gap would, because npm resolved against
/// cached metadata and found the same missing sibling it would have found
/// online. So `ETARGET` offline means "this could not be resolved from what is
/// on disk" — which is precisely the case that must go online, since the
/// networked resolution may well succeed (a newer sibling may have landed since
/// the metadata was cached).
///
/// That makes the honest rule "retry whenever the offline attempt failed for
/// any reason that a download could fix", and the only failures it excludes are
/// the ones a download provably cannot fix:
///
/// - `spawn_failed`: npx could not even be launched.
/// - `ready_timeout`: the process ran but never became ready; the cache served
///   it fine, so the problem is elsewhere (and the timeout path already has its
///   own self-repair).
///
/// Everything else — every early exit with npm stderr — earns exactly one
/// networked attempt. The cost of being wrong in that direction is a slower
/// start; the cost of being wrong in the other is a start that could have
/// worked refusing to.
fn offline_failure_justifies_download(failure: &SpawnFailure) -> bool {
    match spawn_failure_issue(failure) {
        SpawnIssue::ReadyTimeout => false,
        SpawnIssue::Other => {
            failure.0 != "spawn_failed" && failure.2.as_deref().is_some_and(|d| !d.trim().is_empty())
        }
    }
}

/// Whether this stderr is npm's "a required version does not exist" failure —
/// `ETARGET` / `notarget`.
///
/// ```text
/// npm error code ETARGET
/// npm error notarget No matching version found for @deepseek-ai/dsh-session-projection@^0.1.5-rc.3.
/// ```
///
/// This is *not* a corrupt cache and must never be repaired by deleting one.
/// npm resolves the whole dependency closure before it touches the `_npx`
/// entry, so the failure happens before anything is installed: the entry is
/// untouched and purging it would only throw away a good download.
///
/// The cause is upstream. dsh's transient dependencies are published in waves
/// that do not land atomically, and its intra-release ranges are caret ranges
/// over a prerelease (`^0.1.5-rc.1`). A caret range treats the whole `0.1.5`
/// prerelease ladder as one compatible series, so it resolves to the newest
/// `0.1.5-rc.*` — which is why a start pinned at `rc.1` still asks npm for the
/// `rc.3` closure. When a wave has published a parent before one of its
/// children (`dsh-web-app@rc.3` requiring
/// `dsh-client-ui-sidebar-documentpreview@^0.1.5-rc.3` before that package's
/// rc.3 exists) the closure is unsatisfiable.
///
/// Whether it heals by waiting depends on which side of the gap you are on,
/// which is exactly what [`closure_drifted_from_pin`] separates: an unpinned
/// `latest` start heals as soon as the missing sibling lands, while a pin whose
/// ladder is walked forward is deterministic and keeps failing until the pin
/// moves. Detection here stays deliberately broad; the caller narrows it.
///
/// Detected from the stderr **without** a filesystem target: unlike the ENOENT
/// path there is no directory to clean, and returning `None` here keeps the
/// repair chain from inventing one.
fn is_unresolvable_version_failure(detail: &str) -> bool {
    detail.contains("ETARGET") || detail.contains("notarget")
}

/// Remove one corrupt npx cache entry so the next start reinstalls it.
///
/// The stderr text is the only evidence pointing at `dir`, so two facts are
/// re-checked against the filesystem before anything is deleted: the target is
/// exactly `<cache>/_npx/<entry>` (never the `_npx` root or an unrelated
/// directory) and the claimed missing `package.json` is genuinely missing —
/// deleting a healthy entry would only cost a re-download, but a misparsed
/// path must never delete anything at all.
fn purge_corrupt_npx_cache(dir: &Path) -> Result<(), String> {
    let entry_name = dir.file_name().and_then(|name| name.to_str()).unwrap_or("");
    let under_npx = dir
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some("_npx");
    if entry_name.is_empty() || !under_npx {
        return Err(format!(
            "路径不是 npx 缓存条目，已拒绝清理：{}",
            dir.display()
        ));
    }
    if !dir.is_dir() {
        // Nothing to purge — another process may have cleaned it up already;
        // the retry below simply starts against a fresh cache.
        return Ok(());
    }
    if dir.join("package.json").exists() {
        return Err("package.json 存在，该缓存并未损坏，已取消清理。".to_string());
    }
    std::fs::remove_dir_all(dir)
        .map_err(|error| format!("清理损坏的 npx 缓存（{}）失败：{error}", dir.display()))
}

/// One `_npx` entry a failed start has reason to suspect.
///
/// The ENOENT repair above keys off stderr, but an incomplete install can also
/// time out or exit without naming its cache path. The entry directory remains,
/// so npx may reuse it as-is forever.
///
/// Two independent facts must both hold before an entry is touched:
///
/// 1. **Touched inside the start's window.** The entry (or anything directly
///    inside it) changed at or after `window_start`, so it belongs to *this*
///    attempt rather than to some other package's healthy cache.
/// 2. **Structurally incomplete.** Either the entry `package.json` is missing
///    (the ENOENT shape, proven before the first purge existed) or the dsh
///    package directory itself is absent from `node_modules`
///    (`node_modules/@deepseek-ai/dsh`), i.e. the payload never landed.
///
/// Both checks are re-derived from the filesystem at scan time; nothing from
/// the timed-out process's output is trusted to name a directory.
fn suspect_npx_cache_entries(cache_dir: &Path, window_start: Instant) -> Vec<PathBuf> {
    let mut suspects = Vec::new();
    let npx_root = cache_dir.join("_npx");
    let Ok(entries) = std::fs::read_dir(&npx_root) else {
        return suspects;
    };
    // Instants cannot be compared against filesystem mtimes directly; convert
    // the window start to wall time once and compare in that domain.
    let window_start_system = SystemTime::now()
        .checked_sub(window_start.elapsed())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let touched = latest_mtime(&dir, 1).is_some_and(|mtime| mtime >= window_start_system);
        if !touched {
            continue;
        }
        let incomplete = !dir.join("package.json").exists()
            || !dir
                .join("node_modules")
                .join("@deepseek-ai")
                .join("dsh")
                .is_dir();
        if incomplete {
            suspects.push(dir);
        }
    }
    suspects
}

/// Newest mtime of `dir` itself or anything directly inside it, down to
/// `depth` levels. A start that so much as looks at its cache entry bumps the
/// entry's own mtime on some filesystems and the payload's on all of them, so
/// a shallow scan is enough — and it never descends into `node_modules`.
fn latest_mtime(dir: &Path, depth: u8) -> Option<SystemTime> {
    let mut latest = std::fs::metadata(dir).and_then(|m| m.modified()).ok();
    if depth > 0 {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Some(candidate) = latest_mtime(&entry.path(), depth - 1) {
                    if latest.is_none_or(|current| candidate > current) {
                        latest = Some(candidate);
                    }
                }
            }
        }
    }
    latest
}

/// Remove every suspect entry from a timed-out start. A purge failure on one
/// entry is logged and does not stop the others: the retry still gets whatever
/// cleanup was possible.
fn purge_suspect_npx_cache_entries(cache_dir: &Path, window_start: Instant) -> bool {
    let suspects = suspect_npx_cache_entries(cache_dir, window_start);
    if suspects.is_empty() {
        return false;
    }
    let mut purged = false;
    for dir in suspects {
        // The entry-level guard from the stderr path is reused verbatim: even
        // though the scan already checked structure, the purge must refuse a
        // directory that stopped looking corrupt between scan and delete.
        match purge_incomplete_npx_entry(&dir) {
            Ok(()) => {
                purged = true;
                eprintln!(
                    "dsh 启动超时，检测到疑似中断的 npx 缓存（{}），已自动清理。",
                    dir.display()
                );
            }
            Err(error) => {
                eprintln!("dsh 启动超时，自动清理 npx 缓存被拒绝：{error}");
            }
        }
    }
    purged
}

/// Purge one `_npx` entry that a timeout scan judged incomplete. Distinct from
/// [`purge_corrupt_npx_cache`]: that one proves corruption through npm's own
/// ENOENT report (missing `package.json`), while a timeout can also face an
/// entry whose `package.json` landed but whose payload did not. The guard is
/// the same in spirit — re-verify before deleting, never touch the `_npx`
/// root, tolerate an entry someone else already removed.
fn purge_incomplete_npx_entry(dir: &Path) -> Result<(), String> {
    let entry_name = dir.file_name().and_then(|name| name.to_str()).unwrap_or("");
    let under_npx = dir
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some("_npx");
    if entry_name.is_empty() || !under_npx {
        return Err(format!(
            "路径不是 npx 缓存条目，已拒绝清理：{}",
            dir.display()
        ));
    }
    if !dir.is_dir() {
        return Ok(());
    }
    let has_manifest = dir.join("package.json").exists();
    let has_payload = dir
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh")
        .is_dir();
    if has_manifest && has_payload {
        return Err("条目同时具有 package.json 与 dsh 载荷，结构完整，已取消清理。".to_string());
    }
    std::fs::remove_dir_all(dir)
        .map_err(|error| format!("清理不完整的 npx 缓存条目（{}）失败：{error}", dir.display()))
}

/// Start dsh, preferring the local cache and only touching the network when the
/// cache cannot serve the request.
///
/// The order of attempts is the whole point, so it is worth stating plainly:
///
/// 1. **Offline first.** A pinned start already has its closure on disk, and
///    `--offline` makes npm use exactly that instead of re-resolving the
///    dependency tree against the registry. This is what makes a normal start
///    immune to registry problems: a pinned release whose siblings are still
///    being published upstream simply never comes up, because nothing asks.
/// 2. **Networked whenever the cache cannot serve the spec.** The gate is
///    deliberately broad — any early exit carrying npm stderr earns one
///    download, because offline and online produce *identical* error text for
///    a partially cached closure. See [`offline_failure_justifies_download`].
/// 3. **Self-repair.** A cache hit that never becomes ready, or an npm report
///    naming a damaged entry, gets the entry purged and one clean retry.
/// 4. **Closure locking.** Steps 2 and 3 both resolve against the registry, so
///    both ask [`resolve_start_cutoff`] for the `--before` instant that keeps
///    the resolution inside the pinned version's own release wave. Without it,
///    `^0.1.5-rc.1` is free to resolve up to a partially published `rc.3` —
///    the failure this whole chain exists to survive. The lookup is
///    best-effort: if it cannot be made, the attempt proceeds unlocked.
///
/// The first attempt is offline only when a version is pinned. Without a pin
/// the spec is bare `latest`, which by definition cannot be resolved from
/// cache — asking offline would fail every time, so an unpinned start goes
/// straight to the networked path.
fn spawn_with_cache_repair(
    app: &AppHandle,
    npx: &Path,
    overlay: &Path,
    port: u16,
    version: Option<&str>,
) -> Result<SpawnOutcome, SpawnFailure> {
    let mut repair_window_start = Instant::now();

    // Step 1: offline, for pinned starts. An unpinned start has no cache to
    // consult — `latest` is a registry lookup by definition — so it skips
    // straight to the download. No cutoff is passed: this attempt reads the
    // cached entry rather than resolving, so `--before` would have no effect.
    let first = match spawn_and_wait(app, npx, overlay, port, version, false, version.is_some(), None)
    {
        Ok(outcome) => return Ok(outcome),
        Err(failure) => failure,
    };

    // Step 2: the cache could not serve it, so go online. The *reason* is only
    // used for the log line — the decision itself is deliberately broad, since
    // offline and online produce identical error text for a partially cached
    // closure. See `offline_failure_justifies_download`.
    let first = if version.is_some() && offline_failure_justifies_download(&first) {
        let reason = if first.2.as_deref().is_some_and(is_offline_cache_miss) {
            "本地缓存中没有该版本"
        } else {
            "本地缓存无法解析该版本的依赖"
        };
        eprintln!(
            "{reason}（dsh{}），改为联网获取。",
            version.map(|v| format!("@{v}")).unwrap_or_default()
        );
        let before = resolve_start_cutoff(version);
        if let Some(cutoff) = before.as_deref() {
            eprintln!("为避免依赖被解析到更新的发布批次，本次安装限定在 {cutoff} 之前发布的版本。");
        }
        repair_window_start = Instant::now();
        match spawn_and_wait(
            app,
            npx,
            overlay,
            port,
            version,
            true,
            false,
            before.as_deref(),
        ) {
            Ok(outcome) => return Ok(outcome),
            // A networked install can also leave an incomplete `_npx` entry.
            // Let the same guarded repair path inspect that failure instead of
            // returning it before the cache checks below can run.
            Err(failure) => failure,
        }
    } else {
        first
    };

    // Fast path, keyed off npm's own report: the entry is corrupt and npm said
    // exactly which one. Works for the early-exit failure shape.
    //
    // The ETARGET check comes *first* on purpose. A version-resolution failure
    // also exits early with npm stderr, but it is not corruption and its
    // stderr carries no `_npx` path at all — so it would fall through the
    // ENOENT parser anyway. Checking it explicitly keeps the two apart at the
    // point where the decision is made, and gives the user the one piece of
    // information that actually helps: this is an upstream publishing gap, and
    // retrying later is the fix.
    if let Some(detail) = first.2.as_deref() {
        if is_unresolvable_version_failure(detail) {
            // Two different situations reach this point, and the user needs to
            // know which one they are in — "wait a bit" and "pick another
            // version" are not interchangeable advice.
            let message = match closure_drifted_from_pin(detail, version) {
                // The pin could not hold its closure: npm walked the
                // prerelease ladder past `version` and landed on a wave with
                // unpublished siblings. Waiting will not help while the pin
                // stays where it is, because the resolution is deterministic —
                // the same spec resolves the same way every time.
                Some(drifted) => format!(
                    "dsh 依赖解析越过了你固定的版本：固定 v{pinned}，但它的依赖范围\
                     （^ 前缀）被 npm 解析到了 v{drifted}，而 v{drifted} 这一批配套包还没发布完。\
                     重试不会有变化，请在「启动设置」里改用一个配套包已发布完整的版本。",
                    pinned = version.unwrap_or(""),
                    drifted = drifted
                ),
                // No pin, or the message named nothing comparable: the plain
                // upstream-gap wording is the accurate one.
                None => "dsh 所需的某个依赖版本在注册表上尚不存在（npm ETARGET）。这是 dsh \
                         官方发布不同步导致的——本次要装的版本已发布，但它依赖的配套包还没跟上。\
                         无需改动配置，稍后重试即可；也可在「启动设置」里改用其它版本。"
                    .to_string(),
            };
            return Err(("version_unavailable".to_string(), message, first.2));
        }
    }
    if let Some(cache_dir) = first.2.as_deref().and_then(corrupt_npx_cache_dir) {
        if let Err(error) = purge_corrupt_npx_cache(&cache_dir) {
            eprintln!("dsh 启动失败，且无法自动修复 npx 缓存：{error}");
            return Err((
                first.0,
                format!("{}（检测到 npx 缓存损坏，自动清理失败：{error}）", first.1),
                first.2,
            ));
        }
        eprintln!(
            "检测到损坏的 npx 缓存（{}），已自动清理并重新尝试启动 dsh。",
            cache_dir.display()
        );
        // The entry was just deleted, so the retry must download: it is a
        // networked attempt, and it must not inherit the cache-hit window that
        // fired this timeout. Being networked, it is also one of the two
        // attempts that carry the closure cutoff.
        let before = resolve_start_cutoff(version);
        return spawn_and_wait(
            app,
            npx,
            overlay,
            port,
            version,
            true,
            false,
            before.as_deref(),
        )
        .map_err(|(issue, message, detail)| {
            (
                issue,
                format!("{message}（已自动清理损坏的 npx 缓存并重试一次。）"),
                detail,
            )
        });
    }

    // A timeout or early exit can leave an incomplete `_npx` entry without an
    // ENOENT path in stderr. Identify it structurally instead: touched during
    // the failed attempt and missing its manifest or dsh payload. This also
    // covers a networked install that downloaded data before failing.
    if failure_may_leave_incomplete_npx_entry(&first) {
        if let Some(cache_dir) = npm_cache_dir() {
            if purge_suspect_npx_cache_entries(&cache_dir, repair_window_start) {
                // Same reasoning as the fast path: the retry is a fresh
                // download, so give it the download window, not the cache-hit
                // window that just fired — and, being networked, the cutoff.
                let before = resolve_start_cutoff(version);
                return spawn_and_wait(
                    app,
                    npx,
                    overlay,
                    port,
                    version,
                    true,
                    false,
                    before.as_deref(),
                )
                .map_err(|(issue, message, detail)| {
                    (
                        issue,
                        format!("{message}（已自动清理疑似中断的 npx 缓存并重试一次。）"),
                        detail,
                    )
                });
            }
        }
    }
    Err(first)
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

/// Terminate the whole process tree and retain every failure for the caller.
/// npx spawns `cmd.exe` → `node` → dsh's own children, so killing only the
/// direct child can leave the actual listener alive.
fn terminate_child(child: &mut Child) -> Vec<String> {
    let pid = child.id();
    let mut errors = Vec::new();
    let child_was_running = match child.try_wait() {
        Ok(Some(_)) => false,
        Ok(None) => true,
        Err(error) => {
            errors.push(format!("无法查询 dsh 启动进程 PID {pid}: {error}"));
            true
        }
    };
    // Never signal an already-reaped PID: Windows may have reused the numeric
    // identifier. Descendants that outlived their wrapper are handled through
    // the listener-PID snapshot in `terminate_managed_process`.
    if child_was_running {
        if let Err(error) = terminate_pid(pid) {
            errors.push(error);
        }
        if child.try_wait().ok().flatten().is_none() {
            if let Err(error) = child.kill() {
                errors.push(format!("无法结束 dsh 启动进程 PID {pid}: {error}"));
            }
        }
    }
    if let Err(error) = child.wait() {
        errors.push(format!("无法回收 dsh 启动进程 PID {pid}: {error}"));
    }
    errors
}

fn wait_for_port_release(access: DshAccess, port: u16, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        if !port_is_occupied(access.probe_host(), port) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Stop one supervised server and prove that its listening socket disappeared.
///
/// `taskkill /T` normally removes the entire npx tree. The listener PID is also
/// snapshotted before that operation, because wrappers can exit or re-parent a
/// descendant while Windows is walking the tree. Only that known PID is used as
/// a fallback, so a new unrelated process that races onto the port is never
/// killed.
fn terminate_managed_process(
    child: &mut Child,
    access: DshAccess,
    port: u16,
) -> Result<(), String> {
    let listener_pids = listening_pids(port);
    let protected = self_process_chain();
    let mut errors = terminate_child(child);

    // `taskkill` is synchronous, but allow socket teardown to become visible
    // before reaching for the listener fallback.
    if wait_for_port_release(access, port, Duration::from_millis(500)) {
        return Ok(());
    }

    let still_listening = listening_pids(port);
    for pid in listener_pids {
        if !still_listening.contains(&pid) {
            continue;
        }
        if protected.contains(&pid) {
            errors.push(format!("监听端口 {port} 的 PID {pid} 属于启动器进程链，已拒绝结束。"));
            continue;
        }
        if let Err(error) = terminate_pid(pid) {
            errors.push(error);
        }
    }

    if wait_for_port_release(access, port, STOP_GRACE) {
        return Ok(());
    }

    let occupant = occupant_label(&listening_processes(port));
    if errors.is_empty() {
        errors.push("进程终止命令已返回，但监听端口没有释放。".to_string());
    }
    Err(format!(
        "端口 {port} 仍被 {occupant} 占用。{}",
        errors.join("；")
    ))
}

fn terminate_server(server: &mut DshServer) -> Result<(), String> {
    terminate_managed_process(&mut server.child, server.access, server.port)
}

fn with_shutdown_error(detail: Option<String>, outcome: Result<(), String>) -> Option<String> {
    let Err(error) = outcome else {
        return detail;
    };
    let shutdown = format!("停止残留 dsh 进程失败：{}", sanitize_diagnostic(&error));
    Some(match detail {
        Some(detail) if !detail.is_empty() => format!("{detail}\n{shutdown}"),
        _ => shutdown,
    })
}

/// The version of an unpinned dsh start, read from the local cache.
///
/// A first-ever start has no recorded pin. Recheck the same package spec and
/// registry offline so this lookup cannot switch to a newer release while the
/// service is starting. npm may still lack the metadata needed to answer a
/// bare `latest` spec offline; in that case the version remains unknown.
fn read_version(npx: &Path, spec: &str) -> Option<String> {
    let output = hidden_command(npx)
        .args(["--offline", "--yes", spec, "-V"])
        .env("NPM_CONFIG_REGISTRY", DSH_NPM_REGISTRY)
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

fn locate_npm() -> Option<PathBuf> {
    crate::platform_env::locate_executable("npm")
}

/// Every published version of the dsh package, newest first.
///
/// `npm view` answers from registry metadata without downloading anything, so
/// this stays a quick read-only call; the actual download still happens on the
/// next start. `--json` is used because the field is a list and the default
/// `npm view` output for a list is not a stable parse target.
///
/// Ordering is **newest first** and is deliberately npm's own order reversed
/// rather than a hand-rolled semver comparison: the list is a picker, and the
/// registry's ordering is the one users see in `npm view` itself. Prereleases
/// are kept — dsh ships almost exclusively prereleases, so filtering them out
/// would leave an empty list.
fn query_available_versions(npm: &Path) -> Result<Vec<String>, String> {
    let output = hidden_command(npm)
        .args(["view", DSH_PACKAGE, "versions", "--json"])
        .output()
        .map_err(|error| format!("无法运行 npm: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail: Vec<&str> = stderr
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(3)
            .collect();
        return Err(format!(
            "查询 dsh 可用版本失败（{}）。请检查网络与代理后重试。",
            if detail.is_empty() {
                format!("npm 退出码 {}", output.status.code().unwrap_or(-1))
            } else {
                detail.join("；")
            }
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // npm prints a bare string instead of a one-element array when exactly one
    // version exists; accept both so a single-version registry is not an error.
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|_| "npm 返回的 dsh 版本列表无法解析。".to_string())?;
    let mut versions: Vec<String> = match parsed {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        serde_json::Value::String(single) => vec![single],
        _ => Vec::new(),
    };
    versions.retain(|version| is_valid_pinned_version(version));
    versions.reverse();
    if versions.is_empty() {
        return Err("npm 返回的 dsh 版本列表为空。".to_string());
    }
    Ok(versions)
}

/// The `latest` dist-tag of the dsh package, from npm registry metadata.
fn query_latest_version(npm: &Path) -> Result<String, String> {
    let output = hidden_command(npm)
        .args(["view", DSH_PACKAGE, "version"])
        .output()
        .map_err(|error| format!("无法运行 npm: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail: Vec<&str> = stderr
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(3)
            .collect();
        return Err(format!(
            "查询 dsh 最新版本失败（{}）。请检查网络与代理后重试。",
            if detail.is_empty() {
                format!("npm 退出码 {}", output.status.code().unwrap_or(-1))
            } else {
                detail.join("；")
            }
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .map(str::trim)
        .find(|line| is_valid_pinned_version(line))
        .map(str::to_string)
        .ok_or_else(|| "npm 返回的 dsh 版本号无法识别。".to_string())
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
        uptime_secs: None,
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
        uptime_secs: None,
        issue: None,
        detail: None,
    }
}

fn stop_outcome_status(
    access: DshAccess,
    port: u16,
    outcome: Result<(), String>,
) -> DshRuntimeStatus {
    match outcome {
        Ok(()) => stopped_status(access, port),
        Err(detail) => failure_status(
            access,
            port,
            "stop_failed",
            &format!(
                "关闭 DeepSeek Harness 失败，端口 {port} 尚未释放。请重试关闭，或确认后使用「一键清理占用」。"
            ),
            Some(sanitize_diagnostic(&detail)),
        ),
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
    // The recorded version is read at the start of the launch so the whole
    // attempt runs against one pin, even if 「更新版本」 moves it meanwhile.
    let pinned = start_pinned_version();

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

    let outcome = match spawn_with_cache_repair(app, &npx, &overlay, port, pinned.as_deref()) {
        Ok(outcome) => outcome,
        Err((issue, message, detail)) => {
            return failure_status(access, port, &issue, &message, detail)
        }
    };

    let SpawnOutcome {
        mut child,
        local_url,
        lan_url,
    } = outcome;
    if shutdown_requested() {
        let detail = with_shutdown_error(
            None,
            terminate_managed_process(&mut child, access, port),
        );
        return failure_status(
            access,
            port,
            "exited",
            "应用正在退出，已关闭刚启动的 dsh 服务。",
            detail,
        );
    }
    // An exact top-level pin already tells us the running release. Re-running
    // npx offline to discover it can fail when npm cached the install under a
    // `--before` resolution, leaving a healthy service with no shown version.
    let version = pinned
        .clone()
        .or_else(|| read_version(&npx, &dsh_package_spec(None)));
    if let Some(version) = version.as_deref() {
        // This is the version record every later start is pinned to. Persisting
        // it is best-effort: a failed write must not turn a healthy start into
        // a failure.
        record_started_version(pinned.as_deref(), version);
    }
    if shutdown_requested() {
        let detail = with_shutdown_error(
            None,
            terminate_managed_process(&mut child, access, port),
        );
        return failure_status(
            access,
            port,
            "exited",
            "应用正在退出，已关闭刚启动的 dsh 服务。",
            detail,
        );
    }

    let mut guard = match lock_registry() {
        Ok(guard) => guard,
        Err(error) => {
            let detail = with_shutdown_error(
                None,
                terminate_managed_process(&mut child, access, port),
            );
            return failure_status(access, port, "spawn_failed", &error, detail);
        }
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
        uptime_secs: Some(0),
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
    // Do not hold the registry while taskkill/process-group shutdown waits. UI
    // status probes should remain responsive and the `busy` flag already keeps
    // another start/stop operation out.
    drop(guard);

    let status = stop_outcome_status(access, port, terminate_server(&mut server));

    if let Ok(mut guard) = lock_registry() {
        guard.failure = (status.phase == DshPhase::Failed).then(|| status.clone());
    }
    status
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
            // 「已运行 …」由前端按 uptime_secs 每秒刷新，这里只给快照值。
            message: "DeepSeek Harness 正在运行。".to_string(),
            uptime_secs: Some(server.started_at.elapsed().as_secs()),
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
                uptime_secs: None,
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
            uptime_secs: None,
            issue: Some("exited".to_string()),
            detail: None,
        }),
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Proof that the dsh CLI is usable: the supervised server is alive right now.
///
/// `cli_runtime`'s capability check probes dsh through `npx … -V`, which costs
/// seconds even on a warm cache; a running supervised service already proves
/// everything that probe would, and knows its version, so the check asks here
/// first. Returns the server's recorded version and the npx path it started
/// with. A dead-but-registered server is reaped by `live_status` and reports
/// as not-running.
pub(crate) fn running_supervised_proof() -> Option<(Option<String>, String)> {
    let mut guard = lock_registry().ok()?;
    let status = live_status(&mut guard)?;
    if status.phase != DshPhase::Running {
        return None;
    }
    Some((status.version, status.executable?))
}

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
///
/// The address list is computed per call rather than stored: an interface can
/// come up (Wi-Fi, a VPN) while the service keeps running, and the panel must
/// not keep showing yesterday's addresses.
#[tauri::command]
pub fn dsh_runtime_urls() -> Result<Option<DshRuntimeUrls>, String> {
    let guard = lock_registry()?;
    Ok(guard.server.as_ref().map(|server| DshRuntimeUrls {
        local_url: server.local_url.clone(),
        remote_url: server.lan_url.clone(),
        addresses: access_addresses(server),
    }))
}

#[tauri::command]
pub async fn dsh_runtime_start(
    app: AppHandle,
    access: String,
    port: u16,
) -> Result<DshRuntimeStatus, String> {
    if shutdown_requested() {
        return Err("应用正在退出，无法再启动 dsh。".to_string());
    }
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
        // The registry lock must NOT be held while the child starts: a first
        // start waits on the npx download (180s of no progress, up to a 30
        // minute ceiling while it keeps growing), and the UI thread calls into
        // the registry (`dsh_embed_show`) while it runs. Only the `busy` flag is
        // needed to keep a second start out.
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
                occupant_listen_scope: None,
            };
        }
        let occupant = describe_occupant(port);
        DshPortStatus {
            available: false,
            occupant: Some(occupant.label),
            occupant_is_dsh: occupant.is_dsh,
            occupant_is_supervised: occupant.is_supervised,
            occupant_listen_scope: Some(occupant_listen_scope(port)),
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

/// 「检查更新」: list the versions the registry offers for the picker, plus
/// the currently pinned one. Read-only — the choice is made in the frontend's
/// version dialog and carried out by `dsh_update_version`.
///
/// `npm view` answers from registry metadata without downloading the package,
/// so this stays quick; the download happens on the next start, under the same
/// progress reporting as any other start.
///
/// This is also the **only** place the launcher talks to the registry during
/// normal operation. Starting dsh is offline-first, so nothing on the launch
/// path depends on the network; a user who never opens this dialog never pays
/// for a registry round-trip.
#[tauri::command]
pub async fn dsh_list_versions() -> Result<DshVersionList, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let Some(npm) = locate_npm() else {
            return Err("未检测到 npm。请先安装 Node.js（含 npm/npx）后重试。".to_string());
        };
        let versions = query_available_versions(&npm)?;
        // The `latest` label is a nicety, not a requirement: the list is fully
        // usable without it, so a failure here must not sink the whole call.
        let latest = query_latest_version(&npm).ok();
        Ok(DshVersionList {
            versions,
            pinned: start_pinned_version(),
            latest,
        })
    })
    .await
    .map_err(|error| format!("获取 dsh 版本列表任务异常结束: {error}"))?
}

/// Read the local runnable dsh releases without contacting the registry.
#[tauri::command]
pub async fn dsh_list_cached_versions() -> Result<Vec<DshCachedVersion>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let cache = npm_cache_dir().ok_or_else(|| "无法确定 npm 缓存目录。".to_string())?;
        let mut versions: Vec<_> = cached_dsh_entries(&cache)?
            .into_iter()
            .map(|(version, entries)| DshCachedVersion { version, entries: entries.len() })
            .collect();
        versions.reverse();
        Ok(versions)
    })
    .await
    .map_err(|error| format!("读取本机 dsh 版本任务异常结束: {error}"))?
}

/// Remove every `_npx` entry containing the selected release. npm's shared
/// tarball cache is deliberately untouched because other packages can use it.
#[tauri::command]
pub async fn dsh_delete_cached_version(version: String) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !is_version_key(&version) {
            return Err("要删除的 dsh 版本号无法识别。".to_string());
        }
        let cache = npm_cache_dir().ok_or_else(|| "无法确定 npm 缓存目录。".to_string())?;
        let mut guard = lock_registry()?;
        if guard.busy {
            return Err("dsh 正在启动或停止，请稍后再删除缓存。".to_string());
        }
        if let Some(status) = live_status(&mut guard) {
            if status.phase == DshPhase::Running
                && (status.version.as_deref() == Some(version.as_str()) || status.version.is_none())
            {
                return Err("该版本正在运行。请先停止 dsh 服务，再删除本机缓存。".to_string());
            }
        }
        delete_cached_dsh_version_from(&cache, &version)
    })
    .await
    .map_err(|error| format!("删除本机 dsh 版本任务异常结束: {error}"))?
}

/// 「检查更新」: resolve the registry's `latest` and compare it with the
/// recorded pin. Read-only — whether to apply the update is decided by the
/// frontend's confirmation dialog and carried out by `dsh_update_version`.
///
/// `npm view` answers from registry metadata without downloading the package,
/// so this check stays quick; the download happens on the next start, under
/// the same progress reporting as any other start.
#[tauri::command]
pub async fn dsh_check_update() -> Result<DshVersionCheck, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let Some(npm) = locate_npm() else {
            return Err("未检测到 npm。请先安装 Node.js（含 npm/npx）后重试。".to_string());
        };
        let latest = query_latest_version(&npm)?;
        let pinned = start_pinned_version();
        let update_available = pinned.as_deref() != Some(latest.as_str());
        let message = if update_available {
            match pinned.as_deref() {
                Some(old) => format!("发现新版本：v{old} → v{latest}。"),
                None => format!("尚未固定版本，注册表最新版本为 v{latest}。"),
            }
        } else {
            format!("当前已是最新版本（v{latest}）。")
        };
        Ok(DshVersionCheck {
            latest,
            pinned,
            update_available,
            message,
        })
    })
    .await
    .map_err(|error| format!("检查 dsh 更新任务异常结束: {error}"))?
}

/// 「更新版本」的确认后一半: record `version` (the value `dsh_check_update`
/// just reported and the user confirmed) as the pinned version.
///
/// The version comes from the check instead of being re-queried here: the
/// dialog showed that exact version, and re-resolving `latest` could pin a
/// release the user never saw. Deliberately not tied to the start/stop lock:
/// the pin only takes effect on the *next* start, and a start already in
/// flight keeps the pin it began with (`record_started_version` refuses to
/// overwrite a pin that moved). Restarting a running service is the
/// frontend's follow-up decision, not part of this command.
#[tauri::command]
pub async fn dsh_update_version(version: String) -> Result<DshVersionUpdate, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let version = version.trim().to_string();
        if !is_valid_pinned_version(&version) {
            return Err("要固定的 dsh 版本号无法识别，未做任何更改。".to_string());
        }
        let previous = start_pinned_version();
        if previous.as_deref() == Some(version.as_str()) {
            return Ok(DshVersionUpdate {
                message: format!("v{version} 已是当前固定版本。"),
                version,
                previous,
                changed: false,
            });
        }
        crate::persistent_state::save_dsh_pinned_version(&version)?;
        let message = match previous.as_deref() {
            Some(old) => {
                format!("已更新 dsh 版本：v{old} → v{version}。重启 dsh 服务后生效。")
            }
            None => format!("已固定 dsh 版本 v{version}，下次启动将使用该版本。"),
        };
        Ok(DshVersionUpdate {
            version,
            previous,
            changed: true,
            message,
        })
    })
    .await
    .map_err(|error| format!("更新 dsh 版本任务异常结束: {error}"))?
}

/// Stop the managed server during application shutdown.
pub fn stop() {
    request_shutdown();
    // A start may still be downloading or waiting for its ready line. Give that
    // worker time to observe the shutdown flag and run the same verified cleanup
    // before the application process disappears underneath it.
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let Ok(mut guard) = registry().lock() else {
            return;
        };
        let server = guard.server.take();
        let busy = guard.busy;
        drop(guard);

        if let Some(mut server) = server {
            if let Err(error) = terminate_server(&mut server) {
                eprintln!("退出应用时未能完全关闭 dsh：{}", sanitize_diagnostic(&error));
            }
            return;
        }
        if !busy {
            return;
        }
        if Instant::now() >= deadline {
            eprintln!("退出应用时等待 dsh 启动任务结束超时。");
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
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

    /// A native `<cache>/_npx` root for cache-repair tests: the parser works on
    /// real `Path` components, so each platform builds its own spelling.
    fn npx_root() -> PathBuf {
        let base = if cfg!(windows) {
            "C:\\npm-cache"
        } else {
            "/home/tester/.npm"
        };
        Path::new(base).join("_npx")
    }

    fn npx_entry(key: &str) -> PathBuf {
        npx_root().join(key)
    }

    /// The stderr block npm 11 prints for the corrupt-cache failure.
    fn npm_enoent_detail(package_json: &Path) -> String {
        format!(
            "npm error code ENOENT\n\
             npm error syscall open\n\
             npm error path {}\n\
             npm error errno -4058\n\
             npm error enoent Could not read package.json: Error: ENOENT: no such file or directory, open '{}'\n\
             npm error enoent This is related to npm not being able to find a file.\n\
             npm error enoent \n\
             npm error A complete log of this run can be found in: C:\\logs\\debug-0.log",
            package_json.display(),
            package_json.display()
        )
    }

    #[test]
    fn closure_drift_is_detected_when_a_pin_resolves_forward() {
        // The exact 2026-09-22 shape: pin rc.1, npm resolves a rc.3 sibling.
        let detail = "npm error code ETARGET\n\
                      npm error notarget No matching version found for @deepseek-ai/dsh-client-ui-sidebar-documentpreview@^0.1.5-rc.3.";
        assert_eq!(
            closure_drifted_from_pin(detail, Some("0.1.5-rc.1")).as_deref(),
            Some("0.1.5-rc.3")
        );
    }

    #[test]
    fn closure_drift_is_detected_for_the_agent_closure_shape() {
        // The first error this project ever saw, from the same gap.
        let detail = "npm error code ETARGET\n\
                      npm error notarget No matching version found for @deepseek-ai/dsh-session-projection@^0.1.5-rc.3.";
        assert_eq!(
            closure_drifted_from_pin(detail, Some("0.1.5-rc.1")).as_deref(),
            Some("0.1.5-rc.3")
        );
    }

    #[test]
    fn closure_drift_needs_a_pin_to_compare_against() {
        // Without a recorded pin the launcher asked for `latest`; there is no
        // requested version for the message to have drifted *from*, so the
        // generic wording must be used instead of inventing a comparison.
        let detail = "npm error code ETARGET\n\
                      npm error notarget No matching version found for @deepseek-ai/dsh-session-projection@^0.1.5-rc.3.";
        assert_eq!(closure_drifted_from_pin(detail, None), None);
    }

    #[test]
    fn closure_drift_ignores_a_failure_inside_the_pinned_wave() {
        // npm failing on the pinned version itself names the same version, not
        // a later one — that is not drift.
        let detail = "npm error code ETARGET\n\
                      npm error notarget No matching version found for @deepseek-ai/dsh@^0.1.5-rc.1.";
        assert_eq!(closure_drifted_from_pin(detail, Some("0.1.5-rc.1")), None);
    }

    #[test]
    fn closure_drift_ignores_an_unrelated_version_line() {
        // A different release core is a different package's problem, not the
        // pinned ladder being walked forward.
        let detail = "npm error code ETARGET\n\
                      npm error notarget No matching version found for @deepseek-ai/dsh@^0.1.6-alpha.1.";
        assert_eq!(closure_drifted_from_pin(detail, Some("0.1.5-rc.1")), None);
    }

    #[test]
    fn prerelease_core_splits_the_ladder_from_the_core() {
        assert_eq!(prerelease_core("0.1.5-rc.1"), "0.1.5");
        assert_eq!(prerelease_core("0.1.6-alpha.2"), "0.1.6");
        assert_eq!(prerelease_core("1.2.3"), "1.2.3");
    }

    #[test]
    fn unresolvable_version_failure_is_read_from_the_etarget_block() {
        // The real shape observed on 2026-09-22: dsh's own peer closure named a
        // sibling release that had not been published yet.
        let detail = "npm error code ETARGET\n\
                      npm error notarget No matching version found for @deepseek-ai/dsh-session-projection@^0.1.5-rc.3.\n\
                      npm error notarget In most cases you or one of your dependencies are requesting\n\
                      npm error notarget a package version that doesn't exist.";
        assert!(is_unresolvable_version_failure(detail));
    }

    #[test]
    fn unresolvable_version_failure_accepts_the_bare_notarget_line() {
        // npm also prints `notarget` without the `code ETARGET` line depending
        // on which error path reports it; either spelling must classify.
        let detail = "npm error notarget No matching version found for @deepseek-ai/dsh-agent@^0.1.5-rc.3.";
        assert!(is_unresolvable_version_failure(detail));
    }

    #[test]
    fn unresolvable_version_failure_rejects_the_corrupt_cache_shape() {
        // The two failures must never overlap: an ENOENT block is a corrupt
        // entry that *should* be purged, and must not be reported as an
        // upstream publishing gap (which would leave the entry in place and
        // make every later start fail the same way).
        let detail = npm_enoent_detail(&npx_entry("1da1392061ab1944").join("package.json"));
        assert!(!is_unresolvable_version_failure(&detail));
        assert!(corrupt_npx_cache_dir(&detail).is_some());
    }

    #[test]
    fn etarget_failure_names_no_cache_entry_to_purge() {
        // Nothing about a version-resolution failure points at a directory, so
        // the repair chain must find no target and leave the cache untouched.
        let detail = "npm error code ETARGET\n\
                      npm error notarget No matching version found for @deepseek-ai/dsh@^0.1.5-rc.3.";
        assert_eq!(corrupt_npx_cache_dir(detail), None);
    }

    #[test]
    fn corrupt_cache_dir_is_read_from_the_npm_enoent_block() {
        let entry = npx_entry("1da1392061ab1944");
        let detail = npm_enoent_detail(&entry.join("package.json"));
        assert_eq!(corrupt_npx_cache_dir(&detail).as_deref(), Some(entry.as_path()));
    }

    #[test]
    fn corrupt_cache_dir_accepts_the_bare_path_line() {
        let entry = npx_entry("b86ed90107c62dab");
        let detail = format!(
            "npm error code ENOENT\nnpm error path {}",
            entry.join("package.json").display()
        );
        assert_eq!(corrupt_npx_cache_dir(&detail).as_deref(), Some(entry.as_path()));
    }

    #[test]
    fn corrupt_cache_dir_rejects_unrelated_or_unsafe_paths() {
        // An ENOENT about a file outside the npx cache is not this failure.
        let outside = if cfg!(windows) {
            "D:\\project\\package.json"
        } else {
            "/tmp/project/package.json"
        };
        assert_eq!(corrupt_npx_cache_dir(&npm_enoent_detail(Path::new(outside))), None);
        // A package.json directly inside `_npx` names no entry directory, and
        // deleting `_npx` itself must never be on the table.
        assert_eq!(
            corrupt_npx_cache_dir(&npm_enoent_detail(&npx_root().join("package.json"))),
            None
        );
        // A different missing file inside an entry is not the signature.
        assert_eq!(
            corrupt_npx_cache_dir(&npm_enoent_detail(&npx_entry("abc").join("index.js"))),
            None
        );
        // Without the ENOENT marker the lines are not treated as this failure.
        let detail = format!("npm error path {}", npx_entry("abc").join("package.json").display());
        assert_eq!(corrupt_npx_cache_dir(&detail), None);
    }

    #[test]
    fn purge_removes_a_proven_corrupt_entry() {
        let directory = tempfile::tempdir().expect("temp dir");
        let entry = directory.path().join("_npx").join("1da1392061ab1944");
        std::fs::create_dir_all(entry.join("node_modules")).expect("create corrupt entry");
        std::fs::write(entry.join("node_modules").join("dsh.js"), "x").expect("write payload");
        purge_corrupt_npx_cache(&entry).expect("purge should succeed");
        assert!(!entry.exists());
    }

    #[test]
    fn purge_refuses_an_entry_with_a_package_json() {
        let directory = tempfile::tempdir().expect("temp dir");
        let entry = directory.path().join("_npx").join("healthy");
        std::fs::create_dir_all(&entry).expect("create entry");
        std::fs::write(entry.join("package.json"), "{}").expect("write package.json");
        let error = purge_corrupt_npx_cache(&entry).expect_err("healthy entry must be kept");
        assert!(error.contains("并未损坏"));
        assert!(entry.join("package.json").exists());
    }

    #[test]
    fn purge_refuses_a_path_that_is_not_an_npx_cache_entry() {
        let directory = tempfile::tempdir().expect("temp dir");
        let outside = directory.path().join("projects").join("app");
        std::fs::create_dir_all(&outside).expect("create outside dir");
        let error = purge_corrupt_npx_cache(&outside).expect_err("outside path must be refused");
        assert!(error.contains("已拒绝清理"));
        assert!(outside.exists());
        // The `_npx` root itself is not an entry either.
        let root = directory.path().join("_npx");
        std::fs::create_dir_all(&root).expect("create _npx root");
        assert!(purge_corrupt_npx_cache(&root).is_err());
        assert!(root.exists());
    }

    #[test]
    fn purge_tolerates_an_entry_someone_else_already_removed() {
        let directory = tempfile::tempdir().expect("temp dir");
        let entry = directory.path().join("_npx").join("gone");
        purge_corrupt_npx_cache(&entry).expect("a missing entry is already clean");
    }

    // ── readiness-timeout self-heal ────────────────────────────────────────

    /// Build `_npx/<key>` under a temp cache root with the given structure.
    fn cache_entry(root: &Path, key: &str, manifest: bool, payload: bool) -> PathBuf {
        let entry = root.join("_npx").join(key);
        std::fs::create_dir_all(&entry).expect("create entry");
        if manifest {
            std::fs::write(entry.join("package.json"), "{}").expect("write manifest");
        }
        if payload {
            let package = entry.join("node_modules").join("@deepseek-ai").join("dsh");
            std::fs::create_dir_all(&package).expect("create payload");
            std::fs::write(package.join("index.js"), "x").expect("write payload file");
        }
        entry
    }

    fn runnable_cache_entry(root: &Path, key: &str, version: &str) -> PathBuf {
        let entry = cache_entry(root, key, true, true);
        std::fs::write(
            entry.join("package.json"),
            format!(r#"{{"dependencies":{{"@deepseek-ai/dsh":"^{version}"}}}}"#),
        ).expect("write npx manifest");
        std::fs::write(
            entry.join("node_modules").join("@deepseek-ai").join("dsh").join("package.json"),
            format!(r#"{{"name":"@deepseek-ai/dsh","version":"{version}"}}"#),
        ).expect("write dsh manifest");
        entry
    }

    #[test]
    fn local_cache_groups_versions_and_delete_removes_only_the_chosen_release() {
        let cache = tempfile::tempdir().expect("temp cache");
        let first = runnable_cache_entry(cache.path(), "first", "0.1.5-rc.1");
        let second = runnable_cache_entry(cache.path(), "second", "0.1.5-rc.1");
        let other = runnable_cache_entry(cache.path(), "other", "0.1.6-alpha.1");
        let incomplete = cache_entry(cache.path(), "incomplete", true, false);
        let versions = cached_dsh_entries(cache.path()).expect("scan cache");
        assert_eq!(versions["0.1.5-rc.1"].len(), 2);
        assert_eq!(versions["0.1.6-alpha.1"].len(), 1);
        assert_eq!(versions.len(), 2);

        assert_eq!(delete_cached_dsh_version_from(cache.path(), "0.1.5-rc.1").unwrap(), 2);
        assert!(!first.exists());
        assert!(!second.exists());
        assert!(other.exists());
        assert!(incomplete.exists());
        assert!(delete_cached_dsh_version_from(cache.path(), "0.1.5-rc.1").is_err());
    }

    #[test]
    fn local_cache_ignores_an_unrelated_npx_package() {
        let cache = tempfile::tempdir().expect("temp cache");
        let entry = runnable_cache_entry(cache.path(), "unrelated", "0.1.5-rc.1");
        std::fs::write(entry.join("package.json"), r#"{"dependencies":{"another-package":"1.0.0"}}"#)
            .expect("replace npx manifest");
        assert!(cached_dsh_entries(cache.path()).unwrap().is_empty());
        assert!(entry.exists());
    }

    #[test]
    fn local_cache_refuses_a_non_directory_npx_root() {
        let cache = tempfile::tempdir().expect("temp cache");
        std::fs::write(cache.path().join("_npx"), "not a directory").expect("write file");
        assert!(cached_dsh_entries(cache.path()).is_err());
    }

    #[test]
    fn timeout_scan_flags_an_entry_that_is_missing_its_manifest() {
        let cache = tempfile::tempdir().expect("temp cache");
        let entry = cache_entry(cache.path(), "1da1392061ab1944", false, false);
        let suspects = suspect_npx_cache_entries(cache.path(), Instant::now() - Duration::from_secs(1));
        assert_eq!(suspects, vec![entry]);
    }

    #[test]
    fn timeout_scan_flags_an_entry_that_is_missing_its_dsh_payload() {
        let cache = tempfile::tempdir().expect("temp cache");
        let entry = cache_entry(cache.path(), "1e7f6d9597241db0", true, false);
        let suspects = suspect_npx_cache_entries(cache.path(), Instant::now() - Duration::from_secs(1));
        assert_eq!(suspects, vec![entry]);
    }

    #[test]
    fn timeout_scan_keeps_a_structurally_complete_entry() {
        let cache = tempfile::tempdir().expect("temp cache");
        cache_entry(cache.path(), "healthy", true, true);
        let suspects = suspect_npx_cache_entries(cache.path(), Instant::now() - Duration::from_secs(1));
        assert!(suspects.is_empty());
    }

    #[test]
    fn timeout_scan_ignores_entries_older_than_the_attempt_window() {
        let cache = tempfile::tempdir().expect("temp cache");
        let entry = cache_entry(cache.path(), "stale", false, false);
        // A window that starts *after* the entry was built must not claim it:
        // it belongs to some earlier attempt, not to the one that timed out.
        let suspects = suspect_npx_cache_entries(cache.path(), Instant::now() + Duration::from_secs(3600));
        assert!(suspects.is_empty());
        assert!(entry.exists());
    }

    #[test]
    fn timeout_scan_flags_an_entry_whose_own_mtime_is_inside_the_window() {
        let cache = tempfile::tempdir().expect("temp cache");
        // The start touched the entry directory itself but never wrote a
        // manifest or payload — the classic lock-file-only remnant.
        let entry = cache.path().join("_npx").join("1da1392061ab1944");
        std::fs::create_dir_all(&entry).expect("create entry");
        let suspects = suspect_npx_cache_entries(cache.path(), Instant::now() - Duration::from_secs(1));
        assert_eq!(suspects, vec![entry]);
    }

    #[test]
    fn timeout_purge_removes_suspects_and_reports_progress() {
        let cache = tempfile::tempdir().expect("temp cache");
        let corrupt = cache_entry(cache.path(), "corrupt", false, false);
        let payload_less = cache_entry(cache.path(), "payloadless", true, false);
        let complete = cache_entry(cache.path(), "complete", true, true);
        assert!(purge_suspect_npx_cache_entries(
            cache.path(),
            Instant::now() - Duration::from_secs(1)
        ));
        assert!(!corrupt.exists());
        assert!(!payload_less.exists());
        assert!(complete.exists());
    }

    #[test]
    fn timeout_purge_reports_nothing_to_do_on_a_healthy_cache() {
        let cache = tempfile::tempdir().expect("temp cache");
        cache_entry(cache.path(), "complete", true, true);
        assert!(!purge_suspect_npx_cache_entries(
            cache.path(),
            Instant::now() - Duration::from_secs(1)
        ));
    }

    #[test]
    fn timeout_purge_refuses_an_entry_that_healed_between_scan_and_delete() {
        let directory = tempfile::tempdir().expect("temp dir");
        let entry = cache_entry(directory.path(), "healed", true, true);
        let error = purge_incomplete_npx_entry(&entry).expect_err("complete entry must be kept");
        assert!(error.contains("结构完整"));
        assert!(entry.exists());
    }

    #[test]
    fn spawn_issue_recognizes_only_the_ready_timeout() {
        let timeout: SpawnFailure = ("ready_timeout".to_string(), "msg".to_string(), None);
        assert_eq!(spawn_failure_issue(&timeout), SpawnIssue::ReadyTimeout);
        let exit: SpawnFailure = ("exited".to_string(), "msg".to_string(), None);
        assert_eq!(spawn_failure_issue(&exit), SpawnIssue::Other);
        let spawn: SpawnFailure = ("spawn_failed".to_string(), "msg".to_string(), None);
        assert_eq!(spawn_failure_issue(&spawn), SpawnIssue::Other);
    }

    #[test]
    fn early_exit_after_a_partial_download_earns_guarded_cache_repair() {
        let failure: SpawnFailure = ("exited".to_string(), "npm exited".to_string(), None);
        assert!(failure_may_leave_incomplete_npx_entry(&failure));
        let spawn_failed: SpawnFailure = ("spawn_failed".to_string(), "msg".to_string(), None);
        assert!(!failure_may_leave_incomplete_npx_entry(&spawn_failed));

        let cache = tempfile::tempdir().expect("temp cache");
        let incomplete = cache_entry(cache.path(), "partial", true, false);
        let complete = cache_entry(cache.path(), "healthy", true, true);
        assert!(purge_suspect_npx_cache_entries(
            cache.path(),
            Instant::now() - Duration::from_secs(1)
        ));
        assert!(!incomplete.exists());
        assert!(complete.exists());
    }

    fn address(ip: &str, interface: Option<&str>, has_gateway: bool) -> LocalAddress {
        LocalAddress {
            ip: ip.parse().expect("test address should parse"),
            interface: interface.map(str::to_string),
            has_gateway,
        }
    }

    #[test]
    fn addresses_are_classified_by_range_before_anything_else() {
        assert_eq!(
            classify_address("127.0.0.1".parse().unwrap(), None),
            Some(DshAddressKind::Loopback)
        );
        // 100.64.0.0/10 is Tailscale's CGNAT range: a LAN peer cannot open it,
        // which is why it is never filed under 局域网.
        assert_eq!(
            classify_address("100.101.102.103".parse().unwrap(), None),
            Some(DshAddressKind::Tailscale)
        );
        assert_eq!(
            classify_address("100.63.255.255".parse().unwrap(), None),
            Some(DshAddressKind::Other)
        );
        assert_eq!(
            classify_address("100.128.0.1".parse().unwrap(), None),
            Some(DshAddressKind::Other)
        );
        for lan in ["10.1.2.3", "172.16.0.9", "172.31.255.254", "192.168.1.5"] {
            assert_eq!(
                classify_address(lan.parse().unwrap(), None),
                Some(DshAddressKind::Lan),
                "{lan} is RFC 1918 space"
            );
        }
        for other in ["172.15.0.1", "172.32.0.1", "192.169.1.1", "8.8.8.8"] {
            assert_eq!(
                classify_address(other.parse().unwrap(), None),
                Some(DshAddressKind::Other),
                "{other} is not private space"
            );
        }
        // A renamed Tailscale adapter is still Tailscale, and an address nothing
        // can reach is not offered at all.
        assert_eq!(
            classify_address("192.168.1.5".parse().unwrap(), Some("Tailscale")),
            Some(DshAddressKind::Tailscale)
        );
        assert_eq!(classify_address("169.254.10.20".parse().unwrap(), None), None);
        assert_eq!(classify_address("0.0.0.0".parse().unwrap(), None), None);
        assert_eq!(classify_address("224.0.0.1".parse().unwrap(), None), None);
    }

    /// The enumeration is OS-dependent and may legitimately come back empty on a
    /// locked-down host, but calling it must be harmless and must never produce
    /// an address that is not a host address at all. Run with `--nocapture` to
    /// see what this machine reports and how the list comes out.
    #[test]
    fn the_host_address_enumeration_is_safe_to_call() {
        let found = local_ipv4_addresses();
        let summary: Vec<String> = found
            .iter()
            .map(|entry| {
                let interface = entry.interface.clone().unwrap_or_else(|| "?".to_string());
                let kind = classify_address(entry.ip, entry.interface.as_deref());
                format!("{} [{interface}] {kind:?}", entry.ip)
            })
            .collect();
        eprintln!(
            "[dsh] enumerated {} local IPv4 address(es): {}",
            found.len(),
            summary.join(", ")
        );
        for entry in &found {
            assert!(!entry.ip.is_multicast());
            assert!(!entry.ip.is_unspecified());
            assert!(!entry.ip.is_broadcast());
        }

        let listed = build_access_addresses(
            DshAccess::Remote,
            3080,
            "http://127.0.0.1:3080/?token=smoke",
            None,
            found,
        );
        let order: Vec<String> = listed
            .iter()
            .map(|entry| format!("{:?} {}", entry.kind, entry.address))
            .collect();
        eprintln!("[dsh] access list: {}", order.join(" | "));
        // 本机 is what 「打开网页」 falls back to, so it is always listed first.
        assert_eq!(listed.first().map(|entry| entry.kind), Some(DshAddressKind::Loopback));
        assert!(listed.iter().all(|entry| entry.url.ends_with("/?token=smoke")));
    }

    #[test]
    fn ready_line_urls_are_taken_apart_without_touching_the_token() {
        let url = "http://192.168.1.5:3080/?token=a.b%2Fc-d";
        assert_eq!(url_scheme(url), Some("http"));
        assert_eq!(url_authority_host(url).as_deref(), Some("192.168.1.5"));
        assert_eq!(url_token(url).as_deref(), Some("a.b%2Fc-d"));
        assert_eq!(url_authority_host("http://[::1]:3080/?token=x").as_deref(), Some("::1"));
        assert_eq!(url_authority_host("http://host/?token=x").as_deref(), Some("host"));
        assert_eq!(url_token("http://127.0.0.1:3080/"), None);
        assert_eq!(url_authority_host("not a url"), None);
    }

    /// The whole point of the list: a machine running Tailscale must still offer
    /// its LAN address first, and it must be the address another device can
    /// actually open.
    #[test]
    fn the_lan_address_wins_the_default_slot_over_tailscale() {
        let listed = build_access_addresses(
            DshAccess::Remote,
            3080,
            "http://127.0.0.1:3080/?token=t1",
            Some("http://100.101.102.103:3080/?token=t1"),
            vec![
                address("100.101.102.103", Some("Tailscale"), false),
                address("192.168.1.5", Some("WLAN"), true),
            ],
        );

        let order: Vec<DshAddressKind> = listed.iter().map(|entry| entry.kind).collect();
        assert_eq!(
            order,
            vec![
                DshAddressKind::Loopback,
                DshAddressKind::Lan,
                DshAddressKind::Tailscale,
            ]
        );
        assert_eq!(listed[1].address, "192.168.1.5");
        assert_eq!(listed[1].interface.as_deref(), Some("WLAN"));
        assert_eq!(listed[2].address, "100.101.102.103");
        for entry in &listed {
            assert!(
                entry.url.ends_with("/?token=t1"),
                "{} must keep the access token",
                entry.url
            );
        }
        assert_eq!(listed[1].url, "http://192.168.1.5:3080/?token=t1");
    }

    /// A real interface (one with a gateway) outranks a hypervisor's virtual
    /// switch even when the enumeration lists the virtual one first.
    #[test]
    fn gateway_bearing_adapters_are_listed_before_virtual_ones() {
        let listed = build_access_addresses(
            DshAccess::Remote,
            3080,
            "http://127.0.0.1:3080/?token=t",
            None,
            vec![
                address("192.168.56.1", Some("VirtualBox Host-Only"), false),
                address("192.168.1.5", Some("WLAN"), true),
            ],
        );
        let lan: Vec<&str> = listed
            .iter()
            .filter(|entry| entry.kind == DshAddressKind::Lan)
            .map(|entry| entry.address.as_str())
            .collect();
        assert_eq!(lan, vec!["192.168.1.5", "192.168.56.1"]);
    }

    /// dsh's own LAN URL is kept even when the enumeration missed it, and the
    /// loopback address is always present exactly once.
    #[test]
    fn the_reported_lan_address_is_kept_and_duplicates_are_dropped() {
        let listed = build_access_addresses(
            DshAccess::Remote,
            3199,
            "http://127.0.0.1:3199/?token=t",
            Some("http://10.0.0.7:3199/?token=t"),
            vec![address("127.0.0.1", None, false), address("10.0.0.7", Some("以太网"), true)],
        );
        assert_eq!(listed.len(), 2, "the duplicate 10.0.0.7 must appear once");
        let lan = listed
            .iter()
            .find(|entry| entry.kind == DshAddressKind::Lan)
            .expect("10.0.0.7 should be listed as LAN");
        // The enumerated entry wins, so its interface name survives.
        assert_eq!(lan.interface.as_deref(), Some("以太网"));
        assert_eq!(lan.url, "http://10.0.0.7:3199/?token=t");
        assert_eq!(
            listed
                .iter()
                .filter(|entry| entry.kind == DshAddressKind::Loopback)
                .count(),
            1
        );
    }

    /// 本地 mode binds `127.0.0.1`, so listing LAN or Tailscale addresses would
    /// hand out links that cannot connect.
    #[test]
    fn a_local_only_service_lists_nothing_but_loopback() {
        let listed = build_access_addresses(
            DshAccess::Local,
            3080,
            "http://127.0.0.1:3080/?token=t",
            None,
            vec![address("192.168.1.5", Some("WLAN"), true)],
        );
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].kind, DshAddressKind::Loopback);
        assert_eq!(listed[0].url, "http://127.0.0.1:3080/?token=t");
    }

    /// A URL without the token lands on dsh's 401 page, so a detail URL with no
    /// token yields no list at all rather than unusable entries.
    #[test]
    fn the_access_list_is_empty_without_a_token() {
        let listed = build_access_addresses(
            DshAccess::Remote,
            3080,
            "http://127.0.0.1:3080/",
            None,
            vec![address("192.168.1.5", Some("WLAN"), true)],
        );
        assert!(listed.is_empty());
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
        let args = dsh_arguments(Path::new("/tmp/overlay.yml"), 3199, None, false, None);
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
    fn dsh_arguments_pin_the_package_to_the_recorded_version() {
        let args = dsh_arguments(Path::new("/tmp/overlay.yml"), 3199, Some("0.1.0"), false, None);
        assert_eq!(args[1], "@deepseek-ai/dsh@0.1.0");
        assert_eq!(args[0], "--yes");
        assert_eq!(args[2], "web");

        assert_eq!(dsh_package_spec(None), "@deepseek-ai/dsh");
        assert_eq!(dsh_package_spec(Some("0.2.0-beta.1")), "@deepseek-ai/dsh@0.2.0-beta.1");
    }

    #[test]
    fn dsh_arguments_put_offline_ahead_of_the_package_spec() {
        // npm must see `--offline` before the spec it applies to; the app's own
        // flags still trail the package name.
        let args = dsh_arguments(
            Path::new("/tmp/overlay.yml"),
            3199,
            Some("0.1.5-rc.1"),
            true,
            None,
        );
        assert_eq!(args[0], "--offline");
        assert_eq!(args[1], "--yes");
        assert_eq!(args[2], "@deepseek-ai/dsh@0.1.5-rc.1");
        assert_eq!(args[3], "web");
        assert!(!dsh_arguments(
            Path::new("/tmp/overlay.yml"),
            3199,
            Some("0.1.5-rc.1"),
            false,
            None
        )
        .contains(&"--offline".to_string()));
    }

    #[test]
    fn dsh_arguments_carry_the_cutoff_only_when_resolving_online() {
        // `--before` changes what npm may *resolve*, so it belongs on the
        // networked attempt and nowhere else. An offline attempt reads the
        // cached entry instead, and putting the flag there would be noise the
        // reader has to reason about for no effect.
        let args = dsh_arguments(
            Path::new("/tmp/overlay.yml"),
            3199,
            Some("0.1.5-rc.1"),
            false,
            Some("2026-09-16T10:26:16Z"),
        );
        assert_eq!(args[0], "--before=2026-09-16T10:26:16Z");
        assert_eq!(args[1], "--yes");
        assert_eq!(args[2], "@deepseek-ai/dsh@0.1.5-rc.1");
        assert_eq!(args[3], "web");

        // Offline wins the slot: npm sees `--offline`, and the cutoff is gone.
        let offline = dsh_arguments(
            Path::new("/tmp/overlay.yml"),
            3199,
            Some("0.1.5-rc.1"),
            true,
            Some("2026-09-16T10:26:16Z"),
        );
        assert_eq!(offline[0], "--offline");
        assert_eq!(offline[1], "--yes");
        assert!(!offline.iter().any(|arg| arg.starts_with("--before")));
    }

    #[test]
    fn version_keys_are_told_apart_from_the_time_objects_metadata() {
        // `created` and `modified` live in the same object as the version keys
        // and would pass a charset-only check, because they are all
        // alphanumeric. Treating them as versions would put a bogus entry in
        // the timeline and corrupt the cutoff arithmetic.
        assert!(is_version_key("0.1.5-rc.1"));
        assert!(is_version_key("0.1.6-alpha.2"));
        assert!(is_version_key("1.2.3"));
        assert!(!is_version_key("created"));
        assert!(!is_version_key("modified"));
        assert!(!is_version_key("0"));
    }

    fn at(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .expect("test timestamps are valid rfc3339")
            .with_timezone(&Utc)
    }

    /// The measured `0.1.5` timeline, trimmed to the entries the cutoff uses.
    /// The wave shape is the whole reason the rule is a midpoint: the top
    /// package publishes last within its own wave.
    fn observed_timeline() -> Vec<(String, DateTime<Utc>)> {
        vec![
            ("0.1.5-rc.1".to_string(), at("2026-09-10T03:12:53Z")),
            ("0.1.6-alpha.1".to_string(), at("2026-09-15T03:23:13Z")),
            ("0.1.5-rc.2".to_string(), at("2026-09-10T14:57:10Z")),
            ("0.1.6-alpha.2".to_string(), at("2026-09-17T13:52:10Z")),
            ("0.1.5-rc.3".to_string(), at("2026-09-22T05:55:20Z")),
        ]
    }

    #[test]
    fn release_cutoff_lands_between_the_pin_and_the_next_prerelease() {
        let cutoff = release_cutoff(&observed_timeline(), "0.1.5-rc.2").expect("a cutoff exists");
        // Midpoint of rc.2 (09-10 14:57:10) and rc.3 (09-22 05:55:20) is
        // 09-16 10:26:15, i.e. ~6 days clear of either wave — far outside the
        // ~16 minute window in which a wave actually lands.
        assert_eq!(cutoff, "2026-09-16T10:26:15Z");
        let cutoff = at(&cutoff);
        assert!(cutoff > at("2026-09-10T14:57:10Z"), "must include the pin");
        assert!(cutoff < at("2026-09-22T05:55:20Z"), "must exclude the next wave");
    }

    #[test]
    fn release_cutoff_ignores_other_release_cores() {
        // `0.1.6-alpha.1` was published between rc.1 and rc.2. It is a different
        // ladder, so it must not be mistaken for rc.1's successor — otherwise
        // every rc.1 pin would be cut off days too early, and only by luck
        // after rc.2.
        let cutoff = release_cutoff(&observed_timeline(), "0.1.5-rc.1").expect("a cutoff exists");
        assert_eq!(cutoff, "2026-09-10T09:05:01Z");
        assert!(at(&cutoff) > at("2026-09-10T03:12:53Z"));
        assert!(at(&cutoff) < at("2026-09-10T14:57:10Z"));
    }

    #[test]
    fn release_cutoff_is_absent_for_the_newest_prerelease() {
        // Nothing follows rc.3, so there is no wave to cut it off from. The
        // honest answer is "cannot help", not a cutoff invented from the whole
        // version list — an arbitrary one would silently hide valid versions.
        assert_eq!(release_cutoff(&observed_timeline(), "0.1.5-rc.3"), None);
    }

    #[test]
    fn release_cutoff_needs_the_pin_itself_in_the_timeline() {
        // A pin the registry has since removed (or a timeline that failed to
        // parse its entry) must not produce a cutoff at all: without the pin's
        // own instant there is no anchor, and any value would be a guess.
        assert_eq!(release_cutoff(&observed_timeline(), "0.1.5-rc.9"), None);
    }

    #[test]
    fn release_cutoff_uses_the_earliest_later_publish_not_the_next_number() {
        // Waves have landed out of order, so "the next version number" is the
        // wrong anchor — the goal is to clear the entire following wave, which
        // means cutting at its first package, whichever number that carries.
        let times = vec![
            ("0.1.5-rc.1".to_string(), at("2026-01-01T00:00:00Z")),
            ("0.1.5-rc.3".to_string(), at("2026-01-02T00:00:00Z")),
            ("0.1.5-rc.2".to_string(), at("2026-01-09T00:00:00Z")),
        ];
        let cutoff = release_cutoff(&times, "0.1.5-rc.1").expect("a cutoff exists");
        // Midpoint with rc.3 (the earliest later publish), not with rc.2.
        assert_eq!(cutoff, "2026-01-01T12:00:00Z");
    }

    #[test]
    fn offline_cache_miss_is_read_from_the_enotcached_block() {
        // The real shape npm prints when `--offline` cannot satisfy the spec.
        let detail = "npm error code ENOTCACHED\n\
                      npm error request to https://registry.npmjs.org/left-pad failed: cache mode is 'only-if-cached' but no cached response is available.";
        assert!(is_offline_cache_miss(detail));
    }

    #[test]
    fn a_partially_cached_closure_still_earns_a_networked_retry() {
        // Measured: with metadata cached but the closure unsatisfiable, npm
        // reports ETARGET offline — identical text to the upstream-gap case.
        // Offline cannot tell the two apart, so the retry gate must not try to:
        // it has to fire on both or a stale cached metadata set would wedge the
        // start forever.
        let etarget: SpawnFailure = (
            "exited".to_string(),
            "dsh 进程在就绪前退出。".to_string(),
            Some(
                "npm error code ETARGET\n\
                 npm error notarget No matching version found for @deepseek-ai/dsh-client-ui-sidebar-documentpreview@^0.1.5-rc.3."
                    .to_string(),
            ),
        );
        assert!(
            offline_failure_justifies_download(&etarget),
            "an ETARGET offline must still get one online attempt"
        );
    }

    #[test]
    fn a_damaged_entry_earns_the_repair_path_not_a_plain_download() {
        // An ENOENT offline is a damaged entry; it also justifies going online,
        // but the repair chain runs first and handles it there.
        let enoent: SpawnFailure = (
            "exited".to_string(),
            "dsh 进程在就绪前退出。".to_string(),
            Some(npm_enoent_detail(&npx_entry("1da1392061ab1944").join("package.json"))),
        );
        assert!(offline_failure_justifies_download(&enoent));
    }

    #[test]
    fn a_failure_a_download_cannot_fix_does_not_earn_a_retry() {
        // npx could not be launched at all: no download changes that.
        let spawn_failed: SpawnFailure = (
            "spawn_failed".to_string(),
            "无法启动 dsh。".to_string(),
            Some("some spawn error".to_string()),
        );
        assert!(!offline_failure_justifies_download(&spawn_failed));

        // A readiness timeout means the cache served it and the process ran,
        // so the problem is not resolution — the timeout self-repair owns it.
        let timeout: SpawnFailure = (
            "ready_timeout".to_string(),
            "超时。".to_string(),
            Some("whatever".to_string()),
        );
        assert!(!offline_failure_justifies_download(&timeout));

        // No npm stderr at all: nothing to conclude from, so no retry.
        let silent: SpawnFailure = ("exited".to_string(), "退出。".to_string(), None);
        assert!(!offline_failure_justifies_download(&silent));
    }

    /// The recorded version ends up inside an npx package spec on a command
    /// line, so anything outside the semver/dist-tag charset is rejected —
    /// a hand-edited state file must fall back to the bare spec, never reach
    /// the shell.
    #[test]
    fn pinned_versions_accept_only_the_npm_spec_charset() {
        for valid in ["0.1.0", "1.2.3-beta.4", "0.0.0-nightly.20240101+build.5"] {
            assert!(is_valid_pinned_version(valid), "{valid} should be pinnable");
        }
        for invalid in [
            "",
            " 0.1.0",
            "0.1.0 ",
            "0.1.0;rm -rf",
            "0.1.0&calc",
            "$(x)",
            "0.1.0/next",
            "0.1.0\n0.2.0",
        ] {
            assert!(!is_valid_pinned_version(invalid), "{invalid:?} must be rejected");
        }
        let too_long = "1".repeat(65);
        assert!(!is_valid_pinned_version(&too_long), "an over-long pin must be rejected");
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
        assert_eq!(parse_authority_port("127.0.0.1:3080"), Some(3080));
        assert_eq!(parse_authority_port("0.0.0.0:3080"), Some(3080));
        assert_eq!(parse_authority_port("[::]:3080"), Some(3080));
        assert_eq!(parse_authority_port("[::1]:3199"), Some(3199));
        assert_eq!(parse_authority_port("127.0.0.1:"), None);
        assert_eq!(parse_authority_port("garbage"), None);
        assert_eq!(parse_authority_port(""), None);
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

    /// A loopback-only listener must not be mistaken for a network service:
    /// the runtime panel compares this scope with the saved 访问范围.
    #[test]
    fn a_loopback_listener_reports_local_scope() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        assert_eq!(occupant_listen_scope(port), DshListenScope::Local);
    }

    /// A wildcard listener answers on the machine's LAN address, not just on
    /// loopback — the client-side probe is what tells the two bindings apart.
    #[test]
    fn a_wildcard_listener_reports_remote_scope() {
        // A fully offline host has no non-loopback address to probe, so the
        // assertion would be meaningless there.
        if !local_ipv4_addresses()
            .iter()
            .any(|entry| !entry.ip.is_loopback())
        {
            eprintln!("[dsh] no non-loopback address on this host; skipping assertion");
            return;
        }
        let listener = TcpListener::bind("0.0.0.0:0").expect("bind wildcard port");
        let port = listener.local_addr().expect("local addr").port();
        assert_eq!(occupant_listen_scope(port), DshListenScope::Remote);
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

    #[test]
    fn a_failed_stop_is_never_reported_as_stopped() {
        let status = stop_outcome_status(
            DshAccess::Remote,
            3080,
            Err("node.exe（PID 21436）仍在监听".to_string()),
        );
        assert_eq!(status.phase, DshPhase::Failed);
        assert_eq!(status.issue.as_deref(), Some("stop_failed"));
        assert!(status.message.contains("端口 3080 尚未释放"));
        assert!(status
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("PID 21436")));

        let stopped = stop_outcome_status(DshAccess::Remote, 3080, Ok(()));
        assert_eq!(stopped.phase, DshPhase::Stopped);
        assert!(stopped.issue.is_none());
    }

    const PROCESS_TREE_ROLE: &str = "AGENTS_LAUNCHER_DSH_TREE_TEST_ROLE";
    const PROCESS_TREE_PORT: &str = "AGENTS_LAUNCHER_DSH_TREE_TEST_PORT";

    /// Subprocess entry point for `a_supervised_process_tree_releases_its_port`.
    /// The first helper waits on a second helper that owns a wildcard listener,
    /// reproducing the important launcher → wrapper → server shape without
    /// starting dsh or depending on npm.
    #[test]
    fn process_tree_test_helper() {
        let Ok(role) = std::env::var(PROCESS_TREE_ROLE) else {
            return;
        };
        let port: u16 = std::env::var(PROCESS_TREE_PORT)
            .expect("tree helper port")
            .parse()
            .expect("numeric tree helper port");

        if role == "listener" {
            let _listener = TcpListener::bind(("0.0.0.0", port)).expect("tree helper listener");
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        }

        assert_eq!(role, "parent");
        let mut child = Command::new(std::env::current_exe().expect("test executable"));
        child
            .args(["--exact", "dsh_runtime::tests::process_tree_test_helper"])
            .env(PROCESS_TREE_ROLE, "listener")
            .env(PROCESS_TREE_PORT, port.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = child.spawn().expect("listener helper");
        let _ = child.wait();
    }

    #[test]
    fn a_supervised_process_tree_releases_its_port() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener);

        let mut command = Command::new(std::env::current_exe().expect("test executable"));
        command
            .args(["--exact", "dsh_runtime::tests::process_tree_test_helper"])
            .env(PROCESS_TREE_ROLE, "parent")
            .env(PROCESS_TREE_PORT, port.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_process_tree(&mut command);
        let child = command.spawn().expect("parent helper");

        let start_deadline = Instant::now() + Duration::from_secs(5);
        while !port_is_occupied(DshAccess::Remote.probe_host(), port)
            && Instant::now() < start_deadline
        {
            std::thread::sleep(Duration::from_millis(50));
        }

        let mut server = DshServer {
            child,
            access: DshAccess::Remote,
            port,
            local_url: format!("http://127.0.0.1:{port}/?token=test"),
            lan_url: None,
            version: None,
            executable: "test-helper".to_string(),
            started_at: Instant::now(),
        };
        assert!(
            port_is_occupied(DshAccess::Remote.probe_host(), port),
            "listener helper did not become ready"
        );
        let outcome = terminate_server(&mut server);
        assert!(outcome.is_ok(), "tree termination failed: {outcome:?}");
        assert!(
            !port_is_occupied(DshAccess::Remote.probe_host(), port),
            "tree listener survived a successful stop"
        );
    }

    // ── first-download progress vs. the readiness deadline ───────────────────

    fn snapshot_at(bytes: u64, last_growth: Instant, grew: bool) -> ProgressSnapshot {
        ProgressSnapshot {
            bytes,
            last_growth,
            grew,
        }
    }

    /// A cached start downloads nothing, so there is no progress to wait for:
    /// it gets the short cache-hit window instead of the download window.
    /// Sliding it here would mean a wedged process could hold the app in
    /// 「正在启动」 forever.
    #[test]
    fn a_download_that_never_grows_keeps_the_absolute_deadline() {
        let started_at = Instant::now();
        let snapshot = snapshot_at(0, started_at, false);
        assert_eq!(
            ready_deadline(Some(snapshot), started_at, false),
            started_at + READY_TIMEOUT_CACHED
        );
    }

    /// The point of the change: growth buys more time, measured from the last
    /// growth rather than from the spawn.
    #[test]
    fn download_growth_slides_the_deadline_forward() {
        let started_at = Instant::now();
        let last_growth = started_at + Duration::from_secs(170);
        let snapshot = snapshot_at(40 * 1024 * 1024, last_growth, true);
        assert_eq!(
            ready_deadline(Some(snapshot), started_at, false),
            last_growth + READY_TIMEOUT
        );
        assert!(
            ready_deadline(Some(snapshot), started_at, false) > started_at + READY_TIMEOUT,
            "growth must extend the wait past the plain 180s window"
        );
    }

    /// The two windows must stay distinct: a cache hit never inherits the
    /// download timeout, and a download never falls back to the short
    /// cache-hit one — a slow network is not a wedged cache.
    #[test]
    fn the_cache_hit_window_stays_shorter_than_the_download_window() {
        assert!(READY_TIMEOUT_CACHED < READY_TIMEOUT);

        let started_at = Instant::now();
        // Grew, but only just: the deadline is the download window from the
        // last growth, not the cache-hit window from the spawn.
        let grew = snapshot_at(
            PROGRESS_ACTIVE_THRESHOLD_BYTES,
            started_at + READY_TIMEOUT_CACHED + Duration::from_secs(5),
            true,
        );
        assert_eq!(
            ready_deadline(Some(grew), started_at, false),
            grew.last_growth + READY_TIMEOUT
        );
        // Same shape without observed growth: the cache-hit window applies,
        // and the longer download window must not leak into it.
        let cached = snapshot_at(0, started_at, false);
        assert_eq!(
            ready_deadline(Some(cached), started_at, false),
            started_at + READY_TIMEOUT_CACHED
        );
    }

    /// The self-repair retry has just emptied the cache, so it downloads from
    /// scratch. Even with no growth observed yet, it must get the download
    /// window — the 30s cache-hit window that fired the timeout being repaired
    /// would cut a healthy reinstall off almost immediately.
    #[test]
    fn a_self_repair_retry_uses_the_download_window_until_growth() {
        let started_at = Instant::now();
        let snapshot = snapshot_at(0, started_at, false);
        assert_eq!(
            ready_deadline(Some(snapshot), started_at, true),
            started_at + READY_TIMEOUT
        );
        // Once growth is observed the flag stops mattering: the sliding window
        // takes over either way.
        let grew = snapshot_at(
            PROGRESS_ACTIVE_THRESHOLD_BYTES,
            started_at + Duration::from_secs(60),
            true,
        );
        assert_eq!(
            ready_deadline(Some(grew), started_at, true),
            grew.last_growth + READY_TIMEOUT
        );
    }

    /// However long the download keeps trickling, one attempt cannot outlive the
    /// ceiling — otherwise the `busy` flag would never clear.
    #[test]
    fn the_deadline_never_passes_the_ceiling() {
        let started_at = Instant::now();
        let snapshot = snapshot_at(1, started_at + Duration::from_secs(10_000), true);
        assert_eq!(
            ready_deadline(Some(snapshot), started_at, false),
            started_at + READY_TIMEOUT_CEILING
        );
    }

    /// No npm cache directory means no growth signal at all. That must degrade
    /// to the old absolute timeout rather than to an unbounded wait.
    #[test]
    fn a_missing_sampler_falls_back_to_the_absolute_timeout() {
        let started_at = Instant::now();
        assert_eq!(
            ready_deadline(None, started_at, false),
            started_at + READY_TIMEOUT
        );
    }

    /// The first reading only establishes the baseline: treating it as growth
    /// would report the entire existing cache as freshly downloaded.
    #[test]
    fn the_first_reading_only_establishes_the_baseline() {
        let mut progress = DownloadProgress::new();
        assert_eq!(progress.observe(500 * 1024 * 1024), (0, 0));
        assert_eq!(progress.bytes, 0);
        assert!(!progress.grew);
        assert_eq!(progress.observe(500 * 1024 * 1024 + 4096), (4096, 4096));
    }

    /// A slow-but-steady download must count as progress. Judging single samples
    /// would call 20 KB per 500ms (≈40 KB/s) stalled, and then kill it.
    #[test]
    fn sub_threshold_samples_accumulate_into_observed_growth() {
        let mut progress = DownloadProgress::new();
        progress.observe(0);
        let before = progress.last_growth;
        let per_sample = PROGRESS_ACTIVE_THRESHOLD_BYTES / 4;
        let mut total = 0;
        for _ in 0..4 {
            total += per_sample;
            progress.observe(total);
        }
        assert!(progress.grew, "four quarter-threshold samples are progress");
        assert!(progress.last_growth >= before);
        assert_eq!(progress.bytes, 4 * per_sample);
    }

    /// Nothing arrives at all: the deadline must not move, so the wait ends on
    /// the original schedule.
    #[test]
    fn a_stalled_cache_leaves_the_deadline_alone() {
        let mut progress = DownloadProgress::new();
        progress.observe(1024);
        let before = progress.last_growth;
        for _ in 0..20 {
            progress.observe(1024);
        }
        assert!(!progress.grew);
        assert_eq!(progress.last_growth, before);
    }

    #[test]
    fn human_bytes_switches_units_at_the_expected_thresholds() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1 KiB");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(human_bytes(14 * 1024 * 1024 + 512 * 1024), "14.5 MiB");
    }
}
