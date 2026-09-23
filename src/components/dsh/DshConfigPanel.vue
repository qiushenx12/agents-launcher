<template>
  <div class="dsh-config-panel">
    <Transition name="dsh-left-pane">
      <div
        v-if="!props.sidebarCollapsed"
        class="dsh-config-panel__sidebar-shell"
        :style="{ width: `${leftWidth + 9}px`, flexBasis: `${leftWidth + 9}px` }"
      >
        <aside class="provider-sidebar" :style="{ width: `${leftWidth}px`, flexBasis: `${leftWidth}px` }">
          <button class="btn btn-primary provider-sidebar__new" type="button" @click="createProvider">
            新建供应商
          </button>

          <div class="provider-sidebar__body">
            <div v-if="store.loading && !store.loaded" class="provider-sidebar__empty">正在读取…</div>
            <div v-else-if="store.providers.length === 0" class="provider-sidebar__empty">
              settings.yaml 里还没有供应商。
            </div>
            <!--
              文件里有供应商、但一条自定义的都没有：清单会空着。这里必须解释一句，
              否则用户会以为自己的供应商丢了（它们还在文件里，只是由 dsh 自带目录
              提供全部字段，本页没有可编辑的内容）。
            -->
            <div v-else-if="store.visibleProviders.length === 0" class="provider-sidebar__empty">
              本页只显示自定义供应商；settings.yaml 里剩下的都是 dsh 自带的目录路由。
            </div>
            <div v-else class="provider-list">
              <button
                v-for="(item, index) in store.visibleProviders"
                :key="item.id"
                data-drag-item
                class="provider-list__item"
                :class="{
                  'provider-list__item--selected': pane === 'provider' && store.selectedProvider === item,
                  'provider-list__item--dragging': draggingIndex === index,
                  'provider-list__item--drag-over': draggingIndex !== null
                    && draggingIndex !== index
                    && overIndex === index,
                }"
                type="button"
                @click="onProviderClick(item, index)"
              >
                <span
                  class="provider-list__drag-handle"
                  title="拖拽排序（仅调整列表顺序，不改写文件）"
                  @pointerdown="onPointerDown(index, $event)"
                />
                <span class="provider-list__content">
                  <strong>{{ item.displayName || item.id }}</strong>
                  <small>{{ item.id }} · {{ item.models.length }} 个模型</small>
                </span>
                <span
                  class="provider-list__state"
                  :class="{ 'provider-list__state--draft': isDirty(item) }"
                >
                  {{ item.originalId === null
                    ? '未写入'
                    : isDirty(item) ? '待更新' : '已写入' }}
                </span>
              </button>
            </div>
          </div>

          <!--
            侧边栏页脚：「启动设置」在「设置」上方，两个入口同属一层，尺寸也一致。
            `.settings-entry` 这个类名不能改——App.vue 的「点击空白关闭浮层」靠它
            识别触发按钮，而且外观要与其它 CLI 左下角的设置入口一致。
          -->
          <footer class="provider-sidebar__footer">
            <button
              class="sidebar-entry"
              :class="{ 'sidebar-entry--selected': pane === 'startup' }"
              type="button"
              @click="selectStartup"
            >
              ▶ <span>启动设置</span>
            </button>
            <button class="settings-entry" type="button" @click="toggleSettings($event)">
              ⚙ <span>设置</span>
            </button>
          </footer>
        </aside>

        <div
          class="dsh-config-panel__divider"
          :class="{ 'dsh-config-panel__divider--dragging': isDragging }"
          @mousedown="onMouseDown"
        />
      </div>
    </Transition>

    <main class="config-content">
      <!--
        状态条只留 warning / error（读取失败、写入失败、校验拦截……）：它们要用户
        处理，必须常驻可见。成功/信息类的操作反馈走底部飘字（3 秒消失），不进横幅。
        横幅放在两个面板**之外**：用户停在「启动设置」页时一次读盘失败也看得见。
      -->
      <ConfigStatusBanner
        v-if="store.status"
        :message="store.status.message"
        :tone="store.status.tone"
      />

      <!--
        启动设置用 v-show 而不是 v-if：它内部有每秒推进的「已运行 …」计时器和
        端口探测，切走再切回不该把它们重置。
      -->
      <DshStartupSettingsPane v-show="pane === 'startup'" @open-runtime="emit('open-runtime')" />

      <template v-if="pane === 'provider'">
        <DshProviderEditor />

        <section class="card source-note">
          <div class="card-title">说明</div>
          <p>只显示自定义供应商。</p>
          <p>
            写入只改你正在编辑的这一个供应商，settings.yaml 里的其他内容（其他供应商、
            未列出的字段）原样保留。
          </p>
          <p>
            写入前会把原文件备份为同目录的 <code>settings.yaml.bak</code>；
            被改动的那个字段会按规范格式重排，它内部的注释不会保留。
          </p>
          <p class="source-note__version">
            本页面的字段对应 dsh <strong>v{{ supportedVersionLabel }}</strong>（开发者预览版，字段可能随版本变动）
          </p>
          <div class="config-path-row">
            <button class="btn btn-secondary" type="button" @click="openSettingsDirectory">
              打开设置目录
            </button>
            <button
              class="btn btn-secondary"
              type="button"
              :disabled="store.loading"
              @click="store.load()"
            >
              {{ store.loading ? '读取中…' : '重新读取' }}
            </button>
          </div>
        </section>
      </template>
    </main>

    <Transition name="dsh-draft-toast">
      <div
        v-if="visibleToast"
        class="dsh-config-panel__toast"
        role="status"
        aria-live="polite"
      >
        {{ visibleToast }}
      </div>
    </Transition>
  </div>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useDshModelsStore, type DshProviderDraft } from '@/stores/dshModels'
