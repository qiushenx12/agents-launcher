/**
 * Pure helpers shared by the dsh configuration panel, the runtime panel and the
 * frontend tests. Keeping them free of Tauri imports is what makes them
 * testable with `node --test`.
 *
 * The authoritative implementations of the overlay template, the ready-line
 * parser and the port probe live in `src-tauri/src/dsh_runtime.rs`; the values
 * here must stay in sync with it.
 */

export type DshAccess = 'local' | 'remote'

export const DSH_DEFAULT_PORT = 3080
export const DSH_PORT_MIN = 1
export const DSH_PORT_MAX = 65535

/**
 * Ports are user-supplied and must be a fixed choice: dsh accepts `--port 0`
 * (OS-assigned), but a port that changes on every start makes the URL
 * impossible to bookmark or share.
 */
export function isValidDshPort(value: unknown): value is number {
  return typeof value === 'number'
    && Number.isInteger(value)
    && value >= DSH_PORT_MIN
    && value <= DSH_PORT_MAX
}

/** Anything other than the literal `"remote"` is the safe, loopback-only mode. */
export function normalizeDshAccess(value: unknown): DshAccess {
  return value === 'remote' ? 'remote' : 'local'
}

/**
 * Address the managed server binds to. `0.0.0.0` is unreachable through the dsh
 * CLI (it rejects `--host 0.0.0.0`), which is exactly why this switch exists in
 * the launcher and not upstream.
 */
export function dshBindHost(access: DshAccess): string {
  return access === 'remote' ? '0.0.0.0' : '127.0.0.1'
}

/** Human-readable description of the access switch, used in status copy. */
export function dshAccessLabel(access: DshAccess): string {
  return access === 'remote' ? '远程' : '本地'
}

/**
 * The one phrasing every surface uses for a taken port: «当前占用进程为 …».
 *
 * It deliberately says nothing about where the occupant came from. The port can
 * perfectly well be held by the very dsh this launcher started — an application
 * restart loses the child handle, so the runtime status says "not running" while
 * our own process still listens — and calling that "另一个进程" is wrong and
 * alarming. Naming the process is accurate in every case.
 */
export function describePortOccupant(occupant: string | null | undefined, port: number): string {
  const label = occupant?.trim()
  return label ? `当前占用进程为 ${label}。` : `端口 ${port} 的当前占用进程未知。`
}

/**
 * How a port occupant relates to the saved dsh configuration, for the runtime
 * panel's «清理并启动» card.
 *
 * Only the listen scope can be verified against the saved config: the port
 * matches by construction (it is the port that was probed), and the access
 * token cannot be checked without knowing it — a dsh index page answers 401 to
 * exactly the request that would identify it.
 */
export type PortConflictKind =
  /** The launcher-supervised service holds it: 「关闭」 is the right action. */
  | 'supervised'
  /** A non-dsh program holds it. */
  | 'other-program'
  /** A dsh service holds it and its listen scope matches the saved 访问范围. */
  | 'dsh-match'
  /** A dsh service holds it, but listens on a different scope than saved. */
  | 'dsh-mismatch'
  /** A dsh service holds it, but its scope could not be determined. */
  | 'dsh-unknown'

export function classifyPortConflict(
  occupancy: {
    occupantIsDsh: boolean
    occupantIsSupervised: boolean
    occupantListenScope?: DshAccess | null
  },
  savedAccess: DshAccess,
): PortConflictKind {
  if (occupancy.occupantIsSupervised) return 'supervised'
  if (!occupancy.occupantIsDsh) return 'other-program'
  if (occupancy.occupantListenScope == null) return 'dsh-unknown'
  return occupancy.occupantListenScope === savedAccess ? 'dsh-match' : 'dsh-mismatch'
}

/**
 * The explanatory paragraph of the runtime panel's port-conflict card. The
 * button row is separate; this text says *who* holds the port (the same
 * «当前占用进程为 …» sentence every surface uses), *why* the resident instance
 * cannot simply be reused, and what happens after the cleanup.
 *
 * A resident dsh can never be adopted — even a config-consistent one — because
 * the launcher does not hold its access token, and without the token every
 * page load lands on dsh's 401. Cleanup + a fresh supervised start is the only
 * path into the embedded UI.
 */
