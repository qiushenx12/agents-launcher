<template>
  <div ref="panelRef" class="project-panel">
    <Transition name="top-pane">
      <div
        v-if="store.sidebarOpen && store.sidebarPlacement === 'top'"
        class="project-panel__top-shell"
        :style="{ height: `${topHeight + 9}px`, flexBasis: `${topHeight + 9}px` }"
      >
        <RightSidebar orientation="top" :height="topHeight" />
        <div
          class="project-panel__divider project-panel__divider--horizontal"
          :class="{ 'project-panel__divider--dragging': topDivider.isDragging.value }"
          @mousedown="topDivider.start"
        />
      </div>
    </Transition>

    <div class="project-panel__row">
      <Transition name="left-pane">
        <div
          v-if="!store.leftSidebarCollapsed"
          class="project-panel__left-shell"
          :style="{ width: `${leftWidth + 9}px`, flexBasis: `${leftWidth + 9}px` }"
        >
          <ProjectSidebar
            :width="leftWidth"
            @open-settings="emit('open-settings')"
          />
          <div
            class="project-panel__divider project-panel__divider--left"
            :class="{ 'project-panel__divider--dragging': sharedLeftSidebarDragging }"
            @mousedown="startSharedLeftSidebarResize"
          />
        </div>
      </Transition>

      <section class="project-panel__main">
        <ModuleToolbar
          :show-claude-controls="showClaudeToolbarControls"
          :claude-view="activeClaudeView"
          :claude-status="claudeStatusLabel"
          :claude-status-state="claudeStatusState"
          :claude-status-detail="claudeStatusDetail"
          @select-claude-view="selectClaudeView"
        />
        <div ref="contentRef" class="project-panel__content">
        <div v-if="sidebarDropHint === 'right'" class="project-panel__drop-hint">
          <span>松开以在侧边栏打开</span>
        </div>
        <div v-if="sidebarDropHint === 'top'" class="project-panel__drop-hint project-panel__drop-hint--top">
          <span>松开以在上侧边栏打开</span>
        </div>
          <ProjectTerminalArea
            :claude-view="activeClaudeView"
            @select-claude-view="selectClaudeView"
          />
          <Transition name="right-pane">
            <div
              v-if="store.sidebarOpen && store.sidebarPlacement === 'right'"
              class="project-panel__right-shell"
              :style="{ width: `${rightWidth + 9}px`, flexBasis: `${rightWidth + 9}px` }"
            >
              <div
                class="project-panel__divider project-panel__divider--right"
                :class="{ 'project-panel__divider--dragging': rightDivider.isDragging.value }"
                @mousedown="rightDivider.start"
              />
              <RightSidebar :width="rightWidth" />
            </div>
          </Transition>
        </div>
      </section>
    </div>

    <Transition name="bottom-pane">
      <div
        v-show="store.bottomSidebarOpen"
        class="project-panel__bottom-shell"
        :style="{ height: `${bottomHeight + 9}px`, flexBasis: `${bottomHeight + 9}px` }"
      >
        <div
          class="project-panel__divider project-panel__divider--horizontal"
          :class="{ 'project-panel__divider--dragging': bottomDivider.isDragging.value }"
          @mousedown="bottomDivider.start"
        />
        <BottomSidebar :height="bottomHeight" />
      </div>
    </Transition>

    <div v-if="store.statusMessage" class="project-panel__toast">
      {{ store.statusMessage }}
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useProjectStore } from '@/stores/project'
import { useClaudeObserverStore } from '@/stores/claudeObserver'
import { useResizableDivider } from '@/composables/useResizableDivider'
import { useSharedLeftSidebarWidth } from '@/composables/useSharedLeftSidebarWidth'
import { useTauriDrop } from '@/composables/useTauriDrop'
import { isInSidebarDropZone, isInTopSidebarDropZone } from '@/composables/useSidebarDropZone'
import ProjectSidebar from './ProjectSidebar.vue'
import ModuleToolbar from './ModuleToolbar.vue'
import ProjectTerminalArea from './ProjectTerminalArea.vue'
import RightSidebar from './RightSidebar.vue'
import BottomSidebar from './BottomSidebar.vue'
import type { CliKind } from '@/types/cli'

