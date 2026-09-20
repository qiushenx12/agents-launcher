/**
 * 回归：dsh 配置页「保存后 api key 被清空 / 令牌丢失」。
 *
 * 用**真实的 store 代码**（src/stores/dshModels.ts）+ 模拟后端（复刻
 * dsh_settings.rs 的对外契约），数据形状取本机的真实文件：
 * settings.yaml 的 vllm（自定义路由，apiKeyEnv: VLLM_API_KEY）与
 * kimi-coding（目录路由，custom=false，左侧不显示），.credentials.yaml 的
 * VLLM_API_KEY: sk（本地 vLLM 占位密钥）。
 *
 * 根因（2026-09-20）：凭据状态按草稿对象存进 WeakMap，但 drafts 是 reactive
 * 数组——adopt() 重建时把未保存的令牌草稿 set 在**原始对象**上，而界面与
 * refreshCredentials 拿到的都是 Vue 的**响应式代理**；WeakMap 按引用区分，
 * 代理上 get 永远读不到原始对象上的条目。于是「保存 → load 重建」后脏草稿
 * 被 refresh 判成「没读过」并用文件旧值覆盖，跟随保存不触发，用户刚输入的
 * api key 被静默丢弃（字段回落旧值，原本没有令牌时就是「清空」）。
 * 同族的改名缺陷：继承与跟随保存按旧路由键（originalId）查找，改名写入后
 * 落空。
 *
 * 运行方式（需要 Node 22.15+ 的 --experimental-test-module-mocks 与
 * node:module 的 registerHooks）：
 *
 *   node --experimental-test-module-mocks --test tests/dshSaveTokenFlow.test.ts
 */
import assert from 'node:assert/strict'
import test from 'node:test'
import { mock } from 'node:test'
import { registerHooks } from 'node:module'

// `@/` 是 vite 的 src 别名，node 测试里手动解析。
const SRC = 'file:///D:/Project/agents-launcher/src/'
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier.startsWith('@/')) {
      const rel = specifier.slice(2)
      const withExt = /\.[mc]?[jt]s$/.test(rel) ? rel : `${rel}.ts`
      return nextResolve(`${SRC}${withExt}`, context)
    }
    return nextResolve(specifier, context)
  },
})

// ---------------------------------------------------------------------------
// 模拟后端：复刻 dsh_settings.rs 命令的对外契约（结构化数据层面）。
// ---------------------------------------------------------------------------

type ProviderState = {
  id: string
  displayName: string | null
  api: string | null
  baseUrl: string | null
  apiKeyEnv: string | null
  reasoning: string | null
  models: unknown[]
  unmanagedFields: string[]
  custom: boolean
}

function deepClone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T
}

function deriveRef(routeKey: string): string {
  return `${routeKey.toUpperCase().replace(/[^A-Z0-9]+/g, '_')}_API_KEY`
}

