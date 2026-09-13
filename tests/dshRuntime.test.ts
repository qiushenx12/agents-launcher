import test from 'node:test'
import assert from 'node:assert/strict'
import {
  DSH_DEFAULT_PORT,
  DSH_PORT_MAX,
  DSH_PORT_MIN,
  describeCopyTarget,
  describeInstallProgress,
  dshAccessLabel,
  dshBindHost,
  formatBytes,
  formatDuration,
  hasAccessToken,
  isUsableEmbedBounds,
  isValidDshPort,
  normalizeDshAccess,
  pickCopyUrl,
} from '../src/utils/dshRuntime.ts'

test('dsh port validation accepts only the documented integer range', () => {
  assert.equal(isValidDshPort(DSH_DEFAULT_PORT), true)
  assert.equal(isValidDshPort(DSH_PORT_MIN), true)
  assert.equal(isValidDshPort(DSH_PORT_MAX), true)

  // dsh itself accepts `--port 0` (OS-assigned), but the launcher must not:
  // a port that changes every start cannot be bookmarked or shared.
  assert.equal(isValidDshPort(0), false)
  assert.equal(isValidDshPort(DSH_PORT_MAX + 1), false)
  assert.equal(isValidDshPort(-1), false)
  assert.equal(isValidDshPort(3080.5), false)
  assert.equal(isValidDshPort('3080'), false)
  assert.equal(isValidDshPort(null), false)
  assert.equal(isValidDshPort(Number.NaN), false)
})

test('dsh access normalizes unknown values to the loopback-only mode', () => {
  assert.equal(normalizeDshAccess('remote'), 'remote')
  assert.equal(normalizeDshAccess('local'), 'local')
  assert.equal(normalizeDshAccess('REMOTE'), 'local')
  assert.equal(normalizeDshAccess(undefined), 'local')
  assert.equal(normalizeDshAccess('0.0.0.0'), 'local')
})

test('dsh access maps to the host the overlay and the probe must use', () => {
  // 本地 listens on loopback only; 远程 must bind 0.0.0.0 so LAN peers can
  // connect, which is unreachable through the dsh CLI's own --host flag.
  assert.equal(dshBindHost('local'), '127.0.0.1')
  assert.equal(dshBindHost('remote'), '0.0.0.0')
  assert.equal(dshAccessLabel('local'), '本地')
  assert.equal(dshAccessLabel('remote'), '远程')
})

test('dsh embed bounds reject zero, fractional and non-finite rectangles', () => {
  assert.equal(isUsableEmbedBounds({ x: 12, y: 44, width: 800, height: 600 }), true)
  assert.equal(isUsableEmbedBounds({ x: 12, y: 44, width: 1, height: 600 }), false)
  assert.equal(isUsableEmbedBounds({ x: 12, y: 44, width: 800, height: 0 }), false)
  assert.equal(isUsableEmbedBounds({ x: Number.NaN, y: 44, width: 800, height: 600 }), false)
  assert.equal(
    isUsableEmbedBounds({ x: 12, y: Number.POSITIVE_INFINITY, width: 800, height: 600 }),
    false,
  )
})

test('dsh urls without a token are never offered as the way in', () => {
  assert.equal(hasAccessToken('http://127.0.0.1:3080/?token=abc.def'), true)
  assert.equal(hasAccessToken('http://192.168.1.5:3080/?token=abc&x=1'), true)
  assert.equal(hasAccessToken('http://127.0.0.1:3080/'), false)
  assert.equal(hasAccessToken('http://127.0.0.1:3080/?token='), false)
  assert.equal(hasAccessToken('http://127.0.0.1:3080/?other=abc'), false)
  assert.equal(hasAccessToken(null), false)
  assert.equal(hasAccessToken(''), false)
})

