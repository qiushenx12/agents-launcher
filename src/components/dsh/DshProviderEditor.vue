<template>
  <section v-if="provider" class="card provider-editor">
    <header class="provider-header">
      <!-- 标题与 Claude Code 的编辑器对齐，固定「配置编辑」；当前是谁看左侧选中项
           与下面的「供应商 ID」，写入状态看右侧徽标。 -->
      <div class="card-title">配置编辑</div>
      <span
        class="provider-write-state"
        :class="{ 'provider-write-state--draft': store.isSelectedDirty }"
      >
        {{ provider.originalId === null ? '未写入' : store.isSelectedDirty ? '待更新' : '已写入' }}
      </span>
    </header>

    <div class="field-row">
      <label class="field-label">供应商 ID</label>
      <input
        v-model="provider.id"
        class="input"
        type="text"
        placeholder="例如 vllm、my-gateway"
        spellcheck="false"
      >
    </div>

    <!--
      `displayName`（显示名称）**刻意不放在界面上**：dsh 在它缺失时回落到路由键
      （`dsh-llm-pi-ai/lib/index.js` 的 `source.displayName ?? provider`），所以这
      只是一个可选的美化名，用户没有必须做出的决定。要改美化名去 dsh 的
      「设置 → 模型」页，那里有这一栏；这条路由叫什么，看左侧清单的选中项。

      值仍在读取与回写路径上：目录里的供应商多半带着这个名字，读进来多少就原样
      写回去，界面不碰它。注意 dsh 拒绝空字符串（`displayName.length === 0` 直接
      抛错），「留空」在文件里的合法形态是该行整行不存在——后端的写法正是如此。
    -->
    <!--
      协议是 dsh schema 里的封闭枚举（`api: z.union(supportedProtocols())`），
      所以用下拉框把三种全列出来。留空是合法的第四种状态——目录里已有的供应商
      由目录决定。用输入框 + datalist 会按当前值过滤候选：一条已经写着
      `openai-completions` 的路由点开只看得到它自己，看着就像只有一种协议。
    -->
    <div class="field-row">
      <label class="field-label">wire 协议</label>
      <select v-model="apiChoice" class="select">
        <option value="">留空（由目录决定）</option>
        <option v-for="protocol in DSH_API_PROTOCOLS" :key="protocol" :value="protocol">
          {{ protocol }}
        </option>
        <!-- 手写进 settings.yaml 的协议：列出来，免得下拉空白、一保存就把它改掉。 -->
        <option v-if="unlistedApi" :value="unlistedApi">{{ unlistedApi }}（dsh 不支持）</option>
      </select>
    </div>
    <p class="field-help">手工声明的网关必须填。</p>

    <div class="field-row">
      <label class="field-label">API 地址</label>
      <input
        v-model="provider.baseUrl"
        class="input"
        type="text"
        placeholder="https://your-gateway.example/v1"
        spellcheck="false"
      >
    </div>

    <!--
      认证令牌：**值**存 `$DSH_HOME/.credentials.yaml` 的 refs 分节，settings.yaml
      里只有引用名 `apiKeyEnv`。引用名不设输入框：文件里已有就沿用，没有就按 dsh
      的派生规则（路由键大写、连续非字母数字折叠成 `_`、后缀 `_API_KEY`，即
      `deriveKeyRef`）生成并自动补写一行——dsh 自己的模型页录入密钥时也是这么做的
      （`schema.setPath(draft, ["apiKeyEnv"], keyRef)`）。保存空值 = 移除令牌。
      dsh 每次请求按引用名现读现用，保存即生效，不需要重启。启动 dsh 的环境里若有
      同名变量，dsh 会优先用那个只读的值——界面里存的值要等那层清掉才会被看到。
    -->
    <SecretField
      v-model="credentialDraft"
      label="认证令牌"
      placeholder="网关的 API Key，例如 sk-…"
    />
    <div class="token-action-row">
      <button
        class="btn btn-secondary"
        type="button"
        :disabled="
          !store.isCredentialDirty(provider)
            || store.credentialSaving
            || provider.originalId === null
            || !!store.credentialErrorOf(provider)
        "
        @click="store.saveCredential(provider)"
      >
        {{ isRemovingStoredToken ? '移除令牌' : '保存令牌' }}
      </button>
      <span v-if="provider.originalId === null">请先写入供应商，再保存令牌。</span>
      <span v-else-if="store.credentialErrorOf(provider)">{{ store.credentialErrorOf(provider) }}</span>
    </div>
    <div class="field-row">
      <label class="field-label">默认思考档</label>
      <div class="field-inline">
        <input
          v-model="provider.reasoning"
          class="input"
          type="text"
          list="dsh-reasoning-levels"
          placeholder="可留空；请求未指定档位时用它"
          spellcheck="false"
        >
        <datalist id="dsh-reasoning-levels">
          <option v-for="level in DSH_THINKING_LEVELS" :key="level" :value="level" />
        </datalist>
      </div>
    </div>

    <p v-if="provider.unmanagedFields.length" class="unmanaged-note">
      另有 {{ provider.unmanagedFields.length }} 个字段由 settings.yaml 直接管理：
      <code v-for="field in provider.unmanagedFields" :key="field">{{ field }}</code>
    </p>

    <section class="models-section">
      <header class="models-header">
        <strong>模型</strong>
      </header>

      <div class="model-add-row">
        <input
          v-model="modelDraft"
          class="input"
          type="text"
          list="dsh-known-models"
          placeholder="输入模型 ID，例如 Qwen3.8-27B"
          spellcheck="false"
          @keydown.enter.prevent="addModel"
        >
        <datalist id="dsh-known-models">
          <option v-for="id in store.knownModelIds" :key="id" :value="id" />
        </datalist>
        <button class="btn btn-secondary" type="button" @click="addModel">添加模型</button>
      </div>

      <div v-if="provider.models.length === 0" class="model-empty">暂无模型</div>
      <div v-else class="model-list">
        <!--
          表格里**没有「显示名称」列**，同供应商的 `displayName` 一个道理：dsh 在模型
          条目缺 `name` 时回落到模型 ID（`dsh-llm-pi-ai/lib/index.js` 的
          `entry.name ?? base?.name ?? entry.id`），所以它不是必填项，用户在这里没有
          要做出的决定。模型条目里的 `name` 仍然原样往返（目录里的模型多半带着它），
          界面只是不给编辑。要改美化名去 dsh 的「设置 → 模型」页。
        -->
        <div class="model-columns" aria-hidden="true">
          <span>模型 ID</span>
          <span>上下文长度</span>
          <span>输出上限</span>
          <span>输入能力</span>
          <span></span>
        </div>
        <div v-for="model in provider.models" :key="model.id" class="model-card">
          <div class="model-row">
            <input v-model="model.id" class="input" type="text" aria-label="模型 ID" spellcheck="false">
            <input
              :value="model.contextWindow ?? ''"
              class="input"
              type="number"
              min="1"
              step="1"
              aria-label="上下文长度"
              placeholder="例如 262144"
              @input="updateCount(model, 'contextWindow', $event)"
            >
            <input
              :value="model.maxTokens ?? ''"
              class="input"
              type="number"
              min="1"
              step="1"
              aria-label="输出上限"
              placeholder="例如 32768"
              @input="updateCount(model, 'maxTokens', $event)"
            >
            <div class="modality-options" aria-label="模型输入能力">
              <label><input :checked="hasModality(model, 'text')" type="checkbox" @change="toggleModality(model, 'text')"> Text</label>
              <label><input :checked="hasModality(model, 'image')" type="checkbox" @change="toggleModality(model, 'image')"> Image</label>
            </div>
            <button class="icon-button" type="button" title="删除模型" @click="store.removeModel(provider, model.id)">×</button>
          </div>

          <!--
            思考档位是**按模型**的能力，不是供应商级的：同一个供应商下的模型对
            档位的支持并不一致，放在供应商上必然会被其中一些模型拒绝。所以它跟
            着每个模型走。
            左边勾选 dsh 提供哪几档，右边填这一档真正发给网关的值——两者可以不同，
            例如把 max 映射成网关自己的 ultra。
          -->
          <div class="model-reasoning">
            <div class="model-reasoning__levels">
              <span class="model-reasoning__label">思考档位</span>
              <label v-for="level in DSH_THINKING_LEVELS" :key="level" class="level-option">
                <input
                  :checked="hasLevel(model, level)"
                  type="checkbox"
                  @change="store.toggleReasoningLevel(model, level)"
                >
                {{ level }}
              </label>
              <label class="level-option level-option--disable">
                <input v-model="model.reasoningDisabled" type="checkbox"> 非推理模型
              </label>
            </div>

            <div v-if="hasLevelsToWire(model)" class="model-reasoning__wires">
              <label v-for="item in model.reasoningEfforts" :key="item.level" class="wire-field">
                <span>{{ item.level }}</span>
                <input
                  :value="item.wire ?? ''"
                  class="input"
                  type="text"
                  :placeholder="item.level === 'off' ? '留空 = 不发送参数' : '发给网关的值'"
                  spellcheck="false"
                  @input="store.setReasoningWire(item, ($event.target as HTMLInputElement).value)"
                >
              </label>
            </div>

            <p v-if="model.unmanagedFields.length" class="unmanaged-note unmanaged-note--model">
              另有字段由 settings.yaml 管理：
              <code v-for="field in model.unmanagedFields" :key="field">{{ field }}</code>
            </p>
          </div>
        </div>
      </div>
    </section>

    <div class="provider-action-row">
      <button
        class="btn btn-primary"
        type="button"
        :disabled="store.saving"
        @click="store.writeSelectedProvider()"
      >
        {{ store.saving ? '处理中…' : provider.originalId ? '写入当前修改' : '写入 settings.yaml' }}
      </button>
      <button
        class="btn btn-secondary danger-button"
        type="button"
        :disabled="store.saving"
        @click="removeProvider"
      >
        {{ provider.originalId ? '从 settings.yaml 删除' : '删除草稿' }}
      </button>
    </div>
  </section>

  <section v-else class="card provider-empty">
    <div class="card-title">供应商与模型</div>
    <p>从左侧选择一个供应商，或点「新建供应商」加一条。</p>
  </section>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { confirm } from '@tauri-apps/plugin-dialog'
