import assert from 'node:assert/strict'
import test from 'node:test'
import { createClaudeMarkdownRenderer } from '../src/utils/claudeMarkdown.ts'

test('fenced command blocks include one accessible copy control', () => {
  const markdown = createClaudeMarkdownRenderer()
  const html = markdown.render('```bash\nprintf "<tag>&value"\ngit status --short\n```')

  assert.match(html, /class="conversation-code-block"/)
  assert.match(html, /class="conversation-code-block__copy"/)
  assert.match(html, /aria-label="复制代码"/)
  assert.equal(html.match(/conversation-code-block__copy"/g)?.length, 1)
  assert.match(html, /printf &quot;&lt;tag&gt;&amp;value&quot;\ngit status --short\n/)
  assert.match(html, /conversation-code-block__error-icon/)
})

test('indented code blocks also include a copy control', () => {
  const markdown = createClaudeMarkdownRenderer()
  const html = markdown.render('    npm run build\n')

  assert.match(html, /class="conversation-code-block"/)
  assert.match(html, /class="conversation-code-block__copy"/)
})

test('inline code remains inline and does not get a copy control', () => {
  const markdown = createClaudeMarkdownRenderer()
  const html = markdown.render('Run `npm test` next.')

  assert.match(html, /<code>npm test<\/code>/)
  assert.doesNotMatch(html, /conversation-code-block/)
})

test('markdown tables keep semantic markup inside a horizontal scroll wrapper', () => {
  const markdown = createClaudeMarkdownRenderer()
  const html = markdown.render([
    '| 步骤 | 命令 | 产物 |',
    '| --- | --- | --- |',
    '| 生产构建 | `npm run tauri build` | Windows 安装包 |',
  ].join('\n'))

  assert.match(html, /<div class="conversation-table-wrap"><table>/)
  assert.match(html, /<thead>/)
  assert.match(html, /<th>步骤<\/th>/)
  assert.match(html, /<td><code>npm run tauri build<\/code><\/td>/)
  assert.match(html, /<\/table>\s*<\/div>/)
})
