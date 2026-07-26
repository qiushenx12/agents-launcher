const scrollPositions = new Map<string, number>()

export function getClaudeConversationScroll(sessionId: string) {
  return scrollPositions.get(sessionId)
}

export function saveClaudeConversationScroll(sessionId: string, scrollTop: number) {
  scrollPositions.set(sessionId, Math.max(0, scrollTop))
}

export function clearClaudeConversationScroll(sessionId: string) {
  scrollPositions.delete(sessionId)
}
