<template>
  <div class="dsh-runtime-panel">
    <!--
      浮层清单（§4.9 A 组）：子 WebView 是盖在 DOM 之上的原生控件，任何
      fixed / Teleport 浮层都必须先让它隐藏，否则浮层整个看不见。
      让位的条件集中在下面 `embedAllowed` 一处：面板是否当前表面 +
      App.vue 的 dshOverlayOpen（全局设置浮层、顶栏排序弹窗、CLI 门禁的
      最短展示窗口）+ 二维码浮层 + 启动前检测。新增浮层时加进这一处即可，
      不要另写 hide 调用。
    -->
    <!--
      out-in：离场分支先走完再挂载进场分支。默认的交叉过渡会把两个分支同时
      放进 flex 布局，运行态占位区在过渡的 150ms 内被压成一半高度——子
      WebView 按错误的 rect 创建，随后被重测阶梯校正，表现为一次可见的跳动；
      而且创建 WebView2 会阻塞事件循环数百毫秒，正好把交叉过渡冻结在半透明
      状态，解冻后再次突变。
    -->
    <Transition name="dsh-fade" mode="out-in">
      <!--
        端口冲突：自动启动前先探测「保存的端口」，被占用时直接给补救卡片，
        而不是等启动失败再报错。「清理并启动」复用配置页「一键清理占用」的
        后端命令，完成后按保存的配置启动。
      -->
      <div v-if="portConflict" class="dsh-runtime-panel__empty">
        <div class="card dsh-card">
          <div class="card-title">端口 {{ portConflict.port }} 已被占用</div>
          <p class="dsh-note">{{ conflictText }}</p>
          <div v-if="store.isBusy" class="dsh-progress">
            {{ progressText }}
          </div>
          <ConfigStatusBanner v-if="store.actionError" :message="store.actionError" tone="error" />
          <div class="action-row">
            <button
              v-if="conflictKind !== 'supervised'"
              class="btn btn-primary"
              type="button"
              :disabled="store.releasing || store.isBusy"
              @click="cleanupAndStart"
            >
              {{ store.releasing ? '正在清理…' : '清理并启动' }}
            </button>
            <button class="btn btn-secondary" type="button" @click="openConfig()">
              打开配置
            </button>
          </div>
        </div>
      </div>

      <div v-else-if="hasError" class="dsh-runtime-panel__error">
        <div class="card dsh-card">
          <div class="card-title">DeepSeek Harness {{ errorAction }}失败</div>
          <ConfigStatusBanner :message="store.status?.message || embedError" tone="error" />
          <pre v-if="store.status?.detail" class="dsh-detail">{{ store.status.detail }}</pre>
          <div class="action-row">
            <button class="btn btn-primary" type="button" :disabled="store.isBusy" @click="retry">
              重试
            </button>
            <button class="btn btn-secondary" type="button" @click="openConfig()">
              打开配置
            </button>
          </div>
        </div>
      </div>

      <!--
        未运行：进入此界面即按保存的配置自动启动，这张卡片多数时候只展示
        启动进度；启动按钮是自动启动未能执行（如端口探测失败）时的兜底。
      -->
      <div v-else-if="!store.isRunning" class="dsh-runtime-panel__empty">
        <div class="card dsh-card">
          <div class="card-title">DeepSeek Harness 未启动</div>
          <div v-if="store.isBusy || autoStarting || !store.status" class="dsh-progress">
            {{ progressText }}
          </div>
          <template v-else>
            <p class="dsh-note">
              进入此界面会按保存的配置（{{ savedConfigLabel }}）自动启动 dsh 服务，应用内始终通过本机地址访问。
            </p>
            <ConfigStatusBanner v-if="store.actionError" :message="store.actionError" tone="error" />
            <div class="action-row">
              <button
                class="btn btn-primary"
                type="button"
                :disabled="!store.loaded"
                @click="manualStart"
              >
                启动
              </button>
              <button class="btn btn-secondary" type="button" @click="openConfig()">
                打开配置
              </button>
            </div>
          </template>
        </div>
      </div>

      <!--
        运行中：只有子 WebView 的占位区。状态与操作（复制链接、重启、关闭、二维码）
        全部在配置面板里，这里保持纯界面，占位区内不能有任何可见子元素。
      -->
      <div v-else class="dsh-runtime-panel__live">
        <div ref="placeholder" class="dsh-embed-host" />
      </div>
    </Transition>
  </div>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { confirm } from '@tauri-apps/plugin-dialog'
import { useDshConfigStore, type DshPortStatus } from '@/stores/dshConfig'
import { useConfigWorkspaceStore } from '@/stores/configWorkspace'
import {
  classifyPortConflict,
  describeInstallProgress,
  describeRuntimePortConflict,
  dshAccessLabel,
  isValidDshPort,
  type PortConflictKind,
} from '@/utils/dshRuntime'
import ConfigStatusBanner from '@/components/config/ConfigStatusBanner.vue'
import { useDshEmbed } from './useDshEmbed'

