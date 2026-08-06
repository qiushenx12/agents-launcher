<template>
  <div class="codex-config-panel">
    <Transition name="codex-left-pane">
    <div
      v-if="!props.sidebarCollapsed"
      class="codex-config-panel__sidebar-shell"
      :style="{ width: `${leftWidth + 9}px`, flexBasis: `${leftWidth + 9}px` }"
    >
    <aside class="codex-config-panel__sidebar" :style="{ width: `${leftWidth}px`, flexBasis: `${leftWidth}px` }">
      <button class="btn btn-primary sidebar__new-btn" type="button" @click="store.newProfile()">
        新建配置
      </button>

      <div class="codex-config-panel__sidebar-body">
        <div v-if="store.loading" class="codex-config-panel__empty">正在加载…</div>
        <div v-else-if="store.orderedProfiles.length === 0" class="codex-config-panel__empty">
          暂无 CodeX 配置
        </div>
        <div v-else class="codex-profile-list">
        <button
          v-for="(item, index) in store.orderedProfiles"
          :key="item.id"
          data-drag-item
          type="button"
          class="codex-profile-item"
          :class="{
            'codex-profile-item--selected': store.selectedProfileId === item.id,
            'codex-profile-item--applied': store.activeProfileId === item.id,
            'codex-profile-item--dragging': draggingIndex === index,
            'codex-profile-item--drag-over': draggingIndex !== null
              && draggingIndex !== index
              && overIndex === index,
          }"
          @click="onProfileClick(item.id)"
        >
          <span
            class="codex-profile-item__drag-handle"
            title="拖拽排序"
            @pointerdown="onPointerDown(index, $event)"
          />
          <span class="codex-profile-item__content">
            <strong>{{ item.name }}</strong>
            <small>{{ item.authMode === 'official' ? 'Codex 官方登录' : item.providerId }}</small>
          </span>
          <span
            v-if="profileStateLabel(item.id)"
            class="codex-profile-item__badge"
          >
            {{ profileStateLabel(item.id) }}
          </span>
          <span
            class="codex-profile-item__delete"
            role="button"
            tabindex="0"
            title="删除配置"
            @click.stop="store.deleteProfile(item.id)"
            @keydown.enter.stop="store.deleteProfile(item.id)"
          >×</span>
        </button>
        </div>
      </div>
      <footer class="codex-config-panel__sidebar-footer">
        <button class="settings-entry" type="button" @click="toggleSettings($event)">⚙ <span>设置</span></button>
      </footer>
    </aside>

    <div
      class="codex-config-panel__divider"
      :class="{ 'codex-config-panel__divider--dragging': isDragging }"
      @mousedown="onMouseDown"
    />
    </div>
    </Transition>

    <main class="codex-config-panel__content">
      <section class="card config-editor">
        <div class="card-title">配置编辑</div>

        <div class="field-row">
          <label class="field-label">配置名称</label>
          <input v-model="profile.name" class="input" type="text" placeholder="输入配置名称" />
        </div>

        <hr class="separator" style="margin: 10px 0;" />

        <div class="field-row">
          <label class="field-label">认证模式</label>
          <div class="radio-group">
            <label class="radio-label">
              <input v-model="profile.authMode" type="radio" value="official" />
              Codex 官方登录
            </label>
            <label class="radio-label">
              <input v-model="profile.authMode" type="radio" value="custom" />
              第三方 API
            </label>
          </div>
        </div>

        <template v-if="profile.authMode === 'official'">
          <div class="field-row">
            <label class="field-label">API 地址</label>
            <input
              v-model="profile.openaiBaseUrl"
              class="input"
              type="text"
              placeholder="留空使用 Codex 官方地址"
            />
          </div>
          <ConfigStatusBanner
            :message="store.authStatusLabel"
            :tone="store.authStatus.error ? 'error' : store.authStatus.hasCredentials ? 'success' : 'info'"
          />
        </template>

        <template v-else>
          <div class="field-row">
            <label class="field-label">API 地址</label>
            <div class="field-inline">
              <input
                v-model="profile.baseUrl"
                class="input"
                type="text"
                placeholder="https://proxy.example.com/v1"
              />
              <button
                class="btn btn-secondary"
                type="button"
                :disabled="store.modelsFetching"
                @click="store.fetchModels()"
              >
                {{ store.modelsFetching ? '获取中…' : '获取模型' }}
              </button>
            </div>
          </div>

          <SecretField
            :key="profile.id || 'new-codex-profile'"
            v-model="store.apiKeyInput"
            label="API Key"
            :has-stored-value="profile.hasStoredApiKey && !store.clearStoredApiKey"
            :stored-value-revealed="store.storedApiKeyRevealed"
            :loading-stored-value="store.apiKeyRevealing"
            :placeholder="profile.hasStoredApiKey ? '已安全保存；点击显示后按需读取' : '可留空并使用现有环境变量'"
            @reveal-stored-value="store.revealApiKey()"
          />

          <div class="field-row">
            <label class="field-label">Provider ID</label>
            <input v-model="profile.providerId" class="input" type="text" placeholder="例如 company_proxy" />
          </div>
          <div class="field-row">
            <label class="field-label">显示名称</label>
            <input v-model="profile.providerName" class="input" type="text" placeholder="例如 Company Proxy" />
          </div>
          <div class="field-row">
            <label class="field-label">Key 环境变量</label>
            <input v-model="profile.envKey" class="input" type="text" placeholder="OPENAI_API_KEY" />
          </div>
          <div class="field-row">
            <label class="field-label">Wire API</label>
            <select v-model="profile.wireApi" class="select">
              <option value="responses">responses</option>
            </select>
          </div>
          <label v-if="profile.hasStoredApiKey" class="clear-secret">
            <input v-model="store.clearStoredApiKey" type="checkbox" />
            删除已加密保存的 API Key，改用系统环境变量
          </label>
        </template>

        <ModelField
          v-if="!profile.modelCatalog"
          v-model="profile.model"
          label="默认模型"
          :models="store.availableModels"
          placeholder="留空则继承下层配置"
        />

        <div v-if="profile.authMode === 'custom'" class="model-catalog-editor">
          <div class="field-row model-catalog-header">
            <label class="field-label">模型目录</label>
            <button
              v-if="!profile.modelCatalog"
              class="btn btn-secondary"
              type="button"
              @click="store.enableModelCatalog()"
            >
              使用模板
            </button>
            <span v-else class="field-help-inline">未编辑的字段会沿用官方模板。</span>
          </div>

          <template v-if="profile.modelCatalog">
            <div class="model-add-row">
              <select
                v-model="selectedFetchedModel"
                class="select model-fetch-select"
                :disabled="fetchedModelOptions.length === 0"
                aria-label="从已获取模型中选择"
                @change="addFetchedModel"
              >
                <option value="">从已获取模型中添加</option>
                <option v-for="model in fetchedModelOptions" :key="model" :value="model">
                  {{ model }}
                </option>
              </select>
              <input
                v-model="modelDraftSlug"
                class="input"
                type="text"
                placeholder="输入模型 slug，例如 deepseek-v4-flash"
                @keydown.enter.prevent="addModel"
              />
              <button class="btn btn-secondary" type="button" @click="addModel">
                添加模型
              </button>
            </div>

            <div v-if="catalogModels.length === 0" class="model-empty">
              暂无模型
            </div>
            <div v-else class="model-list">
              <div class="model-columns" aria-hidden="true">
                <span>默认</span>
                <span>模型 slug</span>
                <span>显示名称</span>
                <span>上下文长度（token）</span>
                <span>最大上下文（token）</span>
                <span>有效比例</span>
                <span>Image</span>
                <span>原图细节</span>
                <span />
              </div>
              <div
                v-for="(model, index) in catalogModels"
                :key="index"
                class="model-row"
              >
                <label class="model-default-field">
                  <input
                    type="radio"
                    :name="`codex-default-model-${profile.id || 'draft'}`"
                    :checked="defaultModelSlug === model.slug"
                    :disabled="catalogModels.length === 1"
                    :aria-label="`设为默认模型 ${model.slug}`"
                    @change="store.setDefaultModel(model.slug)"
                  />
                </label>
                <input
                  :value="model.slug"
                  class="input"
                  type="text"
                  aria-label="模型 slug"
                  @input="updateModelSlug(model, $event)"
                />
                <input
                  v-model="model.displayName"
                  class="input"
                  type="text"
                  aria-label="模型显示名称"
                />
                <input
                  :value="model.contextWindow || ''"
                  class="input"
                  type="number"
                  min="1"
                  step="1"
                  aria-label="上下文长度"
                  @input="updateModelNumber(model, 'contextWindow', $event)"
                />
                <input
                  :value="model.maxContextWindow || ''"
                  class="input"
                  type="number"
                  min="1"
                  step="1"
                  aria-label="最大上下文长度"
                  @input="updateModelNumber(model, 'maxContextWindow', $event)"
                />
                <div class="field-inline model-percent-field">
                  <input
                    :value="model.effectiveContextWindowPercent || ''"
                    class="input"
                    type="number"
                    min="1"
                    max="100"
                    step="1"
                    aria-label="有效上下文比例"
                    @input="updateModelNumber(model, 'effectiveContextWindowPercent', $event)"
                  />
                  <span class="field-suffix">%</span>
                </div>
                <label class="model-image-field">
                  <input
                    type="checkbox"
                    :checked="hasImageModality(model)"
                    :aria-label="`启用模型 ${model.slug} 的 Image 输入`"
                    @change="updateModelImageModality(model, $event)"
                  />
                </label>
                <label class="model-image-field">
                  <input
                    type="checkbox"
                    :checked="model.supportsImageDetailOriginal"
                    :disabled="!hasImageModality(model)"
                    :aria-label="`启用模型 ${model.slug} 的原图细节支持`"
                    @change="updateModelImageDetailOriginal(model, $event)"
                  />
                </label>
                <button
                  class="icon-button"
                  type="button"
                  title="删除模型"
                  :disabled="catalogModels.length === 1"
                  @click="store.removeModel(model.slug)"
                >
                  ×
                </button>
              </div>
            </div>
            <p class="field-help">
              默认模型会写入 Codex profile 的 <code>model</code>；其它模型字段按模板生成，
              上下文长度和有效比例可按第三方模型实际能力调整。
            </p>
            <p class="field-help">
              Text is always enabled; Image writes <code>input_modalities</code>; 原图细节 writes
              <code>supports_image_detail_original</code> and requires Image.
            </p>
          </template>
        </div>


        <div class="field-row">
          <label class="field-label">推理强度</label>
          <div class="field-inline">
            <input
              v-model="profile.reasoningEffort"
              class="input"
              type="text"
              placeholder="minimal / low / medium / high / xhigh / ultra / max"
            />
            <select class="select effort-select" @change="onEffortSelect">
              <option value="" disabled selected>选择</option>
              <option v-for="effort in reasoningEfforts" :key="effort" :value="effort">
                {{ effort }}
              </option>
            </select>
          </div>
        </div>

        <p class="field-help">
          {{ profile.authMode === 'official'
            ? '复用 CLI、桌面端和 IDE 的 Codex 官方登录，不读取、覆盖或清除 auth.json。'
            : secretStorageDescription }}
        </p>

        <hr class="separator" style="margin: 12px 0 10px;" />

        <div class="scope-row">
          <span class="scope-label">应用范围</span>
          <label class="radio-label">
            <input
              v-model="store.syncToGlobal"
              type="checkbox"
              :disabled="globalApplied || (profile.authMode === 'custom' && !store.customGlobalSyncSupported)"
            />
            同时同步到全局配置
          </label>
          <span class="scope-hint">
            {{ globalApplied
              ? '当前方案已经同步到全局；选择另一方案同步时会替换它'
              : '不勾选时只改变启动器当前方案' }}
          </span>
        </div>
        <p v-if="profile.authMode === 'custom' && store.secretStorageKind === 'macos_plaintext'" class="scope-warning">
          macOS 会将第三方 Key 以明文保存到启动器的私有凭据文件，不使用 Keychain；新启动的 CodeX 会按 profile 读取对应 Key。
        </p>
        <p v-if="store.syncToGlobal" class="scope-warning">
          <template v-if="profile.authMode === 'official'">
            将更新 {{ store.globalConfigPath || '~/.codex/config.toml' }}；auth.json 保持只读。
          </template>
          <template v-else-if="store.customGlobalKeySyncSupported">
            将更新 {{ store.globalConfigPath || '~/.codex/config.toml' }}；并把
            <code>{{ profile.envKey || 'API Key 环境变量' }}</code> 写入 Windows 当前用户环境。该环境变量不是密文存储；外部终端和 CodeX 桌面端需重启后读取新环境。
          </template>
          <template v-else-if="store.secretStorageKind === 'macos_plaintext' && profile.hasStoredApiKey">
            将把第三方 Provider 和模型同步到 {{ store.globalConfigPath || '~/.codex/config.toml' }}，并配置 Codex 的命令式认证从启动器凭据文件读取 Key。明文不会写入 TOML 或 shell，不使用 Keychain。
          </template>
          <template v-else>
            将把第三方 Provider、模型和 <code>env_key</code> 同步到
            {{ store.globalConfigPath || '~/.codex/config.toml' }}。Key 不会写入 TOML 或 shell；启动器内会自动注入，外部 CodeX 需要自行设置
            <code>{{ profile.envKey || 'API Key 环境变量' }}</code>，或配置 Codex 命令式认证。
          </template>
        </p>

        <div class="action-row">
          <button
            class="btn btn-primary"
            type="button"
            :disabled="!store.isDirty || store.saving || store.apiKeyRevealing"
            @click="store.saveProfile()"
          >
            {{ store.saving ? '保存并校验中…' : store.isDirty ? '保存配置' : '已保存' }}
          </button>
          <button
            class="btn btn-primary"
            type="button"
            :disabled="applyDisabled"
            @click="store.applyProfile()"
          >
            {{ applyButtonLabel }}
          </button>
        </div>

        <ConfigStatusBanner
          v-if="store.globalConfigError"
          :message="store.globalConfigError"
          tone="error"
        />
        <ConfigStatusBanner
          v-if="store.statusMessage"
          :message="store.statusMessage"
          :tone="statusTone"
        />

        <div class="preflight-entry">
          <button class="btn btn-secondary" type="button" @click="workspaceStore.openPreflight()">
            启动前检测
          </button>
          <span>检查 CLI、配置来源、实际应用 profile 和脱敏后的启动上下文。</span>
        </div>
      </section>

      <section class="card codex-source-note">
        <div class="card-title">配置边界</div>
        <p>全局配置：{{ store.globalConfigPath || '~/.codex/config.toml' }}</p>
        <p>启动器索引：{{ store.profilesPath || defaultProfilesPath }}</p>
        <p>启动器通过独立的 <code>--profile {{ profile.managedProfileName || 'agents-launcher-…' }}</code> 启动；只有主动勾选“同时同步到全局配置”才修改全局文件。</p>
        <p>CodeX 优先级：CLI 参数 → 项目 <code>.codex/config.toml</code> → 当前启动器 profile → 全局配置。</p>
      </section>
    </main>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useCodexConfigStore } from '@/stores/codexConfig'
