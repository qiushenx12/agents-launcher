import { computed, reactive, ref } from 'vue'
import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  DSH_DEFAULT_PORT,
  DSH_PORT_MAX,
  DSH_PORT_MIN,
  describePortOccupant,
  isValidDshPort,
  normalizeDshAccess,
  type DshAccess,
  type DshRuntimeUrls,
} from '@/utils/dshRuntime'

/**
 * DeepSeek Harness runtime settings.
 *
 * Only the two persisted settings (`access` / `port`) plus their draft state and
 * the derived runtime status live here. Token-bearing URLs are deliberately
 * absent: they stay in component-local state and are never stored.
 */

export type { DshAccess, DshRuntimeUrls }
export { DSH_DEFAULT_PORT, DSH_PORT_MAX, DSH_PORT_MIN, isValidDshPort }

export type DshPhase = 'stopped' | 'preparing' | 'starting' | 'running' | 'failed'

export interface DshRuntimeStatus {
  phase: DshPhase
  access: DshAccess
  port: number
  version: string | null
  executable: string | null
  message: string
  issue: string | null
  detail: string | null
}

export interface DshPortStatus {
  available: boolean
  /** Neutral identity of the listener: `node.exe（PID 21436）`. */
  occupant: string | null
  occupantIsDsh: boolean
  /** True when the occupant is the dsh this launcher started. */
  occupantIsSupervised: boolean
}

export interface DshPortProcess {
  pid: number
  name: string
}

export interface DshPortReleaseReport {
  released: boolean
  killed: DshPortProcess[]
  failed: string[]
  selfProtected: boolean
  message: string
}

export interface DshInstallProgress {
  bytes: number
  bytesPerSecond: number
  elapsedMs: number
  cached: boolean
}

export interface DshRuntimeConfig {
  access: DshAccess
  port: number
}

function normalizeStatusAccess(value: unknown): DshAccess {
  return normalizeDshAccess(value)
}

