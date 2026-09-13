import { computed, onBeforeUnmount, ref, watch, type Ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { dshAddressRows, type DshAddressRow, type DshRuntimeUrls } from '@/utils/dshRuntime'

/**
 * The dsh access list: every address the running service answers on, each with
 * the token-bearing URL its own row actions (复制 / 打开网页 / 二维码) use.
 *
 * The panel has no global copy button on purpose — which address is the right
 * one depends on where the receiving device is (same Wi-Fi, same tailnet), and
 * only the user knows that.
 *
 * The URLs are fetched on demand and held in component-local state only. They
 * are never written to a store, disk, a diagnostic payload or a log, and are
 * dropped when the owning component unmounts.
 */
export function useDshLink(options: {
  /** Reactive flag: only fetch while the service is running. */
  running: Ref<boolean>
  /** Optional side channel for surfacing failures in the panel. */
  onError?: (message: string) => void
}) {
  const urls = ref<DshRuntimeUrls | null>(null)
  /** URL most recently copied, so the row that did it can say 「已复制」. */
  const copiedKey = ref<string | null>(null)
  let copiedTimer: ReturnType<typeof setTimeout> | null = null

  const rows = computed<DshAddressRow[]>(() => dshAddressRows(urls.value))

  async function loadUrls() {
    if (!options.running.value) {
      urls.value = null
      return
    }
    try {
      urls.value = await invoke<DshRuntimeUrls | null>('dsh_runtime_urls')
    } catch {
      urls.value = null
    }
  }

  /**
   * The current token-bearing URL of a listed address, or null when it is gone.
   *
   * Every row action goes through here: a restart issues a new process token, so
   * the URL captured when the row was rendered is stale the moment the service
   * comes back, and both copying and opening it would land on dsh's 401 page.
   */
  async function resolveAddress(row: DshAddressRow): Promise<string | null> {
    await loadUrls()
    const fresh = rows.value.find((entry) => entry.display === row.display)
    if (!fresh) {
      options.onError?.('该地址当前不可用，请刷新状态后重试。')
      return null
    }
    return fresh.url
  }

  function flagCopied(key: string) {
    copiedKey.value = key
    if (copiedTimer !== null) clearTimeout(copiedTimer)
    copiedTimer = setTimeout(() => {
      copiedKey.value = null
      copiedTimer = null
    }, 2200)
  }

  /** Returns true when the clipboard write succeeded. */
  async function copyValue(value: string): Promise<boolean> {
    try {
      await navigator.clipboard.writeText(value)
      flagCopied(value)
      return true
    } catch (error) {
      options.onError?.(`复制失败：${String(error)}`)
      return false
    }
  }

  /** The 复制 button of one address row. */
  async function copyAddress(row: DshAddressRow): Promise<boolean> {
    const value = await resolveAddress(row)
    if (!value) return false
    return copyValue(value)
  }

  // The URL set changes on every service restart (a new process token), so
  // re-read it whenever the service starts.
  watch(() => options.running.value, (running) => {
    if (running) void loadUrls()
    else urls.value = null
  }, { immediate: true })

  onBeforeUnmount(() => {
    if (copiedTimer !== null) clearTimeout(copiedTimer)
    // Drop the tokens with the component instead of caching them.
    urls.value = null
    copiedKey.value = null
  })

  return {
    urls,
    rows,
    copiedKey,
    copyAddress,
    resolveAddress,
    loadUrls,
  }
}