const store = useProjectStore()
const claudeObserverStore = useClaudeObserverStore()
const props = defineProps<{
  cliKind: CliKind
}>()
const emit = defineEmits<{
  (event: 'open-settings'): void
  (event: 'left-width-change', width: number): void
}>()

const RIGHT_KEY = 'project-right-sidebar'
const RIGHT_RATIO_KEY = 'project-right-sidebar-ratio'
const TOP_KEY = 'project-top-sidebar'
const TOP_RATIO_KEY = 'project-top-sidebar-ratio'
const BOTTOM_KEY = 'project-bottom-sidebar'
const BOTTOM_RATIO_KEY = 'project-bottom-sidebar-ratio'

const MIN_RIGHT = 200
const MIN_MAIN_CONTENT = 400
const MIN_TOP = 160
const MIN_BOTTOM = 160
const contentRef = ref<HTMLElement | null>(null)
const panelRef = ref<HTMLElement | null>(null)

type ClaudeView = 'conversation' | 'terminal' | 'log'

const claudeViews = ref<Record<number, ClaudeView>>({})
const activeTerminalId = computed(() => {
  const sessionId = store.activeSessionId
  return sessionId ? store.sessionTerminalIds[sessionId] : undefined
})
const showClaudeToolbarControls = computed(() => (
  !!activeTerminalId.value && store.activeSession?.cliKind === 'claude'
))
const activeClaudeView = computed<ClaudeView>(() => {
  const tabId = activeTerminalId.value
  return tabId ? (claudeViews.value[tabId] ?? 'conversation') : 'conversation'
})
const activeClaudeState = computed(() => {
  const tabId = activeTerminalId.value
  return tabId ? claudeObserverStore.states[tabId] : undefined
})
const claudeStatusState = computed(() => activeClaudeState.value?.runState ?? 'starting')
const claudeStatusDetail = computed(() => activeClaudeState.value?.degradedReason ?? '')
const claudeStatusLabel = computed(() => {
  switch (claudeStatusState.value) {
    case 'idle': return '等待输入'
    case 'working': return 'Claude 正在处理'
    case 'permission': return '等待终端确认'
    case 'stopped': return '会话已结束'
    default: return '正在连接 Claude'
  }
})

async function selectClaudeView(view: ClaudeView) {
  const tabId = activeTerminalId.value
  if (!tabId || view === activeClaudeView.value) return
  const previousView = activeClaudeView.value

  if (view === 'terminal') {
    try {
      await claudeObserverStore.pausePromptQueueForRawTerminal(tabId)
    } catch (error) {
      store.statusMessage = `切换终端前无法安全暂停等待队列：${String(error)}`
    }
  }

  claudeViews.value[tabId] = view
  if (previousView === 'terminal' && view !== 'terminal') {
    claudeObserverStore.resumePromptQueueFromRawTerminal(tabId)
  }
}

function availableRightSidebarWidth() {
  return contentRef.value?.clientWidth || window.innerWidth
}

function availableVerticalSidebarHeight() {
  return panelRef.value?.clientHeight || window.innerHeight
}

function clampRight(value: number) {
  const max = Math.max(MIN_RIGHT, availableRightSidebarWidth() - MIN_MAIN_CONTENT)
  return Math.max(MIN_RIGHT, Math.min(max, value))
}

function clampTop(value: number) {
  const max = Math.max(MIN_TOP, availableVerticalSidebarHeight() - 320)
  return Math.max(MIN_TOP, Math.min(max, value))
}

function clampBottom(value: number) {
  const max = Math.max(MIN_BOTTOM, availableVerticalSidebarHeight() - 320)
  return Math.max(MIN_BOTTOM, Math.min(max, value))
}

