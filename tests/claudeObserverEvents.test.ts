import assert from 'node:assert/strict'
import test from 'node:test'
import {
  claudeObservedModelSwitchApplied,
  parseClaudeModelCommandResult,
  reduceClaudeAgentEvents,
} from '../src/utils/claudeObserverEvents.ts'
import type { ClaudeAgentEvent } from '../src/types/claudeObserver.ts'

function event(
  id: string,
  eventName: string,
  payload: Record<string, unknown> = {},
): ClaudeAgentEvent {
  return {
    id,
    sequence: Number(id),
    captureId: 'capture-1',
    tabId: 1,
    eventName,
    receivedAt: `2026-07-21T00:00:0${id}.000Z`,
    payload: { hook_event_name: eventName, ...payload },
  }
}

test('MessageDisplay batches merge into one assistant message', () => {
  const reduced = reduceClaudeAgentEvents([
    event('1', 'MessageDisplay', { message_id: 'message-1', index: 0, final: false, delta: '第一行\n' }),
    event('2', 'MessageDisplay', { message_id: 'message-1', index: 1, final: true, delta: '第二行' }),
  ])

  assert.equal(reduced.items.length, 1)
  assert.equal(reduced.items[0].kind, 'assistant')
  assert.equal(reduced.items[0].text, '第一行\n第二行')
})

test('duplicate hook delivery is ignored by event id', () => {
  const userPrompt = event('1', 'UserPromptSubmit', { prompt: 'hello' })
  const reduced = reduceClaudeAgentEvents([userPrompt, userPrompt])

  assert.equal(reduced.items.length, 1)
  assert.equal(reduced.items[0].text, 'hello')
})

test('replayed MessageDisplay index is not appended twice', () => {
  const reduced = reduceClaudeAgentEvents([
    event('1', 'MessageDisplay', { message_id: 'message-1', index: 0, final: false, delta: '一次' }),
    event('2', 'MessageDisplay', { message_id: 'message-1', index: 0, final: false, delta: '一次' }),
    event('3', 'MessageDisplay', { message_id: 'message-1', index: 1, final: true, delta: '完成' }),
  ])

  assert.equal(reduced.items[0].text, '一次完成')
})

test('tool start and result become one completed card', () => {
  const reduced = reduceClaudeAgentEvents([
    event('1', 'PreToolUse', {
      tool_use_id: 'tool-1',
      tool_name: 'Bash',
      tool_input: { command: 'npm test' },
    }),
    event('2', 'PostToolUse', {
      tool_use_id: 'tool-1',
      tool_name: 'Bash',
      tool_response: { stdout: 'ok' },
    }),
  ])

  assert.equal(reduced.items.length, 1)
  assert.equal(reduced.items[0].toolName, 'Bash')
  assert.equal(reduced.items[0].state, 'success')
  assert.deepEqual(reduced.items[0].toolResult, { stdout: 'ok' })
})

test('AskUserQuestion becomes an inline waiting card and resumes after its result', () => {
  const reduced = reduceClaudeAgentEvents([
    event('1', 'PreToolUse', {
      tool_use_id: 'question-1',
      tool_name: 'AskUserQuestion',
      tool_input: {
        questions: [{
          question: 'Which layout should be used?',
          options: [{ label: 'Thumbnail strip' }, { label: 'Inline file icon' }],
        }],
      },
    }),
  ])

  assert.equal(reduced.runState, 'permission')
  assert.equal(reduced.items.length, 1)
  assert.equal(reduced.items[0].kind, 'tool')
  assert.equal(reduced.items[0].state, 'waiting')
  assert.equal(reduced.items[0].toolName, 'AskUserQuestion')

  const completed = reduceClaudeAgentEvents([
    event('1', 'PreToolUse', {
      tool_use_id: 'question-1',
      tool_name: 'AskUserQuestion',
      tool_input: {
        questions: [{
          question: 'Which layout should be used?',
          options: [{ label: 'Thumbnail strip' }, { label: 'Inline file icon' }],
        }],
      },
    }),
    event('2', 'PostToolUse', {
      tool_use_id: 'question-1',
      tool_name: 'AskUserQuestion',
      tool_response: { answers: { 'Which layout should be used?': 'Thumbnail strip' } },
    }),
  ])

  assert.equal(completed.runState, 'working')
  assert.equal(completed.items[0].state, 'success')
})

