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
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
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
///
/// This bounds *staleness*, not the whole attempt: as long as the download is
/// demonstrably still growing the wait goes on (see [`ready_deadline`]).
/// Interrupting a healthy download at 180s throws away everything already
/// transferred and leaves the user to start over, which is worse than waiting.
const READY_TIMEOUT: Duration = Duration::from_secs(180);
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

/// `npx` is always used: dsh's bin name is not on `PATH` unless the user
/// installed the package globally, and this feature must work without that.
const DSH_PACKAGE: &str = "@deepseek-ai/dsh";

/// Package spec handed to npx. A recorded version pins the start to exactly
/// that release (`@deepseek-ai/dsh@0.1.0`), which npx serves from its cache
/// without re-resolving `latest`; without a record the bare spec keeps the
/// previous "resolve latest" behaviour for the first-ever start.
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
    /// `exited` / `stop_failed`.
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
fn dsh_arguments(overlay: &Path, port: u16, version: Option<&str>) -> Vec<String> {
    vec![
        "--yes".to_string(),
        dsh_package_spec(version),
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
    /// nothing keeps the plain 180s deadline instead of waiting indefinitely.
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
/// process wedged before it printed anything, keeps the original 180s), and it
/// can never pass `started_at + READY_TIMEOUT_CEILING`.
fn ready_deadline(snapshot: Option<ProgressSnapshot>, started_at: Instant) -> Instant {
    match snapshot {
        Some(snapshot) => {
            (snapshot.last_growth + READY_TIMEOUT).min(started_at + READY_TIMEOUT_CEILING)
        }
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

type SpawnFailure = (String, String, Option<String>);

fn spawn_and_wait(
    app: &AppHandle,
    npx: &Path,
    overlay: &Path,
    port: u16,
    version: Option<&str>,
) -> Result<SpawnOutcome, SpawnFailure> {
    let mut command = hidden_command(npx);
    command
        .args(dsh_arguments(overlay, port, version))
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
        // ever reaches the deadline.
        let snapshot = progress_snapshot(&progress);
        let deadline = ready_deadline(snapshot, started_at);
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            progress_active.store(false, Ordering::Relaxed);
            let detail = stderr_summary(&stderr_tail);
            let detail = with_shutdown_error(
                detail,
                terminate_managed_process(&mut child, DshAccess::Local, port),
            );
            // Two distinct causes, two distinct messages: a download that was
            // still moving when the ceiling arrived is a very different problem
            // from one that never got going.
            let message = match snapshot {
                Some(snapshot) if snapshot.grew => format!(
                    "启动 dsh 超过 {} 分钟上限仍未就绪（已下载 {}）。下载一直在进行，但耗时过长，请检查网络与代理后重试。",
                    READY_TIMEOUT_CEILING.as_secs() / 60,
                    human_bytes(snapshot.bytes)
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

/// One spawn attempt plus a single self-heal retry for the corrupt-npx-cache
/// failure. Clearing the broken entry and starting over is exactly what a user
/// would do by hand; the retry gets exactly one chance so a persistent failure
/// still surfaces instead of looping.
fn spawn_with_cache_repair(
    app: &AppHandle,
    npx: &Path,
    overlay: &Path,
    port: u16,
    version: Option<&str>,
) -> Result<SpawnOutcome, SpawnFailure> {
    let first = match spawn_and_wait(app, npx, overlay, port, version) {
        Ok(outcome) => return Ok(outcome),
        Err(failure) => failure,
    };
    let Some(cache_dir) = first.2.as_deref().and_then(corrupt_npx_cache_dir) else {
        return Err(first);
    };
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
    spawn_and_wait(app, npx, overlay, port, version).map_err(|(issue, message, detail)| {
        (
            issue,
            format!("{message}（已自动清理损坏的 npx 缓存并重试一次。）"),
            detail,
        )
    })
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

/// The version of the dsh package behind one spec. A pinned spec is served
/// from the npx cache without a registry round-trip; the bare spec resolves
/// `latest`, which is why it is only used before the first version record
/// exists.
fn read_version(npx: &Path, spec: &str) -> Option<String> {
    let output = hidden_command(npx)
        .args(["--yes", spec, "-V"])
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

/// The `latest` dist-tag of the dsh package, from npm registry metadata.
///
/// `npm view` answers without downloading the package, keeping 「更新版本」 a
/// quick check; the download happens on the next start, under the same
/// progress reporting as any other start.
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
    let version = read_version(&npx, &dsh_package_spec(pinned.as_deref()));
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
        let args = dsh_arguments(Path::new("/tmp/overlay.yml"), 3199, None);
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
        let args = dsh_arguments(Path::new("/tmp/overlay.yml"), 3199, Some("0.1.0"));
        assert_eq!(args[1], "@deepseek-ai/dsh@0.1.0");
        assert_eq!(args[0], "--yes");
        assert_eq!(args[2], "web");

        assert_eq!(dsh_package_spec(None), "@deepseek-ai/dsh");
        assert_eq!(dsh_package_spec(Some("0.2.0-beta.1")), "@deepseek-ai/dsh@0.2.0-beta.1");
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

    /// A cached start downloads nothing, so the deadline must stay exactly where
    /// it always was: 180s from the spawn. Sliding it here would mean a wedged
    /// process could hold the app in 「正在启动」 forever.
    #[test]
    fn a_download_that_never_grows_keeps_the_absolute_deadline() {
        let started_at = Instant::now();
        let snapshot = snapshot_at(0, started_at, false);
        assert_eq!(
            ready_deadline(Some(snapshot), started_at),
            started_at + READY_TIMEOUT
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
            ready_deadline(Some(snapshot), started_at),
            last_growth + READY_TIMEOUT
        );
        assert!(
            ready_deadline(Some(snapshot), started_at) > started_at + READY_TIMEOUT,
            "growth must extend the wait past the plain 180s window"
        );
    }

    /// However long the download keeps trickling, one attempt cannot outlive the
    /// ceiling — otherwise the `busy` flag would never clear.
    #[test]
    fn the_deadline_never_passes_the_ceiling() {
        let started_at = Instant::now();
        let snapshot = snapshot_at(1, started_at + Duration::from_secs(10_000), true);
        assert_eq!(
            ready_deadline(Some(snapshot), started_at),
            started_at + READY_TIMEOUT_CEILING
        );
    }

    /// No npm cache directory means no growth signal at all. That must degrade
    /// to the old absolute timeout rather than to an unbounded wait.
    #[test]
    fn a_missing_sampler_falls_back_to_the_absolute_timeout() {
        let started_at = Instant::now();
        assert_eq!(
            ready_deadline(None, started_at),
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
