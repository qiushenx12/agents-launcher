<template>
  <div class="dsh-config-panel">
    <main class="config-content">
      <section class="card">
        <header class="editor-header">
          <div>
            <div class="card-title">DeepSeek Harness</div>
            <p>通过 npx 运行 dsh web，仅需设置访问范围与端口。</p>
          </div>
          <button
            class="btn btn-secondary"
            type="button"
            :disabled="store.loading"
            @click="store.refreshStatus()"
          >
            {{ store.statusChecking ? '刷新中…' : '刷新状态' }}
          </button>
        </header>

        <!-- 访问范围 -->
        <div class="field-row">
          <label class="field-label">访问范围</label>
          <div class="segmented" role="radiogroup" aria-label="访问范围">
            <button
              v-for="option in accessOptions"
              :key="option.value"
              class="segmented__item"
              :class="{ 'segmented__item--active': store.access === option.value }"
              type="button"
              role="radio"
              :aria-checked="store.access === option.value"
              @click="store.access = option.value"
            >
              {{ option.label }}
            </button>
          </div>
        </div>
        <p class="field-help">{{ accessHelp }}</p>

        <!-- 端口 -->
        <div class="field-row">
          <label class="field-label" for="dsh-port-input">端口</label>
          <input
            id="dsh-port-input"
            v-model.number="store.port"
            class="input"
            type="number"
            :min="DSH_PORT_MIN"
            :max="DSH_PORT_MAX"
            step="1"
            inputmode="numeric"
          >
        </div>
        <div class="field-help field-help--row">
          <span>{{ portHelp }}</span>
          <button
            class="btn btn-secondary field-help__action"
            type="button"
            :disabled="store.portChecking || store.releasing"
            @click="recheckPort"
          >
            {{ store.portChecking ? '检测中…' : '重新检测端口' }}
          </button>
        </div>

        <ConfigStatusBanner
          v-if="store.isRemote"
          message="远程模式会让同一网络内的其他设备访问该服务。dsh 不提供 TLS，任何拿到启动 URL（含 token）的人都能以你的身份操作这台机器上的 agent 与 shell。仅在可信网络中使用。"
          tone="warning"
        />
        <ConfigStatusBanner v-if="store.portError" :message="store.portError" tone="error" />
        <ConfigStatusBanner v-if="store.actionError" :message="store.actionError" tone="error" />
        <ConfigStatusBanner
          v-if="store.pendingRestart"
          :message="`端口或访问范围已保存，重启服务后生效（当前 ${runningDescription}）。`"
          tone="warning"
        />
        <ConfigStatusBanner v-if="store.actionNotice" :message="store.actionNotice" tone="success" />

        <!--
          端口被占用时的唯一一键清理入口。
          这里只负责确认，不做「杀掉安不安全」的判断——后端会拒绝结束启动器自身
          及其父进程，并把结果如实回报（released / selfProtected）。
        -->
        <div v-if="showPortCleanup" class="port-cleanup">
          <button
            class="btn btn-primary"
            type="button"
            :disabled="store.releasing"
            @click="cleanUpPort"
          >
            {{ store.releasing ? '清理中…' : '一键清理占用' }}
          </button>
          <span>{{ cleanupHint }}</span>
        </div>

        <!-- 运行状态 -->
        <div class="status-block">
          <div class="status-block__head">
            <span class="status-dot" :class="`status-dot--${statusTone}`" />
            <strong>{{ statusLabel }}</strong>
            <span v-if="store.status?.version" class="status-block__version">v{{ store.status.version }}</span>
          </div>
          <p v-if="store.status?.message" class="status-block__message">{{ store.status.message }}</p>
          <pre v-if="store.status?.detail" class="status-block__detail">{{ store.status.detail }}</pre>
          <div v-if="store.isBusy && store.progress" class="status-block__progress">
            {{ progressText }}
          </div>
        </div>

        <div class="action-row">
          <button
            class="btn btn-primary"
            type="button"
            :disabled="store.saving || !store.isDirty"
            @click="store.save()"
          >
            {{ store.saving ? '保存中…' : store.isDirty ? '保存设置' : '设置已保存' }}
          </button>
          <button
            v-if="!store.isRunning"
            class="btn btn-primary"
            type="button"
            :disabled="store.isBusy || !!store.portError || !store.loaded"
            @click="store.start()"
          >
            {{ store.isBusy ? '启动中…' : '启动' }}
          </button>
          <template v-else>
            <button class="btn btn-secondary" type="button" :disabled="store.isBusy" @click="store.restart()">
              重启
            </button>
            <button class="btn btn-secondary" type="button" :disabled="store.isBusy" @click="store.stop()">
              关闭
            </button>
          </template>
          <button
            class="btn btn-secondary"
            type="button"
            :disabled="!canCopy || store.isBusy"
            :title="copyHint"
            @click="copyCurrentLink"
          >
            {{ copied ? '已复制' : '复制链接' }}
          </button>
          <button
            v-if="store.isRemote"
            class="btn btn-secondary"
            type="button"
            :disabled="!canShowQr || store.isBusy"
            @click="store.openQr()"
          >
            二维码
          </button>
        </div>
        <p class="field-help field-help--action">
          {{ copyHint }}
        </p>

        <div class="preflight-entry">
          <button class="btn btn-secondary" type="button" @click="workspaceStore.openPreflight()">
            启动前检测
          </button>
          <span>在 dsh 标签页里查看界面。</span>
        </div>
      </section>

      <!-- 局域网二维码：与启动前检测一样是 Teleport 浮层，Rust 侧会先隐藏子 WebView -->
      <Teleport to="body">
        <div
          v-if="store.qrVisible"
          class="dsh-qr-overlay"
          role="presentation"
          @click.self="store.closeQr()"
        >
          <section class="dsh-qr-card" role="dialog" aria-modal="true" aria-labelledby="dsh-qr-title">
            <header class="dsh-qr-card__header">
              <h2 id="dsh-qr-title">扫码打开局域网界面</h2>
              <button
                class="dsh-qr-card__close"
                type="button"
                title="关闭"
                aria-label="关闭二维码"
                @click="store.closeQr()"
              >
                ×
              </button>
            </header>
            <img v-if="qrDataUrl" :src="qrDataUrl" alt="局域网访问二维码" width="220" height="220">
            <p v-else class="dsh-qr-card__pending">正在生成二维码…</p>
            <code class="dsh-qr-card__url">{{ remoteUrl }}</code>
            <p class="dsh-qr-card__warning">
              二维码与链接都包含访问令牌，任何拿到它的人都能以你的身份操作这台机器上的 agent 与 shell。
              请仅在可信网络中分享。
            </p>
          </section>
        </div>
      </Teleport>

      <!-- 凭据说明：不做检测，也不弹引导 -->
      <section class="card">
        <div class="card-title">模型与凭据</div>
        <p class="source-note">
          DeepSeek Harness 自行管理模型与 API Key，启动器不读取也不写入。
          凭据按以下优先级解析：环境变量（如 <code>DEEPSEEK_API_KEY</code>）→
          <code>$DSH_HOME/.credentials.yaml</code> → 启动目录 <code>.env</code> →
          <code>$DSH_HOME/.env</code>；也可以在 dsh 界面内的设置页配置。
        </p>
        <p class="source-note">
          启动器与手动运行的 <code>dsh web</code> 共用同一份 <code>$DSH_HOME</code>，
          因此会话、凭据与登录态是同一套。
        </p>
      </section>
    </main>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { confirm } from '@tauri-apps/plugin-dialog'
