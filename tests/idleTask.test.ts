import test from 'node:test'
import assert from 'node:assert/strict'
import { scheduleIdleTask } from '../src/utils/idleTask.ts'

test('idle task uses requestIdleCallback when available', () => {
  let callback: (() => void) | undefined
  let cancelled: number | undefined
  const testWindow = {
    requestIdleCallback(next: () => void) {
      callback = next
      return 7
    },
    cancelIdleCallback(handle: number) {
      cancelled = handle
    },
  }
  ;(globalThis as { window?: unknown }).window = testWindow
  let calls = 0

  const cancel = scheduleIdleTask(() => {
    calls += 1
  })
  callback?.()
  cancel()

  assert.equal(calls, 1)
  assert.equal(cancelled, 7)
})

test('idle task fallback can be cancelled', async () => {
  const testWindow = {
    setTimeout,
    clearTimeout,
  }
  ;(globalThis as { window?: unknown }).window = testWindow
  let called = false

  const cancel = scheduleIdleTask(() => {
    called = true
  }, 5)
  cancel()
  await new Promise(resolve => setTimeout(resolve, 15))

  assert.equal(called, false)
})

