import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'

/**
 * Guards the custom NSIS template that `src-tauri/tauri.conf.json` hands to the
 * bundler via `bundle.windows.nsis.template`.
 *
 * That file must stay a *Handlebars source*. It once was replaced by the
 * bundler's own output — `target/release/nsis/x64/installer.nsi`, captured from
 * a build that ran in an earlier checkout directory. Every `{{...}}` had already
 * been rendered, so the template silently pinned `INSTALLERICON`,
 * `MAINBINARYSRCPATH` and the language `!include` to that dead path. The rust
 * build kept succeeding and only `makensis` failed, one 60-second build later,
 * with `can't open file`.
 *
 * The assertions below fail on the *first* sign of that mistake instead: no
 * placeholder at all, or a placeholder the installed bundler does not fill.
 */

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const templatePath = 'src-tauri/nsis/installer.nsi'

const template = readFileSync(resolve(repoRoot, templatePath), 'utf8')

/** Keys the tauri-bundler handlebars context is known to provide. */
const REQUIRED_PLACEHOLDERS = [
  'installer_icon',
  'main_binary_path',
  'main_binary_name',
  'product_name',
  'version',
  'manufacturer',
]

/**
 * A rendered template has paths but no braces. Anything that looks like a drive
 * letter or a UNC prefix is a path the bundler should have supplied itself.
 */
const ABSOLUTE_PATH = /(?:^|["\s])(?:[A-Za-z]:[\\/]|\\\\)/m

test('NSIS template still contains the placeholders the bundler fills', () => {
  const missing = REQUIRED_PLACEHOLDERS.filter(
    (name) => !template.includes(`{{${name}}}`),
  )
  assert.deepEqual(
    missing,
    [],
    `${templatePath} is missing ${missing.join(', ')}. A template with no ` +
      'placeholders is the bundler\'s rendered output, not a source file.',
  )
})

test('NSIS template carries no baked-in absolute path', () => {
  const match = ABSOLUTE_PATH.exec(template)
  assert.equal(
    match,
    null,
    `${templatePath} hardcodes the path ${JSON.stringify(match?.[0]?.trim())}. ` +
      'Paths differ per checkout, so they must arrive as placeholders.',
  )
})

test('upgrade defaults to an in-place install', () => {
  // The one intentional divergence from upstream: upgrading should reinstall
  // over the old version, with "uninstall first" left as a manual choice. Both
  // the page text and the leave handler have to agree, or the radio button
  // reads "覆盖安装" while the code behind it uninstalls.
  assert.ok(
    template.includes('覆盖安装（推荐）'),
    'the upgrade page no longer offers an in-place reinstall by default',
  )
  assert.ok(
    template.includes('; User chose to upgrade in place'),
    'PageLeaveReinstall no longer maps the first radio button to reinst_done',
  )
})
