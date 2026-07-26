import { sanitizeClaudeConversationInput } from './claudeConversationInput.ts'
import type {
  ClaudeBusyInputMode,
  ClaudePromptReceipt,
  ClaudeQueuedPrompt,
} from '@/types/claudeObserver'

export const CLAUDE_PROMPT_QUEUE_LIMIT = 5

export const CLAUDE_NATIVE_QUEUE_RECALL_WRITES = [
  '\x1b[A',
  '\r',
  '\x15',
] as const

export function normalizeClaudeBusyInputMode(value: unknown): ClaudeBusyInputMode {
  return value === 'after-stop' ? 'after-stop' : 'native'
}

export function normalizeClaudePromptForMatch(value: string): string {
  return sanitizeClaudeConversationInput(value).trim()
}

export function createClaudeQueuedPrompt(
  id: string,
  text: string,
  mode: ClaudeBusyInputMode,
): ClaudeQueuedPrompt | null {
  const sanitized = sanitizeClaudeConversationInput(text)
  if (!sanitized.trim()) return null
  return {
    id,
    text: sanitized,
    mode,
    delivery: 'queued',
  }
}

export function takeMatchingClaudePromptReceipt(
  receipts: ClaudePromptReceipt[],
  prompt: string,
): ClaudePromptReceipt | undefined {
  const normalizedPrompt = normalizeClaudePromptForMatch(prompt)
  const index = receipts.findIndex(receipt => (
    normalizeClaudePromptForMatch(receipt.text) === normalizedPrompt
  ))
  if (index < 0) return undefined
  return receipts.splice(index, 1)[0]
}

export function findMatchingClaudeNativePrompt(
  prompts: ClaudeQueuedPrompt[],
  prompt: string,
): ClaudeQueuedPrompt | undefined {
  const normalizedPrompt = normalizeClaudePromptForMatch(prompt)
  return prompts.find(item => (
    item.mode === 'native'
    && normalizeClaudePromptForMatch(item.text) === normalizedPrompt
  ))
}
