import assert from 'node:assert/strict'
import test from 'node:test'
import {
  MAX_CONVERSATION_IMAGE_ATTACHMENTS,
  arrayBufferToBase64,
  imageMimeToExtension,
  partitionPathsByImageLimit,
} from '../src/utils/claudeFileDrop.ts'

test('partitionPathsByImageLimit accepts images up to the remaining quota', () => {
  const result = partitionPathsByImageLimit(
    ['a.png', 'b.jpg', 'c.png'],
    0,
    3,
  )
  assert.deepEqual(result, { accepted: ['a.png', 'b.jpg', 'c.png'], rejected: [] })
})

test('partitionPathsByImageLimit rejects images once the limit is reached', () => {
  const result = partitionPathsByImageLimit(
    ['a.png', 'b.jpg', 'c.png'],
    2,
    3,
  )
  assert.deepEqual(result, { accepted: ['a.png'], rejected: ['b.jpg', 'c.png'] })
})

test('partitionPathsByImageLimit never rejects non-image paths', () => {
  const result = partitionPathsByImageLimit(
    ['a.png', 'notes.txt', 'b.png', 'more.txt'],
    5,
    5,
  )
  assert.deepEqual(result, { accepted: ['notes.txt', 'more.txt'], rejected: ['a.png', 'b.png'] })
})

test('partitionPathsByImageLimit defaults to the shared conversation limit', () => {
  const paths = Array.from({ length: MAX_CONVERSATION_IMAGE_ATTACHMENTS + 2 }, (_, i) => `img${i}.png`)
  const result = partitionPathsByImageLimit(paths, 0)
  assert.equal(result.accepted.length, MAX_CONVERSATION_IMAGE_ATTACHMENTS)
  assert.equal(result.rejected.length, 2)
})

test('imageMimeToExtension maps known MIME types', () => {
  assert.equal(imageMimeToExtension('image/png'), 'png')
  assert.equal(imageMimeToExtension('image/jpeg'), 'jpg')
  assert.equal(imageMimeToExtension('image/gif'), 'gif')
  assert.equal(imageMimeToExtension('image/webp'), 'webp')
  assert.equal(imageMimeToExtension('image/bmp'), 'bmp')
  assert.equal(imageMimeToExtension('IMAGE/PNG'), 'png')
})

test('imageMimeToExtension rejects unsupported MIME types', () => {
  assert.equal(imageMimeToExtension('image/svg+xml'), null)
  assert.equal(imageMimeToExtension('application/pdf'), null)
})

test('arrayBufferToBase64 round-trips arbitrary bytes', () => {
  const bytes = Uint8Array.from({ length: 300 }, (_, i) => i % 256)
  const encoded = arrayBufferToBase64(bytes.buffer)
  const decoded = Uint8Array.from(Buffer.from(encoded, 'base64'))
  assert.deepEqual(decoded, bytes)
})

test('arrayBufferToBase64 handles empty buffers', () => {
  assert.equal(arrayBufferToBase64(new ArrayBuffer(0)), '')
})
