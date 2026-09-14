import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * Regression tests for two dsh-specific defects:
 *
 * 1. The settings popover offered「项目终端拖入文件」while dsh was active,
 *    even though dsh has no project terminal — its interface is served by
 *    `dsh web` itself, so the option could only confuse.
 * 2. Dropping a file into the embedded dsh page did nothing, while the same
 *    drop works in a browser. Tauri installs its own drag-drop handler on
 *    every webview, converting drops into `tauri://drag-drop` events the page
 *    never listens to; the child WebView must opt out to get HTML5 drag & drop
 *    back.
 *
 * Both fixes are structural, so these tests read the component/module sources.
 */

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')

function source(relativePath: string): string {
  return readFileSync(resolve(repoRoot, relativePath), 'utf8')
}

test('the embedded dsh webview keeps HTML5 drag & drop', () => {
  const embed = source('src-tauri/src/dsh_embed.rs')

  // The builder that hosts the dsh page must disable Tauri's handler; without
  // it drops never reach the page's own drop handlers (browser parity).
  const builder = /fn create_webview[\s\S]*?WebviewBuilder::new([\s\S]*?);/.exec(embed)
  assert.ok(builder, 'missing the embedded webview builder')
  assert.match(
    builder[1],
    /\.disable_drag_drop_handler\(\)/,
    'Tauri drag-drop handler must be disabled for the dsh page to receive file drops',
  )
})

test('the settings popover drops 项目终端拖入文件 while dsh is active', () => {
  const app = source('src/App.vue')

  // The trigger is gated on a dsh-aware flag…
  const trigger = /<button\s+v-if="showProjectDropPathSettings"[\s\S]*?<span>项目终端拖入文件<\/span>/.exec(app)
  assert.ok(trigger, 'the 项目终端拖入文件 trigger must be gated by showProjectDropPathSettings')

  // …which is false exactly when the settings context is dsh (and true for the
  // terminal/orchestration tabs where the popover keeps every global entry).
  assert.match(
    app,
    /const showProjectDropPathSettings = computed\(\(\) => settingsCliKind\.value !== 'dsh'\)/,
  )

  // An open submenu must not outlive its hidden trigger.
  assert.match(
    app,
    /watch\(showProjectDropPathSettings, \(visible\) => \{[\s\S]*?activeSettingsSubmenu\.value = null/,
  )
})