import { useConfigWorkspaceStore } from '@/stores/configWorkspace'
import ConfigStatusBanner from '@/components/config/ConfigStatusBanner.vue'
import ModelField from '@/components/config/ModelField.vue'
import SecretField from '@/components/config/SecretField.vue'
import { useDragReorder } from '@/composables/useDragReorder'
import { useSharedLeftSidebarWidth } from '@/composables/useSharedLeftSidebarWidth'
import { useSettingsPopover } from '@/composables/useSettingsPopover'
import type { CodexModelDefinition } from '@/types/config'

const { toggleSettings } = useSettingsPopover()
import { usePlatform } from '@/composables/usePlatform'

const store = useCodexConfigStore()
const workspaceStore = useConfigWorkspaceStore()
const props = defineProps<{
  sidebarCollapsed?: boolean
}>()
const emit = defineEmits<{
  (event: 'left-width-change', width: number): void
}>()
const { isMacOS } = usePlatform()
const { draggingIndex, overIndex, justDragged, onPointerDown } = useDragReorder(
  () => store.orderedProfiles.map(item => item.id),
  (newOrder: string[]) => store.reorderProfiles(newOrder),
)
const profile = computed(() => store.editingProfile)
const modelDraftSlug = ref('')
const selectedFetchedModel = ref('')
const catalogModels = computed(() => profile.value.modelCatalog?.models ?? [])
const defaultModelSlug = computed(() => {
  const models = catalogModels.value
  return models.some(model => model.slug === profile.value.model)
    ? profile.value.model
    : models[0]?.slug ?? ''
})
const fetchedModelOptions = computed(() => {
  const existingSlugs = new Set(
    catalogModels.value.map(model => model.slug.trim()).filter(Boolean),
  )
  return store.availableModels.filter(model => !existingSlugs.has(model))
})
const defaultProfilesPath = computed(() => isMacOS.value
  ? '~/Library/Application Support/ClaudeEnvManager/codex/profiles.json'
  : '%APPDATA%\\ClaudeEnvManager\\codex\\profiles.json')