import {
  DSH_API_PROTOCOLS,
  DSH_THINKING_LEVELS,
  type DshModelProfile,
  type DshThinkingLevel,
} from '@/types/config'
import { useDshModelsStore } from '@/stores/dshModels'
import SecretField from '@/components/config/SecretField.vue'

/**
 * 一个供应商路由的编辑器。
 *
 * 字段集合是照着 dsh 自己的「设置 → 模型」页定的（`@deepseek-ai/dsh-client-ui-settings-models`
 * 的 `ProviderEditor`）：它同样只暴露 凭据 / `baseURL` / 显示名 / wire 协议，
 * 其余留在 settings.yaml。那里有一句设计说明值得照抄——**思考强度是 per-MODEL
 * 的能力**，同一个供应商下的模型对它并不一致，所以做成供应商级开关只会「设成
 * 某个值、然后被一部分模型拒绝」。因此档位编辑跟着每个模型走。
 *
 * ⚠️ dsh 仍是 developer preview，字段与层级会变。这里对齐 v0.1.5-rc.1；一旦
 * 失效，按 `src-tauri/src/dsh_settings.rs` 顶部注释里的路径去查官方文档
 * （仓库 `deepseek-ai/deepseek-harness` 的 docs/config-catalog.zh.md 与
 * docs/user/guide/providers.zh.md），再同步两侧。
 */
