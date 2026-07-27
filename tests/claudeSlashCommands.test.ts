import assert from 'node:assert/strict'
import test from 'node:test'
import {
  filterClaudeSlashCommands,
  getClaudeSlashCommands,
  setClaudeSkillCommands,
  validateClaudeSlashCommand,
} from '../src/utils/claudeSlashCommands.ts'

test('slash command catalog starts with built-ins and is populated from discovered skills', () => {
  setClaudeSkillCommands([])
  assert.deepEqual(
    getClaudeSlashCommands().map(command => command.command),
    ['/compact', '/clear', '/contest'],
  )

  setClaudeSkillCommands([{ name: 'doc-coauthoring', description: 'Write documentation' }])
  assert.deepEqual(
    getClaudeSlashCommands().map(command => command.command),
    ['/compact', '/clear', '/contest', '/doc-coauthoring'],
  )
})

test('supported slash commands allow arguments and leading whitespace', () => {
  setClaudeSkillCommands([{ name: 'html-anything', description: 'Render HTML' }])
  assert.equal(validateClaudeSlashCommand('hello').kind, 'plain')
  assert.equal(validateClaudeSlashCommand('/init').kind, 'allowed')
  assert.equal(validateClaudeSlashCommand('  /compact keep the latest work').kind, 'allowed')
  assert.equal(validateClaudeSlashCommand('/html-anything report.md').kind, 'allowed')
})

test('model, bare skill, and unknown slash commands are blocked', () => {
  assert.equal(validateClaudeSlashCommand('/model').kind, 'unsupported')
  assert.equal(validateClaudeSlashCommand('/model haiku').kind, 'unsupported')
  assert.equal(validateClaudeSlashCommand('/skill').kind, 'unsupported')
  assert.equal(validateClaudeSlashCommand('/unknown').kind, 'unsupported')
})

test('slash suggestions filter by the command prefix and hide after arguments begin', () => {
  assert.deepEqual(
    filterClaudeSlashCommands('/c').map(command => command.command),
    ['/compact', '/clear', '/contest'],
  )
  assert.deepEqual(
    filterClaudeSlashCommands('/html').map(command => command.command),
    ['/html-anything'],
  )
  assert.deepEqual(filterClaudeSlashCommands('/compact '), [])
  assert.deepEqual(filterClaudeSlashCommands('/init'), [])
  assert.deepEqual(filterClaudeSlashCommands('/model'), [])
})