const secretStorageDescription = computed(() => store.secretStorageKind === 'macos_plaintext'
  ? 'Key 以明文保存在启动器私有 credentials.json 中；Codex 通过命令式认证按 profile 读取，不使用 Keychain。'
  : 'Key 使用 Windows DPAPI 加密保存，并且只注入启动器创建的 CodeX 子进程。')
watch(
  () => [profile.value.authMode, store.customGlobalSyncSupported] as const,
  ([authMode, supported]) => {
    if (authMode === 'custom' && !supported) store.syncToGlobal = false
  },
)
const reasoningEfforts = ['minimal', 'low', 'medium', 'high', 'xhigh', 'ultra', 'max']
const appApplied = computed(() => Boolean(
  profile.value.id && store.activeProfileId === profile.value.id,
))
const globalApplied = computed(() => Boolean(
  profile.value.id && store.globalProfileId === profile.value.id,
))
const applyDisabled = computed(() => (
  !profile.value.id
  || store.isDirty
  || store.applying
  || store.apiKeyRevealing
  || (appApplied.value && (!store.syncToGlobal
    || (globalApplied.value && store.globalProfileInSync)))
))
const applyButtonLabel = computed(() => {
  if (store.applying) return '应用并校验中…'
  if (!profile.value.id || store.isDirty) return '请先保存'
  if (store.syncToGlobal) {
    if (appApplied.value && globalApplied.value) {
      return store.globalProfileInSync ? '全局应用中' : '重新同步全局'
    }
    return '应用并同步全局'
  }
  if (appApplied.value) return '应用中'
  return '应用此配置'
})
const statusTone = computed<'info' | 'success' | 'warning' | 'error'>(() => {
  if (/失败|错误|无效|不一致|不存在/.test(store.statusMessage)) return 'error'
  if (/已保存|已删除|已切换|已应用|应用中|已获取/.test(store.statusMessage)) return 'success'
  return 'info'
})