const {
  leftWidth: sharedLeftSidebarWidth,
  isDragging: sharedLeftSidebarDragging,
  onMouseDown: startSharedLeftSidebarResize,
  loadWidth: loadSharedLeftSidebarWidth,
} = useSharedLeftSidebarWidth()

const rightDivider = useResizableDivider(320, {
  min: MIN_RIGHT,
  invert: true,
  onChange: (value) => {
    rightWidth.value = clampRight(value)
    rightWidthRatio.value = rightWidth.value / availableRightSidebarWidth()
  },
})

const topDivider = useResizableDivider(240, {
  min: MIN_TOP,
  axis: 'y',
  onChange: (value) => {
    topHeight.value = clampTop(value)
    topHeightRatio.value = topHeight.value / availableVerticalSidebarHeight()
  },
})

const bottomDivider = useResizableDivider(240, {
  min: MIN_BOTTOM,
  axis: 'y',
  invert: true,
  onChange: (value) => {
    bottomHeight.value = clampBottom(value)
    bottomHeightRatio.value = bottomHeight.value / availableVerticalSidebarHeight()
  },
})

const leftWidth = sharedLeftSidebarWidth
const rightWidth = rightDivider.value
const rightWidthRatio = ref(rightWidth.value / availableRightSidebarWidth())
const topHeight = topDivider.value
const topHeightRatio = ref(topHeight.value / availableVerticalSidebarHeight())
const bottomHeight = bottomDivider.value
const bottomHeightRatio = ref(bottomHeight.value / availableVerticalSidebarHeight())

watch(leftWidth, (width) => {
  emit('left-width-change', width)
}, { immediate: true })

// Right-edge drop zone: while the sidebar is closed, dropping a file on the
// right 20% of the content area opens the sidebar with that file.
// Top-edge drop zone: dropping on the top 20% opens the top sidebar instead.
const sidebarDropHint = ref<'right' | 'top' | null>(null)
let rightSidebarResizeObserver: ResizeObserver | undefined
let verticalSidebarResizeObserver: ResizeObserver | undefined

function scaleRightSidebar() {
  rightWidth.value = clampRight(availableRightSidebarWidth() * rightWidthRatio.value)
}

function scaleVerticalSidebars() {
  topHeight.value = clampTop(availableVerticalSidebarHeight() * topHeightRatio.value)
  bottomHeight.value = clampBottom(availableVerticalSidebarHeight() * bottomHeightRatio.value)
}

function scaleSidebars() {
  scaleRightSidebar()
  scaleVerticalSidebars()
}

useTauriDrop((paths, position) => {
  sidebarDropHint.value = null
  if (store.sidebarOpen) return
  if (!paths[0]) return
  if (isInSidebarDropZone(position, contentRef.value)) {
    store.openFile(paths[0])
  } else if (isInTopSidebarDropZone(position, contentRef.value)) {
    store.openFile(paths[0], 'top')
  }
}, {
  onOver: (position) => {
    if (store.sidebarOpen) {
      sidebarDropHint.value = null
    } else if (isInSidebarDropZone(position, contentRef.value)) {
      sidebarDropHint.value = 'right'
    } else if (isInTopSidebarDropZone(position, contentRef.value)) {
      sidebarDropHint.value = 'top'
    } else {
      sidebarDropHint.value = null
    }
  },
  onLeave: () => {
    sidebarDropHint.value = null
  },
})

