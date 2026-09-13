import { onBeforeUnmount, onScopeDispose, ref, watch, type Ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'

/**
 * Measure a placeholder element and drive the child WebView that renders the
 * dsh UI on top of it.
 *
 * The bounds are computed in the frontend (layout lives there) and applied in
 * Rust, which converts logical pixels to device pixels for the platform.
 *
 * A child WebView is a native control, not DOM. It is invisible to `v-show` and
 * survives unmounting, so it must be told explicitly when to go away — and,
 * just as important, it must not be told to come back while the panel is not
 * the visible surface. Three guards enforce that here:
 *
 * * `enabled` is the single source of truth for "this panel owns the surface".
 *   It is set from the panel's `active` prop, and while it is false nothing in
 *   this composable can call `dsh_embed_show` — not the ResizeObserver, not the
 *   window resize listener, not a pending animation frame. Everything else
 *   (`v-show`, the placeholder staying laid out with a non-zero rect, a resize
 *   during the tab switch) would otherwise re-show the control on top of the
 *   surface that just replaced it.
 * * `generation` invalidates in-flight show requests: an `invoke` that resolves
 *   after a `hide()` must not mark the embed visible again.
 * * **One show at a time.** `dsh_embed_show` builds a whole WebView2 environment
 *   on the application's event loop the first time it runs, which freezes the
 *   window for hundreds of milliseconds. Firing it several times in a row (the
 *   re-measure ladder, a resize storm, the service reaching `running`) queued
 *   several creations, each destroying the other's control — a frozen UI and,
 *   in the worst ordering, a live native control that no state referenced and
 *   therefore nothing could hide. Requests are now serialized and coalesced,
 *   and the ladder re-measures instead of re-showing.
 *
 * See §4.9 of `docs/dsh-integration-plan.md` for the overlay checklist.
 */

interface Bounds {
  x: number
  y: number
  width: number
  height: number
  [key: string]: number
}

const MIN_EMBED_SIZE = 2

/**
 * Re-measure delays after any layout change, in milliseconds.
 *
 * The list is deliberately front-loaded: the interesting corrections happen in
 * the first few frames, and the tail covers slower layout work (font loading,
 * the workspace's own transition, a first paint of the embedded page).
 */
const REASSERT_DELAYS_MS = [50, 120, 250, 500]

/**
 * How often to verify that the placeholder is still on screen while the control
 * is visible. Cheap (one `checkVisibility()` plus one rect read) and it is the
 * backstop for the whole class of "the dsh UI stayed on top of another surface"
 * bugs: CSS cannot hide a native control, so if anything removes the panel from
 * the layout without the gate above noticing, the control is hidden here.
 */
const WATCHDOG_MS = 500

/**
 * Delay before re-assert attempt `attempt`, or `undefined` when the ladder is
 * exhausted. Exported so the re-measure ladder itself is testable.
 */
export function reassertDelay(attempt: number): number | undefined {
  return REASSERT_DELAYS_MS[attempt]
}

/**
 * Injection seam for the Tauri bridge. It defaults to the real `invoke`; the
 * visibility regression test drives the composable with a recorder instead of
 * a live WebView.
 */
export interface DshEmbedOptions {
  invoke?: (command: string, args?: Record<string, unknown>) => Promise<unknown>
  /**
   * Append geometry to `<app data>/dsh/embed-debug.log` on every measurement.
   * Off by default; it exists to diagnose native-control placement on a real
   * window, where the DOM rect and the control's own geometry can diverge.
   */
  diagnostics?: boolean
}

/** True when the DOM says the element (and its ancestors) are displayed. */
export function isDisplayedElement(element: HTMLElement): boolean {
  // Only an explicit disconnect counts: the tests drive this with a stand-in
  // element that has no such property.
  if (element.isConnected === false) return false
  const check = (element as HTMLElement & {
    checkVisibility?: (options?: { checkVisibilityCSS?: boolean }) => boolean
  }).checkVisibility
  if (typeof check === 'function') {
    // `checkVisibilityCSS` covers `visibility: hidden` as well as `display: none`
    // and `content-visibility: hidden`. Opacity is deliberately *not* checked:
    // the panel fades in and out, and a fade is not an absence of the surface.
    return check.call(element, { checkVisibilityCSS: true })
  }
  // Engines without `checkVisibility`: a subtree that is not rendered has no box.
  const rect = element.getBoundingClientRect()
  return rect.width >= MIN_EMBED_SIZE && rect.height >= MIN_EMBED_SIZE
}

export function useDshEmbed(
  placeholder: Ref<HTMLElement | null>,
  options: DshEmbedOptions = {},
) {
  const call = options.invoke ?? ((command, args) => invoke(command, args))
  const logGeometry = options.diagnostics
    ? (line: string) => {
        void call('dsh_debug_log', { line }).catch(() => {})
      }
    : () => {}
  const visible = ref(false)
  const error = ref('')
  /** True only while this panel is the visible workspace surface. */
  const enabled = ref(false)
  let observer: ResizeObserver | null = null
  let frame = 0
  let lastApplied: string | null = null
  let reassertTimer: ReturnType<typeof setTimeout> | null = null
  let reassertAttempts = 0
  let watchdogTimer: ReturnType<typeof setInterval> | null = null
  /**
   * Bumped by every hide/close. A show request captures it before awaiting and
   * drops its result if the value moved, so a slow round-trip cannot resurrect
   * a control that was already hidden.
   */
  let generation = 0
  /** In-flight `dsh_embed_show`, if any. Never more than one. */
  let showRun: Promise<void> | null = null
  let showQueued = false
  let showQueuedForce = false

  function boundsKey(bounds: Bounds): string {
    return `${bounds.x.toFixed(1)}:${bounds.y.toFixed(1)}:${bounds.width.toFixed(1)}:${bounds.height.toFixed(1)}`
  }

  function measure(): Bounds | null {
    const element = placeholder.value
    if (!element) return null
    const rect = element.getBoundingClientRect()
    if (rect.width < MIN_EMBED_SIZE || rect.height < MIN_EMBED_SIZE) return null
    const bounds = { x: rect.left, y: rect.top, width: rect.width, height: rect.height }
    if (options.diagnostics) {
      const content = typeof document === 'undefined'
        ? null
        : document.querySelector('.app-content')?.getBoundingClientRect() ?? null
      logGeometry(
        `rect=${JSON.stringify(bounds)} dpr=${window.devicePixelRatio}`
        + ` inner=${window.innerWidth}x${window.innerHeight}`
        + ` content=${content ? JSON.stringify({ x: content.left, y: content.top, w: content.width, h: content.height }) : 'n/a'}`,
      )
    }
    return bounds
  }

  function cancelScheduled() {
    if (frame) {
      cancelAnimationFrame(frame)
      frame = 0
    }
  }

  async function runShow(force: boolean): Promise<void> {
    // The hard guard: never show a native control over a surface this panel no
    // longer owns.
    if (!enabled.value) return
    const bounds = measure()
    if (!bounds) {
      await hide()
      return
    }
    const key = boundsKey(bounds)
    if (!force && key === lastApplied && visible.value) return
    const requestGeneration = generation
    let visibleAfter: boolean | undefined
    try {
      const result = await call('dsh_embed_show', bounds) as { visible?: boolean } | undefined
      visibleAfter = result?.visible
    } catch (cause) {
      error.value = String(cause)
      visible.value = false
      return
    }
    if (requestGeneration !== generation || !enabled.value) {
      // A hide landed while this request was in flight, so the native control is
      // now visible even though it must not be. Re-assert the hidden state
      // instead of trusting the call ordering.
      await hide()
      return
    }
    lastApplied = key
    // The Rust side reports whether the control actually ended up on screen: it
    // refuses to show one whose intent was revoked while the creation ran, and
    // this side must not claim otherwise.
    visible.value = visibleAfter !== false
    error.value = ''
  }

  /**
   * Ask for the control to cover the placeholder, at most one native call at a
   * time. A request that arrives while one is in flight collapses into a single
   * follow-up, which is enough: the follow-up re-measures and no-ops if nothing
   * moved.
   */
  function apply(force = false): Promise<void> {
    if (showRun) {
      showQueued = true
      showQueuedForce = showQueuedForce || force
      return showRun
    }
    showRun = runShow(force).finally(() => {
      showRun = null
      if (showQueued) {
        showQueued = false
        const queued = showQueuedForce
        showQueuedForce = false
        void apply(queued)
      }
    })
    return showRun
  }

  /** Coalesce resize storms into one apply per animation frame. */
  function scheduleApply() {
    if (frame || !enabled.value) return
    frame = requestAnimationFrame(() => {
      frame = 0
      void apply()
      armReassert()
    })
  }

  /**
   * Window resize / maximize / fullscreen: the layout often keeps changing for
   * a few frames after the event, so re-assert instead of trusting one read.
   */
  function scheduleResize() {
    if (!enabled.value) return
    lastApplied = null
    scheduleApply()
  }

  /**
   * Keep re-measuring until the geometry stops changing.
   *
   * A single measurement is not trustworthy: this composable runs while the
   * workspace is still settling (tab switch, panel mount, transition), and a
   * rect read mid-transition reports a partial height. The native control was
   * then created too short and stayed that way — the ResizeObserver only fires
   * on *changes*, so a rect that is already final by the next frame produces no
   * further callback. The visible symptom was the dsh page occupying only the
   * bottom of the content area with the app background above it.
   *
   * The ladder only asks for a new show when the geometry actually moved (or the
   * previous show never landed), so confirming a measurement costs one rect read
   * — not another native call.
   */
  function armReassert() {
    if (!enabled.value) return
    reassertAttempts = 0
    scheduleReassert()
  }

  function scheduleReassert() {
    if (!enabled.value || reassertTimer !== null) return
    const delay = reassertDelay(reassertAttempts)
    if (delay === undefined) return
    reassertTimer = setTimeout(() => {
      reassertTimer = null
      reassertAttempts += 1
      if (!enabled.value) return
      const bounds = measure()
      if (bounds && (!visible.value || boundsKey(bounds) !== lastApplied)) void apply()
      // Keep going while the geometry is still settling.
      scheduleReassert()
    }, delay)
  }
  function cancelReassert() {
    if (reassertTimer !== null) {
      clearTimeout(reassertTimer)
      reassertTimer = null
    }
    reassertAttempts = 0
  }

  /**
   * Verify that the placeholder is still part of the rendered surface.
   *
   * The native control cannot be affected by CSS, so a panel that leaves the
   * layout by any route the `enabled` gate does not cover would leave the dsh UI
   * painted over the surface that replaced it. This is that backstop.
   */
  function watchdogTick() {
    if (!visible.value) return
    const element = placeholder.value
    if (!enabled.value || !element || !isDisplayedElement(element)) void hide()
  }

  function startWatchdog() {
    if (watchdogTimer !== null) return
    watchdogTimer = setInterval(watchdogTick, WATCHDOG_MS)
  }

  function stopWatchdog() {
    if (watchdogTimer !== null) {
      clearInterval(watchdogTimer)
      watchdogTimer = null
    }
  }

  /** Called from the panel's `active` watcher. */
  function setEnabled(next: boolean) {
    if (enabled.value === next) return
    enabled.value = next
    if (next) {
      lastApplied = null
      scheduleApply()
    } else {
      cancelScheduled()
      cancelReassert()
      void hide()
    }
  }

  /**
   * Hide now, and — when the reason is a transient overlay — stay hidden until
   * the overlay closes. Without the `enabled` half, a resize while an overlay
   * is open would re-show the control straight over it.
   */
  async function suspend() {
    if (enabled.value) {
      enabled.value = false
      cancelScheduled()
      cancelReassert()
    }
    await hide()
  }

  /** Re-show after {@link suspend}, only if the panel still owns the surface. */
  function resume(shouldResume: boolean) {
    if (!shouldResume || enabled.value) return
    enabled.value = true
    lastApplied = null
    scheduleApply()
  }

  async function hide() {
    generation += 1
    cancelScheduled()
    cancelReassert()
    lastApplied = null
    visible.value = false
    try {
      // Deliberately unconditional: the Rust side is the authority on whether a
      // native control exists, and this composable may have been remounted since
      // it was created. `dsh_embed_hide` is idempotent and cheap, so trading one
      // wasted call for never leaving a control on screen is the right trade.
      // It is also serialized against creation in Rust, so a hide issued while a
      // show is still building the control wins instead of racing it.
      await call('dsh_embed_hide')
    } catch {
      // Hiding is best-effort: the panel is going away anyway.
    }
  }

  async function close() {
    generation += 1
    cancelScheduled()
    cancelReassert()
    lastApplied = null
    visible.value = false
    try {
      await call('dsh_embed_close')
    } catch {
      // Nothing actionable at teardown time.
    }
  }

  async function reload() {
    lastApplied = null
    try {
      await call('dsh_embed_reload')
    } catch (cause) {
      error.value = String(cause)
    }
    await apply(true)
  }

  /** Re-measure after the element moves without resizing (tab switches, etc.). */
  function refresh() {
    if (!enabled.value) return
    lastApplied = null
    scheduleApply()
  }

  /**
   * Re-assert the geometry now and over the next few frames. Exposed for
   * window-resize / fullscreen handling, where the layout may settle after the
   * event that announced it.
   */
  function reassert() {
    if (!enabled.value) return
    lastApplied = null
    void apply(true)
    armReassert()
  }

  watch(visible, (isVisible) => {
    if (isVisible) startWatchdog()
    else stopWatchdog()
  }, { immediate: true })

  // ResizeObserver callbacks are gated on `enabled` inside `scheduleApply`, so a
  // panel kept alive by `v-show` cannot re-show the control from a layout change.
  watch(placeholder, (element) => {
    observer?.disconnect()
    observer = null
    // The listener is re-registered with the new element, so drop the previous
    // one first: without this, every placeholder swap leaked another listener.
    window.removeEventListener('resize', scheduleResize)
    if (!element) return
    observer = new ResizeObserver(scheduleApply)
    observer.observe(element)
    window.addEventListener('resize', scheduleResize)
    scheduleApply()
  }, { immediate: true })

  let torn = false
  function teardown() {
    stopWatchdog()
    if (torn) return
    torn = true
    // A native control ignores unmounting: hide it explicitly so it cannot
    // outlive the panel and cover the next surface.
    enabled.value = false
    void hide()
    observer?.disconnect()
    observer = null
    window.removeEventListener('resize', scheduleResize)
    cancelScheduled()
    cancelReassert()
  }

  onBeforeUnmount(teardown)
  // `onBeforeUnmount` only fires for a component; the tests drive this composable
  // inside a bare effect scope, where a scope dispose is what actually runs.
  onScopeDispose(teardown)

  return {
    visible,
    error,
    enabled,
    setEnabled,
    suspend,
    resume,
    apply,
    hide,
    close,
    reload,
    refresh,
    reassert,
  }
}
