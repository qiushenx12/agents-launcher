<template>
  <div class="opencode-config-panel">
    <main class="config-content">
      <section class="card config-editor">
        <header class="editor-header">
          <div>
            <div class="card-title">OpenCode 配置</div>
            <p>{{ config.configPath || 'C:\\Users\\<用户>\\.config\\opencode\\opencode.jsonc' }}</p>
          </div>
          <button class="btn btn-secondary" type="button" :disabled="store.loading" @click="store.refreshConfig()">
            {{ store.loading ? '读取中…' : '重新读取' }}
          </button>
        </header>

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

        <div class="provider-title">
          <div>
            <strong>自定义 Provider</strong>
            <span>这里只显示并修改 opencode.jsonc 中带 npm 的自定义供应商</span>
          </div>
          <button class="btn btn-secondary" type="button" @click="store.addProvider()">
            添加 Provider
          </button>
        </div>

        <div v-if="store.loading && !store.loaded" class="empty-state">正在读取当前配置…</div>
        <div v-else-if="config.providers.length === 0" class="empty-state">
          当前文件中没有自定义 Provider。OpenCode 内置 Provider 不会在这里显示。
        </div>

        <article
          v-for="(provider, providerIndex) in config.providers"
          :key="provider.originalId || providerIndex"
          class="provider-card"
        >
          <header class="provider-header">
            <div>
              <strong>{{ provider.name || provider.id || `Provider ${providerIndex + 1}` }}</strong>
              <span>{{ provider.models.length }} 个模型</span>
            </div>
            <button class="btn btn-secondary danger-button" type="button" @click="store.removeProvider(provider)">
              删除
            </button>
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
            placeholder="保存到 OpenCode auth.json"
          />
          <div class="key-action-row">
            <button class="btn btn-secondary" type="button" @click="store.saveProviderKey(provider)">
              保存 Key
            </button>
            <span>单独保存到 auth.json；启用或禁用 Provider 都不会清空它</span>
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
              <button class="btn btn-primary" type="button" @click="store.saveConnection(provider)">
                {{ connectionButtonLabel(provider) }}
              </button>
              <button
                v-if="store.connectionState(provider) !== 'disabled'"
                class="btn btn-secondary"
                type="button"
                @click="store.disconnectConnection(provider)"
              >
                禁用
              </button>
            </div>
          </div>
          <p class="connection-help">
            “启用 / 重新连接”只从 disabled_providers 移除此 ID；“禁用”只加入该列表，均不改动 Key。
          </p>

          <details class="advanced-options">
            <summary>高级：直接在 JSONC 配置 API Key</summary>
            <SecretField
              v-model="provider.apiKey"
              label="配置 API Key"
              placeholder="可填写密钥或 {env:变量名}"
            />
            <p>这是 <code>options.apiKey</code>，可在不使用 OpenCode 连接凭据时作为替代。</p>
          </details>

          <section class="models-section">
            <header class="models-header">
              <div>
                <strong>模型</strong>
                <span>Text / Image 会同步到 modalities.input；其它模型参数保持原样</span>
              </div>
            </header>

            <div class="model-add-row">
              <input
                v-model="modelDrafts[store.providerKey(provider)]"
                class="input"
                type="text"
                :list="`models-${providerIndex}`"
                placeholder="输入或选择模型 ID"
                @keydown.enter.prevent="addModel(provider)"
              />
              <datalist :id="`models-${providerIndex}`">
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
        </article>

        <div class="action-row">
          <button class="btn btn-primary" type="button" :disabled="!store.isDirty || store.saving" @click="store.saveConfig()">
            {{ store.saving ? '保存并校验中…' : store.isDirty ? '保存配置' : '已同步' }}
          </button>
          <button class="btn btn-secondary" type="button" @click="openConfigDirectory">打开配置目录</button>
        </div>

        <ConfigStatusBanner
          v-if="store.statusMessage"
          :message="store.statusMessage"
          :tone="statusTone"
        />

        <div class="preflight-entry">
          <button class="btn btn-secondary" type="button" @click="workspaceStore.openPreflight()">启动前检测</button>
          <span>启动时会重新读取此文件，并在项目目录解析当前有效配置。</span>
        </div>
      </section>

      <section class="card source-note">
        <div class="card-title">同步说明</div>
        <p>界面和 <code>{{ config.configPath || '~/.config/opencode/opencode.jsonc' }}</code> 使用同一份数据，不再创建或应用启动器独立方案。</p>
        <p>“保存配置”只更新自定义 Provider、默认模型和 Small model；内置 Provider、shell、模型 limit 及其它未展示字段保持不变。启用/禁用只切换当前自定义 Provider 在 disabled_providers 中的状态。</p>
        <p>“保存 Key”写入 <code>{{ config.authPath || '~/.local/share/opencode/auth.json' }}</code>；只管理当前自定义 Provider 的 API Key 类型连接，启用/禁用不会删除它。</p>
      </section>
    </main>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { OpencodeGlobalModel, OpencodeGlobalProvider } from '@/types/config'
