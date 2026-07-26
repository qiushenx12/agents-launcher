const BRACKETED_PASTE_START = '\x1b[200~'
const BRACKETED_PASTE_END = '\x1b[201~'

export function sanitizeClaudeConversationInput(input: string): string {
  const normalized = input.replace(/\r\n?/g, '\n')
  let output = ''
  for (const character of normalized) {
    const code = character.codePointAt(0) ?? 0
    const isPrintable = code >= 0x20 && code !== 0x7f && !(code >= 0x80 && code <= 0x9f)
    if (isPrintable || character === '\n' || character === '\t') {
      output += character
    }
  }
  return output
}

export function encodeClaudeConversationInput(input: string): string {
  const sanitized = sanitizeClaudeConversationInput(input)
  if (!sanitized.trim()) return ''
  if (sanitized.includes('\n') || sanitized.includes('\t')) {
    return `${BRACKETED_PASTE_START}${sanitized}${BRACKETED_PASTE_END}\r`
  }
  return `${sanitized}\r`
}

export function encodeClaudeConversationInputWrites(input: string): string[] {
  const encoded = encodeClaudeConversationInput(input)
  if (!encoded) return []
  return [encoded.slice(0, -1), '\r']
}
