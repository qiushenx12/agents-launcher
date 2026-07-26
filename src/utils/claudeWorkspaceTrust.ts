export type ClaudeWorkspaceTrustAction = 'confirm' | 'cancel'

export type ClaudeWorkspaceTrustResolution =
  | { kind: 'confirm-in-terminal' }
  | { kind: 'reject-and-close-terminal' }

export function resolveClaudeWorkspaceTrustAction(
  action: ClaudeWorkspaceTrustAction,
): ClaudeWorkspaceTrustResolution {
  return action === 'confirm'
    ? { kind: 'confirm-in-terminal' }
    : { kind: 'reject-and-close-terminal' }
}
