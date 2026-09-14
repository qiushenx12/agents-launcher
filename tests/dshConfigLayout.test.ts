import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')

function scopedStyle(relativePath: string): string {
  const source = readFileSync(resolve(repoRoot, relativePath), 'utf8')
  const styleBlock = /<style\s+scoped>([\s\S]*?)<\/style>/.exec(source)
  assert.ok(styleBlock, `${relativePath} has no scoped style block`)
  return styleBlock[1].replace(/\/\*[\s\S]*?\*\//g, '')
}

function declarations(css: string, selector: string): Map<string, string> {
  const escapedSelector = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const rule = new RegExp(`${escapedSelector}\\s*\\{([^}]*)\\}`).exec(css)
  assert.ok(rule, `missing rule for ${selector}`)

  const result = new Map<string, string>()
  for (const declaration of rule[1].split(';')) {
    const separator = declaration.indexOf(':')
    if (separator === -1) continue
    result.set(declaration.slice(0, separator).trim(), declaration.slice(separator + 1).trim())
  }
  return result
}

test('dsh access and port labels align to the left edge of their shared column', () => {
  const css = scopedStyle('src/components/dsh/DshConfigPanel.vue')
  const label = declarations(css, '.field-label')

  assert.equal(label.get('width'), '110px')
  assert.equal(label.get('text-align'), 'left')
})

/**
 * The port row used to be a full-width input with a hint line underneath and
 * 「重新检测端口」 at the hint's far end. The row is now compact: a shortened
 * input, the recheck button pinned to the row's right edge, and no hint text —
 * an occupied or invalid port still surfaces through the portError banner and
 * the 一键清理占用 block.
 */
test('the port row pairs a shortened input with the recheck button and drops the hint line', () => {
  const markup = readFileSync(resolve(repoRoot, 'src/components/dsh/DshConfigPanel.vue'), 'utf8')
  const row = /<div class="field-row">\s*<label class="field-label" for="dsh-port-input">端口<\/label>([\s\S]*?)<\/div>/.exec(markup)
  assert.ok(row, 'missing the port field row')

  // The recheck button lives on the same row, after the input.
  assert.match(row[1], /id="dsh-port-input"[\s\S]*?@click="recheckPort"/)
  assert.match(row[1], /重新检测端口/)

  // The input keeps a fixed small width instead of filling the row, and the
  // button is pinned to the row's right edge.
  const css = scopedStyle('src/components/dsh/DshConfigPanel.vue')
  const input = declarations(css, '.field-row > .input--port')
  assert.equal(input.get('flex'), '0 0 auto')
  assert.ok(input.get('width') !== undefined, 'the port input must be shortened to a fixed width')
  assert.equal(declarations(css, '.field-row__port-action').get('margin-left'), 'auto')

  // The hint text line under the row is gone for good.
  assert.doesNotMatch(markup, /field-help--row|portHelp/)
})

test('the dsh action row holds only the service controls', () => {
  const markup = readFileSync(resolve(repoRoot, 'src/components/dsh/DshConfigPanel.vue'), 'utf8')
  const actions = /<div class="action-row">([\s\S]*?)<\/div>/.exec(markup)
  assert.ok(actions, 'missing dsh action row')

  // 复制链接 / 打开网页 / 二维码 used to live here and handed out a single
  // guessed address. They are per-address row actions now, so the action row
  // must not grow a second way to reach the service.
  assert.match(actions[1], /保存设置|设置已保存/)
  assert.match(actions[1], /@click="store\.start\(\)"/)
  assert.match(actions[1], /@click="store\.stop\(\)"/)
  assert.doesNotMatch(actions[1], /复制链接|打开网页|二维码/)
})

/**
 * dsh has no sidebar footer, so its settings entry lives at the left of the
 * bottom row (where the 进入DeepSeek Harness button first appeared), and the
 * jump button sits at the row's right edge. The settings entry must stay the
 * same one as the other front-ends' bottom-left one: the shared
 * `useSettingsPopover` popover, the `.settings-entry` class (App.vue's
 * click-outside closer recognises the trigger by that class), and the ⚙ 设置
 * label.
 */
test('the dsh bottom row holds the settings entry on the left and the jump button on the right', () => {
  const markup = readFileSync(resolve(repoRoot, 'src/components/dsh/DshConfigPanel.vue'), 'utf8')
  const row = /<div class="runtime-entry">([\s\S]*?)<\/div>/.exec(markup)
  assert.ok(row, 'missing the dsh bottom row')

  // Order inside the row: 设置 first (left), 进入DeepSeek Harness second (right).
  assert.match(
    row[1],
    /@click="toggleSettings\(\$event\)"[\s\S]*?@click="emit\('open-runtime'\)"/,
  )
  // Same trigger contract as the sidebar footers: class + label + handler.
  assert.match(row[1], /class="settings-entry runtime-entry__settings"/)
  assert.match(row[1], /⚙ <span>设置<\/span>/)
  assert.match(markup, /import \{ useSettingsPopover \} from '@\/composables\/useSettingsPopover'/)
  assert.match(markup, /const \{ toggleSettings \} = useSettingsPopover\(\)/)

  // The action row no longer carries the settings entry.
  const actions = /<div class="action-row">([\s\S]*?)<\/div>/.exec(markup)
  assert.ok(actions, 'missing dsh action row')
  assert.doesNotMatch(actions[1], /toggleSettings/)

  const css = scopedStyle('src/components/dsh/DshConfigPanel.vue')
  // 两端分布是把「右侧」落成布局的那一行。
  assert.equal(declarations(css, '.runtime-entry').get('justify-content'), 'space-between')
  // The sidebar-footer style is width: 100%; the inline use must take it back.
  const settings = declarations(css, '.runtime-entry__settings')
  assert.equal(settings.get('width'), 'auto')
  assert.equal(settings.get('flex'), '0 0 auto')
})

/**
 * The reported bug: with Tailscale installed, 「复制链接」 handed out the 100.x
 * address dsh reported as its LAN one, which no phone on the same Wi-Fi can
 * open. The panel now lists 本机 / 局域网 / Tailscale from the runtime address
 * list, and every action lives on the row it belongs to.
 */
test('every address row carries 二维码 / 复制 / 打开网页, in that order', () => {
  const markup = readFileSync(resolve(repoRoot, 'src/components/dsh/DshConfigPanel.vue'), 'utf8')
  const start = markup.indexOf('<div class="address-list">')
  const end = markup.indexOf('<div class="runtime-entry">')
  assert.ok(start !== -1 && end > start, 'missing dsh access list')
  const block = markup.slice(start, end)

  // Rows come from the runtime address list, so a new interface or a tailnet
  // appears without another UI change.
  assert.match(block, /v-for="row in rows"/)
  assert.match(block, /\{\{ row\.label \}\}/)
  assert.match(block, /\{\{ row\.display \}\}/)

  // 二维码 sits left of 复制, and 打开网页 right of it, all inside the row.
  assert.match(
    block,
    /<li[^>]*v-for="row in rows"[\s\S]*?@click="openQr\(row\)"[\s\S]*?二维码[\s\S]*?@click="copyRowLink\(row\)"[\s\S]*?复制[\s\S]*?@click="openRowLink\(row\)"[\s\S]*?打开网页[\s\S]*?<\/li>/,
  )
  // QR only where another device can actually land: loopback is excluded, so a
  // 127.0.0.1 row has no QR button.
  assert.match(block, /v-if="needsQr\(row\)"[\s\S]*?@click="openQr\(row\)"/)
  assert.match(markup, /function needsQr\(row: DshAddressRow\): boolean \{[\s\S]*?row\.kind === 'lan' \|\| row\.kind === 'tailscale'/)

  assert.match(markup, /function copyRowLink\(row: DshAddressRow\) \{[\s\S]*?copyAddress\(row\)/)
  assert.match(
    markup,
    /async function openRowLink\(row: DshAddressRow\) \{[\s\S]*?await resolveAddress\(row\)[\s\S]*?await open\(value\)/,
  )
  assert.match(markup, /import \{ open \} from '@tauri-apps\/plugin-shell'/)
})

/**
 * The dsh panel's 启动前检测 entry (a secondary button plus a hint sentence) is
 * replaced by a single「进入DeepSeek Harness」jump to the dsh tab. The button
 * must read as a destination, not an action or a warning: a violet of its own,
 * explicitly not the primary blue, the success green, or a warning/danger hue.
 */
test('the dsh panel replaces 启动前检测 with a distinctively coloured jump button', () => {
  const markup = readFileSync(resolve(repoRoot, 'src/components/dsh/DshConfigPanel.vue'), 'utf8')

  // The old entry is gone: no preflight button, no hint sentence.
  assert.doesNotMatch(markup, /preflight-entry|openPreflight|启动前检测/)

  // The new entry emits the jump; the chain CliDshPanel → ConfigWorkspace →
  // App.vue carries it to openCliTab('dsh'), the same path as the title bar.
  assert.match(markup, /class="btn runtime-entry__button"[^>]*@click="emit\('open-runtime'\)"/)
  assert.match(markup, /进入DeepSeek Harness/)

  const cliPanel = readFileSync(resolve(repoRoot, 'src/components/cli/CliDshPanel.vue'), 'utf8')
  assert.match(cliPanel, /@open-runtime="emit\('open-runtime'\)"/)
  const workspace = readFileSync(resolve(repoRoot, 'src/components/config/ConfigWorkspace.vue'), 'utf8')
  assert.match(workspace, /@open-runtime="emit\('open-runtime'\)"/)
  const app = readFileSync(resolve(repoRoot, 'src/App.vue'), 'utf8')
  assert.match(app, /@open-runtime="openDshRuntimeTab"/)
  assert.match(app, /function openDshRuntimeTab\(\) \{[\s\S]*?openCliTab\('dsh'\)/)

  const css = scopedStyle('src/components/dsh/DshConfigPanel.vue')
  const button = declarations(css, '.runtime-entry__button')
  const background = button.get('background-color') ?? ''
  assert.ok(background, 'the jump button must carry its own colour')
  // 紫色 #6B5CE7 的 RGB：蓝最高、红次之、绿最低。主色蓝的绿分量远高于红，警告/
  // 危险色的红远高于蓝——这条规则把「特殊但不是警告色」落成可检查的约束。
  const rgb = /#([0-9A-Fa-f]{2})([0-9A-Fa-f]{2})([0-9A-Fa-f]{2})/.exec(background)
  assert.ok(rgb, `background ${background} must be a hex colour the test can reason about`)
  const [r, g, b] = rgb.slice(1).map((hex) => Number.parseInt(hex, 16))
  assert.ok(b > r && r > g, `background ${background} must be a violet, not blue/amber/red`)
  assert.equal(button.get('color'), '#FFFFFF')
})