export function describeRuntimePortConflict(conflict: {
  port: number
  occupant: string | null
  kind: PortConflictKind
  occupantListenScope?: DshAccess | null
  savedAccess: DshAccess
  savedPort: number
}): string {
  const who = describePortOccupant(conflict.occupant, conflict.port)
  const saved = `保存的配置（${dshAccessLabel(conflict.savedAccess)} · 端口 ${conflict.savedPort}）`
  switch (conflict.kind) {
    case 'supervised':
      return `端口 ${conflict.port} 已被占用。${who}`
        + '它由启动器启动，当前状态只是没跟踪到它。请到配置页点击「关闭」停止它，或改用其它端口。'
    case 'dsh-match':
      return `端口 ${conflict.port} 已被占用。${who}`
        + '它是一个已在运行的 dsh 服务，配置与保存的一致，但启动器未持有它的访问凭据，无法直接内嵌。'
        + `清理后将按${saved}重新启动。`
    case 'dsh-mismatch':
      return `端口 ${conflict.port} 已被占用。${who}`
        + `它是一个已在运行的 dsh 服务，但运行配置（${dshAccessLabel(conflict.occupantListenScope ?? 'local')} · 端口 ${conflict.port}）与${saved}不一致。`
        + `清理后将按${saved}重新启动。`
    case 'dsh-unknown':
      return `端口 ${conflict.port} 已被占用。${who}`
        + '它是一个已在运行的 dsh 服务，但无法确认其运行配置是否与保存的一致。'
        + `清理后将按${saved}重新启动。`
    default:
      return `端口 ${conflict.port} 已被占用。${who}`
        + `清理后将按${saved}启动 dsh；也可以在配置页改用其它端口。`
  }
}

export interface DshEmbedBounds {
  x: number
  y: number
  width: number
  height: number
}

/**
 * A placeholder of zero or fractional size cannot host a native control, and a
 * non-finite rectangle would be rejected by the platform layer.
 */
export function isUsableEmbedBounds(bounds: DshEmbedBounds): boolean {
  return Number.isFinite(bounds.x)
    && Number.isFinite(bounds.y)
    && bounds.width >= 2
    && bounds.height >= 2
}

/**
 * Token-bearing URL check used before showing the copy / QR affordances. A URL
 * without a token would land on the 401 page, so it must never be presented as
 * the way in.
 */
export function hasAccessToken(url: string | null | undefined): boolean {
  if (!url) return false
  const query = url.split('?')[1]
  if (!query) return false
  return query.split('&').some((pair) => {
    const [key, value] = pair.split('=')
    return key === 'token' && !!value
  })
}

/**
 * Token-bearing URLs, fetched only when the UI needs them. They are never
 * persisted and never included in diagnostics.
 */
export interface DshRuntimeUrls {
  localUrl: string
  remoteUrl: string | null
  /** Every interface address the running service answers on. */
  addresses: DshAccessAddress[]
}

/**
 * Which network an entry point belongs to. Mirrors `DshAddressKind` in
 * `src-tauri/src/dsh_runtime.rs`.
 */
export type DshAddressKind = 'loopback' | 'lan' | 'tailscale' | 'other'

/** One address the running service answers on, with its token-bearing URL. */
export interface DshAccessAddress {
  kind: DshAddressKind
  /** Bare address, e.g. `192.168.1.5`. */
  address: string
  /** Interface name when the OS reports one (`WLAN`, `Tailscale`, `en0`). */
  interface: string | null
  /** Token-bearing URL for this address. */
  url: string
}

/** Label per kind. `Tailscale` stays untranslated: it is a product name. */
const ADDRESS_LABELS: Record<DshAddressKind, string> = {
  loopback: '本机',
  lan: '局域网',
  tailscale: 'Tailscale',
  other: '其它网络',
}

export function dshAddressLabel(kind: DshAddressKind): string {
  return ADDRESS_LABELS[kind] ?? ADDRESS_LABELS.other
}

/**
 * `http://192.168.1.5:3080/?token=abc` → `192.168.1.5:3080`.
 *
 * The panel shows this instead of the whole URL: the token is a full-access
 * credential and does not belong on screen when the copy button already hands
 * it out.
 */