test('permission request locks structured input until terminal confirmation', () => {
  const reduced = reduceClaudeAgentEvents([
    event('1', 'PermissionRequest', { tool_name: 'Edit' }),
  ])

  assert.equal(reduced.runState, 'permission')
  assert.equal(reduced.items[0].kind, 'permission')
})

test('AskUserQuestion permission request does not duplicate its inline question card', () => {
  const questions = [{
    question: 'Which layout should be used?',
    options: [{ label: 'Thumbnail strip' }, { label: 'Inline file icon' }],
  }]
  const reduced = reduceClaudeAgentEvents([
    event('1', 'PreToolUse', {
      tool_use_id: 'question-1',
      tool_name: 'AskUserQuestion',
      tool_input: { questions },
    }),
    event('2', 'PermissionRequest', {
      tool_name: 'AskUserQuestion',
      tool_input: { questions },
    }),
  ])

  assert.equal(reduced.runState, 'permission')
  assert.equal(reduced.items.length, 1)
  assert.equal(reduced.items[0].kind, 'tool')
  assert.equal(reduced.items[0].state, 'waiting')
})

test('out-of-order snapshot and live events reduce by backend sequence', () => {
  const prompt = event('1', 'UserPromptSubmit', { prompt: 'hello' })
  const answer = event('2', 'MessageDisplay', {
    message_id: 'message-1', index: 0, final: true, delta: 'done',
  })
  const stop = event('3', 'Stop')
  const reduced = reduceClaudeAgentEvents([stop, prompt, answer])

  assert.deepEqual(reduced.items.map(item => item.kind), ['user', 'assistant'])
  assert.equal(reduced.runState, 'idle')
})

test('resumed transcript messages appear before the new session starts', () => {
  const reduced = reduceClaudeAgentEvents([
    event('3', 'SessionStart', { source: 'resume' }),
    event('1', 'HistoricalUserMessage', { text: 'earlier question', historical: true }),
    event('2', 'HistoricalAssistantMessage', { text: 'earlier answer', historical: true }),
  ])

  assert.deepEqual(reduced.items.map(item => item.kind), ['user', 'assistant'])
  assert.deepEqual(reduced.items.map(item => item.text), ['earlier question', 'earlier answer'])
  assert.equal(reduced.runState, 'idle')
})

test('resumed transcript preserves tool execution between assistant messages', () => {
  const reduced = reduceClaudeAgentEvents([
    event('1', 'HistoricalAssistantMessage', { text: 'checking', historical: true }),
    event('2', 'PreToolUse', {
      tool_use_id: 'tool-1', tool_name: 'Grep', tool_input: { pattern: 'public camera' }, historical: true,
    }),
    event('3', 'PostToolUse', {
      tool_use_id: 'tool-1', tool_name: 'Grep', tool_response: 'one match', historical: true,
    }),
    event('4', 'HistoricalAssistantMessage', { text: 'updated', historical: true }),
    event('5', 'SessionStart', { source: 'resume' }),
  ])

  assert.deepEqual(reduced.items.map(item => item.kind), ['assistant', 'tool', 'assistant'])
  assert.equal(reduced.items[1].toolName, 'Grep')
  assert.deepEqual(reduced.items[1].toolInput, { pattern: 'public camera' })
  assert.equal(reduced.items[1].toolResult, 'one match')
  assert.equal(reduced.items[1].state, 'success')
  assert.equal(reduced.runState, 'idle')
})

test('SessionEnd does not stop a still-running Claude terminal', () => {
  const reduced = reduceClaudeAgentEvents([
    event('1', 'SessionStart', { source: 'resume' }),
    event('2', 'SessionEnd', { reason: 'clear' }),
  ])

  assert.equal(reduced.runState, 'idle')
})

test('clear session start removes the previous logical conversation and stays editable', () => {
  const reduced = reduceClaudeAgentEvents([
    event('1', 'UserPromptSubmit', { prompt: 'old question' }),
    event('2', 'MessageDisplay', {
      message_id: 'message-1', index: 0, final: true, delta: 'old answer',
    }),
    event('3', 'Stop'),
    event('4', 'SessionEnd', { reason: 'clear' }),
    event('5', 'SessionStart', { source: 'clear' }),
  ])

  assert.deepEqual(reduced.items, [])
  assert.equal(reduced.runState, 'idle')
})

