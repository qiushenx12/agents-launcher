import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'

export type ClaudeView = 'conversation' | 'terminal'

function normalizeClaudeView(value: string): ClaudeView {
  if (value === 'conversation') return value
  return 'terminal'
}

export const useClaudeViewModeStore = defineStore('claude-view-mode', () => {
  const startupView = ref<ClaudeView>('terminal')
  const savedView = ref<ClaudeView>('terminal')
  const runtimeView = ref<ClaudeView>('terminal')
  const logOutputEnabled = ref(false)
  const loaded = ref(false)
  let loadPromise: Promise<void> | null = null

  const structuredCaptureEnabled = computed(() => startupView.value !== 'terminal')
  const pendingRestartView = computed<ClaudeView | null>(() => (
    startupView.value === 'terminal' && savedView.value !== startupView.value
      ? savedView.value
      : null
  ))

  async function load() {
    if (loaded.value) return
    if (loadPromise) return loadPromise

    loadPromise = (async () => {
      try {
        const [viewValue, logOutput] = await Promise.all([
          invoke<string>('load_claude_startup_view'),
          invoke<boolean>('load_claude_log_output_enabled'),
        ])
        const view = normalizeClaudeView(viewValue)
        startupView.value = view
        savedView.value = view
        runtimeView.value = view
        logOutputEnabled.value = logOutput
      } catch {
        // Keep the terminal defaults when persisted state is unavailable.
      } finally {
        loaded.value = true
        loadPromise = null
      }
    })()
    return loadPromise
  }

  async function save(view: ClaudeView) {
    await invoke('save_claude_startup_view', { view })
    savedView.value = view
  }

  function setRuntimeView(view: ClaudeView) {
    runtimeView.value = view
  }

  async function setLogOutputEnabled(enabled: boolean) {
    await invoke('set_claude_log_output_enabled', { enabled })
    logOutputEnabled.value = enabled
  }

  return {
    startupView,
    savedView,
    runtimeView,
    logOutputEnabled,
    loaded,
    structuredCaptureEnabled,
    pendingRestartView,
    load,
    save,
    setRuntimeView,
    setLogOutputEnabled,
  }
})
