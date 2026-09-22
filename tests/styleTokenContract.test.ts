import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/**
 * 项目的配色令牌只有一套，定义在 `src/assets/styles/theme.css`（亮/暗两个主题块），
 * 少数由运行时经 `document.documentElement.style.setProperty` 写入。
 *
 * 这里钉的是一件很隐蔽的事：**`var(--token)` 不带兜底值时，如果 `--token` 根本
 * 没定义，整条声明会在计算期被判无效 —— 没有任何报错，浏览器也不提示，只是那一行
 * 样式静默消失。** 真实后果是 dsh 版本选择弹窗写了一套**本项目中并不存在**的
 * `--color-background-primary` 名字，于是 `background` 被丢掉，面板变成全透明，
 * 只剩文字浮在应用界面上；同一批失效的还有边框与配色。
 *
 * 所以约束取「用了就必须有定义」：名字写错会在测试里立刻暴露，而不是等到肉眼
 * 看见界面出问题。带兜底的 `var(--x, fallback)` 不在约束内 —— 那种写法本身就
 * 允许未定义。
 */
function collectFiles(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name)
    if (entry.isDirectory()) {
      if (!/node_modules|dist|target/.test(entry.name)) collectFiles(full, out)
    } else {
      out.push(full)
    }
  }
  return out
}

const SOURCE_FILES = collectFiles(resolve(repoRoot, 'src'))
  .filter((file) => /\.(vue|css|ts)$/.test(file))

/** 定义：样式块里的 `--x:` 声明，以及运行时注入的 `setProperty('--x', …)`。 */
function definedTokens(): Set<string> {
  const defined = new Set<string>()
  const consumers = [
    ...SOURCE_FILES,
    resolve(repoRoot, 'index.html'),
  ]
  for (const file of consumers) {
    const source = readFileSync(file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')
    for (const match of source.matchAll(/(--[a-zA-Z0-9-]+)\s*:/g)) defined.add(match[1])
    for (const match of source.matchAll(/setProperty\(\s*['"](--[a-zA-Z0-9-]+)['"]/g)) {
      defined.add(match[1])
    }
  }
  return defined
}

interface Usage {
  file: string
  token: string
}

/** 用法：只取不带兜底值的 `var(--x)`。 */
function bareUsages(): Usage[] {
  const usages: Usage[] = []
  for (const file of SOURCE_FILES.filter((name) => name.endsWith('.vue'))) {
    const source = readFileSync(file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')
    for (const match of source.matchAll(/var\(\s*(--[a-zA-Z0-9-]+)\s*\)/g)) {
      usages.push({ file: file.slice(repoRoot.length + 1).replace(/\\/g, '/'), token: match[1] })
    }
  }
  return usages
}

test('every var() without a fallback resolves to a defined style token', () => {
  const defined = definedTokens()
  const unresolved = bareUsages()
    .filter((usage) => !defined.has(usage.token))
    .map((usage) => `${usage.file}: var(${usage.token})`)

  assert.deepEqual(
    [...new Set(unresolved)].sort(),
    [],
    'these var() references have no fallback and the token is defined nowhere; '
      + 'the whole declaration is silently dropped at computed-value time',
  )
})

/**
 * 面板的背景色必须真的解析得出来。上面那条通则已经覆盖了「名字写错」，
 * 这里再钉一次最要命的那个属性：弹窗没有背景就等于没有容器。
 */
function scopedStyle(relativePath: string): string {
  const source = readFileSync(resolve(repoRoot, relativePath), 'utf8')
  const block = /<style\s+scoped>([\s\S]*?)<\/style>/.exec(source)
  assert.ok(block, `${relativePath} has no scoped style block`)
  return block[1].replace(/\/\*[\s\S]*?\*\//g, '')
}

function declarations(css: string, selector: string): Map<string, string> {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const rule = new RegExp(`${escaped}\\s*\\{([^}]*)\\}`).exec(css)
  assert.ok(rule, `missing rule for ${selector}`)

  const result = new Map<string, string>()
  for (const declaration of rule[1].split(';')) {
    const separator = declaration.indexOf(':')
    if (separator === -1) continue
    result.set(declaration.slice(0, separator).trim(), declaration.slice(separator + 1).trim())
  }
  return result
}

const PICKER = 'src/components/dsh/DshVersionPickerDialog.vue'

test('the version picker panel paints an opaque surface', () => {
  const panel = declarations(scopedStyle(PICKER), '.version-dialog__panel')
  const background = panel.get('background')
  assert.ok(background, 'the panel must declare a background or it renders see-through')

  const token = /var\(\s*(--[a-zA-Z0-9-]+)/.exec(background)
  if (token) {
    assert.ok(
      definedTokens().has(token[1]),
      `${background} names a token that is defined nowhere, so the background is dropped`,
    )
  }
})

/**
 * 高度上限必须落在视口单位上。此前写的是 `min(520px, 100%)`，而父级高度由内容撑开、
 * 是不确定值，百分比解析不出结果 → 整条 `max-height` 失效 → 列表一路溢出到窗口外，
 * 页脚的按钮被推出可视区。
 */
test('the version picker bounds its height against the viewport', () => {
  const panel = declarations(scopedStyle(PICKER), '.version-dialog__panel')
  const maxHeight = panel.get('max-height')
  assert.ok(maxHeight, 'the panel must bound its own height')
  assert.match(
    maxHeight,
    /vh/,
    `${maxHeight} has no viewport unit; a percentage here resolves against a `
      + 'content-sized parent and the whole declaration is dropped',
  )
})

test('the version list is the only scroll region', () => {
  const css = scopedStyle(PICKER)
  const list = declarations(css, '.version-list')
  assert.equal(list.get('overflow-y'), 'auto', 'the list must own the scrolling')
  assert.equal(
    list.get('min-height'),
    '0',
    'without min-height: 0 a flex child refuses to shrink and defeats its own scroller',
  )

  // 标题与页脚不参与收缩，否则长列表会把它们挤扁。
  assert.equal(declarations(css, '.version-dialog__head').get('flex-shrink'), '0')
  assert.equal(declarations(css, '.version-dialog__foot').get('flex-shrink'), '0')
})