test('model command transcript becomes one concise status item', () => {
  const reduced = reduceClaudeAgentEvents([
    event('1', 'HistoricalUserMessage', {
      text: '<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args></command-args>',
      historical: true,
    }),
    event('2', 'HistoricalUserMessage', {
      text: '<local-command-stdout>Set model to \u001b[1mSonnet 5\u001b[22m and saved as your default for new sessions\n.claude\\settings.json pins Opus 4.8</local-command-stdout>',
      historical: true,
    }),
    event('3', 'HistoricalAssistantMessage', {
      text: 'No response requested.',
      historical: true,
    }),
  ])

  assert.equal(reduced.items.length, 1)
  assert.equal(reduced.items[0].kind, 'status')
  assert.equal(reduced.items[0].text, '模型已切换为 Sonnet 5')
})

test('compact command transcript becomes one centered status item', () => {
  const reduced = reduceClaudeAgentEvents([
    event('1', 'HistoricalUserMessage', {
      text: '<command-name>/compact</command-name>\n<command-message>compact</command-message>\n<command-args></command-args>',
      historical: true,
    }),
    event('2', 'HistoricalLocalCommand', {
      text: '<local-command-stdout>\u001b[2mCompacted (ctrl+o to see full summary)\u001b[22m</local-command-stdout>',
      historical: true,
    }),
    event('3', 'HistoricalAssistantMessage', {
      text: 'No response requested.',
      historical: true,
    }),
  ])

  assert.deepEqual(reduced.items.map(item => ({ kind: item.kind, text: item.text })), [{
    kind: 'status',
    text: '已完成上下文压缩',
  }])
})

test('compact no-op terminal echo becomes a centered status item', () => {
  const reduced = reduceClaudeAgentEvents([
    event('1', 'UserPromptSubmit', {
      prompt: '❯ /compact\n  ⎿  Not enough messages to compact.',
    }),
  ])

  assert.deepEqual(reduced.items.map(item => ({ kind: item.kind, text: item.text })), [{
    kind: 'status',
    text: '当前消息不足，无需压缩',
  }])
  assert.equal(reduced.runState, 'idle')
})

test('live model command and no-response placeholder stay out of conversation history', () => {
  const reduced = reduceClaudeAgentEvents([
    event('1', 'UserPromptSubmit', { prompt: '/model' }),
    event('2', 'MessageDisplay', {
      message_id: 'message-1',
      index: 0,
      final: true,
      delta: 'No response requested.',
    }),
  ])

  assert.equal(reduced.items.length, 0)
})

test('local model completion immediately creates a concise status', () => {
  const reduced = reduceClaudeAgentEvents([
    event('1', 'SessionStart'),
    event('2', 'ModelSwitchCompleted', { model: 'Haiku 4.5', changed: true }),
  ])

  assert.equal(reduced.runState, 'idle')
  assert.equal(reduced.items.length, 1)
  assert.equal(reduced.items[0].text, '模型已切换为 Haiku 4.5')
})

test('model result parser reads the latest raw terminal result and strips ANSI', () => {
  const result = parseClaudeModelCommandResult([
    'Kept model as Opus 4.8',
    'Set model to \u001b[1mSonnet 5\u001b[22m and saved as your default for new sessions',
  ].join('\n'))

  assert.deepEqual(result, {
    model: 'Sonnet 5',
    changed: true,
    text: '模型已切换为 Sonnet 5',
    context: '200k',
  })
})

test('model result separates the true model name from context and truncated save text', () => {
  assert.deepEqual(
    parseClaudeModelCommandResult('Set model to Opus 4.8 (1M context) (default) and saved as your default for new sessions'),
    {
      model: 'Opus 4.8',
      changed: true,
      text: '模型已切换为 Opus 4.8',
      context: '1m',
    },
  )
  assert.deepEqual(
    parseClaudeModelCommandResult('Set model to Sonnet 5 and saved as your default f...'),
    {
      model: 'Sonnet 5',
      changed: true,
      text: '模型已切换为 Sonnet 5',
      context: '200k',
    },
  )
})

test('observed footer model decides whether the switch applied to this session', () => {
  assert.equal(
    claudeObservedModelSwitchApplied('Haiku 4.5', 'Sonnet 5', 'Sonnet 5'),
    false,
  )
  assert.equal(
    claudeObservedModelSwitchApplied('Opus 4.8', 'Opus 4.8', 'Sonnet 5'),
    true,
  )
  assert.equal(
    claudeObservedModelSwitchApplied('claude-sonnet-5', 'Sonnet 5', 'Haiku 4.5'),
    true,
  )
})
