<template>
  <div class="opencode-config-panel">
    <aside class="provider-sidebar" :style="{ width: `${leftWidth}px`, flexBasis: `${leftWidth}px` }">
      <button class="btn btn-primary provider-sidebar__new" type="button" @click="store.addProvider()">
        新建供应商
      </button>

      <div class="provider-sidebar__body">
        <div v-if="store.loading && !store.loaded" class="provider-sidebar__empty">
          正在读取…
        </div>
        <div v-else-if="config.providers.length === 0" class="provider-sidebar__empty">
          暂无自定义供应商
        </div>
        <div v-else class="provider-list">
        <button
          v-for="(item, index) in store.orderedProviders"
          :key="store.providerKey(item)"
          data-drag-item
          class="provider-list__item"
          :class="{
            'provider-list__item--selected': store.selectedProvider === item,
            'provider-list__item--dragging': draggingIndex === index,
            'provider-list__item--drag-over': draggingIndex !== null
              && draggingIndex !== index
              && overIndex === index,
          }"
          type="button"
          @click="onProviderClick(item)"
        >
          <span
            class="provider-list__drag-handle"
            title="拖拽排序"
            @pointerdown="onPointerDown(index, $event)"
          />
          <span class="provider-list__content">
            <strong>{{ item.name || item.id || `供应商 ${index + 1}` }}</strong>
            <small>{{ item.id }} · {{ item.models.length }} 个模型</small>
          </span>
          <span
            class="provider-list__state"
            :class="{
              'provider-list__state--draft': !item.originalId
                || (store.selectedProvider === item && store.isSelectedProviderDirty),
            }"
          >
            {{ !item.originalId
              ? '未写入'
              : store.selectedProvider === item && store.isSelectedProviderDirty
                ? '待更新'
                : '已写入' }}
          </span>
        </button>
        </div>
      </div>
      <footer class="provider-sidebar__footer">
        <button class="settings-entry" type="button" @click="toggleSettings()">⚙ <span>设置</span></button>
      </footer>
    </aside>

    <div
      class="opencode-config-panel__divider"
      :class="{ 'opencode-config-panel__divider--dragging': isDragging }"
      @mousedown="onMouseDown"
    />

    <main class="config-content">
      <section class="card global-options">
        <header class="editor-header">
          <div>
            <div class="card-title">OpenCode 全局选项</div>
            <p>{{ config.configPath || defaultConfigPath }}</p>
          </div>
          <button class="btn btn-secondary" type="button" :disabled="store.loading" @click="store.refreshConfig()">
            {{ store.loading ? '读取中…' : '重新读取' }}
          </button>
        </header>

        <section
          v-if="store.permissionStatus.supported && store.permissionStatus.requiresRepair"
          class="permission-setup"
          role="alert"
        >
          <div>
            <strong>首次使用需要获取 OpenCode 目录权限</strong>
            <p>检测到以下目录不可写或不属于当前用户：</p>
            <code v-for="path in store.permissionStatus.blockedDirectories" :key="path">{{ path }}</code>
            <p>点击按钮后 macOS 会显示管理员授权窗口。启动器只会创建并修复三个 OpenCode 专属目录，不会修改整个用户目录。</p>
          </div>
          <button
            class="btn btn-primary"
            type="button"
            :disabled="store.permissionRepairing"
            @click="store.repairPermissions()"
          >
            {{ store.permissionRepairing ? '等待 macOS 授权…' : '一键修复权限' }}
          </button>
        </section>
        <p v-else-if="store.permissionError" class="permission-check-error">
          {{ store.permissionError }}
        </p>

        <ModelField
          v-model="config.model"
          label="默认模型"
          :models="store.configuredModelIds"
          placeholder="provider_id/model_id；留空使用最近选择"
        />
        <ModelField
          v-model="config.smallModel"
          label="Small model"
          :models="store.configuredModelIds"
          placeholder="可留空"
        />
        <div class="global-action-row">
          <button
            class="btn btn-secondary"
            type="button"
            :disabled="!store.isGlobalOptionsDirty || store.saving || store.permissionStatus.requiresRepair"
            @click="store.saveGlobalOptions()"
          >
            {{ store.isGlobalOptionsDirty ? '保存全局选项' : '全局选项已同步' }}
          </button>
          <button class="btn btn-secondary" type="button" @click="openConfigDirectory">
            打开配置目录
          </button>
        </div>
      </section>

      <section v-if="provider" class="card provider-editor">
        <header class="provider-header">
          <div>
            <div class="card-title">{{ provider.name || provider.id || '未命名供应商' }}</div>
            <span>
              {{ provider.originalId
                ? '当前供应商已存在于目标 JSON；写入只更新这一项'
                : '当前为新供应商草稿；写入后加入目标 JSON' }}
            </span>
          </div>
          <span
            class="provider-write-state"
            :class="{ 'provider-write-state--draft': store.isSelectedProviderDirty }"
          >
            {{ !provider.originalId ? '未写入' : store.isSelectedProviderDirty ? '待更新' : '已写入' }}
          </span>
        </header>

        <div class="field-row">
          <label class="field-label">配置名称</label>
          <input v-model="provider.name" class="input" type="text" placeholder="例如 llama-cpp" />
        </div>
        <div class="field-row">
          <label class="field-label">Provider ID</label>
          <input v-model="provider.id" class="input" type="text" placeholder="例如 llama-cpp" />
        </div>
        <div class="field-row">
          <label class="field-label">API 地址</label>
          <div class="field-inline">
            <input v-model="provider.baseUrl" class="input" type="text" placeholder="https://api.example.com/v1" />
            <button
              class="btn btn-secondary"
              type="button"
              :disabled="store.modelsFetchingId !== null"
              @click="store.fetchModels(provider)"
            >
              {{ store.modelsFetchingId === store.providerKey(provider) ? '获取中…' : '获取模型' }}
            </button>
          </div>
        </div>

        <SecretField
          v-model="store.connectionKeyInputs[store.providerKey(provider)]"
          label="连接 Key"
          placeholder="写入供应商后可保存到 OpenCode auth.json"
        />
        <div class="key-action-row">
          <button
            class="btn btn-secondary"
            type="button"
            :disabled="!provider.originalId || store.permissionStatus.requiresRepair"
            @click="store.saveProviderKey(provider)"
          >
            保存 Key
          </button>
          <span>{{ provider.originalId ? '单独保存到 auth.json' : '请先将供应商写入目标 JSON' }}</span>
        </div>

        <div class="field-row connection-row">
          <label class="field-label">Provider 状态</label>
          <div class="connection-control">
            <span
              class="connection-state"
              :class="{
                'connection-state--online': store.connectionState(provider) === 'connected',
                'connection-state--disabled': store.connectionState(provider) === 'disabled',
                'connection-state--offline': store.connectionState(provider) === 'disconnected',
              }"
            >
              {{ connectionStateLabel(provider) }}
            </span>
            <button
              class="btn btn-secondary"
              type="button"
              :disabled="!provider.originalId || store.permissionStatus.requiresRepair"
              @click="store.saveConnection(provider)"
            >
              {{ connectionButtonLabel(provider) }}
            </button>
            <button
              v-if="store.connectionState(provider) !== 'disabled'"
              class="btn btn-secondary"
              type="button"
              :disabled="!provider.originalId || store.permissionStatus.requiresRepair"
              @click="store.disconnectConnection(provider)"
            >
              禁用
            </button>
          </div>
        </div>
        <p class="connection-help">
          启用或禁用只调整 <code>disabled_providers</code>，不会删除供应商配置或 Key。
        </p>

        <details class="advanced-options">
          <summary>高级：直接在 JSONC 配置 API Key</summary>
          <SecretField
            v-model="provider.apiKey"
            label="配置 API Key"
            placeholder="可填写密钥或 {env:变量名}"
          />
          <p>这是 <code>options.apiKey</code>，可在不使用 OpenCode 连接凭据时作为替代。</p>
          <p v-if="isPlaintextApiKey(provider.apiKey)" class="inline-key-warning">
            当前值会以明文写入配置文件；Unix 上启动器会将文件权限收紧为仅当前用户可读写。优先使用 auth.json 或 <code>{env:变量名}</code>。
          </p>
        </details>

        <section class="models-section">
          <header class="models-header">
            <div>
              <strong>模型</strong>
              <span>Text / Image 会同步到 modalities.input；其它未展示参数保持原样</span>
            </div>
          </header>

          <div class="model-add-row">
            <input
              v-model="modelDrafts[store.providerKey(provider)]"
              class="input"
              type="text"
              :list="`models-${store.providerKey(provider)}`"
              placeholder="输入或选择模型 ID"
              @keydown.enter.prevent="addModel(provider)"
            />
            <datalist :id="`models-${store.providerKey(provider)}`">
              <option
                v-for="modelId in store.availableModels[store.providerKey(provider)] || []"
                :key="modelId"
                :value="modelId"
              />
            </datalist>
            <button class="btn btn-secondary" type="button" @click="addModel(provider)">
              添加模型
            </button>
          </div>

          <div v-if="provider.models.length === 0" class="model-empty">暂无模型</div>
          <div v-else class="model-list">
            <div class="model-columns" aria-hidden="true">
              <span>模型 ID</span>
              <span>显示名称</span>
              <span>上下文长度</span>
              <span>输出上限</span>
              <span>输入能力</span>
              <span></span>
            </div>
            <div v-for="model in provider.models" :key="model.originalId || model.id" class="model-row">
              <input v-model="model.id" class="input" type="text" aria-label="模型 ID" />
              <input v-model="model.name" class="input" type="text" aria-label="模型显示名称" />
              <input
                :value="model.contextLimit ?? ''"
                class="input"
                type="number"
                min="1"
                step="1"
                aria-label="上下文长度"
                placeholder="例如 200000"
                @input="updateModelLimit(model, 'contextLimit', $event)"
              />
              <input
                :value="model.outputLimit ?? ''"
                class="input"
                type="number"
                min="1"
                step="1"
                aria-label="输出上限"
                placeholder="例如 65536"
                @input="updateModelLimit(model, 'outputLimit', $event)"
              />
              <div class="modality-options" aria-label="模型输入能力">
                <label><input v-model="model.inputText" type="checkbox" /> Text</label>
                <label><input v-model="model.inputImage" type="checkbox" /> Image</label>
              </div>
              <button class="icon-button" type="button" title="删除模型" @click="store.removeModel(provider, model.id)">×</button>
            </div>
          </div>
        </section>

        <div class="provider-action-row">
          <button
            class="btn btn-primary"
            type="button"
            :disabled="store.saving || store.permissionStatus.requiresRepair"
            @click="store.writeSelectedProvider()"
          >
            {{ store.saving ? '处理中…' : provider.originalId ? '写入当前修改' : '写入目标 JSON' }}
          </button>
          <button
            class="btn btn-secondary danger-button"
            type="button"
            :disabled="store.saving || store.permissionStatus.requiresRepair"
            @click="store.deleteSelectedProvider()"
          >
            {{ provider.originalId ? '从目标 JSON 删除' : '删除草稿' }}
          </button>
          <span>仅操作当前供应商，列表中的其他供应商不会被覆盖。</span>
        </div>
      </section>

      <section v-else class="card provider-empty">
        <div class="card-title">供应商配置</div>
        <p>从左侧选择一个供应商，或新建供应商后写入目标 JSON。</p>
      </section>

      <ConfigStatusBanner
        v-if="store.statusMessage"
        :message="store.statusMessage"
        :tone="statusTone"
      />

      <section class="card source-note">
        <div class="card-title">同步说明</div>
        <p>左侧显示 <code>{{ config.configPath || defaultConfigPath }}</code> 中带 <code>npm</code> 的自定义供应商；内置供应商不会显示，也不会被修改。</p>
        <p>“写入”只新增或更新当前供应商，“删除”只移除当前供应商；其他供应商和未知字段保持不变，因此多个列表供应商可以同时写入并共存。</p>
        <p>“保存 Key”单独写入 <code>{{ config.authPath || '~/.local/share/opencode/auth.json' }}</code>；从目标 JSON 删除供应商时不会删除 Key。</p>
        <div class="preflight-entry">
          <button class="btn btn-secondary" type="button" @click="workspaceStore.openPreflight()">启动前检测</button>
          <span>启动时会重新读取目标文件，并在项目目录解析当前有效配置。</span>
        </div>
      </section>
    </main>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { OpencodeGlobalModel, OpencodeGlobalProvider } from '@/types/config'