import { useOpencodeConfigStore } from '@/stores/opencodeConfig'
import { useConfigWorkspaceStore } from '@/stores/configWorkspace'
import ConfigStatusBanner from '@/components/config/ConfigStatusBanner.vue'
import ModelField from '@/components/config/ModelField.vue'
import SecretField from '@/components/config/SecretField.vue'

const store = useOpencodeConfigStore()
const workspaceStore = useConfigWorkspaceStore()
const config = computed(() => store.editingConfig)
const modelDrafts = reactive<Record<string, string>>({})
const statusTone = computed<'info' | 'success' | 'warning' | 'error'>(() => {
  if (/失败|错误|无效|不存在|已被其它程序修改/.test(store.statusMessage)) return 'error'
  if (/已保存|已同步|已读取|已获取|读取 \d+ 个/.test(store.statusMessage)) return 'success'
  return 'info'
})

function connectionStateLabel(provider: OpencodeGlobalProvider) {
  const state = store.connectionState(provider)
  if (state === 'connected') return '已连接'
  if (state === 'disabled') return '已禁用'
  return '未连接'
}

function connectionButtonLabel(provider: OpencodeGlobalProvider) {
  const state = store.connectionState(provider)
  if (state === 'connected') return '重新连接'
  if (state === 'disabled') return '重新启用'
  return '启用'
}

function addModel(provider: OpencodeGlobalProvider) {
  const key = store.providerKey(provider)
  if (store.addModel(provider, modelDrafts[key] || '')) modelDrafts[key] = ''
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

onMounted(() => {
  store.loadConfig(true).catch(() => {})
})
</script>

<style scoped>
.opencode-config-panel { height: 100%; min-height: 0; background: var(--bg); }
.config-content { height: 100%; padding: 12px; overflow-y: auto; }
.config-editor, .source-note { max-width: 900px; margin: 0 auto 12px; }
.editor-header, .provider-title, .provider-header, .models-header { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.editor-header { margin-bottom: 10px; }
.editor-header p, .provider-title span, .provider-header span, .models-header span, .source-note p { color: var(--text-secondary); font-size: var(--font-size-small); }
.editor-header p { margin: 4px 0 0; overflow-wrap: anywhere; }
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
.advanced-options :deep(.field-label) { width: 100px; }
.provider-title { margin: 16px 0 8px; padding-top: 12px; border-top: 1px solid var(--separator); }
.provider-title > div, .provider-header > div, .models-header > div { display: flex; flex-direction: column; gap: 2px; }
.provider-card { margin: 10px 0; padding: 13px; border: 1px solid var(--separator); border-radius: var(--radius-md); background: var(--bg-secondary); }
.provider-header { margin-bottom: 8px; }
.danger-button { color: var(--danger, #d96c6c); }
.empty-state, .model-empty { padding: 18px 8px; color: var(--text-secondary); text-align: center; font-size: var(--font-size-small); }
.models-section { margin-top: 12px; padding-top: 10px; border-top: 1px solid var(--separator); }
.model-add-row { display: flex; gap: 6px; margin-top: 9px; }
.model-add-row .input { min-width: 0; flex: 1; }
.model-list { margin-top: 10px; }
.model-columns, .model-row { display: grid; grid-template-columns: minmax(150px, 1.2fr) minmax(130px, 1fr) minmax(120px, 0.8fr) minmax(110px, 0.8fr) 170px 30px; gap: 7px; align-items: center; }
.model-columns { padding: 0 2px 4px; color: var(--text-secondary); font-size: var(--font-size-small); }
.model-row { margin-top: 6px; }
.model-row .input { min-width: 0; }
.modality-options { display: flex; align-items: center; gap: 12px; white-space: nowrap; }
.modality-options label { display: inline-flex; align-items: center; gap: 4px; color: var(--text-secondary); font-size: var(--font-size-small); }
.icon-button { border: 0; background: transparent; color: var(--text-secondary); cursor: pointer; font-size: 19px; }
.action-row { display: flex; gap: 8px; margin-top: 16px; padding-top: 12px; border-top: 1px solid var(--separator); }
.preflight-entry { margin-top: 12px; display: flex; align-items: center; gap: 10px; color: var(--text-secondary); font-size: var(--font-size-small); }
.source-note p { margin: 6px 0; line-height: 1.55; }
@media (max-width: 760px) {
  .model-columns { display: none; }
  .model-row { grid-template-columns: 1fr 1fr 30px; padding-top: 7px; border-top: 1px solid var(--separator); }
  .model-row > :nth-child(3) { grid-column: 1 / 3; grid-row: 2; }
  .model-row > :nth-child(4) { grid-column: 1 / 3; grid-row: 3; }
  .modality-options { grid-column: 1 / 3; grid-row: 4; }
  .model-row .icon-button { grid-column: 3; grid-row: 1 / 5; }
}
</style>
