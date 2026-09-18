import { computed, reactive, ref } from 'vue'
import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import type {
  DshModelProfile,
  DshProviderProfile,
  DshReasoningLevel,
  DshSettingsDocument,
  DshSettingsWriteResult,
  DshThinkingLevel,
} from '@/types/config'
import { DSH_THINKING_LEVELS, deriveDshCredentialRef } from '@/types/config'

/**
 * DeepSeek Harness 的「供应商与模型」编辑。
 *
 * 这一层管两个文件里启动器负责的部分；YAML 的解析与改写全在 Rust 侧
 * （`src-tauri/src/dsh_settings.rs`），前端拿到的是已经结构化的数据，回写时也只送
 * 结构化数据。两条边界：
 *
 * 1. **settings.yaml 只存引用名。** 令牌的**值**在 `$DSH_HOME/.credentials.yaml`
 *    的 `refs` 分节，由「认证令牌」栏经后端写入（对齐 dsh 自己的文件格式）；这条
 *    settings 侧的 `apiKeyEnv` 只是引用，从来不是密钥。引用名沿用文件里已有的，
 *    没有时按 dsh 的派生规则生成并自动补写。
 * 2. **只编辑声明，不改 dsh 不认识的字段。** 供应商下的 `compat`、`headers`、
 *    `retryPolicy` 等字段会被原样保留，界面上只标注它们的存在。
 *
 * dsh 仍在快速迭代：这份结构对齐 v0.1.5-rc.1，字段若变动，见
 * `src-tauri/src/dsh_settings.rs` 顶部注释里的排查路径。
 */

/** 界面上的供应商草稿：比落盘结构多一个「文件里原来的键」。 */
export interface DshProviderDraft extends DshProviderProfile {
  /** 文件里原来的路由键；`null` 表示尚未写入的新供应商。 */
  originalId: string | null
}

export interface DshModelsStatus {
  tone: 'info' | 'success' | 'warning' | 'error'
  message: string
}

function emptyModel(id: string): DshModelProfile {
  return {
    id,
    name: null,
    contextWindow: null,
    maxTokens: null,
    input: null,
    reasoningDisabled: false,
    reasoningEfforts: [],
    unmanagedFields: [],
  }
}

function emptyProvider(id: string): DshProviderDraft {
  return {
    id,
    displayName: null,
    api: null,
    baseUrl: null,
    apiKeyEnv: null,
    reasoning: null,
    models: [],
    unmanagedFields: [],
    // 新建的草稿一律是自定义路由：用户自己声明的，没有目录可依赖。
    custom: true,
    originalId: null,
  }
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T
}

/** 剥掉界面专用的字段，得到送给后端的结构。 */
function toPayload(draft: DshProviderDraft): DshProviderProfile {
  const { originalId: _originalId, ...rest } = draft
  return clone(rest)
}

/** 用于脏检查的快照键：只比较落盘结构，不含 `originalId`。 */
function fingerprint(draft: DshProviderDraft): string {
  return JSON.stringify(toPayload(draft))
}

