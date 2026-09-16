import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'

/**
 * Regression tests for the 模型目录（models.json）drag-and-drop sorting.
 *
 * Request: 「codex的使用模板配置，需要增加排序拖动功能，现在无法进行排序，要注意的是
 * 配置好之后，在codex内部使用的时候排序也要是正确的，所以不能只是app内配置面板顺序
 * 的挪动」
 *
 * The trap this file guards against is the second half: Codex orders its model list by
 * the per-model `priority` field, not by the array order — the app-server `model/list`
 * endpoint the pickers read sorts by `priority` ascending and marks the first entry
 * `isDefault` (measured on Codex 0.154.0: an array of [p9, p1, p5] comes back as
 * p1, p5, p9). So a reorder has to reach the data that is written to models.json; the
 * Rust side projects the array positions onto `priority` and is covered by
 * `model_catalog_projects_model_order_onto_priority` in codex_config.rs.
 *
 * What is asserted here is the frontend half: the drag is wired to the store (not to
 * anything display-only), the drag lands on a dedicated handle so the row's inputs stay
 * usable, and the header row keeps one column per grid track.
 */

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
/** Checked in with CRLF on Windows; normalise so the patterns stay readable. */
const readSource = (relativePath: string) => readFileSync(resolve(repoRoot, relativePath), 'utf8')
  .replace(/\r\n/g, '\n')
const panelSource = readSource('src/components/codex/CodexConfigPanel.vue')
const storeSource = readSource('src/stores/codexConfig.ts')

/** The `<div class="model-row" …>` opening tag of the model list. */
function modelRowTag(): string {
  return /<div\s+v-for="\(model, index\) in catalogModels"[\s\S]*?>/.exec(panelSource)?.[0] ?? ''
}

/** The whole model row block (from the v-for down to the delete button). */
function modelRowBlock(): string {
  const start = panelSource.indexOf('<div\n                v-for="(model, index) in catalogModels"')
  const end = panelSource.indexOf('title="删除模型"')
  return start >= 0 && end > start ? panelSource.slice(start, end) : ''
}

test('the model list rows are drag items', () => {
  assert.ok(modelRowTag().includes('data-drag-item'), 'the row must be a drag item')
})

test('the drag is wired to the store, so the reorder reaches the data', () => {
  assert.match(panelSource, /=> store\.reorderModels\(newOrder\)/)
  assert.match(panelSource, /useDragReorder<CodexModelDefinition>\(/)
  assert.match(panelSource, /\(\) => catalogModels\.value/)
  // The store must really permute the catalog array that gets rendered into
  // models.json, not only re-render the list in a different order.
  assert.match(storeSource, /function reorderModels\(nextOrder: CodexModelDefinition\[\]\)/)
  assert.match(storeSource, /models\.splice\(0, models\.length, \.\.\.nextOrder\)/)
  assert.match(storeSource, /^\s{4}reorderModels,$/m)
})

test('each drag instance is independent of the profile list drag', () => {
  // Both lists live in this one component; sharing draggingIndex/overIndex made the
  // two drags fight over the same state.
  assert.match(panelSource, /draggingIndex: modelDraggingIndex/)
  assert.match(panelSource, /overIndex: modelOverIndex/)
  assert.match(panelSource, /onPointerDown: onModelDragPointerDown/)
  assert.match(panelSource, /modelDraggingIndex === index/)
})

test('the drag handle is the only pointerdown target, so the row inputs stay usable', () => {
  const rowTag = modelRowTag()
  assert.equal(
    rowTag.includes('@pointerdown'),
    false,
    'the row itself must not capture pointerdown: its inputs and selects need it',
  )
  const block = modelRowBlock()
  assert.ok(block.includes('class="model-row__drag-handle"'), 'the handle is missing')
  assert.ok(
    block.includes('@pointerdown="onModelDragPointerDown(index, $event)"'),
    'the handle must start the drag',
  )
})

test('the row pitch passed to the drag matches the CSS row spacing', () => {
  const gap = /useDragReorder<CodexModelDefinition>\([\s\S]*?\{ gapPx: (\d+) \}/.exec(panelSource)
  const css = /\.model-row \{ margin-top: (\d+)px; \}/.exec(panelSource)
  assert.ok(gap && css, 'gapPx or the .model-row margin is missing')
  assert.equal(gap[1], css[1], 'gapPx must equal the .model-row margin-top')
})

test('the column header keeps one cell per grid track', () => {
  const header = /<div class="model-columns"[\s\S]*?<\/div>/.exec(panelSource)?.[0] ?? ''
  const headerCells = (header.match(/<span/g) ?? []).length
  const tracks = /\.model-columns,\s*\.model-row \{[\s\S]*?grid-template-columns:([^;]+);/
    .exec(panelSource)?.[1]
    .match(/minmax\([^)]*\)|\S+/g)?.length ?? 0
  assert.equal(headerCells, tracks, 'the header labels drifted out of the grid columns')
})
