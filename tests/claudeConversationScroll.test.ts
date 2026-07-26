import assert from 'node:assert/strict'
import test from 'node:test'
import {
  clearClaudeConversationScroll,
  getClaudeConversationScroll,
  saveClaudeConversationScroll,
} from '../src/utils/claudeConversationScroll.ts'

test('conversation scroll positions are isolated per stable session id', () => {
  clearClaudeConversationScroll('session-101')
  clearClaudeConversationScroll('session-202')

  assert.equal(getClaudeConversationScroll('session-101'), undefined)
  saveClaudeConversationScroll('session-101', 480)
  saveClaudeConversationScroll('session-202', 125)

  assert.equal(getClaudeConversationScroll('session-101'), 480)
  assert.equal(getClaudeConversationScroll('session-202'), 125)

  clearClaudeConversationScroll('session-101')
  clearClaudeConversationScroll('session-202')
})

test('invalid negative scroll offsets are clamped and closing clears state', () => {
  saveClaudeConversationScroll('session-303', -50)
  assert.equal(getClaudeConversationScroll('session-303'), 0)

  clearClaudeConversationScroll('session-303')
  assert.equal(getClaudeConversationScroll('session-303'), undefined)
})

test('a restarted terminal keeps the position because the session id is unchanged', () => {
  const sessionId = 'session-restarted-terminal'
  saveClaudeConversationScroll(sessionId, 720)

  assert.equal(getClaudeConversationScroll(sessionId), 720)

  clearClaudeConversationScroll(sessionId)
})
