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

/*
 * ── DeepSeek Harness 设置文档里的「供应商与模型」 ──────────────────────────
 *
 * 对齐 dsh **v0.1.5-rc.1** 的 `$DSH_HOME/settings.yaml`（`llm-pi-ai:` 分区）。
 * dsh 仍在快速迭代（官方自述 developer preview、会有 breaking changes），
 * 字段名与层级都可能随版本变动。
 *
 * **如果这里失效了**（新增供应商报错、写出的字段 dsh 不认、读不到已有供应商），
 * 去官方仓库 https://github.com/deepseek-ai/deepseek-harness 看
 * `docs/user/guide/providers.zh.md` 与 `docs/config-catalog.zh.md`，或包内
 * `@deepseek-ai/dsh-llm-pi-ai/README.zh.md` 与 `lib/types/catalog.d.ts`，
 * 找到当前版本的实现后同步 Rust 侧的 `src-tauri/src/dsh_settings.rs`。
 *
 * Rust 侧以 camelCase 序列化，所以 `base_url` 到这里是 `baseUrl`（不是 `baseURL`
 * ——那是 YAML 里的字段名，只在写文件时出现）。
 */

/** dsh/pi-ai 允许的思考档位，升序。与 Rust 侧的 `THINKING_LEVELS` 一致。 */
export const DSH_THINKING_LEVELS = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'] as const
export type DshThinkingLevel = (typeof DSH_THINKING_LEVELS)[number]

/**
 * dsh 允许手工声明的**全部** wire 协议，顺序即 dsh 的默认顺序（第一个是推荐值）。
 *
 * 这是个封闭集合，不是「常用的几个」：dsh 的配置 schema 写的就是
 * `api: z.union(supportedProtocols())`，值落在集合外会被直接拒绝服务。
 * pi-ai 上游还有 Bedrock / Vertex / Azure / Codex 等协议，dsh 刻意不收——
 * 它们的认证方式（region、project、api-version、OAuth）不是「密钥 + 端点 +
 * 标头」这套配置能表达的，那些协议只能通过目录里已有的供应商到达。
 *
 * 界面上的下拉框直接 v-for 这份列表，因此这里加一个值就等于放开一个选项。
 */
export const DSH_API_PROTOCOLS = [
  'openai-completions',
  'openai-responses',
  'anthropic-messages',
] as const

/**
 * 一个思考档位的声明。
 *
 * `level` 是 dsh 的档位 ID（选择器里显示的就是它），`wire` 才是真正发给网关的
 * 值——两者可以不同，例如把 `max` 映射成网关自己的 `ultra`。
 */
export interface DshReasoningLevel {
  level: DshThinkingLevel
  /** 只有 `off` 允许为空：表示「支持这一档，但不发送任何参数」。 */
  wire: string | null
}

/** 一个模型条目。 */
export interface DshModelProfile {
  id: string
  /**
   * 模型显示名。**界面不编辑**，理由与 `DshProviderProfile.displayName` 相同：dsh 在
   * 它缺失时回落到模型 ID（`entry.name ?? base?.name ?? entry.id`），不是必填项。
   * 目录里的模型多半带着它，所以仍留在读写路径上原样往返。
   */
  name: string | null
  contextWindow: number | null
  maxTokens: number | null
  /** 请求模态，例如 `['text', 'image']`。留空表示不声明（dsh 按纯文本对待）。 */
  input: string[] | null
  /** 显式声明为非推理模型（写 `reasoningEfforts: false`）。 */
  reasoningDisabled: boolean
  reasoningEfforts: DshReasoningLevel[]
  /** 启动器不编辑的字段名，只用于界面提示。 */
  unmanagedFields: string[]
}

