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

/**
 * Claude Code's question picker starts on the first option. These are the
 * equivalent PTY key sequences for answering each question in order.
 */
export function encodeClaudeQuestionResponseWrites(
  questions: ClaudeAskUserQuestion[],
  selections: number[][],
): string[] {
  if (questions.length === 0 || questions.length !== selections.length) return []

  return questions.flatMap((question, questionIndex) => {
    const selected = validSelection(question, selections[questionIndex])
    if (!selected) return []

    let cursor = 0
    let input = ''
    for (const optionIndex of selected) {
      input += '\x1b[B'.repeat(optionIndex - cursor)
      if (question.multiSelect) input += ' '
      cursor = optionIndex
    }
    input += '\r'
    return [input]
  })
}
