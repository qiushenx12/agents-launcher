import assert from 'node:assert/strict'
import test from 'node:test'
import {
  isInteractiveTitleBarTarget,
  shouldStartTitleBarDrag,
  shouldToggleTitleBarMaximize,
} from '../src/utils/windowTitleBar.ts'

function targetMatching(matches: boolean) {
  return {
    closest() {
      return matches ? {} : null
    },
  }
}

test('blank title-bar containers can start a window drag', () => {
  assert.equal(shouldStartTitleBarDrag({
    button: 0,
    detail: 1,
    target: targetMatching(false),
  }), true)
})

test('interactive descendants never start a window drag', () => {
  const target = targetMatching(true)
  assert.equal(isInteractiveTitleBarTarget(target), true)
  assert.equal(shouldStartTitleBarDrag({ button: 0, detail: 1, target }), false)
})

test('right clicks and the second click of a double click do not start dragging', () => {
  const target = targetMatching(false)
  assert.equal(shouldStartTitleBarDrag({ button: 2, detail: 1, target }), false)
  assert.equal(shouldStartTitleBarDrag({ button: 0, detail: 2, target }), false)
})

test('double click toggles maximize only on blank title-bar content', () => {
  assert.equal(shouldToggleTitleBarMaximize({
    button: 0,
    detail: 2,
    target: targetMatching(false),
  }), true)
  assert.equal(shouldToggleTitleBarMaximize({
    button: 0,
    detail: 2,
    target: targetMatching(true),
  }), false)
})
