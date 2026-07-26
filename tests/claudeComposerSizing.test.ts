import assert from 'node:assert/strict'
import test from 'node:test'
import {
  CLAUDE_COMPOSER_MAX_ROWS,
  CLAUDE_COMPOSER_MIN_ROWS,
  getClaudeComposerTextareaMetrics,
} from '../src/utils/claudeComposerSizing.ts'

const LINE_HEIGHT = 22

test('Claude composer keeps two rows before content needs to grow', () => {
  assert.deepEqual(
    getClaudeComposerTextareaMetrics(LINE_HEIGHT, LINE_HEIGHT),
    { height: LINE_HEIGHT * CLAUDE_COMPOSER_MIN_ROWS, overflowY: 'hidden' },
  )
  assert.deepEqual(
    getClaudeComposerTextareaMetrics(LINE_HEIGHT * 2, LINE_HEIGHT),
    { height: LINE_HEIGHT * CLAUDE_COMPOSER_MIN_ROWS, overflowY: 'hidden' },
  )
})

test('Claude composer grows from the third row through the ninth row', () => {
  assert.deepEqual(
    getClaudeComposerTextareaMetrics(LINE_HEIGHT * 3, LINE_HEIGHT),
    { height: LINE_HEIGHT * 3, overflowY: 'hidden' },
  )
  assert.deepEqual(
    getClaudeComposerTextareaMetrics(LINE_HEIGHT * 9, LINE_HEIGHT),
    { height: LINE_HEIGHT * CLAUDE_COMPOSER_MAX_ROWS, overflowY: 'hidden' },
  )
})

test('Claude composer caps at nine rows and enables vertical scrolling', () => {
  assert.deepEqual(
    getClaudeComposerTextareaMetrics(LINE_HEIGHT * 10, LINE_HEIGHT),
    { height: LINE_HEIGHT * CLAUDE_COMPOSER_MAX_ROWS, overflowY: 'auto' },
  )
})
