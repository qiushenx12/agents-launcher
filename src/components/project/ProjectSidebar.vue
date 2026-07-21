<template>
  <aside
    ref="sidebarRef"
    class="project-sidebar"
    :style="props.width ? { width: `${props.width}px`, flexBasis: `${props.width}px` } : undefined"
  >
    <header class="project-sidebar__header">
      <button
        class="project-sidebar__title"
        :title="projectListExpanded ? '收起项目列表' : '展开项目列表'"
        @click="projectListExpanded = !projectListExpanded"
      >
        {{ CLI_DESCRIPTORS[store.activeCliKind].label }} 项目
        <span>{{ projectListExpanded ? '▾' : '▸' }}</span>
      </button>
      <div class="project-sidebar__actions">
        <button
          class="icon-btn project-sidebar__menu-btn"
          title="项目功能"
          aria-label="项目功能"
          @click.stop="toggleProjectOptions"
        >
          ⋯
        </button>
        <button class="icon-btn icon-btn--primary" title="新增项目目录" @click="store.pickAndAddProject">
          <svg class="add-project-icon" width="16" height="16" viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg" fill="currentColor">
            <path d="M2 4.5V6H5.58579C5.71839 6 5.84557 5.94732 5.93934 5.85355L7.29289 4.5L5.93934 3.14645C5.84557 3.05268 5.71839 3 5.58579 3H3.5C2.67157 3 2 3.67157 2 4.5ZM1 4.5C1 3.11929 2.11929 2 3.5 2H5.58579C5.98361 2 6.36514 2.15804 6.64645 2.43934L8.20711 4H12.5C13.8807 4 15 5.11929 15 6.5V7.25716C14.6929 7.00353 14.3578 6.78261 14 6.59971V6.5C14 5.67157 13.3284 5 12.5 5H8.20711L6.64645 6.56066C6.36514 6.84197 5.98361 7 5.58579 7H2V11.5C2 12.3284 2.67157 13 3.5 13H6.20703C6.30564 13.3486 6.43777 13.6832 6.59971 14H3.5C2.11929 14 1 12.8807 1 11.5V4.5ZM16 11.5C16 13.9853 13.9853 16 11.5 16C9.01472 16 7 13.9853 7 11.5C7 9.01472 9.01472 7 11.5 7C13.9853 7 16 9.01472 16 11.5ZM12 9C12 8.72386 11.7761 8.5 11.5 8.5C11.2239 8.5 11 8.72386 11 9V11H9C8.72386 11 8.5 11.2239 8.5 11.5C8.5 11.7761 8.72386 12 9 12H11V14C11 14.2761 11.2239 14.5 11.5 14.5C11.7761 14.5 12 14.2761 12 14V12H14C14.2761 12 14.5 11.7761 14.5 11.5C14.5 11.2239 14.2761 11 14 11H12V9Z"/>
          </svg>
        </button>
        <div v-if="projectOptionsOpen" class="project-options-menu">
          <div class="project-options-menu__label">项目排序</div>
          <button
            class="project-options-menu__item"
            :class="{ active: store.projectSortMode === 'manual' }"
            @click="setProjectSortMode('manual')"
          >
            <span>{{ store.projectSortMode === 'manual' ? '✓' : '' }}</span>
            手动排序
          </button>
          <button
            class="project-options-menu__item"
            :class="{ active: store.projectSortMode === 'time' }"
            @click="setProjectSortMode('time')"
          >
            <span>{{ store.projectSortMode === 'time' ? '✓' : '' }}</span>
            时间排序
          </button>
        </div>
      </div>
    </header>

    <div v-show="projectListExpanded" class="project-sidebar__tree">
      <div
        v-for="(project, index) in sortedProjects"
        :key="project.id"
        data-drag-item
        class="project-block"
        :class="{
          'project-block--dragging': projectDraggingIndex === index,
          'project-block--drag-over': projectDraggingIndex !== null && projectDraggingIndex !== index && projectOverIndex === index,
          'project-block--menu-open': openMenuProjectId === project.id,
        }"
      >
        <div
          class="project-row"
          :class="{
            active: project.id === store.activeProjectId,
            'project-row--sortable': store.projectSortMode === 'manual',
          }"
          :data-project-id="project.id"
          @pointerdown="onProjectRowPointerDown(index, $event)"
          @click="onProjectRowClick(project.id)"
        >
          <span class="project-row__toggle">{{ isExpanded(project.id) ? '▾' : '▸' }}</span>
          <span class="project-row__folder">
            <svg viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
              <path d="M1.5 3C1.5 2.17157 2.17157 1.5 3 1.5H5.5L7 3.5H13C13.8284 3.5 14.5 4.17157 14.5 5V12C14.5 12.8284 13.8284 13.5 13 13.5H3C2.17157 13.5 1.5 12.8284 1.5 12V3Z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"/>
            </svg>
          </span>
          <span class="project-row__name">{{ project.name }}</span>
          <div class="project-row__actions" @click.stop>
            <button class="project-row__action" title="项目更多菜单" @click="toggleProjectActions(project.id)">
              ⋯
            </button>
            <button class="project-row__action project-row__action--new" title="新建项目会话" @click="store.createSession(project.id)" />
          </div>
        </div>

        <div v-if="openMenuProjectId === project.id" class="project-actions-menu">
          <button @click="renameProject(project.id)">✎ 重命名项目</button>
          <button @click="openProjectDirectory(project.path)">↗ 打开项目目录</button>
          <button class="danger" @click="removeProject(project.id)">⌫ 删除项目</button>
        </div>

        <div v-if="isExpanded(project.id)" class="session-list">
          <div
            v-for="session in sessionsForProject(project.id)"
            :key="session.id"
            class="session-row"
            :class="{
              active: session.id === store.activeSessionId,
              'session-row--closeable': sessionIsCloseable(session.id),
            }"
            @click="store.activateSession(session.id)"
            @contextmenu.prevent="renameSession(session.id)"
          >
            <span class="session-row__status" :class="sessionStatus(session.id)" />
            <span class="session-row__name">{{ displaySessionName(session.name) }}</span>
            <span class="session-row__meta">
              <span class="session-row__time">{{ formatRelativeTime(session.updatedAt) }}</span>
              <button
                v-if="sessionIsCloseable(session.id)"
                type="button"
                class="session-row__close"
                title="关闭终端"
                @click.stop="store.closeSessionTerminal(session.id)"
              >
                ×
              </button>
            </span>
          </div>
          <button
            v-if="showSessionToggle(project.id)"
            type="button"
            class="session-list__more"
            @click.stop.prevent="toggleSessionDisplay(project.id)"
          >
            {{ sessionToggleLabel(project.id) }}
          </button>
        </div>
      </div>

      <button
        v-if="store.visibleProjects.length === 0"
        class="project-empty"
        @click="store.pickAndAddProject"
      >
        选择项目目录开始
      </button>
    </div>

    <footer class="project-sidebar__footer">
      <button class="settings-entry" @click="emit('open-settings')">⚙ <span>设置</span></button>
    </footer>
  </aside>
