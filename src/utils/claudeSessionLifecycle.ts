import type { ClaudeAgentEvent } from '../types/claudeObserver.ts'

export type ClaudeClearSessionLifecycleEvent =
  | {
      kind: 'end'
      tabId: number
      sessionId: string
    }
  | {
      kind: 'start'
      tabId: number
      sessionId: string
    }

export interface ClaudeSessionBindingRecord {
  cliKind: string
  id: string
  projectId: string
  name: string
  claudeSessionId?: string
  nativeSessionId?: string
  launchMode?: 'new' | 'resume' | 'resume_picker'
  createdAt: number
  updatedAt: number
  order: number
}

export interface ClaudeClearSessionRebindResult<T extends ClaudeSessionBindingRecord> {
  liveSession: T
  oldSession?: T
  removedDuplicate?: T
}

function payloadString(payload: Record<string, unknown>, ...keys: string[]) {
  for (const key of keys) {
    const value = payload[key]
    if (typeof value === 'string' && value.trim()) return value.trim()
  }
  return undefined
}

export function parseClaudeClearSessionLifecycleEvent(
  event: ClaudeAgentEvent,
): ClaudeClearSessionLifecycleEvent | null {
  if (typeof event.tabId !== 'number') return null

  const sessionId = payloadString(event.payload, 'session_id', 'sessionId')
  if (!sessionId) return null

  if (
    event.eventName === 'SessionEnd'
    && payloadString(event.payload, 'reason')?.toLowerCase() === 'clear'
  ) {
    return { kind: 'end', tabId: event.tabId, sessionId }
  }

  if (
    event.eventName === 'SessionStart'
    && payloadString(event.payload, 'source')?.toLowerCase() === 'clear'
  ) {
    return { kind: 'start', tabId: event.tabId, sessionId }
  }

  return null
}

export function rebindClaudeSessionRecordsAfterClear<T extends ClaudeSessionBindingRecord>(
  sessions: T[],
  sessionTerminalIds: Readonly<Record<string, number>>,
  options: {
    tabId: number
    previousNativeSessionId?: string
    nextNativeSessionId: string
    timestamp: number
    makeOldSessionId: (nativeSessionId: string) => string
    makeNewSessionName: (otherProjectSessions: T[]) => string
  },
): ClaudeClearSessionRebindResult<T> | null {
  const liveSession = sessions.find(session => (
    session.cliKind === 'claude'
    && sessionTerminalIds[session.id] === options.tabId
  ))
  if (!liveSession) return null

  const previousNativeSessionId = options.previousNativeSessionId
    || liveSession.nativeSessionId
    || liveSession.claudeSessionId
  const nextNativeSessionId = options.nextNativeSessionId.trim()
  if (!nextNativeSessionId) return null

  if (
    liveSession.nativeSessionId === nextNativeSessionId
    && liveSession.claudeSessionId === nextNativeSessionId
  ) {
    return { liveSession }
  }

  const oldName = liveSession.name
  const oldCreatedAt = liveSession.createdAt
  const oldOrder = liveSession.order
  const existingNewSession = sessions.find(session => (
    session.id !== liveSession.id
    && session.cliKind === 'claude'
    && session.projectId === liveSession.projectId
    && (session.nativeSessionId === nextNativeSessionId || session.claudeSessionId === nextNativeSessionId)
  ))
  let removedDuplicate: T | undefined

  if (existingNewSession && sessionTerminalIds[existingNewSession.id] === undefined) {
    const duplicateIndex = sessions.indexOf(existingNewSession)
    if (duplicateIndex !== -1) sessions.splice(duplicateIndex, 1)
    removedDuplicate = existingNewSession
  }

  let oldSession: T | undefined
  if (previousNativeSessionId && previousNativeSessionId !== nextNativeSessionId) {
    oldSession = sessions.find(session => (
      session.id !== liveSession.id
      && session.cliKind === 'claude'
      && session.projectId === liveSession.projectId
      && (
        session.nativeSessionId === previousNativeSessionId
        || session.claudeSessionId === previousNativeSessionId
      )
    ))

    if (!oldSession) {
      oldSession = {
        ...liveSession,
        id: options.makeOldSessionId(previousNativeSessionId),
        name: oldName,
        claudeSessionId: previousNativeSessionId,
        nativeSessionId: previousNativeSessionId,
        launchMode: 'resume',
        createdAt: oldCreatedAt,
        updatedAt: Math.max(oldCreatedAt, options.timestamp - 1),
        order: oldOrder,
      }
      sessions.push(oldSession)
    }
  }

  const otherProjectSessions = sessions.filter(session => (
    session.id !== liveSession.id
    && session.cliKind === 'claude'
    && session.projectId === liveSession.projectId
  ))
  const discoveredName = removedDuplicate?.name.trim()
  liveSession.name = discoveredName && discoveredName !== nextNativeSessionId
    ? discoveredName
    : options.makeNewSessionName(otherProjectSessions)
  liveSession.claudeSessionId = nextNativeSessionId
  liveSession.nativeSessionId = nextNativeSessionId
  liveSession.launchMode = 'resume'
  liveSession.createdAt = options.timestamp
  liveSession.updatedAt = options.timestamp

  const orderedProjectSessions = sessions
    .filter(session => session.cliKind === 'claude' && session.projectId === liveSession.projectId)
    .sort((left, right) => {
      if (left.id === liveSession.id) return -1
      if (right.id === liveSession.id) return 1
      return left.order - right.order || left.createdAt - right.createdAt
    })
  orderedProjectSessions.forEach((session, index) => {
    session.order = index
  })

  return { liveSession, oldSession, removedDuplicate }
}
