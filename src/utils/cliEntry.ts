/**
 * Pure predicates for "can this frontend be entered right now?".
 *
 * Three places in `App.vue` have to agree on one invariant:
 *
 *   **The dsh entry path is decoupled from the CLI availability check.**
 *
 * dsh reaches the workspace through a supervised `dsh web` service and its own
 * browser UI, not through a discovered project/session workspace. The check for
 * it is an `npx` probe: it resolves the package over the network and routinely
 * takes tens of seconds, while the answer cannot gate anything — when the
 * service is down the runtime panel offers to start it, and when it is up the
 * backend answers from the live process and skips the probe entirely.
 *
 * Coupling the entry to that probe produced the reported bug: clicking
 * 「进入DeepSeek Harness」— or the 项目 tab and the dsh entry next to it — did
 * nothing at all for as long as a probe happened to be in flight, because the
 * click was dropped and the dsh panel was not mounted; once the probe settled,
 * the same click worked ("过了比较久之后可以进入").
 *
 * The rules live here, instead of inline in `App.vue`, so the invariant can be
 * stated once and covered by `tests/cliEntry.test.ts`. They are deliberately
 * free of imports so `node --test` can load this module directly.
 */

/** The one CLI whose workspace is not discovered, and therefore not gated. */
const UNGATED_KIND = 'dsh'

/**
 * True when a workspace entry request must be ignored because a check is
 * already running.
 *
 * Only for the discovered frontends: their entry does await the check, so a
 * second click while it runs would duplicate workspace preparation. dsh must
 * never be dropped this way — it switches the tab immediately and reads
 * whatever the check has cached, so a silent drop is pure loss.
 */
export function entryBlockedWhileChecking(
  kind: string,
  checking: Record<string, boolean | undefined>,
): boolean {
  return kind !== UNGATED_KIND && checking[kind] === true
}

/**
 * True when the shared project workspace panel may be mounted for this
 * frontend, given its CLI status.
 *
 * The discovered frontends only have a workspace once their check says
 * `ready` — before that the CLI gate covers the content area instead. dsh has
 * no workspace to discover, and its runtime panel renders its own
 * empty/starting/failed states, so it mounts unconditionally.
 */
export function cliWorkspaceCanMount(kind: string | null, state: string | null | undefined): boolean {
  if (!kind) return false
  return kind === UNGATED_KIND || state === 'ready'
}

/**
 * True when the CLI availability gate applies to this frontend.
 *
 * The gate reports a missing or unusable executable and offers a re-check. For
 * dsh that verdict is never the whole story (the service can be running, or
 * startable, regardless of what an `npx` probe says), and covering its tab
 * with the gate is what made a click look like it did nothing.
 */
export function cliAvailabilityGateApplies(kind: string | null | undefined): boolean {
  return kind !== UNGATED_KIND
}