function profileStateLabel(profileId: string) {
  const active = store.activeProfileId === profileId
  const global = store.globalProfileId === profileId
  const globalStale = global && !store.globalProfileInSync
  if (active && globalStale) return '应用中 · 全局待更新'
  if (active && global) return '应用中 · 全局'
  if (active) return '应用中'
  if (globalStale) return '全局待更新'
  if (global) return '全局'
  return ''
}

function onProfileClick(profileId: string) {
  if (justDragged.value) return
  store.selectProfile(profileId)
}

function addModel() {
  if (store.addModel(modelDraftSlug.value)) modelDraftSlug.value = ''
}

function addFetchedModel() {
  const slug = selectedFetchedModel.value
  if (!slug) return
  store.addModel(slug)
  selectedFetchedModel.value = ''
}

function updateModelSlug(model: { slug: string }, event: Event) {
  const previousSlug = model.slug
  const nextSlug = (event.target as HTMLInputElement).value
  model.slug = nextSlug
  if (profile.value.model === previousSlug || !profile.value.model) {
    profile.value.model = nextSlug
  }
}

function updateModelNumber(
  model: {
    contextWindow: number
    maxContextWindow: number
    effectiveContextWindowPercent: number
  },
  field: 'contextWindow' | 'maxContextWindow' | 'effectiveContextWindowPercent',
  event: Event,
) {
  const raw = (event.target as HTMLInputElement).value.trim()
  const parsed = Number(raw)
  model[field] = raw && Number.isSafeInteger(parsed) ? parsed : 0
}

