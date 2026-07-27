const TITLE_BAR_INTERACTIVE_SELECTOR = [
  'button',
  'a',
  'input',
  'textarea',
  'select',
  'option',
  'summary',
  '[role="button"]',
  '[contenteditable="true"]',
  '[draggable="true"]',
  '[data-tauri-drag-region="false"]',
].join(', ')

interface TitleBarMouseEvent {
  button: number
  detail: number
  target: unknown
}

interface ClosestTarget {
  closest: (selector: string) => unknown
}

function isClosestTarget(target: unknown): target is ClosestTarget {
  if (typeof target !== 'object' || target === null) return false
  return typeof Reflect.get(target, 'closest') === 'function'
}

export function isInteractiveTitleBarTarget(target: unknown): boolean {
  if (!isClosestTarget(target)) return false
  return target.closest(TITLE_BAR_INTERACTIVE_SELECTOR) !== null
}

export function shouldStartTitleBarDrag(event: TitleBarMouseEvent): boolean {
  return event.button === 0
    && event.detail === 1
    && !isInteractiveTitleBarTarget(event.target)
}

export function shouldToggleTitleBarMaximize(event: TitleBarMouseEvent): boolean {
  return event.button === 0
    && event.detail === 2
    && !isInteractiveTitleBarTarget(event.target)
}
