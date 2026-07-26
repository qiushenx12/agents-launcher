import assert from 'node:assert/strict'
import test from 'node:test'
import {
  parseClaudeClearSessionLifecycleEvent,
  rebindClaudeSessionRecordsAfterClear,
  type ClaudeSessionBindingRecord,
} from '../src/utils/claudeSessionLifecycle.ts'
import type { ClaudeAgentEvent } from '../src/types/claudeObserver.ts'

function event(
  eventName: string,
  payload: Record<string, unknown>,
  tabId: number | undefined = 7,
): ClaudeAgentEvent {
  return {
    id: `${eventName}-1`,
    sequence: 1,
    captureId: 'capture-1',
    tabId,
    eventName,
    receivedAt: '2026-07-24T11:03:36.286Z',
    payload,
  }
}

function session(
  id: string,
  nativeSessionId: string | undefined,
  order = 0,
): ClaudeSessionBindingRecord {
  return {
    cliKind: 'claude',
    id,
    projectId: 'project-1',
    name: nativeSessionId ? 'Old conversation' : 'Main terminal',
    claudeSessionId: nativeSessionId,
    nativeSessionId,
    launchMode: nativeSessionId ? 'resume' : 'new',
    createdAt: 100,
    updatedAt: 200,
    order,
  }
}

test('clear lifecycle parser accepts only the paired clear events', () => {
  assert.deepEqual(
    parseClaudeClearSessionLifecycleEvent(event('SessionEnd', {
      reason: 'clear',
      session_id: 'old-session',
    })),
    { kind: 'end', tabId: 7, sessionId: 'old-session' },
  )
  assert.deepEqual(
    parseClaudeClearSessionLifecycleEvent(event('SessionStart', {
      source: 'clear',
      session_id: 'new-session',
    })),
    { kind: 'start', tabId: 7, sessionId: 'new-session' },
  )
  assert.equal(parseClaudeClearSessionLifecycleEvent(event('SessionEnd', {
    reason: 'other',
    session_id: 'old-session',
  })), null)
  assert.equal(parseClaudeClearSessionLifecycleEvent(event('SessionStart', {
    source: 'resume',
    session_id: 'old-session',
  })), null)
})

test('clear rebind keeps the live local row and preserves the old native session', () => {
  const sessions = [session('local-live', 'old-session')]

  const result = rebindClaudeSessionRecordsAfterClear(sessions, { 'local-live': 7 }, {
    tabId: 7,
    previousNativeSessionId: 'old-session',
    nextNativeSessionId: 'new-session',
    timestamp: 300,
    makeOldSessionId: nativeId => `history-${nativeId}`,
    makeNewSessionName: () => 'New conversation 2',
  })

  assert.equal(result?.liveSession.id, 'local-live')
  assert.equal(result?.liveSession.nativeSessionId, 'new-session')
  assert.equal(result?.liveSession.name, 'New conversation 2')
  assert.equal(result?.liveSession.order, 0)
  assert.equal(result?.oldSession?.id, 'history-old-session')
  assert.equal(result?.oldSession?.nativeSessionId, 'old-session')
  assert.equal(result?.oldSession?.name, 'Old conversation')
  assert.equal(result?.oldSession?.order, 1)
})

test('clear rebind merges an unopened row already discovered for the new session', () => {
  const live = session('local-live', 'old-session')
  const duplicate = session('history-new', 'new-session', 1)
  duplicate.name = 'First prompt in new session'
  const sessions = [live, duplicate]

  const result = rebindClaudeSessionRecordsAfterClear(sessions, { 'local-live': 7 }, {
    tabId: 7,
    previousNativeSessionId: 'old-session',
    nextNativeSessionId: 'new-session',
    timestamp: 300,
    makeOldSessionId: nativeId => `history-${nativeId}`,
    makeNewSessionName: () => 'Unused fallback',
  })

  assert.equal(result?.removedDuplicate?.id, 'history-new')
  assert.equal(result?.liveSession.name, 'First prompt in new session')
  assert.equal(sessions.some(item => item.id === 'history-new'), false)
  assert.equal(sessions.filter(item => item.nativeSessionId === 'new-session').length, 1)
  assert.equal(sessions.filter(item => item.nativeSessionId === 'old-session').length, 1)
})

test('clear rebind uses the SessionEnd id when the live placeholder is not bound yet', () => {
  const sessions = [session('local-live', undefined)]

  const result = rebindClaudeSessionRecordsAfterClear(sessions, { 'local-live': 7 }, {
    tabId: 7,
    previousNativeSessionId: 'old-session',
    nextNativeSessionId: 'new-session',
    timestamp: 300,
    makeOldSessionId: nativeId => `history-${nativeId}`,
    makeNewSessionName: () => 'New conversation 2',
  })

  assert.equal(result?.liveSession.nativeSessionId, 'new-session')
  assert.equal(result?.oldSession?.nativeSessionId, 'old-session')
})
