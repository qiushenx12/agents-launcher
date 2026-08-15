import { computed, ref, watch } from 'vue'
import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import { confirm, message } from '@tauri-apps/plugin-dialog'
import deepSeekModelCatalogTemplate from '../deepseekModelsTemplate.json'
import { useAppSettingsStore } from './appSettings'
import type {
  CliProfileRef,
  CodexModelDefinition,
  CodexLaunchContext,
  CodexProfile,
  CodexProfilesPayload,
} from '@/types/config'

const TEMPLATE_MODEL_FIELDS = new Set([
  'slug',
  'display_name',
  'input_modalities',
  'supports_image_detail_original',
  'context_window',
  'max_context_window',
  'effective_context_window_percent',
  'truncation_policy',
  'default_reasoning_level',
  'supported_reasoning_levels',
])

function templateModelForSlug(slug: string) {
  return deepSeekModelCatalogTemplate.models.find(model => model.slug === slug)
    ?? deepSeekModelCatalogTemplate.models[0]
}

function normalizeInputModalities(value: unknown): string[] {
  const modalities = Array.isArray(value)
    ? value.filter((item): item is string => typeof item === 'string')
    : []
  const normalized = modalities
    .map(item => item.trim().toLowerCase())
    .filter(Boolean)
    .filter((item, index, items) => items.indexOf(item) === index)
  if (!normalized.includes('text')) normalized.unshift('text')
  return normalized
}

function emptyModelDefinition(
  slug = '',
  options: { blankWhenEmpty?: boolean } = {},
): CodexModelDefinition {
  const normalizedSlug = slug.trim()
  const template = templateModelForSlug(normalizedSlug)
  const templateExtra = Object.fromEntries(
    Object.entries(template).filter(([key]) => !TEMPLATE_MODEL_FIELDS.has(key)),
  )
  const resolvedSlug = normalizedSlug || (options.blankWhenEmpty ? '' : template.slug)
  return {
    ...templateExtra,
    slug: resolvedSlug,
    displayName: resolvedSlug,
    inputModalities: normalizeInputModalities(template.input_modalities),
    supportsImageDetailOriginal: template.supports_image_detail_original ?? false,
    contextWindow: template.context_window,
    maxContextWindow: template.max_context_window,
    effectiveContextWindowPercent: template.effective_context_window_percent,
    truncationPolicy: JSON.parse(JSON.stringify(template.truncation_policy)) as CodexModelDefinition['truncationPolicy'],
    defaultReasoningLevel: template.default_reasoning_level,
    supportedReasoningLevels: JSON.parse(JSON.stringify(template.supported_reasoning_levels)) as CodexModelDefinition['supportedReasoningLevels'],
  }
}

function emptyProfile(): CodexProfile {
  return {
    id: '',
    name: '',
    authMode: 'official',
    model: '',
    reasoningEffort: '',
    openaiBaseUrl: '',
    providerId: '',
    providerName: '',
    baseUrl: '',
    wireApi: 'responses',
    protocolConversion: false,
    chatUpstreamModel: '',
    promptCacheRouting: 'auto',
    envKey: 'OPENAI_API_KEY',
    hasStoredApiKey: false,
    managedProfileName: '',
    modelCatalog: null,
  }
}

function cloneProfile(profile: CodexProfile): CodexProfile {
  return JSON.parse(JSON.stringify(profile)) as CodexProfile
}

function defaultProfileName(): string {
  return `profile_${crypto.randomUUID().replace(/-/g, '').slice(0, 8)}`
}

function providerIdFromProfileName(name: string, fallback: string): string {
  let providerId = ''
  let needsSeparator = false
  for (const character of name.trim()) {
    if (/^[A-Za-z0-9_-]$/.test(character)) {
      if (needsSeparator && providerId && !/[-_]$/.test(providerId)) providerId += '_'
      providerId += character
      needsSeparator = false
    } else if (providerId) {
      needsSeparator = true
    }
  }
  providerId = providerId.replace(/^[-_]+|[-_]+$/g, '')
  if (!providerId) return fallback
  if (['openai', 'ollama', 'lmstudio'].includes(providerId.toLowerCase())) {
    return `${providerId}_custom`
  }
  return providerId
}