import {
  DSH_DEFAULT_PORT,
  DSH_PORT_MAX,
  DSH_PORT_MIN,
  useDshConfigStore,
  type DshAccess,
} from '@/stores/dshConfig'
import { describeCopyTarget, describeInstallProgress, describePortOccupant, hasAccessToken } from '@/utils/dshRuntime'
import { useConfigWorkspaceStore } from '@/stores/configWorkspace'
import ConfigStatusBanner from '@/components/config/ConfigStatusBanner.vue'
import { useDshLink } from './useDshLink'
import { describePortCleanupHint, shouldOfferPortCleanup } from './portCleanup'

const store = useDshConfigStore()
const workspaceStore = useConfigWorkspaceStore()

/**
 * 「复制链接」按访问范围选择要复制的 URL：本地 → 回环地址；远程 → 局域网地址。
 * 两者都带 `?token=`，否则打开的人只会看到 dsh 的 401 页面。
 */
const { canCopy, copied, copyUrl, copyLink, urls } = useDshLink({
  access: computed(() => store.access),
  running: computed(() => store.isRunning),
  onError: (message) => { store.setActionError(message) },
})

const copyHint = computed(() => (canCopy.value
  ? describeCopyTarget(store.access, copyUrl.value)
  : '服务运行后才能复制链接，链接中已包含访问令牌'))

