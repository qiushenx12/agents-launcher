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
 * The reported bug: with Tailscale installed, 「复制链接」 handed out the 100.x
 * address dsh reported as its LAN one, which no phone on the same Wi-Fi can
 * open. The panel now lists 本机 / 局域网 / Tailscale from the runtime address
 * list, and every action lives on the row it belongs to.
 */
test('every address row carries 二维码 / 复制 / 打开网页, in that order', () => {
  const markup = readFileSync(resolve(repoRoot, 'src/components/dsh/DshConfigPanel.vue'), 'utf8')
  const start = markup.indexOf('<div class="address-list">')
  const end = markup.indexOf('<div class="preflight-entry">')
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
