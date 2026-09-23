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
    /*
     * Vue 的 `:style` 绑定写的是对象字面量键 —— `'--x': \`${n}px\``。令牌与冒号
     * 之间隔着一个引号，上面那条 CSS 声明的模式匹配不到，于是这类运行时注入的
     * 名字会被误判成「无处定义」。它们确实是定义：绑定生效后令牌就在元素上。
     */
    for (const match of source.matchAll(/['"](--[a-zA-Z0-9-]+)['"]\s*:/g)) {
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
 * 兜底值本身也是一条静默失效的路。
 *
 * `CodexSessionIssuesDialog` 写的是 `var(--bg-primary, #fff)` 与
 * `var(--border-color, #e2e5ea)`，两个名字在本项目里都不存在 —— 而带兜底的
 * `var()` 恰好落在上面那条通则的盲区里（通则只钉「不带兜底又没定义」）。结果
 * 是深色主题下弹窗照旧白底黑字、边框浅灰，只有按钮跟着主题变了，整块面板像
 * 贴在界面上的外来窗口，而测试全绿。
 *
 * 所以把兜底值也纳入约束：只要写了 `var(--x, …)`，`--x` 就必须真的被定义。
 * 想给某个令牌留「未注入时退回」的语义，就该用运行时注入的那个名字（如
 * `--settings-menu-max-height`，由 App.vue 的 setProperty 声明），它同样能通过。
 */
test('every var() with a fallback still names a defined style token', () => {
  const defined = definedTokens()
  const unresolved: string[] = []
  for (const file of SOURCE_FILES.filter((name) => name.endsWith('.vue'))) {
    const source = readFileSync(file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')
    for (const match of source.matchAll(/var\(\s*(--[a-zA-Z0-9-]+)\s*,/g)) {
      if (!defined.has(match[1])) {
        unresolved.push(`${file.slice(repoRoot.length + 1).replace(/\\/g, '/')}: var(${match[1]}, …)`)
      }
    }
  }

  assert.deepEqual(
    [...new Set(unresolved)].sort(),
    [],
    'a fallback hides a misspelled token name: the fallback paints in BOTH themes, '
      + 'so the light-theme colour leaks into the dark theme and nothing errors',
  )
})

/**
 * 会话完整性弹窗的面板必须取主题令牌，不能靠兜底值。
 *
 * 这条把「弹窗在深色主题下是白底」这个具体故障钉住：面板底色、正文色与边框
 * 三者缺一不可，只修按钮是修不好的。
 */
const SESSION_DIALOG = 'src/components/codex/CodexSessionIssuesDialog.vue'

test('the codex session dialog follows the theme instead of its fallbacks', () => {
  const panel = declarations(scopedStyle(SESSION_DIALOG), '.session-issues')
  for (const property of ['background', 'color', 'border']) {
    const value = panel.get(property)
    assert.ok(value, `.session-issues must declare ${property}`)
    assert.match(
      value,
      /var\(\s*--/,
      `${property}: ${value} is hard-coded, so it cannot follow the theme`,
    )
    assert.doesNotMatch(
      value,
      /#fff|#ffffff|rgba\(/i,
      `${property}: ${value} carries a literal colour, which paints the same in both themes`,
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
