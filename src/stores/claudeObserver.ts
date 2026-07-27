import { defineStore } from 'pinia'
import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import {
  claudeModelContextFromText,
  claudeObservedModelSwitchApplied,
  normalizeClaudeModelDisplayName,
  parseClaudeModelCommandResult,
  reduceClaudeAgentEvents,
} from '@/utils/claudeObserverEvents'
import { encodeClaudeConversationInputWrites } from '@/utils/claudeConversationInput'
import { validateClaudeSlashCommand } from '@/utils/claudeSlashCommands'
import {
  CLAUDE_NATIVE_QUEUE_RECALL_WRITES,
  CLAUDE_PROMPT_QUEUE_LIMIT,
  createClaudeQueuedPrompt,
  findMatchingClaudeNativePrompt,
  normalizeClaudeBusyInputMode,
  normalizeClaudePromptForMatch,
  takeMatchingClaudePromptReceipt,
} from '@/utils/claudePromptQueue'
import {
  encodeClaudeQuestionAnswerWrites,
  type ClaudeAskUserQuestion,
  type ClaudeQuestionAnswer,
} from '@/utils/claudeQuestion'
import { ClaudeObserverSnapshotGate } from '@/utils/claudeObserverSnapshotGate'
import type { PtyStatus, PtyTitle } from '@/types/terminal'
import type {
  ClaudeAgentEvent,
  ClaudeBusyInputMode,
  ClaudeConversationState,
  ClaudeModelContext,
  ClaudeObserverSnapshot,
  ClaudeObserverStatus,
  ClaudePromptReceipt,
  ClaudePromptSubmissionBaseline,
  ClaudeQueuedPrompt,
  ClaudeTerminalLogResult,
  ClaudeTerminalPrompt,
} from '@/types/claudeObserver'

export type ClaudeDefaultPermissionMode =
  | 'bypassPermissions'
  | 'auto'
  | 'default'
  | 'acceptEdits'
  | 'plan'

function normalizeDefaultPermissionMode(mode: string | null | undefined): ClaudeDefaultPermissionMode {
  switch (mode) {
    case 'bypassPermissions':
    case 'auto':
    case 'default':
    case 'acceptEdits':
    case 'plan':
      return mode
    case 'manual':
      return 'default'
    default:
      return 'auto'
  }
}

function permissionModeFromTerminalLabel(label: string | null | undefined): ClaudeDefaultPermissionMode | null {
  const normalized = label?.toLowerCase() ?? ''
  if (normalized.includes('bypass permissions')) return 'bypassPermissions'
  if (normalized.includes('auto mode')) return 'auto'
  if (normalized.includes('manual mode') || normalized.includes('default mode')) return 'default'
  if (normalized.includes('auto-accept edits') || normalized.includes('accept edits')) return 'acceptEdits'
  if (normalized.includes('plan mode')) return 'plan'
  return null
}

function normalizeClaudeTerminalPrompt(
  prompt: ClaudeTerminalPrompt | null | undefined,
): ClaudeTerminalPrompt | undefined {
  if (!prompt) return undefined

  if (prompt.kind === 'pluginInstall') {
    const legacy = prompt as unknown as { plugin_name?: unknown }
    const pluginName = typeof prompt.pluginName === 'string'
      ? prompt.pluginName
      : typeof legacy.plugin_name === 'string'
        ? legacy.plugin_name
        : ''
    return { ...prompt, pluginName }
  }

  if (prompt.kind === 'modelSwitchConfirm') {
    const legacy = prompt as unknown as { selected_index?: unknown }
    const selectedIndex = Number.isInteger(prompt.selectedIndex)
      ? prompt.selectedIndex
      : Number.isInteger(legacy.selected_index)
        ? legacy.selected_index as number
        : -1
    if (selectedIndex < 0 || selectedIndex >= prompt.options.length) return undefined
    return { ...prompt, selectedIndex }
  }

  if (prompt.kind === 'planApproval') {
    const legacy = prompt as unknown as { selected_index?: unknown }
    const selectedIndex = Number.isInteger(prompt.selectedIndex)
      ? prompt.selectedIndex
      : Number.isInteger(legacy.selected_index)
        ? legacy.selected_index as number
        : -1
    if (selectedIndex < 0 || selectedIndex >= prompt.options.length) return undefined
    return { ...prompt, selectedIndex }
  }

  return prompt
}

function createState(tabId: number): ClaudeConversationState {
  return {
    tabId,
    statusRevision: 0,
    available: false,
    active: true,
    sessionReady: false,
    runState: 'starting',
    items: [],
    terminalLog: '',
    loading: true,
    terminalPrompt: undefined,
    activityStatus: undefined,
    compactCompletionRevision: 0,
    permissionMode: '? for shortcuts',
    queuedPrompts: [],
    queueActionPending: false,
  }
}