function syncProviderIdentity(profile: CodexProfile) {
  if (profile.authMode !== 'custom') return
  profile.providerName = profile.name.trim()
  profile.providerId = providerIdFromProfileName(profile.name, profile.id)
}

function serializeDraft(profile: CodexProfile, apiKeyInput: string, clearApiKey: boolean): string {
  return JSON.stringify({
    id: profile.id,
    name: profile.name,
    authMode: profile.authMode,
    model: profile.model,
    reasoningEffort: profile.reasoningEffort,
    openaiBaseUrl: profile.openaiBaseUrl,
    providerId: profile.providerId,
    providerName: profile.providerName,
    baseUrl: profile.baseUrl,
    wireApi: profile.wireApi,
    protocolConversion: profile.protocolConversion,
    chatUpstreamModel: profile.chatUpstreamModel,
    promptCacheRouting: profile.promptCacheRouting,
    envKey: profile.envKey,
    modelCatalog: profile.modelCatalog,
    apiKeyInput,
    clearApiKey,
  })
}

export const useCodexConfigStore = defineStore('codexConfig', () => {
  const appSettingsStore = useAppSettingsStore()
  const profiles = ref<CodexProfile[]>([])
  const order = ref<string[]>([])
  const activeProfileId = ref<string | null>(null)
  const globalProfileId = ref<string | null>(null)
  const globalProfileInSync = ref(false)
  const globalSyncRepairRequired = ref(false)
  const selectedProfileId = ref<string | null>(null)
  const editingProfile = ref<CodexProfile>(emptyProfile())
  const apiKeyInput = ref('')
  const revealedStoredApiKey = ref('')
  const storedApiKeyRevealed = ref(false)
  const apiKeyRevealing = ref(false)
  const clearStoredApiKey = ref(false)
  const baseline = ref(serializeDraft(editingProfile.value, '', false))
  const loaded = ref(false)
  const loadError = ref('')
  const loading = ref(false)
  const saving = ref(false)
  const applying = ref(false)
  const modelsFetching = ref(false)
  const availableModels = ref<string[]>([])
  const syncToGlobal = ref(false)
  const statusMessage = ref('')
  const profilesPath = ref('')
  const globalConfigPath = ref('')
  const authPath = ref('')
  const globalConfigError = ref<string | null>(null)
  const authStatus = ref<CodexProfilesPayload['authStatus']>({
    mode: null,
    hasAuthFile: false,
    hasCredentials: false,
    error: null,
  })
  const customGlobalSyncSupported = ref(false)
  const customGlobalKeySyncSupported = ref(false)
  const secretStorageKind = ref<CodexProfilesPayload['secretStorageKind']>('unsupported')
  const platform = ref('unknown')

  const orderedProfiles = computed(() => {
    const byId = new Map(profiles.value.map(profile => [profile.id, profile]))
    const ordered = order.value.map(id => byId.get(id)).filter(Boolean) as CodexProfile[]
    const included = new Set(ordered.map(profile => profile.id))
    return [...ordered, ...profiles.value.filter(profile => !included.has(profile.id))]
  })

  const activeProfile = computed(() =>
    profiles.value.find(profile => profile.id === activeProfileId.value) ?? null,
  )

  const activeProfileRef = computed<CliProfileRef | null>(() => activeProfile.value
    ? { cliKind: 'codex', profileId: activeProfile.value.id }
    : null)

  const draftApiKeyInput = computed(() =>
    apiKeyInput.value === revealedStoredApiKey.value ? '' : apiKeyInput.value,
  )

  const isDirty = computed(() => {
    const expected = cloneProfile(editingProfile.value)
    syncProviderIdentity(expected)
    return serializeDraft(editingProfile.value, draftApiKeyInput.value, clearStoredApiKey.value)
        !== baseline.value
      || editingProfile.value.providerId !== expected.providerId
      || editingProfile.value.providerName !== expected.providerName
  })

  const authStatusLabel = computed(() => {
    if (authStatus.value.error) return `登录状态文件异常：${authStatus.value.error}`
    if (authStatus.value.mode === 'chatgpt') return '已检测到 Codex ChatGPT 登录'
    if (authStatus.value.mode) return `已检测到 Codex 登录模式：${authStatus.value.mode}`
    if (authStatus.value.hasCredentials) return '已检测到 Codex 登录凭据'
    return '未在 auth.json 中检测到登录；也可能由系统凭据存储管理'
  })

  function markClean() {
    baseline.value = serializeDraft(editingProfile.value, draftApiKeyInput.value, clearStoredApiKey.value)
  }

  function resetApiKeyEditor() {
    apiKeyInput.value = ''
    revealedStoredApiKey.value = ''
    storedApiKeyRevealed.value = false
    clearStoredApiKey.value = false
  }

  function editProfile(profile: CodexProfile) {
    selectedProfileId.value = profile.id
    const cloned = cloneProfile(profile)
    cloned.protocolConversion = Boolean(cloned.protocolConversion)
    cloned.chatUpstreamModel = typeof cloned.chatUpstreamModel === 'string'
      ? cloned.chatUpstreamModel
      : ''
    cloned.promptCacheRouting = cloned.promptCacheRouting === 'enabled'
      || cloned.promptCacheRouting === 'disabled'
      ? cloned.promptCacheRouting
      : 'auto'
    editingProfile.value = cloned
    resetApiKeyEditor()
    availableModels.value = []
    syncToGlobal.value = Boolean(profile.id && globalProfileId.value === profile.id)
    markClean()
  }

  function enableModelCatalog() {
    if (editingProfile.value.modelCatalog) return
    const initialModel = emptyModelDefinition('', { blankWhenEmpty: true })
    editingProfile.value.modelCatalog = {
      models: [initialModel],
    }
    editingProfile.value.model = ''
    statusMessage.value = '已启用第三方 models.json 配置，模型字段沿用模板，请填写模型 slug'
  }

  function disableModelCatalog() {
    const models = editingProfile.value.modelCatalog?.models
    const selectedModel = models?.find(model => model.slug === editingProfile.value.model.trim())
      ?? models?.[0]
    if (selectedModel?.slug.trim()) editingProfile.value.model = selectedModel.slug.trim()
    editingProfile.value.modelCatalog = null
    statusMessage.value = '已切换为不使用模型目录，默认模型将直接写入 CodeX profile'
  }

  function syncModelCatalogDraft(profile: CodexProfile) {
    const models = profile.modelCatalog?.models
    if (!models || models.length === 0) return
    const selectedModel = models.find(model => (
      model.slug.trim() === profile.model.trim()
      || model.displayName.trim() === profile.model.trim()
    ))
    const selectedModelName = selectedModel
      ? (selectedModel.slug.trim() || selectedModel.displayName.trim())
      : ''
    for (const model of models) {
      model.slug = model.slug.trim()
      model.displayName = model.slug
      model.maxContextWindow = model.contextWindow
      model.inputModalities = normalizeInputModalities(model.inputModalities)
      if (typeof model.supportsImageDetailOriginal !== 'boolean') {
        model.supportsImageDetailOriginal = false
      }
    }
    profile.model = selectedModelName && models.some(model => model.slug === selectedModelName)
      ? selectedModelName
      : models[0].slug
  }

  function addModel(slug: string): boolean {
    if (!editingProfile.value.modelCatalog) enableModelCatalog()
    const models = editingProfile.value.modelCatalog?.models
    if (!models) return false
    const normalizedSlug = slug.trim()
    if (!normalizedSlug) {
      statusMessage.value = '请输入模型名称'
      return false
    }
    if (!/^[A-Za-z0-9._-]+$/.test(normalizedSlug)) {
      statusMessage.value = '模型名称只能包含字母、数字、短横线、下划线和点'
      return false
    }
    if (models.some(model => model.slug === normalizedSlug)) {
      statusMessage.value = `模型 '${normalizedSlug}' 已存在`
      return false
    }
    const placeholderIndex = models.findIndex(model => !model.slug.trim())
    if (placeholderIndex >= 0) models[placeholderIndex] = emptyModelDefinition(normalizedSlug)
    else models.push(emptyModelDefinition(normalizedSlug))
    if (!editingProfile.value.model.trim()) editingProfile.value.model = normalizedSlug
    statusMessage.value = `已添加模型 '${normalizedSlug}'`
    return true
  }

  function removeModel(slug: string): boolean {
    const models = editingProfile.value.modelCatalog?.models
    if (!models) return false
    if (models.length <= 1) {
      statusMessage.value = '至少需要保留一个模型'
      return false
    }
    const index = models.findIndex(model => model.slug === slug)
    if (index < 0) return false
    const wasDefault = editingProfile.value.model === slug
    models.splice(index, 1)
    if (wasDefault || !models.some(model => model.slug === editingProfile.value.model)) {
      editingProfile.value.model = models[0]?.slug ?? ''
    }
    statusMessage.value = `已移除模型 '${slug}'`
    return true
  }

  function setDefaultModel(slug: string): boolean {
    const models = editingProfile.value.modelCatalog?.models
    if (!models?.some(model => model.slug === slug)) return false
    editingProfile.value.model = slug
    return true
  }

  function applyPayload(payload: CodexProfilesPayload, preferredProfileId?: string | null) {
    profiles.value = payload.profiles.map(cloneProfile)
    order.value = [...payload.order]
    activeProfileId.value = payload.activeProfileId
      && profiles.value.some(profile => profile.id === payload.activeProfileId)
      ? payload.activeProfileId
      : null
    globalProfileId.value = payload.globalProfileId
    globalProfileInSync.value = payload.globalProfileInSync
    globalSyncRepairRequired.value = payload.globalSyncRepairRequired
    profilesPath.value = payload.profilesPath
    globalConfigPath.value = payload.globalConfigPath
    authPath.value = payload.authPath
    globalConfigError.value = payload.globalConfigError
    authStatus.value = payload.authStatus
    customGlobalSyncSupported.value = payload.customGlobalSyncSupported
    customGlobalKeySyncSupported.value = payload.customGlobalKeySyncSupported
    secretStorageKind.value = payload.secretStorageKind
    platform.value = payload.platform
    const fallbackSelectedId = selectedProfileId.value
      && profiles.value.some(profile => profile.id === selectedProfileId.value)
      ? selectedProfileId.value
      : activeProfileId.value
        && profiles.value.some(profile => profile.id === activeProfileId.value)
        ? activeProfileId.value
        : orderedProfiles.value[0]?.id ?? null
    const selectedId = preferredProfileId
      && profiles.value.some(profile => profile.id === preferredProfileId)
      ? preferredProfileId
      : fallbackSelectedId
    const selected = profiles.value.find(profile => profile.id === selectedId)
    if (selected) editProfile(selected)
    else {
      selectedProfileId.value = null
      editingProfile.value = emptyProfile()
      resetApiKeyEditor()
      availableModels.value = []
      syncToGlobal.value = false
      markClean()
    }
  }

  async function loadProfiles(force = false) {
    if (loading.value || (loaded.value && !force)) return
    const initialLoad = !loaded.value
    loading.value = true
    try {
      const payload = await invoke<CodexProfilesPayload>('load_codex_profiles')
      applyPayload(payload)
      loaded.value = true
      loadError.value = ''
      if (payload.globalConfigError) statusMessage.value = payload.globalConfigError
      if (initialLoad && payload.globalSyncRepairRequired) {
        void promptGlobalSyncRepair()
      }
    } catch (error) {
      loaded.value = false
      loadError.value = `加载 CodeX 配置失败：${error}`
      statusMessage.value = loadError.value
    } finally {
      loading.value = false
    }
  }

  async function promptGlobalSyncRepair(): Promise<void> {
    const profile = (globalProfileId.value
      && profiles.value.find(item => item.id === globalProfileId.value))
      ?? profiles.value.find(item => item.id === selectedProfileId.value)
      ?? orderedProfiles.value[0]
    if (!profile) return

    const shouldSync = await confirm(
      '检测到 Codex 全局配置与启动器状态不一致，当前无法安全恢复协议转换代理。点击“同步”将使用当前配置重新写入并同步全局；点击“取消”仅关闭此提示，不修改配置。',
      {
        title: '需要重新同步 Codex 全局配置',
        kind: 'warning',
        okLabel: '同步',
        cancelLabel: '取消',
      },
    )
    if (!shouldSync) return

    editProfile(profile)
    syncToGlobal.value = true
    await applyProfile()
  }

  async function ensureLoaded() {
    await loadProfiles()
    if (!loaded.value) throw new Error(loadError.value || 'CodeX 配置尚未加载')
  }

  function discardChanges() {
    const selected = profiles.value.find(profile => profile.id === selectedProfileId.value)
    if (selected) editProfile(selected)
    else {
      selectedProfileId.value = null
      editingProfile.value = emptyProfile()
      resetApiKeyEditor()
      availableModels.value = []
      syncToGlobal.value = false
      markClean()
    }
  }

  async function confirmDiscardChanges(action: string): Promise<boolean> {
    if (!isDirty.value) return true
    const accepted = await confirm(
      `当前 CodeX 配置有未保存的修改。${action}将放弃这些修改，是否继续？`,
      { title: '未保存的 CodeX 配置', kind: 'warning' },
    )
    if (accepted) discardChanges()
    return accepted
  }

  async function selectProfile(profileId: string): Promise<boolean> {
    if (profileId === selectedProfileId.value) return true
    if (!(await confirmDiscardChanges('切换配置方案'))) return false
    const profile = profiles.value.find(item => item.id === profileId)
    if (!profile) return false
    editProfile(profile)
    statusMessage.value = profileId === activeProfileId.value
      ? `CodeX 配置 '${profile.name}' 当前应用中`
      : ''
    return true
  }

  async function newProfile(): Promise<boolean> {
    if (!(await confirmDiscardChanges('新建配置方案'))) return false
    selectedProfileId.value = null
    editingProfile.value = emptyProfile()
    editingProfile.value.name = defaultProfileName()
    resetApiKeyEditor()
    availableModels.value = []
    syncToGlobal.value = false
    markClean()
    return true
  }

  async function saveProfile(): Promise<boolean> {
    if (saving.value || apiKeyRevealing.value) return false
    const profile = cloneProfile(editingProfile.value)
    if (!profile.id) profile.id = `profile-${crypto.randomUUID()}`
    const previousProfile = profiles.value.find(item => item.id === profile.id)
    const nameChanged = Boolean(previousProfile)
      && previousProfile!.name.trim() !== profile.name.trim()
    const isActiveProfile = profile.id === activeProfileId.value
    const isGlobalProfile = profile.id === globalProfileId.value
    if (nameChanged && (isActiveProfile || isGlobalProfile)) {
      const appliedScopes = [
        isActiveProfile ? '启动器当前应用' : '',
        isGlobalProfile ? 'Codex 全局配置' : '',
      ].filter(Boolean).join('和')
      await message(
        `当前配置正在${appliedScopes}中，不能直接改名。改名会破坏配置与 Provider 的关联。请先将其他配置应用到对应范围，使当前配置不再处于应用状态，然后再修改名称并保存。`,
        { title: '无法修改已应用配置名称', kind: 'warning' },
      )
      return false
    }
    saving.value = true
    const requestedGlobalSync = syncToGlobal.value
    syncProviderIdentity(profile)
    syncModelCatalogDraft(profile)
    const nextOrder = order.value.includes(profile.id) ? [...order.value] : [...order.value, profile.id]
    const apiKeyForSave = apiKeyInput.value === revealedStoredApiKey.value
      ? null
      : apiKeyInput.value.trim() || null
    try {
      const payload = await invoke<CodexProfilesPayload>('save_codex_profile', {
        request: {
          profile,
          apiKey: apiKeyForSave,
          clearApiKey: clearStoredApiKey.value,
          order: nextOrder,
          activeProfileId: activeProfileId.value,
        },
      })
      applyPayload(payload, profile.id)
      syncToGlobal.value = requestedGlobalSync
      statusMessage.value = `CodeX 配置 '${editingProfile.value.name}' 已保存并通过磁盘校验`
      return true
    } catch (error) {
      statusMessage.value = `保存 CodeX 配置失败，表单内容已保留：${error}`
      return false
    } finally {
      saving.value = false
    }
  }

  async function reorderProfiles(newOrder: string[]): Promise<boolean> {
    const previousOrder = [...order.value]
    if (JSON.stringify(newOrder) === JSON.stringify(previousOrder)) return true

    try {
      await invoke('save_config_order', { key: 'codex', order: newOrder })
      const persistedOrder = await invoke<string[]>('load_config_order', { key: 'codex' })
      if (JSON.stringify(persistedOrder) !== JSON.stringify(newOrder)) {
        throw new Error('排序写入后回读不一致')
      }
      order.value = [...newOrder]
      return true
    } catch (error) {
      try {
        await invoke('save_config_order', { key: 'codex', order: previousOrder })
        const restoredOrder = await invoke<string[]>('load_config_order', { key: 'codex' })
        if (JSON.stringify(restoredOrder) !== JSON.stringify(previousOrder)) {
          throw new Error('旧排序恢复后回读不一致')
        }
        statusMessage.value = `保存 CodeX 配置排序失败，旧排序已恢复：${error}`
      } catch (rollbackError) {
        statusMessage.value = `保存 CodeX 配置排序失败且旧排序恢复未通过校验：${rollbackError}；原始错误：${error}`
      }
      return false
    }
  }

  async function applyProfile(): Promise<boolean> {
    const profile = profiles.value.find(item => item.id === selectedProfileId.value)
    if (!profile || isDirty.value || applying.value || apiKeyRevealing.value) return false
    const applyToGlobal = syncToGlobal.value
      && (profile.authMode === 'official' || customGlobalSyncSupported.value)
    let protocolConversionWithoutTray = false

    if (profile.protocolConversion && profile.authMode === 'custom') {
      await appSettingsStore.load()
      if (!appSettingsStore.minimizeToTray) {
        const shouldEnable = await confirm(
          '协议转换代理需要在应用关闭后继续运行。建议开启“关闭时最小化到托盘”，以免关闭启动器后代理停止。点击“确定”自动开启；点击“取消”保持关闭。',
          {
            title: '建议开启关闭时最小化到托盘',
            kind: 'warning',
            okLabel: '确定',
            cancelLabel: '取消',
          },
        )
        if (shouldEnable) {
          try {
            await appSettingsStore.setMinimizeToTray(true)
          } catch (error) {
            statusMessage.value = `关闭时最小化到托盘设置保存失败：${error}`
          }
        }
        protocolConversionWithoutTray = !appSettingsStore.minimizeToTray
      }
    }

    if (profile.id === activeProfileId.value
      && (!applyToGlobal
        || (profile.id === globalProfileId.value && globalProfileInSync.value))) return true

    applying.value = true
    try {
      const payload = await invoke<CodexProfilesPayload>('apply_codex_profile', {
        request: {
          profileId: profile.id,
          applyToGlobal,
        },
      })
      applyPayload(payload, profile.id)
      if (payload.activeProfileId !== profile.id) {
        throw new Error('活动方案写入后回读不一致')
      }
      if (applyToGlobal && payload.globalProfileId !== profile.id) {
        throw new Error('全局方案写入后回读不一致')
      }
      syncToGlobal.value = applyToGlobal
      if (applyToGlobal
        && profile.authMode === 'custom'
        && secretStorageKind.value === 'macos_plaintext'
        && profile.hasStoredApiKey) {
        statusMessage.value = `CodeX 配置 '${profile.name}' 已同步到启动器和全局；Codex 将通过命令式认证从启动器凭据文件读取 Key`
      } else if (applyToGlobal
        && profile.authMode === 'custom'
        && !customGlobalKeySyncSupported.value) {
        statusMessage.value = `CodeX 配置 '${profile.name}' 已同步到启动器和全局；外部 CodeX 仍需自行设置 ${profile.envKey}`
      } else {
        statusMessage.value = applyToGlobal
          ? `CodeX 配置 '${profile.name}' 已应用到启动器和全局；请完全退出（含后台）并重新打开外部终端及 CodeX 桌面端后生效`
          : `CodeX 配置 '${profile.name}' 已应用；新启动或重新打开的 CodeX 终端将使用该配置`
      }
      if (protocolConversionWithoutTray) {
        statusMessage.value += '；注意：关闭时最小化到托盘未开启，关闭启动器后协议转换代理将停止'
      }
      return true
    } catch (error) {
      await loadProfiles(true)
      syncToGlobal.value = applyToGlobal
      statusMessage.value = `应用 CodeX 配置失败：${error}`
      return false
    } finally {
      applying.value = false
    }
  }

  async function revealApiKey(): Promise<boolean> {
    const profile = editingProfile.value
    if (!profile.id
      || profile.authMode !== 'custom'
      || !profile.hasStoredApiKey
      || apiKeyRevealing.value) return false

    const requestedProfileId = profile.id
    const requestedProfileName = profile.name
    apiKeyRevealing.value = true
    try {
      const apiKey = await invoke<string | null>('reveal_codex_profile_api_key', {
        profileId: requestedProfileId,
      })
      if (editingProfile.value.id !== requestedProfileId) return false
      if (!apiKey) {
        editingProfile.value.hasStoredApiKey = false
        storedApiKeyRevealed.value = false
        const storedProfile = profiles.value.find(item => item.id === requestedProfileId)
        if (storedProfile) storedProfile.hasStoredApiKey = false
        statusMessage.value = `CodeX 配置 '${requestedProfileName}' 没有可读取的已保存 Key`
        return false
      }

      apiKeyInput.value = apiKey
      revealedStoredApiKey.value = apiKey
      storedApiKeyRevealed.value = true
      clearStoredApiKey.value = false
      const storageLabel = secretStorageKind.value === 'macos_plaintext'
        ? '启动器凭据文件'
        : secretStorageKind.value === 'windows_dpapi'
          ? '安全凭据存储'
          : '凭据存储'
      statusMessage.value = `已从 ${storageLabel} 读取 Key；点击“隐藏”可重新遮挡`
      return true
    } catch (error) {
      if (editingProfile.value.id === requestedProfileId) {
        statusMessage.value = `读取已保存的 CodeX Key 失败：${error}`
      }
      return false
    } finally {
      apiKeyRevealing.value = false
    }
  }

  async function fetchModels(): Promise<boolean> {
    if (editingProfile.value.authMode !== 'custom') {
      statusMessage.value = '官方登录模式使用 Codex 提供的模型选择，无需从第三方地址获取'
      return false
    }
    if (!editingProfile.value.baseUrl.trim()) {
      statusMessage.value = '请先输入第三方 Base URL'
      return false
    }
    if (modelsFetching.value) return false
    modelsFetching.value = true
    availableModels.value = []
    try {
      const models = await invoke<string[]>('fetch_codex_models', {
        request: {
          profileId: editingProfile.value.id,
          baseUrl: editingProfile.value.baseUrl,
          apiKey: apiKeyInput.value.trim() || null,
          envKey: editingProfile.value.envKey,
        },
      })
      availableModels.value = models
      statusMessage.value = `已获取 ${models.length} 个第三方模型`
      return true
    } catch (error) {
      statusMessage.value = `获取模型失败：${error}`
      return false
    } finally {
      modelsFetching.value = false
    }
  }

  async function deleteProfile(profileId: string): Promise<boolean> {
    const profile = profiles.value.find(item => item.id === profileId)
    if (!profile) return false
    if (!(await confirmDiscardChanges(`删除配置“${profile.name}”`))) return false
    const globalNotice = globalProfileId.value === profileId
      ? '该方案曾同步到全局；删除方案不会撤销已写入的全局 config.toml，请先同步另一个全局方案。'
      : ''
    const accepted = await confirm(
      `确定删除 CodeX 配置“${profile.name}”吗？对应的启动器 profile TOML 和加密凭据也会删除。${globalNotice}`,
      { title: '删除 CodeX 配置', kind: 'warning' },
    )
    if (!accepted) return false
    const nextOrder = order.value.filter(id => id !== profileId)
    const nextActive = activeProfileId.value === profileId
      ? null
      : activeProfileId.value
    const nextSelected = selectedProfileId.value === profileId
      ? nextOrder[0] ?? null
      : selectedProfileId.value
    try {
      const payload = await invoke<CodexProfilesPayload>('delete_codex_profile', {
        request: {
          profileId,
          order: nextOrder,
          activeProfileId: nextActive,
        },
      })
      applyPayload(payload, nextSelected)
      const activeNotice = activeProfileId.value ? '' : '；当前没有应用启动器方案'
      const removedGlobalNotice = globalNotice ? '；已写入的全局配置保持不变' : ''
      statusMessage.value = `CodeX 配置 '${profile.name}' 已删除${activeNotice}${removedGlobalNotice}`
      return true
    } catch (error) {
      statusMessage.value = `删除 CodeX 配置失败：${error}`
      return false
    }
  }

  async function resolveLaunchContext(profileId: string): Promise<CodexLaunchContext> {
    await ensureLoaded()
    return invoke<CodexLaunchContext>('resolve_codex_profile', { profileId })
  }

  watch(apiKeyInput, (value, previous) => {
    if (value) clearStoredApiKey.value = false
    if (value !== previous) availableModels.value = []
  })

  watch(
    () => [editingProfile.value.authMode, editingProfile.value.baseUrl],
    () => { availableModels.value = [] },
  )

  return {
    profiles,
    order,
    orderedProfiles,
    activeProfileId,
    globalProfileId,
    globalProfileInSync,
    globalSyncRepairRequired,
    selectedProfileId,
    activeProfile,
    activeProfileRef,
    editingProfile,
    enableModelCatalog,
    disableModelCatalog,
    addModel,
    removeModel,
    setDefaultModel,
    apiKeyInput,
    storedApiKeyRevealed,
    apiKeyRevealing,
    clearStoredApiKey,
    isDirty,
    loaded,
    loadError,
    loading,
    saving,
    applying,
    modelsFetching,
    availableModels,
    syncToGlobal,
    statusMessage,
    profilesPath,
    globalConfigPath,
    authPath,
    globalConfigError,
    authStatus,
    customGlobalSyncSupported,
    customGlobalKeySyncSupported,
    secretStorageKind,
    platform,
    authStatusLabel,
    loadProfiles,
    ensureLoaded,
    selectProfile,
    newProfile,
    saveProfile,
    reorderProfiles,
    applyProfile,
    revealApiKey,
    fetchModels,
    deleteProfile,
    discardChanges,
    confirmDiscardChanges,
    resolveLaunchContext,
  }
})