const props = defineProps<{
  /** True while this panel is the visible workspace surface. */
  active?: boolean
  /**
   * True while a DOM overlay that must cover the workspace is open (the global
   * settings popover, the top-bar order modal). The native WebView has to step
   * aside for these; it cannot be covered by them.
   */
  overlayOpen?: boolean
}>()
const emit = defineEmits<{
  (event: 'open-config'): void
  (event: 'left-width-change', width: number): void
}>()

const store = useDshConfigStore()
const workspaceStore = useConfigWorkspaceStore()
const placeholder = ref<HTMLElement | null>(null)
const { error: embedError, apply, hide, close, reload, refresh, reassert, suspend, resume }
  = useDshEmbed(placeholder, {
    // Native-control placement can only be validated on a real window, so the
    // geometry trace is available in dev builds only.
    diagnostics: import.meta.env.DEV,
  })

const hasError = computed(() => store.status?.phase === 'failed')
const errorAction = computed(() => store.status?.issue === 'stop_failed' ? '关闭' : '启动')

const progressText = computed(() => describeInstallProgress(store.progress))

/**
 * 进入界面时对「保存的端口」的探测结果；被占用时展示「清理并启动」卡片。
 * 与配置面板的 `portStatus` 刻意分开：那边跟着草稿走，这边只认保存值。
 */
const portConflict = ref<(DshPortStatus & { port: number }) | null>(null)
/** True while the entry auto-start flow (probe → start) is in flight. */
const autoStarting = ref(false)

const savedConfigLabel = computed(() => (
  `${dshAccessLabel(store.saved.access)} · 端口 ${store.saved.port}`
))

const conflictKind = computed<PortConflictKind>(() => (portConflict.value
  ? classifyPortConflict(portConflict.value, store.saved.access)
  : 'other-program'))

const conflictText = computed(() => {
  const conflict = portConflict.value
  if (!conflict) return ''
  return describeRuntimePortConflict({
    port: conflict.port,
    occupant: conflict.occupant,
    kind: conflictKind.value,
    occupantListenScope: conflict.occupantListenScope,
    savedAccess: store.saved.access,
    savedPort: store.saved.port,
  })
})

function openConfig() {
  emit('open-config')
}

/** 探测保存的端口；只在仍被占用时保留冲突卡片。 */
async function probeSavedPort() {
  if (!isValidDshPort(store.saved.port)) return
  const probe = await store.probePort(store.saved.access, store.saved.port)
  portConflict.value = probe && !probe.available
    ? { ...probe, port: store.saved.port }
    : null
}

async function startWithSaved() {
  await store.startWith(store.saved.access, store.saved.port)
  // 探测与绑定之间端口仍可能被别的程序抢走：把这种 port_in_use 失败也转成
  // 补救卡片，而不是一张只有「重试」的裸露错误。
  if (store.status?.phase === 'failed' && store.status.issue === 'port_in_use') {
    await probeSavedPort()
  }
}

/**
 * 进入 dsh 项目界面时按「保存的配置」自动启动。
 *
 * 触发点只有激活（挂载 / active 变为 true），不监听 stopped 状态本身——
 * 否则用户在配置页点「关闭」后切回来会被立刻重启。启动前先探测保存的端口：
 * 被占用时给「清理并启动」卡片，而不是等启动失败再报错；失败的启动不自动
 * 重试，交给错误卡片的「重试」这个明确的手动动作。
 */
async function ensureAutoStart() {
  if (autoStarting.value || store.isBusy) return
  autoStarting.value = true
  try {
    await store.load()
    await store.refreshStatus()
    const phase = store.status?.phase
    if (phase === 'running' || phase === 'preparing' || phase === 'starting') return
    if (phase === 'failed') {
      // 上次失败若就是端口占用，直接给补救卡片。
      if (store.status?.issue === 'port_in_use') await probeSavedPort()
      return
    }
    if (!isValidDshPort(store.saved.port)) return
    const probe = await store.probePort(store.saved.access, store.saved.port)
    if (probe && !probe.available) {
      portConflict.value = { ...probe, port: store.saved.port }
      return
    }
    portConflict.value = null
    await startWithSaved()
  } finally {
    autoStarting.value = false
  }
}

function manualStart() {
  void ensureAutoStart()
}

/**
 * 「清理并启动」：与配置页「一键清理占用」相同的确认文案与后端命令，区别是
 * 目标端口固定为保存的端口，且清理成功后立即按保存的配置启动。
 */
async function cleanupAndStart() {
  const conflict = portConflict.value
  if (!conflict) return
  const occupant = conflict.occupant ?? `端口 ${conflict.port} 上的进程`
  const warning = conflict.occupantIsSupervised
    ? '⚠ 注意：该进程由启动器启动，只是当前状态没有跟踪到它。\n\n'
    : conflict.occupantIsDsh
      ? '⚠ 注意：这是一个 dsh web 服务，可能就是你当前正在使用的那个。'
        + '清理它会立即中断该会话。\n\n'
      : ''
  const accepted = await confirm(
    `${warning}当前占用进程为 ${occupant}。\n\n`
    + `将强制结束该进程（含其子进程）以释放端口 ${conflict.port}，`
    + `随后按保存的配置（${dshAccessLabel(store.saved.access)} · 端口 ${store.saved.port}）启动 dsh。`
    + '该进程里未保存的内容会丢失。是否继续？',
    { title: '清理并启动', kind: 'warning' },
  )
  if (!accepted) return
  const report = await store.releasePort(conflict.port)
  // 失败原因已在 store.actionError，显示在卡片上。
  if (!report.released) return
  portConflict.value = null
  await startWithSaved()
}