</template>

<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useProjectStore } from '@/stores/project'
import type { Project, ProjectSession, ProjectSortMode } from '@/stores/project'
import { useTauriDrop, isInside } from '@/composables/useTauriDrop'
import { useDragReorder } from '@/composables/useDragReorder'
import { CLI_DESCRIPTORS } from '@/types/cli'

const store = useProjectStore()
const emit = defineEmits<{
  (event: 'open-settings'): void
}>()
const projectOptionsOpen = ref(false)
const openMenuProjectId = ref<string | null>(null)
const projectListExpanded = ref(true)
const sidebarRef = ref<HTMLElement | null>(null)
const currentTime = ref(Date.now())
const sessionDisplayLimits = ref<Record<string, number>>({})
let relativeTimeTimer: number | undefined
const SESSION_DISPLAY_STEP = 5

useTauriDrop((paths, position) => {
  if (!isInside(position, sidebarRef.value)) return
  const path = paths[0]
  if (!path) return

  // If dropped over a specific project row, treat it as a project-targeted drop.
  const target = document.elementFromPoint(position.x, position.y)
  const projectRow = target?.closest('.project-row') as HTMLElement | null
  const targetProjectId = projectRow?.dataset.projectId

  handleDroppedPath(path, targetProjectId)
})

const props = defineProps<{
  width?: number
}>()

