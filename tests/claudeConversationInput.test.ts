import assert from 'node:assert/strict'
import test from 'node:test'
import {
  encodeClaudeConversationInput,
  encodeClaudeConversationInputWrites,
} from '../src/utils/claudeConversationInput.ts'

test('single-line Claude input preserves intentional surrounding spaces', () => {
  assert.equal(encodeClaudeConversationInput('  hello  '), '  hello  \r')
})

test('multiline Claude input uses bracketed paste before submitting once', () => {
  assert.equal(
    encodeClaudeConversationInput('line one\nline two'),
    '\x1b[200~line one\nline two\x1b[201~\r',
  )
})

test('Windows newlines normalize inside bracketed paste', () => {
  assert.equal(
    encodeClaudeConversationInput('one\r\ntwo'),
    '\x1b[200~one\ntwo\x1b[201~\r',
  )
})

test('single-line tabs use bracketed paste instead of terminal key input', () => {
  assert.equal(
    encodeClaudeConversationInput('one\ttwo'),
    '\x1b[200~one\ttwo\x1b[201~\r',
  )
})

test('terminal control characters from pasted text cannot escape paste mode', () => {
  assert.equal(
    encodeClaudeConversationInput('one\x1b[201~two\nthree'),
    '\x1b[200~one[201~two\nthree\x1b[201~\r',
  )
})

test('DEL and C1 terminal controls are removed from message text', () => {
  assert.equal(encodeClaudeConversationInput('one\x7ftwo\u0085three'), 'onetwothree\r')
})

test('whitespace-only input is ignored', () => {
  assert.equal(encodeClaudeConversationInput(' \n\t '), '')
})

test('terminal text and the submit key are written in separate PTY frames', () => {
  assert.deepEqual(
    encodeClaudeConversationInputWrites('hello'),
    ['hello', '\r'],
  )
  assert.deepEqual(
    encodeClaudeConversationInputWrites('line one\nline two'),
    ['\x1b[200~line one\nline two\x1b[201~', '\r'],
  )
  assert.deepEqual(encodeClaudeConversationInputWrites(' \n\t '), [])
})
