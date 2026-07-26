import assert from 'node:assert/strict'
import test from 'node:test'
import { resolveClaudeWorkspaceTrustAction } from '../src/utils/claudeWorkspaceTrust.ts'

test('trust confirmation is submitted to the native selector', () => {
  assert.deepEqual(
    resolveClaudeWorkspaceTrustAction('confirm'),
    { kind: 'confirm-in-terminal' },
  )
})

test('workspace rejection closes the terminal instead of sending Escape', () => {
  assert.deepEqual(
    resolveClaudeWorkspaceTrustAction('cancel'),
    { kind: 'reject-and-close-terminal' },
  )
})
