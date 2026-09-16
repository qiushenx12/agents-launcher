import { ref } from 'vue'

// Gap between items in the config list (matches CSS gap: 2px)
const ITEM_GAP = 2

export function useDragReorder<T>(
  getItems: () => T[],
  onReorder: (newItems: T[]) => void,
  options: {
    startDelayMs?: number
    /**
     * Vertical pitch between two items, added to an item's height when working
     * out the drag slots. It must match the list's own spacing (a CSS gap or a
     * row margin) or the items sliding out of the way land a few pixels off the
     * slot they are standing in.
     */
    gapPx?: number
    onDragStart?: (item: T, index: number) => void | Promise<void>
  } = {},
) {
  const itemGap = typeof options.gapPx === 'number' && options.gapPx >= 0
    ? options.gapPx
    : ITEM_GAP
  const draggingIndex = ref<number | null>(null)
  const overIndex = ref<number | null>(null)
  // Stays true for a short window after a drag completes so click handlers can ignore it
  const justDragged = ref(false)

  // Mutable drag state — not reactive, updated on every pointermove
  let dragClone: HTMLElement | null = null
  let originalItem: HTMLElement | null = null
  let itemElements: HTMLElement[] = []
  let startY = 0
  let cloneStartTop = 0
  let itemHeight = 0
  let activeHandle: HTMLElement | null = null
  let activePonterId = -1
  let dragStarted = false
  let dragArmed = false
  let longPressTriggered = false
  let startTimer: number | undefined

  // ─── pointer handlers ────────────────────────────────────────────────────

  function onPointerDown(index: number, e: PointerEvent) {
    // Only respond to primary button (left click / touch)
    if (e.button !== 0 && e.pointerType === 'mouse') return
    const hasStartDelay = !!(options.startDelayMs && options.startDelayMs > 0)
    if (!hasStartDelay) {
      e.preventDefault()
      e.stopPropagation()
    }

    const handle = e.currentTarget as HTMLElement
    const item = handle.closest('[data-drag-item]') as HTMLElement
    if (!item) return

    const container = item.parentElement
    if (!container) return

    itemElements = Array.from(container.querySelectorAll<HTMLElement>('[data-drag-item]'))
    const rect = item.getBoundingClientRect()
    itemHeight = rect.height + itemGap

    startY = e.clientY
    cloneStartTop = rect.top
    draggingIndex.value = index
    overIndex.value = index
    originalItem = item
    activeHandle = handle
    activePonterId = e.pointerId
    dragStarted = false
    longPressTriggered = false

    if (hasStartDelay) {
      dragArmed = false
      startTimer = window.setTimeout(async () => {
        longPressTriggered = true
        const index = draggingIndex.value
        if (index !== null) await options.onDragStart?.(getItems()[index], index)
        if (draggingIndex.value === null) return
        refreshDragMeasurements()
        dragArmed = true
      }, options.startDelayMs)
    } else {
      dragArmed = true
      const index = draggingIndex.value
      if (index !== null) options.onDragStart?.(getItems()[index], index)
    }

    // Don't create the clone yet — wait for the dead zone to be crossed in onPointerMove

    // Capture so pointermove/pointerup fire even if cursor leaves the handle
    handle.setPointerCapture(e.pointerId)
    handle.addEventListener('pointermove', onPointerMove)
    handle.addEventListener('pointerup', onPointerUp)
    handle.addEventListener('pointercancel', onPointerCancel)
  }

  function onPointerMove(e: PointerEvent) {
    if (draggingIndex.value === null) return

    const deltaY = e.clientY - startY

    if (!dragArmed) {
      if (Math.abs(deltaY) > 5) {
        cleanup()
        releaseHandle(e.pointerId)
      }
      return
    }

    // Dead zone: don't start the visual drag until the pointer has moved at least 5px
    if (!dragStarted) {
      if (Math.abs(deltaY) <= 5) return

      // Threshold crossed — create the clone and dim the original
      dragStarted = true
      const item = originalItem!
      const rect = { left: item.getBoundingClientRect().left, width: item.getBoundingClientRect().width }
      dragClone = item.cloneNode(true) as HTMLElement
      dragClone.style.cssText = [
        `position: fixed`,
        `left: ${rect.left}px`,
        `top: ${cloneStartTop}px`,
        `width: ${rect.width}px`,
        `height: ${itemHeight - itemGap}px`,
        `margin: 0`,
        `z-index: 9999`,
        `pointer-events: none`,
        `opacity: 0.95`,
        `box-shadow: ${document.documentElement.getAttribute('data-theme') === 'dark' ? '0 8px 24px rgba(0,0,0,0.45)' : '0 8px 24px rgba(0,0,0,0.18)'}`,
        `transform: translateY(${deltaY}px) scale(1.02)`,
        `transition: box-shadow 0.15s ease, transform 0.15s ease`,
        `border-radius: var(--radius-sm, 4px)`,
      ].join(';')
      document.body.appendChild(dragClone)
      item.style.opacity = '0'
      item.style.transition = 'none'
    }

    if (!dragClone) return

    // Move the clone
    dragClone.style.transform = `translateY(${deltaY}px) scale(1.02)`

    // Determine which slot the clone centre is hovering over.
    // Swap threshold: 1/3 of the item height from the leading edge of the target.
    // Downward: swap when clone centre passes target's top + 1/3 of its height.
    // Upward:   swap when clone centre passes target's bottom - 1/3 of its height.
    const cloneCentreY = cloneStartTop + deltaY + (itemHeight - itemGap) / 2
    let newOver = draggingIndex.value

    // Downward: find the furthest item below draggingIndex whose 1/3-from-top threshold the clone has passed
    for (let i = draggingIndex.value + 1; i < itemElements.length; i++) {
      const r = itemElements[i].getBoundingClientRect()
      if (cloneCentreY > r.top + r.height / 3) newOver = i
    }
    // Upward: find the furthest item above draggingIndex whose 1/3-from-bottom threshold the clone has passed.
    // Iterate in descending order so we start from the closest item above and work upward;
    // each qualifying item overwrites newOver, so the final value is the topmost reached slot.
    for (let i = draggingIndex.value - 1; i >= 0; i--) {
      const r = itemElements[i].getBoundingClientRect()
      if (cloneCentreY < r.bottom - r.height / 3) {
        newOver = i
      } else {
        // Items are ordered top-to-bottom; once we find one whose threshold we haven't
        // crossed, all items above it are also not crossed — stop early.
        break
      }
    }
    newOver = Math.max(0, Math.min(getItems().length - 1, newOver))

    if (newOver !== overIndex.value) {
      overIndex.value = newOver
      applyItemTransforms()
    }
  }

  function onPointerUp(e: PointerEvent) {
    if (draggingIndex.value === null) return

    // If the dead zone was never crossed, treat as a no-op click
    if (!dragStarted) {
      if (longPressTriggered) {
        justDragged.value = true
        setTimeout(() => { justDragged.value = false }, 300)
      }
      cleanup()
      releaseHandle(e.pointerId)
      return
    }

    const from = draggingIndex.value
    const to = overIndex.value ?? from

    // Signal that a drag just completed so click handlers can ignore the
    // synthetic click that may fire after pointerup.
    justDragged.value = true
    setTimeout(() => { justDragged.value = false }, 300)

    // Skip the slide animation entirely.
    // Items are already visually in the right positions via CSS transforms;
    // committing the reorder makes Vue re-render them in the new DOM order.
    // Clear transforms with transition:none first so there is no flash.
    commitReorder(from, to)
    cleanup()
    releaseHandle(e.pointerId)
  }

  function onPointerCancel(_e: PointerEvent) {
    // Restore original item and bail out without reordering
    if (originalItem) {
      originalItem.style.opacity = ''
      originalItem.style.transition = ''
    }
    clearItemTransforms()
    cleanup()
    releaseHandle(activePonterId)
  }

  // ─── helpers ─────────────────────────────────────────────────────────────

  function applyItemTransforms() {
    const from = draggingIndex.value
    const to = overIndex.value
    if (from === null || to === null) return

    itemElements.forEach((el, i) => {
      if (i === from) {
        // The original stays put (dimmed); transforms are for the others
        el.style.transform = ''
        return
      }
      let shift = 0
      if (from < to) {
        // Dragging downward: items between from+1..to shift up
        if (i > from && i <= to) shift = -itemHeight
      } else {
        // Dragging upward: items between to..from-1 shift down
        if (i >= to && i < from) shift = itemHeight
      }
      el.style.transition = 'transform 0.18s ease'
      el.style.transform = shift !== 0 ? `translateY(${shift}px)` : ''
    })
  }

  function clearItemTransforms() {
    itemElements.forEach((el) => {
      el.style.transition = 'transform 0.18s ease'
      el.style.transform = ''
    })
  }

  function refreshDragMeasurements() {
    if (!originalItem) return
    const container = originalItem.parentElement
    if (container) {
      itemElements = Array.from(container.querySelectorAll<HTMLElement>('[data-drag-item]'))
    }
    const rect = originalItem.getBoundingClientRect()
    itemHeight = rect.height + itemGap
    cloneStartTop = rect.top
  }

  function commitReorder(from: number, to: number) {
    // Clear all item transforms immediately (no animation) BEFORE calling onReorder.
    // This ensures no stale translateY values remain when Vue re-renders the new order.
    itemElements.forEach((el) => {
      el.style.transition = 'none'
      el.style.transform = ''
    })
    if (from !== to) {
      const items = [...getItems()]
      const [moved] = items.splice(from, 1)
      items.splice(to, 0, moved)
      onReorder(items)
    }
  }

  function cleanup() {
    if (startTimer !== undefined) {
      window.clearTimeout(startTimer)
      startTimer = undefined
    }
    // Remove clone
    if (dragClone) {
      dragClone.remove()
      dragClone = null
    }
    // Restore original item styles (Vue will re-render with correct order)
    if (originalItem) {
      originalItem.style.opacity = ''
      originalItem.style.transition = ''
      originalItem = null
    }
    // Transforms were already cleared in commitReorder; just reset the array.
    itemElements = []
    draggingIndex.value = null
    overIndex.value = null
    dragStarted = false
    dragArmed = false
    longPressTriggered = false
  }

  function releaseHandle(pointerId: number) {
    if (!activeHandle) return
    activeHandle.releasePointerCapture(pointerId)
    activeHandle.removeEventListener('pointermove', onPointerMove)
    activeHandle.removeEventListener('pointerup', onPointerUp)
    activeHandle.removeEventListener('pointercancel', onPointerCancel)
    activeHandle = null
    activePonterId = -1
  }

  return { draggingIndex, overIndex, justDragged, onPointerDown }
}