export const useClaudeObserverStore = defineStore('claudeObserver', () => {
  const states = ref<Record<number, ClaudeConversationState>>({})
  const eventsByTab = new Map<number, ClaudeAgentEvent[]>()
  const ptyRunStateByTab = new Map<number, 'idle' | 'working'>()
  const snapshotGate = new ClaudeObserverSnapshotGate()
  const closedTabs = new Set<number>()
  const stoppedPtyTabs = new Set<number>()
  const unconfirmedTerminalInputTabs = new Set<number>()
  const promptReceiptsByTab = new Map<number, ClaudePromptReceipt[]>()
  const queuePumpTabs = new Set<number>()
  const nativeQueueWriteTabs = new Set<number>()
  const promptQueuePausedTabs = new Set<number>()
  const interruptRequestedTabs = new Set<number>()
  const pendingDirectPrompts = new Map<number, string>()
  const interruptedInputDrafts = new Map<number, string>()
  const retractedPromptEventIds = new Set<string>()
  const retractedPromptTexts = new Map<number, string>()
  const questionResponseTabs = new Set<number>()
  const pendingModelSwitchByTab = new Map<number, {
    model: string
    context?: ClaudeModelContext
    previousModel?: string
  }>()
  const compactCompletionWatchBaselines = new Map<number, string>()
  const busyInputMode = ref<ClaudeBusyInputMode>('native')
  let listenerPromise: Promise<void> | null = null
  let queuedPromptSequence = 0
  let localModelEventSequence = 0

  const CLAUDE_SUBMIT_KEY_DELAY_MS = 100
  const CLAUDE_QUEUE_KEY_DELAY_MS = 70
  const CLAUDE_QUESTION_KEY_DELAY_MS = 180
  const CLAUDE_INTERRUPT_TIMEOUT_MS = 5_000
  const CLAUDE_MODEL_CONFIRMATION_TIMEOUT_MS = 4_000
  const CLAUDE_MODEL_CONFIRMATION_POLL_MS = 120
  const CLAUDE_MODEL_SELECTION_TIMEOUT_MS = 3_000
  const CLAUDE_COMPACT_COMPLETION_TIMEOUT_MS = 5 * 60_000
  const CLAUDE_COMPACT_COMPLETION_POLL_MS = 500
  const CLAUDE_PROMPT_HOOK_TIMEOUT_MS = 6_000

  function waitForClaudeInputFrame(delayMs: number) {
    return new Promise<void>((resolve) => window.setTimeout(resolve, delayMs))
  }

  async function waitForTerminalPromptToClear(
    tabId: number,
    promptKind: 'modelSwitchConfirm' | 'pluginInstall',
  ) {
    const deadline = Date.now() + 1_000
    while (Date.now() < deadline) {
      const state = stateFor(tabId)
      if (state.terminalPrompt?.kind !== promptKind) return
      await waitForClaudeInputFrame(CLAUDE_QUEUE_KEY_DELAY_MS)
      await loadSnapshot(tabId)
    }
    throw new Error('终端未确认退出当前选择器，请切换到原始终端检查')
  }

  async function waitForModelSwitchConfirmationIndex(tabId: number, expectedIndex: number) {
    const deadline = Date.now() + CLAUDE_MODEL_SELECTION_TIMEOUT_MS
    while (Date.now() < deadline) {
      await waitForClaudeInputFrame(CLAUDE_QUEUE_KEY_DELAY_MS)
      await loadSnapshot(tabId)
      const prompt = stateFor(tabId).terminalPrompt
      if (prompt?.kind !== 'modelSwitchConfirm') {
        await invoke('pty_write', { tabId, data: '\u001b' })
        ptyRunStateByTab.set(tabId, 'idle')
        const state = stateFor(tabId)
        state.runState = 'idle'
        state.activityStatus = undefined
        throw new Error('模型选择器状态已丢失，已同步取消终端选择')
      }
      if (prompt.selectedIndex === expectedIndex) return
    }
    throw new Error('终端未确认模型选择移动，请重试或按 Esc 取消')
  }

  function stateFor(tabId: number): ClaudeConversationState {
    if (!states.value[tabId]) states.value[tabId] = createState(tabId)
    return states.value[tabId]
  }

  function modelFromSwitchConfirmation(prompt: ClaudeTerminalPrompt | null | undefined) {
    if (prompt?.kind !== 'modelSwitchConfirm') return undefined
    for (const option of prompt.options) {
      const match = option.match(/\bswitch to\s+(.+)$/i)
      if (match?.[1]) return match[1].trim()
    }
    return prompt.prompt.match(/\bSwitching to\s+(.+?)\s+means\b/i)?.[1]?.trim()
  }

  function terminalLogDelta(previous: string, current: string) {
    if (!previous) return current
    if (current.startsWith(previous)) return current.slice(previous.length)
    return ''
  }

  function recordModelSwitch(
    tabId: number,
    model: string,
    changed = true,
    context?: ClaudeModelContext,
  ) {
    const normalized = normalizeClaudeModelDisplayName(model)
    if (!normalized) return
    const existing = eventsByTab.get(tabId) ?? []
    const nextSequence = existing.reduce((maximum, event) => Math.max(maximum, event.sequence), 0) + 1
    const state = stateFor(tabId)
    ptyRunStateByTab.set(tabId, 'idle')
    state.runState = 'idle'
    state.activityStatus = undefined
    state.currentModel = normalized
    if (context) state.currentContext = context
    applyEvents(tabId, [{
      id: `local-model-${tabId}-${Date.now()}-${++localModelEventSequence}`,
      sequence: nextSequence,
      captureId: state.captureId ?? 'local',
      tabId,
      eventName: 'ModelSwitchCompleted',
      receivedAt: new Date().toISOString(),
      payload: { model: normalized, changed },
    }])
  }

  function normalizedModelKey(model: string | null | undefined) {
    return normalizeClaudeModelDisplayName(model ?? '')?.toLowerCase() ?? ''
  }

  async function resolveCurrentModelAfterSwitch(
    tabId: number,
    reportedModel: string,
    previousModel?: string,
  ) {
    const expectedKey = normalizedModelKey(reportedModel)
    const previousKey = normalizedModelKey(previousModel)
    const deadline = Date.now() + 1_200
    while (Date.now() < deadline) {
      const currentModel = stateFor(tabId).currentModel?.trim()
      const currentKey = normalizedModelKey(currentModel)
      if (
        currentModel
        && (
          !previousKey
          || currentKey !== previousKey
          || currentKey === expectedKey
        )
      ) return currentModel
      await waitForClaudeInputFrame(CLAUDE_MODEL_CONFIRMATION_POLL_MS)
      await loadSnapshot(tabId)
    }
    return stateFor(tabId).currentModel?.trim()
      || normalizeClaudeModelDisplayName(reportedModel)
  }

  async function commitModelSwitch(
    tabId: number,
    reportedModel: string,
    changed = true,
    context?: ClaudeModelContext,
    previousModel?: string,
  ) {
    const actualModel = await resolveCurrentModelAfterSwitch(tabId, reportedModel, previousModel)
    if (!actualModel) return
    const switchApplied = claudeObservedModelSwitchApplied(
      reportedModel,
      actualModel,
      previousModel,
    )
    recordModelSwitch(
      tabId,
      actualModel,
      changed && switchApplied,
      switchApplied ? context : undefined,
    )
  }

  function eventPrompt(event: ClaudeAgentEvent): string | undefined {
    for (const key of ['prompt', 'text', 'message']) {
      const value = event.payload[key]
      if (typeof value === 'string' && value.length > 0) return value
    }
    return undefined
  }

  function receiptsFor(tabId: number): ClaudePromptReceipt[] {
    const existing = promptReceiptsByTab.get(tabId)
    if (existing) return existing
    const created: ClaudePromptReceipt[] = []
    promptReceiptsByTab.set(tabId, created)
    return created
  }

  function removePromptReceipt(tabId: number, queuedPromptId: string) {
    const receipts = promptReceiptsByTab.get(tabId)
    if (!receipts) return
    const index = receipts.findIndex(receipt => receipt.queuedPromptId === queuedPromptId)
    if (index >= 0) receipts.splice(index, 1)
  }

  function removePromptReceiptByIdentity(tabId: number, target: ClaudePromptReceipt) {
    const receipts = promptReceiptsByTab.get(tabId)
    if (!receipts) return
    const index = receipts.indexOf(target)
    if (index >= 0) receipts.splice(index, 1)
  }

  function retractUnansweredPrompt(tabId: number, prompt: string) {
    const normalizedPrompt = normalizeClaudePromptForMatch(prompt)
    const existingEvents = eventsByTab.get(tabId) ?? []
    let matchedEvent = false
    for (let index = existingEvents.length - 1; index >= 0; index--) {
      const event = existingEvents[index]
      if (
        event.eventName === 'UserPromptSubmit'
        && normalizeClaudePromptForMatch(eventPrompt(event) ?? '') === normalizedPrompt
      ) {
        retractedPromptEventIds.add(event.id)
        matchedEvent = true
        break
      }
    }
    if (!matchedEvent) retractedPromptTexts.set(tabId, normalizedPrompt)

    const remainingEvents = existingEvents.filter(event => !retractedPromptEventIds.has(event.id))
    eventsByTab.set(tabId, remainingEvents)
    stateFor(tabId).items = reduceClaudeAgentEvents(remainingEvents).items
  }

  function isCompactCommand(prompt: string) {
    return /^\/compact(?:\s|$)/i.test(prompt.trimStart())
  }

  function isContextCompactionActivity(tabId: number) {
    const label = stateFor(tabId).activityStatus?.label.trim() ?? ''
    return label.replace(/[….]+$/u, '').toLowerCase() === 'compacting conversation'
  }

  function compactCommandResult(
    tabId: number,
    receipt: ClaudePromptReceipt,
  ): 'inProgress' | 'completed' | 'notNeeded' | undefined {
    if (!isCompactCommand(receipt.text)) return undefined
    const output = terminalLogDelta(receipt.terminalLogBaseline ?? '', stateFor(tabId).terminalLog)
    if (/\b(?:conversation\s+)?compacted\s+\(ctrl\+o (?:to see full summary|for history)\)/i.test(output)) {
      return 'completed'
    }
    if (/\bnot enough messages to compact\b/i.test(output)) return 'notNeeded'
    if (isContextCompactionActivity(tabId)) return 'inProgress'
    return undefined
  }

  function announceCompactCompletion(tabId: number) {
    stateFor(tabId).compactCompletionRevision += 1
  }

  function watchCompactCompletion(tabId: number, terminalLogBaseline: string) {
    if (compactCompletionWatchBaselines.has(tabId)) return
    compactCompletionWatchBaselines.set(tabId, terminalLogBaseline)
    void (async () => {
      const deadline = Date.now() + CLAUDE_COMPACT_COMPLETION_TIMEOUT_MS
      while (
        compactCompletionWatchBaselines.get(tabId) === terminalLogBaseline
        && Date.now() < deadline
      ) {
        const terminalLog = await refreshTerminalLog(tabId)
        const output = terminalLog
          ? terminalLogDelta(terminalLogBaseline, terminalLog)
          : ''
        if (/\b(?:conversation\s+)?compacted\s+\(ctrl\+o (?:to see full summary|for history)\)/i.test(output)) {
          announceCompactCompletion(tabId)
          break
        }
        if (closedTabs.has(tabId) || !stateFor(tabId).active) break
        await waitForClaudeInputFrame(CLAUDE_COMPACT_COMPLETION_POLL_MS)
      }
      if (compactCompletionWatchBaselines.get(tabId) === terminalLogBaseline) {
        compactCompletionWatchBaselines.delete(tabId)
      }
    })()
  }

  function confirmCompactCommandReceipt(tabId: number) {
    for (const receipt of receiptsFor(tabId)) {
      if (receipt.kind !== 'direct') continue
      const result = compactCommandResult(tabId, receipt)
      if (!result) continue
      removePromptReceiptByIdentity(tabId, receipt)
      if (result === 'inProgress') {
        watchCompactCompletion(tabId, receipt.terminalLogBaseline ?? stateFor(tabId).terminalLog)
      } else if (result === 'completed') {
        announceCompactCompletion(tabId)
      }
      return true
    }
    return false
  }

  function removeQueuedPrompt(tabId: number, queuedPromptId: string) {
    const state = stateFor(tabId)
    const index = state.queuedPrompts.findIndex(item => item.id === queuedPromptId)
    if (index >= 0) state.queuedPrompts.splice(index, 1)
  }

  function markUnconfirmedTerminalInput(tabId: number, reason: string) {
    const state = stateFor(tabId)
    unconfirmedTerminalInputTabs.add(tabId)
    state.available = false
    state.degradedReason = reason
  }

  async function sendQueuedPromptToNative(tabId: number, item: ClaudeQueuedPrompt) {
    const state = stateFor(tabId)
    const writes = encodeClaudeConversationInputWrites(item.text)
    if (writes.length === 0) throw new Error('等待消息为空')
    item.delivery = 'sending'
    nativeQueueWriteTabs.add(tabId)
    state.queueActionPending = true
    let contentWritten = false
    let receiptAdded = false
    try {
      await invoke('pty_write', { tabId, data: writes[0] })
      contentWritten = true
      await waitForClaudeInputFrame(CLAUDE_SUBMIT_KEY_DELAY_MS)
      receiptsFor(tabId).push({
        text: item.text,
        kind: 'native-queue',
        queuedPromptId: item.id,
      })
      receiptAdded = true
      await invoke('pty_write', { tabId, data: writes[1] })
      if (state.queuedPrompts.some(queued => queued.id === item.id)) item.delivery = 'native'
    } catch (error) {
      if (receiptAdded) removePromptReceipt(tabId, item.id)
      if (contentWritten) {
        markUnconfirmedTerminalInput(
          tabId,
          '等待消息正文已写入原始终端，但提交键发送失败；请切换到终端确认。',
        )
      } else {
        item.delivery = 'queued'
      }
      throw error
    } finally {
      nativeQueueWriteTabs.delete(tabId)
      state.queueActionPending = false
    }
  }

  async function pumpPromptQueue(tabId: number, allowIdleDispatch = false) {
    if (
      queuePumpTabs.has(tabId)
      || promptQueuePausedTabs.has(tabId)
      || closedTabs.has(tabId)
    ) return
    const state = stateFor(tabId)
    if (
      state.queueActionPending
      || !state.available
      || !state.active
      || !state.sessionReady
      || state.terminalPrompt
    ) return
    const item = state.queuedPrompts[0]
    if (!item || item.delivery !== 'queued') return

    queuePumpTabs.add(tabId)
    let failed = false
    let dispatchedNative = false
    try {
      if (state.runState === 'working' && item.mode === 'native') {
        await sendQueuedPromptToNative(tabId, item)
        dispatchedNative = true
      } else if (allowIdleDispatch && state.runState === 'idle') {
        item.delivery = 'sending'
        try {
          await submitPrompt(tabId, item.text)
          removeQueuedPrompt(tabId, item.id)
        } catch (error) {
          failed = true
          if (state.available) item.delivery = 'queued'
          state.degradedReason = `等待消息自动发送失败：${String(error)}`
        }
      }
    } catch (error) {
      failed = true
      state.degradedReason = `Claude 原生等待队列同步失败：${String(error)}`
    } finally {
      queuePumpTabs.delete(tabId)
    }

    if (!failed && dispatchedNative && state.runState === 'working') {
      void pumpPromptQueue(tabId)
    }
  }

  function applyEvents(
    tabId: number,
    events: ClaudeAgentEvent[],
    { updateRuntimeStatus = true }: { updateRuntimeStatus?: boolean } = {},
  ) {
    if (closedTabs.has(tabId)) return
    const existingEvents = eventsByTab.get(tabId) ?? []
    const existingIds = new Set(existingEvents.map(event => event.id))
    const acceptedEvents: ClaudeAgentEvent[] = []
    let queueMayAdvance = false
    let completedTurn = false
    for (const event of events) {
      if (retractedPromptEventIds.has(event.id)) continue
      if (
        event.eventName === 'UserPromptSubmit'
        && retractedPromptTexts.get(tabId) === normalizeClaudePromptForMatch(eventPrompt(event) ?? '')
      ) {
        retractedPromptTexts.delete(tabId)
        retractedPromptEventIds.add(event.id)
        continue
      }
      if (existingIds.has(event.id)) continue
      acceptedEvents.push(event)
      if (
        event.eventName === 'PreToolUse'
        || event.eventName === 'MessageDisplay'
        || event.eventName === 'PermissionRequest'
      ) {
        // Once Claude starts replying, Esc leaves no editable prompt in the
        // terminal. Do not restore the already-processed user message.
        pendingDirectPrompts.delete(tabId)
      }
      if (event.eventName === 'UserPromptSubmit') {
        const prompt = eventPrompt(event)
        const receipt = prompt
          ? takeMatchingClaudePromptReceipt(receiptsFor(tabId), prompt)
          : undefined
        const nativePrompt = !receipt && prompt
          ? findMatchingClaudeNativePrompt(stateFor(tabId).queuedPrompts, prompt)
          : undefined
        if (receipt?.kind === 'native-queue' && receipt.queuedPromptId) {
          removeQueuedPrompt(tabId, receipt.queuedPromptId)
          queueMayAdvance = true
        } else if (nativePrompt) {
          // The hook can race with the recall action and arrive after its
          // receipt was removed. Reconcile by prompt text so a sent item does
          // not remain visible in the local queue forever.
          removeQueuedPrompt(tabId, nativePrompt.id)
          queueMayAdvance = true
        }
        if (!receipt) {
          const directIdx = receiptsFor(tabId).findIndex(r => r.kind === 'direct')
          if (directIdx >= 0) receiptsFor(tabId).splice(directIdx, 1)
        }
        const recovered = unconfirmedTerminalInputTabs.delete(tabId)
        const current = stateFor(tabId)
        current.activityStatus = undefined
        if (recovered) current.degradedReason = undefined
      }
      if (
        event.eventName === 'PreToolUse'
        || event.eventName === 'MessageDisplay'
        || event.eventName === 'SessionStart'
      ) {
        const receipts = receiptsFor(tabId)
        const directIdx = receipts.findIndex(r => r.kind === 'direct')
        if (directIdx >= 0) {
          receipts.splice(directIdx, 1)
          if (unconfirmedTerminalInputTabs.delete(tabId)) {
            stateFor(tabId).degradedReason = undefined
          }
        }
      }
      if (event.eventName === 'UserPromptSubmit' || event.eventName === 'PreToolUse') {
        ptyRunStateByTab.set(tabId, 'working')
      } else if (
        event.eventName === 'Stop'
        || event.eventName === 'StopFailure'
        || event.eventName === 'SessionStart'
        || event.eventName === 'SessionEnd'
      ) {
        ptyRunStateByTab.set(tabId, 'idle')
        stateFor(tabId).activityStatus = undefined
        if (event.eventName === 'Stop' || event.eventName === 'StopFailure') {
          completedTurn = true
          queueMayAdvance = true
        }
      }
      if (event.eventName === 'SessionStart') {
        stateFor(tabId).sessionReady = true
      }
    }
    const unique = new Map<string, ClaudeAgentEvent>()
    for (const event of existingEvents) unique.set(event.id, event)
    for (const event of acceptedEvents) unique.set(event.id, event)
    const merged = Array.from(unique.values())
      .sort((left, right) => left.sequence - right.sequence || left.receivedAt.localeCompare(right.receivedAt))
      .slice(-500)
    eventsByTab.set(tabId, merged)
    const reduced = reduceClaudeAgentEvents(merged)
    const state = stateFor(tabId)
    state.items = reduced.items
    if (!updateRuntimeStatus || !state.active) return
    state.runState = reduced.runState === 'working' && ptyRunStateByTab.get(tabId) === 'idle'
      ? 'idle'
      : reduced.runState
    if (acceptedEvents.length > 0) {
      state.available = !unconfirmedTerminalInputTabs.has(tabId)
      if (
        state.available
        && state.degradedReason?.startsWith('发送后未收到 Claude Hook 事件')
      ) {
        state.degradedReason = undefined
      }
    }
    if (queueMayAdvance) {
      void pumpPromptQueue(tabId, completedTurn && state.runState === 'idle')
    }
  }

  function applyStatus(status: ClaudeObserverStatus) {
    if (closedTabs.has(status.tabId)) return
    const state = stateFor(status.tabId)
    if (status.statusRevision < state.statusRevision) return
    const hadActivityStatus = !!state.activityStatus
    state.statusRevision = status.statusRevision
    state.captureId = status.captureId ?? undefined
    state.available = status.available && !unconfirmedTerminalInputTabs.has(status.tabId)
    state.active = stoppedPtyTabs.has(status.tabId) ? false : status.active
    state.degradedReason = status.degradedReason ?? undefined
    state.logDir = status.logDir ?? undefined
    const observedCurrentModel = status.currentModel?.trim()
    if (observedCurrentModel) state.currentModel = observedCurrentModel
    if (status.permissionMode) {
      const observedPermissionMode = permissionModeFromTerminalLabel(status.permissionMode)
      if (!state.pendingPermissionMode || observedPermissionMode === state.pendingPermissionMode) {
        state.permissionMode = status.permissionMode
        state.pendingPermissionMode = undefined
      }
    }
    state.contextUsage = status.contextUsage ?? undefined
    const nextTerminalPrompt = state.active
      ? normalizeClaudeTerminalPrompt(status.terminalPrompt)
      : undefined
    state.terminalPrompt = nextTerminalPrompt
    state.activityStatus = state.active ? status.activityStatus : undefined
    confirmCompactCommandReceipt(status.tabId)
    state.loading = false
    if (
      interruptRequestedTabs.has(status.tabId)
      && state.runState === 'working'
      && hadActivityStatus
      && !state.activityStatus
    ) {
      ptyRunStateByTab.set(status.tabId, 'idle')
      state.runState = 'idle'
    }
    if (status.available && state.active && state.sessionReady && state.runState === 'starting') {
      state.runState = 'idle'
    }
    if (!state.active && state.runState !== 'stopped') state.runState = 'stopped'
  }

  function applyLiveStatus(status: ClaudeObserverStatus) {
    if (closedTabs.has(status.tabId)) return
    snapshotGate.markLiveStatus(status.tabId)
    applyStatus(status)
  }

  async function ensureListeners() {
    if (listenerPromise) return listenerPromise
    listenerPromise = (async () => {
      await listen<ClaudeAgentEvent>('claude_agent_event', (event) => {
        const tabId = event.payload.tabId
        if (typeof tabId !== 'number') return
        snapshotGate.markLiveStatus(tabId)
        applyEvents(tabId, [event.payload])
      })
      await listen<ClaudeObserverStatus>('claude_observer_status', (event) => {
        applyLiveStatus(event.payload)
      })
      await listen<PtyTitle>('pty_title', (event) => {
        const { tab_id: tabId, cli_kind: cliKind, has_spinner: hasSpinner } = event.payload
        const state = states.value[tabId]
        if (!state || closedTabs.has(tabId) || cliKind !== 'claude') return
        if (state.runState === 'permission' || state.runState === 'stopped') return
        if (hasSpinner) {
          if (state.runState !== 'working') state.activityStatus = undefined
          ptyRunStateByTab.set(tabId, 'working')
          state.runState = 'working'
          void pumpPromptQueue(tabId)
        } else if (state.runState === 'working') {
          ptyRunStateByTab.set(tabId, 'idle')
          state.runState = 'idle'
          state.activityStatus = undefined
        }
      })
      await listen<PtyStatus>('pty_status', (event) => {
        const { tab_id: tabId, cli_kind: cliKind, alive } = event.payload
        const state = states.value[tabId]
        if (!state || closedTabs.has(tabId) || cliKind !== 'claude') return
        if (!alive) {
          stoppedPtyTabs.add(tabId)
          snapshotGate.markLiveStatus(tabId)
          state.active = false
          state.runState = 'stopped'
          state.terminalPrompt = undefined
          state.activityStatus = undefined
          state.loading = false
        }
      })
    })()
    return listenerPromise
  }

  async function loadSnapshot(tabId: number) {
    const state = stateFor(tabId)
    state.loading = true
    const request = snapshotGate.beginSnapshot(tabId)
    try {
      await ensureListeners()
      const snapshot = await invoke<ClaudeObserverSnapshot>('get_claude_observer_snapshot', { tabId })
      const isLatestRequest = snapshotGate.isLatestRequest(request)
      const canApplySnapshotStatus = snapshotGate.canApplyStatus(request)
      if (canApplySnapshotStatus) applyStatus(snapshot)
      applyEvents(tabId, snapshot.events, { updateRuntimeStatus: canApplySnapshotStatus })
      if (isLatestRequest) state.terminalLog = snapshot.terminalLog
      state.loading = false
    } catch (error) {
      if (snapshotGate.canApplyStatus(request)) {
        state.available = false
        state.loading = false
        state.degradedReason = `Claude 结构化观察数据加载失败：${error}`
      }
    }
  }

  async function refreshTerminalLog(
    tabId: number,
    projectSessionId?: string,
  ): Promise<string | undefined> {
    const state = stateFor(tabId)
    try {
      const result = await invoke<ClaudeTerminalLogResult>('get_claude_terminal_log', {
        tabId,
        maxLines: 800,
        projectSessionId,
      })
      state.terminalLog = result.text
      state.logDir = result.logDir
      if (result.historical) {
        state.degradedReason = '当前终端没有实时观察日志，正在显示该项目会话最近一次历史日志。'
      } else if (state.degradedReason?.startsWith('终端日志读取失败：')) {
        state.degradedReason = undefined
      }
      return result.text
    } catch (error) {
      state.degradedReason = `终端日志读取失败：${error}`
      return undefined
    }
  }

  function hasModelSwitchConfirmation(text: string) {
    const switchIndex = text.lastIndexOf('Switch model?')
    if (switchIndex < 0) return false
    const prompt = text.slice(switchIndex)
    return /Yes,\s*switch to/i.test(prompt) && /No,\s*go back/i.test(prompt)
  }

  function assertSupportedClaudeSlashCommand(prompt: string) {
    if (validateClaudeSlashCommand(prompt).kind === 'unsupported') {
      throw new Error('暂不支持该命令')
    }
  }

  async function changeModel(tabId: number, model: string): Promise<boolean> {
    const state = stateFor(tabId)
    const previousCurrentModel = state.currentModel
    const writes = encodeClaudeConversationInputWrites(`/model ${model}`)
    if (writes.length === 0) return false
    if (
      !state.available
      || !state.sessionReady
      || !state.active
      || state.runState !== 'idle'
      || !!state.terminalPrompt
    ) {
      throw new Error('当前状态需要在原始终端中继续')
    }

    const baselineTerminalLog = await refreshTerminalLog(tabId) ?? state.terminalLog
    let latestTerminalText = ''
    await invoke('pty_write', { tabId, data: writes[0] })
    await waitForClaudeInputFrame(CLAUDE_SUBMIT_KEY_DELAY_MS)
    await invoke('pty_write', { tabId, data: writes[1] })

    const deadline = Date.now() + CLAUDE_MODEL_CONFIRMATION_TIMEOUT_MS
    while (Date.now() < deadline) {
      await waitForClaudeInputFrame(CLAUDE_MODEL_CONFIRMATION_POLL_MS)
      const terminalLog = await refreshTerminalLog(tabId)
      if (!terminalLog) continue
      if (baselineTerminalLog && !terminalLog.startsWith(baselineTerminalLog)) continue
      const newTerminalText = terminalLog.slice(baselineTerminalLog.length)
      latestTerminalText = newTerminalText
      if (!hasModelSwitchConfirmation(newTerminalText)) continue

      await invoke('pty_write', { tabId, data: '\r' })
      await waitForClaudeInputFrame(CLAUDE_SUBMIT_KEY_DELAY_MS)
      const completedTerminalLog = await refreshTerminalLog(tabId)
      if (completedTerminalLog?.startsWith(baselineTerminalLog)) {
        latestTerminalText = completedTerminalLog.slice(baselineTerminalLog.length)
      }
      const result = parseClaudeModelCommandResult(latestTerminalText)
      const reportedModel = result?.model ?? model
      await commitModelSwitch(
        tabId,
        reportedModel,
        result?.changed ?? true,
        result?.context ?? claudeModelContextFromText(model),
        previousCurrentModel,
      )
      return true
    }

    // Some Claude Code versions apply the change without showing the
    // confirmation screen. In that case the command has already completed;
    // importantly, do not send a blind extra Enter into the conversation.
    const result = parseClaudeModelCommandResult(latestTerminalText)
    const reportedModel = result?.model ?? model
    await commitModelSwitch(
      tabId,
      reportedModel,
      result?.changed ?? true,
      result?.context ?? claudeModelContextFromText(model),
      previousCurrentModel,
    )
    return true
  }

  async function openLogDirectory(tabId: number, projectSessionId?: string) {
    await invoke('open_claude_log_dir', { tabId, projectSessionId })
  }

  async function loadBusyInputMode() {
    try {
      const saved = await invoke<string>('load_claude_busy_input_mode')
      busyInputMode.value = normalizeClaudeBusyInputMode(saved)
    } catch {
      busyInputMode.value = 'native'
    }
  }

  async function setBusyInputMode(mode: ClaudeBusyInputMode) {
    const normalized = normalizeClaudeBusyInputMode(mode)
    const previous = busyInputMode.value
    busyInputMode.value = normalized
    try {
      await invoke('save_claude_busy_input_mode', { mode: normalized })
    } catch (error) {
      busyInputMode.value = previous
      throw error
    }
  }

  async function queuePrompt(tabId: number, prompt: string): Promise<boolean> {
    const state = stateFor(tabId)
    assertSupportedClaudeSlashCommand(prompt)
    if (
      !state.available
      || !state.sessionReady
      || !state.active
      || state.runState !== 'working'
      || !!state.terminalPrompt
    ) {
      throw new Error('当前状态不能加入等待队列')
    }
    if (state.queueActionPending && !nativeQueueWriteTabs.has(tabId)) {
      throw new Error('正在处理等待消息，请稍后再试')
    }
    if (state.queuedPrompts.length >= CLAUDE_PROMPT_QUEUE_LIMIT) {
      throw new Error(`等待队列最多保留 ${CLAUDE_PROMPT_QUEUE_LIMIT} 条消息`)
    }

    const item = createClaudeQueuedPrompt(
      `queued-${tabId}-${Date.now()}-${++queuedPromptSequence}`,
      prompt,
      busyInputMode.value,
    )
    if (!item) return false
    state.queuedPrompts.push(item)
    if (state.queuedPrompts[0] === item && item.mode === 'native') {
      queuePumpTabs.add(tabId)
      try {
        await sendQueuedPromptToNative(tabId, item)
      } catch (error) {
        removeQueuedPrompt(tabId, item.id)
        throw error
      } finally {
        queuePumpTabs.delete(tabId)
      }
      void pumpPromptQueue(tabId)
    }
    return true
  }

  async function recallNativeQueuedPrompt(tabId: number, item: ClaudeQueuedPrompt) {
    const state = stateFor(tabId)
    if (
      !state.available
      || !state.active
      || state.runState !== 'working'
      || !!state.terminalPrompt
    ) {
      throw new Error('当前终端状态不能安全撤回 Claude 原生等待消息')
    }
    if (state.queuedPrompts[0]?.id !== item.id || item.delivery !== 'native') {
      throw new Error('该消息尚未进入 Claude 原生等待队列')
    }
    try {
      for (const data of CLAUDE_NATIVE_QUEUE_RECALL_WRITES) {
        await invoke('pty_write', { tabId, data })
        await waitForClaudeInputFrame(CLAUDE_QUEUE_KEY_DELAY_MS)
      }
    } catch (error) {
      markUnconfirmedTerminalInput(
        tabId,
        `撤回 Claude 原生等待消息时终端按键发送不完整；请切换到终端确认队列：${String(error)}`,
      )
      throw error
    }
    removePromptReceipt(tabId, item.id)
    if (state.queuedPrompts.some(queued => queued.id === item.id)) item.delivery = 'queued'
  }

  async function withdrawQueuedPrompt(tabId: number, queuedPromptId: string): Promise<string> {
    const state = stateFor(tabId)
    const item = state.queuedPrompts.find(queued => queued.id === queuedPromptId)
    if (!item) throw new Error('等待消息已不存在')
    if (
      state.queueActionPending
      || state.queuedPrompts.some(queued => queued.delivery === 'sending')
    ) {
      throw new Error('正在处理等待消息，请稍后再试')
    }

    state.queueActionPending = true
    try {
      if (item.delivery === 'native') await recallNativeQueuedPrompt(tabId, item)
      removeQueuedPrompt(tabId, item.id)
      return item.text
    } finally {
      state.queueActionPending = false
      void pumpPromptQueue(tabId)
    }
  }

  async function waitForClaudeIdle(tabId: number, acceptedPromptId?: string) {
    const startedAt = Date.now()
    while (Date.now() - startedAt < CLAUDE_INTERRUPT_TIMEOUT_MS) {
      const state = stateFor(tabId)
      if (
        acceptedPromptId
        && !state.queuedPrompts.some(queued => queued.id === acceptedPromptId)
      ) return
      if (state.runState === 'idle') return
      if (state.runState === 'permission') throw new Error('Claude 正在等待终端确认')
      if (!state.active || state.runState === 'stopped') throw new Error('Claude 会话已经结束')
      await waitForClaudeInputFrame(50)
    }
    throw new Error('等待 Claude 停止当前处理超时')
  }

  async function interruptRun(tabId: number): Promise<string | undefined> {
    const state = stateFor(tabId)
    if (state.runState !== 'working') return
    if (!state.available || !state.active) throw new Error('Claude 会话当前不可停止')

    interruptRequestedTabs.add(tabId)
    try {
      await invoke('pty_write', { tabId, data: '\u001b' })
      await waitForClaudeIdle(tabId)
      const restoredPrompt = pendingDirectPrompts.get(tabId)
      if (restoredPrompt !== undefined) {
        pendingDirectPrompts.delete(tabId)
        // Esc returns an unanswered prompt to Claude's native input. Clear
        // that native draft before the next structured submission so the
        // restored text is not appended a second time.
        interruptedInputDrafts.set(tabId, restoredPrompt)
        retractUnansweredPrompt(tabId, restoredPrompt)
      }
      return restoredPrompt
    } finally {
      interruptRequestedTabs.delete(tabId)
    }
  }

  async function cyclePermissionMode(tabId: number, expectedMode?: ClaudeDefaultPermissionMode) {
    const state = stateFor(tabId)
    if (!state.available || !state.active || !!state.terminalPrompt) {
      throw new Error('当前 Claude 会话无法切换权限模式')
    }
    if (expectedMode) state.pendingPermissionMode = expectedMode
    try {
      await invoke('pty_write', { tabId, data: '\u001b[Z' })
    } catch (error) {
      if (state.pendingPermissionMode === expectedMode) state.pendingPermissionMode = undefined
      throw error
    }
  }

  async function loadDefaultPermissionMode(): Promise<ClaudeDefaultPermissionMode> {
    const mode = await invoke<string>('load_claude_default_permission_mode')
    return normalizeDefaultPermissionMode(mode)
  }

  async function saveDefaultPermissionMode(mode: ClaudeDefaultPermissionMode) {
    await invoke('save_claude_default_permission_mode', { mode })
  }

  async function insertQueuedPromptNow(tabId: number, queuedPromptId: string) {
    const state = stateFor(tabId)
    const item = state.queuedPrompts.find(queued => queued.id === queuedPromptId)
    if (!item) throw new Error('等待消息已不存在')
    if (
      !state.available
      || !state.active
      || (state.runState !== 'working' && state.runState !== 'idle')
      || !!state.terminalPrompt
    ) {
      throw new Error('当前终端状态不能安全插入等待消息')
    }
    if (
      state.queueActionPending
      || state.queuedPrompts.some(queued => queued.delivery === 'sending')
    ) {
      throw new Error('正在处理等待消息，请稍后再试')
    }

    state.queueActionPending = true
    try {
      const nativeItem = state.queuedPrompts.find(queued => queued.delivery === 'native')
      if (nativeItem) await recallNativeQueuedPrompt(tabId, nativeItem)
      if (!state.queuedPrompts.some(queued => queued.id === item.id)) {
        // The hook may confirm the target while the native queue is being
        // recalled. It is already delivered, so insertion has completed.
        return
      }
      if (state.runState === 'working') {
        interruptRequestedTabs.add(tabId)
        try {
          await invoke('pty_write', { tabId, data: '\u001b' })
          await waitForClaudeIdle(tabId, item.id)
        } finally {
          interruptRequestedTabs.delete(tabId)
        }
      }
      if (!state.queuedPrompts.some(queued => queued.id === item.id)) {
        // Claude accepted the item while the interrupt was in flight. Do not
        // submit it a second time after the stop confirmation.
        return
      }
      await submitPrompt(tabId, item.text)
      removeQueuedPrompt(tabId, item.id)
    } finally {
      state.queueActionPending = false
      void pumpPromptQueue(tabId)
    }
  }

  async function pausePromptQueueForRawTerminal(tabId: number) {
    const state = stateFor(tabId)
    promptQueuePausedTabs.add(tabId)

    const waitStartedAt = Date.now()
    while (
      nativeQueueWriteTabs.has(tabId)
      && Date.now() - waitStartedAt < CLAUDE_INTERRUPT_TIMEOUT_MS
    ) {
      await waitForClaudeInputFrame(25)
    }

    const nativeItem = state.queuedPrompts.find(item => item.delivery === 'native')
    if (
      !nativeItem
      || !state.available
      || !state.active
      || state.runState !== 'working'
      || !!state.terminalPrompt
    ) return

    state.queueActionPending = true
    try {
      await recallNativeQueuedPrompt(tabId, nativeItem)
    } finally {
      state.queueActionPending = false
    }
  }

  function resumePromptQueueFromRawTerminal(tabId: number) {
    promptQueuePausedTabs.delete(tabId)
    compactCompletionWatchBaselines.delete(tabId)
    const state = stateFor(tabId)
    void pumpPromptQueue(tabId, state.runState === 'idle')
  }

  async function submitPrompt(
    tabId: number,
    prompt: string,
    { isCancelled = () => false }: { isCancelled?: () => boolean } = {},
  ): Promise<boolean> {
    const state = stateFor(tabId)
    assertSupportedClaudeSlashCommand(prompt)
    const writes = encodeClaudeConversationInputWrites(prompt)
    if (writes.length === 0 || isCancelled()) return false
    if (
      !state.available
      || !state.sessionReady
      || !state.active
      || state.runState !== 'idle'
      || !!state.terminalPrompt
    ) {
      throw new Error('当前状态需要在原始终端中继续')
    }
    let submissionBaseline: ClaudePromptSubmissionBaseline | undefined
    try {
      submissionBaseline = await invoke<ClaudePromptSubmissionBaseline>(
        'begin_claude_prompt_submission',
        { tabId },
      )
    } catch {
      // The PTY write still provides the primary delivery result. A missing
      // baseline only disables the transcript fallback used by the watchdog.
    }
    const terminalLogBaseline = await refreshTerminalLog(tabId) ?? state.terminalLog
    ptyRunStateByTab.set(tabId, 'working')
    state.runState = 'working'
    state.activityStatus = undefined
    let contentWritten = false
    let promptReceipt: ClaudePromptReceipt | undefined
    try {
      if (interruptedInputDrafts.has(tabId)) {
        await invoke('pty_write', { tabId, data: '\x15' })
        interruptedInputDrafts.delete(tabId)
      }
      await invoke('pty_write', { tabId, data: writes[0] })
      contentWritten = true
      await waitForClaudeInputFrame(CLAUDE_SUBMIT_KEY_DELAY_MS)
      if (isCancelled()) {
        ptyRunStateByTab.set(tabId, 'idle')
        if (state.active && state.sessionReady) {
          state.runState = 'idle'
          unconfirmedTerminalInputTabs.add(tabId)
          state.available = false
          state.degradedReason = '发送已取消，消息正文可能仍保留在原始终端输入框；请切换到终端确认。'
        }
        return false
      }
      if (closedTabs.has(tabId) || !state.active) {
        throw new Error('Claude 会话已在发送过程中结束')
      }
      promptReceipt = { text: prompt, kind: 'direct', terminalLogBaseline }
      receiptsFor(tabId).push(promptReceipt)
      // Start tracking before Enter is written: hook events can arrive as
      // soon as the terminal accepts the submission.
      pendingDirectPrompts.set(tabId, prompt)
      await invoke('pty_write', { tabId, data: writes[1] })
    } catch (error) {
      if (promptReceipt) {
        removePromptReceiptByIdentity(tabId, promptReceipt)
      }
      pendingDirectPrompts.delete(tabId)
      ptyRunStateByTab.set(tabId, 'idle')
      if (state.active && state.sessionReady) {
        state.runState = 'idle'
        if (contentWritten) {
          unconfirmedTerminalInputTabs.add(tabId)
          state.available = false
          state.degradedReason = '消息正文已写入原始终端，但提交键发送失败；请切换到终端继续，不要重复发送。'
        }
      }
      throw error
    }
    window.setTimeout(async () => {
      if (closedTabs.has(tabId)) return
      const current = stateFor(tabId)
      if (!promptReceipt || !receiptsFor(tabId).includes(promptReceipt)) return
      if (!current.active) {
        removePromptReceiptByIdentity(tabId, promptReceipt)
        return
      }
      await loadSnapshot(tabId)
      if (confirmCompactCommandReceipt(tabId)) return
      if (receiptsFor(tabId).includes(promptReceipt) && current.active) {
        let submissionAccepted = false
        if (submissionBaseline) {
          try {
            submissionAccepted = await invoke<boolean>('confirm_claude_prompt_submission', {
              tabId,
              prompt,
              baseline: submissionBaseline,
            })
          } catch {
            submissionAccepted = false
          }
        }
        if (!receiptsFor(tabId).includes(promptReceipt) || !current.active) return
        removePromptReceiptByIdentity(tabId, promptReceipt)
        pendingDirectPrompts.delete(tabId)
        unconfirmedTerminalInputTabs.add(tabId)
        current.available = false
        current.degradedReason = submissionAccepted
          ? '消息已提交到 Claude，但未收到对应 Hook 事件；已停止结构化输入，请在原始终端查看回复。'
          : '未能确认 Claude 已接收消息，且未收到对应 Hook 事件；已停止结构化输入，请切换到原始终端确认后继续。'
        invoke('report_claude_observer_timeout', { tabId, submissionAccepted }).catch(() => {})
      }
    }, CLAUDE_PROMPT_HOOK_TIMEOUT_MS)
    return true
  }

  async function respondToTerminalPrompt(
    tabId: number,
    action: 'confirm' | 'cancel' | 'up' | 'down' | number,
  ) {
    const state = stateFor(tabId)
    if (state.terminalPrompt?.kind === 'modelSwitchConfirm') {
      const confirmationPrompt = state.terminalPrompt
      if (action === -1 || action === 'cancel') {
        await invoke('pty_write', { tabId, data: '\u001b' })
        pendingModelSwitchByTab.delete(tabId)
        await waitForTerminalPromptToClear(tabId, 'modelSwitchConfirm')
        return
      }
      if (action === 'up' || action === 'down') {
        const optionCount = state.terminalPrompt.options.length
        if (optionCount === 0) throw new Error('Invalid model switch confirmation')
        const offset = action === 'up' ? -1 : 1
        const expectedIndex = (
          state.terminalPrompt.selectedIndex + offset + optionCount
        ) % optionCount
        await invoke('pty_write', {
          tabId,
          data: action === 'up' ? '\u001b[A' : '\u001b[B',
        })
        await waitForModelSwitchConfirmationIndex(tabId, expectedIndex)
        return
      }
      if (action === 'confirm') {
        const selectedIndex = confirmationPrompt.selectedIndex
        const selectedOption = confirmationPrompt.options[selectedIndex] ?? ''
        const accepted = /\bYes\b|\bswitch to\b/i.test(selectedOption)
          && !/\bNo\b|go back/i.test(selectedOption)
        const previousTerminalLog = state.terminalLog
        await invoke('pty_write', { tabId, data: '\r' })
        await waitForTerminalPromptToClear(tabId, 'modelSwitchConfirm')
        if (accepted) {
          const result = parseClaudeModelCommandResult(
            terminalLogDelta(previousTerminalLog, state.terminalLog),
          )
          const model = result?.model
            ?? pendingModelSwitchByTab.get(tabId)?.model
            ?? modelFromSwitchConfirmation(confirmationPrompt)
          const pending = pendingModelSwitchByTab.get(tabId)
          if (model) {
            await commitModelSwitch(
              tabId,
              model,
              result?.changed ?? true,
              result?.context ?? pending?.context,
              pending?.previousModel,
            )
          }
        }
        pendingModelSwitchByTab.delete(tabId)
        return
      }
      if (
        typeof action !== 'number'
        || !Number.isInteger(action)
        || action < 0
        || action >= state.terminalPrompt.options.length
      ) {
        throw new Error('Invalid model switch confirmation')
      }
      const delta = action - state.terminalPrompt.selectedIndex
      const selectedOption = confirmationPrompt.options[action] ?? ''
      const accepted = /\bYes\b|\bswitch to\b/i.test(selectedOption)
        && !/\bNo\b|go back/i.test(selectedOption)
      const previousTerminalLog = state.terminalLog
      const direction = delta >= 0 ? '\u001b[B' : '\u001b[A'
      for (let index = 0; index < Math.abs(delta); index += 1) {
        await invoke('pty_write', { tabId, data: direction })
        await waitForClaudeInputFrame(CLAUDE_QUEUE_KEY_DELAY_MS)
      }
      await invoke('pty_write', { tabId, data: '\r' })
      await waitForTerminalPromptToClear(tabId, 'modelSwitchConfirm')
      if (accepted) {
        const result = parseClaudeModelCommandResult(
          terminalLogDelta(previousTerminalLog, state.terminalLog),
        )
        const model = result?.model
          ?? pendingModelSwitchByTab.get(tabId)?.model
          ?? modelFromSwitchConfirmation(confirmationPrompt)
        const pending = pendingModelSwitchByTab.get(tabId)
        if (model) {
          await commitModelSwitch(
            tabId,
            model,
            result?.changed ?? true,
            result?.context ?? pending?.context,
            pending?.previousModel,
          )
        }
      }
      pendingModelSwitchByTab.delete(tabId)
      return
    }
    if (state.terminalPrompt?.kind === 'planApproval') {
      if (action === -1 || action === 'cancel') {
        await invoke('pty_write', { tabId, data: '\u001b' })
        return
      }
      if (
        typeof action !== 'number'
        || !Number.isInteger(action)
        || action < 0
        || action >= state.terminalPrompt.options.length
      ) {
        throw new Error('Invalid plan approval choice')
      }
      const delta = action - state.terminalPrompt.selectedIndex
      const direction = delta >= 0 ? '\u001b[B' : '\u001b[A'
      for (let index = 0; index < Math.abs(delta); index += 1) {
        await invoke('pty_write', { tabId, data: direction })
        await waitForClaudeInputFrame(CLAUDE_QUEUE_KEY_DELAY_MS)
      }
      await invoke('pty_write', { tabId, data: '\r' })
      return
    }
    if (state.terminalPrompt?.kind === 'pluginInstall') {
      if (action === -1) {
        await invoke('pty_write', { tabId, data: '\u001b' })
        return
      }
      if (
        typeof action !== 'number'
        || !Number.isInteger(action)
        || action < 0
        || action >= state.terminalPrompt.options.length
      ) {
        throw new Error('Invalid plugin installation choice')
      }
      for (let index = 0; index < action; index += 1) {
        await invoke('pty_write', { tabId, data: '\u001b[B' })
        await waitForClaudeInputFrame(CLAUDE_QUEUE_KEY_DELAY_MS)
      }
      await invoke('pty_write', { tabId, data: '\r' })
      return
    }
    if (typeof action !== 'string') {
      throw new Error('The current terminal prompt does not support this choice')
    }
    if (!state.active || !state.terminalPrompt) {
      throw new Error('当前没有等待处理的工作区确认')
    }
    if (action !== 'confirm') {
      throw new Error('不信任工作区时应直接关闭当前终端')
    }
    await invoke('pty_write', {
      tabId,
      data: '\r',
    })
  }

  async function respondToAskUserQuestion(
    tabId: number,
    question: ClaudeAskUserQuestion,
    answer: ClaudeQuestionAnswer,
  ) {
    const state = stateFor(tabId)
    const writes = encodeClaudeQuestionAnswerWrites(question, answer)
    if (!writes?.length) {
      throw new Error('璇疯涓烘瘡涓€涓棶棰樻彁渚涘悎娉曠殑閫夐」')
    }
    if (
      !state.available
      || !state.active
      || !state.sessionReady
      || state.runState !== 'permission'
      || !!state.terminalPrompt
    ) {
      throw new Error('褰撳墠娌℃湁绛夊緟涓殑 Claude 閫夐」')
    }
    if (questionResponseTabs.has(tabId)) {
      throw new Error('姝ｅ湪鎻愪氦 Claude 閫夐」')
    }

    questionResponseTabs.add(tabId)
    try {
      for (const data of writes) {
        await invoke('pty_write', { tabId, data })
        await waitForClaudeInputFrame(CLAUDE_QUESTION_KEY_DELAY_MS)
      }
    } finally {
      questionResponseTabs.delete(tabId)
    }
  }

  function removeTab(tabId: number) {
    closedTabs.add(tabId)
    delete states.value[tabId]
    eventsByTab.delete(tabId)
    ptyRunStateByTab.delete(tabId)
    snapshotGate.clear(tabId)
    stoppedPtyTabs.delete(tabId)
    unconfirmedTerminalInputTabs.delete(tabId)
    promptReceiptsByTab.delete(tabId)
    queuePumpTabs.delete(tabId)
    nativeQueueWriteTabs.delete(tabId)
    promptQueuePausedTabs.delete(tabId)
    interruptRequestedTabs.delete(tabId)
    pendingDirectPrompts.delete(tabId)
    interruptedInputDrafts.delete(tabId)
    retractedPromptTexts.delete(tabId)
    questionResponseTabs.delete(tabId)
    pendingModelSwitchByTab.delete(tabId)
  }

  return {
    states,
    busyInputMode,
    loadSnapshot,
    refreshTerminalLog,
    openLogDirectory,
    loadBusyInputMode,
    setBusyInputMode,
    submitPrompt,
    changeModel,
    cyclePermissionMode,
    loadDefaultPermissionMode,
    saveDefaultPermissionMode,
    interruptRun,
    queuePrompt,
    withdrawQueuedPrompt,
    insertQueuedPromptNow,
    pausePromptQueueForRawTerminal,
    resumePromptQueueFromRawTerminal,
    respondToTerminalPrompt,
    respondToAskUserQuestion,
    removeTab,
  }
})
