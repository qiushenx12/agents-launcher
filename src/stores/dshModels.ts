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

function emptyModel(id: string): DshModelProfile {  return {
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
  /**
   * 当前选中的草稿**对象**，不是 id。
   *
   * 供应商的 `id` 就是 dsh 的路由键（settings.yaml 里的 `providers.<id>`），
   * 用户随时能改它，也可能与另一条已存在的供应商撞车。用字符串 id 记选中时，
   * `syncSelection` 会在撞车的情况下误判"选中没丢"（因为 id 还在列表里），
   * 于是把编辑器切到同名的另一条上去。用对象身份引用，选中就跟人走，
   * 不跟名字走（2026-09-18 任务 202609181729240000）。
   */
  const selectedDraft = ref<DshProviderDraft | null>(null)
  const status = ref<DshModelsStatus | null>(null)

  // 认证令牌：值在 dsh 的凭据文件里，按供应商各存一份「已存的值」与「输入框草稿」。
  // 明文只进内存（与 Claude 配置页同一条路），输入框默认打码、点「显示」才亮。
  //
  // 索引键是**草稿对象**而不是供应商 id（2026-09-18，任务：改 id 令牌消失）：
  // 供应商 id 用户随时能改，用字符串索引时改名的瞬间新 id 下什么都没有，输入框
  // 立刻变空白。WeakMap 跟对象走——改名只是改 draft.id 的值，凭据状态原地不动，
  // 界面怎么改 id 都不受影响；真正动凭据引用名的时机是「写入当前修改」（见
  //  writeSelectedProvider）。
  const credentialDrafts = new WeakMap<DshProviderDraft, string>()
  const credentialStored = new WeakMap<DshProviderDraft, string>()
  /** 读取失败的供应商记在这里：值未知时必须挡住保存，免得覆盖掉已有的令牌。 */
  const credentialErrors = new WeakMap<DshProviderDraft, string>()
  /**
   * 凭据状态的响应式版本号：WeakMap 本身不是响应式的，每次写入后自增它，
   * 读访问器把它作为依赖，界面才能跟着 refreshCredentials 落地重算。
   * （2026-09-18 实证：写入成功后 adopt 重建草稿 → computed 重算读到空；
   * 随后 refresh 把值写进裸 WeakMap，不触发任何更新，输入框钉死在空，
   * 重启后因为首次求值晚于 refresh 落地而"恢复正常"。）
   */
  const credentialVersion = ref(0)
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
  const selectedProvider = computed(() => selectedDraft.value)
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

  /**
   * 操作反馈的飘字：success / info 走这里，3 秒消失（2026-09-18 任务
   * 202609181728080000——「已写入供应商 …」这类提示不该常驻顶部）。`seq` 逐次
   * 递增，面板 watch 它重置计时器；连发两条相同文案也会重新计满 3 秒。
   *
   * warning / error 仍走 `status` 的顶部横幅常驻——它们要用户处理（校验拦截、
   * 读写失败、令牌保存失败但供应商已生效），不该 3 秒飘走。
   */
  const toast = ref<string | null>(null)
  const toastSeq = ref(0)

  function showToast(message: string) {
    toast.value = message
    toastSeq.value += 1
  }

  function setStatus(tone: DshModelsStatus['tone'], message: string) {
    if (tone === 'success' || tone === 'info') {
      // 成功/信息进了飘字通道，横幅不再显示它们；若之前留着一条横幅（比如上一条
      // 错误），新操作成功了就该把它撤掉。
      status.value = null
      showToast(message)
      return
    }
    status.value = { tone, message }
  }

  function clearStatus() {
    status.value = null
  }

  /** 让选中项落在清单里看得见的那一条上（目录路由被过滤后，选中项可能不可见）。 */
  function syncSelection() {
    const current = selectedDraft.value
    if (current && visibleProviders.value.includes(current)) {
      return
    }
    // adopt 重建了 drafts：对象引用全换，按路由键找回同一条，选中不丢。
    // 候选键有两个：**写入文件的路由键**（current.id——改名时新文档挂在新键下）
    // 与**写入前的路由键**（current.originalId——改过名但没写入就重读时，文件恢复
    // 的还是旧键）。只按旧键找，改名写入后会落空，选中跳到另一条供应商上，编辑器
    // 里显示的令牌就成了别人的（或空白）。
    if (current) {
      const candidates = new Set<string>([current.id])
      if (current.originalId !== null) candidates.add(current.originalId)
      selectedDraft.value = drafts.find(
        (item) => item.originalId !== null && candidates.has(item.originalId),
      )
        ?? visibleProviders.value[0]
        ?? null
      return
    }
    selectedDraft.value = visibleProviders.value[0] ?? null
  }

  function adopt(next: DshSettingsDocument) {
    document.value = next
    // 重建前先把旧草稿上**未保存的**凭据状态按路由键抢出来：drafts 马上换成全新
    // 对象，WeakMap 里旧对象上的值全部不可达。脏草稿（用户输入了还没保存的令牌）
    // 与读取错误继承到新对象上；干净的草稿与 stored 不继承——refreshCredentials
    // 紧跟着会把它们从凭据文件刷成最新（2026-09-18 修复：写入配置后令牌变空，
    // 就是重建后新对象在 WeakMap 里什么都没有、refresh 又把空判成「脏」不覆盖）。
    const carried = new Map<string, { draft?: string; error?: string }>()
    for (const old of drafts) {
      // 键必须是**写进文件的路由键**（old.id）：改名的草稿 originalId 还是旧键，
      // 而重建后的文档里那条供应商挂在**新键**下——按旧键继承会把未保存的凭据
      // 状态挂到一个不存在的键上，重建后原地蒸发（改名 + 改令牌一起保存时令牌丢失）。
      const key = old.id
      const dirtyDraft = credentialDrafts.get(old)
      if (dirtyDraft !== undefined && dirtyDraft !== credentialStored.get(old)) {
        carried.set(key, { ...carried.get(key), draft: dirtyDraft })
      }
      const error = credentialErrors.get(old)
      if (error !== undefined) {
        carried.set(key, { ...carried.get(key), error })
      }
    }
    drafts.splice(0, drafts.length)
    const nextBaseline: Record<string, string> = {}
    for (const provider of next.providers) {
      const draft: DshProviderDraft = { ...clone(provider), originalId: provider.id }
      drafts.push(draft)
      nextBaseline[provider.id] = fingerprint(draft)
      const state = carried.get(provider.id)
      // **WeakMap 的键必须是数组里的那个对象**：drafts 是 reactive 数组，push 进去
      // 的元素会被 Vue 包成响应式代理，界面与 refreshCredentials 拿到的都是代理，
      // 而 WeakMap 按对象引用区分——在 push 之前的原始对象上 set，代理上 get 永远
      // 读不到。结果就是「写入后 load 重建」把未保存的令牌草稿挂丢，随后的
      // refresh 把它判成「没读过」，用文件旧值覆盖，用户刚输入的 api key 被静默
      // 丢弃、跟随保存也不触发（2026-09-20「保存后 api key 被清空」的根因）。
      const storedDraft = drafts[drafts.length - 1]
      if (state?.draft !== undefined) credentialDrafts.set(storedDraft, state.draft)
      if (state?.error !== undefined) credentialErrors.set(storedDraft, state.error)
    }
    baseline.value = nextBaseline
    // 继承写入完成，通知凭据的读取方重算（WeakMap 写入本身非响应式）。
    credentialVersion.value++
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
      // 那是空状态指引——告诉用户写第一个供应商时会自动建文件。空态要常驻，所以
      // 直接写 status，不走 setStatus 的飘字分流。
      if (!silent && !next.exists) {
        status.value = {
          tone: 'success',
          message: '尚未创建 dsh 设置文件；写入第一个供应商时会自动创建。',
        }
      }
      return next
    } catch (error) {
      setStatus('error', `读取 dsh 设置失败：${error}`)
      return null
    } finally {
      loading.value = false
    }
  }

  function selectProvider(draft: DshProviderDraft) {
    selectedDraft.value = draft
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
    selectedDraft.value = draft
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

  /**
   * 把当前选中的供应商写入设置文件（新增或更新）。
   *
   * 顺带保存令牌（2026-09-18，用户要求合并两个按钮）：写入成功后，若「认证令牌」
   * 栏有脏草稿就自动跟一次 `dsh_save_credential`——供应商此时已存在于文件里，
   * 引用名补写的顺序依赖天然满足。两个例外留在独立的「保存令牌」按钮上：
   * **清空 = 移除**（破坏性动作要显式确认）与**读取失败**（值未知，不能拿草稿
   * 盖掉可能存在的令牌）。令牌保存失败不回滚供应商写入——两个文件、两条命令，
   * 状态条分开说明结果。
   *
   * 改了路由键的情形（2026-09-18，任务：改 id 令牌消失）：凭据的引用名在写入前
   * 从旧键换到新键（见下面 try 块开头的注释），settings.yaml 侧不为此改任何
   * 字节——id 不在 MANAGED_PROVIDER_KEYS 里，重建块时块内字段逐字节保留。
   */
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
      // 改了路由键时，先把凭据文件里的引用名从旧键换到新键（值原样保留）。
      //
      // 只在三个条件同时成立时才动：
      //   1. 真的改了 id（originalId 与 id 不同）；
      //   2. 引用名是派生的（settings.yaml 没写 apiKeyEnv）——手写引用名的供应商
      //      引用名跟 id 无关，settings 侧重建块时整行保留，凭据文件完全不用动；
      //   3. 旧引用名下确实存着值——没有值就没有可换的东西。
      // settings.yaml 侧不用为这个改任何字节：id 不在 MANAGED_PROVIDER_KEYS 里，
      // 删旧块重建新块时块内字段（含 apiKeyEnv）逐字节保留，是纯 id 改名时连
      // settings 文件都内容不变。
      if (
        draft.originalId !== null
        && draft.originalId !== draft.id
        && !draft.apiKeyEnv?.trim()
      ) {
        const oldRef = deriveDshCredentialRef(draft.originalId)
        const newRef = deriveDshCredentialRef(draft.id)
        if (oldRef !== newRef) {
          try {
            const existing = await invoke<string | null>('dsh_read_credential', {
              request: { refName: oldRef },
            })
            if (existing !== null && existing !== '') {
              await invoke('dsh_rename_credential_ref', {
                request: { oldRef, newRef },
              })
            }
          } catch (error) {
            setStatus(
              'error',
              `写入已取消：迁移认证令牌的引用名失败（${error}）。`
                + '供应商与凭据文件均未改动，请重试；若持续失败请检查 dsh 凭据文件。',
            )
            return false
          }
        }
      }
      const result = await invoke<DshSettingsWriteResult>('dsh_write_provider', {
        request: {
          baseRevision: current.revision,
          originalId: draft.originalId,
          provider: toPayload(draft),
        },
      })
      document.value = { ...current, revision: result.revision }
      // 先等凭据状态刷新到最新，再决定要不要跟随保存令牌——load 里 refreshCredentials
      // 是异步的，直接读 credentialStored/credentialErrors 会拿到上一轮的陈旧值。
      await load(true)
      await refreshCredentials()
      const baseMessage = `已写入供应商「${draft.id}」。写入前的内容已备份为 ${result.backupPath}。`

      // load 重建了 drafts，找回同一条草稿再判断令牌。按**写入文件的路由键**
      // （draft.id）匹配：这次写入就是按它落盘的，新草稿的 originalId 即它。
      // 不能按 draft.originalId 找——改名时那是旧键，新文档里没有挂旧键的草稿，
      // 会找到 undefined 导致令牌跟随保存被整个跳过（改名 + 改令牌一起保存时丢令牌）。
      const saved = drafts.find((item) => item.originalId === draft.id)
      if (
        saved
        && isCredentialDirty(saved)
        && credentialDraftOf(saved).trim() !== ''
        && !credentialErrorOf(saved)
      ) {
        if (await saveCredential(saved, true)) {
          setStatus('success', `${baseMessage}认证令牌也已保存。`)
        } else {
          setStatus(
            'warning',
            `${baseMessage}但认证令牌保存失败，供应商修改已生效；请重试「保存令牌」。`,
          )
        }
      } else {
        setStatus('success', baseMessage)
      }
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
    // 依赖 credentialVersion：WeakMap 写入非响应式，靠版本号通知界面重算。
    void credentialVersion.value
    return credentialDrafts.get(draft) ?? ''
  }

  function setCredentialDraft(draft: DshProviderDraft, value: string) {
    credentialDrafts.set(draft, value)
    credentialVersion.value++
  }

  function credentialStoredOf(draft: DshProviderDraft): string {
    void credentialVersion.value
    return credentialStored.get(draft) ?? ''
  }

  function credentialErrorOf(draft: DshProviderDraft): string {
    void credentialVersion.value
    return credentialErrors.get(draft) ?? ''
  }

  function isCredentialDirty(draft: DshProviderDraft): boolean {
    return credentialDraftOf(draft) !== credentialStoredOf(draft)
  }

  /**
   * 把每个可见供应商当前存的令牌读进来。输入框默认打码，明文只在内存里。
   *
   * 已改但尚未保存的草稿**不刷新**：「写入当前修改」成功后这里会静默重读一次，
   * 而凭据文件要等「保存令牌」才动——若在这时把草稿盖回文件里的旧值，用户刚输入
   * 的令牌就被静默吞了（2026-09-18 任务 202609181731590000：令牌在写入时丢失）。
   * 干净的草稿刷成文件值是无害的（覆盖的是同一个值），且能让外部改动及时出现。
   *
   * ⚠️ 脏判定必须先于 `credentialStored` 的更新：拿刚读到的新值去比，初始为空的
   * 草稿会永远被误判为脏，于是重启后令牌永远填不进输入框（2026-09-18 实测
   * 「重启后看不到令牌」的根因）。
   */
  async function refreshCredentials() {
    await Promise.all(
      visibleProviders.value.map(async (draft) => {
        try {
          const refName = credentialRefOf(draft)
          const value = await invoke<string | null>('dsh_read_credential', {
            request: { refName },
          })
          const stored = value ?? ''
          // 「没读过的草稿不判脏」：WeakMap 里无记录（undefined）说明界面还没展示过
          // 任何值，拿 undefined 与 '' 比会误判成脏、把文件里的值挡在输入框外
          // （2026-09-18「写入配置后令牌变空」的第二道断点）。只有用户真的改过
          // （有记录且与 stored 不同）才保留草稿。
          const draftValue = credentialDrafts.get(draft)
          const dirty = draftValue !== undefined && draftValue !== credentialStored.get(draft)
          credentialStored.set(draft, stored)
          if (!dirty) credentialDrafts.set(draft, stored)
          credentialErrors.delete(draft)
        } catch (error) {
          // 读不出来就当没有，但保存必须挡住——一次失败的读取不该诱导用户
          // 把已有的令牌覆盖掉。错误显示在令牌保存行上。同样的，脏草稿不清空：
          // 值未知时把用户输入的草稿抹掉只会雪上加霜。
          const draftValue = credentialDrafts.get(draft)
          const dirty = draftValue !== undefined && draftValue !== credentialStored.get(draft)
          credentialStored.set(draft, '')
          if (!dirty) credentialDrafts.set(draft, '')
          credentialErrors.set(draft, String(error))
        }
      }),
    )
    // 凭据落盘后统一自增版本号：WeakMap 写入本身不触发响应式更新，界面靠
    // 版本号重算（2026-09-18 实证「保存后令牌钉死在空」的根因）。
    credentialVersion.value++
  }

  /**
   * 保存（或移除）一个供应商的认证令牌。
   *
   * 值写入 `$DSH_HOME/.credentials.yaml` 的 refs，按引用名取用；settings.yaml 里
   * 只写引用名。引用名沿用文件里已有的 `apiKeyEnv`，没有时按 dsh 的派生规则生成
   * 并由后端补写——返回的 settingsRevision 用来同步修订号，免得下一次供应商写入
   * 撞上「文件被改过」的护栏。dsh 每次请求按引用名现读现用，保存即生效，无需重启。
   *
   * `quiet` 给「写入当前修改」的自动跟随保存用：状态条由调用方组合成一条消息，
   * 这里不写，免得先闪一条「已保存令牌」又被盖掉。
   */
  async function saveCredential(draft: DshProviderDraft, quiet = false): Promise<boolean> {
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
      credentialStored.set(draft, credentialDraftOf(draft))
      credentialErrors.delete(draft)
      credentialVersion.value++
      if (!quiet) {
        setStatus(
          'success',
          removed
            ? `已移除「${draft.id}」的认证令牌。`
            : `已保存「${draft.id}」的认证令牌（引用名 ${result.refName}），dsh 下一次请求就会用上它。`,
        )
      }
      return true
    } catch (error) {
      if (!quiet) setStatus('error', `保存令牌失败：${error}`)
      return false
    } finally {
      credentialSaving.value = false
    }
  }

  // -- 网关模型清单 -----------------------------------------------------------

  /**
   * 「获取模型」拉到的网关模型 ID，按供应商各存一份，只进内存——它是给「添加
   * 模型」的下拉做候选的瞬时数据，不写进 settings.yaml，也不参与脏检查。
   * 与凭据状态同理由按草稿对象索引：改 id 不该把已拉到的清单弄丢。版本号同理
   * （WeakMap 写入非响应式），与凭据共用同一个 credentialVersion 通知读取方。
   */
  const availableModels = new WeakMap<DshProviderDraft, string[]>()
  const modelsFetchingId = ref<string | null>(null)

  function availableModelsOf(provider: DshProviderDraft): string[] {
    void credentialVersion.value
    return availableModels.get(provider) ?? []
  }

  /**
   * 从供应商的 API 地址拉取可用模型清单，与 Claude 配置页同一条后端链路
   * （OpenAI / Anthropic / Gemini 候选端点逐个试）。令牌用「认证令牌」栏当前
   * 的值——那一栏已初始化为凭据文件里存的值，所以未保存的修改也能立刻生效。
   */
  async function fetchModels(provider: DshProviderDraft): Promise<boolean> {
    if (modelsFetchingId.value) return false
    const baseUrl = provider.baseUrl?.trim() ?? ''
    if (!baseUrl) {
      setStatus('error', '请先填写 API 地址，再获取模型。')
      return false
    }
    modelsFetchingId.value = provider.id
    try {
      const models = await invoke<string[]>('fetch_dsh_models', {
        baseUrl,
        authToken: credentialDraftOf(provider).trim(),
      })
      availableModels.set(provider, models)
      credentialVersion.value++
      setStatus('success', `已从网关获取 ${models.length} 个模型；在「添加模型」右侧的下拉里选择。`)
      return true
    } catch (error) {
      setStatus('error', `获取模型失败：${error}`)
      return false
    } finally {
      modelsFetchingId.value = null
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
    loading,
    loaded,
    saving,
    status,
    settingsPath,
    supportedVersion,
    toast,
    toastSeq,
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
    availableModelsOf,
    modelsFetchingId,
    fetchModels,
    addModel,
    removeModel,
    toggleReasoningLevel,
    setReasoningWire,
    reorderVisible,
    setStatus,
    clearStatus,
  }
})