import { useOpencodeConfigStore } from '@/stores/opencodeConfig'
import { useConfigWorkspaceStore } from '@/stores/configWorkspace'
import ConfigStatusBanner from '@/components/config/ConfigStatusBanner.vue'
import ModelField from '@/components/config/ModelField.vue'
import SecretField from '@/components/config/SecretField.vue'
import { useDragReorder } from '@/composables/useDragReorder'
import { useResizablePanes } from '@/composables/useResizablePanes'
import { useSettingsPopover } from '@/composables/useSettingsPopover'

const { toggleSettings } = useSettingsPopover()
import { usePlatform } from '@/composables/usePlatform'

const store = useOpencodeConfigStore()
const workspaceStore = useConfigWorkspaceStore()
const { isWindows } = usePlatform()
const { draggingIndex, overIndex, justDragged, onPointerDown } = useDragReorder(
  () => store.orderedProviders.map(item => store.providerKey(item)),
  (newOrder: string[]) => store.reorderProviders(newOrder),
)
const config = computed(() => store.editingConfig)
const provider = computed(() => store.selectedProvider)
const defaultConfigPath = computed(() => isWindows.value
  ? 'C:\\Users\\<用户>\\.config\\opencode\\opencode.jsonc'
  : '~/.config/opencode/opencode.jsonc')
