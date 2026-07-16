<script setup lang="ts">
import '@xterm/xterm/css/xterm.css'
import { ref, watch, onMounted, onBeforeUnmount, nextTick } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebLinksAddon } from '@xterm/addon-web-links'
import { useTerminalStore } from '@/stores/terminal'
import {
  createTerminalOutputWriter,
  type TerminalOutputWriter,
} from '@/utils/codexTerminalOutput'
import { encodeCodexConptyInput } from '@/utils/codexTerminalInput'

const props = defineProps<{
  tabId: number
  active: boolean
}>()

const store = useTerminalStore()
const containerRef = ref<HTMLDivElement | null>(null)
const initialized = ref(false)

let term: Terminal | null = null
let fitAddon: FitAddon | null = null
let resizeObserver: ResizeObserver | null = null
let intersectionObserver: IntersectionObserver | null = null
let retryTimer: ReturnType<typeof setTimeout> | null = null
let outputWriter: TerminalOutputWriter | null = null
let isFirstIntersection = true
const MAX_RETRIES = 40  // 40 × 50ms = 2 seconds max wait

function containerHasDimensions(): boolean {
  const el = containerRef.value
  if (!el) return false
  const rect = el.getBoundingClientRect()
  return rect.width > 0 && rect.height > 0
}

function fitVisibleTerminal(source: string) {
  if (!fitAddon || !containerHasDimensions()) return
  try {
    fitAddon.fit()
  } catch (e) {
    console.warn(`[PTY] tab ${props.tabId}: ${source} fitAddon.fit() threw:`, e)
  }
}

function initTerminal() {
  if (initialized.value || !containerRef.value) return

  // xterm.js opened in a zero-size container creates a broken terminal that
  // never renders correctly even after the container becomes visible.
  // Wait until the container has real dimensions before calling term.open().
  if (!containerHasDimensions()) {
    return
  }

  const cliKind = store.tabs.find(tab => tab.id === props.tabId)?.cliKind ?? 'claude'
  initialized.value = true
  term = new Terminal({
    fontFamily: '"Cascadia Code", "Cascadia Mono", Consolas, monospace',
    fontSize: store.fontSize,
    cursorBlink: cliKind !== 'codex',
    allowTransparency: false,
    scrollback: 5000,
    theme: {
      background: '#1E1E1E',
      foreground: '#F0F0F0',
      cursor: '#F0F0F0',
      selectionBackground: '#3A5A8C',
      black: '#1E1E1E',
      red: '#CC0000',
      green: '#4E9A06',
      yellow: '#C4A000',
      blue: '#3465A4',
      magenta: '#75507B',
      cyan: '#06989A',
      white: '#D3D7CF',
      brightBlack: '#555753',
      brightRed: '#EF2929',
      brightGreen: '#8AE234',
      brightYellow: '#FCE94F',
      brightBlue: '#729FCF',
      brightMagenta: '#AD7FA8',
      brightCyan: '#34E2E2',
      brightWhite: '#EEEEEC',
    },
  })

  fitAddon = new FitAddon()
  term.loadAddon(fitAddon)
  term.loadAddon(new WebLinksAddon())

  term.open(containerRef.value!)

  // Don't call fitAddon.fit() immediately after open() — the container may not
  // have its final dimensions yet (e.g. transitioning from display:none).
  // Keep the IntersectionObserver alive so it re-fits whenever the terminal
  // becomes visible again (e.g. switching sessions or returning to Project).
  intersectionObserver = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (entry.isIntersecting && fitAddon) {
        // Use requestAnimationFrame so the browser has finished layout before we measure.
        requestAnimationFrame(() => {
          fitVisibleTerminal('IntersectionObserver')

          if (isFirstIntersection) {
            isFirstIntersection = false
            // Force xterm to recalculate the IME helper-textarea position now that
            // the container has its final dimensions.
            setTimeout(() => {
              if (term) {
                term.blur()
                term.focus()
              }
            }, 50)
          }
        })
      }
    }
  }, { threshold: 0.1 })
  intersectionObserver.observe(containerRef.value!)

  const t = term
  term.attachCustomKeyEventHandler((e: KeyboardEvent) => {
    if (e.type !== 'keydown') return true
    if (e.ctrlKey && e.key === 'c') {
      const sel = t.getSelection()
      if (sel) {
        const ta = document.createElement('textarea')
        ta.value = sel
        ta.style.cssText = 'position:fixed;left:-9999px'
        document.body.appendChild(ta)
        ta.select()
        document.execCommand('copy')
        document.body.removeChild(ta)
        t.clearSelection()
        return false
      }
    }
    if (e.ctrlKey && e.key === 'v') return false  // let xterm.js built-in paste handle it
    return true
  })

  // Forward input to PTY
  term.onData((data) => {
    const ptyData = cliKind === 'codex' ? encodeCodexConptyInput(data) : data
    invoke('pty_write', { tabId: props.tabId, data: ptyData }).catch(() => {})
  })

  // Forward resize to PTY
  term.onResize(({ cols, rows }) => {
    invoke('pty_resize', { tabId: props.tabId, cols, rows }).catch(() => {})
  })

  // Register writer in store — this also flushes any buffered output
  outputWriter = createTerminalOutputWriter(cliKind, bytes => t.write(bytes), {
    gateCursorDuringOutput: cliKind === 'codex',
  })
  store.registerWriter(props.tabId, bytes => outputWriter?.write(bytes))

  resizeObserver = new ResizeObserver(() => {
    // Never fit when the container has zero dimensions — this happens when the
    // terminal pane is hidden (v-show on self or ancestor).  Fitting at 0×0
    // would shrink the PTY to minimum cols, causing all output received while
    // hidden to be permanently wrapped at 2–3 characters per line.
    fitVisibleTerminal('ResizeObserver')
  })
  resizeObserver.observe(containerRef.value!)
  // term.focus() is intentionally deferred to the IntersectionObserver callback
  // so it runs after fitAddon.fit() has set correct dimensions. Calling focus()
  // here (before fit) would lock the IME textarea at stale/zero coordinates.
}