function copyCurrentLink() {
  void copyLink()
}

/**
 * 二维码只在「远程」模式下有意义——本地回环地址手机扫了也打不开，所以必须同时
 * 拿到带令牌的局域网 URL 才允许打开。
 */
const remoteUrl = computed(() => urls.value?.remoteUrl ?? null)
const canShowQr = computed(() => hasAccessToken(remoteUrl.value))
const qrDataUrl = ref('')

async function renderQr() {
  if (!store.qrVisible || !canShowQr.value || !remoteUrl.value) {
    qrDataUrl.value = ''
    return
  }
  const { toDataURL } = await import('qrcode')
  qrDataUrl.value = await toDataURL(remoteUrl.value, { margin: 1, width: 220 })
}

watch([() => store.qrVisible, remoteUrl], () => {
  void renderQr().catch(() => { qrDataUrl.value = '' })
})

// 切回「本地」模式后二维码没有意义，别把它留在屏幕上。
watch(() => store.isRemote, (remote) => {
  if (!remote) store.closeQr()
})

/**
 * 「一键清理占用」的显示条件。
 *
 * 不要求端口是草稿状态：把端口改成一个被占用的值、或当前端口被别人抢走，同样
 * 需要这个入口（提示文案会同时建议改用其它端口）。唯一排除的情况是启动器托管的
 * 服务正在运行——那时占用者就是它自己，该点「关闭」。
 *
 * 判定逻辑在 `portCleanup.ts` 里，便于直接单测——历史上正是这个条件把按钮整块
 * 藏掉了。
 */
const showPortCleanup = computed(() => shouldOfferPortCleanup({
  portStatus: store.portStatus,
  isRunning: store.isRunning,
  releasing: store.releasing,
}))

const cleanupHint = computed(() => describePortCleanupHint({
  occupantIsDsh: !!store.portStatus?.occupantIsDsh,
  occupantIsSupervised: !!store.portStatus?.occupantIsSupervised,
}))

/**
 * 破坏性操作必须先确认，并且把「要杀谁」和「会有什么后果」写清楚。
 * 文案与其它入口一致：先给「当前占用进程为 xxx」，再给后果。占用者是 dsh 时
 * 额外加重提示：很可能正是用户此刻正在对话的实例；由启动器自己拉起、只是状态
 * 没跟踪到时也要说明，否则用户会以为那是别人的进程。
 */
async function cleanUpPort() {
  const occupant = store.portStatus?.occupant ?? `端口 ${store.port} 上的进程`
  const isSupervised = !!store.portStatus?.occupantIsSupervised
  const isDsh = !!store.portStatus?.occupantIsDsh
  const warning = isSupervised
    ? '⚠ 注意：该进程由启动器启动，只是当前状态没有跟踪到它。\n\n'
    : isDsh
      ? '⚠ 注意：这是一个 dsh web 服务，可能就是你当前正在使用的那个。'
        + '清理它会立即中断该会话。\n\n'
      : ''
  const accepted = await confirm(
    `${warning}当前占用进程为 ${occupant}。\n\n`
    + `将强制结束该进程（含其子进程）以释放端口 ${store.port}。`
    + '该进程里未保存的内容会丢失。是否继续？',
    { title: '清理端口占用', kind: 'warning' },
  )
  if (!accepted) return
  await store.releasePort()
}

const accessOptions: Array<{ value: DshAccess; label: string }> = [
  { value: 'local', label: '本地' },
  { value: 'remote', label: '远程' },
]

