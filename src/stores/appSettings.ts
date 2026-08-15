import { ref } from 'vue'
import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'

export const useAppSettingsStore = defineStore('appSettings', () => {
  const minimizeToTray = ref(false)
  const loaded = ref(false)
  let loadPromise: Promise<void> | null = null

  async function load() {
    if (loaded.value) return
    if (loadPromise) return loadPromise

    loadPromise = (async () => {
      try {
        minimizeToTray.value = await invoke<boolean>('load_minimize_to_tray')
      } catch {
        minimizeToTray.value = false
      } finally {
        loaded.value = true
        loadPromise = null
      }
    })()
    return loadPromise
  }

  async function setMinimizeToTray(enabled: boolean) {
    await load()
    const previous = minimizeToTray.value
    minimizeToTray.value = enabled
    try {
      await invoke('save_minimize_to_tray', { enabled })
    } catch (error) {
      minimizeToTray.value = previous
      throw error
    }
  }

  return {
    minimizeToTray,
    loaded,
    load,
    setMinimizeToTray,
  }
})
