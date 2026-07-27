<template>
  <section class="claude-log-pane">
    <header class="claude-log-pane__header">
      <div>
        <strong>终端诊断日志</strong>
        <p>仅记录稳定后的屏幕差分，不逐帧保存 Claude 的终端重绘。</p>
      </div>
      <div class="claude-log-pane__actions">
        <button class="btn btn-secondary" @click="refresh">刷新</button>
        <button class="btn btn-secondary" :disabled="!state.logDir" @click="openDirectory">
          打开日志目录
        </button>
      </div>
    </header>
    <div v-if="state.degradedReason" class="claude-log-pane__warning">
      {{ state.degradedReason }}
    </div>
    <pre v-if="state.terminalLog" class="claude-log-pane__content">{{ state.terminalLog }}</pre>
    <div v-else class="claude-log-pane__empty">尚未产生稳定的终端屏幕差分。</div>
  </section>
</template>

<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useClaudeObserverStore } from '@/stores/claudeObserver'

const props = defineProps<{
  tabId: number
  projectSessionId: string
}>()
const observerStore = useClaudeObserverStore()
const state = computed(() => observerStore.states[props.tabId] ?? {
  tabId: props.tabId,
  statusRevision: 0,
  available: false,
  active: true,
  sessionReady: false,
  runState: 'starting' as const,
  items: [],
  terminalLog: '',
  loading: true,
})

async function refresh() {
  await observerStore.refreshTerminalLog(props.tabId, props.projectSessionId)
}

async function openDirectory() {
  try {
    await observerStore.openLogDirectory(props.tabId, props.projectSessionId)
  } catch (error) {
    state.value.degradedReason = `打开日志目录失败：${error}`
  }
}

onMounted(async () => {
  await observerStore.loadSnapshot(props.tabId)
  await refresh()
})
</script>

<style scoped>
.claude-log-pane {
  --claude-log-page-bg: var(--card, #fff);
  --claude-log-surface-bg: var(--bg);
  --claude-log-border: var(--separator);
  position: absolute;
  inset: 0;
  z-index: 4;
  display: flex;
  flex-direction: column;
  color: var(--text-primary);
  background: var(--claude-log-page-bg);
}

[data-theme="dark"] .claude-log-pane {
  --claude-log-page-bg: #080a0d;
  --claude-log-surface-bg: #101318;
  --claude-log-border: #252b33;
}

.claude-log-pane__header {
  min-height: 72px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 18px;
  padding: 12px 170px 12px 18px;
  border-bottom: 1px solid var(--claude-log-border);
}

.claude-log-pane__header p {
  margin: 4px 0 0;
  color: var(--text-secondary);
  font-size: 12px;
}

.claude-log-pane__actions { display: flex; gap: 8px; }
.claude-log-pane__warning {
  padding: 8px 18px;
  color: #d29922;
  background: color-mix(in srgb, #d29922 10%, var(--claude-log-page-bg));
}
.claude-log-pane__content {
  flex: 1;
  min-height: 0;
  margin: 0;
  padding: 16px 20px 30px;
  overflow: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  color: var(--text-primary);
  background: var(--claude-log-surface-bg);
  font-family: var(--font-mono);
  font-size: 12px;
  line-height: 1.5;
}
.claude-log-pane__empty {
  flex: 1;
  display: grid;
  place-content: center;
  color: var(--text-secondary);
}
</style>