const accessHelp = computed(() => (
  store.isRemote
    ? '监听 0.0.0.0，同一局域网内的其它设备可以访问。'
    : '仅监听 127.0.0.1，只有这台电脑可以访问。'
))

const portHelp = computed(() => {
  if (store.portChecking) return '正在检测端口占用情况…'
  const portStatus = store.portStatus
  if (!portStatus) return `dsh 默认端口为 ${DSH_DEFAULT_PORT}；端口被占用时不会自动更换。`
  if (portStatus.available) {
    return store.isDirty
      ? `端口 ${store.port} 可用。修改后需要重新启动服务才能生效。`
      : `端口 ${store.port} 可用。`
  }
  // 占用时先说清是谁（与其它入口同一句文案），再把用户直接指向操作入口。
  return `端口 ${store.port} 已被占用，${describePortOccupant(portStatus.occupant, store.port)}`
    + '可用下方「一键清理占用」结束它，或改用其它端口。'
})

const statusTone = computed(() => {
  switch (store.status?.phase) {
    case 'running': return 'running'
    case 'failed': return 'failed'
    case 'preparing':
    case 'starting': return 'pending'
    default: return 'stopped'
  }
})

const statusLabel = computed(() => {
  switch (store.status?.phase) {
    case 'running': return '运行中'
    case 'failed': return '启动失败'
    case 'preparing': return '准备中'
    case 'starting': return '启动中'
    default: return '未启动'
  }
})

const runningDescription = computed(() => {
  const status = store.status
  if (!status) return '未知'
  return `${status.access === 'remote' ? '远程' : '本地'} · 端口 ${status.port}`
})

/**
 * 冷缓存与缓存命中的文案必须分开：命中时字节几乎不增长，显示 0 MB/s 会误导。
 * 采样到的是 tarball 落盘量，所以不承诺"精确网速"。
 */
const progressText = computed(() => describeInstallProgress(store.progress))

onMounted(() => {
  // 挂载时必须主动探测一次：否则 portStatus 保持 null，占用提示与「一键清理占用」
  // 都不会渲染，而这正是用户进配置页要找的东西。
  void store.load()
    .then(() => store.refreshStatus())
    .then(() => store.checkPort())
})

/**
 * 端口占用探测的触发点。
 *
 * 这里必须覆盖"刚进入配置页"和"服务状态变化"两个时机：只在草稿端口变化时探测
 * 会让 portStatus 保持 null，于是占用提示与「一键清理占用」整块都不渲染——
 * 而那正是用户最需要它的时候。
 *
 * 只在 dsh 配置页可见时探测，避免在别的 CLI 面板背后白跑。
 */
watch(
  [
    () => workspaceStore.activeKind,
    () => store.port,
    () => store.access,
    () => store.status?.phase,
  ],
  () => {
    if (workspaceStore.activeKind !== 'dsh') return
    void store.checkPort()
  },
  { immediate: true },
)

// 手动重新检测：探测失败或端口刚被别的程序释放时不必切页重进。
function recheckPort() {
  void store.checkPort()
}
</script>

<style scoped>
.dsh-config-panel {
  height: 100%;
  display: flex;
}

/* dsh has no profile sidebar, so the recessed editor pane supplies that band
   itself: the 28px inset in the app background on the left stands in for the
   light rail the other configuration workspaces get from their sidebar list.
   The 12px top margin keeps the chrome above the recess, and the interior top
   padding is trimmed by the same amount so the first row keeps its offset. */
.config-content {
  min-width: 0;
  flex: 1;
  height: 100%;
  margin-top: var(--editor-pane-inset, 12px);
  padding: 0 16px 12px 28px;
  overflow-y: auto;
  border-top-left-radius: var(--radius-lg, 12px);
}

.card {
  max-width: 760px;
  margin: 0 auto 12px;
}

.editor-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 10px;
}

.editor-header p {
  margin: 4px 0 0;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
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

.field-row > .input {
  min-width: 0;
  flex: 1;
}

.field-help {
  margin: 2px 0 8px 120px;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
}

/* 操作行下方的说明：与字段行左对齐（120px + 10px 间距），不缩进到标签列。 */
.field-help--action {
  margin: 6px 0 0;
}

/* 端口说明 + 重新检测：占用状态可能随时变化，给一个显式入口。 */
.field-help--row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 2px 0 8px 120px;
}

