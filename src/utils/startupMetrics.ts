const STARTUP_PREFIX = 'agents-launcher:startup:'

export function markStartup(stage: string) {
  const name = `${STARTUP_PREFIX}${stage}`
  performance.clearMarks(name)
  performance.mark(name)
}

export function beginStartupMeasure(stage: string) {
  const startedAt = performance.now()
  markStartup(`${stage}:start`)
  let finished = false

  return () => {
    if (finished) return
    finished = true
    const endedAt = performance.now()
    markStartup(`${stage}:end`)
    const measureName = `${STARTUP_PREFIX}${stage}`
    performance.clearMeasures(measureName)
    performance.measure(measureName, {
      start: startedAt,
      end: endedAt,
    })
    if (import.meta.env.DEV) {
      console.debug(`[startup] ${stage}: ${(endedAt - startedAt).toFixed(1)} ms`)
    }
  }
}

export async function measureStartup<T>(stage: string, task: () => Promise<T>): Promise<T> {
  const finish = beginStartupMeasure(stage)
  try {
    return await task()
  } finally {
    finish()
  }
}
