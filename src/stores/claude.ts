import { defineStore } from 'pinia'
import { ref, computed, nextTick, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { confirm } from '@tauri-apps/plugin-dialog'
import type { ClaudeSettings, CliProfileRef, SessionEntry } from '@/types/config'
import { formatRedactedEntries } from '@/utils/configSecurity'
import { useConfigWorkspaceStore } from './configWorkspace'
import { useProjectStore } from './project'

export interface ClaudeCodeCheckResult {
  installed: boolean
  path: string | null
  version: string | null
  message: string
}

const KNOWN_ENV_KEYS = new Set([
  'ANTHROPIC_BASE_URL',
  'ANTHROPIC_AUTH_TOKEN',
  'ANTHROPIC_MODEL',
  'ANTHROPIC_DEFAULT_OPUS_MODEL',
  'ANTHROPIC_DEFAULT_SONNET_MODEL',
  'ANTHROPIC_DEFAULT_HAIKU_MODEL',
  'CLAUDE_CODE_SUBAGENT_MODEL',
  'CLAUDE_CODE_EFFORT_LEVEL',
  'DISABLE_AUTOUPDATER',
  'CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS',
])

export const useClaudeStore = defineStore('claude', () => {
  // ── State ──────────────────────────────────────────────────────────────────
  const configs = ref<Record<string, Record<string, string>>>({})
  const configOrder = ref<string[]>([])
  const profileIds = ref<Record<string, string>>({})
  const activeConfigName = ref<string | null>(null)
  const editingConfig = ref<{ name: string; vars: Record<string, string> }>({
    name: '',
    vars: {},
  })
  const activeSource = ref<'env' | 'manual' | string>('env')
  const sessions = ref<SessionEntry[]>([])
  const sessionDisplayCount = ref(10)
  const availableModels = ref<string[]>([])
  const modelsFetching = ref(false)
  const launchDir = ref('')
  const launchDirHistory = ref<string[]>([])
  const claudeExePath = ref<string | null>(null)
  const claudeInstalled = ref(false)
  const skipPermissions = ref(false)
  const awaySummaryDisabled = ref(false)
  const useBuiltinTerminal = ref(false)
  const projectDropPathMode = ref<'filename' | 'relative'>('relative')
  const switchToTerminal = ref(false)
  const switchToProject = ref(false)
  const scope = ref<'user' | 'system'>('user')
  const statusMessage = ref('')
  const settingsSourcePath = ref('')
  const settingsSourceKind = ref<'settings' | 'legacy' | 'missing' | string>('missing')
  const settingsUsingLegacyPath = ref(false)
  const editingBaseline = ref(serializeDraft(editingConfig.value))

  // ── Computed ───────────────────────────────────────────────────────────────
  const visibleSessions = computed(() =>
    sessions.value.slice(0, sessionDisplayCount.value)
  )

  const hasMoreSessions = computed(() =>
    sessions.value.length > sessionDisplayCount.value
  )

  const isConfigDirty = computed(() => serializeDraft(editingConfig.value) !== editingBaseline.value)

  const activeProfileRef = computed<CliProfileRef | null>(() => activeConfigName.value
    && profileIds.value[activeConfigName.value]
    ? { cliKind: 'claude', profileId: profileIds.value[activeConfigName.value] }
    : null)

  function createProfileId(): string {
    return `profile-${crypto.randomUUID()}`
  }

  function serializeRecord(record: Record<string, string>): string {
    return JSON.stringify(Object.entries(record).sort(([a], [b]) => a.localeCompare(b)))
  }

  function serializeConfigs(record: Record<string, Record<string, string>>): string {
    return JSON.stringify(
      Object.entries(record)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([name, vars]) => [
          name,
          Object.entries(vars).sort(([a], [b]) => a.localeCompare(b)),
        ]),
    )
  }

  function serializeDraft(draft: { name: string; vars: Record<string, string> }): string {
    return JSON.stringify({
      name: draft.name,
      vars: Object.fromEntries(Object.entries(draft.vars).sort(([a], [b]) => a.localeCompare(b))),
    })
  }

  function markEditingClean() {
    editingBaseline.value = serializeDraft(editingConfig.value)
  }

  function commitSelectedConfig(name: string) {
    activeConfigName.value = name
    const vars = configs.value[name] ?? {}
    editingConfig.value = { name, vars: { ...vars } }
    activeSource.value = name
    markEditingClean()
  }

  function discardConfigChanges() {
    if (activeConfigName.value && configs.value[activeConfigName.value]) {
      commitSelectedConfig(activeConfigName.value)
      return
    }
    editingConfig.value = { name: '', vars: {} }
    activeSource.value = 'manual'
    markEditingClean()
  }

  async function confirmDiscardConfigChanges(action: string): Promise<boolean> {
    if (!isConfigDirty.value) return true
    const accepted = await confirm(
      `当前配置有未保存的修改。${action}将放弃这些修改，是否继续？`,
      { title: '未保存的配置', kind: 'warning' },
    )
    if (accepted) discardConfigChanges()
    return accepted
  }

  async function persistActiveProfileId(profileId: string | null) {
    try {
      await invoke('save_active_profile_id', { key: 'claude', profileId })
    } catch (error) {
      statusMessage.value = `配置已切换，但活动方案状态保存失败: ${error}`
    }
  }

  // ── Actions ────────────────────────────────────────────────────────────────
  async function loadConfigs() {
    try {
      const result = await invoke<Record<string, Record<string, string>>>('load_claude_configs')
      configs.value = result
      activeConfigName.value = null
      editingConfig.value = { name: '', vars: {} }
      activeSource.value = 'env'

      // Load saved order; fall back to Object.keys if none saved
      try {
        const savedOrder = await invoke<string[]>('load_config_order', { key: 'claude' })
        if (savedOrder && savedOrder.length > 0) {
          // Only keep names that actually exist in the loaded configs
          const validOrder = savedOrder.filter(n => n in result)
          // Append any new names not in the saved order
          const extra = Object.keys(result).filter(n => !validOrder.includes(n))
          configOrder.value = [...validOrder, ...extra]
        } else {
          configOrder.value = Object.keys(result)
        }
      } catch {
        configOrder.value = Object.keys(result)
      }

      let loadedProfileIds: Record<string, string> = {}
      try {
        loadedProfileIds = await invoke<Record<string, string>>('load_profile_ids', { key: 'claude' })
      } catch {
        // Existing named profiles are assigned stable IDs below.
      }
      const nextProfileIds = Object.fromEntries(
        Object.keys(result).map(name => [name, loadedProfileIds[name] || createProfileId()]),
      )
      profileIds.value = nextProfileIds

      // Load saved useBuiltinTerminal
      try {
        useBuiltinTerminal.value = await invoke<boolean>('load_use_builtin_terminal', { key: 'claude' })
      } catch {
        // keep default false
      }

      // Load saved project drop path mode
      try {
        const savedMode = await invoke<string>('load_project_drop_path_mode', { key: 'claude' })
        if (savedMode === 'filename' || savedMode === 'relative') {
          projectDropPathMode.value = savedMode
        }
      } catch {
        // keep default relative
      }

      // Restore the selected profile as a (cli_kind, profile_id) pair.
      let restoredSelection = false
      let savedActiveProfileId: string | null = null
      try {
        savedActiveProfileId = await invoke<string | null>('load_active_profile_id', { key: 'claude' })
        const selectedName = savedActiveProfileId
          ? Object.entries(profileIds.value).find(([, profileId]) => profileId === savedActiveProfileId)?.[0]
            // Compatibility with the short-lived name-as-ID Stage C development build.
            ?? (savedActiveProfileId in result ? savedActiveProfileId : null)
          : null
        if (selectedName) {
          commitSelectedConfig(selectedName)
          restoredSelection = true
        }
      } catch {
        // Environment matching below remains a compatibility fallback.
      }

      // Auto-select the config that matches the current process environment.
      const allVarNames = new Set<string>()
      for (const vars of Object.values(result)) {
        for (const k of Object.keys(vars)) allVarNames.add(k)
      }
      if (!restoredSelection && allVarNames.size > 0) {
        try {
          const currentEnv = await invoke<Record<string, string>>('get_current_env_vars', {
            varNames: Array.from(allVarNames),
          })
          for (const name of configOrder.value) {
            const vars = result[name]
            const matches = Object.entries(vars).every(([k, v]) => {
              if (!v) return true // skip empty values
              return currentEnv[k] === v
            })
            if (matches && Object.keys(vars).length > 0) {
              commitSelectedConfig(name)
              break
            }
          }
        } catch {
          // ignore — env var lookup is best-effort
        }
      }
      markEditingClean()
      const activeProfileId = activeProfileRef.value?.profileId ?? null
      if (serializeRecord(loadedProfileIds) !== serializeRecord(profileIds.value)
        || savedActiveProfileId !== activeProfileId) {
        await invoke('save_profile_index', {
          key: 'claude',
          order: configOrder.value,
          profileIds: profileIds.value,
          activeProfileId,
        })
      }
    } catch (e) {
      statusMessage.value = `加载配置失败: ${e}`
    }
  }

  async function selectConfig(name: string): Promise<boolean> {
    if (activeConfigName.value === name) return true
    if (!(await confirmDiscardConfigChanges('切换配置方案'))) return false
    commitSelectedConfig(name)
    await persistActiveProfileId(profileIds.value[name] ?? null)
    return true
  }

  async function newConfig(): Promise<boolean> {
    if (!(await confirmDiscardConfigChanges('新建配置方案'))) return false
    activeConfigName.value = null
    editingConfig.value = { name: '', vars: {} }
    activeSource.value = 'manual'
    markEditingClean()
    await persistActiveProfileId(null)
    return true
  }

  async function saveConfig(): Promise<boolean> {
    const name = editingConfig.value.name.trim()
    if (!name) {
      statusMessage.value = '请输入配置名称'
      return false
    }
    if (name in configs.value && activeConfigName.value !== name) {
      statusMessage.value = `配置名称 '${name}' 已存在`
      return false
    }

    const nextConfigs = Object.fromEntries(
      Object.entries(configs.value).map(([profileId, vars]) => [profileId, { ...vars }]),
    )
    const previousName = activeConfigName.value
    const nextVars = previousName ? { ...(nextConfigs[previousName] ?? {}) } : {}
    for (const key of KNOWN_ENV_KEYS) {
      const value = editingConfig.value.vars[key]
      if (value !== '' && value !== undefined && value !== null) nextVars[key] = value
      else delete nextVars[key]
    }
    if (previousName && previousName !== name) delete nextConfigs[previousName]
    nextConfigs[name] = nextVars

    const nextOrder = [...configOrder.value]
    if (previousName && previousName !== name) {
      const index = nextOrder.indexOf(previousName)
      if (index !== -1) nextOrder[index] = name
    }
    if (!nextOrder.includes(name)) nextOrder.push(name)
    const nextProfileIds = { ...profileIds.value }
    const profileId = previousName
      ? nextProfileIds[previousName] ?? createProfileId()
      : createProfileId()
    if (previousName && previousName !== name) delete nextProfileIds[previousName]
    nextProfileIds[name] = profileId

    try {
      await persistConfigs(nextConfigs, nextOrder, nextProfileIds, profileId)
      configs.value = nextConfigs
      configOrder.value = nextOrder
      profileIds.value = nextProfileIds
      commitSelectedConfig(name)
      statusMessage.value = `配置 '${name}' 已保存`
      return true
    } catch (error) {
      statusMessage.value = `保存配置失败，表单内容已保留: ${error}`
      return false
    }
  }

  async function deleteConfig(name: string) {
    const nextConfigs = Object.fromEntries(
      Object.entries(configs.value)
        .filter(([profileId]) => profileId !== name)
        .map(([profileId, vars]) => [profileId, { ...vars }]),
    )
    const nextOrder = configOrder.value.filter(profileId => profileId !== name)
    const nextProfileIds = { ...profileIds.value }
    delete nextProfileIds[name]
    const deletingActiveProfile = activeConfigName.value === name
    const nextActiveProfileId = deletingActiveProfile
      ? null
      : activeProfileRef.value?.profileId ?? null
    try {
      await persistConfigs(nextConfigs, nextOrder, nextProfileIds, nextActiveProfileId)
      configs.value = nextConfigs
      configOrder.value = nextOrder
      profileIds.value = nextProfileIds
      if (deletingActiveProfile) {
        activeConfigName.value = null
        editingConfig.value = { name: '', vars: {} }
        activeSource.value = 'env'
        markEditingClean()
      }
      statusMessage.value = `配置 '${name}' 已删除`
    } catch (error) {
      statusMessage.value = `删除配置失败: ${error}`
    }
  }

  async function reorderConfigs(newOrder: string[]) {
    try {
      await persistConfigs(
        configs.value,
        newOrder,
        profileIds.value,
        activeProfileRef.value?.profileId ?? null,
      )
      configOrder.value = [...newOrder]
    } catch (error) {
      statusMessage.value = `保存配置排序失败: ${error}`
    }
  }

  async function persistConfigs(
    nextConfigs: Record<string, Record<string, string>>,
    nextOrder: string[],
    nextProfileIds: Record<string, string>,
    nextActiveProfileId: string | null,
  ) {
    const previousConfigs = Object.fromEntries(
      Object.entries(configs.value).map(([name, vars]) => [name, { ...vars }]),
    )
    const previousOrder = [...configOrder.value]
    const previousProfileIds = { ...profileIds.value }
    const previousActiveProfileId = activeProfileRef.value?.profileId ?? null
    let configsWritten = false
    let profileIndexWritten = false

    const verifyPersistedState = async (
      expectedConfigs: Record<string, Record<string, string>>,
      expectedOrder: string[],
      expectedProfileIds: Record<string, string>,
      expectedActiveProfileId: string | null,
      phase: '保存' | '回滚',
    ) => {
      const [storedConfigs, storedOrder, storedProfileIds, storedActiveProfileId] = await Promise.all([
        invoke<Record<string, Record<string, string>>>('load_claude_configs'),
        invoke<string[]>('load_config_order', { key: 'claude' }),
        invoke<Record<string, string>>('load_profile_ids', { key: 'claude' }),
        invoke<string | null>('load_active_profile_id', { key: 'claude' }),
      ])
      const mismatches: string[] = []
      if (serializeConfigs(storedConfigs) !== serializeConfigs(expectedConfigs)) mismatches.push('配置方案')
      if (JSON.stringify(storedOrder) !== JSON.stringify(expectedOrder)) mismatches.push('方案顺序')
      if (serializeRecord(storedProfileIds) !== serializeRecord(expectedProfileIds)) mismatches.push('profile ID')
      if (storedActiveProfileId !== expectedActiveProfileId) mismatches.push('当前方案')
      if (mismatches.length > 0) {
        throw new Error(`${phase}后磁盘校验不一致：${mismatches.join('、')}`)
      }
    }

    try {
      await invoke('save_claude_configs', {
        configs: nextConfigs,
      })
      configsWritten = true
      await invoke('save_profile_index', {
        key: 'claude',
        order: nextOrder,
        profileIds: nextProfileIds,
        activeProfileId: nextActiveProfileId,
      })
      profileIndexWritten = true
      await verifyPersistedState(
        nextConfigs,
        nextOrder,
        nextProfileIds,
        nextActiveProfileId,
        '保存',
      )
    } catch (e) {
      if (configsWritten || profileIndexWritten) {
        try {
          await invoke('save_claude_configs', { configs: previousConfigs })
          await invoke('save_profile_index', {
            key: 'claude',
            order: previousOrder,
            profileIds: previousProfileIds,
            activeProfileId: previousActiveProfileId,
          })
          await verifyPersistedState(
            previousConfigs,
            previousOrder,
            previousProfileIds,
            previousActiveProfileId,
            '回滚',
          )
        } catch (rollbackError) {
          throw new Error(`保存失败，自动恢复旧数据也未通过校验：${rollbackError}；原始错误：${e}`)
        }
      }
      throw e
    }
  }

  // Persist useBuiltinTerminal whenever it changes
  watch(useBuiltinTerminal, async (val) => {
    try {
      await invoke('save_use_builtin_terminal', { key: 'claude', value: val })
    } catch {
      // ignore
    }
  })

  // Persist projectDropPathMode whenever it changes
  watch(projectDropPathMode, async (val) => {
    try {
      await invoke('save_project_drop_path_mode', { key: 'claude', value: val })
    } catch {
      // ignore
    }
  })

  async function applyToRegistry() {
    const vars = editingConfig.value.vars
    // Build a full map: for every known env key, use the config value or empty string.
    // Empty strings tell the backend to delete the registry entry.
    const fullVars: Record<string, string> = {}
    for (const k of KNOWN_ENV_KEYS) {
      fullVars[k] = vars[k] ?? ''
    }
    const nonEmpty = Object.entries(fullVars).filter(([, v]) => v)
    if (nonEmpty.length === 0) {
      statusMessage.value = '没有需要应用的环境变量'
      return
    }
    const scopeLabel = scope.value === 'system' ? '系统（所有用户）' : '当前用户'
    const confirmed = await confirm(
      `将以下 ${nonEmpty.length} 个环境变量写入注册表（范围: ${scopeLabel}）:\n\n` +
        formatRedactedEntries(Object.fromEntries(nonEmpty)).map(line => `  ${line}`).join('\n'),
      { title: '确认应用环境变量', kind: 'warning' }
    )
    if (!confirmed) return
    try {
      await invoke('apply_env_vars', {
        vars: fullVars,
        scope: scope.value,
      })
      statusMessage.value = `已应用到${scope.value === 'system' ? '系统' : '用户'}环境变量`
      // WM_SETTINGCHANGE broadcast can reset the DWM dark-mode attribute;
      // re-apply the current theme after a short delay.
      const currentTheme = document.documentElement.getAttribute('data-theme')
      if (currentTheme) {
        setTimeout(() => {
          invoke('set_titlebar_theme', { dark: currentTheme === 'dark' }).catch(() => {})
        }, 300)
      }
    } catch (e) {
      statusMessage.value = `应用失败: ${e}`
    }
  }

  async function fetchModels() {
    const baseUrl = editingConfig.value.vars['ANTHROPIC_BASE_URL'] ?? ''
    const authToken = editingConfig.value.vars['ANTHROPIC_AUTH_TOKEN'] ?? ''
    if (!baseUrl) {
      statusMessage.value = '请先输入 ANTHROPIC_BASE_URL'
      return
    }
    modelsFetching.value = true
    availableModels.value = []
    try {
      const models = await invoke<string[]>('fetch_claude_models', { baseUrl, authToken })
      availableModels.value = models
      statusMessage.value = `已获取 ${models.length} 个模型`
    } catch (e) {
      statusMessage.value = `获取模型失败: ${e}`
    } finally {
      modelsFetching.value = false
    }
  }

  async function loadSessions(options: { resetDisplayCount?: boolean } = {}) {
    const resetDisplayCount = options.resetDisplayCount ?? true
    try {
      const result = await invoke<SessionEntry[]>('load_claude_sessions', {
        targetDir: launchDir.value,
      })
      sessions.value = result ?? []
      if (resetDisplayCount) {
        sessionDisplayCount.value = 10
      } else {
        sessionDisplayCount.value = Math.max(10, Math.min(sessionDisplayCount.value, sessions.value.length || 10))
      }
    } catch (e) {
      sessions.value = []
    }
  }

  function loadMoreSessions() {
    sessionDisplayCount.value += 10
  }

  async function loadRecentProjects() {
    try {
      const result = await invoke<string[]>('load_claude_recent_projects')
      launchDirHistory.value = result ?? []
    } catch {
      launchDirHistory.value = []
    }
  }

  async function loadSettings() {
    try {
      const result = await invoke<ClaudeSettings>('load_claude_settings')
      skipPermissions.value = result.skipPermissions
      awaySummaryDisabled.value = result.awaySummaryDisabled
      settingsSourcePath.value = result.sourcePath ?? ''
      settingsSourceKind.value = result.sourceKind ?? 'settings'
      settingsUsingLegacyPath.value = result.usingLegacyPath ?? false
    } catch (error) {
      statusMessage.value = `加载 Claude Code settings.json 失败: ${error}`
    }
  }

  async function saveSettings() {
    try {
      await invoke('save_claude_settings', {
        settings: {
          skipPermissions: skipPermissions.value,
          awaySummaryDisabled: awaySummaryDisabled.value,
        }
      })
      settingsSourceKind.value = 'settings'
      settingsUsingLegacyPath.value = false
      settingsSourcePath.value = settingsSourcePath.value
        ? settingsSourcePath.value.replace(/(?:claude|config)\.json$/i, 'settings.json')
        : '~/.claude/settings.json'
    } catch (e) {
      statusMessage.value = `保存设置失败: ${e}`
    }
  }

  async function checkClaudeCode(): Promise<ClaudeCodeCheckResult> {
    try {
      const result = await invoke<ClaudeCodeCheckResult>('check_claude_code_installed')
      claudeExePath.value = result.installed ? result.path : null
      claudeInstalled.value = result.installed
      return result
    } catch (error) {
      claudeExePath.value = null
      claudeInstalled.value = false
      return {
        installed: false,
        path: null,
        version: null,
        message: `无法检查 Claude Code：${String(error)}`,
      }
    }
  }

  async function findClaudeExe() {
    const result = await checkClaudeCode()
    return result.installed ? result.path : null
  }

  async function launchClaude(sessionId?: string) {
    if (!claudeExePath.value) {
      statusMessage.value = 'Claude Code 未安装'
      return
    }

    const args: string[] = []
    if (skipPermissions.value) args.push('--dangerously-skip-permissions')
    if (sessionId) args.push('-r', sessionId)

    const envVars: Record<string, string> = {}
    if (editingConfig.value) {
      for (const [k, v] of Object.entries(editingConfig.value.vars)) {
        if (v) envVars[k] = v
      }
    }

    try {
      if (useBuiltinTerminal.value) {
        if (!launchDir.value.trim()) {
          statusMessage.value = '启动目录为空，请先设置启动目录'
          return
        }

        const configWorkspaceStore = useConfigWorkspaceStore()
        if (!(await configWorkspaceStore.confirmDiscardActiveChanges('启动内置 Claude Code 会话'))) {
          return
        }

        // Switch to Project before creating the xterm instance, so the
        // terminal container is visible when xterm measures itself.
        switchToProject.value = true
        await nextTick()

        const projectStore = useProjectStore()
        const session = await projectStore.launchClaudeFromConfig(sessionId)
        if (session) {
          statusMessage.value = 'Claude Code 已在项目终端启动'
        }
      } else {
        await invoke('launch_claude', {
          exe: claudeExePath.value,
          envVars,
          args,
          cwd: launchDir.value || null,
        })
        statusMessage.value = 'Claude Code 已启动'
      }
    } catch (e) {
      statusMessage.value = `启动失败: ${e}`
    }
  }

  async function loadLaunchDir() {
    try {
      const dir = await invoke<string>('load_launch_dir', { key: 'claude' })
      launchDir.value = dir ?? ''
    } catch {
      launchDir.value = ''
    }
  }

  async function saveLaunchDir() {
    try {
      await invoke('save_launch_dir', { key: 'claude', dir: launchDir.value })
    } catch {
      // ignore
    }
  }

  return {
    // state
    configs,
    configOrder,
    profileIds,
    activeConfigName,
    editingConfig,
    activeSource,
    sessions,
    sessionDisplayCount,
    availableModels,
    modelsFetching,
    launchDir,
    launchDirHistory,
    claudeExePath,
    claudeInstalled,
    skipPermissions,
    awaySummaryDisabled,
    useBuiltinTerminal,
    projectDropPathMode,
    switchToTerminal,
    switchToProject,
    scope,
    statusMessage,
    settingsSourcePath,
    settingsSourceKind,
    settingsUsingLegacyPath,
    // computed
    visibleSessions,
    hasMoreSessions,
    isConfigDirty,
    activeProfileRef,
    // actions
    loadConfigs,
    selectConfig,
    newConfig,
    saveConfig,
    deleteConfig,
    reorderConfigs,
    applyToRegistry,
    fetchModels,
    loadSessions,
    loadMoreSessions,
    loadRecentProjects,
    loadSettings,
    saveSettings,
    checkClaudeCode,
    findClaudeExe,
    launchClaude,
    loadLaunchDir,
    saveLaunchDir,
    discardConfigChanges,
    confirmDiscardConfigChanges,
  }
})
