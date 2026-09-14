import test from 'node:test'
import assert from 'node:assert/strict'
import {
  DSH_DEFAULT_PORT,
  DSH_PORT_MAX,
  DSH_PORT_MIN,
  describeCopyTarget,
  describeInstallProgress,
  dshAccessLabel,
  dshAddressLabel,
  dshAddressRows,
  dshBindHost,
  formatBytes,
  formatDuration,
  formatUptimeZh,
  hasAccessToken,
  isUsableEmbedBounds,
  isValidDshPort,
  normalizeDshAccess,
  pickCopyUrl,
  pickShareUrl,
  urlHostPort,
  type DshRuntimeUrls,
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

const LOCAL = 'http://127.0.0.1:3080/?token=local-token'
const LAN = 'http://192.168.1.5:3080/?token=local-token'
const TAILSCALE = 'http://100.101.102.103:3080/?token=local-token'

/** A running service with both a Wi-Fi address and a Tailscale address. */
const dualStack: DshRuntimeUrls = {
  localUrl: LOCAL,
  remoteUrl: TAILSCALE,
  addresses: [
    { kind: 'loopback', address: '127.0.0.1', interface: null, url: LOCAL },
    { kind: 'lan', address: '192.168.1.5', interface: 'WLAN', url: LAN },
    { kind: 'tailscale', address: '100.101.102.103', interface: 'Tailscale', url: TAILSCALE },
  ],
}

test('复制链接 prefers the LAN address over the Tailscale one', () => {
  // The reported bug: with Tailscale installed, dsh's own `(LAN: …)` segment was
  // the 100.x address, so 复制链接 handed out a link no phone on the same Wi-Fi
  // could open. The address list decides instead.
  const picked = pickCopyUrl(dualStack)
  assert.equal(picked, LAN)
  assert.equal(pickShareUrl(dualStack), LAN)

  // The whole point of the button: whatever is copied must carry the token,
  // otherwise the recipient only sees dsh's 401 page.
  assert.ok(hasAccessToken(picked))
  assert.match(picked!, /token=local-token/)
})

test('复制链接 falls back to the tailnet and then to loopback', () => {
  // No LAN interface in use: the tailnet address is the only shareable one.
  const tailnetOnly: DshRuntimeUrls = {
    localUrl: LOCAL,
    remoteUrl: TAILSCALE,
    addresses: [
      { kind: 'loopback', address: '127.0.0.1', interface: null, url: LOCAL },
      { kind: 'tailscale', address: '100.101.102.103', interface: 'Tailscale', url: TAILSCALE },
    ],
  }
  assert.equal(pickCopyUrl(tailnetOnly), TAILSCALE)

  // 本地 mode binds 127.0.0.1, so the backend lists nothing else and the
  // loopback URL is the only reachable one.
  const localOnly: DshRuntimeUrls = {
    localUrl: LOCAL,
    remoteUrl: null,
    addresses: [{ kind: 'loopback', address: '127.0.0.1', interface: null, url: LOCAL }],
  }
  assert.equal(pickCopyUrl(localOnly), LOCAL)
  assert.equal(pickShareUrl(localOnly), null, 'loopback is never offered for sharing')
})

test('复制链接 refuses a URL that lost its token', () => {
  // A token-less URL would land on the 401 page; better to disable the button.
  assert.equal(pickCopyUrl({ localUrl: 'http://127.0.0.1:3080/', remoteUrl: null, addresses: [] }), null)
  assert.equal(pickCopyUrl(null), null)
  assert.equal(pickCopyUrl(undefined), null)
  // A backend that reports no address list at all still answers through the
  // loopback URL, and through dsh's LAN URL when it has one.
  assert.equal(pickCopyUrl({ localUrl: LOCAL, remoteUrl: null, addresses: [] }), LOCAL)
  assert.equal(pickCopyUrl({ localUrl: LOCAL, remoteUrl: LAN, addresses: [] }), LAN)
  assert.equal(
    pickCopyUrl({
      localUrl: LOCAL,
      remoteUrl: 'http://10.0.0.2:3080/',
      addresses: [],
    }),
    LOCAL,
    'a shareable URL without a token is not shareable at all',
  )
})

test('the access list labels every group and marks the default row', () => {
  const rows = dshAddressRows(dualStack)
  assert.deepEqual(
    rows.map((row) => [row.label, row.display]),
    [
      ['本机', '127.0.0.1:3080'],
      ['局域网', '192.168.1.5:3080'],
      ['Tailscale', '100.101.102.103:3080'],
    ],
  )
  // Exactly one row is the one 「复制链接」 copies, and it is the LAN one.
  assert.deepEqual(rows.map((row) => row.preferred), [false, true, false])
  assert.equal(rows[1].interface, 'WLAN')
  assert.match(rows[1].url, /^http:\/\/192\.168\.1\.5:3080\/\?token=local-token$/)

  assert.equal(dshAddressLabel('loopback'), '本机')
  assert.equal(dshAddressLabel('lan'), '局域网')
  assert.equal(dshAddressLabel('tailscale'), 'Tailscale')
  assert.equal(dshAddressLabel('other'), '其它网络')
})

test('the access list drops entries that would only reach the 401 page', () => {
  const rows = dshAddressRows({
    localUrl: LOCAL,
    remoteUrl: null,
    addresses: [
      { kind: 'loopback', address: '127.0.0.1', interface: null, url: 'http://127.0.0.1:3080/' },
      { kind: 'lan', address: '192.168.1.5', interface: '', url: LAN },
    ],
  })
  assert.deepEqual(rows.map((row) => row.kind), ['lan'])
  assert.equal(rows[0].interface, null, 'a blank interface name is not shown')
  assert.equal(rows[0].preferred, true)
  assert.deepEqual(dshAddressRows(null), [])
  assert.deepEqual(dshAddressRows(undefined), [])
})

test('the panel shows an address without the token it copies', () => {
  assert.equal(urlHostPort(LAN), '192.168.1.5:3080')
  assert.equal(urlHostPort('https://100.64.0.1:3199/?token=t'), '100.64.0.1:3199')
  assert.equal(urlHostPort('http://[::1]:3080/?token=t'), '[::1]:3080')
  assert.equal(urlHostPort(''), '')
  assert.equal(urlHostPort(null), '')
})

test('复制链接 hint names the address it will hand out', () => {
  const rows = dshAddressRows(dualStack)
  assert.match(describeCopyTarget(rows[1]), /局域网.*192\.168\.1\.5:3080/)
  assert.match(describeCopyTarget(rows[2]), /Tailscale.*100\.101\.102\.103:3080/)
  // The hint is also where the token warning lives: the copied text is a
  // full-access credential even though the panel never shows it.
  assert.match(describeCopyTarget(rows[0]), /令牌/)
  assert.equal(describeCopyTarget(null), '服务运行后才能复制链接')
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

test('uptime starts with bare seconds and stacks units as it grows', () => {
  // 初始只有秒：没有「0分」之类的前导零单位。
  assert.equal(formatUptimeZh(0), '0秒')
  assert.equal(formatUptimeZh(5), '5秒')
  assert.equal(formatUptimeZh(59), '59秒')

  // 满足分钟：mm分ss秒；秒补零，让每秒刷新的列宽稳定。
  assert.equal(formatUptimeZh(60), '1分00秒')
  assert.equal(formatUptimeZh(65), '1分05秒')
  assert.equal(formatUptimeZh(3599), '59分59秒')

  // 满足小时：hh小时mm分ss秒。
  assert.equal(formatUptimeZh(3600), '1小时00分00秒')
  assert.equal(formatUptimeZh(3723), '1小时02分03秒')
  assert.equal(formatUptimeZh(86_399), '23小时59分59秒')

  // 满足一天：d天hh小时mm分ss秒。
  assert.equal(formatUptimeZh(86_400), '1天00小时00分00秒')
  assert.equal(formatUptimeZh(90_061), '1天01小时01分01秒')
  assert.equal(formatUptimeZh(2 * 86_400 + 3599), '2天00小时59分59秒')

  // 负值与非整数防御性归零/取整。
  assert.equal(formatUptimeZh(-3), '0秒')
  assert.equal(formatUptimeZh(5.9), '5秒')
})
