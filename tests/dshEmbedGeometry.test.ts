import test from 'node:test'
import assert from 'node:assert/strict'
import { effectScope, nextTick, ref } from 'vue'
import { reassertDelay, useDshEmbed } from '../src/components/dsh/useDshEmbed.ts'

/**
 * Regression tests for "the dsh page only covers the bottom half of the tab".
 *
 * Cause: the geometry was measured once, while the workspace was still settling.
 * The rect read mid-transition reported a partial height, the native control was
 * created that short, and nothing corrected it — a ResizeObserver only fires on
 * *changes*, so a rect that is already final by the next frame produced no
 * further callback.
 *
 * Fix: after any layout change, keep re-applying the geometry over a short
 * ladder of delays until it stops changing.
 */

interface InvokeCall {
  command: string
  args: { width?: number; height?: number; y?: number } | undefined
}

const realSetTimeout = globalThis.setTimeout

async function withHarness(
  rects: Array<{ left: number; top: number; width: number; height: number }>,
  run: (harness: {
    embed: ReturnType<typeof useDshEmbed>
    calls: InvokeCall[]
    setEnabled: (next: boolean) => void
  }) => Promise<void>,
) {
  const calls: InvokeCall[] = []
  const frames: Array<() => void> = []
  let rectIndex = 0

  globalThis.requestAnimationFrame = ((callback: () => void) => {
    frames.push(callback)
    return frames.length
  }) as unknown as typeof globalThis.requestAnimationFrame
  globalThis.cancelAnimationFrame = (() => {}) as typeof globalThis.cancelAnimationFrame
  globalThis.ResizeObserver = class {
    observe() {}
    disconnect() {}
  } as unknown as typeof globalThis.ResizeObserver
  globalThis.window = {
    devicePixelRatio: 2,
    innerWidth: 1920,
    innerHeight: 1044,
    addEventListener: () => {},
    removeEventListener: () => {},
  } as unknown as typeof globalThis.window

  const element = {
    // Each read advances through the script so "the layout settles later" is
    // actually simulated; the last entry repeats forever.
    getBoundingClientRect: () => {
      const rect = rects[Math.min(rectIndex, rects.length - 1)]
      rectIndex += 1
      return rect
    },
  } as unknown as HTMLElement
  const placeholder = ref<HTMLElement | null>(element)
  const scope = effectScope()
  let embed!: ReturnType<typeof useDshEmbed>
  scope.run(() => {
    embed = useDshEmbed(placeholder, {
      invoke: async (command, args) => {
        calls.push({ command, args: args as InvokeCall['args'] })
      },
    })
  })
  await nextTick()

  const flushFrames = () => {
    for (const callback of frames.splice(0, frames.length)) callback()
  }

  try {
    await run({
      embed,
      calls,
      setEnabled: (next) => {
        embed.setEnabled(next)
        flushFrames()
      },
    })
  } finally {
    scope.stop()
    globalThis.ResizeObserver = undefined as unknown as typeof globalThis.ResizeObserver
    globalThis.window = undefined as unknown as typeof globalThis.window
  }
}

/** Wait for the ladder, using real timers with a bounded budget. */
async function settleLadder(totalMs: number) {
  const step = 25
  for (let waited = 0; waited < totalMs; waited += step) {
    await new Promise((resolve) => realSetTimeout(resolve, step))
  }
}

test('the re-measure ladder is front-loaded and finite', () => {
  assert.equal(reassertDelay(0), 50)
  assert.equal(reassertDelay(1), 120)
  assert.ok(reassertDelay(2)! > 0)
  assert.ok(reassertDelay(3)! > 0)
  assert.equal(reassertDelay(4), undefined, 'the ladder must end')
})

test('a late layout change is picked up after the first measurement', async () => {
  // First read happens mid-transition (short), the next reads are still short,
  // then the layout settles. The ladder must keep measuring and catch up.
  const settled = { left: 0, top: 38, width: 1920, height: 1006 }
  const midway = { left: 0, top: 38, width: 1920, height: 265 }

  await withHarness([midway, midway, settled], async ({ embed, calls, setEnabled }) => {
    setEnabled(true)
    assert.equal(calls.length, 1, 'the control is shown from the first measurement')
    assert.equal(calls[0].args?.height, 265, 'first read is the unsettled height')

    // The ladder keeps re-measuring; once the rect settles it must re-apply.
    await settleLadder(1000)

    const applied = calls.filter((call) => call.command === 'dsh_embed_show')
    assert.ok(applied.length > 1, 'the ladder must re-apply, not stop after one read')
    const last = applied[applied.length - 1]
    assert.equal(
      last.args?.height,
      1006,
      'the settled height must reach the native control',
    )
    assert.equal(last.args?.y, 38)
    assert.equal(last.args?.width, 1920)
  })
})

test('the ladder stops once the geometry is stable', async () => {
  const settled = { left: 0, top: 38, width: 1920, height: 1006 }

  await withHarness([settled], async ({ embed, calls, setEnabled }) => {
    setEnabled(true)
    await settleLadder(1000)

    const applied = calls.filter((call) => call.command === 'dsh_embed_show')
    assert.equal(
      applied.length,
      1,
      'an unchanged geometry must not be re-sent on every ladder step',
    )
    assert.equal(embed.visible.value, true)
  })
})

test('the ladder is abandoned when the panel stops being the surface', async () => {
  const midway = { left: 0, top: 38, width: 1920, height: 265 }
  const settled = { left: 0, top: 38, width: 1920, height: 1006 }

  await withHarness([midway, settled], async ({ embed, calls, setEnabled }) => {
    setEnabled(true)
    setEnabled(false)

    await settleLadder(1000)

    const afterHiding = calls.filter((call) => call.command === 'dsh_embed_show').length
    assert.equal(afterHiding, 1, 'no show may be sent after the panel was left')
    assert.equal(embed.enabled.value, false)
    assert.equal(calls[calls.length - 1].command, 'dsh_embed_hide')
  })
})

test('reassert re-applies immediately and then confirms over the ladder', async () => {
  const settled = { left: 0, top: 38, width: 1920, height: 1006 }

  await withHarness([settled], async ({ embed, calls, setEnabled }) => {
    setEnabled(true)
    const before = calls.filter((call) => call.command === 'dsh_embed_show').length

    // Window resize / fullscreen: the layout may settle over the next frames.
    embed.reassert()
    await settleLadder(120)

    const after = calls.filter((call) => call.command === 'dsh_embed_show').length
    assert.ok(after > before, 'reassert must force a re-apply even when unchanged')
  })
})