import { useDshConfigStore } from '@/stores/dshConfig'
import { useSettingsPopover } from '@/composables/useSettingsPopover'
import { useDragReorder } from '@/composables/useDragReorder'
import { useSharedLeftSidebarWidth } from '@/composables/useSharedLeftSidebarWidth'
import { beginStartupMeasure } from '@/utils/startupMetrics'
import ConfigStatusBanner from '@/components/config/ConfigStatusBanner.vue'
import DshProviderEditor from './DshProviderEditor.vue'
import DshStartupSettingsPane from './DshStartupSettingsPane.vue'

/**
 * dsh 配置工作台的外壳。
 *
 * 布局与其它三个 CLI 对齐：左边一列清单、右边是选中项的内容。dsh 的清单是
 * **供应商**（settings.yaml 的 `llm-pi-ai.providers`），页脚两个入口——
 * 「启动设置」（原来的整张卡片）在「设置」上方。
 *
 * 供应商与模型的读写全部交给 Rust（`src-tauri/src/dsh_settings.rs`）：前端只拿
 * 结构化的供应商列表、也只回写结构化数据，YAML 的解析与最小化改写都在后端。
 * 之所以不让前端拼 YAML，是因为要保证「只重写启动器负责的字段、其余逐字节保留」，
 * 而这件事只能由掌握原始行号的一侧来做。
 *
 * ⚠️ dsh 仍在快速迭代（官方称 developer preview）。这里的字段对齐
 * v0.1.5-rc.1。一旦失效，按下面这条路径去核对官方文档：仓库
 * `deepseek-ai/deepseek-harness` 的 `docs/user/guide/providers.zh.md` 与
 * `docs/config-catalog.zh.md`（逐字段真源，不随 npm 包发布），以及包内
 * `@deepseek-ai/dsh-llm-pi-ai/README.zh.md`；改的时候两处要同步：
 * `src-tauri/src/dsh_settings.rs` 与 `src/types/config.ts`。
 *
 * 这份排查指引只留在注释里——界面上只写「对齐哪一版」，不再展开文档路径。
 */
const store = useDshModelsStore()
const runtimeStore = useDshConfigStore()
const { toggleSettings } = useSettingsPopover()

const props = defineProps<{
  sidebarCollapsed?: boolean
}>()
const emit = defineEmits<{
  (event: 'left-width-change', width: number): void
  (event: 'open-runtime'): void
}>()

/**
 * 右侧当前显示哪个面板。
 *
 * 本次运行的**首次**进入停在「启动设置」，不是供应商编辑器：dsh 与其它三个前端的
 * 形状不同，那三个只有一屏配置内容，进来落在编辑器上是对的；dsh 这一页进来第一件
 * 要做的事通常是把服务启起来（访问范围、端口、状态都在「启动设置」里）。之后不再
 * 强制回位——面板是 v-show 常驻的（计时器与端口探测不能随切走销毁），切走再切回
 * 显示的就是用户上一次选的那一屏（2026-09-18 用户定稿）。
 */
type DshPane = 'startup' | 'provider'
const pane = ref<DshPane>('startup')

/**
 * 飘字通道（2026-09-18 任务 202609181728080000）：成功/信息类的操作反馈
 * （写入供应商、删除、令牌保存、获取模型、新增草稿……）都在这里，3 秒消失。
 * 文案由 store 统一产生（setStatus 分流：success/info → toast，warning/error →
 * 顶部横幅常驻）；面板 watch `toastSeq` 重置计时器，连发两条也各自计满 3 秒。
 */