test('复制链接 follows the access scope and always keeps the token', () => {
  const local = 'http://127.0.0.1:3080/?token=local-token'
  const lan = 'http://192.168.1.5:3080/?token=local-token'

  // 本地 mode: the loopback URL, which is the only reachable one.
  const localPick = pickCopyUrl({ localUrl: local, remoteUrl: lan }, 'local')
  assert.equal(localPick, local)

  // 远程 mode: the LAN URL, so a phone or another machine gets something usable.
  const remotePick = pickCopyUrl({ localUrl: local, remoteUrl: lan }, 'remote')
  assert.equal(remotePick, lan)

  // The whole point of the button: whatever is copied must carry the token,
  // otherwise the recipient only sees dsh's 401 page.
  assert.ok(hasAccessToken(localPick))
  assert.ok(hasAccessToken(remotePick))
  assert.match(remotePick!, /token=local-token/)
})

test('复制链接 in 远程 mode falls back to loopback instead of failing', () => {
  const local = 'http://127.0.0.1:3199/?token=t'
  // dsh only prints a LAN URL while it listens on 0.0.0.0, so a stopped or
  // loopback-only service has none. That is not an error worth blocking on.
  assert.equal(pickCopyUrl({ localUrl: local, remoteUrl: null }, 'remote'), local)
})

test('复制链接 refuses a URL that lost its token', () => {
  // A token-less URL would land on the 401 page; better to disable the button.
  assert.equal(pickCopyUrl({ localUrl: 'http://127.0.0.1:3080/', remoteUrl: null }, 'local'), null)
  assert.equal(
    pickCopyUrl({ localUrl: 'http://127.0.0.1:3080/?token=x', remoteUrl: 'http://10.0.0.2:3080/' }, 'remote'),
    'http://127.0.0.1:3080/?token=x',
  )
  assert.equal(pickCopyUrl({ localUrl: 'http://x/?token=', remoteUrl: null }, 'local'), null)
  assert.equal(pickCopyUrl(null, 'local'), null)
  assert.equal(pickCopyUrl(undefined, 'remote'), null)
})

test('复制链接 hint names the URL it will hand out', () => {
  assert.match(describeCopyTarget('local', 'http://127.0.0.1:3080/?token=t'), /本地链接/)
  assert.match(describeCopyTarget('remote', 'http://10.0.0.2:3080/?token=t'), /局域网链接/)
  assert.match(describeCopyTarget('local', 'http://127.0.0.1:3080/?token=t'), /令牌/)
  assert.equal(describeCopyTarget('remote', null), '服务运行后才能复制链接')
})

test('first-download progress never reports a cache hit as 0 MB/s', () => {
  const cached = describeInstallProgress({
    bytes: 0,
    bytesPerSecond: 0,
    elapsedMs: 4200,
    cached: true,
  })
  assert.match(cached, /本地缓存/)
  assert.doesNotMatch(cached, /0 B\/s/)
  assert.match(cached, /00:04/)

  const downloading = describeInstallProgress({
    bytes: 12 * 1024 * 1024,
    bytesPerSecond: 2 * 1024 * 1024,
    elapsedMs: 7000,
    cached: false,
  })
  assert.match(downloading, /首次运行需要下载/)
  assert.match(downloading, /12\.0 MB/)
  assert.match(downloading, /2\.0 MB\/s/)
  assert.match(downloading, /00:07/)

  assert.equal(describeInstallProgress(null), '正在准备 dsh…')
})

test('byte and duration formatting stays readable at the boundaries', () => {
  assert.equal(formatBytes(0), '0 B')
  assert.equal(formatBytes(512), '512 B')
  assert.equal(formatBytes(2048), '2.0 KB')
  assert.equal(formatBytes(3 * 1024 * 1024), '3.0 MB')
  assert.equal(formatBytes(Number.NaN), '0 B')
  assert.equal(formatDuration(0), '00:00')
  assert.equal(formatDuration(61_000), '01:01')
  assert.equal(formatDuration(-500), '00:00')
})