async function retry() {
  portConflict.value = null
  await startWithSaved()
  if (store.isRunning) await apply(true)
}

// 浮层与模式切换的统一开关。
// 原生控件不是 DOM：既不随 `v-show` 消失，也不会被浮层盖住——它盖住浮层。
// 因此只要出现"这块区域不该由 dsh 界面占据"的情况，就必须先 suspend（隐藏并
// 锁住 show 路径），条件解除后再 resume。漏掉任何一条的表现都是"dsh 界面盖住
// 了别的界面/弹窗"。
const embedAllowed = computed(() => !!props.active
  && !props.overlayOpen
  // 二维码浮层挂在配置面板上但 Teleport 到 body，覆盖整窗，所以这里也要让位。
  && !store.qrVisible
  && !workspaceStore.preflightVisible)

watch(embedAllowed, (allowed) => {
  if (allowed) resume(true)
  else void suspend()
}, { immediate: true })

// 服务进入运行态：显示内嵌界面；停止或失败时立刻隐藏并销毁原生控件。
// 重启会换进程令牌，所以先 reload 再定位，否则旧页面会停在 401。
// URL 由 useDshLink 跟随同样的状态取回与丢弃。
watch(() => store.status?.phase, async (phase) => {
  if (phase === 'running') {
    portConflict.value = null
    await reload()
    await apply(true)
  } else {
    await hide()
    // A crashed service must not keep showing a dead page.
    if (phase === 'failed' || phase === 'stopped') await close()
  }
}, { immediate: true })

// 服务可能被外部因素终止（用户手动 kill、崩溃）。只在面板可见且服务在运行时
// 轮询：`dsh_runtime_status` 只做进程存活检查，不访问网络、不返回 token。
const STATUS_POLL_MS = 5000
let statusTimer: ReturnType<typeof setInterval> | null = null
function stopStatusPoll() {
  if (statusTimer !== null) {
    clearInterval(statusTimer)
    statusTimer = null
  }
}

watch(
  [() => props.active, () => store.status?.phase],
  ([active, phase]) => {
    stopStatusPoll()
    if (!active || phase !== 'running') return
    statusTimer = setInterval(() => {
      void store.refreshStatus()
    }, STATUS_POLL_MS)
  },
  { immediate: true },
)

onMounted(() => {
  if (props.active) void ensureAutoStart()
  else void store.load().then(() => store.refreshStatus())
})

// 切回本界面时同样走自动启动（已运行/启动中是廉价的 no-op）。
watch(() => props.active, (active) => {
  if (active) void ensureAutoStart()
})

onBeforeUnmount(() => {
  stopStatusPoll()
})

// 重新测量并「确认到稳定」：窗口缩放/全屏切换后布局往往还要再走几帧，
// 单次测量会得到过渡中的尺寸。App.vue 的 resize 与浮层关闭都走这里。
defineExpose({ hide, refresh, reassert, apply, suspend, resume })
</script>

<style scoped>
.dsh-runtime-panel {
  height: 100%;
  display: flex;
  flex-direction: column;
  min-height: 0;
  background: transparent;
}

.dsh-runtime-panel__empty,
.dsh-runtime-panel__error {
  flex: 1;
  min-height: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 18px;
}

.dsh-card {
  width: min(560px, 100%);
}

.dsh-note {
  margin: 6px 0 10px;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  line-height: 1.55;
}

.dsh-progress {
  margin: 6px 0;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
}

.dsh-detail {
  max-height: 180px;
  margin: 8px 0 0;
  padding: 8px;
  overflow: auto;
  border-radius: var(--radius-sm);
  background: rgba(0, 0, 0, 0.16);
  font-size: var(--font-size-small);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.dsh-runtime-panel__live {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.dsh-qr {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 10px 12px;
  border-bottom: 1px solid var(--separator);
  background: var(--tab-bg);
}

.dsh-qr img {
  width: 188px;
  height: 188px;
  border-radius: var(--radius-sm);
  background: #fff;
}

.dsh-qr p {
  margin: 4px 0 0;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  overflow-wrap: anywhere;
}

/* 占位区只负责布局测量：它内部的任何可见元素都会被原生控件盖住。 */
.dsh-embed-host {
  flex: 1;
  min-height: 0;
}

.dsh-fade-enter-active,
.dsh-fade-leave-active {
  transition: opacity 0.15s ease;
}

.dsh-fade-enter-from,
.dsh-fade-leave-to {
  opacity: 0;
}
</style>
