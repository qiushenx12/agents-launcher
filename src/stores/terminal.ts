import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { TerminalTab, PtyOutput, PtyStatus, PtyTitle } from '@/types/terminal'
import type { CliKind } from '@/types/cli'

export interface CreateTerminalTabOptions {
  scope?: TerminalTab['scope']
  projectSessionId?: string
  sidebarTabId?: string
  activate?: boolean
  cliKind?: CliKind
}

export const useTerminalStore = defineStore('terminal', () => {
  const tabs = ref<TerminalTab[]>([])
  const activeTabId = ref<number | null>(null)
  const fontSize = ref(10)
  const refitSignal = ref(0)
  let listenerReady = false

  // Per-tab output buffers: collect data before xterm is ready
  const outputBuffers = new Map<number, Uint8Array[]>()
  // Per-tab last PTY output timestamp — used to trigger post-activity syncs
  const outputActivity = ref<Record<number, number>>({})
  // Per-tab xterm write callbacks: set when TerminalPane calls registerWriter
  const writers = new Map<number, (data: Uint8Array) => void>()

  const activeTab = computed(() =>
    tabs.value.find((t) => t.id === activeTabId.value) ?? null
  )

  const terminalTabs = computed(() =>
    tabs.value.filter((t) => !t.scope || t.scope === 'terminal')
  )

  // Per-tab idle timers (used to clear active state if no further title events arrive)
  const activityTimers = new Map<number, ReturnType<typeof setTimeout>>()

  // Tabs that have emitted at least one OSC 0 title — these are Claude sessions.
  // Only these tabs get active=true; non-Claude sessions stay green (alive) or gray (dead).
  const claudeTabs = new Set<number>()

  function clearIdleTimer(tabId: number) {
    const existing = activityTimers.get(tabId)
    if (existing) {
      clearTimeout(existing)
      activityTimers.delete(tabId)
    }
  }

  // Register global PTY event listeners once.
  let listenerReadyPromise: Promise<void> | null = null

  async function ensureListeners() {
    if (listenerReady) return
    if (listenerReadyPromise) {
      await listenerReadyPromise
      return
    }

    listenerReadyPromise = (async () => {
      await listen<PtyOutput>('pty_output', (event) => {
        const { tab_id, cli_kind, data } = event.payload
        const tab = tabs.value.find((item) => item.id === tab_id)
        if (tab && tab.cliKind !== cli_kind) return
        outputActivity.value[tab_id] = Date.now()

        const binary = atob(data)
        const bytes = new Uint8Array(binary.length)
        for (let i = 0; i < binary.length; i++) {
          bytes[i] = binary.charCodeAt(i)
        }

        const writer = writers.get(tab_id)
        if (writer) {
          writer(bytes)
        } else {
          // Buffer until xterm is ready
          let buf = outputBuffers.get(tab_id)
          if (!buf) {
            buf = []
            outputBuffers.set(tab_id, buf)
          }
          buf.push(bytes)
        }
      })

      await listen<PtyStatus>('pty_status', (event) => {
        const { tab_id, cli_kind, alive } = event.payload
        const tab = tabs.value.find((t) => t.id === tab_id)
        if (tab?.cliKind === cli_kind) tab.alive = alive
      })

      await listen<PtyTitle>('pty_title', (event) => {
        const { tab_id, cli_kind, title, has_spinner } = event.payload
        const tab = tabs.value.find((t) => t.id === tab_id)
        if (!tab || tab.cliKind !== cli_kind) return

        // Mark this tab as a Claude session (it emits OSC 0 titles)
        claudeTabs.add(tab_id)

        // Set active based on spinner presence
        tab.active = has_spinner

        // Auto-update tab title from OSC 0 (unless user has manually edited it)
        if (!tab.titleEdited) {
          // Clean the title — remove spinner chars for display
          const cleanTitle = title.replace(/[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]\s*/g, '').trim()
          if (cleanTitle) {
            tab.title = cleanTitle
          }
        }

        // Clear any pending idle timer since we have fresh state info
        clearIdleTimer(tab_id)
      })

      listenerReady = true
    })()

    await listenerReadyPromise
  }

  // Called by TerminalPane when xterm is ready to receive data
  function registerWriter(tabId: number, writer: (data: Uint8Array) => void) {
    writers.set(tabId, writer)
    // Flush buffered output
    const buf = outputBuffers.get(tabId)
    if (buf) {
      for (const chunk of buf) {
        writer(chunk)
      }
      outputBuffers.delete(tabId)
    }
  }

  function unregisterWriter(tabId: number) {
    writers.delete(tabId)
    // Intentionally keep outputBuffers so PTY output emitted while the pane is
    // unmounted (v-if) is preserved and flushed when it remounts.
  }

  async function createTab(
    cmd: string[],
    env: Record<string, string> = {},
    cwd: string | null = null,
    title?: string,
    options: CreateTerminalTabOptions = {},
  ) {
    // Ensure listeners are registered BEFORE creating the PTY
    await ensureListeners()

    const sessionIdIdx = cmd.indexOf('-r')
    const sessionId = sessionIdIdx !== -1 ? cmd[sessionIdIdx + 1] : undefined

    const id = await invoke<number>('pty_create', {
      cmd,
      env,
      cwd,
      cols: 120,
      rows: 30,
      sessionId,
      cliKind: options.cliKind ?? 'claude',
    })

    const tab: TerminalTab = {
      id,
      title: title ?? `终端 ${id}`,
      alive: true,
      active: false,
      // Project/session terminals should keep the user-defined session name and
      // not be overwritten by the shell's OSC 0 title updates.
      titleEdited: options.scope === 'project' || options.scope === 'sidebar' ? true : false,
      sessionId,
      scope: options.scope ?? 'terminal',
      projectSessionId: options.projectSessionId,
      sidebarTabId: options.sidebarTabId,
      cliKind: options.cliKind ?? 'claude',
    }
    tabs.value.push(tab)
    if (options.activate !== false && tab.scope === 'terminal') {
      activeTabId.value = id
    }
    return id
  }

  async function closeTab(id: number) {
    // Fire-and-forget the backend kill so the UI removes the tab instantly.
    invoke('pty_kill', { tabId: id }).catch(() => {})
    unregisterWriter(id)
    outputBuffers.delete(id)
    // Clean up timers and Claude session tracking
    clearIdleTimer(id)
    claudeTabs.delete(id)
    const idx = tabs.value.findIndex((t) => t.id === id)
    if (idx !== -1) tabs.value.splice(idx, 1)

    if (activeTabId.value === id) {
      const visibleTabs = terminalTabs.value
      activeTabId.value = visibleTabs.length > 0
        ? visibleTabs[visibleTabs.length - 1].id
        : null
    }
  }

  function activateTab(id: number) {
    activeTabId.value = id
  }

  function updateTabStatus(id: number, isAlive: boolean) {
    const tab = tabs.value.find((t) => t.id === id)
    if (tab) tab.alive = isAlive
  }

  function updateTabTitle(id: number, title: string) {
    const tab = tabs.value.find((t) => t.id === id)
    if (tab) {
      tab.title = title
      tab.titleEdited = true
    }
  }

  async function setFontSize(size: number) {
    const clamped = Math.max(6, Math.min(28, size))
    fontSize.value = clamped
    await invoke('save_terminal_font_size', { fontSize: clamped }).catch(() => {})
  }

  async function loadFontSize() {
    try {
      const size = await invoke<number>('load_terminal_font_size')
      fontSize.value = Math.max(6, Math.min(28, size))
    } catch {
      // keep default
    }
  }

  function reorderTabs(newTabs: TerminalTab[]) {
    tabs.value = newTabs
  }

  function reorderTerminalTabs(newTerminalTabs: TerminalTab[]) {
    const scoped = tabs.value.filter((t) => t.scope && t.scope !== 'terminal')
    tabs.value = [...newTerminalTabs, ...scoped]
  }

  function triggerRefit() {
    refitSignal.value++
  }

  return {
    tabs,
    activeTabId,
    activeTab,
    terminalTabs,
    fontSize,
    refitSignal,
    outputActivity,
    createTab,
    closeTab,
    activateTab,
    updateTabStatus,
    updateTabTitle,
    setFontSize,
    loadFontSize,
    registerWriter,
    unregisterWriter,
    reorderTabs,
    reorderTerminalTabs,
    triggerRefit,
  }
})
