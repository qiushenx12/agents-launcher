import type { CliKind } from './cli'

export interface EnvConfig {
  name: string
  vars: Record<string, string>
}

export interface ClaudeSettings {
  skipPermissions: boolean
  awaySummaryDisabled: boolean
  sourcePath?: string
  sourceKind?: 'settings' | 'legacy' | 'missing' | string
  usingLegacyPath?: boolean
}

export interface CliProfileRef {
  cliKind: CliKind
  profileId: string
}

export type CodexAuthMode = 'official' | 'custom'

export interface CodexReasoningLevel {
  effort: string
  description: string
  [key: string]: unknown
}

export interface CodexTruncationPolicy {
  mode: string
  limit: number
  [key: string]: unknown
}

export interface CodexModelDefinition {
  slug: string
  displayName: string
  inputModalities: string[]
  supportsImageDetailOriginal: boolean
  contextWindow: number
  maxContextWindow: number
  effectiveContextWindowPercent: number
  truncationPolicy: CodexTruncationPolicy | null
  defaultReasoningLevel: string
  supportedReasoningLevels: CodexReasoningLevel[]
  [key: string]: unknown
}

export interface CodexModelCatalog {
  models: CodexModelDefinition[]
  [key: string]: unknown
}

export interface CodexProfile {
  id: string
  name: string
  authMode: CodexAuthMode
  model: string
  reasoningEffort: string
  modelContextWindow: number | null
  modelContextWindowConfigured: boolean
  modelAutoCompactRatio: number | null
  modelAutoCompactRatioConfigured: boolean
  openaiBaseUrl: string
  providerId: string
  providerName: string
  baseUrl: string
  wireApi: 'responses'
  protocolConversion: boolean
  chatUpstreamModel: string
  promptCacheRouting: 'auto' | 'enabled' | 'disabled' | string
  envKey: string
  hasStoredApiKey: boolean
  managedProfileName: string
  modelCatalog: CodexModelCatalog | null
  [key: string]: unknown
}

export interface CodexAuthStatus {
  mode: string | null
  hasAuthFile: boolean
  hasCredentials: boolean
  error: string | null
}

export type CodexSessionIssueKind =
  | 'orphaned_tail'
  | 'parent_missing'
  | 'base_beyond_end'
  | 'writer_newer_than_cli'

export interface CodexSessionIssue {
  threadId: string
  cwd: string
  projectName: string
  preview: string
  kind: CodexSessionIssueKind | string
  repairable: boolean
  orphanedBytes: number
  orphanedRecords: number
  orphanedUserMessages: number
  orphanedFirstAt: string | null
  orphanedLastAt: string | null
  pageCount: number
  writerCliVersion: string | null
  localCliVersion: string | null
  message: string
}

export interface CodexSessionRepairResult {
  threadId: string
  strategy: string
  backupDir: string
  pageCount: number
  mergedBytes: number
  keptRecords: number
  skippedRecords: number
  message: string
}

export interface CodexProfilesPayload {
  profiles: CodexProfile[]
  order: string[]
  activeProfileId: string | null
  globalProfileId: string | null
  globalProfileInSync: boolean
  globalSyncRepairRequired: boolean
  profilesPath: string
  globalConfigPath: string
  authPath: string
  globalConfigError: string | null
  authStatus: CodexAuthStatus
  customGlobalSyncSupported: boolean
  customGlobalKeySyncSupported: boolean
  secretStorageKind: 'windows_dpapi' | 'macos_plaintext' | 'unsupported'
  platform: string
  /** 切换配置前的会话完整性预检结果；仅在 apply 流程中填充 */
  sessionIssues?: CodexSessionIssue[]
  /** true 表示本次 apply 因检测到可修复的会话断链而未执行 */
  sessionIssuesBlocked?: boolean
}

export interface CodexLaunchContext {
  managedProfileName: string
  modelProvider: string
  envVars: Record<string, string>
}

export type OpencodeProviderAuthMode = 'existing' | 'managed'
export type OpencodeApiType = 'chat_completions' | 'responses'
export type OpencodeProviderKind = 'builtin' | 'custom'

export interface OpencodeHeader {
  name: string
  value: string
}

export interface OpencodeModel {
  id: string
  name: string
  contextLimit: number | null
  outputLimit: number | null
}

export interface OpencodeProvider {
  credentialId: string
  id: string
  name: string
  providerKind: OpencodeProviderKind
  authMode: OpencodeProviderAuthMode
  apiType: OpencodeApiType
  baseUrl: string
  envKey: string
  models: OpencodeModel[]
  headers: OpencodeHeader[]
  hasStoredApiKey: boolean
}

export interface OpencodeProfile {
  id: string
  name: string
  providers: OpencodeProvider[]
  model: string
  smallModel: string
  managedConfigPath: string
  [key: string]: unknown
}

export interface OpencodeProfilesPayload {
  profiles: OpencodeProfile[]
  order: string[]
  activeProfileId: string | null
  profilesPath: string
  globalConfigPath: string
  authPath: string
  modelStatePath: string
  connectedProviderIds: string[]
  providerStatusError: string | null
}

export interface OpencodeLaunchContext {
  configPath: string
  envVars: Record<string, string>
  configuredModel: string
  model: string
  smallModel: string
  providerIds: string[]
  modelSource: 'config' | 'recent' | 'provider_default' | 'none'
}

export interface OpencodeGlobalModel {
  originalId: string
  id: string
  name: string
  contextLimit: number | null
  outputLimit: number | null
  inputText: boolean
  inputImage: boolean
}

export interface OpencodeGlobalProvider {
  originalId: string
  id: string
  name: string
  npm: string
  baseUrl: string
  apiKey: string
  models: OpencodeGlobalModel[]
}

export interface OpencodeGlobalConfigPayload {
  configPath: string
  revision: string
  authPath: string
  authRevision: string
  connectedProviderIds: string[]
  disabledProviderIds: string[]
  connectionKeys: Record<string, string>
  model: string
  smallModel: string
  providers: OpencodeGlobalProvider[]
}

export interface OpencodeConnectionStatusPayload {
  authPath: string
  authRevision: string
  connectedProviderIds: string[]
  configRevision: string
  disabledProviderIds: string[]
  connectionKeys: Record<string, string>
}

export interface OpencodePermissionStatus {
  supported: boolean
  requiresRepair: boolean
  directories: string[]
  blockedDirectories: string[]
}

export interface SessionEntry {
  id: string
  display: string
  ts: number
  /** Claude Code 官方 AI 会话标题（来自会话文件的 ai-title 行），可能不存在 */
  title?: string
}

export interface WindowState {
  width: number
  height: number
  x: number
  y: number
  paneSizes: number[]
}
