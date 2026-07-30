import assert from 'node:assert/strict'
import test from 'node:test'
import {
  isClaudeCommandTool,
  summarizeClaudeTool,
} from '../src/utils/claudeToolPresentation.ts'

test('command tools can be kept collapsed by default', () => {
  assert.equal(isClaudeCommandTool('Bash'), true)
  assert.equal(isClaudeCommandTool('PowerShell'), true)
  assert.equal(isClaudeCommandTool('Read'), false)
})

test('Bash card preview shows its command on one line', () => {
  assert.equal(
    summarizeClaudeTool('Bash', { command: 'git status --short\n&& git log --oneline -5' }),
    'git status --short && git log --oneline -5',
  )
})

test('PowerShell card preview also uses command input', () => {
  assert.equal(
    summarizeClaudeTool('PowerShell', { command: 'Get-SmbShare | Format-Table' }),
    'Get-SmbShare | Format-Table',
  )
})

test('file and search tools expose the useful target', () => {
  assert.equal(
    summarizeClaudeTool('Read', { file_path: 'docs/方案.md', offset: 10 }),
    'docs/方案.md',
  )
  assert.equal(
    summarizeClaudeTool('Grep', { pattern: '公共镜头', path: 'docs' }),
    '搜索 公共镜头 · docs',
  )
})

test('agent-style tools prefer a short description over their full prompt', () => {
  assert.equal(
    summarizeClaudeTool('Task', {
      description: '复核终端输出实现',
      prompt: '这是供子代理执行的很长任务说明，折叠状态不应优先展示它。',
    }),
    '复核终端输出实现',
  )
})

test('long previews are bounded while expanded values remain untouched', () => {
  const command = `echo ${'x'.repeat(240)}`
  const summary = summarizeClaudeTool('Bash', { command })

  assert.equal(summary.length, 180)
  assert.match(summary, /…$/)
  assert.equal(command.length, 245)
})