export function urlHostPort(url: string | null | undefined): string {
  if (!url) return ''
  const match = /^[a-z][a-z0-9+.-]*:\/\/([^/?#]+)/i.exec(url)
  return match ? match[1] : url
}

/** One row of the panel's access list. */
export interface DshAddressRow {
  kind: DshAddressKind
  label: string
  /** Address as shown, e.g. `192.168.1.5:3080`. */
  display: string
  interface: string | null
  /** Token-bearing URL this row's 复制 button hands out. */
  url: string
  /** True for the row the plain 「复制链接」 button copies. */
  preferred: boolean
}

/**
 * URLs another device can open, best first: 局域网 → Tailscale → 其它.
 *
 * Loopback is excluded on purpose — it is the one address a phone scanning the
 * QR code can never reach.
 */
export function pickShareUrl(urls: DshRuntimeUrls | null | undefined): string | null {
  const shareable = (urls?.addresses ?? []).filter(
    (entry) => entry.kind !== 'loopback' && hasAccessToken(entry.url),
  )
  const ordered = [
    ...shareable.filter((entry) => entry.kind === 'lan'),
    ...shareable.filter((entry) => entry.kind === 'tailscale'),
    ...shareable.filter((entry) => entry.kind === 'other'),
  ]
  if (ordered.length > 0) return ordered[0].url
  // A run whose enumeration found nothing still reports the LAN URL dsh printed
  // at startup, which is better than offering nothing to share.
  return hasAccessToken(urls?.remoteUrl) ? urls!.remoteUrl : null
}

/**
 * What 「复制链接」 hands out.
 *
 * The choice follows the *running* service rather than the draft access
 * setting: the backend lists exactly the addresses the service answers on, so
 * the first shareable one is right in both modes — 局域网 when there is one (a
 * phone on the same Wi-Fi), otherwise the tailnet, otherwise loopback.
 */
export function pickCopyUrl(urls: DshRuntimeUrls | null | undefined): string | null {
  if (!urls) return null
  const shared = pickShareUrl(urls)
  if (shared) return shared
  if (hasAccessToken(urls.localUrl)) return urls.localUrl
  const loopback = (urls.addresses ?? []).find(
    (entry) => entry.kind === 'loopback' && hasAccessToken(entry.url),
  )
  return loopback?.url ?? null
}

/**
 * Access-list rows for the panel, in the backend's order (本机 → 局域网 →
 * Tailscale → 其它). Entries without a token are dropped: they would only open
 * dsh's 401 page.
 */
export function dshAddressRows(urls: DshRuntimeUrls | null | undefined): DshAddressRow[] {
  const entries = (urls?.addresses ?? []).filter((entry) => hasAccessToken(entry.url))
  const preferred = pickCopyUrl(urls)
  return entries.map((entry) => ({
    kind: entry.kind,
    label: dshAddressLabel(entry.kind),
    display: urlHostPort(entry.url),
    interface: entry.interface?.trim() || null,
    url: entry.url,
    preferred: entry.url === preferred,
  }))
}

/** Hint text for a copy button, naming the address it will hand out. */
export function describeCopyTarget(row: DshAddressRow | null | undefined): string {
  if (!row) return '服务运行后才能复制链接'
  return `将复制「${row.label}」链接：${row.display}（含访问令牌）`
}

/**
 * Download progress text for the "preparing" phase. A cache hit must not read
 * as `0 MB/s`; the sampled byte count is the tarball write volume, so the
 * wording avoids promising an exact network speed.
 */
export function describeInstallProgress(progress: {
  bytes: number
  bytesPerSecond: number
  elapsedMs: number
  cached: boolean
} | null): string {
  if (!progress) return '正在准备 dsh…'
  const elapsed = formatDuration(progress.elapsedMs)
  if (progress.cached) return `使用本地缓存，正在启动 · 已用 ${elapsed}`
  return `正在准备 dsh（首次运行需要下载）· 已下载 ${formatBytes(progress.bytes)}`
    + ` · ${formatBytes(progress.bytesPerSecond)}/s · ${elapsed}`
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B'
  if (bytes < 1024) return `${Math.round(bytes)} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}

export function formatDuration(elapsedMs: number): string {
  const total = Math.max(0, Math.floor(elapsedMs / 1000))
  const minutes = String(Math.floor(total / 60)).padStart(2, '0')
  const seconds = String(total % 60).padStart(2, '0')
  return `${minutes}:${seconds}`
}
