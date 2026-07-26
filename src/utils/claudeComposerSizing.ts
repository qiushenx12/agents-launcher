export const CLAUDE_COMPOSER_MIN_ROWS = 2
export const CLAUDE_COMPOSER_MAX_ROWS = 9

const FALLBACK_LINE_HEIGHT = 22

export interface ClaudeComposerTextareaMetrics {
  height: number
  overflowY: 'auto' | 'hidden'
}

export function getClaudeComposerTextareaMetrics(
  scrollHeight: number,
  lineHeight: number,
): ClaudeComposerTextareaMetrics {
  const safeLineHeight = Number.isFinite(lineHeight) && lineHeight > 0
    ? lineHeight
    : FALLBACK_LINE_HEIGHT
  const minHeight = safeLineHeight * CLAUDE_COMPOSER_MIN_ROWS
  const maxHeight = safeLineHeight * CLAUDE_COMPOSER_MAX_ROWS
  const safeScrollHeight = Number.isFinite(scrollHeight) && scrollHeight >= 0
    ? scrollHeight
    : minHeight

  return {
    height: Math.min(maxHeight, Math.max(minHeight, safeScrollHeight)),
    overflowY: safeScrollHeight > maxHeight + 0.5 ? 'auto' : 'hidden',
  }
}
