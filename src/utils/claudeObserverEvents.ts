import type {
  ClaudeAgentEvent,
  ClaudeConversationItem,
  ClaudeConversationRunState,
  ClaudeModelContext,
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
        }
        runState = 'idle'
        break
      case 'UserPromptSubmit': {
        runState = 'working'
        const text = stringValue(payload, 'prompt', 'text', 'message') ?? '已提交用户消息'
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
        toolItems.set(key, item)
        items.push(item)
        break
      }
      case 'PostToolUse':
      case 'PostToolUseFailure': {
        const key = toolKey(event)
        const existing = toolItems.get(key)
        const state = eventName === 'PostToolUse' ? 'success' : 'failed'
        if (existing) {
          existing.state = state
          existing.toolResult = payload.tool_response ?? payload.tool_result ?? payload.error
          existing.eventId = event.id
          existing.timestamp = event.receivedAt
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
      case 'PermissionRequest':
        runState = 'permission'
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