const store = useDshModelsStore()
const modelDraft = ref('')

/** 模态声明的固定顺序；写回时按它排列，避免同一份配置每次产生不同文本。 */
const DSH_MODALITY_ORDER = ['text', 'image'] as const

// 直接读 store 的选中项：编辑器只有一个，不必再广播一份选中状态。
const provider = computed(() => store.selectedProvider)

/**
 * 下拉框的选中值。留空在草稿里是 `null`（写回时该字段整行省略），下拉框用空串
 * 表示，所以这里做一个映射，避免「选了留空」被脏检查当成有改动。
 */
const apiChoice = computed({
  get: () => provider.value?.api ?? '',
  set: (value: string) => {
    const target = store.selectedProvider
    if (target) target.api = value || null
  },
})

/**
 * 文件里已有、但不在 dsh 支持范围内的协议值。列成兜底项，否则下拉框显示空白，
 * 用户随手一选就把原本的值改掉了——那份配置是用户手写的，不该由界面悄悄抹平。
 */
const unlistedApi = computed(() => {
  const value = provider.value?.api
  if (!value) return null
  return (DSH_API_PROTOCOLS as readonly string[]).includes(value) ? null : value
})

/** 令牌输入框的草稿：按供应商各存一份，选中谁编辑谁的。 */
const credentialDraft = computed({
  get: () => (provider.value ? store.credentialDraftOf(provider.value) : ''),
  set: (value: string) => {
    const target = store.selectedProvider
    if (target) store.setCredentialDraft(target, value)
  },
})

