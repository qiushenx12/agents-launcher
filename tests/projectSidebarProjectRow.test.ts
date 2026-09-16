import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'

/**
 * Regression tests for the 项目列表 collapse affordance.
 *
 * Request: 「项目界面左侧的项目列表，折叠箭头可以去掉，但是不要把折叠功能也删了！
 * 折叠功能还是要保留的，点击项目名称那块就能展开或折叠」
 *
 * The per-project ▸/▾ arrow is gone; collapsing sessions is still available, and
 * its click surface is the whole project row — the project name included, since
 * the name is a child of that row. These tests are structural (they read the
 * component source) because the behaviour lives in the template wiring, not in a
 * function that can be imported.
 */

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const source = readFileSync(resolve(repoRoot, 'src/components/project/ProjectSidebar.vue'), 'utf8')

test('the project row no longer renders a collapse arrow', () => {
  assert.equal(source.includes('project-row__toggle'), false)
})

test('the click surface is the project row itself, not a dedicated toggle', () => {
  const rowTag = /<div\s+class="project-row"[\s\S]*?>/.exec(source)?.[0] ?? ''
  assert.ok(rowTag, 'the .project-row element was not found')
  assert.ok(
    rowTag.includes('@click="onProjectRowClick(project.id)"'),
    'the project row must keep its own click handler',
  )
  // Manual sorting drags the same row, so the toggle must coexist with it.
  assert.ok(rowTag.includes('@pointerdown="onProjectRowPointerDown(index, $event)"'))
})

test('clicking the row still toggles the session list', () => {
  assert.match(source, /async function onProjectRowClick\(projectId: string\)/)
  assert.match(source, /await store\.toggleProjectExpanded\(projectId\)/)
  // And the list it controls is still driven by the expanded set.
  assert.match(source, /v-if="isExpanded\(project\.id\)"/)
})

test('the project name is still part of that row', () => {
  assert.match(source, /<span class="project-row__name">\{\{ project\.name \}\}<\/span>/)
})
