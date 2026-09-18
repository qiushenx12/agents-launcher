<template>
  <section class="card">
    <header class="editor-header">
      <div class="card-title">启动设置</div>
      <button
        class="btn btn-secondary"
        type="button"
        :disabled="store.loading"
        @click="refreshStatus()"
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

    <!--
      端口：输入框收窄，「重新检测端口」在同一行最右侧。不再单独输出一行
      提示文字——占用/非法端口仍由下方 banner 与「一键清理占用」区呈现。
    -->
    <div class="field-row">
      <label class="field-label" for="dsh-port-input">端口</label>
      <input
        id="dsh-port-input"
        v-model.number="store.port"
        class="input input--port"
        type="number"
        :min="DSH_PORT_MIN"
        :max="DSH_PORT_MAX"
        step="1"
        inputmode="numeric"
      >
      <button
        class="btn btn-secondary field-row__port-action"
        type="button"
        :disabled="store.portChecking || store.releasing"
        @click="recheckPort"
      >
        {{ store.portChecking ? '检测中…' : '重新检测端口' }}
      </button>
    </div>

    <!--
      固定版本：每次启动成功后后端会记录当时实际运行的版本，下次启动用
      npx 按该版本启动（命中本地缓存，不再每次解析 latest）。「检查更新」
      是唯一会重新解析 latest 的入口：先只读查询，发现新版本时弹窗确认，
      确认后才固定；运行中的服务不会被隐式重启，只标注重启后生效。
    -->
    <div class="field-row">
      <label class="field-label">版本</label>
      <div class="version-row">
        <span class="version-row__value">{{ pinnedVersionLabel }}</span>
        <button
          class="btn btn-secondary"
          type="button"
          :disabled="store.updatingVersion || store.isBusy"
          @click="checkForUpdate"
        >
          {{ store.updatingVersion ? '正在检查更新…' : '检查更新' }}
        </button>
      </div>
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
      :message="pendingRestartText"
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
      <p v-if="statusMessage" class="status-block__message">{{ statusMessage }}</p>
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
          {{ store.stopping ? '关闭中…' : '关闭' }}
        </button>
      </template>
    </div>

    <!--
      访问地址清单：同一台机器可能同时有局域网地址和 Tailscale 地址，而 dsh 只
      上报其中一个（装了 Tailscale 的机器上常常正是 100.x，别的设备反而打不开）。
      所以这里把本机 / 局域网 / Tailscale 全部列出来，每行自带
      「二维码 / 复制 / 打开网页」，三个动作都作用于这一行的地址。
      「默认」标出最可能想用的那一个（局域网优先，其次 Tailscale）。
    -->
    <div class="address-list">
      <div class="address-list__head">
        <span class="address-list__title">访问地址</span>
        <span v-if="rows.length" class="address-list__note">链接含访问令牌，请勿外发</span>
      </div>
      <ul v-if="rows.length" class="address-list__items">
        <li v-for="row in rows" :key="row.url" class="address-list__item">
          <span class="address-list__kind" :class="`address-list__kind--${row.kind}`">
            {{ row.label }}
          </span>
          <code class="address-list__address">{{ row.display }}</code>
          <span v-if="row.interface" class="address-list__interface">{{ row.interface }}</span>
          <span v-if="row.preferred" class="address-list__badge">默认</span>
          <!--
            二维码只给别的设备能打开的地址：回环地址手机扫了也进不去，所以那里
            不出现这个按钮。
          -->
          <button
            v-if="needsQr(row)"
            class="btn btn-secondary address-list__action"
            type="button"
            title="用二维码分享这个地址（含访问令牌）"
            @click="openQr(row)"
          >
            二维码
          </button>
          <button
            class="btn btn-secondary address-list__action"
            type="button"
            :title="describeCopyTarget(row)"
            @click="copyRowLink(row)"
          >
            {{ copiedKey === row.url ? '已复制' : '复制' }}
          </button>
          <button
            class="btn btn-secondary address-list__action"
            type="button"
            :title="`在默认浏览器中打开 ${row.display}（链接中已包含访问令牌）`"
            @click="openRowLink(row)"
          >
            打开网页
          </button>
        </li>
      </ul>
      <p v-else class="address-list__empty">{{ addressHint }}</p>
    </div>

    <!--
      跳转 dsh 自己界面的入口。紫色在配置页里一眼可辨：它不是常规的主/次操作，
      也不是警告——只是把用户带去 dsh 的界面。
      （左下角的「设置」入口不在这里：它跟着侧边栏页脚走，与其它前端一致。）
    -->
    <div class="runtime-entry">
      <button class="btn runtime-entry__button" type="button" @click="emit('open-runtime')">
        进入DeepSeek Harness
      </button>
    </div>

    <!-- 局域网二维码：Teleport 浮层覆盖全窗，Rust 侧会先隐藏子 WebView -->
    <Teleport to="body">
      <div
        v-if="store.qrVisible"
        class="dsh-qr-overlay"
        role="presentation"
        @click.self="store.closeQr()"
      >
        <section class="dsh-qr-card" role="dialog" aria-modal="true" aria-labelledby="dsh-qr-title">
          <header class="dsh-qr-card__header">
            <h2 id="dsh-qr-title">扫码打开{{ qrKindSuffix }}</h2>
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
          <img v-if="qrDataUrl" :src="qrDataUrl" alt="访问地址二维码" width="220" height="220">
          <p v-else class="dsh-qr-card__pending">正在生成二维码…</p>
          <code class="dsh-qr-card__url">{{ qrTarget?.display }}</code>
          <p class="dsh-qr-card__warning">
            二维码与链接都包含访问令牌，任何拿到它的人都能以你的身份操作这台机器上的 agent 与 shell。
            请仅在可信网络中分享。
          </p>
        </section>
      </div>
    </Teleport>
  </section>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { confirm } from '@tauri-apps/plugin-dialog'
