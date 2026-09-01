import test from 'node:test'
import assert from 'node:assert/strict'
import { InFlightTaskCache } from '../src/utils/inFlightTaskCache.ts'

test('in-flight task cache shares one running request per key', async () => {
  const cache = new InFlightTaskCache<string, number>()
  let calls = 0
  let resolve!: (value: number) => void
  const pending = new Promise<number>((done) => {
    resolve = done
  })

  const first = cache.run('claude', async () => {
    calls += 1
    return pending
  })
  const second = cache.run('claude', async () => {
    calls += 1
    return 99
  })
  resolve(42)

  assert.equal(await first, 42)
  assert.equal(await second, 42)
  assert.equal(calls, 1)
})

test('in-flight task cache clears failed requests so they can retry', async () => {
  const cache = new InFlightTaskCache<string, number>()
  await assert.rejects(cache.run('git', async () => {
    throw new Error('failed')
  }))

  assert.equal(await cache.run('git', async () => 7), 7)
})

