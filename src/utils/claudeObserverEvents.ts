import type {
  ClaudeAgentEvent,
  ClaudeConversationItem,
  ClaudeConversationRunState,
  ClaudeModelContext,
  ClaudePlanApprovalPrompt,
  ClaudeSubagentToolUse,
} from '../types/claudeObserver.ts'
import { parseClaudeAskUserQuestions } from './claudeQuestion.ts'

export interface ClaudeEventReduction {
  items: ClaudeConversationItem[]
  runState: ClaudeConversationRunState
}

export interface ClaudeModelCommandResult {
  model: string
  changed: boolean
  text: string
  context?: ClaudeModelContext
}

interface ClaudeCompactCommandResult {
  text: string
}

function record(payload: Record<string, unknown>, key: string): Record<string, unknown> | undefined {
  const value = payload[key]
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined
}

function stringValue(payload: Record<string, unknown>, ...keys: string[]): string | undefined {
  for (const key of keys) {
    const value = payload[key]
    if (typeof value === 'string' && value.length > 0) return value
  }
  return undefined
}

function assistantText(payload: Record<string, unknown>): string {
  const direct = stringValue(payload, 'delta', 'text', 'content', 'message')
  if (direct) return direct

  const message = record(payload, 'message')
  if (message) {
    const messageText = stringValue(message, 'text', 'content')
    if (messageText) return messageText
  }

  return ''
}

function messageKey(event: ClaudeAgentEvent): string {
  return stringValue(event.payload, 'message_id', 'turn_id') ?? event.id
}

function toolKey(event: ClaudeAgentEvent): string {
  return stringValue(event.payload, 'tool_use_id', 'tool_call_id') ?? event.id
}

function toolName(payload: Record<string, unknown>): string {
  return stringValue(payload, 'tool_name', 'tool') ?? '工具调用'
}

function isSubagentLauncherTool(name: string): boolean {
  const normalized = name.trim().toLowerCase()
  return normalized === 'agent' || normalized === 'task'
}

export function isClaudeExitPlanModeTool(name: string): boolean {
  return name.trim().toLowerCase() === 'exitplanmode'
}

export interface ClaudePendingExitPlanModePrompt {
  sequence: number
  prompt: ClaudePlanApprovalPrompt
}

export function pendingClaudeExitPlanModePrompt(
  events: ClaudeAgentEvent[],
): ClaudePendingExitPlanModePrompt | undefined {
  let pending: ClaudeAgentEvent | undefined
  for (const event of [...events].sort((left, right) => (
    left.sequence - right.sequence || left.receivedAt.localeCompare(right.receivedAt)
  ))) {
    if (!isClaudeExitPlanModeTool(toolName(event.payload))) continue
    if (event.eventName === 'PreToolUse' || event.eventName === 'PermissionRequest') {
      pending = event
    } else if (event.eventName === 'PostToolUse' || event.eventName === 'PostToolUseFailure') {
      pending = undefined
    }
  }
  if (!pending) return undefined
  return {
    sequence: pending.sequence,
    prompt: {
      kind: 'planApproval',
      prompt: 'Exit plan mode? Claude wants to exit plan mode.',
      options: ['Yes', 'No'],
      selectedIndex: 0,
    },
  }
}

export function claudePermissionModeLabelFromHookEvent(
  event: ClaudeAgentEvent,
): string | undefined {
  const mode = stringValue(event.payload, 'permission_mode', 'permissionMode')
  switch (mode) {
    case 'bypassPermissions': return '⏵⏵ bypass permissions'
    case 'acceptEdits': return '⏵⏵ accept edits'
    case 'auto': return '⏵⏵ auto mode'
    case 'default': return '⏸ manual mode'
    case 'plan': return '⏸ plan mode'
    default: return undefined
  }
}

function subagentId(payload: Record<string, unknown>): string | undefined {
  return stringValue(payload, 'agent_id', 'agentId')
}

function numberValue(payload: Record<string, unknown>, ...keys: string[]): number | undefined {
  for (const key of keys) {
    const value = payload[key]
    if (typeof value === 'number' && Number.isFinite(value)) return value
  }
  return undefined
}

