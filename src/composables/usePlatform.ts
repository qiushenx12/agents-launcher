import { ref } from 'vue'

const isWindows = ref(false)
const isMacOS = ref(false)

export function usePlatform() {
  isWindows.value = navigator.platform.includes('Win')
  isMacOS.value = navigator.platform.includes('Mac')
  return { isWindows, isMacOS }
}