const sortedProjects = computed(() =>
  [...store.visibleProjects].sort((a, b) => {
    if (store.projectSortMode === 'time') {
      const byTime = latestSessionTimestamp(b.id) - latestSessionTimestamp(a.id)
      if (byTime !== 0) return byTime
    }
    return safeNumber(a.order) - safeNumber(b.order)
  })
)

const {
  draggingIndex: projectDraggingIndex,
  overIndex: projectOverIndex,
  justDragged: projectJustDragged,
  onPointerDown: onProjectDragPointerDown,
} = useDragReorder<Project>(
  () => sortedProjects.value,
  (newProjects) => store.reorderProjects(newProjects.map((project) => project.id)),
  {
    startDelayMs: 420,
    onDragStart: async (project) => {
      if (!project || !isExpanded(project.id)) return
      resetSessionDisplay(project.id)
      const saving = store.toggleProjectExpanded(project.id)
      await nextTick()
      saving.catch(() => {})
    },
  },
)

const sessionsByProject = computed<Record<string, ProjectSession[]>>(() => {
  const groups: Record<string, ProjectSession[]> = {}
  for (const session of store.visibleSessions) {
    if (!session?.projectId) continue
    const group = groups[session.projectId] ?? []
    group.push(session)
    groups[session.projectId] = group
  }

  for (const group of Object.values(groups)) {
    group.sort((a, b) => {
      const byTime = sessionTimestamp(b.updatedAt) - sessionTimestamp(a.updatedAt)
      return byTime || safeNumber(a.order) - safeNumber(b.order)
    })
  }

  return groups
})

onMounted(() => {
  relativeTimeTimer = window.setInterval(() => {
    currentTime.value = Date.now()
  }, 10_000)
  document.addEventListener('click', closeProjectActionsOnOutsideClick)
})

onUnmounted(() => {
  if (relativeTimeTimer) window.clearInterval(relativeTimeTimer)
  document.removeEventListener('click', closeProjectActionsOnOutsideClick)
})

watch(() => store.sessions, () => {
  currentTime.value = Date.now()
}, { deep: true })

function isExpanded(projectId: string) {
  return store.expandedProjectIds.has(projectId)
}

function allSessionsForProject(projectId: string) {
  return sessionsByProject.value[projectId] ?? []
}

function sessionDisplayLimit(projectId: string) {
  const limit = sessionDisplayLimits.value[projectId]
  return Number.isFinite(limit) && limit > SESSION_DISPLAY_STEP ? limit : SESSION_DISPLAY_STEP
}

function sessionsForProject(projectId: string) {
  return allSessionsForProject(projectId).slice(0, sessionDisplayLimit(projectId))
}

function displaySessionName(name: unknown) {
  const text = String(name ?? '').replace(/\s+/g, ' ').trim()
  const fallback = '未命名会话'
  const normalized = text || fallback
  return normalized.length > 80 ? `${normalized.slice(0, 80)}...` : normalized
}

function sessionStatus(sessionId: unknown) {
  try {
    return typeof sessionId === 'string' && sessionId ? store.getSessionStatus(sessionId) : 'off'
  } catch {
    return 'off'
  }
}

function sessionIsCloseable(sessionId: unknown) {
  return sessionStatus(sessionId) !== 'off'
}

function hiddenSessionCount(projectId: string) {
  return Math.max(0, allSessionsForProject(projectId).length - sessionDisplayLimit(projectId))
}

function showSessionToggle(projectId: string) {
  return allSessionsForProject(projectId).length > SESSION_DISPLAY_STEP
}

function sessionToggleLabel(projectId: string) {
  return hiddenSessionCount(projectId) > 0 ? '展开显示' : '折叠显示'
}

function resetSessionDisplay(projectId: string) {
  if (!Object.prototype.hasOwnProperty.call(sessionDisplayLimits.value, projectId)) return
  const next = { ...sessionDisplayLimits.value }
  delete next[projectId]
  sessionDisplayLimits.value = next
}

