import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import { confirm } from '@tauri-apps/plugin-dialog'
import type {
  OpencodeGlobalConfigPayload,
  OpencodeGlobalProvider,
  OpencodeConnectionStatusPayload,
  OpencodeLaunchContext,
} from '@/types/config'

function emptyConfig(): OpencodeGlobalConfigPayload {
  return {
    configPath: '',
    revision: '',
    authPath: '',
    authRevision: '',
    connectedProviderIds: [],
    disabledProviderIds: [],
    connectionKeys: {},
    model: '',
    smallModel: '',
    providers: [],
  }
}

function cloneConfig(config: OpencodeGlobalConfigPayload): OpencodeGlobalConfigPayload {
  return JSON.parse(JSON.stringify(config)) as OpencodeGlobalConfigPayload
}

function providerKey(provider: OpencodeGlobalProvider) {
  return provider.originalId || provider.id
}

function editableSnapshot(config: OpencodeGlobalConfigPayload) {
  return JSON.stringify({
    model: config.model,
    smallModel: config.smallModel,
    providers: config.providers,
  })
}

export const useOpencodeConfigStore = defineStore('opencodeConfig', () => {
  const editingConfig = ref<OpencodeGlobalConfigPayload>(emptyConfig())
  const baseline = ref(editableSnapshot(editingConfig.value))
  const loaded = ref(false)
  const loading = ref(false)
  const saving = ref(false)
  const modelsFetchingId = ref<string | null>(null)
  const availableModels = ref<Record<string, string[]>>({})
  const connectionKeyInputs = ref<Record<string, string>>({})
  const statusMessage = ref('')
  const loadError = ref('')
  const resolvedPreview = ref<unknown>(null)
  const previewError = ref('')
  const lastLaunchContext = ref<OpencodeLaunchContext | null>(null)

  const isDirty = computed(() => editableSnapshot(editingConfig.value) !== baseline.value)
  const configuredModelIds = computed(() => editingConfig.value.providers.flatMap(provider =>
    provider.models.map(model => `${provider.id}/${model.id}`),
  ))
  const globalConfigPath = computed(() => editingConfig.value.configPath)

  function applyPayload(payload: OpencodeGlobalConfigPayload) {
    editingConfig.value = cloneConfig(payload)
    baseline.value = editableSnapshot(editingConfig.value)
    availableModels.value = {}
    connectionKeyInputs.value = { ...payload.connectionKeys }
    loaded.value = true
    loadError.value = ''
  }

  async function loadConfig(force = false) {
    if (loading.value || (loaded.value && !force)) return
    loading.value = true
    try {
      const payload = await invoke<OpencodeGlobalConfigPayload>('load_opencode_global_config')
      applyPayload(payload)
      statusMessage.value = `已从 ${payload.configPath} 读取 ${payload.providers.length} 个自定义 Provider`
    } catch (error) {
      loaded.value = false
      loadError.value = `读取 opencode.jsonc 失败：${error}`
      statusMessage.value = loadError.value
    } finally {
      loading.value = false
    }
  }

  async function ensureLoaded() {
    await loadConfig()
    if (!loaded.value) throw new Error(loadError.value || 'OpenCode 配置尚未加载')
  }

  function discardChanges() {
    const snapshot = JSON.parse(baseline.value) as Pick<
      OpencodeGlobalConfigPayload,
      'model' | 'smallModel' | 'providers'
    >
    editingConfig.value.model = snapshot.model
    editingConfig.value.smallModel = snapshot.smallModel
    editingConfig.value.providers = snapshot.providers
    availableModels.value = {}
  }

  async function confirmDiscardChanges(action: string) {
    if (!isDirty.value) return true
    const accepted = await confirm(
      `opencode.jsonc 有未保存的修改。${action}将放弃这些修改，是否继续？`,
      { title: '未保存的 OpenCode 配置', kind: 'warning' },
    )
    if (accepted) discardChanges()
    return accepted
  }

  async function refreshConfig() {
    if (!(await confirmDiscardChanges('重新读取文件'))) return false
    await loadConfig(true)
    return loaded.value
  }

  function addProvider() {
    const index = editingConfig.value.providers.length + 1
    editingConfig.value.providers.push({
      originalId: '',
      id: `provider-${index}`,
      name: '',
      npm: '@ai-sdk/openai-compatible',
      baseUrl: '',
      apiKey: '',
      models: [],
    })
  }

  async function removeProvider(provider: OpencodeGlobalProvider) {
    const accepted = await confirm(
      `确定从 opencode.jsonc 删除自定义 Provider“${provider.name || provider.id}”吗？`,
      { title: '删除 OpenCode Provider', kind: 'warning' },
    )
    if (!accepted) return false
    editingConfig.value.providers = editingConfig.value.providers.filter(item => item !== provider)
    return true
  }

  function addModel(provider: OpencodeGlobalProvider, modelId: string) {
    const id = modelId.trim()
    if (!id || provider.models.some(model => model.id === id)) return false
    provider.models.push({
      originalId: '',
      id,
      name: id,
      contextLimit: null,
      outputLimit: null,
      inputText: true,
      inputImage: false,
    })
    return true
  }

  function removeModel(provider: OpencodeGlobalProvider, modelId: string) {
    provider.models = provider.models.filter(model => model.id !== modelId)
  }

  async function fetchModels(provider: OpencodeGlobalProvider) {
    if (modelsFetchingId.value) return false
    const key = providerKey(provider)
    modelsFetchingId.value = key
    try {
      const models = await invoke<string[]>('fetch_opencode_global_models', {
        request: {
          provider: {
            ...provider,
            apiKey: connectionKeyInputs.value[key]?.trim() || provider.apiKey,
          },
        },
      })
      availableModels.value[key] = models
      statusMessage.value = `Provider '${provider.id}' 已获取 ${models.length} 个模型`
      return true
    } catch (error) {
      statusMessage.value = `获取 Provider '${provider.id}' 模型失败：${error}`
      return false
    } finally {
      modelsFetchingId.value = null
    }
  }

  function applyConnectionStatus(payload: OpencodeConnectionStatusPayload, refreshKeys = false) {
    editingConfig.value.authPath = payload.authPath
    editingConfig.value.authRevision = payload.authRevision
    editingConfig.value.connectedProviderIds = [...payload.connectedProviderIds]
    editingConfig.value.revision = payload.configRevision
    editingConfig.value.disabledProviderIds = [...payload.disabledProviderIds]
    editingConfig.value.connectionKeys = { ...payload.connectionKeys }
    if (refreshKeys) connectionKeyInputs.value = { ...payload.connectionKeys }
  }

  function hasCredential(provider: OpencodeGlobalProvider) {
    return editingConfig.value.connectedProviderIds.includes(provider.id)
  }

  function hasUsableKey(provider: OpencodeGlobalProvider) {
    return hasCredential(provider) || provider.apiKey.trim().length > 0
  }

  function isConnected(provider: OpencodeGlobalProvider) {
    return hasUsableKey(provider)
      && !editingConfig.value.disabledProviderIds.includes(provider.id)
  }

  function connectionState(provider: OpencodeGlobalProvider) {
    if (editingConfig.value.disabledProviderIds.includes(provider.id)) return 'disabled'
    return hasUsableKey(provider) ? 'connected' : 'disconnected'
  }

  async function saveProviderKey(provider: OpencodeGlobalProvider) {
    if (!provider.originalId || provider.id !== provider.originalId) {
      statusMessage.value = '新增或修改 Provider ID 后，请先保存 opencode.jsonc 再保存 Key'
      return false
    }
    const key = connectionKeyInputs.value[providerKey(provider)]?.trim() || ''
    if (!key) {
      statusMessage.value = `请输入 Provider '${provider.id}' 的 Key`
      return false
    }
    try {
      const payload = await invoke<OpencodeConnectionStatusPayload>('save_opencode_provider_key', {
        request: {
          providerId: provider.id,
          apiKey: key,
          authRevision: editingConfig.value.authRevision,
        },
      })
      applyConnectionStatus(payload, true)
      statusMessage.value = `Provider '${provider.id}' 的 Key 已保存到 OpenCode auth.json`
      return true
    } catch (error) {
      statusMessage.value = `保存 Provider '${provider.id}' Key 失败：${error}`
      return false
    }
  }

  async function saveConnection(provider: OpencodeGlobalProvider) {
    if (!provider.originalId || provider.id !== provider.originalId) {
      statusMessage.value = '新增或修改 Provider ID 后，请先保存 opencode.jsonc 再建立连接'
      return false
    }
    try {
      const payload = await invoke<OpencodeConnectionStatusPayload>('save_opencode_provider_connection', {
        request: {
          providerId: provider.id,
          authRevision: editingConfig.value.authRevision,
          configRevision: editingConfig.value.revision,
        },
      })
      applyConnectionStatus(payload)
      statusMessage.value = `Provider '${provider.id}' 已启用；Key 保持不变`
      return true
    } catch (error) {
      statusMessage.value = `连接 Provider '${provider.id}' 失败：${error}`
      return false
    }
  }

  async function disconnectConnection(provider: OpencodeGlobalProvider) {
    const accepted = await confirm(
      `确定禁用 Provider“${provider.name || provider.id}”吗？配置和 Key 都会保留。`,
      { title: '禁用 OpenCode Provider', kind: 'warning' },
    )
    if (!accepted) return false
    try {
      const payload = await invoke<OpencodeConnectionStatusPayload>('disconnect_opencode_provider', {
        request: {
          providerId: provider.id,
          authRevision: editingConfig.value.authRevision,
          configRevision: editingConfig.value.revision,
        },
      })
      applyConnectionStatus(payload)
      statusMessage.value = `Provider '${provider.id}' 已禁用；配置和 Key 均已保留`
      return true
    } catch (error) {
      statusMessage.value = `禁用 Provider '${provider.id}' 失败：${error}`
      return false
    }
  }

  async function saveConfig() {
    if (saving.value || !isDirty.value) return false
    saving.value = true
    try {
      const payload = await invoke<OpencodeGlobalConfigPayload>('save_opencode_global_config', {
        request: { config: cloneConfig(editingConfig.value) },
      })
      applyPayload(payload)
      statusMessage.value = `已保存并重新读取 ${payload.configPath}`
      return true
    } catch (error) {
      statusMessage.value = `保存 opencode.jsonc 失败，界面内容已保留：${error}`
      return false
    } finally {
      saving.value = false
    }
  }

  async function resolveLaunchContext(projectPath: string) {
    await loadConfig(true)
    if (!loaded.value) throw new Error(loadError.value || '无法读取 opencode.jsonc')
    const context = await invoke<OpencodeLaunchContext>('resolve_opencode_current_config', {
      projectPath,
    })
    lastLaunchContext.value = context
    statusMessage.value = `已获取当前 OpenCode 配置：${context.providerIds.length} 个 Provider，模型 ${context.model || '由 OpenCode 运行时决定'}`
    return context
  }

  async function previewCurrent(projectPath?: string) {
    try {
      resolvedPreview.value = await invoke<unknown>('preview_opencode_current_config', {
        projectPath: projectPath || null,
      })
      previewError.value = ''
    } catch (error) {
      resolvedPreview.value = null
      previewError.value = `${error}`
    }
  }

  return {
    editingConfig,
    loaded,
    loading,
    saving,
    modelsFetchingId,
    availableModels,
    connectionKeyInputs,
    statusMessage,
    loadError,
    resolvedPreview,
    previewError,
    lastLaunchContext,
    isDirty,
    configuredModelIds,
    globalConfigPath,
    loadConfig,
    ensureLoaded,
    refreshConfig,
    saveConfig,
    discardChanges,
    confirmDiscardChanges,
    addProvider,
    removeProvider,
    addModel,
    removeModel,
    fetchModels,
    hasCredential,
    isConnected,
    connectionState,
    saveProviderKey,
    saveConnection,
    disconnectConnection,
    resolveLaunchContext,
    previewCurrent,
    providerKey,
  }
})