import { open } from '@tauri-apps/plugin-shell'
import {
  DSH_PORT_MAX,
  DSH_PORT_MIN,
  useDshConfigStore,
  type DshAccess,
} from '@/stores/dshConfig'
import { describeCopyTarget, describeInstallProgress, formatUptimeZh, type DshAddressRow } from '@/utils/dshRuntime'
import { useConfigWorkspaceStore } from '@/stores/configWorkspace'
import ConfigStatusBanner from '@/components/config/ConfigStatusBanner.vue'
import { useDshLink } from './useDshLink'
import { describePortCleanupHint, shouldOfferPortCleanup } from './portCleanup'

const store = useDshConfigStore()
const workspaceStore = useConfigWorkspaceStore()

const emit = defineEmits<{
  /** 「进入DeepSeek Harness」：由 App.vue 走与顶栏 CLI 标签相同的路径跳转。 */
  (event: 'open-runtime'): void
}>()

/**
 * 访问地址清单：后端列出服务实际监听的每个地址（本机 / 局域网 / Tailscale），
 * 每行自带「二维码 / 复制 / 打开网页」，三个动作都作用于这一行的地址。
 *
 * 不再有全局的复制/打开按钮：同一台机器上「想复制哪一个」本来就取决于接收方
 * 在哪张网里，让用户对着地址选比让按钮猜更可靠。
 */
const { copiedKey, copyAddress, loadUrls, resolveAddress, rows } = useDshLink({
  running: computed(() => store.isRunning),
  onError: (message) => { store.setActionError(message) },
})

/** 地址清单为空时的说明：服务没起来时它只是还没内容，不是出错了。 */
const addressHint = computed(() => (store.isRunning
  ? '正在读取访问地址…'
  : '服务运行后这里会列出本机、局域网与 Tailscale 地址。'))

function copyRowLink(row: DshAddressRow) {
  void copyAddress(row)
}

/** 行内的「打开网页」：打开这一行的地址，本机地址就是用本机浏览器打开。 */
async function openRowLink(row: DshAddressRow) {
  store.clearActionError()
  // 服务重启会换令牌，所以先把地址重新解析一次再打开，避免打开一个 401 页面。
  const value = await resolveAddress(row)
  if (!value) return
  try {
    await open(value)
  } catch {
    // Some platform errors echo the full target. Keep the token-bearing URL out
    // of UI errors and diagnostics even when the system browser cannot open it.
    store.setActionError('打开网页失败，请检查系统默认浏览器设置。')
  }
}

/**
 * 二维码按行打开：只有别的设备能打开的地址（局域网 / Tailscale）才需要它——回环
 * 地址手机扫了也进不去，所以那里不显示按钮。
 */
const qrTarget = ref<DshAddressRow | null>(null)
const qrDataUrl = ref('')

function needsQr(row: DshAddressRow): boolean {
  return row.kind === 'lan' || row.kind === 'tailscale'
}

/** 「局域网」用引号包住，Tailscale 是拉丁产品名、需要前后空格。 */
const qrKindSuffix = computed(() => {
  const row = qrTarget.value
  if (!row) return '地址'
  return row.kind === 'tailscale' ? ` ${row.label} 地址` : `「${row.label}」地址`
})

function openQr(row: DshAddressRow) {
  qrTarget.value = row
  store.openQr()
}

async function renderQr() {
  const url = qrTarget.value?.url
  if (!store.qrVisible || !url) {
    qrDataUrl.value = ''
    return
  }
  const { toDataURL } = await import('qrcode')
  qrDataUrl.value = await toDataURL(url, { margin: 1, width: 220 })
}