/** 已存过令牌、现在把输入框清空 = 下一次保存是移除。按钮文案跟着这个状态走。 */
const isRemovingStoredToken = computed(() =>
  provider.value !== null
    && store.credentialStoredOf(provider.value) !== ''
    && store.credentialDraftOf(provider.value).trim() === '',
)

function addModel() {
  const target = store.selectedProvider
  if (!target) return
  if (store.addModel(target, modelDraft.value)) modelDraft.value = ''
}

async function removeProvider() {
  const target = store.selectedProvider
  if (!target) return
    if (target.originalId !== null) {
      const accepted = await confirm(
        `将从 settings.yaml 中移除供应商「${target.originalId}」的整块配置。\n\n`
        + '其它供应商与所有未展示的字段都会保留。'
        + '认证令牌保存在 dsh 的凭据文件里，本次删除不会动它。\n\n是否继续？',
        { title: '删除供应商', kind: 'warning' },
      )
      if (!accepted) return
    }
  await store.removeProvider(target)
}

function updateCount(
  model: DshModelProfile,
  field: 'contextWindow' | 'maxTokens',
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

function hasModality(model: DshModelProfile, modality: string): boolean {
  return (model.input ?? []).includes(modality)
}

/**
 * 勾选输入能力。两个都没勾时写成 `null`（字段整个省掉），而不是空数组——
 * dsh 把空数组视为「什么都不收」，和「不声明、沿用目录默认」不是同一件事。
 */
function toggleModality(model: DshModelProfile, modality: string) {
  const current = new Set(model.input ?? [])
  if (current.has(modality)) current.delete(modality)
  else current.add(modality)
  const next = DSH_MODALITY_ORDER.filter((item) => current.has(item))
  model.input = next.length > 0 ? next : null
}

function hasLevel(model: DshModelProfile, level: DshThinkingLevel): boolean {
  return model.reasoningEfforts.some((item) => item.level === level)
}

/** 有需要填 wire 值的档位时才渲染那一行（只勾了 off 就不必了）。 */
function hasLevelsToWire(model: DshModelProfile): boolean {
  return model.reasoningEfforts.some((item) => item.level !== 'off')
}
</script>

<style scoped>
.card {
  max-width: 980px;
  margin: 0 auto 12px;
}

.provider-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 9px;
}

