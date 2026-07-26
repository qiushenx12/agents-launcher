import type { ClaudeConversationState } from '@/types/claudeObserver'

type PromptState = Pick<
  ClaudeConversationState,
  'available' | 'active' | 'sessionReady' | 'runState' | 'degradedReason' | 'terminalPrompt'
>

export class ClaudeStartupPromptCancelledError extends Error {
  constructor() {
    super('Claude 启动发送已取消')
    this.name = 'ClaudeStartupPromptCancelledError'
  }
}

export interface WaitForClaudePromptReadyOptions {
  refresh: () => Promise<void>
  readState: () => PromptState | undefined
  timeoutMs?: number
  intervalMs?: number
  now?: () => number
  delay?: (milliseconds: number) => Promise<void>
  isCancelled?: () => boolean
}

export function canSubmitClaudeStartupPrompt(state: PromptState | undefined) {
  return !!state?.available
    && state.active
    && state.sessionReady
    && state.runState === 'idle'
    && !state.terminalPrompt
}

export async function waitForClaudePromptReady({
  refresh,
  readState,
  timeoutMs = 20_000,
  intervalMs = 350,
  now = Date.now,
  delay = (milliseconds) => new Promise<void>(resolve => globalThis.setTimeout(resolve, milliseconds)),
  isCancelled = () => false,
}: WaitForClaudePromptReadyOptions) {
  let deadline = now() + timeoutMs
  let lastReason = ''

  while (true) {
    if (now() >= deadline) break
    if (isCancelled()) throw new ClaudeStartupPromptCancelledError()
    await refresh()
    if (isCancelled()) throw new ClaudeStartupPromptCancelledError()
    const state = readState()
    if (canSubmitClaudeStartupPrompt(state)) return
    if (state?.runState === 'permission') {
      throw new Error('Claude 正在等待终端确认，请先在终端中完成授权')
    }
    if (state?.runState === 'stopped' || state?.active === false) {
      throw new Error('Claude 会话启动失败或已经结束')
    }
    lastReason = state?.degradedReason ?? lastReason
    await delay(intervalMs)
    if (state?.terminalPrompt) deadline = now() + timeoutMs
  }

  if (isCancelled()) throw new ClaudeStartupPromptCancelledError()
  throw new Error(lastReason || 'Claude 会话启动超时，请打开终端查看启动状态')
}
