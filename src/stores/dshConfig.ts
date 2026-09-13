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
  /** Which networks the occupant serves; null when the port is free. */
  occupantListenScope: DshAccess | null
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
  const stopping = ref(false)
  const actionError = ref('')
  /** Non-error feedback, e.g. the result of a port cleanup. */
  const actionNotice = ref('')
  const progress = ref<DshInstallProgress | null>(null)
  /** Set when a saved change needs a service restart to take effect. */
  const pendingRestart = ref(false)
  /** The address QR dialog lives in the configuration panel. */
  const qrVisible = ref(false)
  let loadPromise: Promise<void> | null = null
  let progressUnlisten: UnlistenFn | null = null
  let statusRequestId = 0
  let portCheckRequestId = 0

  function invalidateStatusRequests() {
    statusRequestId += 1
    statusChecking.value = false
  }

  const isDirty = computed(() => (
    access.value !== saved.access || port.value !== saved.port
  ))
  const isRunning = computed(() => status.value?.phase === 'running')
  const isBusy = computed(() => (
    status.value?.phase === 'preparing' || status.value?.phase === 'starting'
    || stopping.value
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
    const requestId = ++statusRequestId
    statusChecking.value = true
    try {
      const next = await invoke<DshRuntimeStatus>('dsh_runtime_status')
      if (requestId === statusRequestId) {
        status.value = { ...next, access: normalizeStatusAccess(next.access) }
      }
    } catch (error) {
      if (requestId === statusRequestId) {
        actionError.value = `读取 dsh 运行状态失败：${String(error)}`
      }
    } finally {
      if (requestId === statusRequestId) statusChecking.value = false
    }
  }

  async function checkPort() {
    if (!isValidDshPort(port.value)) {
      portCheckRequestId += 1
      portStatus.value = null
      portChecking.value = false
      return
    }
    const requestId = ++portCheckRequestId
    const requestedAccess = access.value
    const requestedPort = port.value
    portChecking.value = true
    try {
      const next = await invoke<DshPortStatus>('dsh_check_port', {
        access: requestedAccess,
        port: requestedPort,
      })
      if (requestId === portCheckRequestId) portStatus.value = next
    } catch {
      if (requestId === portCheckRequestId) portStatus.value = null
    } finally {
      if (requestId === portCheckRequestId) portChecking.value = false
    }
  }

  /**
   * Probe an arbitrary port without touching `portStatus` (that state belongs
   * to the config panel's draft). The runtime panel's auto-start flow uses
   * this to probe the *saved* port even while the draft differs.
   */
  async function probePort(
    probeAccess: DshAccess,
    probePortValue: number,
  ): Promise<DshPortStatus | null> {
    try {
      return await invoke<DshPortStatus>('dsh_check_port', {
        access: probeAccess,
        port: probePortValue,
      })
    } catch {
      return null
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
  async function releasePort(targetPort?: number): Promise<DshPortReleaseReport> {
    releasing.value = true
    actionError.value = ''
    try {
      const report = await invoke<DshPortReleaseReport>('dsh_release_port', {
        port: targetPort ?? port.value,
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

  /**
   * Start the service with an explicit configuration, leaving the draft and
   * the saved values alone. The runtime panel's auto-start uses this: it must
   * run on the *saved* config even while the config panel holds unsaved edits.
   *
   * Returns true only when the service ended up running.
   */
  async function startWith(startAccess: DshAccess, startPort: number): Promise<boolean> {
    actionError.value = ''
    actionNotice.value = ''
    // Invalidate a status poll that began before this explicit transition.
    invalidateStatusRequests()
    await ensureProgressListener()
    status.value = {
      ...(status.value ?? defaultStatus()),
      phase: 'preparing',
      access: startAccess,
      port: startPort,
      message: '正在准备 dsh，首次运行需要下载依赖。',
    }
    try {
      const started = await invoke<DshRuntimeStatus>('dsh_runtime_start', {
        access: startAccess,
        port: startPort,
      })
      invalidateStatusRequests()
      status.value = { ...started, access: normalizeStatusAccess(started.access) }
      if (started.phase === 'failed') {
        actionError.value = started.message
        return false
      }
      pendingRestart.value = false
      return true
    } catch (error) {
      actionError.value = String(error)
      await refreshStatus()
      return false
    } finally {
      disposeProgressListener()
    }
  }

  async function start() {
    if (portError.value) {
      actionError.value = portError.value
      return
    }
    const ok = await startWith(access.value, port.value)
    if (ok) {
      // 从配置页手动启动视为采纳草稿配置：保存值跟随草稿（既有行为）。
      saved.access = access.value
      saved.port = port.value
    }
  }

  async function stop(): Promise<boolean> {
    actionError.value = ''
    actionNotice.value = ''
    stopping.value = true
    // A running-state poll may already be in flight. Its answer is older than
    // the explicit stop result and must never overwrite that result later.
    invalidateStatusRequests()
    try {
      const stopped = await invoke<DshRuntimeStatus>('dsh_runtime_stop')
      invalidateStatusRequests()
      status.value = { ...stopped, access: normalizeStatusAccess(stopped.access) }
      if (stopped.phase !== 'stopped') {
        actionError.value = stopped.message
        return false
      }
      pendingRestart.value = false
      return true
    } catch (error) {
      actionError.value = String(error)
      await refreshStatus()
      return false
    } finally {
      stopping.value = false
      // The status card and the port hint must describe the same verified
      // reality after a stop attempt, successful or otherwise.
      await checkPort()
    }
  }

  async function restart() {
    if (await stop()) await start()
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
    stopping,
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
    probePort,
    setActionError,
    clearActionError,
    releasePort,
    openQr,
    closeQr,
    start,
    startWith,
    stop,
    restart,
    disposeProgressListener,
  }
})
