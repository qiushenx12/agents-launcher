import { ref } from 'vue'

const showSettings = ref(false)

export function useSettingsPopover() {
  function toggleSettings() {
    showSettings.value = !showSettings.value
  }
  function closeSettings() {
    showSettings.value = false
  }
  return { showSettings, toggleSettings, closeSettings }
}
