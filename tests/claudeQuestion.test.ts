import assert from 'node:assert/strict'
import test from 'node:test'
import {
  encodeClaudeQuestionResponseWrites,
  parseClaudeAskUserQuestions,
} from '../src/utils/claudeQuestion.ts'

test('AskUserQuestion input preserves question labels, descriptions, and selection mode', () => {
  assert.deepEqual(
    parseClaudeAskUserQuestions('AskUserQuestion', {
      questions: [{
        header: 'Scope',
        question: 'Which scope should be used?',
        multiSelect: true,
        options: [
          { label: 'Project', description: 'Use the current project' },
          { label: 'Workspace' },
        ],
      }],
    }),
    [{
      header: 'Scope',
      question: 'Which scope should be used?',
      multiSelect: true,
      options: [
        { label: 'Project', description: 'Use the current project' },
        { label: 'Workspace' },
      ],
    }],
  )
})

test('single-select answers move from the default option and submit with Enter', () => {
  const questions = parseClaudeAskUserQuestions('AskUserQuestion', {
    questions: [{
      question: 'Choose one',
      options: [{ label: 'A' }, { label: 'B' }, { label: 'C' }],
    }],
  })!

  assert.deepEqual(encodeClaudeQuestionResponseWrites(questions, [[2]]), [
    '\x1b[B\x1b[B\r',
  ])
})

test('multi-select answers toggle each option before submitting', () => {
  const questions = parseClaudeAskUserQuestions('AskUserQuestion', {
    questions: [{
      question: 'Choose many',
      multiSelect: true,
      options: [{ label: 'A' }, { label: 'B' }, { label: 'C' }],
    }],
  })!

  assert.deepEqual(encodeClaudeQuestionResponseWrites(questions, [[0, 2]]), [
    ' \x1b[B\x1b[B \r',
  ])
})

test('malformed or unrelated tool input is not rendered as a question', () => {
  assert.equal(parseClaudeAskUserQuestions('Bash', { questions: [] }), null)
  assert.equal(parseClaudeAskUserQuestions('AskUserQuestion', { questions: [{}] }), null)
  assert.deepEqual(
    encodeClaudeQuestionResponseWrites(
      parseClaudeAskUserQuestions('AskUserQuestion', {
        questions: [{ question: 'Choose', options: [{ label: 'A' }] }],
      })!,
      [[]],
    ),
    [],
  )
})
