import test from 'node:test'
import assert from 'node:assert/strict'
import { projectStoreFingerprint } from '../src/utils/projectStoreFingerprint.ts'

test('project store fingerprint ignores object key order and undefined fields', () => {
  const left = {
    projects: [{ id: 'p1', name: 'demo' }],
    sessions: [],
    future: { enabled: true, omitted: undefined },
  }
  const right = {
    future: { enabled: true },
    sessions: [],
    projects: [{ name: 'demo', id: 'p1' }],
  }

  assert.equal(projectStoreFingerprint(left), projectStoreFingerprint(right))
})

test('project store fingerprint preserves array order and material values', () => {
  const baseline = {
    projects: [{ id: 'p1' }, { id: 'p2' }],
    sessions: [],
  }

  assert.notEqual(
    projectStoreFingerprint(baseline),
    projectStoreFingerprint({ ...baseline, projects: [...baseline.projects].reverse() }),
  )
  assert.notEqual(
    projectStoreFingerprint(baseline),
    projectStoreFingerprint({ ...baseline, sessions: [{ id: 's1' }] }),
  )
})

