import test from 'node:test'
import assert from 'node:assert/strict'
import { effectScope, nextTick, ref } from 'vue'
import { useDshEmbed } from '../src/components/dsh/useDshEmbed.ts'

/**
 * Regression tests for the bug where the dsh child WebView stayed on top after
 * switching to another surface (the dsh config page, another frontend tab).
 *
 * A child WebView is a native control: `v-show` and unmounting do not affect
 * it. The only thing keeping it off screen is `useDshEmbed` sending
 * `dsh_embed_hide`, so these tests assert the invariant against the real
 * composable with a recorded bridge:
 *
 *   while the panel is not the visible surface, `dsh_embed_show` is never sent,
 *   whatever the ResizeObserver or a window resize does afterwards.
 */

interface InvokeCall {
  command: string
  args: unknown
}

interface Harness {
  embed: ReturnType<typeof useDshEmbed>
  calls: InvokeCall[]
  /** Queue an animation frame; nothing runs until `flushFrames`. */
  flushFrames: () => void
  /** Fire every registered ResizeObserver callback. */
  notifyObservers: () => void
  /** Fire every registered window `resize` listener. */
  emitWindowResize: () => void
  stop: () => void
}

const RECT = { left: 10, top: 40, width: 800, height: 600 }

/** Install the browser globals the composable needs and return the hooks. */
function installGlobals(options: { checkVisibility?: () => boolean } = {}) {
  const frames: Array<() => void> = []
  const observers: Array<() => void> = []
  const listeners: Array<() => void> = []

  globalThis.requestAnimationFrame = ((callback: () => void) => {
    frames.push(callback)
    return frames.length
  }) as unknown as typeof globalThis.requestAnimationFrame
  globalThis.cancelAnimationFrame = (() => {}) as typeof globalThis.cancelAnimationFrame
  globalThis.ResizeObserver = class {
    callback: () => void
    constructor(callback: () => void) {
      this.callback = callback
      observers.push(() => this.callback())
    }
    observe() {}
    disconnect() {}
  } as unknown as typeof globalThis.ResizeObserver
  globalThis.window = {
    devicePixelRatio: 1,
    innerWidth: 1024,
    innerHeight: 768,
    addEventListener: (type: string, callback: () => void) => {
      if (type === 'resize') listeners.push(callback)
    },
    removeEventListener: () => {},
  } as unknown as typeof globalThis.window

  const element = {
    isConnected: true,
    getBoundingClientRect: () => RECT,
    ...(options.checkVisibility ? { checkVisibility: options.checkVisibility } : {}),
  } as unknown as HTMLElement

  return {
    element,
    flushFrames: () => {
      for (const callback of frames.splice(0, frames.length)) callback()
    },
    notifyObservers: () => {
      for (const notify of observers) notify()
    },
    emitWindowResize: () => {
      for (const callback of listeners) callback()
    },
  }
}

function createHarness(): Harness {
  const calls: InvokeCall[] = []
  const globals = installGlobals({ checkVisibility: () => true })

  const placeholder = ref<HTMLElement | null>(globals.element)
  const scope = effectScope()
  let embed!: ReturnType<typeof useDshEmbed>
  scope.run(() => {
    embed = useDshEmbed(placeholder, {
      invoke: async (command, args) => {
        calls.push({ command, args })
      },
    })
  })

  return {
    embed,
    calls,
    flushFrames: globals.flushFrames,
    notifyObservers: globals.notifyObservers,
    emitWindowResize: globals.emitWindowResize,
    stop: () => scope.stop(),
  }
}

/** Let every queued microtask (the recorded async invokes) settle. */
async function settle() {
  await nextTick()
  for (let index = 0; index < 6; index += 1) await Promise.resolve()
}

function commands(calls: InvokeCall[]) {
  return calls.map((call) => call.command)
}

test('dsh embed never re-shows itself after the panel stops being active', async () => {
  const harness = createHarness()
  const { embed, calls } = harness
  try {
    await settle()

    // Service running + tab active: the control is shown exactly once.
    embed.setEnabled(true)
    harness.flushFrames()
    await settle()
    assert.deepEqual(commands(calls), ['dsh_embed_show'])
    assert.equal(embed.enabled.value, true)

    // Switch to another surface (config page / another frontend tab).
    embed.setEnabled(false)
    await settle()
    assert.ok(
      commands(calls).includes('dsh_embed_hide'),
      'leaving the tab must hide the native control',
    )
    const beforeNoise = calls.length

    // Every path that used to resurrect the control must now be inert.
    harness.emitWindowResize()
    harness.notifyObservers()
    harness.flushFrames()
    embed.refresh()
    harness.flushFrames()
    await settle()

    assert.deepEqual(
      commands(calls.slice(beforeNoise)).filter((command) => command === 'dsh_embed_show'),
      [],
      'no show may be sent while the panel is not the visible surface',
    )
  } finally {
    harness.stop()
  }
})