// Schedule initTerminal with polling until the container has real dimensions.
// This handles the case where the terminal panel is still display:none when
// the component mounts (e.g. the v-show on the parent hasn't applied yet).
function scheduleInit(retries = 0) {
  if (initialized.value) return
  if (retryTimer !== null) return  // already scheduled

  retryTimer = setTimeout(() => {
    retryTimer = null
    if (initialized.value) return

    if (containerHasDimensions()) {
      initTerminal()
    } else if (retries < MAX_RETRIES) {
      scheduleInit(retries + 1)
    } else {
      console.error(`[PTY] tab ${props.tabId}: gave up waiting for container dimensions after ${MAX_RETRIES} retries`)
      // Last-ditch attempt: open anyway and hope ResizeObserver saves us
      initTerminal()
    }
  }, 50)
}

onMounted(() => {
  if (props.active) {
    nextTick(() => requestAnimationFrame(() => scheduleInit()))
  }
})

onBeforeUnmount(() => {
  if (retryTimer !== null) {
    clearTimeout(retryTimer)
    retryTimer = null
  }
  intersectionObserver?.disconnect()
  intersectionObserver = null
  outputWriter?.dispose()
  outputWriter = null
  store.unregisterWriter(props.tabId)
  resizeObserver?.disconnect()
  term?.dispose()
  term = null
  fitAddon = null
})

watch(() => props.active, (isActive) => {
  if (isActive) {
    nextTick(() => requestAnimationFrame(() => {
      if (!initialized.value) {
        scheduleInit()
      } else if (term) {
        fitVisibleTerminal('active watcher')
        term.focus()
        // Extra fit after layout settles. Claude's full-screen UI is sensitive
        // to one-frame width changes while project panes animate.
        setTimeout(() => {
          fitVisibleTerminal('active watcher settled')
          term?.focus()
        }, 100)
      }
    }))
  } else if (term) {
    term.blur()
  }
})

watch(() => store.fontSize, (size) => {
  if (term) {
    term.options.fontSize = size
    fitVisibleTerminal('font size watcher')
  }
})

watch(() => store.refitSignal, () => {
  if (!initialized.value) {
    scheduleInit()
  } else if (props.active && term && fitAddon) {
    nextTick(() => {
      requestAnimationFrame(() => {
        fitVisibleTerminal('refit signal')
        setTimeout(() => fitVisibleTerminal('refit signal settled'), 100)
      })
    })
  }
})
</script>

<template>
  <div
    v-show="active"
    class="terminal-pane"
  >
    <div ref="containerRef" class="terminal-pane__inner" />
  </div>
</template>

<style scoped>
.terminal-pane {
  position: absolute;
  inset: 0;
  background: #1E1E1E;
  overflow: hidden;
}

.terminal-pane__inner {
  width: 100%;
  height: calc(100% - 6px);
  padding: 8px;
  position: relative;
}

/* Give the hidden textarea a safe initial position so Windows IME candidate
   windows do not jump to the screen edge. Do not force its position or size:
   xterm updates those inline while an IME composition is active. */
.terminal-pane :deep(.xterm-helper-textarea) {
  left: 0;
  opacity: 0 !important;
  pointer-events: none !important;
  caret-color: transparent !important;
}

.terminal-pane :deep(.xterm) {
  height: 100%;
}

.terminal-pane :deep(.xterm-viewport) {
  overflow-y: auto;
  background: #1E1E1E;
  padding-right: 18px !important;
  box-sizing: border-box;
}

.terminal-pane :deep(.xterm-screen) {
  padding-right: 18px !important;
  box-sizing: border-box;
}

/* Thin overlay scrollbar — 5px line that floats over content */
.terminal-pane :deep(.xterm-viewport)::-webkit-scrollbar {
  width: 5px;
}

.terminal-pane :deep(.xterm-viewport)::-webkit-scrollbar-track {
  background: transparent;
}

.terminal-pane :deep(.xterm-viewport)::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.15);
  border-radius: 3px;
}

.terminal-pane :deep(.xterm-viewport)::-webkit-scrollbar-thumb:hover {
  background: rgba(255, 255, 255, 0.35);
}
</style>
