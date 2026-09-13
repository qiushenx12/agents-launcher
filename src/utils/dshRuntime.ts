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
}

/**
 * Which URL "复制链接" should hand out.
 *
 * The choice follows the configured access scope, because that is the URL the
 * user intends to share:
 *
 * * 本地 → the loopback URL (only this machine can open it),
 * * 远程 → the LAN URL, so a phone or another machine receives something usable.
 *
 * In 远程 mode the LAN URL only exists while the service runs and only when dsh
 * reported one, so the loopback URL is a fallback rather than an error. Every
 * returned URL carries its `token` query parameter — without it the recipient
 * lands on dsh's 401 page, which is exactly the failure this button prevents.
 */
export function pickCopyUrl(
  urls: DshRuntimeUrls | null | undefined,
  access: DshAccess,
): string | null {
  if (!urls) return null
  if (access === 'remote' && hasAccessToken(urls.remoteUrl)) return urls.remoteUrl
  if (hasAccessToken(urls.localUrl)) return urls.localUrl
  return null
}

/** Hint text for the copy button, describing which URL it will hand out. */
export function describeCopyTarget(access: DshAccess, url: string | null): string {
  if (!url) return '服务运行后才能复制链接'
  return access === 'remote' ? '将复制局域网链接（含访问令牌）' : '将复制本地链接（含访问令牌）'
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
