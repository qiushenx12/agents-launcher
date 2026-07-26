import MarkdownIt from 'markdown-it'

const copyButton = `
<button
  class="conversation-code-block__copy"
  type="button"
  aria-label="复制代码"
  title="复制代码"
>
  <svg class="conversation-code-block__copy-icon" aria-hidden="true" viewBox="0 0 24 24">
    <rect x="8" y="8" width="13" height="13" rx="2.5" />
    <path d="M16 8V5.5A2.5 2.5 0 0 0 13.5 3h-8A2.5 2.5 0 0 0 3 5.5v8A2.5 2.5 0 0 0 5.5 16H8" />
  </svg>
  <svg class="conversation-code-block__check-icon" aria-hidden="true" viewBox="0 0 24 24">
    <path d="m5 12.5 4.2 4.2L19 7" />
  </svg>
  <svg class="conversation-code-block__error-icon" aria-hidden="true" viewBox="0 0 24 24">
    <path d="M7 7l10 10M17 7 7 17" />
  </svg>
</button>`

function wrapCopyableCodeBlock(renderedCode: string) {
  return `<div class="conversation-code-block">${renderedCode}${copyButton}</div>\n`
}

export function createClaudeMarkdownRenderer() {
  const markdown = new MarkdownIt({ html: false, linkify: true, breaks: true })

  markdown.renderer.rules.table_open = (tokens, idx, options, _env, self) => (
    `<div class="conversation-table-wrap">${self.renderToken(tokens, idx, options)}`
  )
  markdown.renderer.rules.table_close = (tokens, idx, options, _env, self) => (
    `${self.renderToken(tokens, idx, options)}</div>\n`
  )

  for (const ruleName of ['fence', 'code_block'] as const) {
    const originalRule = markdown.renderer.rules[ruleName]
    if (!originalRule) continue
    markdown.renderer.rules[ruleName] = (tokens, idx, options, env, self) => (
      wrapCopyableCodeBlock(originalRule(tokens, idx, options, env, self))
    )
  }

  return markdown
}
