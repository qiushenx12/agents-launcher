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

export interface ClaudeObserverStatus {
  tabId: number
  statusRevision: number
  captureId?: string | null
  available: boolean
  active: boolean
  degradedReason?: string | null
  logDir?: string | null
  terminalPrompt?: ClaudeTerminalPrompt | null
  activityStatus?: ClaudeActivityStatus | null
  currentModel?: string | null
  permissionMode?: string | null
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
}

export interface ClaudeConversationState {
  tabId: number
  statusRevision: number
  captureId?: string
  available: boolean
  active: boolean
  sessionReady: boolean
  degradedReason?: string
  logDir?: string
  runState: ClaudeConversationRunState
  items: ClaudeConversationItem[]
  terminalLog: string
  loading: boolean
  terminalPrompt?: ClaudeTerminalPrompt | null
  activityStatus?: ClaudeActivityStatus | null
  compactCompletionRevision: number
  currentModel?: string
  permissionMode?: string
  pendingPermissionMode?: 'bypassPermissions' | 'auto' | 'default' | 'acceptEdits' | 'plan'
  currentContext?: ClaudeModelContext
  queuedPrompts: ClaudeQueuedPrompt[]
  queueActionPending: boolean
}
