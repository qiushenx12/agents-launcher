import assert from 'node:assert/strict'
import test from 'node:test'
import {
  encodeClaudeQuestionAnswerWrites,
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

test('single-select answers move from the default option, open Submit, and confirm', () => {
  const questions = parseClaudeAskUserQuestions('AskUserQuestion', {
    questions: [{
      question: 'Choose one',
      options: [{ label: 'A' }, { label: 'B' }, { label: 'C' }],
    }],
  })!

  assert.deepEqual(encodeClaudeQuestionResponseWrites(questions, [{
    selectedOptions: [2],
  }]), [
    '\x1b[B\x1b[B\r',
    '\t',
    '\r',
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

  assert.deepEqual(encodeClaudeQuestionResponseWrites(questions, [{
    selectedOptions: [0, 2],
  }]), [
    ' \x1b[B\x1b[B ',
    '\t',
    '\r',
  ])
})

test('custom answers select Type something before writing text', () => {
  const question = parseClaudeAskUserQuestions('AskUserQuestion', {
    questions: [{
      question: 'Choose one',
      options: [{ label: 'A' }, { label: 'B' }, { label: 'C' }],
    }],
  })![0]

  assert.deepEqual(encodeClaudeQuestionAnswerWrites(question, {
    selectedOptions: [],
    customText: 'A different answer',
  }, 'submit'), [
    '\x1b[B\x1b[B\x1b[B\r',
    'A different answer\r',
    '\t',
    '\r',
  ])
})

test('each question can be encoded independently for sequential submission', () => {
  const questions = parseClaudeAskUserQuestions('AskUserQuestion', {
    questions: [
      { question: 'First?', options: [{ label: 'A' }, { label: 'B' }] },
      { question: 'Second?', options: [{ label: 'C' }, { label: 'D' }] },
    ],
  })!

  assert.deepEqual(encodeClaudeQuestionAnswerWrites(questions[0], {
    selectedOptions: [1],
  }, 'next'), ['\x1b[B\r', '\t'])
  assert.deepEqual(encodeClaudeQuestionAnswerWrites(questions[1], {
    selectedOptions: [0],
  }, 'submit'), ['\r', '\t', '\r'])
})

test('multi-question responses tab through every question before confirming Submit', () => {
  const questions = parseClaudeAskUserQuestions('AskUserQuestion', {
    questions: [
      {
        question: 'First?',
        multiSelect: true,
        options: [{ label: 'A' }, { label: 'B' }],
      },
      {
        question: 'Second?',
        options: [{ label: 'C' }, { label: 'D' }],
      },
    ],
  })!

  assert.deepEqual(encodeClaudeQuestionResponseWrites(questions, [
    { selectedOptions: [0, 1] },
    { selectedOptions: [1] },
  ]), [
    ' \x1b[B ',
    '\t',
    '\x1b[B\r',
    '\t',
    '\r',
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
      [{ selectedOptions: [] }],
    ),
    null,
  )
})