async function toggleSessionDisplay(projectId: string) {
  try {
    const total = allSessionsForProject(projectId).length
    const current = sessionDisplayLimit(projectId)

    if (current >= total) {
      resetSessionDisplay(projectId)
      await nextTick()
      return
    }

    sessionDisplayLimits.value = {
      ...sessionDisplayLimits.value,
      [projectId]: Math.min(current + SESSION_DISPLAY_STEP, total),
    }
    await nextTick()
  } catch (e) {
    resetSessionDisplay(projectId)
    store.statusMessage = `展开会话失败：${e}`
  }
}

function safeNumber(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

function latestSessionTimestamp(projectId: string) {
  const latest = store.sessions
    .filter((session) => session.projectId === projectId)
    .reduce((max, session) => Math.max(max, sessionTimestamp(session.updatedAt)), 0)
  const project = store.projects.find((item) => item.id === projectId)
  return latest || sessionTimestamp(project?.updatedAt)
}

function sessionTimestamp(ts: unknown) {
  const value = typeof ts === 'number' ? ts : Number(ts)
  if (!Number.isFinite(value) || value <= 0) return 0
  return value > 1_000_000_000_000 ? value : value * 1000
}

function formatRelativeTime(ts: unknown) {
  const ms = sessionTimestamp(ts)
  if (!ms) return ''

  const diff = Math.max(0, currentTime.value - ms)
  const minute = 60_000
  const hour = 60 * minute
  const day = 24 * hour
  const month = 30 * day

  if (diff < minute) return '刚刚'
  if (diff < hour) return `${Math.floor(diff / minute)} 分钟`
  if (diff < day) return `${Math.floor(diff / hour)} 小时`
  if (diff < month) return `${Math.floor(diff / day)} 天`
  return `${Math.floor(diff / month)} 月`
}

async function onProjectRowPointerDown(index: number, event: PointerEvent) {
  if (store.projectSortMode !== 'manual') return
  const target = event.target as HTMLElement | null
  if (target?.closest('button') || target?.closest('.project-row__actions')) return
  onProjectDragPointerDown(index, event)
}

async function onProjectRowClick(projectId: string) {
  if (projectJustDragged.value) return
  if (isExpanded(projectId)) {
    resetSessionDisplay(projectId)
  }
  await store.toggleProjectExpanded(projectId)
}

function toggleProjectOptions() {
  projectOptionsOpen.value = !projectOptionsOpen.value
  openMenuProjectId.value = null
}

async function setProjectSortMode(mode: ProjectSortMode) {
  await store.setProjectSortMode(mode)
  projectOptionsOpen.value = false
}

function toggleProjectActions(projectId: string) {
  openMenuProjectId.value = openMenuProjectId.value === projectId ? null : projectId
}

function closeProjectActionsOnOutsideClick(event: MouseEvent) {
  const target = event.target as HTMLElement | null
  if (
    projectOptionsOpen.value
    && !target?.closest('.project-options-menu')
    && !target?.closest('.project-sidebar__menu-btn')
  ) {
    projectOptionsOpen.value = false
  }
  if (!openMenuProjectId.value) return
  if (target?.closest('.project-actions-menu') || target?.closest('.project-row__actions')) return
  openMenuProjectId.value = null
}

async function renameProject(projectId: string) {
  openMenuProjectId.value = null
  const project = store.projects.find((item) => item.id === projectId)
  if (!project) return
  const name = window.prompt('重命名项目', project.name)
  if (name) await store.renameProject(projectId, name)
}

async function renameSession(sessionId: string) {
  const session = store.sessions.find((item) => item.id === sessionId)
  if (!session) return
  const name = window.prompt('重命名会话', session.name)
  if (name) await store.renameSession(sessionId, name)
}

async function removeProject(projectId: string) {
  openMenuProjectId.value = null
  const project = store.projects.find((item) => item.id === projectId)
  if (!project) return
  if (window.confirm(`确定删除项目“${project.name}”吗？会先关闭该项目下运行中的终端。`)) {
    await store.removeProject(projectId)
  }
}

async function openProjectDirectory(path: string) {
  openMenuProjectId.value = null
  await invoke('open_directory', { path }).catch(() => {})
}

async function inspectPath(path: string) {
  try {
    return await invoke<'directory' | 'file' | 'missing'>('path_kind', { path })
  } catch (e) {
    store.statusMessage = `无法识别拖拽路径：${e}`
    return null
  }
}

async function handleDroppedPath(path: string, targetProjectId?: string) {
  const kind = await inspectPath(path)
  if (!kind) return

  if (kind === 'directory') {
    if (!targetProjectId) {
      await store.addProject(path)
      return
    }

    const project = store.projects.find((item) => item.id === targetProjectId)
    if (!project) return
    const replace = window.confirm(
      `将项目“${project.name}”的路径替换为：\n${path}\n\n选择“取消”则作为新项目打开该目录。`,
    )
    if (replace) {
      await store.replaceProjectPath(targetProjectId, path)
    } else {
      await store.addProject(path)
    }
    return
  }

  if (kind === 'file') {
    await store.openFile(path)
    return
  }

  store.statusMessage = `无法打开该路径：${path}`
}

</script>

<style scoped>
.project-sidebar {
  width: 210px;
  flex: 0 0 210px;
  display: flex;
  flex-direction: column;
  min-height: 0;
  background: var(--bg);
  border-right: 1px solid var(--separator);
  position: relative;
}

.project-sidebar__header {
  min-height: 44px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  padding: 8px;
  border-bottom: 1px solid var(--separator);
  background: var(--card);
  position: relative;
}

.project-sidebar__title,
.settings-entry,
.icon-btn,
.project-row__action,
.session-row,
.project-empty {
  font-family: var(--font-base);
}

.project-sidebar__title {
  border: 0;
  background: transparent;
  color: var(--text-primary);
  font-weight: 700;
  cursor: pointer;
}

.project-sidebar__actions {
  position: relative;
  display: flex;
  gap: 4px;
}

.icon-btn {
  width: 28px;
  height: 28px;
  display: grid;
  place-items: center;
  border: 1px solid var(--separator);
  border-radius: var(--radius-sm);
  background: var(--card);
  color: var(--text-secondary);
  cursor: pointer;
  font-size: var(--font-size-small);
}

.icon-btn--primary {
  color: var(--primary);
  background: rgba(0, 122, 255, 0.08);
}

.add-project-icon {
  width: 18px;
  height: 18px;
  display: block;
}

.project-options-menu {
  position: absolute;
  z-index: 30;
  right: 34px;
  top: 34px;
  min-width: 150px;
  padding: 6px;
  border: 1px solid var(--separator);
  border-radius: var(--radius);
  background: var(--card);
  box-shadow: 0 10px 28px rgba(0, 0, 0, 0.16);
}

.project-options-menu__label {
  padding: 4px 8px 6px;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
}

.project-options-menu__item {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 8px;
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-primary);
  font-family: var(--font-base);
  text-align: left;
  cursor: pointer;
}

