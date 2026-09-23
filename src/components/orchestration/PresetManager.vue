<script setup lang="ts">
import { ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useTabCommStore } from '@/stores/tabComm'
import type { OrchestrationPreset, PresetEntry } from '@/types/orchestration'

const props = defineProps<{
  mode: 'list' | 'create' | 'apply'
}>()

const emit = defineEmits<{
  save: [name: string, description: string]
  apply: [preset: OrchestrationPreset]
  close: []
}>()

const tabComm = useTabCommStore()

const internalMode = ref(props.mode)
const presetName = ref('')
const presetDescription = ref('')
const selectedPreset = ref<OrchestrationPreset | null>(null)
const selectedPresetEntry = ref<PresetEntry | null>(null)

watch(() => props.mode, (m) => {
  internalMode.value = m
})

function switchMode(mode: 'list' | 'create' | 'apply') {
  internalMode.value = mode
  if (mode === 'list') {
    tabComm.loadPresets()
  }
}

function onSave() {
  const name = presetName.value.trim()
  if (!name) {
    tabComm.showToast('请输入预设名称', 'error')
    return
  }
  emit('save', name, presetDescription.value.trim())
  presetName.value = ''
  presetDescription.value = ''
}

async function onApplyEntry(entry: PresetEntry) {
  try {
    const preset = await invoke<OrchestrationPreset | null>('load_preset', { id: entry.id })
    if (preset) {
      selectedPreset.value = preset
      selectedPresetEntry.value = entry
      internalMode.value = 'apply'
    } else {
      tabComm.showToast('预设不存在或已损坏', 'error')
    }
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : String(e)
    tabComm.showToast(`加载预设失败: ${msg}`, 'error')
  }
}

function onConfirmApply() {
  if (selectedPreset.value) {
    emit('apply', selectedPreset.value)
    selectedPreset.value = null
    selectedPresetEntry.value = null
  }
}

function onDelete(entry: PresetEntry, e: Event) {
  e.stopPropagation()
  if (confirm(`确定要删除预设 "${entry.name}" 吗？`)) {
    tabComm.deletePreset(entry.id)
  }
}

function formatDate(iso: string): string {
  try {
    const d = new Date(iso)
    return d.toLocaleString('zh-CN')
  } catch {
    return iso
  }
}
</script>

<template>
  <div class="modal-overlay" @click.self="emit('close')">
    <div class="preset-manager">
      <!-- Header -->
      <div class="preset-manager__header">
        <h3 v-if="internalMode === 'list'">编排预设管理</h3>
        <h3 v-else-if="internalMode === 'create'">保存为新预设</h3>
        <h3 v-else-if="internalMode === 'apply'">应用预设</h3>
        <button class="preset-manager__close" @click="emit('close')">&times;</button>
      </div>

      <!-- List mode -->
      <div v-if="internalMode === 'list'" class="preset-manager__body">
        <button class="btn btn-primary preset-manager__new-btn" @click="switchMode('create')">
          + 新建预设
        </button>

        <div v-if="tabComm.presets.length === 0" class="preset-manager__empty">
          暂无预设，点击上方按钮从当前编排创建
        </div>

        <div v-else class="preset-manager__list">
          <div
            v-for="entry in tabComm.presets"
            :key="entry.id"
            class="preset-card"
          >
            <div class="preset-card__info">
              <div class="preset-card__name">{{ entry.name }}</div>
              <div v-if="entry.description" class="preset-card__desc">{{ entry.description }}</div>
              <div class="preset-card__meta">更新于 {{ formatDate(entry.updatedAt) }}</div>
            </div>
            <div class="preset-card__actions">
              <button class="btn btn-primary" @click="onApplyEntry(entry)">应用</button>
              <button class="btn btn-secondary" @click="onDelete(entry, $event)">删除</button>
            </div>
          </div>
        </div>
      </div>

      <!-- Create mode -->
      <div v-else-if="internalMode === 'create'" class="preset-manager__body">
        <div class="preset-form">
          <label class="preset-form__label">
            预设名称 <span class="preset-form__required">*</span>
          </label>
          <input
            v-model="presetName"
            class="preset-form__input"
            placeholder="例如：前后端协作模式"
            @keyup.enter="onSave"
          />

          <label class="preset-form__label">预设描述</label>
          <input
            v-model="presetDescription"
            class="preset-form__input"
            placeholder="可选：描述该预设的用途"
          />
        </div>

        <div class="preset-manager__footer">
          <button class="btn btn-secondary" @click="switchMode('list')">返回</button>
          <button class="btn btn-primary" @click="onSave">保存</button>
        </div>
      </div>

      <!-- Apply mode -->
      <div v-else-if="internalMode === 'apply'" class="preset-manager__body">
        <div v-if="selectedPreset" class="preset-apply">
          <div class="preset-apply__info">
            <div class="preset-apply__name">{{ selectedPreset.name }}</div>
            <div v-if="selectedPreset.description" class="preset-apply__desc">
              {{ selectedPreset.description }}
            </div>
          </div>

          <div class="preset-apply__section">
            <div class="preset-apply__section-title">
              将创建 {{ selectedPreset.agents.length }} 个 Agent：
            </div>
            <div class="preset-apply__agents">
              <div
                v-for="agent in selectedPreset.agents"
                :key="agent.id"
                class="preset-apply__agent"
              >
                <span class="preset-apply__agent-name">{{ agent.name }}</span>
                <span class="preset-apply__agent-role">{{ agent.role?.name || '未命名角色' }}</span>
              </div>
            </div>
          </div>

          <div v-if="selectedPreset.connections.length > 0" class="preset-apply__section">
            <div class="preset-apply__section-title">
              将建立 {{ selectedPreset.connections.length }} 条连线
            </div>
          </div>
        </div>

        <div class="preset-manager__footer">
          <button class="btn btn-secondary" @click="switchMode('list')">取消</button>
          <button class="btn btn-primary" @click="onConfirmApply">确认应用</button>
        </div>
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