.provider-write-state {
  flex: 0 0 auto;
  padding: 3px 7px;
  border-radius: 999px;
  color: var(--success, #22c55e);
  background: color-mix(in srgb, var(--success, #22c55e) 12%, transparent);
  font-size: var(--font-size-small);
}

.provider-write-state--draft {
  color: var(--warning, #d49a45);
  background: color-mix(in srgb, var(--warning, #d49a45) 13%, transparent);
}

.field-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 5px 0;
}

.field-label {
  width: 110px;
  flex: 0 0 auto;
  color: var(--text-secondary);
  text-align: right;
}

.field-row > .input,
.field-row > .select,
.field-inline {
  min-width: 0;
  flex: 1;
}

.field-inline {
  display: flex;
  align-items: center;
  gap: 6px;
}

.field-inline .input {
  min-width: 0;
  flex: 1;
}

.field-help {
  margin: 1px 0 6px 120px;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  line-height: 1.5;
}

.token-action-row {
  margin: 2px 0 8px 120px;
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
}

.unmanaged-note {
  margin: 4px 0 0 120px;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  line-height: 1.6;
}

.unmanaged-note--model {
  margin-left: 0;
}

.unmanaged-note code {
  margin-left: 4px;
  padding: 0 4px;
  border-radius: 3px;
  background: color-mix(in srgb, var(--text-secondary) 14%, transparent);
}

.models-section {
  margin-top: 12px;
  padding-top: 10px;
  border-top: 1px solid var(--separator);
}

.models-header {
  display: flex;
  align-items: center;
  gap: 12px;
}

.model-add-row {
  display: flex;
  gap: 6px;
  margin-top: 9px;
}

.model-add-row .input {
  min-width: 0;
  flex: 1;
}

.model-empty,
.provider-empty {
  padding: 18px;
  color: var(--text-secondary);
  text-align: center;
  font-size: var(--font-size-small);
}

.model-list {
  margin-top: 10px;
}

.model-columns,
.model-row {
  display: grid;
  grid-template-columns:
    minmax(190px, 1.5fr) minmax(120px, 1fr) minmax(110px, 1fr) 150px 30px;
  gap: 7px;
  align-items: center;
}

.model-columns {
  padding: 0 2px 4px;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
}

/* 每个模型是一张卡：上面一行是基本字段，下面一行是它的思考档位。 */
.model-card {
  margin-top: 8px;
  padding: 8px 8px 2px;
  border: 1px solid var(--separator);
  border-radius: var(--radius-sm);
}

.model-card + .model-card {
  margin-top: 8px;
}

.model-row .input {
  min-width: 0;
}

.modality-options {
  display: flex;
  align-items: center;
  gap: 12px;
  white-space: nowrap;
}

.modality-options label {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
}

.icon-button {
  border: 0;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  font-size: 19px;
}

.model-reasoning {
  margin: 8px 0 6px;
  padding-top: 7px;
  border-top: 1px dashed var(--separator);
}

.model-reasoning__levels {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 4px 14px;
}

.model-reasoning__label {
  color: var(--text-secondary);
  font-size: var(--font-size-small);
}

.level-option {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--text-primary);
  font-size: var(--font-size-small);
  font-family: var(--font-mono, monospace);
}

.level-option--disable {
  margin-left: auto;
  color: var(--text-secondary);
  font-family: inherit;
}

.model-reasoning__wires {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px 12px;
  margin-top: 7px;
}

.wire-field {
  display: inline-flex;
  align-items: center;
  gap: 5px;
}

.wire-field span {
  min-width: 52px;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  font-family: var(--font-mono, monospace);
}

.wire-field .input {
  width: 170px;
  font-size: var(--font-size-small);
}

.provider-action-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 16px;
  padding-top: 12px;
  border-top: 1px solid var(--separator);
}

.danger-button {
  color: var(--danger, #d96c6c);
}

@media (max-width: 900px) {
  .model-columns {
    display: none;
  }

  /* 窄屏：模型 ID 占满第一行，删除按钮钉在右上，其余字段各占一行。序号与
     `.model-row` 的子元素顺序一一对应（ID / 上下文 / 输出上限 / 输入能力 / 删除）。 */
  .model-row {
    grid-template-columns: 1fr 1fr 30px;
    padding-top: 7px;
  }

  .model-row > :nth-child(1) { grid-column: 1 / 3; grid-row: 1; }
  .model-row > :nth-child(2) { grid-column: 1 / 3; grid-row: 2; }
  .model-row > :nth-child(3) { grid-column: 1 / 3; grid-row: 3; }
  .model-row > :nth-child(4) { grid-column: 1 / 3; grid-row: 4; }
  .model-row > :nth-child(5) { grid-column: 3; grid-row: 1; }
}
</style>
