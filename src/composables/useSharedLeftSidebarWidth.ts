import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'

const PANE_KEY = 'workspace-left-sidebar'
const DEFAULT_WIDTH = 280
const MIN_WIDTH = 200
const MAX_WIDTH = 400

const leftWidth = ref(DEFAULT_WIDTH)
const isDragging = ref(false)
let initialized: Promise<void> | null = null
let startX = 0
let startWidth = DEFAULT_WIDTH

function clampWidth(width: number) {
  return Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, width))
}

async function loadWidth() {
  if (!initialized) {
    initialized = invoke<number | null>('load_pane_width', { key: PANE_KEY })
      .then((saved) => {
        if (saved !== null && saved !== undefined) leftWidth.value = clampWidth(saved)
      })
      .catch(() => {})
  }
  await initialized
}

async function saveWidth() {
  await invoke('save_pane_width', { key: PANE_KEY, width: leftWidth.value }).catch(() => {})
}

function onMouseMove(event: MouseEvent) {
  leftWidth.value = clampWidth(startWidth + event.clientX - startX)
}

function onMouseUp() {
  isDragging.value = false
  document.removeEventListener('mousemove', onMouseMove)
  document.removeEventListener('mouseup', onMouseUp)
  void saveWidth()
}

function onMouseDown(event: MouseEvent) {
  event.preventDefault()
  startX = event.clientX
  startWidth = leftWidth.value
  isDragging.value = true
  document.addEventListener('mousemove', onMouseMove)
  document.addEventListener('mouseup', onMouseUp)
}

export function useSharedLeftSidebarWidth() {
  return { leftWidth, isDragging, onMouseDown, loadWidth }
}
