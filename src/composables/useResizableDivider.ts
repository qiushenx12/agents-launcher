import { ref, onMounted, onBeforeUnmount } from 'vue'

export interface ResizableDividerOptions {
  min?: number
  max?: number
  invert?: boolean
  axis?: 'x' | 'y'
  onChange?: (value: number) => void
}

export function useResizableDivider(
  initialValue: number,
  options: ResizableDividerOptions = {},
) {
  const value = ref(initialValue)
  const isDragging = ref(false)
  let startX = 0
  let startValue = 0

  function start(e: MouseEvent) {
    isDragging.value = true
    startX = options.axis === 'y' ? e.clientY : e.clientX
    startValue = value.value
    e.preventDefault()
  }

  function move(e: MouseEvent) {
    if (!isDragging.value) return
    const delta = (options.axis === 'y' ? e.clientY : e.clientX) - startX
    const raw = options.invert ? startValue - delta : startValue + delta
    let next = raw
    if (options.min !== undefined) next = Math.max(options.min, next)
    if (options.max !== undefined) next = Math.min(options.max, next)
    value.value = next
    options.onChange?.(next)
  }

  function stop() {
    isDragging.value = false
  }

  onMounted(() => {
    window.addEventListener('mousemove', move)
    window.addEventListener('mouseup', stop)
  })

  onBeforeUnmount(() => {
    window.removeEventListener('mousemove', move)
    window.removeEventListener('mouseup', stop)
  })

  return { value, isDragging, start }
}
