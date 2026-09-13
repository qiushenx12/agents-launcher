<template>
  <div class="dsh-runtime-panel">
    <!--
      浮层清单（§4.9 A 组）：子 WebView 是盖在 DOM 之上的原生控件，任何
      fixed / Teleport 浮层都必须先让它隐藏，否则浮层整个看不见。
      让位的条件集中在下面 `embedAllowed` 一处：面板是否当前表面 +
      App.vue 的 dshOverlayOpen（全局设置浮层、顶栏排序弹窗）+ 二维码浮层 +
      启动前检测。新增浮层时加进这一处即可，不要另写 hide 调用。
    -->
    <Transition name="dsh-fade">
      <div v-if="hasError" class="dsh-runtime-panel__error">
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

      <div v-else-if="!store.isRunning" class="dsh-runtime-panel__empty">
        <div class="card dsh-card">
          <div class="card-title">DeepSeek Harness 未启动</div>
          <p class="dsh-note">
            服务启动后会在这里内嵌显示 dsh 界面；界面由 dsh 自己提供，本应用只负责进程与承载。
          </p>
          <div v-if="store.isBusy" class="dsh-progress">
            {{ progressText }}
          </div>
          <ConfigStatusBanner v-if="store.actionError" :message="store.actionError" tone="error" />
          <div class="action-row">
            <button
              class="btn btn-primary"
              type="button"
              :disabled="store.isBusy || !!store.portError || !store.loaded"
              @click="store.start()"
            >
              {{ store.isBusy ? '启动中…' : '启动' }}
            </button>
            <button class="btn btn-secondary" type="button" @click="openConfig()">
              打开配置
            </button>
          </div>
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
import { useDshConfigStore } from '@/stores/dshConfig'
import { useConfigWorkspaceStore } from '@/stores/configWorkspace'
import { describeInstallProgress } from '@/utils/dshRuntime'
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

function openConfig() {
  emit('open-config')
}

async function retry() {
  await store.start()
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
  void store.load().then(() => store.refreshStatus())
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