watch([() => store.qrVisible, qrTarget], () => {
  void renderQr().catch(() => { qrDataUrl.value = '' })
})

// 浮层关掉后立刻丢掉二维码指向的地址：它是带令牌的，没有理由留在组件状态里。
watch(() => store.qrVisible, (visible) => {
  if (!visible) qrTarget.value = null
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
    case 'failed': return store.status?.issue === 'stop_failed' ? '关闭失败' : '启动失败'
    case 'preparing': return '准备中'
    case 'starting': return '启动中'
    default: return '未启动'
  }
})

// 「已运行 …」实时刷新：后端只随状态给 uptimeSecs 快照（与 statusFetchedAt
// 配对），停在本界面时由这里每秒推进本地时钟，时长会一直走。显示格式随量级
// 扩展：5秒 → 1分05秒 → 1小时02分03秒 → 1天02小时03分04秒。
const uptimeTick = ref(Date.now())
let uptimeTimer: ReturnType<typeof setInterval> | null = null

const uptimeActive = computed(() => (
  store.status?.phase === 'running' && store.status.uptimeSecs != null
))

const liveUptimeText = computed(() => {
  const status = store.status
  if (!status || !uptimeActive.value) return ''
  const extra = Math.max(0, Math.floor((uptimeTick.value - store.statusFetchedAt) / 1000))
  return formatUptimeZh((status.uptimeSecs ?? 0) + extra)
})

/**
 * 状态消息行。后端的 message 是完整句（带句号）；运行中把实时「已运行 …」
 * 插到句号前，保持「DeepSeek Harness 正在运行（已运行 1分05秒）。」的读法。
 */
const statusMessage = computed(() => {
  const status = store.status
  if (!status?.message) return ''
  const uptime = liveUptimeText.value
  if (!uptime) return status.message
  const base = status.message.endsWith('。') ? status.message.slice(0, -1) : status.message
  return `${base}（已运行 ${uptime}）。`
})

watch(uptimeActive, (active) => {
  if (uptimeTimer !== null) {
    clearInterval(uptimeTimer)
    uptimeTimer = null
  }
  if (active) {
    uptimeTick.value = Date.now()
    uptimeTimer = setInterval(() => { uptimeTick.value = Date.now() }, 1000)
  }
}, { immediate: true })

onBeforeUnmount(() => {
  if (uptimeTimer !== null) {
    clearInterval(uptimeTimer)
    uptimeTimer = null
  }
})

const runningDescription = computed(() => {
  const status = store.status
  if (!status) return '未知'
  return `${status.access === 'remote' ? '远程' : '本地'} · 端口 ${status.port}`
})

/**
 * 版本行的显示值。固定版本来自持久化配置（不是运行状态）：服务停止时也要
 * 能看到下次启动会用哪个版本；运行中的实际版本仍由下方状态块展示。
 *
 * 说明直接写进值里（原来它在行的下方占一整行 help）：未固定时讲清「首次启动
 * 取最新版」，固定后讲清「已固定」——两句话都是这一行唯一的未知量。
 */
const pinnedVersionLabel = computed(() => (
  store.saved.pinnedVersion
    ? `v${store.saved.pinnedVersion}（已固定）`
    : '未固定 · 首次启动取最新版'
))

const runningVersionDescription = computed(() => (
  store.status?.version ? `v${store.status.version}` : '旧版本'
))

/**
 * 重启提示按原因区分：配置改动沿用原文案；版本更新则指出正在运行的还是旧
 * 版本（status.version 是本次启动时记录的版本，正是要换掉的那个）。
 */
const pendingRestartText = computed(() => (
  store.pendingRestartReason === 'version'
    ? `新版本 v${store.saved.pinnedVersion} 已记录，重启服务后生效（当前运行 ${runningVersionDescription.value}）。`
    : `端口或访问范围已保存，重启服务后生效（当前 ${runningDescription.value}）。`
))

/**
 * 「检查更新」的完整流程：先只读查询注册表，已是最新时只提示不弹窗；发现
 * 新版本时弹窗确认（写清会从哪个版本换到哪个版本、运行中的服务不受影响、
 * 重启后生效），确认后才固定。弹窗里展示的就是即将固定的版本，后端按确认
 * 值写入，不会在确认与写入之间重新解析 latest。
 */