const modelDrafts = reactive<Record<string, string>>({})
const statusTone = computed<'info' | 'success' | 'warning' | 'error'>(() => {
  if (/失败|错误|无效|不存在|已被其它程序修改|已不在目标/.test(store.statusMessage)) return 'error'
  if (/已保存|已同步|已读取|已获取|已写入|已删除|已移除|读取 \d+ 个/.test(store.statusMessage)) return 'success'
  return 'info'
})

function connectionStateLabel(item: OpencodeGlobalProvider) {
  const state = store.connectionState(item)
  if (state === 'connected') return '已连接'
  if (state === 'disabled') return '已禁用'
  return '未连接'
}

function onProviderClick(item: OpencodeGlobalProvider) {
  if (justDragged.value) return
  store.selectProvider(item)
}

function connectionButtonLabel(item: OpencodeGlobalProvider) {
  const state = store.connectionState(item)
  if (state === 'connected') return '重新连接'
  if (state === 'disabled') return '重新启用'
  return '启用'
}

function isPlaintextApiKey(value: string) {
  const key = value.trim()
  return Boolean(key)
    && !(key.startsWith('{env:') && key.endsWith('}'))
    && !(key.startsWith('{file:') && key.endsWith('}'))
}

function addModel(item: OpencodeGlobalProvider) {
  const key = store.providerKey(item)
  if (store.addModel(item, modelDrafts[key] || '')) modelDrafts[key] = ''
}

