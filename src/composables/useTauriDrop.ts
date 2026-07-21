import { onMounted, onBeforeUnmount } from 'vue'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'

export interface DropPosition {
  x: number
  y: number
}

export interface DropOptions {
  onOver?: (position: DropPosition) => void
  onLeave?: () => void
}

function extractFilePaths(event: DragEvent): string[] {
  const files = event.dataTransfer?.files
  if (!files || files.length === 0) return []
  const paths: string[] = []
  for (let i = 0; i < files.length; i++) {
    const f = files[i] as File & { path?: string }
    if (f.path) {
      paths.push(f.path)
    }
  }
  return paths
}

export function useTauriDrop(
  callback: (paths: string[], position: DropPosition) => void,
  options?: DropOptions,
) {
  let unlisten: (() => void) | null = null
  let nativeCleanup: (() => void) | null = null

  onMounted(async () => {
    // Browser-native drop (primary; works on all platforms)
    const onDragOver = (e: DragEvent) => {
      e.preventDefault()
      options?.onOver?.({ x: e.clientX, y: e.clientY })
    }
    const onDragLeave = () => {
      options?.onLeave?.()
    }
    const onDrop = (e: DragEvent) => {
      e.preventDefault()
      const paths = extractFilePaths(e)
      if (paths.length > 0) {
        callback(paths, { x: e.clientX, y: e.clientY })
      }
    }
    document.addEventListener('dragover', onDragOver)
    document.addEventListener('dragleave', onDragLeave)
    document.addEventListener('drop', onDrop)
    nativeCleanup = () => {
      document.removeEventListener('dragover', onDragOver)
      document.removeEventListener('dragleave', onDragLeave)
      document.removeEventListener('drop', onDrop)
    }

    // Tauri v2 native event as fallback
    try {
      const win = getCurrentWebviewWindow()
      // onDragDropEvent reports physical pixels; getBoundingClientRect uses
      // logical pixels, so positions must be divided by the scale factor.
      let scaleFactor = 1
      try {
        scaleFactor = await win.scaleFactor()
      } catch {
        scaleFactor = window.devicePixelRatio || 1
      }
      unlisten = await win.onDragDropEvent((event) => {
        const payload = event.payload
        if (payload.type === 'leave') {
          options?.onLeave?.()
          return
        }
        if (payload.type === 'over') {
          options?.onOver?.({
            x: payload.position.x / scaleFactor,
            y: payload.position.y / scaleFactor,
          })
          return
        }
        if (payload.type === 'drop' && payload.paths.length > 0) {
          callback(payload.paths, {
            x: payload.position.x / scaleFactor,
            y: payload.position.y / scaleFactor,
          })
        }
      })
    } catch {
      // onDragDropEvent not available
    }
  })

  onBeforeUnmount(() => {
    unlisten?.()
    nativeCleanup?.()
  })
}

export function isInside(
  position: DropPosition,
  element: HTMLElement | null,
): boolean {
  if (!element) return false
  const rect = element.getBoundingClientRect()
  return (
    position.x >= rect.left
    && position.x <= rect.right
    && position.y >= rect.top
    && position.y <= rect.bottom
  )
}