.project-options-menu__item span {
  width: 14px;
  flex: 0 0 14px;
  color: var(--primary);
}

.project-options-menu__item:hover {
  background: var(--tab-bg);
}

.project-options-menu__item.active {
  color: var(--primary);
}

.project-actions-menu {
  position: absolute;
  z-index: 20;
  right: 6px;
  top: 30px;
  min-width: 170px;
  padding: 6px;
  border: 1px solid var(--separator);
  border-radius: var(--radius);
  background: var(--card);
  box-shadow: 0 10px 28px rgba(0, 0, 0, 0.16);
}

.project-actions-menu button {
  width: 100%;
  display: flex;
  gap: 8px;
  align-items: center;
  padding: 7px 8px;
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-primary);
  text-align: left;
  cursor: pointer;
}

.project-actions-menu button:hover {
  background: var(--tab-bg);
}

.project-sidebar__tree {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 8px;
}

.project-block {
  position: relative;
  transition: transform 0.18s ease;
  will-change: transform;
}

.project-block--menu-open {
  z-index: 10;
}

.project-block--dragging {
  opacity: 0.35;
  background: rgba(0, 122, 255, 0.08);
  border-radius: var(--radius-sm);
}

.project-row,
.session-row {
  width: 100%;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 6px;
  min-height: 30px;
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-primary);
  cursor: pointer;
  text-align: left;
}