export const useDshModelsStore = defineStore('dsh-models', () => {
  const document = ref<DshSettingsDocument | null>(null)
  const drafts = reactive<DshProviderDraft[]>([])
  /** 上一次读到的内容，用于判断哪些供应商有待写入的改动。 */
  const baseline = ref<Record<string, string>>({})
  const loading = ref(false)
  const loaded = ref(false)
  const saving = ref(false)
  const selectedId = ref<string | null>(null)
  const status = ref<DshModelsStatus | null>(null)

  // 认证令牌：值在 dsh 的凭据文件里，按供应商各存一份「已存的值」与「输入框草稿」。
  // 明文只进内存（与 Claude 配置页同一条路），输入框默认打码、点「显示」才亮。
  const credentialDrafts = reactive<Record<string, string>>({})
  const credentialStored = reactive<Record<string, string>>({})
  /** 读取失败的供应商记在这里：值未知时必须挡住保存，免得覆盖掉已有的令牌。 */
  const credentialErrors = reactive<Record<string, string>>({})
  const credentialSaving = ref(false)

  const providers = computed(() => drafts)
  /**
   * 左侧清单列出的供应商：只列**自定义**路由。
   *
   * dsh 自带的目录路由（例如只填了 `apiKeyEnv` 的 `kimi-coding`）的协议、端点与
   * 模型都由 dsh 自己的目录提供，在这里没有可编辑的内容，列出来只会让人以为漏了
   * 什么。判定由后端按已安装的目录算好（`custom`），前端不自己猜。
   */
  const visibleProviders = computed(() => drafts.filter((draft) => draft.custom))
  const selectedProvider = computed(
    () => drafts.find((item) => item.id === selectedId.value) ?? null,
  )
  const settingsPath = computed(() => document.value?.path ?? '')
  const supportedVersion = computed(() => document.value?.supportedVersion ?? '')

  /** 单个供应商是否有未写入的改动。新草稿一律算有。 */
  function isProviderDirty(draft: DshProviderDraft): boolean {
    if (draft.originalId === null) return true
    return baseline.value[draft.originalId] !== fingerprint(draft)
  }

  /** 当前选中项是否有未写入的改动。 */
  const isSelectedDirty = computed(() => {
    const draft = selectedProvider.value
    return draft ? isProviderDirty(draft) : false
  })

  const dirtyCount = computed(
    () => drafts.filter((draft) => isProviderDirty(draft)).length,
  )

  /** 文件中已存在的供应商键，用于查重。 */
  const takenIds = computed(
    () => new Set(
      drafts
        .map((draft) => draft.originalId)
        .filter((id): id is string => id !== null),
    ),
  )

  /** 模型 ID 候选：给「添加模型」的输入框做提示，取自文件里已经出现过的模型。 */
  const knownModelIds = computed(() => {
    const ids = new Set<string>()
    for (const draft of drafts) {
      for (const model of draft.models) {
        if (model.id) ids.add(model.id)
      }
    }
    return [...ids]
  })

  function setStatus(tone: DshModelsStatus['tone'], message: string) {
    status.value = { tone, message }
  }

  function clearStatus() {
    status.value = null
  }

  /** 让选中项落在清单里看得见的那一条上（目录路由被过滤后，选中项可能不可见）。 */
  function syncSelection() {
    if (selectedId.value && visibleProviders.value.some((item) => item.id === selectedId.value)) {
      return
    }
    selectedId.value = visibleProviders.value[0]?.id ?? null
  }

  function adopt(next: DshSettingsDocument) {
    document.value = next
    drafts.splice(0, drafts.length)
    const nextBaseline: Record<string, string> = {}
    for (const provider of next.providers) {
      const draft: DshProviderDraft = { ...clone(provider), originalId: provider.id }
      drafts.push(draft)
      nextBaseline[provider.id] = fingerprint(draft)
    }
    baseline.value = nextBaseline
    syncSelection()
  }

  /** 读取设置文件。`silent` 用于写回之后的静默刷新，不覆盖已显示的状态条。 */
  async function load(silent = false) {
    loading.value = true
    if (!silent) clearStatus()
    try {
      const next = await invoke<DshSettingsDocument>('dsh_read_settings')
      adopt(next)
      void refreshCredentials()
      // 读取成功不再提示：进页面就是一次 load，常亮的「已读取 N 个供应商」只是
      // 噪音（2026-09-18 用户要求删掉）；出错仍走 catch。「文件还不存在」例外，
      // 那是空状态指引——告诉用户写第一个供应商时会自动建文件。
      if (!silent && !next.exists) {
        setStatus('success', '尚未创建 dsh 设置文件；写入第一个供应商时会自动创建。')
      }
      return next
    } catch (error) {
      setStatus('error', `读取 dsh 设置失败：${error}`)
      return null
    } finally {
      loading.value = false
    }
  }

  function selectProvider(id: string) {
    selectedId.value = id
  }

  /**
   * 放弃尚未写入的改动：重新从磁盘读取一次。
   *
   * 供配置工作区的草稿守卫使用——切换 CLI 配置页或退出应用时，未写入的供应商
   * 改动会在这里被丢掉，而磁盘上的文件本来就没被动过。
   */
  async function discardChanges() {
    await load(true)
    setStatus('info', '已放弃尚未写入的供应商改动。')
  }

  /** 生成一个既没被文件占用、也没被草稿占用的路由键。 */
  function nextProviderId(): string {
    const used = new Set(drafts.map((draft) => draft.id))
    let index = drafts.length + 1
    while (used.has(`provider-${index}`)) index += 1
    return `provider-${index}`
  }

  function addProvider(): DshProviderDraft {
    const draft = emptyProvider(nextProviderId())
    drafts.push(draft)
    selectedId.value = draft.id
    setStatus('info', `已新增供应商草稿「${draft.id}」；填好字段后写入设置文件。`)
    return draft
  }

  /**
   * 删除供应商。文件里已存在的走后端删除；尚未写入的草稿只从界面移除。
   *
   * 密钥不在这个文件里，所以删除供应商不会、也不需要碰 `credentials.yaml`
   * ——这一点会在界面文案里说明，避免用户以为删了就等于撤了凭据。
   */
  async function removeProvider(draft: DshProviderDraft) {
    if (draft.originalId === null) {
      drafts.splice(drafts.indexOf(draft), 1)
      syncSelection()
      setStatus('info', '已丢弃尚未写入的草稿。')
      return true
    }
    const current = document.value
    if (!current) {
      setStatus('error', '设置文件尚未读取，无法删除。')
      return false
    }
    saving.value = true
    try {
      const result = await invoke<DshSettingsWriteResult>('dsh_delete_provider', {
        request: { baseRevision: current.revision, providerId: draft.originalId },
      })
      document.value = { ...current, revision: result.revision }
      await load(true)
      setStatus('success', `已从设置文件删除供应商「${draft.originalId}」。`)
      return true
    } catch (error) {
      setStatus('error', `删除失败：${error}`)
      return false
    } finally {
      saving.value = false
    }
  }

  /** 把当前选中的供应商写入设置文件（新增或更新）。 */
  async function writeSelectedProvider() {
    const draft = selectedProvider.value
    const current = document.value
    if (!draft) {
      setStatus('error', '请先在左侧选择一个供应商。')
      return false
    }
    if (!current) {
      setStatus('error', '设置文件尚未读取，无法写入。')
      return false
    }
    if (draft.originalId === null && takenIds.value.has(draft.id)) {
      setStatus('error', `供应商 ID「${draft.id}」已被占用，请换一个。`)
      return false
    }
    const duplicate = drafts.find(
      (item) => item !== draft && (item.id === draft.id || item.originalId === draft.id),
    )
    if (duplicate) {
      setStatus('error', `供应商 ID「${draft.id}」与列表中的另一个供应商重复。`)
      return false
    }

    saving.value = true
    try {
      const result = await invoke<DshSettingsWriteResult>('dsh_write_provider', {
        request: {
          baseRevision: current.revision,
          originalId: draft.originalId,
          provider: toPayload(draft),
        },
      })
      document.value = { ...current, revision: result.revision }
      await load(true)
      setStatus(
        'success',
        `已写入供应商「${draft.id}」。写入前的内容已备份为 ${result.backupPath}。`,
      )
      return true
    } catch (error) {
      setStatus('error', `写入失败：${error}`)
      return false
    } finally {
      saving.value = false
    }
  }

  // -- 认证令牌 -----------------------------------------------------------

  /** 令牌的凭据引用名：文件里已有 `apiKeyEnv` 就沿用，否则按 dsh 的规则从路由键派生。 */
  function credentialRefOf(draft: DshProviderDraft): string {
    return draft.apiKeyEnv?.trim() || deriveDshCredentialRef(draft.id)
  }

  function credentialDraftOf(draft: DshProviderDraft): string {
    return credentialDrafts[draft.id] ?? ''
  }

  function setCredentialDraft(draft: DshProviderDraft, value: string) {
    credentialDrafts[draft.id] = value
  }

  function credentialStoredOf(draft: DshProviderDraft): string {
    return credentialStored[draft.id] ?? ''
  }

  function credentialErrorOf(draft: DshProviderDraft): string {
    return credentialErrors[draft.id] ?? ''
  }

  function isCredentialDirty(draft: DshProviderDraft): boolean {
    return credentialDraftOf(draft) !== credentialStoredOf(draft)
  }

  /** 把每个可见供应商当前存的令牌读进来。输入框默认打码，明文只在内存里。 */
  async function refreshCredentials() {
    await Promise.all(
      visibleProviders.value.map(async (draft) => {
        try {
          const value = await invoke<string | null>('dsh_read_credential', {
            request: { refName: credentialRefOf(draft) },
          })
          credentialStored[draft.id] = value ?? ''
          credentialDrafts[draft.id] = value ?? ''
          delete credentialErrors[draft.id]
        } catch (error) {
          // 读不出来就当没有，但保存必须挡住——一次失败的读取不该诱导用户
          // 把已有的令牌覆盖掉。错误显示在令牌保存行上。
          credentialStored[draft.id] = ''
          credentialDrafts[draft.id] = ''
          credentialErrors[draft.id] = String(error)
        }
      }),
    )
  }

  /**
   * 保存（或移除）一个供应商的认证令牌。
   *
   * 值写入 `$DSH_HOME/.credentials.yaml` 的 refs，按引用名取用；settings.yaml 里
   * 只写引用名。引用名沿用文件里已有的 `apiKeyEnv`，没有时按 dsh 的派生规则生成
   * 并由后端补写——返回的 settingsRevision 用来同步修订号，免得下一次供应商写入
   * 撞上「文件被改过」的护栏。dsh 每次请求按引用名现读现用，保存即生效，无需重启。
   */
  async function saveCredential(draft: DshProviderDraft): Promise<boolean> {
    if (draft.originalId === null) {
      setStatus('error', '请先把这个供应商写入 settings.yaml，再保存令牌。')
      return false
    }
    if (!document.value) {
      setStatus('error', '设置文件尚未读取，无法保存令牌。')
      return false
    }
    credentialSaving.value = true
    try {
      const result = await invoke<{ refName: string; settingsRevision: string | null }>(
        'dsh_save_credential',
        { request: { providerId: draft.originalId, value: credentialDraftOf(draft) } },
      )
      if (result.settingsRevision && document.value) {
        // 后端只补了 apiKeyEnv 一行：同步修订号与这条草稿，其它草稿不受影响。
        document.value = { ...document.value, revision: result.settingsRevision }
        if (draft.apiKeyEnv?.trim() !== result.refName) {
          draft.apiKeyEnv = result.refName
          baseline.value = { ...baseline.value, [draft.originalId]: fingerprint(draft) }
        }
      }
      const removed = credentialDraftOf(draft).trim() === ''
      credentialStored[draft.id] = credentialDraftOf(draft)
      delete credentialErrors[draft.id]
      setStatus(
        'success',
        removed
          ? `已移除「${draft.id}」的认证令牌。`
          : `已保存「${draft.id}」的认证令牌（引用名 ${result.refName}），dsh 下一次请求就会用上它。`,
      )
      return true
    } catch (error) {
      setStatus('error', `保存令牌失败：${error}`)
      return false
    } finally {
      credentialSaving.value = false
    }
  }

  /**
   * 添加模型。返回是否成功——失败原因写在状态条里而不是抛异常，调用方据此
   * 决定要不要清空输入框。
   */
  function addModel(provider: DshProviderDraft, rawId: string): boolean {
    const id = rawId.trim()
    if (!id) {
      setStatus('error', '模型 ID 不能为空。')
      return false
    }
    if (provider.models.some((model) => model.id === id)) {
      setStatus('error', `供应商「${provider.id}」下已经有模型「${id}」了。`)
      return false
    }
    provider.models.push(emptyModel(id))
    setStatus('info', `已添加模型「${id}」；写入设置文件后生效。`)
    return true
  }

  function removeModel(provider: DshProviderDraft, modelId: string) {
    const index = provider.models.findIndex((model) => model.id === modelId)
    if (index < 0) return
    provider.models.splice(index, 1)
  }

  /** 勾选/取消一个思考档位。取消时保留其它档位的顺序。 */
  function toggleReasoningLevel(model: DshModelProfile, level: DshThinkingLevel) {
    const index = model.reasoningEfforts.findIndex((item) => item.level === level)
    if (index >= 0) {
      model.reasoningEfforts.splice(index, 1)
      return
    }
    model.reasoningEfforts.push({
      level,
      // 默认 wire 值就是档位名本身——大多数 OpenAI 兼容网关用的就是这套拼写。
      // 只有 off 例外：它的「什么都不发」本身就是一个有意义的声明。
      wire: level === 'off' ? null : level,
    })
    model.reasoningEfforts.sort(
      (left, right) =>
        DSH_THINKING_LEVELS.indexOf(left.level) - DSH_THINKING_LEVELS.indexOf(right.level),
    )
  }

  function setReasoningWire(level: DshReasoningLevel, wire: string) {
    level.wire = wire.trim() === '' ? null : wire
  }

  /**
   * 把左侧清单拖出来的新顺序写回列表。
   *
   * 清单里只有自定义路由，所以这里按它们**原来占的位置**逐一替换：目录路由留在
   * 原位，不会被视图顺序影响。顺序只是视图顺序——settings.yaml 里供应商的书写顺序
   * 不影响 dsh 的行为，因此这里不做任何写入，也不该让谁变成「待更新」。
   */
  function reorderVisible(order: string[]) {
    const byId = new Map(drafts.map((draft) => [draft.id, draft]))
    const slots = drafts
      .map((draft, index) => (draft.custom ? index : -1))
      .filter((index) => index >= 0)
    const next = [...drafts]
    slots.forEach((slot, position) => {
      const draft = byId.get(order[position])
      if (draft) next[slot] = draft
    })
    drafts.splice(0, drafts.length, ...next)
  }

  return {
    document,
    providers,
    visibleProviders,
    selectedProvider,
    selectedId,
    loading,
    loaded,
    saving,
    status,
    settingsPath,
    supportedVersion,
    isSelectedDirty,
    isProviderDirty,
    dirtyCount,
    knownModelIds,
    load,
    selectProvider,
    discardChanges,
    addProvider,
    removeProvider,
    writeSelectedProvider,
    credentialRefOf,
    credentialDraftOf,
    setCredentialDraft,
    credentialStoredOf,
    credentialErrorOf,
    isCredentialDirty,
    credentialSaving,
    saveCredential,
    addModel,
    removeModel,
    toggleReasoningLevel,
    setReasoningWire,
    reorderVisible,
    setStatus,
    clearStatus,
  }
})