function hasImageModality(model: CodexModelDefinition) {
  return Array.isArray(model.inputModalities) && model.inputModalities.includes('image')
}

function updateModelImageModality(model: CodexModelDefinition, event: Event) {
  const checked = (event.target as HTMLInputElement).checked
  const otherModalities = (Array.isArray(model.inputModalities) ? model.inputModalities : [])
    .filter(modality => modality !== 'text' && modality !== 'image')
  model.inputModalities = ['text', ...(checked ? ['image'] : []), ...otherModalities]
  if (!checked) model.supportsImageDetailOriginal = false
}

function updateModelImageDetailOriginal(model: CodexModelDefinition, event: Event) {
  if (!hasImageModality(model)) {
    model.supportsImageDetailOriginal = false
    return
  }
  model.supportsImageDetailOriginal = (event.target as HTMLInputElement).checked
}

function onEffortSelect(event: Event) {
  const value = (event.target as HTMLSelectElement).value
  if (value) profile.value.reasoningEffort = value
  ;(event.target as HTMLSelectElement).value = ''
}

const { leftWidth, isDragging, onMouseDown, loadWidth } = useSharedLeftSidebarWidth()

onMounted(() => {
  loadWidth().catch(() => {})
  store.loadProfiles().catch(() => {})
})

watch(leftWidth, (width) => {
  emit('left-width-change', width)
}, { immediate: true })
</script>