async function checkForUpdate() {
  const report = await store.checkUpdate()
  if (!report || !report.updateAvailable) return
  const pinned = report.pinned ? `v${report.pinned}` : '未固定'
  const accepted = await confirm(
    `npm 注册表最新版本为 v${report.latest}，当前固定版本为 ${pinned}。\n\n`
    + (store.isRunning
      ? '确认后将固定到新版本；正在运行的服务不受影响，重启 dsh 服务后生效。'
      : '确认后将固定到新版本，下次启动时使用。')
    + '是否更新？',
    { title: '更新 dsh 版本', kind: 'info' },
  )
  if (!accepted) return
  await store.applyVersion(report.latest)
}

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
 * 只在 dsh 配置页的「启动设置」可见时探测，避免在别的面板背后白跑。
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
    // 地址清单同样要跟着这两个时机刷新：接口随时可能增减（插网线、连 Wi-Fi、
    // Tailscale 上线/下线），停在旧地址上会让用户复制到一个已经不可达的链接。
    // 服务没在跑时 loadUrls 自己会清空，不需要在这里判断。
    void loadUrls()
  },
  { immediate: true },
)

// 手动重新检测：探测失败或端口刚被别的程序释放时不必切页重进。
function recheckPort() {
  void store.checkPort()
}

/** 「刷新状态」：状态与地址清单一起重取，两者都是"当前现实"。 */
function refreshStatus() {
  void store.refreshStatus()
  void loadUrls()
}
</script>

<style scoped>
/*
  「启动设置」面板是与编辑窗格同色的卡片，缩进与其它 CLI 的编辑器 pane 一致：
  dsh 的模型页有侧边栏，这个 pane 与它共享同一条左缘，所以不再自带 28px 的
  装饰性缩进。
*/
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
  text-align: left;
}

.field-row > .input {
  min-width: 0;
  flex: 1;
}

/* 端口行：输入框收窄，「重新检测端口」放同一行最右侧（margin-left: auto
   吃掉中间的剩余空间），提示文字不再单独占一行。 */
.field-row > .input--port {
  flex: 0 0 auto;
  width: 110px;
}

.field-row__port-action {
  margin-left: auto;
  padding: 2px 8px;
  font-size: var(--font-size-small);
}

/* 版本行：当前固定版本 + 「更新版本」按钮，与端口行的操作位对齐。 */
.version-row {
  min-width: 0;
  flex: 1;
  display: flex;
  align-items: center;
  gap: 10px;
}

.version-row__value {
  min-width: 0;
  flex: 1;
  color: var(--text-primary);
  font-size: var(--font-size-small);
}

.version-row .btn {
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

.runtime-entry {
  margin-top: 10px;
  display: flex;
  justify-content: flex-end;
}

/* 与主色蓝、成功绿、警告/危险都拉开距离的紫色：它是跳转入口而非操作。 */
.runtime-entry__button {
  background-color: #6B5CE7;
  color: #FFFFFF;
}

.runtime-entry__button:hover:not(:disabled),
.runtime-entry__button:active:not(:disabled) {
  background-color: #5848C9;
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

/*
  访问地址清单：一个地址一行，复制按钮固定在这一行末尾，所以「复制哪一个」永远
  是行内唯一按钮，不需要先选中再复制。
*/
.address-list {
  margin-top: 10px;
  padding: 10px 12px;
  border: 1px solid var(--separator);
  border-radius: var(--radius-md);
  background: var(--tab-bg);
}

.address-list__head {
  display: flex;
  align-items: baseline;
  flex-wrap: wrap;
  gap: 4px 8px;
  margin-bottom: 4px;
}

.address-list__title {
  color: var(--text-primary);
  font-size: var(--font-size-small);
}

.address-list__note,
.address-list__empty {
  margin: 0;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  line-height: 1.5;
}

.address-list__items {
  margin: 0;
  padding: 0;
  list-style: none;
}

.address-list__item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 0;
}

.address-list__item + .address-list__item {
  border-top: 1px solid var(--separator);
}

/* 分组标签：本机 / 局域网 / Tailscale / 其它网络。 */
.address-list__kind {
  flex: 0 0 auto;
  width: 68px;
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  background: color-mix(in srgb, var(--text-secondary) 18%, transparent);
  font-size: var(--font-size-small);
  text-align: center;
}

.address-list__kind--lan {
  color: var(--success, #22c55e);
  background: color-mix(in srgb, var(--success, #22c55e) 16%, transparent);
}

.address-list__kind--tailscale {
  color: var(--primary);
  background: color-mix(in srgb, var(--primary) 16%, transparent);
}

.address-list__address {
  flex: 1;
  min-width: 0;
  overflow-wrap: anywhere;
  color: var(--text-primary);
  font-size: var(--font-size-small);
}

.address-list__interface {
  flex: 0 1 auto;
  max-width: 140px;
  overflow: hidden;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.address-list__badge {
  flex: 0 0 auto;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
}

.address-list__action {
  flex: 0 0 auto;
  padding: 2px 10px;
  font-size: var(--font-size-small);
}

/* 二维码浮层：Teleport 到 body 覆盖全屏（含标题栏）。 */
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
