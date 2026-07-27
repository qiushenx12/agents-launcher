export interface ClaudeQuestionOption {
  label: string
  description?: string
}

export interface ClaudeAskUserQuestion {
  question: string
  header?: string
  options: ClaudeQuestionOption[]
  multiSelect: boolean
}

export interface ClaudeQuestionAnswer {
  selectedOptions: number[]
  customText?: string
}

function record(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined
}

function nonEmptyString(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value : undefined
}

export function parseClaudeAskUserQuestions(
  toolName: string | undefined,
  input: unknown,
): ClaudeAskUserQuestion[] | null {
  if (toolName?.toLowerCase() !== 'askuserquestion') return null
  const inputRecord = record(input)
  if (!inputRecord || !Array.isArray(inputRecord.questions)) return null

  const questions: ClaudeAskUserQuestion[] = []
  for (const value of inputRecord.questions) {
    const questionRecord = record(value)
    const question = nonEmptyString(questionRecord?.question)
    if (!question || !Array.isArray(questionRecord?.options)) return null

    const options: ClaudeQuestionOption[] = []
    for (const option of questionRecord.options) {
      const optionRecord = record(option)
      const label = nonEmptyString(optionRecord?.label)
      if (!label) return null
      const description = nonEmptyString(optionRecord?.description)
      options.push({ label, ...(description ? { description } : {}) })
    }
    if (options.length === 0) return null

    const header = nonEmptyString(questionRecord.header)
    questions.push({
      question,
      ...(header ? { header } : {}),
      options,
      multiSelect: questionRecord.multiSelect === true,
    })
  }

  return questions.length > 0 ? questions : null
}

function validSelection(
  question: ClaudeAskUserQuestion,
  selection: number[] | undefined,
): number[] | undefined {
  if (!selection || selection.length === 0) return undefined
  const unique = [...new Set(selection)].sort((left, right) => left - right)
  if (unique.some(index => index < 0 || index >= question.options.length)) return undefined
  if (!question.multiSelect && unique.length !== 1) return undefined
  return unique
}

function validCustomText(value: string | undefined): string | undefined {
  const text = value?.trim()
  return text ? text : undefined
}

/**
 * Claude Code's active question starts on its first option. Return only the
 * writes for that question so the caller can advance in lockstep with the TUI.
 */
export function encodeClaudeQuestionAnswerWrites(
  question: ClaudeAskUserQuestion,
  answer: ClaudeQuestionAnswer,
): string[] | null {
  const customText = validCustomText(answer.customText)
  if (customText) {
    // Claude Code appends "Type something" after the supplied options.
    // Confirm it first, then enter the text in the dedicated input field.
    return ['\x1b[B'.repeat(question.options.length) + '\r', customText + '\r']
  }

  const selected = validSelection(question, answer.selectedOptions)
  if (!selected) return null

  let cursor = 0
  let input = ''
  for (const optionIndex of selected) {
    input += '\x1b[B'.repeat(optionIndex - cursor)
    if (question.multiSelect) input += ' '
    cursor = optionIndex
  }
  input += '\r'
  return [input]
}

export function encodeClaudeQuestionResponseWrites(
  questions: ClaudeAskUserQuestion[],
  answers: ClaudeQuestionAnswer[],
): string[] | null {
  if (questions.length === 0 || questions.length !== answers.length) return null

  const writes: string[] = []
  for (const [questionIndex, question] of questions.entries()) {
    const answer = answers[questionIndex]
    if (!answer) return null
    const questionWrites = encodeClaudeQuestionAnswerWrites(question, answer)
    if (!questionWrites) return null
    writes.push(...questionWrites)
  }
  return writes
}
