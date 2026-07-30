<template>
  <header class="module-toolbar">
    <div class="module-toolbar__title">
      <span class="module-toolbar__session-name">
        {{ store.activeSession?.name ?? '未选择会话' }}
      </span>
      <span
        v-if="props.showClaudeControls"
        class="module-toolbar__claude-status"
        :class="`is-${props.claudeStatusState}`"
      >
        <span class="module-toolbar__claude-status-dot" aria-hidden="true"></span>
        <span>{{ props.claudeStatus }}</span>
        <span v-if="props.claudeStatusDetail" class="module-toolbar__claude-status-detail">
          {{ props.claudeStatusDetail }}
        </span>
      </span>
    </div>
    <div class="module-toolbar__actions">
      <nav
        v-if="props.showClaudeControls"
        class="module-toolbar__claude-view-switch"
        aria-label="Claude 会话视图"
      >
        <button
          v-for="option in claudeViewOptions"
          :key="option.value"
          type="button"
          :class="{ active: props.claudeView === option.value }"
          @click="emit('select-claude-view', option.value)"
        >
          {{ option.label }}
        </button>
      </nav>
      <button
        class="module-toolbar__sidebar"
        :class="{ active: store.bottomSidebarOpen }"
        title="展开/收起下侧边栏"
        aria-label="展开/收起下侧边栏"
        @click="toggleBottomSidebar"
      >
        <span class="sidebar-toggle-icon sidebar-toggle-icon--bottom" aria-hidden="true"></span>
      </button>
      <button
        class="module-toolbar__sidebar"
        :class="{ active: store.sidebarOpen && store.sidebarPlacement === 'top' }"
        title="展开/收起上侧边栏"
        aria-label="展开/收起上侧边栏"
        @click="toggleTopSidebar"
      >
        <span class="sidebar-toggle-icon sidebar-toggle-icon--top" aria-hidden="true"></span>
      </button>
      <button
        class="module-toolbar__sidebar"
        :class="{ active: store.sidebarOpen && store.sidebarPlacement === 'right' }"
        title="展开/收起右侧边栏"
        aria-label="展开/收起右侧边栏"
        @click="toggleSidebar"
      >
        <span class="sidebar-toggle-icon sidebar-toggle-icon--right" aria-hidden="true"></span>
      </button>
    </div>
  </header>
</template>

<script setup lang="ts">
import { useProjectStore } from '@/stores/project'

const store = useProjectStore()
type ClaudeView = 'conversation' | 'terminal' | 'log'

const props = withDefaults(defineProps<{
  showClaudeControls?: boolean
  claudeView?: ClaudeView
  claudeStatus?: string
  claudeStatusState?: string
  claudeStatusDetail?: string
}>(), {
  showClaudeControls: false,
  claudeView: 'conversation',
  claudeStatus: '',
  claudeStatusState: 'starting',
  claudeStatusDetail: '',
})
const emit = defineEmits<{
  (event: 'select-claude-view', view: ClaudeView): void
}>()
const claudeViewOptions: Array<{ value: ClaudeView; label: string }> = [
  { value: 'conversation', label: '对话' },
  { value: 'terminal', label: '终端' },
  { value: 'log', label: '日志' },
]

function toggleBottomSidebar() {
  if (store.bottomSidebarOpen) {
    store.closeBottomSidebar()
  } else {
    store.openBottomSidebar()
  }
}

function toggleSidebar() {
  if (store.sidebarOpen && store.sidebarPlacement === 'right') {
    store.closeSidebar()
  } else {
    store.openSidebar('tools', 'right')
  }
}

function toggleTopSidebar() {
  if (store.sidebarOpen && store.sidebarPlacement === 'top') {
    store.closeSidebar()
  } else {
    store.openSidebar('tools', 'top')
  }
}
</script>

<style scoped>
.module-toolbar {
  height: 38px;
  flex: 0 0 38px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 0 10px;
  border-bottom: 1px solid var(--separator);
  background: var(--card-bg-gradient);
}

.module-toolbar__title {
  min-width: 0;
  flex: 1;
  display: flex;
  align-items: center;
  gap: 10px;
  overflow: hidden;
  color: var(--text-primary);
  font-weight: 600;
}

.module-toolbar__session-name {
  min-width: 0;
  max-width: min(460px, 55%);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.module-toolbar__actions {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 6px;
}

.module-toolbar__claude-status {
  min-width: 0;
  display: inline-flex;
  align-items: center;
  gap: 7px;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  font-weight: 400;
  white-space: nowrap;
}

.module-toolbar__claude-status-dot {
  width: 8px;
  height: 8px;
  flex: 0 0 8px;
  border-radius: 50%;
  background: #8b8b8b;
}

.module-toolbar__claude-status.is-idle .module-toolbar__claude-status-dot { background: #3fb950; }
.module-toolbar__claude-status.is-working .module-toolbar__claude-status-dot { background: #d29922; }
.module-toolbar__claude-status.is-permission .module-toolbar__claude-status-dot { background: #58a6ff; }

.module-toolbar__claude-status-detail {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  color: #d29922;
}

.module-toolbar__claude-view-switch {
  display: flex;
  align-items: center;
  padding: 2px;
  border: 1px solid var(--separator);
  border-radius: 7px;
  background: var(--bg);
}

.module-toolbar__claude-view-switch button {
  min-width: 44px;
  padding: 4px 9px;
  border: 0;
  border-radius: 5px;
  color: var(--text-secondary);
  background: transparent;
  font-size: 12px;
  cursor: pointer;
}

.module-toolbar__claude-view-switch button:hover {
  color: var(--text-primary);
  background: var(--card);
}

.module-toolbar__claude-view-switch button.active {
  color: #fff;
  background: var(--primary);
}

.module-toolbar__sidebar {
  width: 28px;
  height: 28px;
  display: grid;
  place-items: center;
  border: 1px solid var(--separator);
  border-radius: var(--radius-sm);
  background: var(--card);
  color: var(--text-secondary);
  cursor: pointer;
}

.module-toolbar__sidebar.active {
  color: var(--primary);
  background: rgba(0, 122, 255, 0.08);
}
</style>
