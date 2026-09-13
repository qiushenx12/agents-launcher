import test from 'node:test'
import assert from 'node:assert/strict'
import {
  shouldOfferPortCleanup,
  describePortCleanupHint,
} from '../src/components/dsh/portCleanup.ts'
import { describePortOccupant } from '../src/utils/dshRuntime.ts'

/**
 * Regression tests for the port-occupancy copy in the dsh configuration panel.
 *
 * Two bugs are pinned here:
 *
 * 1. «一键清理占用» used to be invisible: the condition additionally required an
 *    unsaved draft port while the port was never probed on mount, so
 *    `portStatus` stayed null and the whole block disappeared.
 * 2. The occupant was described as «另一个 dsh 实例» / «其它程序» even when it was
 *    the service this launcher had started (an application restart loses the
 *    child handle, so the runtime status says "not running" while our own dsh
 *    still holds the port). The copy is now uniformly «当前占用进程为 …».
 */

const OCCUPIED_BY_OURS = {
  available: false,
  occupant: 'node.exe（PID 21436）',
  occupantIsDsh: true,
  occupantIsSupervised: true,
}

const OCCUPIED_BY_OTHER_DSH = {
  available: false,
  occupant: 'node.exe（PID 23336）',
  occupantIsDsh: true,
  occupantIsSupervised: false,
}

const OCCUPIED_BY_UNRELATED = {
  available: false,
  occupant: 'nginx.exe（PID 4242）',
  occupantIsDsh: false,
  occupantIsSupervised: false,
}

test('清理入口 appears whenever the port is occupied and the service is stopped', () => {
  assert.equal(
    shouldOfferPortCleanup({
      portStatus: OCCUPIED_BY_OTHER_DSH,
      isRunning: false,
      releasing: false,
    }),
    true,
    'occupancy alone must be enough — a draft port is still an occupied port',
  )

  assert.equal(
    shouldOfferPortCleanup({
      portStatus: OCCUPIED_BY_UNRELATED,
      isRunning: false,
      releasing: false,
    }),
    true,
    'a non-dsh occupant must also offer cleanup',
  )
})

test('清理入口 stays hidden while it would be wrong or useless', () => {
  // The very first render, before any probe answers: nothing to act on yet.
  assert.equal(
    shouldOfferPortCleanup({ portStatus: null, isRunning: false, releasing: false }),
    false,
    'without a probe result there is no known occupant',
  )

  // A free port needs no cleanup.
  assert.equal(
    shouldOfferPortCleanup({
      portStatus: { ...OCCUPIED_BY_UNRELATED, available: true, occupant: null },
      isRunning: false,
      releasing: false,
    }),
    false,
  )

  // The running service holds its own port; 「关闭」 is the right action and the
  // backend would refuse anyway.
  assert.equal(
    shouldOfferPortCleanup({
      portStatus: OCCUPIED_BY_OURS,
      isRunning: true,
      releasing: false,
    }),
    false,
    'a running managed service must not offer cleanup',
  )

  // No double-submit while a cleanup is in flight.
  assert.equal(
    shouldOfferPortCleanup({
      portStatus: OCCUPIED_BY_UNRELATED,
      isRunning: false,
      releasing: true,
    }),
    false,
  )
})

test('占用描述 is the same sentence everywhere and never guesses the origin', () => {
  assert.equal(
    describePortOccupant(OCCUPIED_BY_OURS.occupant, 3080),
    '当前占用进程为 node.exe（PID 21436）。',
    'the launcher\'s own service must not be described as "another" process',
  )
  assert.equal(
    describePortOccupant(OCCUPIED_BY_UNRELATED.occupant, 3080),
    '当前占用进程为 nginx.exe（PID 4242）。',
  )
  assert.equal(describePortOccupant(null, 9000), '端口 9000 的当前占用进程未知。')
  assert.equal(describePortOccupant('   ', 9000), '端口 9000 的当前占用进程未知。')

  for (const occupancy of [OCCUPIED_BY_OURS, OCCUPIED_BY_OTHER_DSH, OCCUPIED_BY_UNRELATED]) {
    const text = describePortOccupant(occupancy.occupant, 3080)
    assert.doesNotMatch(text, /另一个/, 'the copy must not claim a second instance')
    assert.doesNotMatch(text, /其它程序/)
  }
})

test('清理提示 names the consequence for the occupant that was detected', () => {
  const ours = describePortCleanupHint(OCCUPIED_BY_OURS)
  assert.match(ours, /启动器启动/, 'a process we started must be identified as ours')
  assert.match(ours, /其它端口/, 'the hint must offer the non-destructive alternative')

  const otherDsh = describePortCleanupHint(OCCUPIED_BY_OTHER_DSH)
  assert.match(otherDsh, /中断该会话/)
  assert.match(otherDsh, /其它端口/)

  const unrelated = describePortCleanupHint(OCCUPIED_BY_UNRELATED)
  assert.match(unrelated, /结束该进程/)
  assert.match(unrelated, /其它端口/)
  assert.doesNotMatch(unrelated, /中断该会话/)
})
