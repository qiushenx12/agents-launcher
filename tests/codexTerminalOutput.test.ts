import assert from 'node:assert/strict'
import test from 'node:test'
import { createTerminalOutputWriter } from '../src/utils/codexTerminalOutput.ts'

const encoder = new TextEncoder()
const decoder = new TextDecoder()

const START = '\x1b[?2026h'
const END = '\x1b[?2026l'
const ED2 = '\x1b[2J'
const ED3 = '\x1b[3J'
const RESET_CURSOR = '\x1b[0 q'
const HIDE_CURSOR = '\x1b[?25l'
const SHOW_CURSOR = '\x1b[?25h'
const ENTER_ALT_SCREEN = '\x1b[?1049h'
const LEAVE_ALT_SCREEN = '\x1b[?1049l'

function collector() {
  const chunks: Uint8Array[] = []
  return {
    chunks,
    sink: (data: Uint8Array) => chunks.push(data.slice()),
    text: () => chunks.map(chunk => decoder.decode(chunk)).join(''),
  }
}

test('non-Codex output passes through unchanged', () => {
  const output = collector()
  const writer = createTerminalOutputWriter('claude', output.sink)
  const input = encoder.encode(`${START}${ED2}content${END}`)

  writer.write(input)

  assert.equal(output.chunks.length, 1)
  assert.deepEqual(output.chunks[0], input)
})

test('Codex synchronized frames are joined across PTY chunks', () => {
  const output = collector()
  const writer = createTerminalOutputWriter('codex', output.sink)
  const input = `prefix${START}${ED2}frame${RESET_CURSOR}${END}suffix`
  const bytes = encoder.encode(input)

  writer.write(bytes.slice(0, 10))
  writer.write(bytes.slice(10, 19))
  writer.write(bytes.slice(19, 31))
  writer.write(bytes.slice(31))

  assert.equal(output.text(), `prefix${START}${HIDE_CURSOR}frame${SHOW_CURSOR}${END}suffix`)
  assert.equal(output.chunks.length, 3)
})

test('only clear and cursor side effects inside synchronized frames are removed', () => {
  const output = collector()
  const writer = createTerminalOutputWriter('codex', output.sink)

  writer.write(encoder.encode(`${ED2}${RESET_CURSOR}${START}${ED2}a${ED3}${RESET_CURSOR}${END}`))

  assert.equal(
    output.text(),
    `${ED2}${RESET_CURSOR}${START}${HIDE_CURSOR}a${SHOW_CURSOR}${END}`,
  )
})

test('multiple synchronized frames in one chunk remain separate atomic writes', () => {
  const output = collector()
  const writer = createTerminalOutputWriter('codex', output.sink)

  writer.write(encoder.encode(`${START}one${END}${START}two${END}`))

  assert.equal(output.chunks.length, 2)
  assert.equal(
    decoder.decode(output.chunks[0]),
    `${START}${HIDE_CURSOR}one${SHOW_CURSOR}${END}`,
  )
  assert.equal(
    decoder.decode(output.chunks[1]),
    `${START}${HIDE_CURSOR}two${SHOW_CURSOR}${END}`,
  )
})

test('cursor movement stays hidden until the synchronized frame is complete', () => {
  const output = collector()
  const writer = createTerminalOutputWriter('codex', output.sink)

  writer.write(encoder.encode(`${START}${SHOW_CURSOR}draw${HIDE_CURSOR}more${SHOW_CURSOR}${END}`))

  assert.equal(
    output.text(),
    `${START}${HIDE_CURSOR}drawmore${SHOW_CURSOR}${END}`,
  )
})

test('a frame that finishes with a hidden cursor keeps it hidden', () => {
  const output = collector()
  const writer = createTerminalOutputWriter('codex', output.sink)

  writer.write(encoder.encode(`${START}one${HIDE_CURSOR}${END}${START}two${END}`))

  assert.equal(
    output.text(),
    `${START}${HIDE_CURSOR}one${HIDE_CURSOR}${END}`
      + `${START}${HIDE_CURSOR}two${HIDE_CURSOR}${END}`,
  )
})

