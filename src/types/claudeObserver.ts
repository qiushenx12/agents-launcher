export type ClaudeConversationItemKind =
  | 'user'
  | 'assistant'
  | 'tool'
  | 'permission'
  | 'status'

export type ClaudeConversationRunState =
  | 'starting'
  | 'idle'
  | 'working'
  | 'permission'
  | 'stopped'

export type ClaudeBusyInputMode = 'native' | 'after-stop'
export type ClaudeModelContext = '200k' | '1m'

export interface ClaudeQueuedPrompt {
  id: string
  text: string
  mode: ClaudeBusyInputMode
  delivery: 'queued' | 'native' | 'sending'
}

export interface ClaudePromptReceipt {
  text: string
  kind: 'direct' | 'native-queue'
  queuedPromptId?: string
  terminalLogBaseline?: string
}

export interface ClaudePromptSubmissionBaseline {
  captureId: string
  eventSequence: number
  transcriptLen?: number | null
}

export interface ClaudeTerminalLogResult {
  text: string
  logDir: string
  historical: boolean
}

export interface ClaudeAgentEvent {
  id: string
  sequence: number
  captureId: string
  tabId?: number
  eventName: string
  receivedAt: string
  payload: Record<string, unknown>
}

export interface ClaudeWorkspaceTrustPrompt {
  kind: 'workspaceTrust'
  path: string
}

export interface ClaudePluginInstallPrompt {
  kind: 'pluginInstall'
  pluginName: string
  prompt: string
  options: string[]
}

export interface ClaudeModelSwitchConfirmPrompt {
  kind: 'modelSwitchConfirm'
  prompt: string
  options: string[]
  selectedIndex: number
}

export interface ClaudePlanApprovalPrompt {
  kind: 'planApproval'
  prompt: string
  options: string[]
  selectedIndex: number
}

export type ClaudeTerminalPrompt =
  | ClaudeWorkspaceTrustPrompt
  | ClaudePluginInstallPrompt
  | ClaudeModelSwitchConfirmPrompt
  | ClaudePlanApprovalPrompt

export interface ClaudeActivityStatus {
  label: string
  elapsed?: string | null
  tokenDirection?: '↑' | '↓' | null
  tokenCount?: string | null
  phase?: string | null
}

export interface ClaudeSubagentActivityStatus {
  agentType: string
  description: string
  elapsed?: string | null
  tokenDirection?: '↑' | '↓' | null
  tokenCount?: string | null
}

export interface ClaudeContextUsage {
  usedPercentage: number
  remainingPercentage: number
  usedTokens?: number | null
  contextWindowSize?: number | null
  source: 'native' | 'transcript'
  updatedAt: string
}

export interface ClaudeObserverStatus {
  tabId: number
  statusRevision: number
  captureId?: string | null
  available: boolean
  active: boolean
  degradedReason?: string | null
  terminalError?: string | null
  logDir?: string | null
  terminalPrompt?: ClaudeTerminalPrompt | null
  activityStatus?: ClaudeActivityStatus | null
  subagentActivities?: ClaudeSubagentActivityStatus[] | null
  currentModel?: string | null
  permissionMode?: string | null
  contextUsage?: ClaudeContextUsage | null
}

export interface ClaudeObserverSnapshot extends ClaudeObserverStatus {
  events: ClaudeAgentEvent[]
  terminalLog: string
}

export interface ClaudeConversationItem {
  id: string
  eventId: string
  kind: ClaudeConversationItemKind
  eventName: string
  timestamp: string
  text?: string
  toolName?: string
  toolInput?: unknown
  toolResult?: unknown
  state?: 'running' | 'success' | 'failed' | 'waiting'
  messageKey?: string
  subagentId?: string
  subagentType?: string
  subagentDescription?: string
  subagentRunMode?: 'foreground' | 'background'
  subagentTools?: ClaudeSubagentToolUse[]
  subagentTotalToolUseCount?: number
}

export interface ClaudeSubagentToolUse {
  id: string
  toolName: string
  toolInput?: unknown
  state: 'running' | 'success' | 'failed'
  timestamp: string
}

export interface ClaudeConversationState {
  tabId: number
  statusRevision: number
  captureId?: string
  available: boolean
  active: boolean
  sessionReady: boolean
  degradedReason?: string
  terminalError?: string
  logDir?: string
  runState: ClaudeConversationRunState
  items: ClaudeConversationItem[]
  terminalLog: string
  loading: boolean
  terminalPrompt?: ClaudeTerminalPrompt | null
  activityStatus?: ClaudeActivityStatus | null
  subagentActivities?: ClaudeSubagentActivityStatus[] | null
  compactCompletionRevision: number
  currentModel?: string
  permissionMode?: string
  pendingPermissionMode?: 'bypassPermissions' | 'auto' | 'default' | 'acceptEdits' | 'plan'
  currentContext?: ClaudeModelContext
  contextUsage?: ClaudeContextUsage
  submittedQuestionIds: string[]
  queuedPrompts: ClaudeQueuedPrompt[]
  queueActionPending: boolean
}
