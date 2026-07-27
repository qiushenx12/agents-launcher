const buildPlatform = __AGENTS_LAUNCHER_PLATFORM__
const runtimeIsWindows = typeof navigator !== 'undefined' && navigator.platform.includes('Win')

// Tauri injects TAURI_ENV_PLATFORM during a package build. The navigator
// fallback keeps plain Vite development on Windows using the same font set.
export const isWindowsBuild = buildPlatform
  ? buildPlatform === 'windows'
  : runtimeIsWindows

export const terminalFontFamily = isWindowsBuild
  ? '"Cascadia Code", "Cascadia Mono", Consolas, "Arial Unicode MS", "Segoe UI Symbol", "Segoe UI Emoji", monospace'
  : '"SFMono-Regular", Menlo, Monaco, monospace'

export const monoFontFamily = isWindowsBuild
  ? '"Cascadia Code", "Cascadia Mono", Consolas, monospace'
  : '"SFMono-Regular", Menlo, Monaco, monospace'
