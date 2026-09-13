/**
 * Display rules for the port-occupancy remedy in the dsh configuration panel.
 *
 * These live outside the component so the condition that decides whether the
 * «一键清理占用» button exists can be tested directly — that condition is what
 * previously hid the button completely. The occupant sentence itself is built by
 * `describePortOccupant` in `@/utils/dshRuntime`, so there is exactly one
 * phrasing for it.
 */

export interface PortOccupancy {
  available: boolean
  occupant: string | null
  occupantIsDsh: boolean
  /** True when the occupant is the dsh this launcher supervises. */
  occupantIsSupervised: boolean
}

export interface PortCleanupVisibility {
  /** Latest probe result, or null when the port has not been probed yet. */
  portStatus: PortOccupancy | null
  /** True while the launcher-supervised dsh service is running. */
  isRunning: boolean
  /** True while a cleanup request is in flight. */
  releasing: boolean
}

/**
 * Offer cleanup whenever a known occupant blocks the port and the service is
 * not the one holding it.
 *
 * Deliberately independent of draft state: an occupied port blocks 启动 whether
 * or not the value was saved, and the hint already suggests the non-destructive
 * alternative (pick another port). A running managed service is excluded
 * because 「关闭」 is the correct action there.
 */
export function shouldOfferPortCleanup(visibility: PortCleanupVisibility): boolean {
  if (!visibility.portStatus) return false
  if (visibility.portStatus.available) return false
  if (visibility.isRunning) return false
  if (visibility.releasing) return false
  return true
}

/**
 * Consequence text shown next to the cleanup button.
 *
 * Only the consequence changes with the occupant; the identification is always
 * «当前占用进程为 …» — built by `describePortOccupant` in `@/utils/dshRuntime`,
 * which is also what the port help and the status banner use. The supervised
 * case is called out because it means the launcher started this process: the
 * runtime status simply lost track of it, most often after an app restart.
 */
export function describePortCleanupHint(
  occupancy: Pick<PortOccupancy, 'occupantIsDsh' | 'occupantIsSupervised'>,
): string {
  if (occupancy.occupantIsSupervised) {
    return '它由启动器启动，当前状态只是没跟踪到。清理会结束该进程；也可以改用其它端口。'
  }
  if (occupancy.occupantIsDsh) {
    return '这是一个 dsh 服务，可能就是你正在对话的那个。清理会立即中断该会话；也可以改用其它端口。'
  }
  return '将结束该进程（含其子进程），请先确认它不是你在用的程序；也可以改用其它端口。'
}
