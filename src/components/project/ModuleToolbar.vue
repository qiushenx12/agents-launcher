<template>
  <header class="module-toolbar">
    <div class="module-toolbar__title">
      <span class="module-toolbar__session-name">
        {{ store.activeSession?.name ?? '未选择会话' }}
      </span>
      <span
        v-if="props.claudeError"
        class="module-toolbar__claude-error"
        role="status"
        :title="props.claudeError"
      >
        <span class="module-toolbar__claude-error-dot" aria-hidden="true"></span>
        <span class="module-toolbar__claude-error-text">{{ props.claudeError }}</span>
      </span>
    </div>
    <div class="module-toolbar__actions">
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
const props = withDefaults(defineProps<{
  claudeError?: string
}>(), {
  claudeError: '',
})

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

.module-toolbar__claude-error {
  min-width: 0;
  display: inline-flex;
  align-items: center;
  gap: 7px;
  color: #d29922;
  font-size: var(--font-size-small);
  font-weight: 500;
}

.module-toolbar__claude-error-dot {
  width: 8px;
  height: 8px;
  flex: 0 0 8px;
  border-radius: 50%;
  background: currentColor;
}

.module-toolbar__claude-error-text {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
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
