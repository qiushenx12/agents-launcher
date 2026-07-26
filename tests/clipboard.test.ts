import assert from 'node:assert/strict'
import test from 'node:test'
import { copyTextToClipboard, copyTextWithDomFallback } from '../src/utils/clipboard.ts'

function fakeDocument(options?: { execThrows?: boolean }) {
  let appended = false
  let removed = false
  let selected = false
  const textarea = {
    value: '',
    readOnly: false,
    style: {} as CSSStyleDeclaration,
    select() { selected = true },
    remove() { removed = true },
  }
  const documentRef = {
    createElement() { return textarea },
    body: { appendChild() { appended = true } },
    execCommand() {
      if (options?.execThrows) throw new Error('copy failed')
      return true
    },
  } as unknown as Document

  return {
    documentRef,
    textarea,
    state: () => ({ appended, removed, selected }),
  }
}

test('clipboard writer receives the complete multiline command unchanged', async () => {
  const command = 'printf "<tag>&value"\ngit status --short\n'
  let written = ''

  await copyTextToClipboard(command, {
    clipboard: { async writeText(text) { written = text } },
  })

  assert.equal(written, command)
})

test('rejected Clipboard API falls back to a temporary textarea and removes it', async () => {
  const fake = fakeDocument()

  await copyTextToClipboard('npm run build\n', {
    clipboard: { async writeText() { throw new Error('denied') } },
    document: fake.documentRef,
  })

  assert.equal(fake.textarea.value, 'npm run build\n')
  assert.deepEqual(fake.state(), { appended: true, removed: true, selected: true })
})

test('DOM fallback always removes its textarea when execCommand throws', () => {
  const fake = fakeDocument({ execThrows: true })

  assert.throws(() => copyTextWithDomFallback('git status', fake.documentRef), /copy failed/)
  assert.deepEqual(fake.state(), { appended: true, removed: true, selected: true })
})
