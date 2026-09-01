import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import { confirm } from '@tauri-apps/plugin-dialog'
import type {
  OpencodeGlobalConfigPayload,
  OpencodeGlobalProvider,
  OpencodeConnectionStatusPayload,
  OpencodeLaunchContext,
  OpencodePermissionStatus,
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

function cloneProvider(provider: OpencodeGlobalProvider): OpencodeGlobalProvider {
  return JSON.parse(JSON.stringify(provider)) as OpencodeGlobalProvider
}

const providerClientKeys = new WeakMap<OpencodeGlobalProvider, string>()

function providerKey(provider: OpencodeGlobalProvider) {
  if (provider.originalId) return provider.originalId
  const existing = providerClientKeys.get(provider)
  if (existing) return existing
  const created = `draft-${crypto.randomUUID()}`
  providerClientKeys.set(provider, created)
  return created
}

function editableSnapshot(config: OpencodeGlobalConfigPayload) {
  return JSON.stringify({
    model: config.model,
    smallModel: config.smallModel,
    providers: config.providers,
  })
}

function globalOptionsSnapshot(
  config: Pick<OpencodeGlobalConfigPayload, 'model' | 'smallModel'>,
) {
  return JSON.stringify({ model: config.model, smallModel: config.smallModel })
}

function providerSnapshot(provider: OpencodeGlobalProvider) {
  return JSON.stringify(provider)
}

function emptyPermissionStatus(): OpencodePermissionStatus {
  return {
    supported: false,
    requiresRepair: false,
    directories: [],
    blockedDirectories: [],
  }
}

export const useOpencodeConfigStore = defineStore('opencodeConfig', () => {
  const editingConfig = ref<OpencodeGlobalConfigPayload>(emptyConfig())
  const baseline = ref(editableSnapshot(editingConfig.value))
  const loaded = ref(false)
  const loading = ref(false)
  let loadPromise: Promise<void> | null = null
  const saving = ref(false)
  const modelsFetchingId = ref<string | null>(null)
  const availableModels = ref<Record<string, string[]>>({})
  const connectionKeyInputs = ref<Record<string, string>>({})
  const statusMessage = ref('')
  const loadError = ref('')
  const resolvedPreview = ref<unknown>(null)
  const previewError = ref('')
  const lastLaunchContext = ref<OpencodeLaunchContext | null>(null)
  const permissionStatus = ref<OpencodePermissionStatus>(emptyPermissionStatus())
  const permissionChecking = ref(false)
  const permissionRepairing = ref(false)
  const permissionError = ref('')
  const selectedProviderIndex = ref(-1)
  const providerOrder = ref<string[]>([])

  const isDirty = computed(() => editableSnapshot(editingConfig.value) !== baseline.value)
  const selectedProvider = computed(() =>
    editingConfig.value.providers[selectedProviderIndex.value] ?? null,
  )
  const orderedProviders = computed(() => {
    const providersByKey = new Map(
      editingConfig.value.providers.map(provider => [providerKey(provider), provider]),
    )
    const ordered = providerOrder.value
      .map(key => providersByKey.get(key))
      .filter(Boolean) as OpencodeGlobalProvider[]
    const included = new Set(ordered)
    return [...ordered, ...editingConfig.value.providers.filter(provider => !included.has(provider))]
  })
  const isGlobalOptionsDirty = computed(() => {
    const snapshot = JSON.parse(baseline.value) as Pick<
      OpencodeGlobalConfigPayload,
      'model' | 'smallModel'
    >
    return globalOptionsSnapshot(editingConfig.value) !== globalOptionsSnapshot(snapshot)
  })
  const isSelectedProviderDirty = computed(() => {
    const provider = selectedProvider.value
    if (!provider) return false
    if (!provider.originalId) return true
    const snapshot = JSON.parse(baseline.value) as Pick<OpencodeGlobalConfigPayload, 'providers'>
    const stored = snapshot.providers.find(item => item.id === provider.originalId)
    return !stored || providerSnapshot(provider) !== providerSnapshot(stored)
  })
  const configuredModelIds = computed(() => editingConfig.value.providers.flatMap(provider =>
    provider.models.map(model => `${provider.id}/${model.id}`),
  ))
  const globalConfigPath = computed(() => editingConfig.value.configPath)

  function reconcileProviderOrder(requestedOrder = providerOrder.value) {
    const validKeys = editingConfig.value.providers.map(provider => providerKey(provider))
    const validKeySet = new Set(validKeys)
    const seen = new Set<string>()
    const retainedOrder = requestedOrder.filter((key) => {
      if (!validKeySet.has(key) || seen.has(key)) return false
      seen.add(key)
      return true
    })
    const appendedKeys = validKeys.filter((key) => {
      if (seen.has(key)) return false
      seen.add(key)
      return true
    })
    return [
      ...retainedOrder,
      ...appendedKeys,
    ]
  }

  async function persistProviderOrder(nextOrder: string[]) {
    await invoke('save_config_order', { key: 'opencode', order: nextOrder })
    const persistedOrder = await invoke<string[]>('load_config_order', { key: 'opencode' })
    if (JSON.stringify(persistedOrder) !== JSON.stringify(nextOrder)) {
      throw new Error('排序写入后回读不一致')
    }
  }

  async function checkPermissions() {
    if (permissionChecking.value) return permissionStatus.value
    permissionChecking.value = true
    try {
      permissionStatus.value = await invoke<OpencodePermissionStatus>('check_opencode_permissions')
      permissionError.value = ''
    } catch (error) {
      permissionError.value = `无法检查 OpenCode 目录权限：${error}`
    } finally {
      permissionChecking.value = false
    }
    return permissionStatus.value
  }

  async function repairPermissions() {
    if (permissionRepairing.value) return false
    permissionRepairing.value = true
    const preserveDraft = isDirty.value
    try {
      permissionStatus.value = await invoke<OpencodePermissionStatus>('repair_opencode_permissions')
      permissionError.value = ''
      if (!preserveDraft) await loadConfig(true)
      statusMessage.value = preserveDraft
        ? 'OpenCode 目录权限已修复，界面修改保持不变；请重新执行写入或保存'
        : 'OpenCode 目录权限已修复，配置已重新读取'
      return true
    } catch (error) {
      await checkPermissions()
      statusMessage.value = `修复 OpenCode 目录权限失败：${error}`
      return false
    } finally {
      permissionRepairing.value = false
    }
  }

  async function permissionAwareError(error: unknown) {
    const detail = `${error}`
    if (/Permission denied|os error 13|权限不足|拒绝访问/i.test(detail)) {
      await checkPermissions()
      if (permissionStatus.value.requiresRepair) {
        return `${detail}。请使用页面上方的“一键修复权限”完成 macOS 授权后重试`
      }
    }
    return detail
  }

  function applyPayload(payload: OpencodeGlobalConfigPayload, preferredProviderId?: string | null) {
    editingConfig.value = cloneConfig(payload)
    baseline.value = editableSnapshot(editingConfig.value)
    availableModels.value = {}
    connectionKeyInputs.value = { ...payload.connectionKeys }
    providerOrder.value = reconcileProviderOrder()
    const preferredIndex = preferredProviderId
      ? editingConfig.value.providers.findIndex(provider =>
        provider.originalId === preferredProviderId || provider.id === preferredProviderId,
      )
      : -1
    selectedProviderIndex.value = preferredIndex >= 0
      ? preferredIndex
      : editingConfig.value.providers.length > 0 ? 0 : -1
    loaded.value = true
    loadError.value = ''
  }

  async function loadConfig(force = false) {
    if (loadPromise) return loadPromise
    if (loaded.value && !force) return
    loading.value = true
    loadPromise = (async () => {
      try {
        const [payload, savedOrder] = await Promise.all([
          invoke<OpencodeGlobalConfigPayload>('load_opencode_global_config'),
          invoke<string[]>('load_config_order', { key: 'opencode' }).catch(() => []),
        ])
        providerOrder.value = savedOrder
        const preferredProviderId = selectedProvider.value?.originalId || selectedProvider.value?.id
        applyPayload(payload, preferredProviderId)
        statusMessage.value = `已从 ${payload.configPath} 读取 ${payload.providers.length} 个自定义 Provider`
      } catch (error) {
        loaded.value = false
        loadError.value = `读取 opencode.jsonc 失败：${await permissionAwareError(error)}`
        statusMessage.value = loadError.value
      } finally {
        loading.value = false
        loadPromise = null
      }
    })()
    return loadPromise
  }

  async function ensureLoaded() {
    await loadConfig()
    if (!loaded.value) throw new Error(loadError.value || 'OpenCode 配置尚未加载')
  }

  function discardChanges() {
    const preferredProviderId = selectedProvider.value?.originalId || selectedProvider.value?.id
    const snapshot = JSON.parse(baseline.value) as Pick<
      OpencodeGlobalConfigPayload,
      'model' | 'smallModel' | 'providers'
    >
    editingConfig.value.model = snapshot.model
    editingConfig.value.smallModel = snapshot.smallModel
    editingConfig.value.providers = snapshot.providers
    availableModels.value = {}
    providerOrder.value = reconcileProviderOrder()
    const preferredIndex = preferredProviderId
      ? editingConfig.value.providers.findIndex(provider => provider.id === preferredProviderId)
      : -1
    selectedProviderIndex.value = preferredIndex >= 0
      ? preferredIndex
      : editingConfig.value.providers.length > 0 ? 0 : -1
  }

  function discardSelectedProviderChanges() {
    const provider = selectedProvider.value
    if (!provider || !isSelectedProviderDirty.value) return
    if (!provider.originalId) {
      const removedKey = providerKey(provider)
      editingConfig.value.providers.splice(selectedProviderIndex.value, 1)
      providerOrder.value = providerOrder.value.filter(key => key !== removedKey)
      selectedProviderIndex.value = editingConfig.value.providers.length > 0
        ? Math.min(selectedProviderIndex.value, editingConfig.value.providers.length - 1)
        : -1
      return
    }
    const snapshot = JSON.parse(baseline.value) as Pick<OpencodeGlobalConfigPayload, 'providers'>
    const stored = snapshot.providers.find(item => item.id === provider.originalId)
    if (stored) editingConfig.value.providers[selectedProviderIndex.value] = cloneProvider(stored)
  }

  async function confirmDiscardSelectedProviderChanges(action: string) {
    if (!isSelectedProviderDirty.value) return true
    const provider = selectedProvider.value
    const accepted = await confirm(
      `供应商“${provider?.name || provider?.id || '未命名'}”有未写入的修改。${action}将放弃这些修改，是否继续？`,
      { title: '未写入的 OpenCode 供应商', kind: 'warning' },
    )
    if (accepted) discardSelectedProviderChanges()
    return accepted
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

  async function selectProvider(target: OpencodeGlobalProvider) {
    if (target === selectedProvider.value) return true
    if (!editingConfig.value.providers.includes(target)) return false
    if (!(await confirmDiscardSelectedProviderChanges('切换供应商'))) return false
    const targetIndex = editingConfig.value.providers.indexOf(target)
    if (targetIndex < 0) return false
    selectedProviderIndex.value = targetIndex
    return true
  }

  async function addProvider() {
    if (!(await confirmDiscardSelectedProviderChanges('新建供应商'))) return false
    let index = editingConfig.value.providers.length + 1
    const existingIds = new Set(editingConfig.value.providers.map(provider => provider.id))
    while (existingIds.has(`provider-${index}`)) index += 1
    const provider: OpencodeGlobalProvider = {
      originalId: '',
      id: `provider-${index}`,
      name: '',
      npm: '@ai-sdk/openai-compatible',
      baseUrl: '',
      apiKey: '',
      models: [],
    }
    editingConfig.value.providers.push(provider)
    selectedProviderIndex.value = editingConfig.value.providers.length - 1
    const addedProvider = selectedProvider.value
    if (addedProvider) providerOrder.value.push(providerKey(addedProvider))
    return true
  }

  async function reorderProviders(newOrder: string[]) {
    const previousOrder = [...providerOrder.value]
    if (JSON.stringify(newOrder) === JSON.stringify(previousOrder)) return true
    providerOrder.value = [...newOrder]
    try {
      await persistProviderOrder(newOrder)
      return true
    } catch (error) {
      providerOrder.value = previousOrder
      try {
        await persistProviderOrder(previousOrder)
        statusMessage.value = `保存 OpenCode 供应商排序失败，旧排序已恢复：${error}`
      } catch (rollbackError) {
        statusMessage.value = `保存 OpenCode 供应商排序失败且旧排序恢复未通过校验：${rollbackError}；原始错误：${error}`
      }
      return false
    }
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
      statusMessage.value = `保存 Provider '${provider.id}' Key 失败：${await permissionAwareError(error)}`
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
      statusMessage.value = `连接 Provider '${provider.id}' 失败：${await permissionAwareError(error)}`
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
      statusMessage.value = `禁用 Provider '${provider.id}' 失败：${await permissionAwareError(error)}`
      return false
    }
  }

  async function writeSelectedProvider() {
    const provider = selectedProvider.value
    if (saving.value || !provider) return false
    const providerId = provider.id
    const providerName = provider.name || provider.id
    const currentProviderKey = providerKey(provider)
    const pendingKey = connectionKeyInputs.value[currentProviderKey] || ''
    const preserveGlobalOptions = isGlobalOptionsDirty.value
    const globalDraft = {
      model: editingConfig.value.model,
      smallModel: editingConfig.value.smallModel,
    }
    saving.value = true
    try {
      const payload = await invoke<OpencodeGlobalConfigPayload>('write_opencode_global_provider', {
        request: {
          provider: cloneProvider(provider),
          revision: editingConfig.value.revision,
        },
      })
      providerOrder.value = providerOrder.value.map(key =>
        key === currentProviderKey ? providerId : key,
      )
      applyPayload(payload, providerId)
      let orderWarning = ''
      if (currentProviderKey !== providerId) {
        try {
          await persistProviderOrder(providerOrder.value)
        } catch (error) {
          orderWarning = `；但供应商排序保存失败：${error}`
        }
      }
      if (preserveGlobalOptions) {
        editingConfig.value.model = globalDraft.model
        editingConfig.value.smallModel = globalDraft.smallModel
      }
      if (pendingKey && selectedProvider.value) {
        connectionKeyInputs.value[providerKey(selectedProvider.value)] = pendingKey
      }
      statusMessage.value = `供应商 '${providerName}' 已写入 ${payload.configPath}；其它供应商保持不变${orderWarning}`
      return true
    } catch (error) {
      statusMessage.value = `写入供应商 '${providerName}' 失败，界面内容已保留：${await permissionAwareError(error)}`
      return false
    } finally {
      saving.value = false
    }
  }

  async function deleteSelectedProvider() {
    const provider = selectedProvider.value
    if (saving.value || !provider) return false
    const providerName = provider.name || provider.id
    const accepted = await confirm(
      provider.originalId
        ? `确定从 opencode.jsonc 删除供应商“${providerName}”吗？其它供应商及 auth.json 中的 Key 均会保留。`
        : `供应商“${providerName}”尚未写入 opencode.jsonc，确定放弃这个草稿吗？`,
      { title: '删除 OpenCode 供应商', kind: 'warning' },
    )
    if (!accepted) return false

    if (!provider.originalId) {
      const removedKey = providerKey(provider)
      editingConfig.value.providers.splice(selectedProviderIndex.value, 1)
      providerOrder.value = providerOrder.value.filter(key => key !== removedKey)
      selectedProviderIndex.value = editingConfig.value.providers.length > 0
        ? Math.min(selectedProviderIndex.value, editingConfig.value.providers.length - 1)
        : -1
      try {
        await persistProviderOrder(providerOrder.value)
        statusMessage.value = `未写入的供应商 '${providerName}' 已移除`
      } catch (error) {
        statusMessage.value = `未写入的供应商 '${providerName}' 已移除，但排序状态清理失败：${error}`
      }
      return true
    }

    const deletedIndex = selectedProviderIndex.value
    const preserveGlobalOptions = isGlobalOptionsDirty.value
    const globalDraft = {
      model: editingConfig.value.model,
      smallModel: editingConfig.value.smallModel,
    }
    const remainingProviders = editingConfig.value.providers.filter(item => item !== provider)
    const nextProvider = remainingProviders.length > 0
      ? remainingProviders[Math.min(deletedIndex, remainingProviders.length - 1)]
      : null
    const nextProviderId = nextProvider?.originalId || nextProvider?.id || null
    saving.value = true
    try {
      const payload = await invoke<OpencodeGlobalConfigPayload>('delete_opencode_global_provider', {
        request: {
          providerId: provider.originalId,
          revision: editingConfig.value.revision,
        },
      })
      applyPayload(payload, nextProviderId)
      let orderWarning = ''
      try {
        await persistProviderOrder(providerOrder.value)
      } catch (error) {
        orderWarning = `；但供应商排序清理失败：${error}`
      }
      if (preserveGlobalOptions) {
        editingConfig.value.model = globalDraft.model
        editingConfig.value.smallModel = globalDraft.smallModel
      }
      statusMessage.value = `供应商 '${providerName}' 已从 ${payload.configPath} 删除；其它供应商和 Key 保持不变${orderWarning}`
      return true
    } catch (error) {
      statusMessage.value = `删除供应商 '${providerName}' 失败：${await permissionAwareError(error)}`
      return false
    } finally {
      saving.value = false
    }
  }

  async function saveGlobalOptions() {
    if (saving.value || !isGlobalOptionsDirty.value) return false
    const providerDraft = selectedProvider.value && isSelectedProviderDirty.value
      ? cloneProvider(selectedProvider.value)
      : null
    const selectedId = selectedProvider.value?.originalId || selectedProvider.value?.id || null
    const providerDraftKey = selectedProvider.value && providerDraft
      ? providerKey(selectedProvider.value)
      : null
    const providerDraftOrderIndex = providerDraftKey
      ? providerOrder.value.indexOf(providerDraftKey)
      : -1
    const pendingKey = selectedProvider.value
      ? connectionKeyInputs.value[providerKey(selectedProvider.value)] || ''
      : ''
    saving.value = true
    try {
      const payload = await invoke<OpencodeGlobalConfigPayload>('save_opencode_global_options', {
        request: {
          model: editingConfig.value.model,
          smallModel: editingConfig.value.smallModel,
          revision: editingConfig.value.revision,
        },
      })
      applyPayload(payload, selectedId)
      if (providerDraft) {
        const storedIndex = providerDraft.originalId
          ? editingConfig.value.providers.findIndex(item => item.id === providerDraft.originalId)
          : -1
        if (storedIndex >= 0) {
          editingConfig.value.providers[storedIndex] = providerDraft
          selectedProviderIndex.value = storedIndex
        } else {
          editingConfig.value.providers.push(providerDraft)
          selectedProviderIndex.value = editingConfig.value.providers.length - 1
        }
        const restoredProvider = selectedProvider.value
        if (restoredProvider) {
          if (!restoredProvider.originalId && providerDraftKey) {
            providerClientKeys.set(restoredProvider, providerDraftKey)
          }
          const restoredKey = providerKey(restoredProvider)
          if (!providerOrder.value.includes(restoredKey)) {
            const insertAt = providerDraftOrderIndex >= 0
              ? Math.min(providerDraftOrderIndex, providerOrder.value.length)
              : providerOrder.value.length
            providerOrder.value.splice(insertAt, 0, restoredKey)
          }
          if (pendingKey) connectionKeyInputs.value[restoredKey] = pendingKey
        }
      }
      statusMessage.value = `OpenCode 默认模型选项已保存到 ${payload.configPath}`
      return true
    } catch (error) {
      statusMessage.value = `保存 OpenCode 全局选项失败：${await permissionAwareError(error)}`
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
    permissionStatus,
    permissionChecking,
    permissionRepairing,
    permissionError,
    selectedProviderIndex,
    selectedProvider,
    providerOrder,
    orderedProviders,
    isDirty,
    isGlobalOptionsDirty,
    isSelectedProviderDirty,
    configuredModelIds,
    globalConfigPath,
    checkPermissions,
    repairPermissions,
    loadConfig,
    ensureLoaded,
    refreshConfig,
    writeSelectedProvider,
    deleteSelectedProvider,
    saveGlobalOptions,
    discardChanges,
    discardSelectedProviderChanges,
    confirmDiscardChanges,
    confirmDiscardSelectedProviderChanges,
    selectProvider,
    addProvider,
    reorderProviders,
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