<style scoped>
.codex-config-panel {
  height: 100%;
  min-height: 0;
  display: flex;
  background: var(--app-bg-gradient);
}

.codex-config-panel__sidebar-shell {
  flex: 0 0 auto;
  min-width: 0;
  min-height: 0;
  display: flex;
  overflow: hidden;
}

.codex-left-pane-enter-active,
.codex-left-pane-leave-active {
  transition: width 0.22s ease, flex-basis 0.22s ease, opacity 0.16s ease;
}

.codex-left-pane-enter-from,
.codex-left-pane-leave-to {
  width: 0 !important;
  flex-basis: 0 !important;
  opacity: 0;
}

.codex-config-panel__sidebar {
  width: 280px;
  flex: 0 0 auto;
  min-width: 0;
  padding: 12px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background: var(--bg);
}

.codex-config-panel__divider {
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

.codex-config-panel__divider::after {
  content: '';
  width: 1px;
  height: 100%;
  background-color: var(--separator);
  transition: background-color 0.2s ease, width 0.2s ease, box-shadow 0.2s ease;
}

.codex-config-panel__divider:hover::after,
.codex-config-panel__divider--dragging::after {
  width: 2px;
  background-color: var(--primary);
}

[data-theme="dark"] .codex-config-panel__divider:hover::after,
[data-theme="dark"] .codex-config-panel__divider--dragging::after {
  box-shadow: 0 0 6px 1px rgba(10, 132, 255, 0.5);
}

.codex-config-panel__sidebar-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.codex-config-panel__sidebar-footer {
  flex-shrink: 0;
  padding-top: 8px;
  border-top: 1px solid var(--separator);
}

.sidebar__new-btn {
  width: 100%;
  margin-bottom: 8px;
}

.codex-config-panel__empty {
  padding: 18px 8px;
  color: var(--text-secondary);
  text-align: center;
  font-size: var(--font-size-small);
}

.codex-profile-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.codex-profile-item {
  width: 100%;
  padding: 8px 10px;
  display: flex;
  align-items: center;
  gap: 8px;
  border: 0;
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  background: transparent;
  cursor: pointer;
  text-align: left;
  transition: background-color 0.12s ease, transform 0.18s ease;
  user-select: none;
  position: relative;
  will-change: transform;
}

.codex-profile-item:hover { background: var(--tab-bg); }
.codex-profile-item--selected { color: #fff; background: var(--primary); }
.codex-profile-item--selected:hover { background: var(--primary-hover); }
.codex-profile-item--applied { box-shadow: inset 3px 0 var(--success, #22c55e); }

.codex-profile-item--dragging {
  opacity: 0.3;
  background: var(--tab-bg);
}

.codex-profile-item__drag-handle {
  width: 14px;
  height: 14px;
  flex-shrink: 0;
  position: relative;
  cursor: grab;
  opacity: 0;
  transition: opacity 0.12s ease;
  touch-action: none;
}

.codex-profile-item__drag-handle::before,
.codex-profile-item__drag-handle::after {
  content: '';
  position: absolute;
  left: 1px;
  width: 2.5px;
  height: 2.5px;
  border-radius: 50%;
  background-color: var(--text-secondary);
  box-shadow: 5px 0 0 var(--text-secondary), 10px 0 0 var(--text-secondary);
}

.codex-profile-item__drag-handle::before { top: 1.5px; }
.codex-profile-item__drag-handle::after { bottom: 1.5px; }
.codex-profile-item__drag-handle:active { cursor: grabbing; }
.codex-profile-item:hover .codex-profile-item__drag-handle { opacity: 1; }

.codex-profile-item--selected .codex-profile-item__drag-handle::before,
.codex-profile-item--selected .codex-profile-item__drag-handle::after {
  background-color: rgba(255, 255, 255, 0.72);
  box-shadow: 5px 0 0 rgba(255, 255, 255, 0.72), 10px 0 0 rgba(255, 255, 255, 0.72);
}

.codex-profile-item__content {
  min-width: 0;
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.codex-profile-item__content strong,
.codex-profile-item__content small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.codex-profile-item__content small { opacity: 0.72; font-size: var(--font-size-small); }
.codex-profile-item__badge {
  flex: 0 0 auto;
  padding: 2px 6px;
  border-radius: 4px;
  color: #fff;
  background: var(--success, #22c55e);
  font-size: 11px;
}
.codex-profile-item__delete { padding: 2px 4px; border-radius: 4px; font-size: 18px; }
.codex-profile-item__delete:hover { background: rgba(255, 255, 255, 0.2); }

.codex-config-panel__content {
  min-width: 0;
  flex: 1;
  padding: 12px 16px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.config-editor { flex-shrink: 0; }

.field-row {
  min-width: 0;
  padding: 5px 0;
  display: flex;
  align-items: center;
  gap: 10px;
}

.field-row > .input,
.field-row > .select { min-width: 0; flex: 1; }

.field-label,
.scope-label {
  width: 110px;
  flex-shrink: 0;
  color: var(--text-secondary);
  text-align: right;
  font-size: var(--font-size-base);
}

.field-inline {
  min-width: 0;
  flex: 1;
  display: flex;
  align-items: center;
  gap: 6px;
}

.field-inline .input { min-width: 0; flex: 1; }
.effort-select { width: 100px; flex-shrink: 0; }
.field-help-inline { color: var(--text-secondary); font-size: var(--font-size-small); }
.field-suffix { flex: 0 0 auto; color: var(--text-secondary); }
.model-catalog-editor { margin-top: 4px; }
.model-catalog-header { align-items: center; }
.model-add-row { margin: 2px 0 10px 120px; display: flex; gap: 6px; }
.model-fetch-select { flex: 0 1 260px; min-width: 220px; }
.model-add-row .input { min-width: 0; flex: 1; }
.model-empty { margin-left: 120px; padding: 14px; color: var(--text-secondary); text-align: center; font-size: var(--font-size-small); }
.model-list { margin-left: 120px; overflow-x: auto; }
.model-columns,
.model-row {
  min-width: 1110px;
  display: grid;
  grid-template-columns: 46px minmax(150px, 1.1fr) minmax(150px, 1fr) minmax(130px, 0.9fr) minmax(130px, 0.9fr) minmax(100px, 0.7fr) 54px 74px 30px;
  gap: 7px;
  align-items: center;
}
.model-columns { padding: 0 2px 4px; color: var(--text-secondary); font-size: var(--font-size-small); }
.model-row { margin-top: 6px; }
.model-row .input { min-width: 0; }
.model-default-field { display: flex; align-items: center; justify-content: center; }
.model-image-field { display: flex; align-items: center; justify-content: center; }
.model-percent-field { min-width: 0; }
.model-percent-field .input { min-width: 0; }
.model-row .icon-button { justify-self: center; }

.radio-group,
.scope-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 16px;
}

.scope-row { padding: 4px 0 8px; }
.radio-label,
.clear-secret {
  display: flex;
  align-items: center;
  gap: 5px;
  color: var(--text-primary);
  font-size: var(--font-size-base);
  cursor: pointer;
  user-select: none;
}

.clear-secret,
.field-help,
.scope-warning { margin: 5px 0 8px 120px; }

.field-help,
.scope-hint,
.scope-warning,
.codex-source-note p {
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  line-height: 1.55;
}

.scope-warning { color: var(--warning, #b26a00); overflow-wrap: anywhere; }
.action-row {
  padding: 4px 0;
  display: flex;
  align-items: center;
  gap: 8px;
}

.preflight-entry {
  margin-top: 10px;
  padding-top: 10px;
  display: flex;
  align-items: center;
  gap: 10px;
  border-top: 1px solid var(--separator);
}

.preflight-entry span {
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  line-height: 1.45;
}

.codex-source-note p + p { margin-top: 5px; }

@media (max-width: 820px) {
  .codex-config-panel__sidebar { width: 220px; }
  .model-add-row, .model-empty, .model-list { margin-left: 0; }
}
</style>
