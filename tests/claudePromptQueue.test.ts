import assert from 'node:assert/strict'
import test from 'node:test'
import {
  CLAUDE_NATIVE_QUEUE_RECALL_WRITES,
  CLAUDE_PROMPT_QUEUE_LIMIT,
  createClaudeQueuedPrompt,
  findMatchingClaudeNativePrompt,
  normalizeClaudeBusyInputMode,
  takeMatchingClaudePromptReceipt,
} from '../src/utils/claudePromptQueue.ts'
import type { ClaudePromptReceipt } from '../src/types/claudeObserver.ts'

test('Claude busy input mode falls back to the native queue', () => {
  assert.equal(normalizeClaudeBusyInputMode('native'), 'native')
  assert.equal(normalizeClaudeBusyInputMode('after-stop'), 'after-stop')
  assert.equal(normalizeClaudeBusyInputMode('unknown'), 'native')
})

test('queued prompts are sanitized and limited to five by the shared contract', () => {
  assert.equal(CLAUDE_PROMPT_QUEUE_LIMIT, 5)
  assert.deepEqual(createClaudeQueuedPrompt('q1', 'one\r\ntwo\x1b', 'native'), {
    id: 'q1',
    text: 'one\ntwo',
    mode: 'native',
    delivery: 'queued',
  })
  assert.equal(createClaudeQueuedPrompt('q2', ' \n\t ', 'after-stop'), null)
})

test('prompt receipts preserve direct and queued submissions with identical text', () => {
  const receipts: ClaudePromptReceipt[] = [
    { text: 'same', kind: 'direct' },
    { text: 'same', kind: 'native-queue', queuedPromptId: 'q1' },
  ]

  assert.deepEqual(takeMatchingClaudePromptReceipt(receipts, 'same'), {
    text: 'same',
    kind: 'direct',
  })
  assert.deepEqual(takeMatchingClaudePromptReceipt(receipts, 'same'), {
    text: 'same',
    kind: 'native-queue',
    queuedPromptId: 'q1',
  })
  assert.equal(receipts.length, 0)
})

test('native queued prompt recall selects, opens, and clears the terminal editor', () => {
  assert.deepEqual(CLAUDE_NATIVE_QUEUE_RECALL_WRITES, ['\x1b[A', '\r', '\x15'])
})

test('a native prompt is reconciled when its receipt was removed during recall', () => {
  const native = createClaudeQueuedPrompt('q1', 'already sent', 'native')!
  native.delivery = 'queued'
  const afterStop = createClaudeQueuedPrompt('q2', 'send next', 'after-stop')!

  assert.equal(
    findMatchingClaudeNativePrompt([native, afterStop], 'already sent')?.id,
    'q1',
  )
  assert.equal(findMatchingClaudeNativePrompt([native, afterStop], 'send next'), undefined)
})