const visibleToast = ref('')
let toastTimer: number | null = null

watch(
  () => store.toastSeq,
  () => {
    if (!store.toast) return
    visibleToast.value = store.toast
    if (toastTimer !== null) window.clearTimeout(toastTimer)
    toastTimer = window.setTimeout(() => {
      visibleToast.value = ''
      toastTimer = null
    }, 3000)
  },
)

const supportedVersionLabel = computed(() => store.supportedVersion || '0.1.5-rc.1')

const { draggingIndex, overIndex, justDragged, onPointerDown } = useDragReorder(
  () => store.visibleProviders.map((item) => item.id),
  // 清单里只有自定义路由，交给 store 按位置原地替换——目录路由留在原位。拖拽只改
  // 视图顺序，不触发任何写入。
  (newOrder: string[]) => store.reorderVisible(newOrder),
  { gapPx: 2 },
)

const { leftWidth, isDragging, onMouseDown, loadWidth } = useSharedLeftSidebarWidth()

watch(leftWidth, (width) => {
  emit('left-width-change', width)
}, { immediate: true })

/** 与 store 的脏检查同一套口径：新草稿一律算未写入。 */
const isDirty = (item: DshProviderDraft) => store.isProviderDirty(item)

function onProviderClick(item: DshProviderDraft, index: number) {
  if (justDragged.value) return
  void index
  store.selectProvider(item)
  pane.value = 'provider'
}

function createProvider() {
  const draft = store.addProvider()
  pane.value = 'provider'
  store.setStatus('info', `已新增供应商草稿「${draft.id}」；填好字段后写入设置文件。`)
}

function selectStartup() {
  pane.value = 'startup'
}

/** 打开 settings.yaml 所在目录，方便用户直接手改（dsh 也支持热重载这份文件）。 */
async function openSettingsDirectory() {
  const path = store.settingsPath
  const separator = Math.max(path.lastIndexOf('\\'), path.lastIndexOf('/'))
  if (!path || separator < 0) {
    store.setStatus('error', '尚未定位到 dsh 设置目录。')
    return
  }
  try {
    await invoke('open_directory', { path: path.slice(0, separator) })
  } catch (error) {
    store.setStatus('error', `打开设置目录失败：${error}`)
  }
}

onMounted(async () => {
  const finish = beginStartupMeasure('dsh-config-panel')
  try {
    await Promise.all([
      loadWidth().catch(() => {}),
      runtimeStore.load().catch(() => {}),
      store.load().catch(() => {}),
    ])
  } finally {
    finish()
  }
})

onBeforeUnmount(() => {
  if (toastTimer !== null) window.clearTimeout(toastTimer)
})
</script>

<style scoped>
.dsh-config-panel {
  height: 100%;
  min-height: 0;
  display: flex;
  position: relative;
  background: transparent;
}

.dsh-config-panel__toast {
  position: absolute;
  left: 50%;
  bottom: 18px;
  z-index: 30;
  max-width: min(520px, calc(100% - 48px));
  padding: 8px 12px;
  transform: translateX(-50%);
  border-radius: var(--radius-sm);
  color: #fff;
  background: rgba(29, 29, 31, 0.92);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.18);
  font-size: var(--font-size-small);
  line-height: 1.5;
  text-align: center;
  pointer-events: none;
}

.dsh-draft-toast-enter-active,
.dsh-draft-toast-leave-active {
  transition: opacity 0.18s ease, transform 0.18s ease;
}

.dsh-draft-toast-enter-from,
.dsh-draft-toast-leave-to {
  opacity: 0;
  transform: translateX(-50%) translateY(6px);
}

.dsh-config-panel__sidebar-shell {
  flex: 0 0 auto;
  min-width: 0;
  min-height: 0;
  display: flex;
  overflow: hidden;
}

.dsh-left-pane-enter-active,
.dsh-left-pane-leave-active {
  transition: width 0.22s ease, flex-basis 0.22s ease, opacity 0.16s ease;
}

.dsh-left-pane-enter-from,
.dsh-left-pane-leave-to {
  width: 0 !important;
  flex-basis: 0 !important;
  opacity: 0;
}