function makeBackend() {
  const providers: Record<string, ProviderState> = {
    'kimi-coding': {
      id: 'kimi-coding',
      displayName: null,
      api: null,
      baseUrl: null,
      apiKeyEnv: 'KIMI_CODING_API_KEY',
      reasoning: null,
      models: [],
      unmanagedFields: [],
      custom: false, // 目录路由：左侧不显示
    },
    vllm: {
      id: 'vllm',
      displayName: null,
      api: 'openai-completions',
      baseUrl: 'http://localhost:8080/v1',
      apiKeyEnv: 'VLLM_API_KEY',
      reasoning: null,
      models: [
        {
          id: 'Qwen3.8-27B',
          name: 'Qwen3.8-27B',
          contextWindow: 262144,
          maxTokens: 65536,
          input: ['text', 'image'],
          reasoningDisabled: false,
          reasoningEfforts: [
            { level: 'high', wire: 'high' },
            { level: 'xhigh', wire: 'xhigh' },
            { level: 'max', wire: 'max' },
          ],
          unmanagedFields: [],
        },
      ],
      unmanagedFields: [],
      custom: true,
    },
  }
  const creds: Record<string, string> = {
    DEEPSEEK_API_KEY: 'sk-66655bd3977c4d0a8324a06d9e0bd8',
    KIMI_CODING_API_KEY: 'sk-kimi-ZQMSlDpXW4rhY6dIJGFVI8ADEMw8ijw1uWjlwL26n9875AS8fadWtOGFQ7SE2EJ1',
    VLLM_API_KEY: 'sk',
  }
  let revision = 'rev-0'

  function nextRevision() {
    revision = `rev-${Math.random().toString(36).slice(2, 10)}`
    return revision
  }

  async function invoke(command: string, args?: Record<string, unknown>): Promise<unknown> {
    switch (command) {
      case 'dsh_read_settings': {
        return {
          path: 'C:\\Users\\30919\\.dsh\\settings.yaml',
          exists: true,
          revision,
          supportedVersion: '0.1.5-rc.1',
          providers: Object.values(providers).map((p) => deepClone(p)),
        }
      }
      case 'dsh_write_provider': {
        const request = args?.request as {
          baseRevision: string
          originalId: string | null
          provider: ProviderState
        }
        if (request.baseRevision !== revision) {
          throw new Error('dsh 设置文件在读取之后被其它程序改过了（模拟后端）。')
        }
        if (request.originalId && request.originalId !== request.provider.id) {
          delete providers[request.originalId]
        }
        providers[request.provider.id] = deepClone(request.provider)
        nextRevision()
        return {
          path: 'C:\\Users\\30919\\.dsh\\settings.yaml',
          revision,
          backupPath: 'C:\\Users\\30919\\.dsh\\settings.yaml.bak',
        }
      }
      case 'dsh_read_credential': {
        const request = args?.request as { refName: string }
        const value = creds[request.refName]
        return value === undefined ? null : value
      }
      case 'dsh_save_credential': {
        const request = args?.request as { providerId: string; value: string }
        const value = request.value.trim()
        const provider = providers[request.providerId]
        if (!provider) {
          throw new Error(`settings.yaml 里还没有供应方 ${request.providerId}（模拟后端）。`)
        }
        const existing = provider.apiKeyEnv?.trim() || null
        const refName = existing || deriveRef(request.providerId)
        if (value === '') {
          delete creds[refName]
        } else {
          creds[refName] = value
        }
        let settingsRevision: string | null = null
        if (!existing) {
          provider.apiKeyEnv = refName
          settingsRevision = nextRevision()
        }
        return { refName, settingsRevision }
      }
      case 'dsh_rename_credential_ref': {
        const request = args?.request as { oldRef: string; newRef: string }
        if (creds[request.oldRef] !== undefined) {
          creds[request.newRef] = creds[request.oldRef]
          delete creds[request.oldRef]
        }
        return null
      }
      default:
        throw new Error(`模拟后端不认识的命令：${command}`)
    }
  }

  return { invoke, providers, creds }
}

// ---------------------------------------------------------------------------
// 场景工具
// ---------------------------------------------------------------------------

// 只有一个 mock 实例：store 模块只会导入一次（模块缓存），live binding 指向这里，
// 每个场景通过 current 切换后端状态。
let current: ReturnType<typeof makeBackend> | null = null
mock.module('@tauri-apps/api/core', {
  exports: {
    invoke: (command: string, args?: Record<string, unknown>) => {
      if (!current) throw new Error('current 后端未设置')
      return current.invoke(command, args)
    },
  },
})

const flush = () => new Promise<void>((resolve) => { setTimeout(resolve, 5) })

type Store = {
  load: () => Promise<unknown>
  visibleProviders: Array<{ id: string; originalId: string | null }>
  selectedProvider: { id: string } | null
  selectProvider: (draft: unknown) => void
  writeSelectedProvider: () => Promise<boolean>
  setCredentialDraft: (draft: unknown, value: string) => void
  credentialDraftOf: (draft: unknown) => string
  credentialStoredOf: (draft: unknown) => string
  credentialErrorOf: (draft: unknown) => string
}

async function freshStore(backend: ReturnType<typeof makeBackend>): Promise<Store> {
  // 每个场景独立的 pinia，store 状态从零开始。
  current = backend
  const { createPinia, setActivePinia } = await import('pinia')
  setActivePinia(createPinia())
  const mod = await import('@/stores/dshModels')
  return mod.useDshModelsStore() as unknown as Store
}

