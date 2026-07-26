import assert from 'node:assert/strict'
import test from 'node:test'
import {
  ClaudeStartupPromptCancelledError,
  canSubmitClaudeStartupPrompt,
  waitForClaudePromptReady,
} from '../src/utils/claudeStartupPrompt.ts'

test('startup prompt is accepted only by an available active idle observer', () => {
  assert.equal(canSubmitClaudeStartupPrompt(undefined), false)
  assert.equal(canSubmitClaudeStartupPrompt({
    available: true,
    active: true,
    sessionReady: false,
    runState: 'idle',
  }), false)
  assert.equal(canSubmitClaudeStartupPrompt({
    available: true,
    active: true,
    sessionReady: true,
    runState: 'working',
  }), false)
  assert.equal(canSubmitClaudeStartupPrompt({
    available: true,
    active: true,
    sessionReady: true,
    runState: 'idle',
  }), true)
  assert.equal(canSubmitClaudeStartupPrompt({
    available: true,
    active: true,
    sessionReady: true,
    runState: 'idle',
    terminalPrompt: { kind: 'workspaceTrust', path: String.raw`C:\Users\30919` },
  }), false)
})

test('startup readiness waits through launch and then resolves', async () => {
  let time = 0
  let refreshes = 0

  await waitForClaudePromptReady({
    refresh: async () => { refreshes += 1 },
    readState: () => ({
      available: true,
      active: true,
      sessionReady: refreshes >= 3,
      runState: refreshes >= 3 ? 'idle' : 'starting',
    }),
    timeoutMs: 1_000,
    intervalMs: 100,
    now: () => time,
    delay: async milliseconds => { time += milliseconds },
  })

  assert.equal(refreshes, 3)
})

test('workspace trust confirmation pauses the startup timeout', async () => {
  let time = 0
  let refreshes = 0

  await waitForClaudePromptReady({
    refresh: async () => { refreshes += 1 },
    readState: () => ({
      available: true,
      active: true,
      sessionReady: refreshes >= 6,
      runState: refreshes >= 6 ? 'idle' : 'starting',
      terminalPrompt: refreshes < 6
        ? { kind: 'workspaceTrust', path: String.raw`C:\Users\30919` }
        : undefined,
    }),
    timeoutMs: 200,
    intervalMs: 100,
    now: () => time,
    delay: async milliseconds => { time += milliseconds },
  })

  assert.equal(refreshes, 6)
  assert.equal(time, 500)
})

test('startup readiness surfaces permission and timeout failures', async () => {
  await assert.rejects(() => waitForClaudePromptReady({
    refresh: async () => {},
    readState: () => ({
      available: true,
      active: true,
      sessionReady: true,
      runState: 'permission',
    }),
  }), /终端确认/)

  let time = 0
  await assert.rejects(() => waitForClaudePromptReady({
    refresh: async () => {},
    readState: () => ({
      available: false,
      active: true,
      sessionReady: false,
      runState: 'starting',
      degradedReason: 'Hook 尚未就绪',
    }),
    timeoutMs: 200,
    intervalMs: 100,
    now: () => time,
    delay: async milliseconds => { time += milliseconds },
  }), /Hook 尚未就绪/)
})

test('startup readiness cancels before refreshing or submitting a stale target', async () => {
  let refreshes = 0

  await assert.rejects(() => waitForClaudePromptReady({
    refresh: async () => { refreshes += 1 },
    readState: () => ({
      available: true,
      active: true,
      sessionReady: true,
      runState: 'idle',
    }),
    isCancelled: () => true,
  }), ClaudeStartupPromptCancelledError)

  assert.equal(refreshes, 0)

  let cancelled = false
  await assert.rejects(() => waitForClaudePromptReady({
    refresh: async () => {
      refreshes += 1
      cancelled = true
    },
    readState: () => ({
      available: true,
      active: true,
      sessionReady: true,
      runState: 'idle',
    }),
    isCancelled: () => cancelled,
  }), ClaudeStartupPromptCancelledError)

  assert.equal(refreshes, 1)
})