test('dsh embed hides while an overlay is open and restores afterwards', async () => {
  const harness = createHarness()
  const { embed, calls } = harness
  try {
    await settle()
    embed.setEnabled(true)
    harness.flushFrames()
    await settle()
    assert.deepEqual(commands(calls), ['dsh_embed_show'])

    // The global settings popover opens: the control must step aside.
    await embed.suspend()
    assert.equal(embed.enabled.value, false)
    const hiddenAt = calls.length

    // A resize while the overlay is open must not bring it back over the popover.
    harness.flushFrames()
    embed.refresh()
    harness.flushFrames()
    harness.emitWindowResize()
    harness.flushFrames()
    await settle()
    assert.deepEqual(
      commands(calls.slice(hiddenAt)).filter((command) => command === 'dsh_embed_show'),
      [],
      'an open overlay must keep the control hidden',
    )

    // Closing the overlay restores the control exactly once.
    embed.resume(true)
    harness.flushFrames()
    await settle()
    assert.equal(
      commands(calls.slice(hiddenAt)).filter((command) => command === 'dsh_embed_show').length,
      1,
      'closing the overlay should restore the control',
    )
  } finally {
    harness.stop()
  }
})

test('a hide that lands during an in-flight show wins', async () => {
  const calls: InvokeCall[] = []
  const frames: Array<() => void> = []
  let releaseShow: (() => void) | null = null
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
    addEventListener: () => {},
    removeEventListener: () => {},
  } as unknown as typeof globalThis.window

  const placeholder = ref<HTMLElement | null>({
    getBoundingClientRect: () => RECT,
  } as unknown as HTMLElement)
  const scope = effectScope()
  let embed!: ReturnType<typeof useDshEmbed>
  scope.run(() => {
    embed = useDshEmbed(placeholder, {
      invoke: async (command) => {
        calls.push({ command, args: undefined })
        if (command === 'dsh_embed_show') {
          // Simulate a slow round-trip that resolves after the tab switch.
          await new Promise<void>((resolve) => {
            releaseShow = resolve
          })
        }
      },
    })
  })

  try {
    await settle()
    embed.setEnabled(true)
    for (const callback of frames.splice(0, frames.length)) callback()
    await Promise.resolve()

    // The user switches away while `dsh_embed_show` is still in flight.
    const hiding = embed.hide()
    releaseShow?.()
    await hiding
    await settle()

    assert.equal(embed.visible.value, false)
    assert.ok(
      commands(calls).filter((command) => command === 'dsh_embed_hide').length >= 2,
      'the stale show must be followed by a hide',
    )
    assert.equal(calls[calls.length - 1].command, 'dsh_embed_hide')
  } finally {
    scope.stop()
  }
})

test('a burst of requests never starts a second native show', async () => {
  const calls: InvokeCall[] = []
  const pendingShows: Array<() => void> = []
  let inFlight = 0
  let maxInFlight = 0
  const globals = installGlobals({ checkVisibility: () => true })

  const placeholder = ref<HTMLElement | null>(globals.element)
  const scope = effectScope()
  let embed!: ReturnType<typeof useDshEmbed>
  scope.run(() => {
    embed = useDshEmbed(placeholder, {
      invoke: async (command) => {
        calls.push({ command, args: undefined })
        if (command !== 'dsh_embed_show') return
        inFlight += 1
        maxInFlight = Math.max(maxInFlight, inFlight)
        // Simulate a creation that takes a while: building a WebView2
        // environment blocks the application's event loop for hundreds of ms.
        await new Promise<void>((resolve) => pendingShows.push(resolve))
        inFlight -= 1
      },
    })
  })

  try {
    await settle()
    embed.setEnabled(true)
    globals.flushFrames()
    await Promise.resolve()
    assert.equal(pendingShows.length, 1, 'the panel asks for the control once')

    // Every path that used to fire its own show while the first one was still
    // building the native control: a resize storm, the ResizeObserver, refresh.
    globals.notifyObservers()
    globals.flushFrames()
    globals.emitWindowResize()
    globals.flushFrames()
    embed.refresh()
    globals.flushFrames()
    await Promise.resolve()
    assert.equal(
      pendingShows.length,
      1,
      'concurrent requests must collapse instead of queueing more creations',
    )
    assert.equal(maxInFlight, 1, 'never two native calls at once')

    // Land it: the coalesced follow-up re-measures, finds the same geometry and
    // therefore issues nothing at all.
    pendingShows[0]()
    await settle()
    assert.equal(pendingShows.length, 1, 'an unchanged geometry needs no second call')
    assert.equal(embed.visible.value, true)
  } finally {
    scope.stop()
  }
})

test('the watchdog hides the control when the panel leaves the layout', async () => {
  let displayed = true
  const calls: InvokeCall[] = []
  const globals = installGlobals({ checkVisibility: () => displayed })

  const placeholder = ref<HTMLElement | null>(globals.element)
  const scope = effectScope()
  let embed!: ReturnType<typeof useDshEmbed>
  scope.run(() => {
    embed = useDshEmbed(placeholder, {
      invoke: async (command) => {
        calls.push({ command, args: undefined })
      },
    })
  })

  const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

  try {
    await settle()
    embed.setEnabled(true)
    globals.flushFrames()
    await settle()
    assert.equal(embed.visible.value, true)
    assert.equal(calls.length, 1)

    // A rendered panel must not be hidden by the watchdog.
    await wait(700)
    assert.deepEqual(
      commands(calls).filter((command) => command === 'dsh_embed_hide'),
      [],
      'a displayed panel keeps the control on screen',
    )

    // The panel leaves the layout without the `enabled` gate noticing (any
    // `display: none` ancestor does this). CSS cannot hide a native control, so
    // the watchdog has to.
    displayed = false
    await wait(700)
    assert.equal(embed.visible.value, false)
    assert.equal(calls[calls.length - 1].command, 'dsh_embed_hide')
  } finally {
    scope.stop()
  }
})
