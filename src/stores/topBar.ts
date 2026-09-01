import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import { CLI_DESCRIPTORS, CLI_KINDS, isCliKind, type CliKind } from '@/types/cli'

export const DEFAULT_TOP_BAR_ORDER = ['config', ...CLI_KINDS] as const
export type TopBarItem = typeof DEFAULT_TOP_BAR_ORDER[number]

interface PersistedTopBarLayout {
  order: string[]
  hidden: string[]
}

export function isTopBarItem(value: unknown): value is TopBarItem {
  return value === 'config' || isCliKind(value)
}

export function normalizeTopBarOrder(value: unknown): TopBarItem[] {
  const candidates = Array.isArray(value) ? value : []
  const normalized: TopBarItem[] = []
  for (const item of [...candidates, ...DEFAULT_TOP_BAR_ORDER]) {
    if (isTopBarItem(item) && !normalized.includes(item)) normalized.push(item)
  }
  return normalized
}

export function topBarItemLabel(item: TopBarItem) {
  return item === 'config' ? '配置' : CLI_DESCRIPTORS[item].label
}

export function normalizeTopBarHidden(value: unknown): CliKind[] {
  const candidates = Array.isArray(value) ? value : []
  return CLI_KINDS.filter((kind) => candidates.includes(kind))
}

export const useTopBarStore = defineStore('topBar', () => {
  const order = ref<TopBarItem[]>([...DEFAULT_TOP_BAR_ORDER])
  const hidden = ref<CliKind[]>([])
  const loaded = ref(false)
  const cliOrder = computed<CliKind[]>(() => order.value.filter(isCliKind))
  const visibleOrder = computed<TopBarItem[]>(() =>
    order.value.filter((item) => item === 'config' || !hidden.value.includes(item))
  )

  function hydrateLayout(layout: PersistedTopBarLayout) {
    order.value = normalizeTopBarOrder(layout.order)
    hidden.value = normalizeTopBarHidden(layout.hidden)
    loaded.value = true
  }

  async function loadOrder() {
    if (loaded.value) return
    try {
      const layout = await invoke<PersistedTopBarLayout>('load_top_bar_layout')
      hydrateLayout(layout)
    } catch {
      order.value = [...DEFAULT_TOP_BAR_ORDER]
      hidden.value = []
    } finally {
      loaded.value = true
    }
  }

  async function saveLayout(nextOrder: TopBarItem[], nextHidden: CliKind[]) {
    const normalized = normalizeTopBarOrder(nextOrder)
    const normalizedHidden = normalizeTopBarHidden(nextHidden)
    await invoke('save_top_bar_layout', {
      layout: { order: normalized, hidden: normalizedHidden },
    })
    order.value = normalized
    hidden.value = normalizedHidden
  }

  return {
    order,
    hidden,
    loaded,
    cliOrder,
    visibleOrder,
    hydrateLayout,
    loadOrder,
    saveLayout,
  }
})
