import skillCatalog from '../data/claudeSkills.json' with { type: 'json' }

export type ClaudeSlashCommandKind = 'builtin' | 'skill'

export interface ClaudeSlashCommand {
  command: string
  description: string
  kind: ClaudeSlashCommandKind
}

export type ClaudeSlashCommandValidation =
  | { kind: 'plain' }
  | { kind: 'allowed'; command: ClaudeSlashCommand }
  | { kind: 'unsupported'; command: string }

const BUILTIN_COMMANDS: readonly ClaudeSlashCommand[] = [
  {
    command: '/init',
    description: '初始化项目的 Claude 配置',
    kind: 'builtin',
  },
  {
    command: '/compact',
    description: '压缩当前会话上下文',
    kind: 'builtin',
  },
  {
    command: '/clear',
    description: '清空当前会话上下文',
    kind: 'builtin',
  },
  {
    command: '/contest',
    description: '执行 Claude Code contest 命令',
    kind: 'builtin',
  },
]

function normalizeSkillName(name: string): string | undefined {
  const normalized = name.trim().replace(/^\/+/, '').toLowerCase()
  if (!normalized || !/^[a-z0-9][a-z0-9-]*$/.test(normalized)) return undefined
  return normalized
}

const skillCommands = skillCatalog.skills.flatMap<ClaudeSlashCommand>((skill) => {
  const name = normalizeSkillName(skill.name)
  if (!name) return []
  return [{
    command: `/${name}`,
    description: skill.description.trim() || `运行 ${name} skill`,
    kind: 'skill',
  }]
})

export const CLAUDE_SLASH_COMMANDS: readonly ClaudeSlashCommand[] = [
  ...BUILTIN_COMMANDS,
  ...skillCommands,
]

const commandsByName = new Map(
  CLAUDE_SLASH_COMMANDS.map(command => [command.command, command]),
)

function slashCommandToken(input: string): string | undefined {
  const trimmed = input.trimStart()
  if (!trimmed.startsWith('/')) return undefined
  return trimmed.match(/^\/[^\s]*/)?.[0] ?? '/'
}

export function validateClaudeSlashCommand(input: string): ClaudeSlashCommandValidation {
  const token = slashCommandToken(input)
  if (token === undefined) return { kind: 'plain' }
  const command = commandsByName.get(token.toLowerCase())
  return command
    ? { kind: 'allowed', command }
    : { kind: 'unsupported', command: token }
}

export function filterClaudeSlashCommands(input: string): readonly ClaudeSlashCommand[] {
  const trimmed = input.trimStart()
  if (!trimmed.startsWith('/') || /\s/.test(trimmed)) return []
  const query = trimmed.toLowerCase()
  return CLAUDE_SLASH_COMMANDS.filter(command => command.command.startsWith(query))
}