test('cursor visibility changes outside frames are tracked across chunks', () => {
  const output = collector()
  const writer = createTerminalOutputWriter('codex', output.sink)

  writer.write(encoder.encode(`${START}one${HIDE_CURSOR}${END}`))
  writer.write(encoder.encode(SHOW_CURSOR).slice(0, 3))
  writer.write(encoder.encode(SHOW_CURSOR).slice(3))
  writer.write(encoder.encode(`${START}two${END}`))

  assert.equal(
    output.text(),
    `${START}${HIDE_CURSOR}one${HIDE_CURSOR}${END}`
      + SHOW_CURSOR
      + `${START}${HIDE_CURSOR}two${SHOW_CURSOR}${END}`,
  )
})

test('forced alternate screen is owned by the host for the full Codex session', () => {
  const output = collector()
  const writer = createTerminalOutputWriter('codex', output.sink, { forceAltScreen: true })
  const enter = encoder.encode(ENTER_ALT_SCREEN)
  const leave = encoder.encode(LEAVE_ALT_SCREEN)

  writer.write(encoder.encode('before'))
  writer.write(enter.slice(0, 4))
  writer.write(enter.slice(4))
  writer.write(encoder.encode('middle'))
  writer.write(leave.slice(0, 5))
  writer.write(leave.slice(5))
  writer.write(encoder.encode(`${START}${ENTER_ALT_SCREEN}frame${LEAVE_ALT_SCREEN}${END}`))
  writer.dispose()

  assert.equal(
    output.text(),
    ENTER_ALT_SCREEN
      + 'beforemiddle'
      + `${START}${HIDE_CURSOR}frame${SHOW_CURSOR}${END}`
      + LEAVE_ALT_SCREEN,
  )
})

test('cursor gating keeps Codex in the normal buffer when alternate screen is not forced', async () => {
  const output = collector()
  const writer = createTerminalOutputWriter('codex', output.sink, {
    gateCursorDuringOutput: true,
    cursorIdleDelayMs: 5,
  })

  writer.write(encoder.encode('content'))
  await new Promise(resolve => setTimeout(resolve, 20))

  assert.equal(output.text(), HIDE_CURSOR + 'content' + SHOW_CURSOR)
  assert.equal(output.text().includes(ENTER_ALT_SCREEN), false)
  assert.equal(output.text().includes(LEAVE_ALT_SCREEN), false)
})

test('host-owned cursor stays hidden during output and returns at the final position', async () => {
  const output = collector()
  const writer = createTerminalOutputWriter('codex', output.sink, {
    gateCursorDuringOutput: true,
    cursorIdleDelayMs: 5,
  })

  writer.write(encoder.encode(`before${SHOW_CURSOR}${START}${SHOW_CURSOR}frame${END}`))
  await new Promise(resolve => setTimeout(resolve, 20))

  assert.equal(
    output.text(),
    HIDE_CURSOR + `before${START}frame${END}` + SHOW_CURSOR,
  )
})

test('continuous output hides the host-owned cursor only once per burst', async () => {
  const output = collector()
  const writer = createTerminalOutputWriter('codex', output.sink, {
    gateCursorDuringOutput: true,
    cursorIdleDelayMs: 10,
  })

  writer.write(encoder.encode('one'))
  await new Promise(resolve => setTimeout(resolve, 5))
  writer.write(encoder.encode('two'))
  await new Promise(resolve => setTimeout(resolve, 5))
  writer.write(encoder.encode('three'))
  await new Promise(resolve => setTimeout(resolve, 25))

  assert.equal(output.text(), HIDE_CURSOR + 'onetwothree' + SHOW_CURSOR)
})

test('an incomplete synchronized frame is released by the safety timeout', async () => {
  const output = collector()
  const writer = createTerminalOutputWriter('codex', output.sink, { timeoutMs: 5 })
  const input = `${START}incomplete`

  writer.write(encoder.encode(input))
  await new Promise(resolve => setTimeout(resolve, 20))

  assert.equal(output.text(), input)
})