async function loadWidths() {
  await loadSharedLeftSidebarWidth()
  try {
    const savedRight = await invoke<number | null>('load_pane_width', { key: RIGHT_KEY })
    if (savedRight !== null && savedRight !== undefined) {
      rightWidth.value = clampRight(savedRight)
      rightWidthRatio.value = rightWidth.value / availableRightSidebarWidth()
    }
  } catch {
    // use default
  }
  try {
    const savedRatio = await invoke<number | null>('load_pane_width', { key: RIGHT_RATIO_KEY })
    if (savedRatio !== null && savedRatio !== undefined && Number.isFinite(savedRatio) && savedRatio > 0) {
      rightWidthRatio.value = savedRatio
      scaleRightSidebar()
    }
  } catch {
    // use default
  }
  try {
    const savedTop = await invoke<number | null>('load_pane_width', { key: TOP_KEY })
    if (savedTop !== null && savedTop !== undefined) {
      topHeight.value = clampTop(savedTop)
      topHeightRatio.value = topHeight.value / availableVerticalSidebarHeight()
    }
  } catch {
    // use default
  }
  try {
    const savedTopRatio = await invoke<number | null>('load_pane_width', { key: TOP_RATIO_KEY })
    if (savedTopRatio !== null && savedTopRatio !== undefined && Number.isFinite(savedTopRatio) && savedTopRatio > 0) {
      topHeightRatio.value = savedTopRatio
      topHeight.value = clampTop(availableVerticalSidebarHeight() * topHeightRatio.value)
    }
  } catch {
    // use default
  }
  try {
    const savedBottom = await invoke<number | null>('load_pane_width', { key: BOTTOM_KEY })
    if (savedBottom !== null && savedBottom !== undefined) {
      bottomHeight.value = clampBottom(savedBottom)
      bottomHeightRatio.value = bottomHeight.value / availableVerticalSidebarHeight()
    }
  } catch {
    // use default
  }
  try {
    const savedBottomRatio = await invoke<number | null>('load_pane_width', { key: BOTTOM_RATIO_KEY })
    if (savedBottomRatio !== null && savedBottomRatio !== undefined && Number.isFinite(savedBottomRatio) && savedBottomRatio > 0) {
      bottomHeightRatio.value = savedBottomRatio
      bottomHeight.value = clampBottom(availableVerticalSidebarHeight() * bottomHeightRatio.value)
    }
  } catch {
    // use default
  }
}

async function saveWidth(key: string, value: number) {
  try {
    await invoke('save_pane_width', { key, width: value })
  } catch {
    // ignore
  }
}

watch(rightDivider.isDragging, async (dragging) => {
  if (!dragging) {
    await Promise.all([
      saveWidth(RIGHT_KEY, rightWidth.value),
      saveWidth(RIGHT_RATIO_KEY, rightWidthRatio.value),
    ])
  }
})

watch(topDivider.isDragging, async (dragging) => {
  if (!dragging) {
    await Promise.all([
      saveWidth(TOP_KEY, topHeight.value),
      saveWidth(TOP_RATIO_KEY, topHeightRatio.value),
    ])
  }
})

watch(bottomDivider.isDragging, async (dragging) => {
  if (!dragging) {
    await Promise.all([
      saveWidth(BOTTOM_KEY, bottomHeight.value),
      saveWidth(BOTTOM_RATIO_KEY, bottomHeightRatio.value),
    ])
  }
})

onMounted(async () => {
  await loadWidths()
  window.addEventListener('resize', scaleSidebars)
  rightSidebarResizeObserver = new ResizeObserver(scaleRightSidebar)
  if (contentRef.value) rightSidebarResizeObserver.observe(contentRef.value)
  verticalSidebarResizeObserver = new ResizeObserver(scaleVerticalSidebars)
  if (panelRef.value) verticalSidebarResizeObserver.observe(panelRef.value)
})

onBeforeUnmount(() => {
  window.removeEventListener('resize', scaleSidebars)
  rightSidebarResizeObserver?.disconnect()
  verticalSidebarResizeObserver?.disconnect()
})

watch(() => props.cliKind, (kind) => {
  store.setActiveCliKind(kind)
}, { immediate: true })
</script>

<style scoped>
.project-panel {
  position: relative;
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
  background: var(--bg);
}

.project-panel__row {
  flex: 1;
  min-height: 0;
  display: flex;
  overflow: hidden;
}