.project-row {
  padding: 5px 4px 5px 6px;
}

.project-row--sortable {
  touch-action: none;
}

.project-row__toggle,
.project-row__folder {
  flex: 0 0 16px;
  height: 16px;
  display: grid;
  place-items: center;
}

.project-row__folder svg {
  width: 16px;
  height: 16px;
  display: block;
  color: var(--text-secondary);
}

.project-row__name,
.session-row__name {
  min-width: 0;
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.project-row__actions {
  flex: 0 0 auto;
  display: flex;
  gap: 3px;
  margin-left: auto;
}

.project-row__action {
  position: relative;
  width: 22px;
  height: 22px;
  border: 0;
  border-radius: 5px;
  background: transparent;
  color: currentColor;
  cursor: pointer;
  display: grid;
  place-items: center;
  font-weight: 700;
}

.project-row__action:hover {
  background: rgba(0, 122, 255, 0.12);
}

.project-row__action--new::before {
  content: '';
  width: 11px;
  height: 11px;
  border: 1.5px solid currentColor;
  border-radius: 3px;
}

.project-row__action--new::after {
  content: '';
  position: absolute;
  right: 4px;
  top: 6px;
  width: 8px;
  height: 1.6px;
  border-radius: 999px;
  background: currentColor;
  transform: rotate(-45deg);
}

.project-actions-menu .danger {
  color: var(--danger);
}

.session-list {
  padding: 2px 0 6px 20px;
}

.session-row {
  padding: 5px 6px 5px 7px;
  gap: 7px;
}

.session-row:hover {
  background: rgba(0, 122, 255, 0.06);
}

.session-row__meta {
  flex: 0 0 auto;
  margin-left: auto;
  min-width: 46px;
  height: 24px;
  display: grid;
  align-items: center;
  justify-items: end;
}

.session-row__time,
.session-row__close {
  grid-area: 1 / 1;
}

.session-row__time {
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  line-height: 1;
  white-space: nowrap;
  opacity: 1;
  pointer-events: none;
  transition: opacity 0.12s ease;
}

.session-row__close {
  width: 28px;
  height: 24px;
  display: grid;
  place-items: center;
  border: 0;
  border-radius: 4px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  font-size: 14px;
  line-height: 1;
  opacity: 0;
  pointer-events: none;
  z-index: 1;
  transition: opacity 0.12s ease, background-color 0.12s ease, color 0.12s ease;
}

.session-row--closeable:hover .session-row__time {
  opacity: 0;
}

.session-row--closeable:hover .session-row__close {
  opacity: 1;
  pointer-events: auto;
}

.session-row__close:hover {
  color: var(--danger);
  background-color: rgba(255, 59, 48, 0.1);
}

.session-row__meta:hover .session-row__close {
  color: var(--danger);
  background-color: rgba(255, 59, 48, 0.1);
}

.session-list__more {
  display: block;
  width: 100%;
  padding: 5px 7px 3px 25px;
  border: 0;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  font-family: var(--font-base);
  font-size: var(--font-size-small);
  text-align: left;
}

.session-list__more:hover {
  color: var(--primary);
}

.session-row.active {
  background: rgba(0, 122, 255, 0.1);
}

.session-row__status {
  width: 8px;
  height: 8px;
  border-radius: 999px;
  flex: 0 0 auto;
}

.session-row__status.off {
  background: #a1a1aa;
}

.session-row__status.idle {
  background: var(--success);
}

.session-row__status.running {
  background: var(--dot-running);
  animation: status-blink 1s steps(2, end) infinite;
}

@keyframes status-blink {
  0% {
    opacity: 1;
  }
  50%, 100% {
    opacity: 0;
  }
}

.project-empty {
  width: 100%;
  padding: 18px 10px;
  border: 1px dashed var(--separator);
  border-radius: var(--radius);
  background: var(--card);
  color: var(--text-secondary);
  cursor: pointer;
}

.project-sidebar__footer {
  padding: 8px;
  border-top: 1px solid var(--separator);
}

</style>