.field-help--row > span {
  min-width: 0;
  flex: 1;
}

.field-help__action {
  flex: 0 0 auto;
  padding: 2px 8px;
  font-size: var(--font-size-small);
}

.segmented {
  display: inline-flex;
  border: 1px solid var(--separator);
  border-radius: var(--radius-sm);
  overflow: hidden;
}

.segmented__item {
  padding: 5px 14px;
  border: 0;
  color: var(--text-primary);
  background: transparent;
  cursor: pointer;
  font-size: var(--font-size-small);
}

.segmented__item + .segmented__item {
  border-left: 1px solid var(--separator);
}

.segmented__item--active {
  color: #fff;
  background: var(--primary);
}

.status-block {
  margin: 4px 0 12px;
  padding: 10px 12px;
  border: 1px solid var(--separator);
  border-radius: var(--radius-md);
  background: var(--tab-bg);
}

.status-block__head {
  display: flex;
  align-items: center;
  gap: 8px;
}

.status-block__version {
  color: var(--text-secondary);
  font-size: var(--font-size-small);
}

.status-block__message {
  margin: 6px 0 0;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  line-height: 1.5;
}

.status-block__detail {
  max-height: 160px;
  margin: 8px 0 0;
  padding: 8px;
  overflow: auto;
  border-radius: var(--radius-sm);
  background: rgba(0, 0, 0, 0.16);
  font-size: var(--font-size-small);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.status-block__progress {
  margin-top: 8px;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
}

.status-dot {
  width: 8px;
  height: 8px;
  flex: 0 0 auto;
  border-radius: 50%;
  background: var(--text-secondary);
}

.status-dot--running { background: var(--success, #22c55e); }
.status-dot--failed { background: var(--danger, #d96c6c); }
.status-dot--pending { background: var(--warning, #d49a45); }

.action-row {
  display: flex;
  gap: 8px;
  margin-top: 6px;
  padding-top: 10px;
  border-top: 1px solid var(--separator);
}

.preflight-entry {
  margin-top: 10px;
  display: flex;
  align-items: center;
  gap: 10px;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
}

.source-note {
  margin: 6px 0;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  line-height: 1.55;
}

.source-note code {
  padding: 1px 4px;
  border-radius: 3px;
  background: var(--tab-bg);
}

/* 端口占用时的清理行：按钮 + 后果说明。 */
.port-cleanup {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 0 0 10px;
  padding: 10px 12px;
  border: 1px solid var(--warning, #b26a00);
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--warning, #b26a00) 8%, transparent);
}

.port-cleanup .btn {
  flex: 0 0 auto;
}

.port-cleanup span {
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  line-height: 1.45;
}

/* 二维码浮层：与启动前检测同层，Teleport 到 body 覆盖全屏（含标题栏）。 */
.dsh-qr-overlay {
  position: fixed;
  inset: 0;
  z-index: 1200;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.45);
}

.dsh-qr-card {
  width: min(420px, calc(100vw - 40px));
  padding: 16px 18px 18px;
  border: 1px solid var(--separator);
  border-radius: var(--radius-md);
  background: var(--card-bg, var(--tab-bg));
  text-align: center;
}

.dsh-qr-card__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 12px;
}

.dsh-qr-card__header h2 {
  margin: 0;
  font-size: 1rem;
}

.dsh-qr-card__close {
  border: 0;
  color: var(--text-secondary);
  background: transparent;
  cursor: pointer;
  font-size: 20px;
  line-height: 1;
}

.dsh-qr-card img {
  width: 220px;
  height: 220px;
  border-radius: var(--radius-sm);
  background: #fff;
}

.dsh-qr-card__pending {
  margin: 0;
  padding: 90px 0;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
}

.dsh-qr-card__url {
  display: block;
  margin-top: 12px;
  overflow-wrap: anywhere;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
}

.dsh-qr-card__warning {
  margin: 12px 0 0;
  color: var(--warning, #b26a00);
  font-size: var(--font-size-small);
  line-height: 1.5;
  text-align: left;
}
</style>