function updateModelLimit(
  model: OpencodeGlobalModel,
  field: 'contextLimit' | 'outputLimit',
  event: Event,
) {
  const value = (event.target as HTMLInputElement).value.trim()
  if (!value) {
    model[field] = null
    return
  }
  const parsed = Number(value)
  model[field] = Number.isSafeInteger(parsed) && parsed > 0 ? parsed : null
}

async function openConfigDirectory() {
  const path = config.value.configPath
  const separator = Math.max(path.lastIndexOf('\\'), path.lastIndexOf('/'))
  if (!path || separator < 0) {
    store.statusMessage = '配置目录尚不可用'
    return
  }
  try {
    await invoke('open_directory', { path: path.slice(0, separator) })
  } catch (error) {
    store.statusMessage = `打开配置目录失败：${error}`
  }
}

const { leftWidth, isDragging, onMouseDown, loadSizes, saveSizes } = useResizablePanes(280, 200, 400)
const PANE_KEY = 'opencode-config-panel'

onMounted(() => {
  loadSizes(PANE_KEY).catch(() => {})
  store.checkPermissions().catch(() => {})
  store.loadConfig(true).catch(() => {})
})

watch(isDragging, (dragging) => {
  if (!dragging) saveSizes(PANE_KEY).catch(() => {})
})
</script>

<style scoped>
.opencode-config-panel {
  height: 100%;
  min-height: 0;
  display: flex;
  background: var(--bg);
}

