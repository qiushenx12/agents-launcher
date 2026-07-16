import assert from 'node:assert/strict'
import test from 'node:test'
import { encodeCodexConptyInput } from '../src/utils/codexTerminalInput.ts'

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
