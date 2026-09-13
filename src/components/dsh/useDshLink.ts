import { computed, onBeforeUnmount, ref, watch, type Ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { pickCopyUrl, type DshAccess } from '@/utils/dshRuntime'
import type { DshRuntimeUrls } from '@/stores/dshConfig'

/**
 * "复制链接" for the dsh panels.
 *
 * The token-bearing URLs are fetched on demand and held in component-local
 * state only. They are never written to a store, disk, a diagnostic payload or
 * a log, and are dropped when the owning component unmounts.
 *
 * Both the configuration panel and the runtime panel use this composable so
 * there is exactly one place that decides which URL is copied — see
 * `pickCopyUrl`, which follows the access scope and always keeps the token.
 */
export function useDshLink(options: {
  /** Reactive access scope; decides between the loopback and LAN URL. */
  access: Ref<DshAccess>
  /** Reactive flag: only fetch while the service is running. */
  running: Ref<boolean>
  /** Optional side channel for surfacing failures in the panel. */
  onError?: (message: string) => void
}) {
  const urls = ref<DshRuntimeUrls | null>(null)
  const copied = ref(false)
  let copiedTimer: ReturnType<typeof setTimeout> | null = null

  const copyUrl = computed(() => pickCopyUrl(urls.value, options.access.value))
  const canCopy = computed(() => copyUrl.value !== null)

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

  function flagCopied() {
    copied.value = true
    if (copiedTimer !== null) clearTimeout(copiedTimer)
    copiedTimer = setTimeout(() => {
      copied.value = false
      copiedTimer = null
    }, 2200)
  }

  /** Returns true when the clipboard write succeeded. */
  async function copyLink(): Promise<boolean> {
    await loadUrls()
    const value = copyUrl.value
    if (!value) {
      options.onError?.('服务运行后才能复制链接。')
      return false
    }
    try {
      await navigator.clipboard.writeText(value)
      flagCopied()
      return true
    } catch (error) {
      options.onError?.(`复制失败：${String(error)}`)
      return false
    }
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
  })

  return { urls, copyUrl, canCopy, copied, copyLink, loadUrls }
}
