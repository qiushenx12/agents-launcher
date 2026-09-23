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
  /**
   * Seconds since the supervised service started; non-null only while running.
   * Pair with `statusFetchedAt` to render a live-ticking「已运行 …」label.
   */
  uptimeSecs: number | null
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
  /**
   * The version future starts are pinned to, recorded by the backend after
   * each successful start. Absent before the first recorded start; the
   * frontend never writes it (a save without it preserves the stored pin).
   */
  pinnedVersion?: string | null
}

/** Outcome of the version picker's read-only listing (`dsh_list_versions`). */
export interface DshVersionList {
  /** Every published version, newest first. */
  versions: string[]
  /** The pin future starts use, when one has been recorded. */
  pinned: string | null
  /** The registry's `latest` dist-tag; null when it could not be read. */
  latest: string | null
}

/** A runnable version present in npm's local `_npx` cache. */
export interface DshCachedVersion {
  version: string
  entries: number
}

/** Read-only outcome of 「检查更新」 (`dsh_check_update`): nothing is written. */
export interface DshVersionCheck {
  /** The registry's current `latest`. */
  latest: string
  /** The version future starts are pinned to, when one has been recorded. */
  pinned: string | null
  /** True when the pin differs from `latest`, i.e. an update can be offered. */
  updateAvailable: boolean
  /** Backend-provided user-facing summary, rendered verbatim. */
  message: string
}

/** Outcome of the confirmed 「更新版本」 command (`dsh_update_version`). */
export interface DshVersionUpdate {
  /** The version future starts are now pinned to. */
  version: string
  /** The pin this update replaced, when there was one. */
  previous: string | null
  /** False when the recorded version was already the pinned one. */
  changed: boolean
  /** Backend-provided user-facing summary, rendered verbatim. */
  message: string
}

function normalizePinnedVersion(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value.trim() : null
}

function normalizeStatusAccess(value: unknown): DshAccess {
  return normalizeDshAccess(value)
}