.project-panel__main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.project-panel__left-shell,
.project-panel__right-shell {
  flex: 0 0 auto;
  min-width: 0;
  min-height: 0;
  display: flex;
  overflow: hidden;
}

.project-panel__top-shell {
  flex: 0 0 auto;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.project-panel__bottom-shell {
  flex: 0 0 auto;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.left-pane-enter-active,
.left-pane-leave-active,
.right-pane-enter-active,
.right-pane-leave-active {
  transition: width 0.22s ease, flex-basis 0.22s ease, opacity 0.16s ease;
}

.left-pane-enter-from,
.left-pane-leave-to,
.right-pane-enter-from,
.right-pane-leave-to {
  width: 0 !important;
  flex-basis: 0 !important;
  opacity: 0;
}

.top-pane-enter-active,
.top-pane-leave-active,
.bottom-pane-enter-active,
.bottom-pane-leave-active {
  transition: height 0.22s ease, flex-basis 0.22s ease, opacity 0.16s ease;
}

.top-pane-enter-from,
.top-pane-leave-to,
.bottom-pane-enter-from,
.bottom-pane-leave-to {
  height: 0 !important;
  flex-basis: 0 !important;
  opacity: 0;
}

.project-panel__content {
  flex: 1;
  min-height: 0;
  display: flex;
  position: relative;
  overflow: hidden;
}

.project-panel__divider {
  width: 9px;
  flex: 0 0 9px;
  cursor: col-resize;
  background: transparent;
  position: relative;
  z-index: 10;
  display: flex;
  align-items: center;
  justify-content: center;
}

/* The left resize hit area sits between the sidebar and the module toolbar.
   Keep its right half filled with the toolbar surface so that the two bars
   meet cleanly instead of exposing a dark gap at their junction. */
.project-panel__divider--left::before {
  content: '';
  position: absolute;
  top: 0;
  right: 0;
  width: 50%;
  height: 38px;
  pointer-events: none;
  background: var(--chrome-bridge-bg);
}

.project-panel__divider::after {
  content: '';
  width: 1px;
  height: 100%;
  background-color: var(--separator);
  transition: background-color 0.2s ease, width 0.2s ease, box-shadow 0.2s ease;
}

.project-panel__divider:hover::after,
.project-panel__divider--dragging::after {
  width: 2px;
  background-color: var(--primary);
}

.project-panel__divider--horizontal {
  width: auto;
  height: 9px;
  flex: 0 0 9px;
  cursor: row-resize;
}

.project-panel__divider--horizontal::after {
  width: 100%;
  height: 1px;
}

.project-panel__divider--horizontal:hover::after,
.project-panel__divider--horizontal.project-panel__divider--dragging::after {
  width: 100%;
  height: 2px;
  background-color: var(--primary);
}

[data-theme="dark"] .project-panel__divider:hover::after,
[data-theme="dark"] .project-panel__divider--dragging::after {
  box-shadow: 0 0 6px 1px rgba(10, 132, 255, 0.5);
}

.project-panel__drop-hint {
  position: absolute;
  top: 0;
  right: 0;
  bottom: 0;
  width: 20%;
  display: grid;
  place-items: center;
  border: 2px dashed var(--primary);
  border-radius: var(--radius);
  margin: 8px;
  background: rgba(0, 122, 255, 0.08);
  color: var(--primary);
  font-size: var(--font-size-small);
  pointer-events: none;
  z-index: 25;
}

.project-panel__drop-hint--top {
  top: 0;
  left: 0;
  right: 0;
  bottom: auto;
  width: auto;
  height: 20%;
}

.project-panel__toast {
  position: absolute;
  left: 50%;
  bottom: 18px;
  transform: translateX(-50%);
  max-width: min(480px, calc(100% - 48px));
  padding: 8px 12px;
  border-radius: 999px;
  background: rgba(29, 29, 31, 0.92);
  color: #fff;
  font-size: var(--font-size-small);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.18);
  z-index: 30;
}
</style>