.provider-sidebar {
  width: 280px;
  flex: 0 0 auto;
  min-width: 0;
  padding: 12px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.opencode-config-panel__divider {
  width: 9px;
  flex-shrink: 0;
  cursor: col-resize;
  background: transparent;
  position: relative;
  z-index: 10;
  display: flex;
  align-items: center;
  justify-content: center;
}

.opencode-config-panel__divider::after {
  content: '';
  width: 1px;
  height: 100%;
  background-color: var(--separator);
  transition: background-color 0.2s ease, width 0.2s ease, box-shadow 0.2s ease;
}

.opencode-config-panel__divider:hover::after,
.opencode-config-panel__divider--dragging::after {
  width: 2px;
  background-color: var(--primary);
}

[data-theme="dark"] .opencode-config-panel__divider:hover::after,
[data-theme="dark"] .opencode-config-panel__divider--dragging::after {
  box-shadow: 0 0 6px 1px rgba(10, 132, 255, 0.5);
}

.provider-sidebar__body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.provider-sidebar__footer {
  flex-shrink: 0;
  padding-top: 8px;
  border-top: 1px solid var(--separator);
}

.provider-sidebar__new { width: 100%; margin-bottom: 8px; }
.provider-sidebar__empty { padding: 18px 8px; color: var(--text-secondary); text-align: center; font-size: var(--font-size-small); }
.provider-list { display: flex; flex-direction: column; gap: 2px; }
.provider-list__item { width: 100%; padding: 8px 10px; display: flex; align-items: center; gap: 8px; border: 0; border-radius: var(--radius-sm); color: var(--text-primary); background: transparent; cursor: pointer; text-align: left; transition: background-color 0.12s ease, transform 0.18s ease; user-select: none; position: relative; will-change: transform; }
.provider-list__item:hover { background: var(--tab-bg); }
.provider-list__item--selected { color: #fff; background: var(--primary); }
.provider-list__item--selected:hover { background: var(--primary-hover); }
.provider-list__item--dragging { opacity: 0.3; background: var(--tab-bg); }
.provider-list__drag-handle { width: 14px; height: 14px; flex-shrink: 0; position: relative; cursor: grab; opacity: 0; transition: opacity 0.12s ease; touch-action: none; }
.provider-list__drag-handle::before, .provider-list__drag-handle::after { content: ''; position: absolute; left: 1px; width: 2.5px; height: 2.5px; border-radius: 50%; background-color: var(--text-secondary); box-shadow: 5px 0 0 var(--text-secondary), 10px 0 0 var(--text-secondary); }
.provider-list__drag-handle::before { top: 1.5px; }
.provider-list__drag-handle::after { bottom: 1.5px; }
.provider-list__drag-handle:active { cursor: grabbing; }
.provider-list__item:hover .provider-list__drag-handle { opacity: 1; }
.provider-list__item--selected .provider-list__drag-handle::before, .provider-list__item--selected .provider-list__drag-handle::after { background-color: rgba(255, 255, 255, 0.72); box-shadow: 5px 0 0 rgba(255, 255, 255, 0.72), 10px 0 0 rgba(255, 255, 255, 0.72); }
.provider-list__content { min-width: 0; flex: 1; display: flex; flex-direction: column; gap: 2px; }
.provider-list__content strong, .provider-list__content small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.provider-list__content small { opacity: 0.72; font-size: var(--font-size-small); }
.provider-list__state { flex: 0 0 auto; padding: 2px 5px; border-radius: 4px; color: var(--success, #22c55e); background: color-mix(in srgb, var(--success, #22c55e) 12%, transparent); font-size: 10px; }
.provider-list__state--draft { color: var(--warning, #d49a45); background: color-mix(in srgb, var(--warning, #d49a45) 13%, transparent); }
.provider-list__item--selected .provider-list__state { color: #fff; background: rgba(255, 255, 255, 0.18); }

.config-content { min-width: 0; flex: 1; height: 100%; padding: 12px 16px; overflow-y: auto; }
.global-options, .provider-editor, .provider-empty, .source-note { max-width: 980px; margin: 0 auto 12px; }
.editor-header, .provider-header, .models-header { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.editor-header { margin-bottom: 10px; }
.editor-header p { margin: 4px 0 0; overflow-wrap: anywhere; }
.editor-header p, .provider-header span, .models-header span, .source-note p { color: var(--text-secondary); font-size: var(--font-size-small); }
.global-action-row { display: flex; gap: 8px; margin-top: 9px; padding-top: 10px; border-top: 1px solid var(--separator); }

.permission-setup { display: flex; align-items: center; justify-content: space-between; gap: 14px; margin: 0 0 12px; padding: 12px; border: 1px solid var(--warning, #b26a00); border-radius: var(--radius-md); background: color-mix(in srgb, var(--warning, #b26a00) 10%, transparent); }
.permission-setup > div { min-width: 0; }
.permission-setup p { margin: 4px 0; color: var(--text-secondary); font-size: var(--font-size-small); line-height: 1.45; }
.permission-setup code { display: block; margin-top: 2px; overflow-wrap: anywhere; }
.permission-setup .btn { flex: 0 0 auto; }
.permission-check-error { margin: 0 0 12px; color: var(--danger, #d96c6c); font-size: var(--font-size-small); }

.provider-header { margin-bottom: 9px; }
.provider-header > div, .models-header > div { display: flex; flex-direction: column; gap: 2px; }
.provider-write-state { flex: 0 0 auto; padding: 3px 7px; border-radius: 999px; color: var(--success, #22c55e); background: color-mix(in srgb, var(--success, #22c55e) 12%, transparent); font-size: var(--font-size-small); }
.provider-write-state--draft { color: var(--warning, #d49a45); background: color-mix(in srgb, var(--warning, #d49a45) 13%, transparent); }
.field-row { display: flex; align-items: center; gap: 10px; padding: 5px 0; }
.field-label { width: 110px; flex: 0 0 auto; color: var(--text-secondary); text-align: right; }
.field-row > .input, .field-inline { min-width: 0; flex: 1; }
.field-inline { display: flex; align-items: center; gap: 6px; }
.field-inline .input { min-width: 0; flex: 1; }
.connection-control { min-width: 0; flex: 1; display: flex; align-items: center; gap: 6px; }
.connection-state { min-width: 54px; font-size: var(--font-size-small); }
.connection-state--online { color: var(--success, #4fa86b); }
.connection-state--disabled { color: var(--warning, #d49a45); }
.connection-state--offline { color: var(--danger, #d96c6c); }
.connection-help { margin: 1px 0 6px 120px; color: var(--text-secondary); font-size: var(--font-size-small); }
.key-action-row { margin: 0 0 5px 120px; display: flex; align-items: center; gap: 8px; color: var(--text-secondary); font-size: var(--font-size-small); }
.advanced-options { margin: 5px 0 4px 120px; color: var(--text-secondary); font-size: var(--font-size-small); }
.advanced-options summary { cursor: pointer; user-select: none; }
.advanced-options p { margin: 2px 0 0; }
.advanced-options .inline-key-warning { color: var(--warning, #b26a00); }
.advanced-options :deep(.field-label) { width: 100px; }

.models-section { margin-top: 12px; padding-top: 10px; border-top: 1px solid var(--separator); }
.model-add-row { display: flex; gap: 6px; margin-top: 9px; }
.model-add-row .input { min-width: 0; flex: 1; }
.model-empty, .provider-empty { padding: 18px; color: var(--text-secondary); text-align: center; font-size: var(--font-size-small); }
.model-list { margin-top: 10px; }
.model-columns, .model-row { display: grid; grid-template-columns: minmax(150px, 1.2fr) minmax(130px, 1fr) minmax(120px, 0.8fr) minmax(110px, 0.8fr) 170px 30px; gap: 7px; align-items: center; }
.model-columns { padding: 0 2px 4px; color: var(--text-secondary); font-size: var(--font-size-small); }
.model-row { margin-top: 6px; }
.model-row .input { min-width: 0; }
.modality-options { display: flex; align-items: center; gap: 12px; white-space: nowrap; }
.modality-options label { display: inline-flex; align-items: center; gap: 4px; color: var(--text-secondary); font-size: var(--font-size-small); }
.icon-button { border: 0; background: transparent; color: var(--text-secondary); cursor: pointer; font-size: 19px; }

.provider-action-row { display: flex; align-items: center; gap: 8px; margin-top: 16px; padding-top: 12px; border-top: 1px solid var(--separator); }
.provider-action-row > span { color: var(--text-secondary); font-size: var(--font-size-small); }
.danger-button { color: var(--danger, #d96c6c); }
.source-note p { margin: 6px 0; line-height: 1.55; }
.preflight-entry { margin-top: 10px; display: flex; align-items: center; gap: 10px; color: var(--text-secondary); font-size: var(--font-size-small); }

@media (max-width: 900px) {
  .provider-sidebar { width: 230px; }
  .model-columns { display: none; }
  .model-row { grid-template-columns: 1fr 1fr 30px; padding-top: 7px; border-top: 1px solid var(--separator); }
  .model-row > :nth-child(3) { grid-column: 1 / 3; grid-row: 2; }
  .model-row > :nth-child(4) { grid-column: 1 / 3; grid-row: 3; }
  .model-row > :nth-child(5) { grid-column: 1 / 3; grid-row: 4; }
  .model-row > :nth-child(6) { grid-column: 3; grid-row: 1; }
}
</style>
