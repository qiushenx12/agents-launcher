import { ref } from 'vue'

const showSettings = ref(false)
const settingsAnchorLeft = ref(8)
const settingsAnchorBottom = ref(44)
const settingsMenuMaxHeight = ref(320)
let settingsAnchorElement: HTMLElement | null = null

function updateSettingsAnchor(element?: HTMLElement | null) {
  if (element) settingsAnchorElement = element
  if (!settingsAnchorElement) return

  const bounds = settingsAnchorElement.getBoundingClientRect()
  const gap = 4
  settingsAnchorLeft.value = Math.max(8, Math.min(bounds.left, window.innerWidth - 278))
  settingsAnchorBottom.value = Math.max(0, window.innerHeight - bounds.top + gap)
  settingsMenuMaxHeight.value = Math.max(1, bounds.top - gap - 8)
}

export function useSettingsPopover() {
  function toggleSettings(event?: MouseEvent) {
    const element = event?.currentTarget instanceof HTMLElement
      ? event.currentTarget
      : null
    if (element) updateSettingsAnchor(element)
    showSettings.value = !showSettings.value
  }
  function closeSettings() {
    showSettings.value = false
  }
  return {
    showSettings,
    settingsAnchorLeft,
    settingsAnchorBottom,
    settingsMenuMaxHeight,
    toggleSettings,
    closeSettings,
    updateSettingsAnchor,
  }
}