/** 一个供应方路由。 */
export interface DshProviderProfile {
  /** 路由键。dsh 用它选路由并派生凭据记录键，只能用小写字母、数字、连字符。 */
  id: string
  /**
   * 显示名称。**界面不编辑这个字段**：dsh 在它缺失时回落到路由键
   * （`dsh-llm-pi-ai` 的 `source.displayName ?? provider`），所以它只是一个可选的
   * 美化名，用户没有必须做出的决定；编辑器标题栏与左侧列表显示的就是回落后的结果。
   * 要改美化名去 dsh 的「设置 → 模型」页。
   *
   * 留在读写路径上同样是为了原样往返——目录里的供应商多半带着这个名字。
   * 注意 dsh **拒绝空字符串**（`displayName.length === 0` 直接抛错），所以「留空」
   * 在文件里的合法形态是该行整行不存在。
   */
  displayName: string | null
  /** wire 协议；pi-ai 目录里已有的路由可以留空。 */
  api: string | null
  baseUrl: string | null
  /**
   * 凭据引用名（环境变量名）。**这里存的是引用，不是密钥**——令牌的**值**放在
   * `$DSH_HOME/.credentials.yaml` 的 `refs` 分节，由启动器的「认证令牌」栏写入；
   * settings.yaml 里只有引用名。
   *
   * **界面不给引用名输入框**：保存令牌时沿用文件里已有的名字；没有才按 dsh 的
   * 派生规则（`deriveDshCredentialRef`）生成，并由后端把这一行补写进 settings.yaml。
   * 需要引用的名字不是派生出来的那个（例如环境里已有 `MY_COMPANY_KEY`，或密钥走
   * 系统环境变量 / `$DSH_HOME/.env`）时，直接改 settings.yaml。名字本身必须是
   * POSIX shell 变量名，后端写入闸门 `is_credential_ref_name()` 会拦。
   */
  apiKeyEnv: string | null
  /** 路由级默认思考档。 */
  reasoning: string | null
  models: DshModelProfile[]
  /** 启动器不编辑的字段名（`compat` / `headers` / `retryPolicy` …）。 */
  unmanagedFields: string[]
  /**
   * 是否是「自定义」路由：pi-ai 内置目录在该路由键下不提供任何内容，协议、端点、
   * 模型都由 settings.yaml 声明。与 dsh 自己设置页上那枚「自定义」标签同义。
   *
   * **不是文件里的字段**：后端读取时按已安装的 pi-ai 目录清单算出来（见
   * `src-tauri/src/dsh_settings.rs` 的 `mark_custom_routes`），写回时忽略。左侧清单
   * 只列这类路由——目录路由（例如只填了 `apiKeyEnv` 的 `kimi-coding`）的字段全由
   * dsh 自带目录提供，在这里没有可编辑的内容。新建的草稿一律按自定义处理。
   */
  custom: boolean
}

export interface DshSettingsDocument {
  path: string
  exists: boolean
  /** 文件内容的 sha256，写回时必须原样带回。 */
  revision: string
  supportedVersion: string
  providers: DshProviderProfile[]
}

export interface DshSettingsWriteResult {
  path: string
  revision: string
  backupPath: string
}

export interface DshCredentialSaveResult {
  /** 令牌实际存入的凭据引用名。 */
  refName: string
  /** settings.yaml 被补写 `apiKeyEnv` 之后的修订；没动 settings.yaml 时为 null。 */
  settingsRevision: string | null
}

/**
 * dsh 自己的凭据引用名派生规则（`dsh-client-ui-settings-models/lib/client.js` 的
 * `deriveKeyRef`）：路由键大写、**连续的**非字母数字折叠成一个 `_`、后缀
 * `_API_KEY`。启动器保存令牌时用它生成引用名；名字不是合法的 POSIX 变量名时
 * 后端会拒绝并要求手写 apiKeyEnv。
 */
export function deriveDshCredentialRef(routeKey: string): string {
  return `${routeKey.toUpperCase().replace(/[^A-Z0-9]+/g, '_')}_API_KEY`
}
