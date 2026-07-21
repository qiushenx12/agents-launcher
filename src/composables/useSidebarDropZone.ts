import type { DropPosition } from '@/composables/useTauriDrop'

const ZONE_RATIO = 0.2

// Drop positions are already converted to logical (CSS) pixels in
// useTauriDrop, so the 20% right-edge zone is evaluated against the
// app's current scale via getBoundingClientRect.
export function isInSidebarDropZone(
  position: DropPosition,
  element: HTMLElement | null,
): boolean {
  if (!element) return false
  const rect = element.getBoundingClientRect()
  const zoneWidth = rect.width * ZONE_RATIO
  return (
    position.x >= rect.right - zoneWidth
    && position.x <= rect.right
    && position.y >= rect.top
    && position.y <= rect.bottom
  )
}

// Top 20% zone: opens the dropped file in the top sidebar.
export function isInTopSidebarDropZone(
  position: DropPosition,
  element: HTMLElement | null,
): boolean {
  if (!element) return false
  const rect = element.getBoundingClientRect()
  const zoneHeight = rect.height * ZONE_RATIO
  return (
    position.y >= rect.top
    && position.y <= rect.top + zoneHeight
    && position.x >= rect.left
    && position.x <= rect.right
  )
}
