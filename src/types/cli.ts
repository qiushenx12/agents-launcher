import rawCliContract from '../../contracts/cli-contract.json'

export const CLI_KINDS = ['claude', 'codex', 'opencode', 'dsh'] as const
export type CliKind = typeof CLI_KINDS[number]

export type CliConfigFormat = 'json' | 'toml' | 'jsonc' | 'none'
export type CliIssueCode =
  | 'executable_missing'
  | 'version_command_failed'
  | 'version_too_old'
  | 'config_parse_failed'
  | 'authentication_missing'
  | 'provider_unreachable'
export type CliStatusState = 'checking' | 'ready' | 'blocked' | 'degraded'
export type MainTab = 'config' | CliKind | 'terminal' | 'orchestration'

export interface CliDescriptor {
  kind: CliKind
  label: 'Claude Code' | 'CodeX' | 'OpenCode' | 'DeepSeek Harness'
  command: 'claude' | 'codex' | 'opencode' | 'dsh'
  configFormat: CliConfigFormat
  supportsManagedProfile: boolean
}

export interface CliIssueDefinition {
  code: CliIssueCode
  state: Extract<CliStatusState, 'blocked' | 'degraded'>
  messages: Record<CliKind, string>
}

export interface CliStatus {
  kind: CliKind
  state: CliStatusState
  issueCode: CliIssueCode | null
  message: string
  executablePath: string | null
  version: string | null
}

export interface CliContract {
  contractVersion: number
  serializedCliKindField: 'cliKind'
  legacyDefaultCliKind: CliKind
  mainTab: {
    values: MainTab[]
    legacyAliases: Record<string, MainTab>
    unknownFallback: MainTab
  }
  cliDescriptors: CliDescriptor[]
  issueDefinitions: CliIssueDefinition[]
}

export function isCliKind(value: unknown): value is CliKind {
  return typeof value === 'string' && CLI_KINDS.includes(value as CliKind)
}

function assertCliContract(value: unknown): asserts value is CliContract {
  if (!value || typeof value !== 'object') throw new Error('CLI 契约根节点无效')
  const contract = value as Partial<CliContract>
  if (contract.contractVersion !== 1) throw new Error('CLI 契约版本不受支持')
  if (contract.serializedCliKindField !== 'cliKind') throw new Error('CLI 序列化字段不一致')
  if (!isCliKind(contract.legacyDefaultCliKind)) throw new Error('CLI 默认迁移类型无效')
  if (
    !contract.mainTab
    || !Array.isArray(contract.mainTab.values)
    || typeof contract.mainTab.legacyAliases !== 'object'
    || typeof contract.mainTab.unknownFallback !== 'string'
  ) {
    throw new Error('CLI 主入口契约无效')
  }
  if (!Array.isArray(contract.cliDescriptors) || contract.cliDescriptors.length !== CLI_KINDS.length) {
    throw new Error('CLI 描述表不完整')
  }
  for (const descriptor of contract.cliDescriptors) {
    if (
      !isCliKind(descriptor.kind)
      || typeof descriptor.label !== 'string'
      || typeof descriptor.command !== 'string'
      || !['json', 'toml', 'jsonc', 'none'].includes(descriptor.configFormat)
      || typeof descriptor.supportsManagedProfile !== 'boolean'
    ) {
      throw new Error('CLI 描述项无效')
    }
  }
  const descriptorKinds = new Set(contract.cliDescriptors.map((descriptor) => descriptor.kind))
  if (CLI_KINDS.some((kind) => !descriptorKinds.has(kind))) throw new Error('CLI 描述表缺少稳定标识')
  if (!Array.isArray(contract.issueDefinitions)) throw new Error('CLI 错误定义无效')
  for (const issue of contract.issueDefinitions) {
    if (CLI_KINDS.some((kind) => typeof issue.messages?.[kind] !== 'string')) {
      throw new Error(`CLI 错误提示不完整: ${issue.code}`)
    }
  }
}

assertCliContract(rawCliContract)

export const CLI_CONTRACT: CliContract = rawCliContract
export const CLI_DESCRIPTORS = Object.fromEntries(
  CLI_CONTRACT.cliDescriptors.map((descriptor) => [descriptor.kind, descriptor]),
) as Record<CliKind, CliDescriptor>

export function normalizePersistedMainTab(value: unknown): MainTab {
  if (typeof value !== 'string') return CLI_CONTRACT.mainTab.unknownFallback
  const aliased = CLI_CONTRACT.mainTab.legacyAliases[value] ?? value
  return CLI_CONTRACT.mainTab.values.includes(aliased as MainTab)
    ? aliased as MainTab
    : CLI_CONTRACT.mainTab.unknownFallback
}

export function getCliIssueDefinition(code: CliIssueCode): CliIssueDefinition {
  const definition = CLI_CONTRACT.issueDefinitions.find((item) => item.code === code)
  if (!definition) throw new Error(`未知 CLI 错误码: ${code}`)
  return definition
}
