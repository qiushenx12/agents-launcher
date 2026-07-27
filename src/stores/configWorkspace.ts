import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { confirm } from '@tauri-apps/plugin-dialog'
import { CLI_DESCRIPTORS, isCliKind, type CliKind } from '@/types/cli'

interface DraftGuard {
  isDirty: () => boolean
  discard: () => void
}

const ACTIVE_CONFIG_KIND_KEY = 'active-config-cli-kind'

export const useConfigWorkspaceStore = defineStore('configWorkspace', () => {
  const savedKind = localStorage.getItem(ACTIVE_CONFIG_KIND_KEY)
  const activeKind = ref<CliKind>(isCliKind(savedKind) ? savedKind : 'claude')
  const preflightVisible = ref(false)
  const guards = new Map<CliKind, DraftGuard>()

  const activeHasUnsavedChanges = computed(() => guards.get(activeKind.value)?.isDirty() ?? false)

  function registerDraftGuard(kind: CliKind, guard: DraftGuard) {
    guards.set(kind, guard)
    return () => {
      if (guards.get(kind) === guard) guards.delete(kind)
    }
  }

  async function confirmDiscardActiveChanges(action: string): Promise<boolean> {
    const guard = guards.get(activeKind.value)
    if (!guard?.isDirty()) return true
    const accepted = await confirm(
      `当前配置有未保存的修改。${action}将放弃这些修改，是否继续？`,
      { title: '未保存的配置', kind: 'warning' },
    )
    if (accepted) guard.discard()
    return accepted
  }

  async function selectKind(kind: CliKind): Promise<boolean> {
    if (kind === activeKind.value) return true
    if (!(await confirmDiscardActiveChanges(`切换到 ${CLI_DESCRIPTORS[kind].label} 配置页`))) return false
    activeKind.value = kind
    localStorage.setItem(ACTIVE_CONFIG_KIND_KEY, kind)
    return true
  }

  function openPreflight() {
    preflightVisible.value = true
  }

  function closePreflight() {
    preflightVisible.value = false
  }

  return {
    activeKind,
    preflightVisible,
    activeHasUnsavedChanges,
    registerDraftGuard,
    confirmDiscardActiveChanges,
    selectKind,
    openPreflight,
    closePreflight,
  }
})
