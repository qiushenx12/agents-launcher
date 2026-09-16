import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'
import {
  cliAvailabilityGateApplies,
  cliWorkspaceCanMount,
  entryBlockedWhileChecking,
} from '../src/utils/cliEntry.ts'

/**
 * Regression tests for the reported bug:
 *
 *   「dsh 的配置界面，启动服务后，点击「进入DeepSeek Harness」按钮没有反应，
 *    不仅是这个按钮没反应，点击上方的项目 + dsh 组合都没反应。过了比较久之后
 *    可以进入。」
 *
 * Root cause: the dsh entry was coupled to the CLI availability check. dsh's check
 * is an `npx` probe that resolves over the network and can take tens of seconds,
 * and while one was in flight
 *
 *   1. `openCliTab` dropped the click outright (`checking['dsh']` guard), and
 *   2. the project workspace panel was not mounted at all (its `v-if` required
 *      `state === 'ready'`), the CLI gate covering the content area instead.
 *
 * Both conditions could only clear when the probe settled — hence "过了比较久之后
 * 可以进入". The rules live in `src/utils/cliEntry.ts`; this file pins them, and
 * then checks that `App.vue` still routes through them.
 */

const dsh = { claude: false, codex: false, opencode: false, dsh: true }
const idle = { claude: false, codex: false, opencode: false, dsh: false }

test('a dsh entry is never dropped while a check is in flight', () => {
  // The exact reported symptom: a click that produces no reaction at all.
  assert.equal(entryBlockedWhileChecking('dsh', dsh), false)
})

test('a discovered frontend still refuses a second entry while checking', () => {
  // Unchanged behaviour: its entry awaits the check, so dropping the duplicate is
  // what keeps workspace preparation from running twice.
  assert.equal(entryBlockedWhileChecking('claude', { ...dsh, claude: true }), true)
  assert.equal(entryBlockedWhileChecking('codex', { ...dsh, codex: true }), true)
  assert.equal(entryBlockedWhileChecking('opencode', { ...dsh, opencode: true }), true)
})

test('nothing is blocked for any frontend once no check is running', () => {
  for (const kind of ['claude', 'codex', 'opencode', 'dsh']) {
    assert.equal(entryBlockedWhileChecking(kind, idle), false, kind)
  }
})

test('the dsh workspace mounts whatever the check says', () => {
  // Before the fix only 'ready' mounted the panel, so a `checking` dsh tab showed
  // the CLI gate instead of the runtime panel that carries its own states.
  assert.equal(cliWorkspaceCanMount('dsh', undefined), true)
  assert.equal(cliWorkspaceCanMount('dsh', 'checking'), true)
  assert.equal(cliWorkspaceCanMount('dsh', 'blocked'), true)
  assert.equal(cliWorkspaceCanMount('dsh', 'ready'), true)
})

test('a discovered workspace still waits for a ready check', () => {
  assert.equal(cliWorkspaceCanMount('claude', 'checking'), false)
  assert.equal(cliWorkspaceCanMount('claude', undefined), false)
  assert.equal(cliWorkspaceCanMount('claude', 'blocked'), false)
  assert.equal(cliWorkspaceCanMount('claude', 'ready'), true)
  // No frontend selected yet: nothing to mount.
  assert.equal(cliWorkspaceCanMount(null, 'ready'), false)
})

test('the CLI gate never covers the dsh tab', () => {
  assert.equal(cliAvailabilityGateApplies('dsh'), false)
  for (const kind of ['claude', 'codex', 'opencode']) {
    assert.equal(cliAvailabilityGateApplies(kind), true, kind)
  }
})

test('App.vue routes the entry, the mount and the gate through these rules', () => {
  const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
  const source = readFileSync(resolve(repoRoot, 'src/App.vue'), 'utf8')

  assert.match(source, /from '\.\/utils\/cliEntry'/)
  for (const helper of [
    'entryBlockedWhileChecking(kind, cliRuntimeStore.checking)',
    'cliWorkspaceCanMount(workspaceCliKind, workspaceCliStatus?.state)',
    'cliAvailabilityGateApplies(workspaceCliKind.value)',
  ]) {
    assert.ok(source.includes(helper), `App.vue no longer calls ${helper}`)
  }
})
