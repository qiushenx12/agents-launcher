import { ref } from 'vue'

export const MD_FONT_MIN = 9
export const MD_FONT_MAX = 20
const STORAGE_KEY = 'md-font-size'

const fontSize = ref(13)

function applyFontSize(size: number) {
  const clamped = Math.max(MD_FONT_MIN, Math.min(MD_FONT_MAX, size))
  fontSize.value = clamped
  document.documentElement.style.setProperty('--md-font-size', `${clamped}px`)
  localStorage.setItem(STORAGE_KEY, String(clamped))
}

const saved = parseInt(localStorage.getItem(STORAGE_KEY) ?? '', 10)
applyFontSize(isNaN(saved) ? fontSize.value : saved)

export function useMarkdownFontSize() {
  return { fontSize, setFontSize: applyFontSize }
}