export const useDshConfigStore = defineStore('dshConfig', () => {
  /** Last persisted values; the draft is compared against these. */
  const saved = reactive<DshRuntimeConfig>({ access: 'local', port: DSH_DEFAULT_PORT })
  const access = ref<DshAccess>('local')
  const port = ref<number>(DSH_DEFAULT_PORT)
  const loading = ref(false)
  const saving = ref(false)
  const loaded = ref(false)

  const status = ref<DshRuntimeStatus | null>(null)
  const statusChecking = ref(false)
  const portStatus = ref<DshPortStatus | null>(null)
  const portChecking = ref(false)
  const releasing = ref(false)
  const actionError = ref('')
  /** Non-error feedback, e.g. the result of a port cleanup. */
  const actionNotice = ref('')
  const progress = ref<DshInstallProgress | null>(null)
  /** Set when a saved change needs a service restart to take effect. */
  const pendingRestart = ref(false)
  /** The LAN QR dialog lives in the configuration panel. */
  const qrVisible = ref(false)
  let loadPromise: Promise<void> | null = null
  let progressUnlisten: UnlistenFn | null = null

  const isDirty = computed(() => (
    access.value !== saved.access || port.value !== saved.port
  ))
  const isRunning = computed(() => status.value?.phase === 'running')
  const isBusy = computed(() => (
    status.value?.phase === 'preparing' || status.value?.phase === 'starting'
  ))
  const isRemote = computed(() => access.value === 'remote')

  /**
   * Port problems that must block 启动. The port is never auto-changed: a moving
   * URL cannot be bookmarked or shared.
   *
   * Reported regardless of draft state: an occupied port blocks starting either
   * way, and it is the condition the «一键清理占用» button remedies.
   */
  const portError = computed(() => {
    if (!isValidDshPort(port.value)) return `端口必须是 ${DSH_PORT_MIN}–${DSH_PORT_MAX} 之间的整数。`
    if (portStatus.value && !portStatus.value.available) {
      return `端口 ${port.value} 已被占用：${describePortOccupant(portStatus.value.occupant, port.value)}`
    }
    return ''
  })

  function defaultStatus(): DshRuntimeStatus {
    return {
      phase: 'stopped',
      access: access.value,
      port: port.value,
      version: null,
      executable: null,
      message: 'DeepSeek Harness 未启动。',
      issue: null,
      detail: null,
    }
  }

  async function load() {
    if (loaded.value) return
    if (loadPromise) return loadPromise
    loadPromise = (async () => {
      loading.value = true
      try {
        const config = await invoke<DshRuntimeConfig>('load_dsh_runtime_config')
        saved.access = normalizeDshAccess(config.access)
        saved.port = isValidDshPort(config.port) ? config.port : DSH_DEFAULT_PORT
        access.value = saved.access
        port.value = saved.port
        loaded.value = true
      } catch (error) {
        actionError.value = `读取 dsh 运行设置失败：${String(error)}`
      } finally {
        loading.value = false
        loadPromise = null
      }
    })()
    return loadPromise
  }

  async function save() {
    if (!isValidDshPort(port.value)) {
      actionError.value = `端口必须是 ${DSH_PORT_MIN}–${DSH_PORT_MAX} 之间的整数。`
      return
    }
    saving.value = true
    actionError.value = ''
    try {
      const config = await invoke<DshRuntimeConfig>('save_dsh_runtime_config', {
        config: { access: access.value, port: port.value },
      })
      saved.access = normalizeDshAccess(config.access)
      saved.port = config.port
      access.value = saved.access
      port.value = saved.port
      // 改端口/访问范围后按既有约定标注"下次启动生效"，由面板询问是否立即重启。
      if (isRunning.value) {
        pendingRestart.value = status.value?.access !== saved.access
          || status.value?.port !== saved.port
      }
    } catch (error) {
      actionError.value = `保存 dsh 运行设置失败：${String(error)}`
    } finally {
      saving.value = false
    }
  }

  /** Discard draft edits; used by the shared draft guard. */
  function discardChanges() {
    access.value = saved.access
    port.value = saved.port
    actionError.value = ''
  }

  async function refreshStatus() {
    statusChecking.value = true
    try {
      const next = await invoke<DshRuntimeStatus>('dsh_runtime_status')
      status.value = { ...next, access: normalizeStatusAccess(next.access) }
    } catch (error) {
      actionError.value = `读取 dsh 运行状态失败：${String(error)}`
    } finally {
      statusChecking.value = false
    }
  }

  async function checkPort() {
    if (!isValidDshPort(port.value)) {
      portStatus.value = null
      return
    }
    portChecking.value = true
    try {
      portStatus.value = await invoke<DshPortStatus>('dsh_check_port', {
        access: access.value,
        port: port.value,
      })
    } catch {
      portStatus.value = null
    } finally {
      portChecking.value = false
    }
  }

  /** Surface a panel-level failure (used by the copy-link action). */
  function setActionError(message: string) {
    actionError.value = message
  }

  function clearActionError() {
    actionError.value = ''
  }

  /**
   * Terminate whatever holds the current port so a managed dsh can bind it.
   *
   * Destructive, so the caller must confirm first. The backend additionally
   * refuses to touch this process (or its ancestors) and reports that through
   * `selfProtected` instead of guessing.
   */
  async function releasePort(): Promise<DshPortReleaseReport> {
    releasing.value = true
    actionError.value = ''
    try {
      const report = await invoke<DshPortReleaseReport>('dsh_release_port', {
        port: port.value,
      })
      if (!report.released) {
        actionError.value = report.message
      } else {
        actionNotice.value = report.message
      }
      return report
    } catch (error) {
      const message = `清理端口失败：${String(error)}`
      actionError.value = message
      return {
        released: false,
        killed: [],
        failed: [message],
        selfProtected: false,
        message,
      }
    } finally {
      releasing.value = false
      // The panel's port hint must reflect the new reality either way.
      await checkPort()
    }
  }

  function openQr() {
    qrVisible.value = true
  }

  function closeQr() {
    qrVisible.value = false
  }

  async function ensureProgressListener() {
    if (progressUnlisten) return
    progressUnlisten = await listen<DshInstallProgress>('dsh_install_progress', (event) => {
      progress.value = event.payload
    })
  }

  function disposeProgressListener() {
    progressUnlisten?.()
    progressUnlisten = null
    progress.value = null
  }

  async function start() {
    if (portError.value) {
      actionError.value = portError.value
      return
    }
    actionError.value = ''
    actionNotice.value = ''
    await ensureProgressListener()
    status.value = {
      ...(status.value ?? defaultStatus()),
      phase: 'preparing',
      message: '正在准备 dsh，首次运行需要下载依赖。',
    }
    try {
      const started = await invoke<DshRuntimeStatus>('dsh_runtime_start', {
        access: access.value,
        port: port.value,
      })
      status.value = { ...started, access: normalizeStatusAccess(started.access) }
      if (started.phase === 'failed') {
        actionError.value = started.message
      } else {
        saved.access = access.value
        saved.port = port.value
        pendingRestart.value = false
      }
    } catch (error) {
      actionError.value = String(error)
      await refreshStatus()
    } finally {
      disposeProgressListener()
    }
  }

  async function stop() {
    actionError.value = ''
    actionNotice.value = ''
    try {
      const stopped = await invoke<DshRuntimeStatus>('dsh_runtime_stop')
      status.value = { ...stopped, access: normalizeStatusAccess(stopped.access) }
      pendingRestart.value = false
    } catch (error) {
      actionError.value = String(error)
      await refreshStatus()
    }
  }

  async function restart() {
    await stop()
    await start()
  }

  return {
    saved,
    access,
    port,
    loading,
    saving,
    loaded,
    status,
    statusChecking,
    portStatus,
    portChecking,
    releasing,
    actionError,
    actionNotice,
    progress,
    pendingRestart,
    qrVisible,
    isDirty,
    isRunning,
    isBusy,
    isRemote,
    portError,
    load,
    save,
    discardChanges,
    refreshStatus,
    checkPort,
    setActionError,
    clearActionError,
    releasePort,
    openQr,
    closeQr,
    start,
    stop,
    restart,
    disposeProgressListener,
  }
})
