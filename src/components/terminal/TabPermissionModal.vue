<script setup lang="ts">
import { computed } from 'vue'
import { useTabCommStore } from '@/stores/tabComm'
import { useTerminalStore } from '@/stores/terminal'

const tabComm = useTabCommStore()
const termStore = useTerminalStore()

// All other tabs (exclude the config tab itself)
const otherTabs = computed(() =>
  termStore.tabs.filter(t => t.id !== tabComm.permConfigTabId)
)

// Sync checkbox state
const isEnabled = computed({
  get: () => tabComm.permEnabled,
  set: (v: boolean) => { tabComm.permEnabled = v },
})

function toggleTarget(tabId: number) {
  const idx = tabComm.permAllowedTargets.indexOf(tabId)
  if (idx >= 0) {
    tabComm.permAllowedTargets.splice(idx, 1)
  } else {
    tabComm.permAllowedTargets.push(tabId)
  }
}

function isTargetSelected(tabId: number): boolean {
  return tabComm.permAllowedTargets.includes(tabId)
}
</script>

<template>
  <div class="modal-overlay" @click.self="tabComm.closePermConfig">
    <div class="perm-modal">
      <div class="perm-modal__header">Tab 通信权限配置</div>

      <div class="perm-modal__body">
        <label class="perm-row">
          <input type="checkbox" v-model="isEnabled" />
          <span>启用 Tab 通信工具</span>
        </label>

        <div class="perm-section">
          <div class="perm-section__title">允许操作的目标标签页</div>
          <p class="perm-section__hint" v-if="tabComm.permAllowedTargets.length === 0">
            (留空表示允许所有标签页)
          </p>
          <div class="perm-target-list">
            <label v-for="tab in otherTabs" :key="tab.id" class="perm-target-item">
              <input type="checkbox" :checked="isTargetSelected(tab.id)" @change="toggleTarget(tab.id)" />
              <span>Tab {{ tab.id }} — {{ tab.title }}</span>
            </label>
          </div>
        </div>
      </div>

      <div class="perm-modal__footer">
        <button class="btn btn-secondary" @click="tabComm.closePermConfig">取消</button>
        <button class="btn btn-primary" @click="tabComm.savePermission">保存</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.modal-overlay {
  position: fixed;
  inset: 0;
  background: var(--overlay);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}

.perm-modal {
  background: var(--card);
  border-radius: var(--radius);
  width: 420px;
  max-height: 80vh;
  overflow-y: auto;
  box-shadow: var(--modal-shadow);
}

.perm-modal__header {
  padding: 16px 20px;
  border-bottom: 1px solid var(--separator);
  font-size: var(--font-size-title);
  font-weight: 600;
  color: var(--text-primary);
}

.perm-modal__body {
  padding: 16px 20px;
}

.perm-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 16px;
  font-size: var(--font-size-base);
}

.perm-section__title {
  font-size: var(--font-size-small);
  font-weight: 600;
  color: var(--text-secondary);
  margin-bottom: 4px;
}

.perm-section__hint {
  font-size: var(--font-size-small);
  color: var(--text-secondary);
  margin-bottom: 8px;
}

.perm-target-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.perm-target-item {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: var(--font-size-base);
  padding: 6px 8px;
  border-radius: var(--radius-sm);
  cursor: pointer;
}

.perm-target-item:hover {
  background: var(--bg);
}

.perm-modal__footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 20px;
  border-top: 1px solid var(--separator);
}
</style>