function statusText(eventName: string, payload: Record<string, unknown>): string {
  return stringValue(payload, 'message', 'reason', 'error') ?? eventName
}

function stripAnsi(text: string): string {
  return text.replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, '')
}

function taggedContent(text: string, tagName: string): string | undefined {
  const match = text.match(new RegExp(`<${tagName}>([\\s\\S]*?)</${tagName}>`, 'i'))
  return match?.[1]
}

function isModelCommandText(text: string): boolean {
  const commandName = taggedContent(text, 'command-name')
  if (commandName?.trim().toLowerCase() === '/model') return true
  return /^\/model(?:\s|$)/i.test(text.trim())
}

function isCompactCommandText(text: string): boolean {
  const commandName = taggedContent(text, 'command-name')
  if (commandName?.trim().toLowerCase() === '/compact') return true
  return /^\/compact(?:\s|$)/i.test(text.trim())
}

function parseClaudeCompactCommandResult(text: string): ClaudeCompactCommandResult | undefined {
  const stdout = taggedContent(text, 'local-command-stdout') ?? text
  const plain = stripAnsi(stdout).replace(/\r/g, '')
  if (/\b(?:conversation\s+)?compacted\s+\(ctrl\+o (?:to see full summary|for history)\)/i.test(plain)) {
    return { text: '已完成上下文压缩' }
  }
  if (/\bnot enough messages to compact\b/i.test(plain)) {
    return { text: '当前消息不足，无需压缩' }
  }
  return undefined
}

export function parseClaudeModelCommandResult(text: string): ClaudeModelCommandResult | undefined {
  const stdout = taggedContent(text, 'local-command-stdout') ?? text
  const plain = stripAnsi(stdout).replace(/\r/g, '')
  let latestResult: ClaudeModelCommandResult | undefined
  let latestResultIndex = -1
  const patterns: Array<{ pattern: RegExp; changed: boolean }> = [
    {
      pattern: /\bSet model to[ \t]+(.+?)(?:[ \t]+and saved as your default for new sessions)?(?=\n|$)/gim,
      changed: true,
    },
    {
      pattern: /\bKept model as[ \t]+(.+?)(?=\n|$)/gim,
      changed: false,
    },
  ]

  for (const { pattern, changed } of patterns) {
    for (const match of plain.matchAll(pattern)) {
      const reportedModel = match[1]?.trim()
      if (!reportedModel) continue
      const model = normalizeClaudeModelDisplayName(reportedModel)
      if (!model) continue
      const matchIndex = match.index ?? 0
      if (matchIndex < latestResultIndex) continue
      latestResultIndex = matchIndex
      latestResult = {
        model,
        changed,
        text: changed ? `模型已切换为 ${model}` : `当前模型保持为 ${model}`,
        context: claudeModelContextFromText(reportedModel),
      }
    }
  }
  return latestResult
}

export function claudeModelContextFromText(text: string): ClaudeModelContext | undefined {
  const plain = stripAnsi(text).trim()
  if (!plain) return undefined
  return /\[1m\]|\b1m\s+context\b/i.test(plain) ? '1m' : '200k'
}

export function normalizeClaudeModelDisplayName(text: string): string | undefined {
  const plain = stripAnsi(text)
    .replace(/\s+and saved as your default.*$/i, '')
    .replace(/\s*\((?:1m\s+context|default)\)\s*/gi, ' ')
    .replace(/\s+/g, ' ')
    .trim()
  return plain || undefined
}

export function claudeObservedModelSwitchApplied(
  reportedModel: string,
  observedModel: string,
  previousModel?: string,
): boolean {
  const key = (model: string | undefined) => (
    normalizeClaudeModelDisplayName(model ?? '')?.toLowerCase() ?? ''
  )
  const reportedKey = key(reportedModel)
  const observedKey = key(observedModel)
  const previousKey = key(previousModel)
  return !!observedKey && (
    observedKey === reportedKey
    || (!!previousKey && observedKey !== previousKey)
  )
}