export const useDshConfigStore = defineStore('dshConfig', () => {
  /** Last persisted values; the draft is compared against these. */
  const saved = reactive<DshRuntimeConfig>({
    access: 'local',
    port: DSH_DEFAULT_PORT,
    pinnedVersion: null,
  })
  const access = ref<DshAccess>('local')
  const port = ref<number>(DSH_DEFAULT_PORT)
  const loading = ref(false)
  const saving = ref(false)
  const loaded = ref(false)

  const status = ref<DshRuntimeStatus | null>(null)
  /**
   * Local clock reading (Date.now()) of the moment `status` last came back from
   * the backend. `uptimeSecs` is a snapshot; a live label adds the local time
   * elapsed since this reading.
   */
  const statusFetchedAt = ref(0)
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
  /** What `pendingRestart` is about: a config edit or a version update. */
  const pendingRestartReason = ref<'config' | 'version' | null>(null)
  /** True while 「检查更新」/「更新版本」 is talking to the registry or writing. */
  const updatingVersion = ref(false)
  /** The version picker dialog's visibility. */
  const versionPickerVisible = ref(false)
  /**
   * The picker's loaded data. `null` while loading and after a failed load;
   * `versionPickerError` carries the failure so the dialog can show it instead
   * of an empty list.
   */
  const versionPickerList = ref<DshVersionList | null>(null)
  /** Draft selection inside the picker; committed only on 确定. */
  const versionPickerChoice = ref<string | null>(null)
  const versionPickerError = ref('')
  const cachedVersions = ref<DshCachedVersion[]>([])
  const cachedVersionsLoading = ref(false)
  const cachedVersionsError = ref('')
  const cachedDeleteNotice = ref('')
  const cachedDeleteError = ref('')
  const deletingCachedVersion = ref<string | null>(null)
  let versionPickerRequestId = 0
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
      uptimeSecs: null,
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
        saved.pinnedVersion = normalizePinnedVersion(config.pinnedVersion)
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
      // 保存只带访问范围与端口；固定版本由后端保留（pinnedVersion 缺省不清除）。
      const config = await invoke<DshRuntimeConfig>('save_dsh_runtime_config', {
        config: { access: access.value, port: port.value },
      })
      saved.access = normalizeDshAccess(config.access)
      saved.port = config.port
      saved.pinnedVersion = normalizePinnedVersion(config.pinnedVersion) ?? saved.pinnedVersion
      access.value = saved.access
      port.value = saved.port
      // 改端口/访问范围后按既有约定标注"下次启动生效"，由面板询问是否立即重启。
      if (isRunning.value) {
        pendingRestart.value = status.value?.access !== saved.access
          || status.value?.port !== saved.port
        if (pendingRestart.value) pendingRestartReason.value = 'config'
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
        statusFetchedAt.value = Date.now()
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

  /**
   * 「检查更新」: open the version picker and load the available versions.
   *
   * The dialog opens first and the listing fills in, so a slow registry reads
   * as "loading" rather than as an unresponsive button. The current pin is
   * preselected, which makes 确定 a no-op until the user actually picks
   * something else.
   */
  async function openVersionPicker() {
    const requestId = ++versionPickerRequestId
    versionPickerVisible.value = true
    versionPickerList.value = null
    versionPickerError.value = ''
    versionPickerChoice.value = null
    cachedVersions.value = []
    cachedDeleteNotice.value = ''
    cachedDeleteError.value = ''
    void refreshCachedVersions(requestId)
    updatingVersion.value = true
    actionError.value = ''
    actionNotice.value = ''
    try {
      const list = await invoke<DshVersionList>('dsh_list_versions')
      if (requestId === versionPickerRequestId) {
        versionPickerList.value = list
        versionPickerChoice.value = list.pinned
      }
    } catch (error) {
      if (requestId === versionPickerRequestId) {
        versionPickerError.value = `获取 dsh 版本列表失败：${String(error)}`
      }
    } finally {
      if (requestId === versionPickerRequestId) updatingVersion.value = false
    }
  }

  function closeVersionPicker() {
    versionPickerRequestId += 1
    versionPickerVisible.value = false
    versionPickerList.value = null
    versionPickerChoice.value = null
    versionPickerError.value = ''
  }

  async function refreshCachedVersions(requestId = versionPickerRequestId) {
    cachedVersionsLoading.value = true
    cachedVersionsError.value = ''
    try {
      const versions = await invoke<DshCachedVersion[]>('dsh_list_cached_versions')
      if (requestId === versionPickerRequestId) cachedVersions.value = versions
    } catch (error) {
      if (requestId === versionPickerRequestId) {
        cachedVersionsError.value = `读取本机 dsh 版本失败：${String(error)}`
      }
    } finally {
      if (requestId === versionPickerRequestId) cachedVersionsLoading.value = false
    }
  }

  /** The dialog asks for confirmation before calling this destructive action. */
  async function deleteCachedVersion(version: string): Promise<void> {
    if (deletingCachedVersion.value || isBusy.value) return
    deletingCachedVersion.value = version
    cachedDeleteError.value = ''
    cachedDeleteNotice.value = ''
    try {
      const entries = await invoke<number>('dsh_delete_cached_version', { version })
      cachedDeleteNotice.value = `已删除 v${version} 的 ${entries} 个本机缓存条目。`
    } catch (error) {
      cachedDeleteError.value = `删除 v${version} 失败：${String(error)}`
    } finally {
      deletingCachedVersion.value = null
      if (versionPickerVisible.value) await refreshCachedVersions()
    }
  }

  /** Select a row in the picker. A second click on the same row clears it. */
  function chooseVersion(version: string) {
    versionPickerChoice.value = versionPickerChoice.value === version ? null : version
  }

  /**
   * 确定: pin the chosen version. Closing is the caller's decision so the
   * dialog can stay open when the write fails.
   */
  async function confirmVersionChoice(): Promise<boolean> {
    const choice = versionPickerChoice.value
    if (!choice) return false
    const applied = await applyVersion(choice)
    if (applied) closeVersionPicker()
    return applied
  }

  /**
   * 「检查更新」: ask the registry for the latest dsh release and compare it
   * with the recorded pin. Read-only — applying the update is `applyVersion`,
   * called only after the panel's confirmation dialog is accepted.
   */
  async function checkUpdate(): Promise<DshVersionCheck | null> {
    updatingVersion.value = true
    actionError.value = ''
    actionNotice.value = ''
    try {
      const report = await invoke<DshVersionCheck>('dsh_check_update')
      // 没有可更新的内容时，结论直接作为提示展示，无需弹窗。
      if (!report.updateAvailable) actionNotice.value = report.message
      return report
    } catch (error) {
      actionError.value = `检查 dsh 更新失败：${String(error)}`
      return null
    } finally {
      updatingVersion.value = false
    }
  }

  /**
   * 「更新版本」的确认后一半: pin the version the check reported and the user
   * confirmed. The running service is never restarted implicitly — when the
   * pin moved, a restart-pending marker tells the panel to offer one.
   */
  async function applyVersion(version: string): Promise<boolean> {
    updatingVersion.value = true
    actionError.value = ''
    actionNotice.value = ''
    try {
      const report = await invoke<DshVersionUpdate>('dsh_update_version', { version })
      saved.pinnedVersion = normalizePinnedVersion(report.version)
      if (report.changed && isRunning.value) {
        pendingRestart.value = true
        pendingRestartReason.value = 'version'
      }
      actionNotice.value = report.message
      return true
    } catch (error) {
      actionError.value = `更新 dsh 版本失败：${String(error)}`
      return false
    } finally {
      updatingVersion.value = false
    }
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
   * Re-read just the persisted pin (a cheap local state read). A successful
   * start records the started version backend-side, so without this refresh
   * the version row would keep showing 「未记录」 after the first-ever start
   * until the panel happens to reload.
   */
  async function refreshPinnedVersion() {
    try {
      const config = await invoke<DshRuntimeConfig>('load_dsh_runtime_config')
      saved.pinnedVersion = normalizePinnedVersion(config.pinnedVersion)
    } catch {
      // 版本行停留在旧值即可，不影响启动结果。
    }
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
      // 首次下载的说明由进度文案（describeInstallProgress）给出，这里保持中性。
      message: '正在准备 dsh。',
    }
    try {
      const started = await invoke<DshRuntimeStatus>('dsh_runtime_start', {
        access: startAccess,
        port: startPort,
      })
      invalidateStatusRequests()
      status.value = { ...started, access: normalizeStatusAccess(started.access) }
      statusFetchedAt.value = Date.now()
      if (started.phase === 'failed') {
        actionError.value = started.message
        return false
      }
      pendingRestart.value = false
      pendingRestartReason.value = null
      await refreshPinnedVersion()
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
      statusFetchedAt.value = Date.now()
      if (stopped.phase !== 'stopped') {
        actionError.value = stopped.message
        return false
      }
      pendingRestart.value = false
      pendingRestartReason.value = null
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
    statusFetchedAt,
    statusChecking,
    portStatus,
    portChecking,
    releasing,
    stopping,
    actionError,
    actionNotice,
    progress,
    pendingRestart,
    pendingRestartReason,
    updatingVersion,
    versionPickerVisible,
    versionPickerList,
    versionPickerChoice,
    versionPickerError,
    cachedVersions,
    cachedVersionsLoading,
    cachedVersionsError,
    cachedDeleteNotice,
    cachedDeleteError,
    deletingCachedVersion,
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
    checkUpdate,
    applyVersion,
    openVersionPicker,
    closeVersionPicker,
    chooseVersion,
    confirmVersionChoice,
    deleteCachedVersion,
    start,
    startWith,
    stop,
    restart,
    disposeProgressListener,
  }
})
