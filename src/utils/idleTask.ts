interface IdleDeadlineLike {
  didTimeout: boolean
  timeRemaining: () => number
}

type IdleWindow = Window & {
  requestIdleCallback?: (
    callback: (deadline: IdleDeadlineLike) => void,
    options?: { timeout: number },
  ) => number
  cancelIdleCallback?: (handle: number) => void
}

export function scheduleIdleTask(task: () => void, timeout = 1500) {
  const idleWindow = window as IdleWindow
  if (idleWindow.requestIdleCallback && idleWindow.cancelIdleCallback) {
    const handle = idleWindow.requestIdleCallback(() => task(), { timeout })
    return () => idleWindow.cancelIdleCallback?.(handle)
  }
  const handle = window.setTimeout(task, timeout)
  return () => window.clearTimeout(handle)
}
