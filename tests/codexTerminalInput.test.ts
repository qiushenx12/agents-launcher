import assert from 'node:assert/strict'
import test from 'node:test'
import {
  encodeCodexConptyInput,
  encodeCodexTerminalInput,
} from '../src/utils/codexTerminalInput.ts'

const ESC = '\x1b'

test('ordinary terminal input is returned unchanged', () => {
  const input = `${ESC}[Ahello 中文\r`
  assert.equal(encodeCodexConptyInput(input), input)
})

test('smart quotes become explicit Win32 key-down and key-up records', () => {
  assert.equal(
    encodeCodexConptyInput('‘’“”'),
    `${ESC}[0;0;8216;1;0;1_${ESC}[0;0;8216;0;0;1_`
      + `${ESC}[0;0;8217;1;0;1_${ESC}[0;0;8217;0;0;1_`
      + `${ESC}[0;0;8220;1;0;1_${ESC}[0;0;8220;0;0;1_`
      + `${ESC}[0;0;8221;1;0;1_${ESC}[0;0;8221;0;0;1_`,
  )
})

test('bracketed paste markers remain intact around encoded quotes', () => {
  assert.equal(
    encodeCodexConptyInput(`${ESC}[200~a’b${ESC}[201~`),
    `${ESC}[200~a${ESC}[0;0;8217;1;0;1_${ESC}[0;0;8217;0;0;1_b${ESC}[201~`,
  )
})

test('macOS Codex input passes UTF-8, IME text, and bracketed paste through unchanged', () => {
  const inputs = [
    '中文组合输入‘智能引号’',
    `${ESC}[200~粘贴“原文”${ESC}[201~`,
    'e\u0301',
  ]
  for (const input of inputs) {
    assert.equal(encodeCodexTerminalInput(input, false), input)
  }
})

test('Windows Codex input retains the ConPTY workaround', () => {
  assert.notEqual(encodeCodexTerminalInput('“', true), '“')
})

test('Windows Codex input sends the star symbol as explicit Win32 records', () => {
  const input = encodeCodexTerminalInput('✱', true)

  assert.equal(
    input,
    `${ESC}[0;0;10033;1;0;1_${ESC}[0;0;10033;0;0;1_`,
  )
})
