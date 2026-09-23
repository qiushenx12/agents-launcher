<script setup lang="ts">
import { useTabCommStore } from '@/stores/tabComm'
// SnapshotManager no longer directly restores permissions/roles/canvas.
// It delegates to OrchestrationManager via pendingRestoreSnapshot.

const tabComm = useTabCommStore()

const projectPath = defineModel<string>({ default: '' })

async function handleSave() {
  if (!projectPath.value) {
    tabComm.showToast('请输入项目路径', 'error')
    return
  }
  await tabComm.saveSnapshot(projectPath.value)
}

async function handleLoad(path: string) {
  const snapshot = await tabComm.loadSnapshot(path)
  if (!snapshot) {
    tabComm.showToast('快照不存在', 'error')
    return
  }
  tabComm.pendingRestoreSnapshot = snapshot
  tabComm.closeSnapshotList()
  tabComm.showToast('快照已加载，编排恢复中...')
}
</script>

<template>
  <div class="modal-overlay" @click.self="tabComm.closeSnapshotList">
    <div class="snapshot-modal">
      <div class="snapshot-modal__header">终端快照管理</div>

      <div class="snapshot-modal__body">
        <!-- Save section -->
        <div class="snapshot-save">
          <input
            class="snapshot-save__input"
            type="text"
            v-model="projectPath"
            placeholder="项目路径 (如 D:\Project\my-app)"
          />
          <button class="btn btn-primary" @click="handleSave">保存快照</button>
        </div>

        <!-- Snapshot list -->
        <div class="snapshot-list">
          <div class="snapshot-list__title">已保存的快照</div>
          <div v-if="tabComm.snapshots.length === 0" class="snapshot-list__empty">
            暂无快照
          </div>
          <div
            v-for="snap in tabComm.snapshots"
            :key="snap.id"
            class="snapshot-item"
          >
            <div class="snapshot-item__info">
              <div class="snapshot-item__path">{{ snap.project_path }}</div>
              <div class="snapshot-item__time">{{ snap.timestamp }}</div>
            </div>
            <button class="btn btn-secondary" @click="handleLoad(snap.project_path)">
              加载
            </button>
          </div>
        </div>
      </div>

      <div class="snapshot-modal__footer">
        <button class="btn btn-secondary" @click="tabComm.closeSnapshotList">关闭</button>
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

.snapshot-modal {
  background: var(--card);
  border-radius: var(--radius);
  width: 480px;
  max-height: 80vh;
  overflow-y: auto;
  box-shadow: var(--modal-shadow);
}

.snapshot-modal__header {
  padding: 16px 20px;
  border-bottom: 1px solid var(--separator);
  font-size: var(--font-size-title);
  font-weight: 600;
}

.snapshot-modal__body {
  padding: 16px 20px;
}

.snapshot-save {
  display: flex;
  gap: 8px;
  margin-bottom: 20px;
}

.snapshot-save__input {
  flex: 1;
  padding: 6px 10px;
  font-size: var(--font-size-base);
  font-family: var(--font-mono);
  border: 1px solid var(--input-border);
  border-radius: var(--radius-sm);
  outline: none;
}

.snapshot-save__input:focus {
  border-color: var(--input-focus-border);
  box-shadow: 0 0 0 2px rgba(0, 122, 255, 0.15);
}

[data-theme="dark"] .snapshot-save__input:focus {
  box-shadow: 0 0 0 2px rgba(10, 132, 255, 0.25);
}

.snapshot-list__title {
  font-size: var(--font-size-small);
  font-weight: 600;
  color: var(--text-secondary);
  margin-bottom: 8px;
}

.snapshot-list__empty {
  font-size: var(--font-size-small);
  color: var(--text-secondary);
  padding: 12px 0;
}

.snapshot-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 12px;
  border: 1px solid var(--separator);
  border-radius: var(--radius-sm);
  margin-bottom: 6px;
}

.snapshot-item:hover {
  background: var(--bg);
}

.snapshot-item__path {
  font-size: var(--font-size-base);
  font-family: var(--font-mono);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 300px;
}

.snapshot-item__time {
  font-size: var(--font-size-small);
  color: var(--text-secondary);
  margin-top: 2px;
}

.snapshot-modal__footer {
  display: flex;
  justify-content: flex-end;
  padding: 12px 20px;
  border-top: 1px solid var(--separator);
}
</style>
