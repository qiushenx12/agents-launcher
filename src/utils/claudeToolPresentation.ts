const SUMMARY_LIMIT = 180

type ToolInput = Record<string, unknown>

function asRecord(value: unknown): ToolInput | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? value as ToolInput
    : undefined
}

function asText(value: unknown): string {
  return typeof value === 'string' ? value.trim() : ''
}

function oneLine(value: string): string {
  const compact = value.replace(/\s+/g, ' ').trim()
  if (compact.length <= SUMMARY_LIMIT) return compact
  return `${compact.slice(0, SUMMARY_LIMIT - 1).trimEnd()}…`
}

function firstText(input: ToolInput | undefined, keys: string[]): string {
  if (!input) return ''
  for (const key of keys) {
    const text = asText(input[key])
    if (text) return text
  }
  return ''
}

function withLocation(label: string, location: string): string {
  if (!location) return label
  if (!label) return location
  return `${label} · ${location}`
}

/** Builds a useful, single-line preview for a collapsed Claude tool card. */
export function summarizeClaudeTool(toolName: string | undefined, value: unknown): string {
  const input = asRecord(value)
  const normalizedName = (toolName ?? '').toLowerCase()
  const command = firstText(input, ['command', 'cmd', 'script'])
  const filePath = firstText(input, ['file_path', 'path', 'notebook_path'])
  const pattern = firstText(input, ['pattern', 'query'])

  if (['bash', 'shell', 'powershell', 'terminal'].includes(normalizedName) && command) {
    return oneLine(command)
  }

  if (['read', 'write', 'edit', 'update', 'multiedit'].includes(normalizedName) && filePath) {
    return oneLine(filePath)
  }

  if (normalizedName === 'grep') {
    return oneLine(withLocation(pattern ? `搜索 ${pattern}` : '搜索', filePath))
  }

  if (normalizedName === 'glob') {
    return oneLine(withLocation(pattern, filePath))
  }

  const usefulText = command
    || filePath
    || pattern
    || firstText(input, ['url', 'description', 'prompt'])
    || asText(value)

  if (usefulText) return oneLine(usefulText)

  if (value === undefined || value === null) return ''
  try {
    return oneLine(JSON.stringify(value))
  } catch {
    return oneLine(String(value))
  }
}
