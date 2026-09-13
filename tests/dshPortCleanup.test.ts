import test from 'node:test'
import assert from 'node:assert/strict'
import {
  shouldOfferPortCleanup,
  describePortCleanupHint,
} from '../src/components/dsh/portCleanup.ts'
import {
  classifyPortConflict,
  describePortOccupant,
  describeRuntimePortConflict,
} from '../src/utils/dshRuntime.ts'

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

/**
 * The runtime panel's «清理并启动» card: the conflict classification decides
 * which explanation the user gets, and the port matches by construction (it is
 * the saved port that was probed), so the listen scope is the only axis that
 * can diverge from the saved configuration.
 */
test('冲突分类 compares the occupant listen scope with the saved access mode', () => {
  assert.equal(
    classifyPortConflict(
      { occupantIsDsh: false, occupantIsSupervised: false, occupantListenScope: null },
      'local',
    ),
    'other-program',
  )
  assert.equal(
    classifyPortConflict(
      { occupantIsDsh: true, occupantIsSupervised: true, occupantListenScope: 'local' },
      'local',
    ),
    'supervised',
    'the launcher-supervised service is never a cleanup target',
  )
  assert.equal(
    classifyPortConflict(
      { occupantIsDsh: true, occupantIsSupervised: false, occupantListenScope: 'local' },
      'local',
    ),
    'dsh-match',
  )
  assert.equal(
    classifyPortConflict(
      { occupantIsDsh: true, occupantIsSupervised: false, occupantListenScope: 'remote' },
      'local',
    ),
    'dsh-mismatch',
    'a remote-listening dsh conflicts with a saved 本地 config',
  )
  assert.equal(
    classifyPortConflict(
      { occupantIsDsh: true, occupantIsSupervised: false, occupantListenScope: 'local' },
      'remote',
    ),
    'dsh-mismatch',
    'a loopback-only dsh conflicts with a saved 远程 config',
  )
  assert.equal(
    classifyPortConflict(
      { occupantIsDsh: true, occupantIsSupervised: false, occupantListenScope: null },
      'remote',
    ),
    'dsh-unknown',
    'an undeterminable scope must not be claimed as consistent',
  )
})

test('冲突文案 names the occupant, the mismatch and the saved config', () => {
  const base = {
    port: 3080,
    occupant: 'node.exe（PID 21436）',
    savedAccess: 'local' as const,
    savedPort: 3080,
  }

  const mismatch = describeRuntimePortConflict({
    ...base,
    kind: 'dsh-mismatch',
    occupantListenScope: 'remote',
  })
  assert.match(mismatch, /当前占用进程为 node\.exe（PID 21436）。/)
  assert.match(mismatch, /远程 · 端口 3080/, 'the running config must be named')
  assert.match(mismatch, /保存的配置（本地 · 端口 3080）/, 'the saved config must be named')
  assert.match(mismatch, /清理后将按保存的配置（本地 · 端口 3080）重新启动/)

  // Even a consistent resident dsh cannot be adopted: the launcher does not
  // hold its token, so the copy must say why cleanup is still the way in.
  const match = describeRuntimePortConflict({
    ...base,
    kind: 'dsh-match',
    occupantListenScope: 'local',
  })
  assert.match(match, /配置与保存的一致/)
  assert.match(match, /访问凭据/)
  assert.match(match, /重新启动/)

  const unknown = describeRuntimePortConflict({ ...base, kind: 'dsh-unknown' })
  assert.match(unknown, /无法确认其运行配置/)

  const other = describeRuntimePortConflict({ ...base, kind: 'other-program' })
  assert.doesNotMatch(other, /dsh 服务/, 'a non-dsh occupant must not be called a dsh')
  assert.match(other, /改用其它端口/, 'the non-destructive alternative stays')

  const supervised = describeRuntimePortConflict({ ...base, kind: 'supervised' })
  assert.match(supervised, /启动器启动/)
  assert.match(supervised, /「关闭」/, 'the managed service points at 关闭, not cleanup')
})
