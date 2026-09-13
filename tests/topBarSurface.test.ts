import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'

/**
 * Regression tests for the top-bar surface.
 *
 * Two defects are covered here.
 *
 * 1. A hard seam in the title bar: the bar painted its own `--card-bg-gradient`
 *    band, the sidebar column started another `--app-bg-gradient` at its own
 *    left edge, a full-height `border-right` ran between them and the tab strip
 *    drew a `border-bottom` across the rest of the row.
 * 2. The CLI entries followed the catalog sidebar. The workspace section was
 *    sized from `--project-nav-width`, which the sidebar drag handle reported,
 *    so every sidebar drag — and collapsing it — moved `Claude Code` and the
 *    other entries sideways.
 *
 * Both fixes are structural, so these tests read the component sources and
 * assert the arrangement rather than a rendered layout.
 */

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/** Every `selector { ... }` pair of the first `<style>` block of a component. */
function styleRules(relativePath: string): Map<string, string> {
  const source = readFileSync(resolve(repoRoot, relativePath), 'utf8')
  const styleBlock = /<style[^>]*>([\s\S]*?)<\/style>/.exec(source)
  assert.ok(styleBlock, `${relativePath} has no <style> block`)

  const css = styleBlock[1].replace(/\/\*[\s\S]*?\*\//g, '')
  const rules = new Map<string, string>()
  const pattern = /([^{}]+)\{([^{}]*)\}/g
  let match: RegExpExecArray | null

  while ((match = pattern.exec(css)) !== null) {
    const selector = match[1].trim().replace(/\s+/g, ' ')
    // At-rules such as `@media (...) {` carry no declarations of their own.
    // Nested rules inside them match on their own and are kept.
    if (selector.startsWith('@') || selector.length === 0) continue
    const body = match[2].replace(/\s+/g, ' ').trim()
    // A selector can be declared more than once (a base rule plus a narrower
    // media-query override); later declarations win, as they do in the cascade.
    const previous = rules.get(selector)
    rules.set(selector, previous === undefined ? body : `${previous}; ${body}`)
  }

  return rules
}

function body(rules: Map<string, string>, selector: string): string {
  const found = rules.get(selector)
  assert.ok(found !== undefined, `missing rule for ${selector}`)
  return found
}

function declarations(raw: string): Map<string, string> {
  const parsed = new Map<string, string>()
  // Component sources can be CRLF; the map keys must not carry line endings.
  for (const chunk of raw.replace(/\s+/g, ' ').split(';')) {
    const separator = chunk.indexOf(':')
    if (separator === -1) continue
    parsed.set(chunk.slice(0, separator).trim(), chunk.slice(separator + 1).trim())
  }
  return parsed
}

function template(relativePath: string): string {
  const source = readFileSync(resolve(repoRoot, relativePath), 'utf8')
  const block = /<template>([\s\S]*?)<\/template>/.exec(source)
  assert.ok(block, `${relativePath} has no <template> block`)
  return block[1]
}

test('the layout is the single owner of the app background', () => {
  const app = styleRules('src/App.vue')
  const layout = declarations(body(app, '.app-layout'))

  assert.equal(layout.get('background'), 'var(--app-bg-gradient)')
  // A fixed attachment anchors the gradient to the viewport instead of the box,
  // which is what dragged the sparse dark-theme gradient off the sidebar column.
  assert.equal(layout.has('background-attachment'), false)
  assert.equal(declarations(body(app, '.app-content')).get('background'), 'transparent')
})

test('the title bar keeps its own surface out of the way', () => {
  const app = styleRules('src/App.vue')
  const titleBar = declarations(body(app, '.title-bar'))
  const tabs = declarations(body(app, '.title-bar__tabs'))

  // The bar used to paint `--card-bg-gradient` while everything under it painted
  // `--app-bg-gradient`: the two gradients met as a visible step.
  assert.equal(titleBar.get('background'), 'transparent')
  assert.equal(titleBar.has('background-image'), false)
  assert.equal(tabs.has('border-bottom'), false)
  assert.equal(tabs.get('padding'), '0 8px 0 0')
})

test('the sidebar column of the title bar draws no full-height rule', () => {
  const app = styleRules('src/App.vue')
  const section = declarations(body(app, '.title-bar__workspace-section'))

  assert.equal(section.has('border-right'), false)
  assert.equal(section.has('background'), false, 'a sidebar-local gradient re-introduces the seam')

  // The remaining separator is a short centred hairline between the workspace
  // switch and the CLI entries.
  const divider = declarations(body(app, '.title-bar__divider'))
  assert.equal(divider.get('height'), '16px')
  assert.equal(divider.get('align-self'), 'center')
  assert.equal(divider.has('top'), false, 'a full-height or pinned rule is not a short divider')
  // A 1px painted centre inside a padded box: the padding is what keeps the
  // selected workspace pill off the rule, and it is asymmetric on purpose — the
  // pill side only needs to stop touching, the tab side already has the strip's
  // own 8px inset.
  assert.equal(divider.get('width'), '18px')
  assert.equal(divider.get('padding'), '0 12px 0 5px')
  assert.equal(divider.get('background-clip'), 'content-box')
})

test('the divider is its own element in the bar, not the sidebar edge', () => {
  // A pseudo-element on the workspace section could only ever sit at that
  // section's right edge, which is what tied it to the sidebar width.
  assert.match(template('src/App.vue'), /class="title-bar__divider"/)
  assert.doesNotMatch(template('src/App.vue'), /--project-nav-width/)
})

test('the CLI entries hold a fixed position regardless of the sidebar', () => {
  const app = styleRules('src/App.vue')
  const source = readFileSync(resolve(repoRoot, 'src/App.vue'), 'utf8')

  // The slot is sized by its own content, never by the reported sidebar width.
  const slot = declarations(body(app, '.title-bar__workspace-switch'))
  assert.equal(slot.get('flex'), '0 0 auto')
  // 163px of controls (toggle + both workspace pills) plus the divider's 18px box.
  assert.equal(slot.get('min-width'), '181px')

  const modeTabs = declarations(body(app, '.title-bar__mode-tabs'))
  assert.equal(modeTabs.get('flex'), '0 0 auto')
  assert.equal(modeTabs.get('display'), 'flex')

  // Collapsing the sidebar must not remove the switch from the flow: `v-if`
  // would shrink the slot and pull the CLI entries left.
  const bar = template('src/App.vue')
  assert.match(bar, /class="title-bar__mode-tabs"[\s\S]*?aria-label="工作区"/)
  assert.doesNotMatch(bar, /v-if="leftSidebarOpen"/)
  assert.equal(declarations(body(app, '.title-bar__mode-tabs--hidden')).get('visibility'), 'hidden')

  // The bar carries no inline width binding at all.
  assert.doesNotMatch(source, /'--project-nav-width'/)
})

test('panels under the title bar do not repaint the app background', () => {
  const surfaces: Array<[string, string]> = [
    ['src/components/config/ConfigWorkspace.vue', '.config-workspace'],
    ['src/components/claude/ClaudePanel.vue', '.claude-panel'],
    ['src/components/claude/ClaudePanel.vue', '.claude-panel__sidebar'],
    ['src/components/project/ProjectSidebar.vue', '.project-sidebar'],
    ['src/components/project/ProjectSidebar.vue', '.project-sidebar__header'],
    ['src/components/project/ModuleToolbar.vue', '.module-toolbar'],
    ['src/components/dsh/DshRuntimePanel.vue', '.dsh-runtime-panel'],
  ]

  for (const [file, selector] of surfaces) {
    const style = declarations(body(styleRules(file), selector))
    const painted = style.get('background') ?? style.get('background-color')
    assert.equal(painted, 'transparent', `${selector} must stay transparent over the shared background`)
    assert.equal(style.has('background-image'), false, `${selector} must not layer a gradient`)
    assert.equal(style.has('background-attachment'), false, `${selector} must not fix the gradient`)
  }
})

test('the divider patch between the sidebar and the module toolbar stays clear', () => {
  const projectPanel = styleRules('src/components/project/ProjectPanel.vue')
  const bridge = declarations(body(projectPanel, '.project-panel__divider--left::before'))

  // An opaque bridge colour here showed up as a rectangle punched out of the top bar.
  assert.equal(bridge.get('background'), 'transparent')
  assert.equal(bridge.has('background-color'), false)
})

test('the CLI entries sit against the workspace switch', () => {
  const app = styleRules('src/App.vue')
  const tabs = declarations(body(app, '.title-bar__tabs'))
  const section = declarations(body(app, '.title-bar__workspace-section'))

  // No leading padding on the strip and no trailing gap inside the section: the
  // divider plus the tab strip's own 8px inset is the entire spacing.
  assert.equal(tabs.get('padding'), '0 8px 0 0')
  assert.equal(tabs.get('justify-content'), 'flex-start')
  assert.equal(section.get('padding'), '0 8px')
})

/** Custom properties of one theme block of theme.css. */
function themeTokens(selector: string): Map<string, string> {
  const css = readFileSync(resolve(repoRoot, 'src/assets/styles/theme.css'), 'utf8')
    .replace(/\/\*[\s\S]*?\*\//g, '')
  const block = new RegExp(`(?:^|\\})\\s*${selector}\\s*\\{([^}]*)\\}`).exec(css)
  assert.ok(block, `theme.css has no ${selector} block`)

  const tokens = new Map<string, string>()
  for (const line of block[1].split(';')) {
    const separator = line.indexOf(':')
    if (separator === -1) continue
    tokens.set(line.slice(0, separator).trim(), line.slice(separator + 1).trim())
  }
  return tokens
}

test('the configuration editor pane is recessed only in the dark theme', () => {
  const light = themeTokens(':root')
  const dark = themeTokens('\\[data-theme="dark"\\]')

  assert.equal(light.get('--editor-surface'), light.get('--bg'))
  assert.equal(dark.get('--editor-surface'), '#0F1216')
  // "Clearly deeper than the chrome" is the whole point: the top bar, the profile
  // sidebar and the app background stay light while the editor pane goes below
  // them, and the step has to be big enough to read as a separate region rather
  // than a neighbouring shade.
  const depth = (hex: string) => Number.parseInt(hex.slice(1), 16)
  const editor = depth(dark.get('--editor-surface')!)
  assert.ok(editor < depth(dark.get('--bg')!), 'the dark editor surface must be darker than the chrome')
  assert.ok(depth(dark.get('--bg')!) - editor > 0x0A0000, 'the step must be visible, not a shade')

  // The chrome itself must stay light: an app gradient that drifts down into the
  // editor's range is what made the whole window read as one flat dark mass.
  const gradientEnds = dark.get('--app-bg-gradient')!.match(/#[0-9A-Fa-f]{6}/g) ?? []
  assert.equal(gradientEnds.length, 3)
  for (const end of gradientEnds) {
    assert.ok(depth(end) > editor + 0x0A0000, `gradient stop ${end} is too close to the editor pane`)
  }

  const components = readFileSync(resolve(repoRoot, 'src/assets/styles/components.css'), 'utf8')
  // The theme flag is on `<html>`, so it is an ancestor of `#app`: writing the
  // selector the other way round makes the rule silently dead, which is how the
  // recess never appeared. Check real selectors one by one — comments discuss the
  // broken form on purpose, and plain `#app { … }` custom-property blocks have an
  // empty selector.
  const selectors = components
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .split('{')
    .map((chunk) => (chunk.split('}').pop() ?? '').trim())
    .filter((part) => /[.#[:*]/.test(part))
  for (const selector of selectors) {
    assert.doesNotMatch(selector, /#app\s*\[data-theme/, 'the theme flag is above #app, not inside it')
  }
  // Every configuration workspace must be covered by BOTH the recess and the card
  // rule. OpenCode and dsh share `.config-content`, so dropping that class leaves
  // the OpenCode editor light while the other three go dark.
  const recessRule = /\[data-theme="dark"\] #app :is\(([\s\S]*?)\)\s*\{[^}]*--editor-surface/.exec(components)
  assert.ok(recessRule, 'components.css has no recess rule')
  const cardRule = /\[data-theme="dark"\] #app :is\(([\s\S]*?)\)\s*\.card/.exec(components)
  assert.ok(cardRule, 'components.css has no card rule for the editor panes')
  for (const hook of ['.codex-config-panel__content', '.claude-panel__content', '.config-content', '.dsh-config-panel']) {
    assert.ok(recessRule[1].includes(hook), `${hook} is missing from the recess rule`)
    assert.ok(cardRule[1].includes(hook), `${hook} is missing from the card rule`)
  }
  assert.match(components, /background:\s*var\(--editor-surface\)/)
  // Cards borrow the pane instead of painting their own grey on top of it.
  assert.match(components, /\.card\s*\{[\s\S]*?background-color:\s*transparent/)
})

test('the configuration sidebar shares the title bar surface', () => {
  // A flat `--bg` sidebar against a gradient title bar is what made the two read
  // as different colours; both have to be transparent over the app background.
  const codex = declarations(body(styleRules('src/components/codex/CodexConfigPanel.vue'), '.codex-config-panel__sidebar'))
  assert.equal(codex.get('background'), 'transparent')

  const opencode = declarations(body(styleRules('src/components/opencode/OpenCodeConfigPanel.vue'), '.provider-sidebar'))
  assert.equal(opencode.get('background'), undefined, 'a sidebar without a background already inherits the app surface')

  const app = styleRules('src/App.vue')
  assert.equal(declarations(body(app, '.title-bar')).get('background'), 'transparent')
})

test('the sidebar split keeps its resize handle but drops the resting rule', () => {
  const dividers: Array<[string, string, string]> = [
    ['src/components/codex/CodexConfigPanel.vue', '.codex-config-panel__divider', '.codex-config-panel__divider::after'],
    ['src/components/claude/ClaudePanel.vue', '.claude-panel__divider', '.claude-panel__divider::after'],
    ['src/components/opencode/OpenCodeConfigPanel.vue', '.opencode-config-panel__divider', '.opencode-config-panel__divider::after'],
  ]

  for (const [file, strip, rule] of dividers) {
    const style = styleRules(file)
    // The drag target itself must survive: width, cursor, and the mousedown hook.
    const handle = declarations(body(style, strip))
    assert.equal(handle.get('cursor'), 'col-resize', `${strip} must stay a resize handle`)
    assert.equal(handle.get('width'), '9px', `${strip} must keep its grab width`)
    assert.match(readFileSync(resolve(repoRoot, file), 'utf8'), /@mousedown="onMouseDown"/)

    // Only the 1px resting rule is gone; hover and drag still highlight.
    assert.equal(declarations(body(style, rule)).get('background-color'), 'transparent')
    const hover = declarations(body(style, `${strip}:hover::after, ${strip}--dragging::after`))
    assert.equal(hover.get('background-color'), 'var(--primary)')
    assert.equal(hover.get('width'), '2px')
  }
})

test('the editor pane is rounded where it tucks under the chrome', () => {
  const components = readFileSync(resolve(repoRoot, 'src/assets/styles/components.css'), 'utf8')
  const radiusRule = /#app :is\(([\s\S]*?)\)\s*\{([^}]*border-top-left-radius[^}]*)\}/.exec(components)
  assert.ok(radiusRule, 'components.css has no radius rule for the editor panes')
  for (const hook of ['.codex-config-panel__content', '.claude-panel__content', '.config-content']) {
    assert.ok(radiusRule[1].includes(hook), `${hook} must carry the rounded top-left corner`)
  }
  assert.match(radiusRule[2], /border-top-left-radius:\s*var\(--radius-lg/)

  // Only the top-left corner: the pane still meets the window edge squarely.
  assert.doesNotMatch(radiusRule[2], /[^-]border-radius:/)
  assert.doesNotMatch(radiusRule[2], /top-right|bottom-left|bottom-right/)

  // dsh has no sidebar, so its own pane carries the same corner.
  const dsh = declarations(body(styleRules('src/components/dsh/DshConfigPanel.vue'), '.config-content'))
  assert.equal(dsh.get('border-top-left-radius'), 'var(--radius-lg, 12px)')
})

test('the editor pane starts below the title bar', () => {
  const components = readFileSync(resolve(repoRoot, 'src/assets/styles/components.css'), 'utf8')
  const paneRule = /#app :is\(([\s\S]*?)\)\s*\{([^}]*margin-top:\s*var\(--editor-pane-inset[^}]*)\}/.exec(components)
  assert.ok(paneRule, 'the editor pane has no top inset')

  // A margin, not padding: the pane's own colour must not reach the title bar.
  assert.doesNotMatch(paneRule[2], /^\s*padding(-top)?:\s*var\(--editor-pane-inset/)
  // The interior padding is trimmed by the inset so the first row does not move.
  assert.match(paneRule[2], /padding-top:\s*calc\(var\(--editor-pane-padding-top[^)]*\)\s*-\s*var\(--editor-pane-inset[^)]*\)\)/)
  // The inset is a single token, so it can be retuned in one place. It has to
  // stay a small strip: a huge inset would detach the pane from the title bar.
  const inset = /--editor-pane-inset:\s*(\d+)px/.exec(components)
  assert.ok(inset, 'components.css has no --editor-pane-inset token')
  assert.ok(Number(inset[1]) >= 4 && Number(inset[1]) <= 24, `inset ${inset[1]}px is outside the intended range`)
  // Keep the interior padding formula valid: it must not go negative.
  const paddingTop = /--editor-pane-padding-top:\s*(\d+)px/.exec(components)
  assert.ok(paddingTop, 'components.css has no --editor-pane-padding-top token')
  assert.ok(Number(paddingTop[1]) >= Number(inset[1]), 'the pane padding would become negative')

  // dsh supplies its own padding, so it only takes the margin.
  const dsh = declarations(body(styleRules('src/components/dsh/DshConfigPanel.vue'), '.config-content'))
  assert.equal(dsh.get('margin-top'), 'var(--editor-pane-inset, 12px)')
  assert.equal(dsh.get('padding'), '0 16px 12px 28px')
})

test('the project workspace uses the same chrome and recess as the configuration ones', () => {
  const panel = styleRules('src/components/project/ProjectPanel.vue')

  // An opaque `--bg` on the panel was what painted the whole project view dark and
  // hid the chrome gradient behind the sidebar column.
  assert.equal(declarations(body(panel, '.project-panel')).get('background'), 'transparent')

  // The vertical split keeps its grab strip but drops the resting rule.
  const split = declarations(body(panel, '.project-panel__divider'))
  assert.equal(split.get('cursor'), 'col-resize')
  assert.equal(declarations(body(panel, '.project-panel__divider::after')).get('background-color'), 'transparent')
  const splitHover = declarations(body(panel, '.project-panel__divider:hover::after, .project-panel__divider--dragging::after'))
  assert.equal(splitHover.get('background-color'), 'var(--primary)')

  const components = readFileSync(resolve(repoRoot, 'src/assets/styles/components.css'), 'utf8')
  const terminal = /#app \.project-terminal\s*\{([^}]*)\}/.exec(components)
  assert.ok(terminal, 'the terminal pane has no rule')
  // The same rounded top-left corner as the configuration editor; the remaining
  // corners are rounded too because the pane runs to the window edge.
  assert.match(terminal[1], /border-radius:\s*var\(--radius-lg, 12px\)\s+var\(--radius-lg, 12px\)\s+var\(--radius-lg, 12px\)\s+0/)
  // No gap on the right or bottom, and none on the left either: only the top edge
  // is tunable, for the toolbar row.
  assert.match(terminal[1], /margin:\s*var\(--project-pane-inset, 0\)\s+0\s+0\s+0/)

  // The toolbar is the top of the recessed block: no rule cutting it in two.
  assert.match(components, /\[data-theme="dark"\] #app \.module-toolbar\s*\{[^}]*border-bottom-color:\s*transparent/)

  // Darker than the PTY surface, and a real step rather than a shade.
  const dark = themeTokens('\\[data-theme="dark"\\]')
  const depth = (hex: string) => Number.parseInt(hex.slice(1), 16)
  const project = depth(dark.get('--project-surface')!)
  assert.ok(project < depth(dark.get('--terminal-bg')!), 'the project pane must be deeper than the PTY surface')
  assert.ok(depth(dark.get('--bg')!) - project > 0x040000, 'the step against the chrome must be visible')
  assert.match(components, /\[data-theme="dark"\] #app \.project-terminal\s*\{[^}]*background:\s*var\(--project-surface\)/)
})