async function openVllm(store: Store) {
  await store.load()
  await flush()
  const vllm = store.visibleProviders.find((d) => d.id === 'vllm')
  assert.ok(vllm, 'vllm 应在左侧清单里')
  store.selectProvider(vllm)
  await flush()
  assert.equal(store.credentialDraftOf(vllm), 'sk', '加载后输入框应显示已存的 sk')
  return vllm
}

// ---------------------------------------------------------------------------
// 场景
// ---------------------------------------------------------------------------

test('改默认思考档后保存（用户最近一次保存的现场）：令牌原样保留', async () => {
  const backend = makeBackend()
  const store = await freshStore(backend)
  const vllm = await openVllm(store)

  // 用户改了「默认思考档」= xhigh（真实文件的 diff 正是这一行）。
  ;(vllm as { reasoning: string | null }).reasoning = 'xhigh'

  assert.ok(await store.writeSelectedProvider(), '保存应当成功')

  // 保存后 adopt 重建了 drafts：字段断言必须重新从 selectedProvider 拿**当前**
  // 的草稿对象——界面令牌字段永远绑在 store.selectedProvider 上，从不持有保存前
  // 的旧引用（旧引用上的凭据状态在重建后不再更新，读它测的是幻影不是界面）。
  const after = store.selectedProvider!
  assert.equal(after.id, 'vllm', '选中必须仍落在 vllm 上')
  assert.equal(store.credentialDraftOf(after), 'sk', '字段必须仍显示已存的令牌')
  assert.equal(store.credentialStoredOf(after), 'sk')
  assert.equal(backend.creds.VLLM_API_KEY, 'sk', '凭据文件必须原样')
  assert.equal(backend.providers.vllm?.reasoning, 'xhigh', '供应商修改要落盘')
})

test('无任何修改直接点保存：令牌原样保留', async () => {
  const backend = makeBackend()
  const store = await freshStore(backend)
  const vllm = await openVllm(store)

  assert.ok(await store.writeSelectedProvider(), '保存应当成功')

  // 同场景 A：保存后从 selectedProvider 读**当前**草稿对象（界面就是这么读的）。
  const after = store.selectedProvider!
  assert.equal(after.id, 'vllm')
  assert.equal(store.credentialDraftOf(after), 'sk', '字段不能空白、不能变旧值')
  assert.equal(backend.creds.VLLM_API_KEY, 'sk')
})

test('修改令牌后保存：新值必须落盘且留在输入框（曾经的「保存后被清空」）', async () => {
  const backend = makeBackend()
  const store = await freshStore(backend)
  const vllm = await openVllm(store)

  // 界面输入框写的是「当时选中的草稿对象」（这里是 vllm 代理）——与模板 v-model
  // 同一条路；保存后界面从 selectedProvider 读回，测试同样如此。
  store.setCredentialDraft(vllm, 'sk-new-real-key')
  assert.ok(await store.writeSelectedProvider(), '保存应当成功')

  // 保存重建了草稿：字段断言读**当前**的选中对象，不能读保存前的旧引用。
  const after = store.selectedProvider!
  assert.equal(after.id, 'vllm', '选中必须仍落在 vllm 上')
  assert.equal(store.credentialDraftOf(after), 'sk-new-real-key', '字段必须显示刚保存的新值')
  assert.equal(store.credentialStoredOf(after), 'sk-new-real-key')
  assert.equal(backend.creds.VLLM_API_KEY, 'sk-new-real-key', '新令牌必须写进凭据文件')
})