function isNoResponsePlaceholder(text: string): boolean {
  return text.trim().toLowerCase() === 'no response requested.'
}

function isInternalUserPrompt(payload: Record<string, unknown>, text: string): boolean {
  if (payload.internal === true || payload.is_meta === true || payload.isMeta === true) return true

  const promptSource = stringValue(payload, 'prompt_source', 'promptSource')?.trim().toLowerCase()
  if (promptSource === 'system') return true

  const origin = record(payload, 'origin')
  if (origin && stringValue(origin, 'kind')?.trim().toLowerCase() === 'task-notification') {
    return true
  }

  return /^<task-notification(?:\s|>)/i.test(text.trimStart())
}

function mergeAssistantText(current: string, incoming: string, isDelta: boolean): string {
  if (!incoming) return current
  if (!current) return incoming
  if (isDelta) return current + incoming
  if (incoming.startsWith(current)) return incoming
  if (current.endsWith(incoming)) return current
  return `${current}\n${incoming}`
}

export function reduceClaudeAgentEvents(events: ClaudeAgentEvent[]): ClaudeEventReduction {
  const items: ClaudeConversationItem[] = []
  let runState: ClaudeConversationRunState = 'starting'
  const seenEvents = new Set<string>()
  const assistantItems = new Map<string, ClaudeConversationItem>()
  const assistantIndexes = new Map<string, Set<number>>()
  const toolItems = new Map<string, ClaudeConversationItem>()
  const subagentItems = new Map<string, ClaudeConversationItem>()
  const pendingSubagentItems: ClaudeConversationItem[] = []
  const subagentToolItems = new Map<string, ClaudeSubagentToolUse>()

  const createSubagentItem = (
    event: ClaudeAgentEvent,
    agentId?: string,
  ): ClaudeConversationItem => {
    const type = stringValue(event.payload, 'agent_type')
    const item: ClaudeConversationItem = {
      id: agentId ? `subagent-${agentId}` : `tool-${toolKey(event)}`,
      eventId: event.id,
      kind: 'tool',
      eventName: event.eventName,
      timestamp: event.receivedAt,
      toolName: 'Agent',
      state: 'running',
      subagentId: agentId,
      subagentType: type,
      subagentDescription: type,
      subagentRunMode: 'foreground',
      subagentTools: [],
      subagentTotalToolUseCount: 0,
    }
    if (agentId) subagentItems.set(agentId, item)
    items.push(item)
    return item
  }

  const bindSubagentItem = (
    event: ClaudeAgentEvent,
    agentId: string,
  ): ClaudeConversationItem => {
    const existing = subagentItems.get(agentId)
    if (existing) return existing

    const type = stringValue(event.payload, 'agent_type')
    const matchingPending = [...pendingSubagentItems].reverse().find(item => (
      !item.subagentId
      && item.state === 'running'
      && (!type || !item.subagentType || item.subagentType === type)
    )) ?? [...pendingSubagentItems].reverse().find(item => !item.subagentId && item.state === 'running')

    if (!matchingPending) return createSubagentItem(event, agentId)
    matchingPending.subagentId = agentId
    matchingPending.subagentType = type ?? matchingPending.subagentType
    matchingPending.subagentDescription ??= type
    matchingPending.eventId = event.id
    matchingPending.timestamp = event.receivedAt
    subagentItems.set(agentId, matchingPending)
    return matchingPending
  }

  const orderedEvents = [...events].sort((left, right) =>
    left.sequence - right.sequence || left.receivedAt.localeCompare(right.receivedAt)
  )
  for (const event of orderedEvents) {
    if (seenEvents.has(event.id)) continue
    seenEvents.add(event.id)
    const { payload, eventName } = event

    switch (eventName) {
      case 'HistoricalUserMessage':
      case 'HistoricalAssistantMessage':
      case 'HistoricalLocalCommand': {
        const isLocalCommand = eventName === 'HistoricalLocalCommand'
        const kind = eventName === 'HistoricalUserMessage' ? 'user' : 'assistant'
        const text = stringValue(payload, 'text')
        if (!text) break
        if (kind === 'user' && isInternalUserPrompt(payload, text)) break
        const modelResult = parseClaudeModelCommandResult(text)
        if (modelResult) {
          items.push({
            id: `status-${event.id}`,
            eventId: event.id,
            kind: 'status',
            eventName,
            timestamp: event.receivedAt,
            text: modelResult.text,
          })
          break
        }
        const compactResult = parseClaudeCompactCommandResult(text)
        if (compactResult) {
          items.push({
            id: `status-${event.id}`,
            eventId: event.id,
            kind: 'status',
            eventName,
            timestamp: event.receivedAt,
            text: compactResult.text,
          })
          break
        }
        if (isLocalCommand || isModelCommandText(text) || isCompactCommandText(text) || isNoResponsePlaceholder(text)) break
        items.push({
          id: `history-${event.id}`,
          eventId: event.id,
          kind,
          eventName,
          timestamp: event.receivedAt,
          text,
        })
        break
      }
      case 'ModelSwitchCompleted': {
        const model = stringValue(payload, 'model')
        if (!model) break
        const changed = payload.changed !== false
        runState = 'idle'
        items.push({
          id: `status-${event.id}`,
          eventId: event.id,
          kind: 'status',
          eventName,
          timestamp: event.receivedAt,
          text: changed ? `模型已切换为 ${model}` : `当前模型保持为 ${model}`,
        })
        break
      }
      case 'SessionStart':
        if (stringValue(payload, 'source')?.toLowerCase() === 'clear') {
          items.length = 0
          assistantItems.clear()
          assistantIndexes.clear()
          toolItems.clear()
          subagentItems.clear()
          pendingSubagentItems.length = 0
          subagentToolItems.clear()
        }
        runState = 'idle'
        break
      case 'UserPromptSubmit': {
        runState = 'working'
        const text = stringValue(payload, 'prompt', 'text', 'message') ?? '已提交用户消息'
        if (isInternalUserPrompt(payload, text)) break
        const compactResult = parseClaudeCompactCommandResult(text)
        if (compactResult) {
          runState = 'idle'
          items.push({
            id: `status-${event.id}`,
            eventId: event.id,
            kind: 'status',
            eventName,
            timestamp: event.receivedAt,
            text: compactResult.text,
          })
          break
        }
        if (isModelCommandText(text) || isCompactCommandText(text)) break
        items.push({
          id: `user-${event.id}`,
          eventId: event.id,
          kind: 'user',
          eventName,
          timestamp: event.receivedAt,
          text,
        })
        break
      }
      case 'MessageDisplay': {
        const key = messageKey(event)
        const index = payload.index
        if (typeof index === 'number') {
          const seenIndexes = assistantIndexes.get(key) ?? new Set<number>()
          if (seenIndexes.has(index)) break
          seenIndexes.add(index)
          assistantIndexes.set(key, seenIndexes)
        }
        const incoming = assistantText(payload)
        if (isNoResponsePlaceholder(incoming)) break
        const compactResult = parseClaudeCompactCommandResult(incoming)
        if (compactResult) {
          items.push({
            id: `status-${event.id}`,
            eventId: event.id,
            kind: 'status',
            eventName,
            timestamp: event.receivedAt,
            text: compactResult.text,
          })
          runState = 'idle'
          break
        }
        const existing = assistantItems.get(key)
        if (existing) {
          existing.text = mergeAssistantText(
            existing.text ?? '',
            incoming,
            typeof payload.delta === 'string',
          )
          existing.eventId = event.id
          existing.timestamp = event.receivedAt
        } else {
          const item: ClaudeConversationItem = {
            id: `assistant-${key}`,
            eventId: event.id,
            kind: 'assistant',
            eventName,
            timestamp: event.receivedAt,
            text: incoming,
            messageKey: key,
          }
          assistantItems.set(key, item)
          items.push(item)
        }
        break
      }
      case 'PreToolUse': {
        const key = toolKey(event)
        const currentToolName = toolName(payload)
        const currentSubagentId = subagentId(payload)
        if (currentSubagentId) {
          runState = 'working'
          const subagentItem = bindSubagentItem(event, currentSubagentId)
          const nestedKey = `${currentSubagentId}:${key}`
          const existingTool = subagentToolItems.get(nestedKey)
          if (existingTool) {
            existingTool.toolInput = payload.tool_input ?? payload.input
            existingTool.timestamp = event.receivedAt
          } else {
            const nestedTool: ClaudeSubagentToolUse = {
              id: nestedKey,
              toolName: currentToolName,
              toolInput: payload.tool_input ?? payload.input,
              state: 'running',
              timestamp: event.receivedAt,
            }
            subagentToolItems.set(nestedKey, nestedTool)
            subagentItem.subagentTools ??= []
            subagentItem.subagentTools.push(nestedTool)
            subagentItem.subagentTotalToolUseCount = subagentItem.subagentTools.length
          }
          subagentItem.eventId = event.id
          subagentItem.timestamp = event.receivedAt
          break
        }
        const isQuestion = parseClaudeAskUserQuestions(
          currentToolName,
          payload.tool_input ?? payload.input,
        ) !== null
        runState = isQuestion ? 'permission' : 'working'
        const existing = toolItems.get(key)
        if (existing) {
          existing.eventId = event.id
          existing.timestamp = event.receivedAt
          existing.toolInput = payload.tool_input ?? payload.input
          break
        }
        const item: ClaudeConversationItem = {
          id: `tool-${key}`,
          eventId: event.id,
          kind: 'tool',
          eventName,
          timestamp: event.receivedAt,
          toolName: currentToolName,
          toolInput: payload.tool_input ?? payload.input,
          state: isQuestion ? 'waiting' : 'running',
        }
        if (isSubagentLauncherTool(currentToolName)) {
          const input = record(payload, 'tool_input') ?? record(payload, 'input')
          item.subagentType = input ? stringValue(input, 'subagent_type', 'agent_type') : undefined
          item.subagentDescription = input ? stringValue(input, 'description') : undefined
          item.subagentRunMode = 'foreground'
          item.subagentTools = []
          item.subagentTotalToolUseCount = 0
          pendingSubagentItems.push(item)
        }
        toolItems.set(key, item)
        items.push(item)
        break
      }
      case 'PostToolUse':
      case 'PostToolUseFailure': {
        const key = toolKey(event)
        const currentSubagentId = subagentId(payload)
        if (currentSubagentId) {
          const subagentItem = bindSubagentItem(event, currentSubagentId)
          const nestedKey = `${currentSubagentId}:${key}`
          const nestedState = eventName === 'PostToolUse' ? 'success' : 'failed'
          const existingTool = subagentToolItems.get(nestedKey)
          if (existingTool) {
            existingTool.state = nestedState
            existingTool.timestamp = event.receivedAt
          } else {
            const nestedTool: ClaudeSubagentToolUse = {
              id: nestedKey,
              toolName: toolName(payload),
              toolInput: payload.tool_input ?? payload.input,
              state: nestedState,
              timestamp: event.receivedAt,
            }
            subagentToolItems.set(nestedKey, nestedTool)
            subagentItem.subagentTools ??= []
            subagentItem.subagentTools.push(nestedTool)
            subagentItem.subagentTotalToolUseCount = subagentItem.subagentTools.length
          }
          subagentItem.eventId = event.id
          subagentItem.timestamp = event.receivedAt
          break
        }
        const existing = toolItems.get(key)
        const state = eventName === 'PostToolUse' ? 'success' : 'failed'
        if (existing) {
          existing.state = state
          existing.toolResult = payload.tool_response ?? payload.tool_result ?? payload.error
          existing.eventId = event.id
          existing.timestamp = event.receivedAt
          if (isSubagentLauncherTool(existing.toolName ?? '')) {
            const response = record(payload, 'tool_response') ?? record(payload, 'tool_result')
            const responseStatus = response ? stringValue(response, 'status')?.toLowerCase() : undefined
            const backgrounded = eventName === 'PostToolUse' && (
              response?.isAsync === true || responseStatus === 'async_launched'
            )
            if (backgrounded) {
              existing.state = 'running'
              existing.subagentRunMode = 'background'
            }
            const responseAgentId = response ? stringValue(response, 'agentId', 'agent_id') : undefined
            if (responseAgentId) {
              existing.subagentId = responseAgentId
              existing.subagentType = response
                ? stringValue(response, 'agentType', 'agent_type') ?? existing.subagentType
                : existing.subagentType
              subagentItems.set(responseAgentId, existing)
            }
            const totalToolUseCount = response
              ? numberValue(response, 'totalToolUseCount', 'total_tool_use_count')
              : undefined
            if (totalToolUseCount !== undefined) {
              existing.subagentTotalToolUseCount = Math.max(
                totalToolUseCount,
                existing.subagentTools?.length ?? 0,
              )
            }
          }
          if (existing.toolName?.toLowerCase() === 'askuserquestion') runState = 'working'
        } else {
          const item: ClaudeConversationItem = {
            id: `tool-${key}`,
            eventId: event.id,
            kind: 'tool',
            eventName,
            timestamp: event.receivedAt,
            toolName: toolName(payload),
            toolResult: payload.tool_response ?? payload.tool_result ?? payload.error,
            state,
          }
          toolItems.set(key, item)
          items.push(item)
          if (item.toolName?.toLowerCase() === 'askuserquestion') runState = 'working'
        }
        break
      }
      case 'SubagentStart': {
        runState = 'working'
        const currentSubagentId = subagentId(payload)
        if (!currentSubagentId) break
        const item = bindSubagentItem(event, currentSubagentId)
        item.subagentType = stringValue(payload, 'agent_type') ?? item.subagentType
        item.subagentDescription ??= item.subagentType
        item.subagentRunMode ??= 'foreground'
        item.state = 'running'
        break
      }
      case 'SubagentStop': {
        const currentSubagentId = subagentId(payload)
        if (!currentSubagentId) break
        const item = bindSubagentItem(event, currentSubagentId)
        item.subagentType = stringValue(payload, 'agent_type') ?? item.subagentType
        item.subagentDescription ??= item.subagentType
        item.state = 'success'
        item.eventId = event.id
        item.timestamp = event.receivedAt
        break
      }
      case 'PermissionRequest':
        runState = 'permission'
        if (isClaudeExitPlanModeTool(toolName(payload))) {
          // The store promotes this permission request to the structured plan
          // approval overlay, including a Yes/No fallback when the terminal
          // redraw has not exposed its options yet.
          break
        }
        if (parseClaudeAskUserQuestions(
          toolName(payload),
          payload.tool_input ?? payload.input,
        ) !== null) {
          // PreToolUse already rendered the interactive question card. Claude
          // emits PermissionRequest with the same questions immediately after
          // it, so a second generic terminal-only card would be misleading.
          break
        }
        items.push({
          id: `permission-${event.id}`,
          eventId: event.id,
          kind: 'permission',
          eventName,
          timestamp: event.receivedAt,
          text: `${toolName(payload)} 正在等待确认，请切换到原始终端处理。`,
          toolName: toolName(payload),
          toolInput: payload.tool_input ?? payload.input,
          state: 'waiting',
        })
        break
      case 'Stop':
        runState = 'idle'
        break
      case 'StopFailure':
        runState = 'idle'
        items.push({
          id: `status-${event.id}`,
          eventId: event.id,
          kind: 'status',
          eventName,
          timestamp: event.receivedAt,
          text: statusText(eventName, payload),
          state: 'failed',
        })
        break
      case 'SessionEnd':
        // SessionEnd closes Claude's logical conversation. Commands such as
        // /clear immediately start a fresh session in the same live PTY, so
        // only pty_status(alive=false) may mark the UI as truly stopped.
        runState = 'idle'
        break
      case 'Notification':
        items.push({
          id: `status-${event.id}`,
          eventId: event.id,
          kind: 'status',
          eventName,
          timestamp: event.receivedAt,
          text: statusText(eventName, payload),
        })
        break
      default:
        break
    }
  }

  return { items, runState }
}