.preset-manager {
  background: var(--card);
  border-radius: var(--radius);
  box-shadow: var(--modal-shadow);
  width: 480px;
  max-width: 90vw;
  max-height: 80vh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.preset-manager__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px;
  border-bottom: 1px solid var(--separator);
}

.preset-manager__header h3 {
  margin: 0;
  font-size: var(--font-size-title);
  font-weight: 600;
  color: var(--text-primary);
}

.preset-manager__close {
  background: none;
  border: none;
  font-size: 22px;
  color: var(--text-secondary);
  cursor: pointer;
  line-height: 1;
  padding: 0 4px;
  border-radius: 4px;
  transition: color 0.12s ease;
}

.preset-manager__close:hover {
  color: var(--danger);
}

.preset-manager__body {
  padding: 16px 20px;
  overflow-y: auto;
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.preset-manager__new-btn {
  width: 100%;
  padding: 8px;
  font-size: var(--font-size-base);
}

.preset-manager__empty {
  text-align: center;
  color: var(--text-secondary);
  padding: 32px 0;
  font-size: var(--font-size-base);
}

.preset-manager__list {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.preset-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 14px;
  background: var(--tab-bg);
  border: 1px solid var(--separator);
  border-radius: var(--radius-sm);
  transition: border-color 0.12s ease;
}

.preset-card:hover {
  border-color: var(--primary);
}

.preset-card__info {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
  flex: 1;
}

.preset-card__name {
  font-size: var(--font-size-base);
  font-weight: 500;
  color: var(--text-primary);
}

.preset-card__desc {
  font-size: var(--font-size-small);
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.preset-card__meta {
  font-size: 11px;
  color: var(--text-secondary);
  opacity: 0.7;
}

.preset-card__actions {
  display: flex;
  gap: 6px;
  flex-shrink: 0;
}

.preset-form {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.preset-form__label {
  font-size: var(--font-size-small);
  color: var(--text-primary);
  font-weight: 500;
}

.preset-form__required {
  color: var(--danger);
}

.preset-form__input {
  padding: 8px 10px;
  border: 1px solid var(--separator);
  border-radius: var(--radius-sm);
  background: var(--bg);
  color: var(--text-primary);
  font-size: var(--font-size-base);
  outline: none;
  transition: border-color 0.12s ease;
}

.preset-form__input:focus {
  border-color: var(--primary);
}

.preset-manager__footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding-top: 8px;
  border-top: 1px solid var(--separator);
  margin-top: auto;
}

.preset-apply__info {
  margin-bottom: 8px;
}

.preset-apply__name {
  font-size: var(--font-size-title);
  font-weight: 600;
  color: var(--text-primary);
}

.preset-apply__desc {
  font-size: var(--font-size-small);
  color: var(--text-secondary);
  margin-top: 4px;
}

.preset-apply__section {
  margin-top: 8px;
}

.preset-apply__section-title {
  font-size: var(--font-size-small);
  font-weight: 500;
  color: var(--text-primary);
  margin-bottom: 6px;
}

.preset-apply__agents {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.preset-apply__agent {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 10px;
  background: var(--tab-bg);
  border: 1px solid var(--separator);
  border-radius: var(--radius-sm);
}

.preset-apply__agent-name {
  font-size: var(--font-size-small);
  color: var(--text-primary);
}

.preset-apply__agent-role {
  font-size: 11px;
  color: var(--text-secondary);
  background: rgba(0, 122, 255, 0.1);
  padding: 2px 6px;
  border-radius: var(--radius-sm);
}

[data-theme="dark"] .preset-apply__agent-role {
  background: rgba(10, 132, 255, 0.2);
}
</style>
