import assert from 'node:assert/strict'
import test from 'node:test'
import { ClaudeObserverSnapshotGate } from '../src/utils/claudeObserverSnapshotGate.ts'

test('live status invalidates an older observer snapshot', () => {
  const gate = new ClaudeObserverSnapshotGate()
  const request = gate.beginSnapshot(7)

  assert.equal(gate.canApplyStatus(request), true)
  gate.markLiveStatus(7)
  assert.equal(gate.canApplyStatus(request), false)
})

test('only the latest concurrent snapshot may update observer status', () => {
  const gate = new ClaudeObserverSnapshotGate()
  const first = gate.beginSnapshot(7)
  const second = gate.beginSnapshot(7)

  assert.equal(gate.isLatestRequest(first), false)
  assert.equal(gate.canApplyStatus(first), false)
  assert.equal(gate.isLatestRequest(second), true)
  assert.equal(gate.canApplyStatus(second), true)
})

test('snapshot gates are isolated per terminal tab and can be cleared', () => {
  const gate = new ClaudeObserverSnapshotGate()
  const firstTab = gate.beginSnapshot(7)
  const secondTab = gate.beginSnapshot(8)
  gate.markLiveStatus(7)

  assert.equal(gate.canApplyStatus(firstTab), false)
  assert.equal(gate.canApplyStatus(secondTab), true)

  gate.clear(7)
  assert.equal(gate.isLatestRequest(firstTab), false)
})