test('改名 + 改令牌一起保存：令牌跟随新路由键落盘，选中不跳走', async () => {
  const backend = makeBackend()
  const store = await freshStore(backend)
  const vllm = await openVllm(store)

  // vllm 有显式 apiKeyEnv（VLLM_API_KEY）：引用名与 id 无关，改名后令牌仍存
  // VLLM_API_KEY 下（settings 侧整行保留），但新输入的值必须写进去。
  ;(vllm as { id: string }).id = 'my-gw'
  store.setCredentialDraft(vllm, 'sk-new-real-key')
  assert.ok(await store.writeSelectedProvider(), '保存应当成功')

  assert.equal(store.selectedProvider?.id, 'my-gw', '选中必须留在改名后的这一条上')
  assert.equal(store.credentialDraftOf(store.selectedProvider!), 'sk-new-real-key')
  assert.equal(backend.creds.VLLM_API_KEY, 'sk-new-real-key', '显式引用名下写入新值')
  assert.equal(backend.creds.MY_GW_API_KEY, undefined, '显式引用名不随 id 派生')
  assert.equal(backend.providers['my-gw']?.apiKeyEnv, 'VLLM_API_KEY', '引用行随块保留')
})

test('派生引用名 + 改名 + 改令牌：引用键跟着换名，新值落盘', async () => {
  // 没有显式 apiKeyEnv 的自定义路由：凭据引用名是从路由键派生的（AAA_API_KEY）。
  current = (() => {
    const base = makeBackend()
    base.providers.aaa = {
      id: 'aaa',
      displayName: null,
      api: 'openai-completions',
      baseUrl: 'https://gw.example/v1',
      apiKeyEnv: null,
      reasoning: null,
      models: [{ id: 'm1', name: null, contextWindow: null, maxTokens: null, input: null, reasoningDisabled: false, reasoningEfforts: [], unmanagedFields: [] }],
      unmanagedFields: [],
      custom: true,
    }
    base.creds.AAA_API_KEY = 'sk-old'
    return base
  })()
  const { createPinia, setActivePinia } = await import('pinia')
  setActivePinia(createPinia())
  const mod = await import('@/stores/dshModels')
  const store = mod.useDshModelsStore() as unknown as Store
  const backend = current

  await store.load()
  await flush()
  const aaa = store.visibleProviders.find((d) => d.id === 'aaa')
  assert.ok(aaa, 'aaa 应在清单里')
  store.selectProvider(aaa)
  await flush()
  assert.equal(store.credentialDraftOf(aaa), 'sk-old', '派生引用名要能读回已存值')

  ;(aaa as { id: string }).id = 'bbb'
  store.setCredentialDraft(aaa, 'sk-new')
  assert.ok(await store.writeSelectedProvider(), '保存应当成功')

  assert.equal(store.selectedProvider?.id, 'bbb', '选中必须留在改名后的这一条上')
  assert.equal(store.credentialDraftOf(store.selectedProvider!), 'sk-new')
  assert.equal(backend.creds.BBB_API_KEY, 'sk-new', '新值存进新派生引用名下')
  assert.equal(backend.creds.AAA_API_KEY, undefined, '旧引用名不再保留值')
  assert.equal(backend.providers.bbb?.apiKeyEnv, 'BBB_API_KEY', '补写新的 apiKeyEnv 行')
})

test('新建供应商 + 填令牌 + 写入：令牌落盘且字段不空白', async () => {
  const backend = makeBackend()
  const store = await freshStore(backend)
  await store.load()
  await flush()

  const full = (await import('@/stores/dshModels')).useDshModelsStore() as unknown as {
    addProvider: () => unknown
  }
  full.addProvider() // 新建草稿并选中（与界面「新建供应商」同一条路）

  // 界面一律拿 selectedProvider（响应式代理）操作凭据草稿，测试同样如此。
  const selected = store.selectedProvider!
  ;(selected as { id: string }).id = 'new-gw'
  ;(selected as { api: string | null; baseUrl: string | null }).api = 'openai-completions'
  ;(selected as { api: string | null; baseUrl: string | null }).baseUrl = 'https://gw.example/v1'
  store.setCredentialDraft(selected, 'sk-fresh')
  assert.ok(await store.writeSelectedProvider(), '写入应当成功')

  assert.equal(store.credentialDraftOf(store.selectedProvider!), 'sk-fresh', '字段不能空白')
  assert.equal(store.credentialStoredOf(store.selectedProvider!), 'sk-fresh')
  assert.equal(backend.creds.NEW_GW_API_KEY, 'sk-fresh', '令牌必须落盘')
  assert.equal(backend.providers['new-gw']?.apiKeyEnv, 'NEW_GW_API_KEY')
})