.provider-sidebar {
  width: 280px;
  flex: 0 0 auto;
  min-width: 0;
  padding: 12px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.dsh-config-panel__divider {
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

.dsh-config-panel__divider::after {
  content: '';
  width: 1px;
  height: 100%;
  /* Grab strip only: the sidebar and the editor pane are different surfaces, so
     the colour step already marks the boundary. Hover/drag still highlights. */
  background-color: transparent;
  transition: background-color 0.2s ease, width 0.2s ease, box-shadow 0.2s ease;
}

.dsh-config-panel__divider:hover::after,
.dsh-config-panel__divider--dragging::after {
  width: 2px;
  background-color: var(--primary);
}

[data-theme="dark"] .dsh-config-panel__divider:hover::after,
[data-theme="dark"] .dsh-config-panel__divider--dragging::after {
  box-shadow: 0 0 6px 1px rgba(10, 132, 255, 0.5);
}

.provider-sidebar__body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.provider-sidebar__footer {
  flex-shrink: 0;
  padding-top: 8px;
  border-top: 1px solid var(--separator);
}

.provider-sidebar__new {
  width: 100%;
  margin-bottom: 8px;
}

.provider-sidebar__empty {
  padding: 18px 8px;
  color: var(--text-secondary);
  text-align: center;
  font-size: var(--font-size-small);
  line-height: 1.5;
}

.provider-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.provider-list__item {
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

.provider-list__item:hover { background: var(--tab-bg); }
.provider-list__item--selected { color: #fff; background: var(--primary); }
.provider-list__item--selected:hover { background: var(--primary-hover); }
.provider-list__item--dragging { opacity: 0.3; background: var(--tab-bg); }

.provider-list__drag-handle {
  width: 14px;
  height: 14px;
  flex-shrink: 0;
  position: relative;
  cursor: grab;
  opacity: 0;
  transition: opacity 0.12s ease;
  touch-action: none;
}

.provider-list__drag-handle::before,
.provider-list__drag-handle::after {
  content: '';
  position: absolute;
  left: 1px;
  width: 2.5px;
  height: 2.5px;
  border-radius: 50%;
  background-color: var(--text-secondary);
  box-shadow: 5px 0 0 var(--text-secondary), 10px 0 0 var(--text-secondary);
}

.provider-list__drag-handle::before { top: 1.5px; }
.provider-list__drag-handle::after { bottom: 1.5px; }
.provider-list__drag-handle:active { cursor: grabbing; }
.provider-list__item:hover .provider-list__drag-handle { opacity: 1; }

.provider-list__item--selected .provider-list__drag-handle::before,
.provider-list__item--selected .provider-list__drag-handle::after {
  background-color: rgba(255, 255, 255, 0.72);
  box-shadow: 5px 0 0 rgba(255, 255, 255, 0.72), 10px 0 0 rgba(255, 255, 255, 0.72);
}

.provider-list__content {
  min-width: 0;
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.provider-list__content strong,
.provider-list__content small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.provider-list__content small {
  opacity: 0.72;
  font-size: var(--font-size-small);
}

.provider-list__state {
  flex: 0 0 auto;
  padding: 2px 5px;
  border-radius: 4px;
  color: var(--success, #22c55e);
  background: color-mix(in srgb, var(--success, #22c55e) 12%, transparent);
  font-size: 10px;
}

.provider-list__state--draft {
  color: var(--warning);
  background: color-mix(in srgb, var(--warning) 13%, transparent);
}

.provider-list__item--selected .provider-list__state {
  color: #fff;
  background: rgba(255, 255, 255, 0.18);
}

/* 「启动设置」入口。刻意与 .settings-entry 同款外观（同一字号、同一内边距），
   但排在上面、并且能显示选中态——它切换的是右侧内容，而「设置」打开的是一个
   浮层。 */
.sidebar-entry {
  width: 100%;
  display: flex;
  gap: 8px;
  align-items: center;
  padding: 7px 8px;
  margin-bottom: 2px;
  border: 0;
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  background: transparent;
  cursor: pointer;
  text-align: left;
  font-family: var(--font-base);
  font-size: var(--font-size-base);
}

.sidebar-entry:hover {
  color: var(--text-primary);
  background: var(--tab-bg);
}

.sidebar-entry--selected {
  color: #fff;
  background: var(--primary);
}

.sidebar-entry--selected:hover { background: var(--primary-hover); }

.config-content {
  min-width: 0;
  flex: 1;
  height: 100%;
  padding: 12px 16px;
  overflow-y: auto;
}

.source-note {
  max-width: 980px;
  margin: 0 auto 12px;
}

.source-note p {
  margin: 6px 0;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  line-height: 1.55;
}

.source-note code {
  overflow-wrap: anywhere;
}

.source-note__version {
  padding-top: 6px;
  border-top: 1px solid var(--separator);
}

.config-path-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 10px;
}

@media (max-width: 900px) {
  .provider-sidebar { width: 230px; }
}
</style>
