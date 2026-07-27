<template>
  <main
    ref="terminalRef"
    class="project-terminal"
    :class="{ 'project-terminal--drag-over': dragOver }"
  >
    <div v-if="!store.activeProject" class="project-terminal__empty">
      <div
        v-if="store.activeCliKind === 'codex'"
        class="project-terminal__empty-title"
      >
        CodeX 会同步桌面版和 CLI 的共享会话。请先选择项目目录，启动器会按目录加载真实会话。
      </div>
      <div class="project-terminal__actions">
        <button class="btn btn-primary" @click="store.pickAndAddProject">
          选择项目目录
        </button>
        <button
          v-if="store.activeCliKind === 'codex'"
          class="btn btn-secondary"
          @click="store.pickAndResumeCodexSession"
        >
          选择目录并原生恢复
        </button>
      </div>
    </div>

    <ClaudeConversationPane
      v-else-if="showClaudeEmptyComposer"
      ref="conversationEmptyPaneRef"
      :key="`startup-${claudeEmptyPaneSessionId}`"
      :project-id="store.activeProject!.id"
      :project-name="store.activeProject!.name"
      :session-id="claudeEmptyPaneSessionId"
      :startup-mode="true"
      :startup-pending="claudeEmptyPending"
      v-model:initial-permission-mode="claudeStartupPermissionMode"
      :external-error="claudeEmptyError"
      :submit-startup-prompt="submitClaudeEmptyPrompt"
      :clear-session-draft="clearClaudeSessionDraft"
      :restore-session-draft="restoreClaudeSessionDraft"
      v-model:session-draft="activeClaudeSessionDraft"
      v-model:session-attachment-paths="activeClaudeSessionAttachmentPaths"
    />

    <div v-else-if="!store.activeSession" class="project-terminal__empty">
      <div class="project-terminal__actions">
        <button class="btn btn-primary" @click="store.createSession()">新建项目会话</button>
        <button
          v-if="store.activeCliKind === 'codex'"
          class="btn btn-secondary"
          @click="store.resumeCodexSession()"
        >
          原生恢复会话
        </button>
        <button
          v-if="store.activeCliKind === 'codex' || store.activeCliKind === 'opencode'"
          class="btn btn-secondary"
          @click="store.refreshActiveCliHistory()"
        >
          刷新真实会话
        </button>
      </div>
    </div>

    <template v-else>
      <template v-if="terminalTabIds.length > 0">
        <TerminalPane
          v-for="tabId in terminalTabIds"
          :key="tabId"
          :tab-id="tabId"
          :active="tabId === activeTerminalId"
        />
      </template>
      <template v-if="activeTerminalId && isActiveClaudeSession">
        <ClaudeConversationPane
          v-if="activeClaudeView === 'conversation'"
          ref="conversationPaneRef"
          :key="`conversation-${activeTerminalId}`"
          :tab-id="activeTerminalId"
          :project-id="store.activeSession!.projectId"
          :project-name="store.activeProject!.name"
          :session-id="store.activeSession!.id"
          :startup-pending="activeClaudeStartupPending"
          :initial-permission-mode="claudeStartupPermissionMode"
          :clear-session-draft="clearClaudeSessionDraft"
          :restore-session-draft="restoreClaudeSessionDraft"
          v-model:session-draft="activeClaudeSessionDraft"
          v-model:session-attachment-paths="activeClaudeSessionAttachmentPaths"
          @show-terminal="emit('select-claude-view', 'terminal')"
        />
        <ClaudeTerminalLogPane
          v-else-if="activeClaudeView === 'log'"
          :key="`log-${activeTerminalId}`"
          :tab-id="activeTerminalId"
          :project-session-id="store.activeSession!.id"
        />
      </template>
      <div v-if="!activeTerminalId" class="project-terminal__empty">
        <div class="project-terminal__project-name">{{ store.activeProject?.name }}</div>
        <template v-if="isFreshProject">
          <button
            class="btn btn-primary project-terminal__action-btn"
            @click="store.ensureSessionTerminal(store.activeSession!.id)"
          >
            新会话
          </button>
        </template>
        <template v-else>
          <div class="project-terminal__empty-title">
            {{ store.activeSession?.name }} 未开启
          </div>
          <div class="project-terminal__actions">
            <button class="btn btn-secondary project-terminal__action-btn" @click="store.createSession()">
              新会话
            </button>
            <button
              class="btn btn-primary project-terminal__action-btn"
              @click="store.ensureSessionTerminal(store.activeSession!.id)"
            >
              继续对话
            </button>
            <button
              v-if="store.activeCliKind === 'codex'"
              class="btn btn-secondary project-terminal__action-btn"
              @click="store.resumeCodexSession()"
            >
              原生恢复
            </button>
            <button
              v-if="store.activeCliKind === 'codex' || store.activeCliKind === 'opencode'"
              class="btn btn-secondary project-terminal__action-btn"
              @click="store.refreshActiveCliHistory()"
            >
              刷新真实会话
            </button>
          </div>
        </template>
      </div>
    </template>
  </main>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useProjectStore } from '@/stores/project'
import { useClaudeStore } from '@/stores/claude'
import { useTerminalStore } from '@/stores/terminal'
import { useClaudeObserverStore } from '@/stores/claudeObserver'
import { useTauriDrop, isInside } from '@/composables/useTauriDrop'
import { isInSidebarDropZone, isInTopSidebarDropZone } from '@/composables/useSidebarDropZone'
import TerminalPane from '@/components/terminal/TerminalPane.vue'
import ClaudeConversationPane from '@/components/claude/conversation/ClaudeConversationPane.vue'
import ClaudeTerminalLogPane from '@/components/claude/conversation/ClaudeTerminalLogPane.vue'
import {
  ClaudeStartupPromptCancelledError,
  waitForClaudePromptReady,
} from '@/utils/claudeStartupPrompt'

type ClaudeView = 'conversation' | 'terminal' | 'log'
type ClaudeDefaultPermissionMode = 'bypassPermissions' | 'auto' | 'default' | 'acceptEdits' | 'plan'

const store = useProjectStore()
const claudeStore = useClaudeStore()
const claudeObserverStore = useClaudeObserverStore()
const terminalRef = ref<HTMLElement | null>(null)
const conversationPaneRef = ref<InstanceType<typeof ClaudeConversationPane> | null>(null)
const conversationEmptyPaneRef = ref<InstanceType<typeof ClaudeConversationPane> | null>(null)
const dragOver = ref(false)
const claudeEmptyError = ref('')
const claudeEmptyPending = ref(false)
const claudeStartupPermissionMode = ref<ClaudeDefaultPermissionMode>('auto')
const claudeEmptyDrafts = ref<Record<string, string>>({})
const claudeEmptyAttachmentPaths = ref<Record<string, string[]>>({})
const claudeEmptyPrompt = computed({
  get() {
    const key = claudeEmptyDraftKey(store.activeProjectId, store.activeSessionId)
    return key ? (claudeEmptyDrafts.value[key] ?? '') : ''
  },
  set(prompt: string) {
    const key = claudeEmptyDraftKey(store.activeProjectId, store.activeSessionId)
    setClaudeEmptyDraft(key, prompt)
  },
})
const props = withDefaults(defineProps<{
  claudeView?: ClaudeView
}>(), {
  claudeView: 'conversation',
})
const emit = defineEmits<{
  (event: 'select-claude-view', view: ClaudeView): void
}>()

interface ClaudeEmptyOperation {
  id: number
  projectId: string
  sessionId: string | null
  draftKey: string
  cancelled: boolean
}

interface ClaudeEmptyTarget {
  projectId: string
  sessionId: string
  tabId: number
}

let claudeEmptyOperationSequence = 0
let activeClaudeEmptyOperation: ClaudeEmptyOperation | null = null
let claudeEmptyDisposed = false

const activeTerminalId = computed(() => {
  const sessionId = store.activeSessionId
  return sessionId ? store.sessionTerminalIds[sessionId] : undefined
})

const terminalTabIds = computed(() =>
  store.visibleSessions
    .map((session) => store.sessionTerminalIds[session.id])
    .filter((id): id is number => typeof id === 'number')
)

const isActiveClaudeSession = computed(() => store.activeSession?.cliKind === 'claude')

const showClaudeEmptyComposer = computed(() => (
  !!store.activeProject
  && store.activeCliKind === 'claude'
  && !activeTerminalId.value
))

const claudeEmptyPaneSessionId = computed(() => (
  store.activeSession?.id ?? `new:${store.activeProjectId ?? 'claude'}`
))

const activeClaudeStartupPending = computed(() => {
  const operation = activeClaudeEmptyOperation
  return !!operation
    && claudeEmptyPending.value
    && !operation.cancelled
    && operation.projectId === store.activeProjectId
    && operation.sessionId === store.activeSessionId
})

const activeClaudeSessionDraft = computed({
  get() {
    const key = claudeEmptyDraftKey(store.activeProjectId, store.activeSessionId)
    return key ? (claudeEmptyDrafts.value[key] ?? '') : ''
  },
  set(prompt: string) {
    const key = claudeEmptyDraftKey(store.activeProjectId, store.activeSessionId)
    setClaudeEmptyDraft(key, prompt)
  },
})

const activeClaudeSessionAttachmentPaths = computed({
  get() {
    const key = claudeEmptyDraftKey(store.activeProjectId, store.activeSessionId)
    return key ? (claudeEmptyAttachmentPaths.value[key] ?? []) : []
  },
  set(paths: string[]) {
    const key = claudeEmptyDraftKey(store.activeProjectId, store.activeSessionId)
    if (!key) return
    if (paths.length) claudeEmptyAttachmentPaths.value[key] = paths
    else delete claudeEmptyAttachmentPaths.value[key]
  },
})

const activeClaudeView = computed<ClaudeView>(() => props.claudeView)

const isFreshProject = computed(() => {
  const session = store.activeSession
  if (!session || activeTerminalId.value) return false
  return store.sessionsOfActiveProject.every((s) => !s.nativeSessionId && !s.claudeSessionId)
})

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

function claudeEmptyDraftKey(projectId: string | null, sessionId: string | null) {
  return projectId ? `${projectId}\u0000${sessionId ?? 'new'}` : ''
}

function setClaudeEmptyDraft(key: string, prompt: string) {
  if (!key) return
  if (prompt) claudeEmptyDrafts.value[key] = prompt
  else delete claudeEmptyDrafts.value[key]
}

function clearClaudeSessionDraft(projectId: string, sessionId: string) {
  const key = claudeEmptyDraftKey(projectId, sessionId)
  if (key) delete claudeEmptyDrafts.value[key]
}

function restoreClaudeSessionDraft(projectId: string, sessionId: string, prompt: string) {
  const key = claudeEmptyDraftKey(projectId, sessionId)
  if (!key) return
  const existing = claudeEmptyDrafts.value[key] ?? ''
  setClaudeEmptyDraft(key, existing.trim() ? `${prompt}\n${existing}` : prompt)
}

function moveClaudeEmptyOperationDraft(operation: ClaudeEmptyOperation, sessionId: string) {
  const nextKey = claudeEmptyDraftKey(operation.projectId, sessionId)
  if (nextKey === operation.draftKey) return
  const prompt = claudeEmptyDrafts.value[operation.draftKey] ?? claudeEmptyPrompt.value
  setClaudeEmptyDraft(nextKey, prompt)
  delete claudeEmptyDrafts.value[operation.draftKey]
  operation.draftKey = nextKey
}

function beginClaudeEmptyOperation() {
  const projectId = store.activeProjectId
  if (!projectId || store.activeCliKind !== 'claude') {
    throw new Error('请先选择 Claude Code 项目')
  }
  const operation: ClaudeEmptyOperation = {
    id: ++claudeEmptyOperationSequence,
    projectId,
    sessionId: store.activeSession?.id ?? null,
    draftKey: claudeEmptyDraftKey(projectId, store.activeSession?.id ?? null),
    cancelled: false,
  }
  setClaudeEmptyDraft(operation.draftKey, claudeEmptyPrompt.value)
  activeClaudeEmptyOperation = operation
  return operation
}

function isClaudeEmptyOperationCancelled(
  operation: ClaudeEmptyOperation,
  target?: ClaudeEmptyTarget,
) {
  if (
    claudeEmptyDisposed
    || operation.cancelled
    || activeClaudeEmptyOperation !== operation
    || store.activeCliKind !== 'claude'
    || store.activeProjectId !== operation.projectId
    || (operation.sessionId !== null && store.activeSessionId !== operation.sessionId)
  ) return true

  return !!target && (
    target.projectId !== operation.projectId
    || target.sessionId !== operation.sessionId
    || store.sessionTerminalIds[target.sessionId] !== target.tabId
  )
}

function assertClaudeEmptyOperationActive(
  operation: ClaudeEmptyOperation,
  target?: ClaudeEmptyTarget,
) {
  if (isClaudeEmptyOperationCancelled(operation, target)) {
    throw new ClaudeStartupPromptCancelledError()
  }
}

async function ensureClaudeEmptyTerminal(operation: ClaudeEmptyOperation) {
  assertClaudeEmptyOperationActive(operation)
  const launchOptions = {
    publishStatus: false,
    throwOnError: true,
  }
  let session = store.activeSession
  if (!session) {
    const creation = store.createSession(operation.projectId, undefined, launchOptions)
    operation.sessionId = store.activeSessionId
    if (operation.sessionId) moveClaudeEmptyOperationDraft(operation, operation.sessionId)
    session = await creation
  }
  if (!session) throw new Error('无法创建 Claude 会话')
  if (session.projectId !== operation.projectId || session.cliKind !== 'claude') {
    throw new ClaudeStartupPromptCancelledError()
  }
  operation.sessionId = session.id
  moveClaudeEmptyOperationDraft(operation, session.id)
  assertClaudeEmptyOperationActive(operation)

  const tabId = store.sessionTerminalIds[session.id]
    ?? await store.ensureSessionTerminal(session.id, launchOptions)
  if (!tabId) throw new Error('Claude 终端启动失败')
  const target = { projectId: session.projectId, sessionId: session.id, tabId }
  assertClaudeEmptyOperationActive(operation, target)
  return target
}

function finishClaudeEmptyOperation(operation: ClaudeEmptyOperation) {
  if (activeClaudeEmptyOperation !== operation) return
  activeClaudeEmptyOperation = null
  claudeEmptyPending.value = false
}

async function submitClaudeEmptyPrompt(prompt: string) {
  if (claudeEmptyPending.value || !prompt.trim()) return false
  setClaudeEmptyDraft(
    claudeEmptyDraftKey(store.activeProjectId, store.activeSessionId),
    prompt,
  )
  let operation: ClaudeEmptyOperation
  try {
    operation = beginClaudeEmptyOperation()
  } catch (error) {
    const message = errorMessage(error)
    claudeEmptyError.value = message
    store.statusMessage = message
    return false
  }
  claudeEmptyPending.value = true
  claudeEmptyError.value = ''

  try {
    const target = await ensureClaudeEmptyTerminal(operation)
    await waitForClaudePromptReady({
      refresh: () => claudeObserverStore.loadSnapshot(target.tabId),
      readState: () => claudeObserverStore.states[target.tabId],
      isCancelled: () => isClaudeEmptyOperationCancelled(operation, target),
    })
    assertClaudeEmptyOperationActive(operation, target)
    const submitted = await claudeObserverStore.submitPrompt(target.tabId, prompt, {
      isCancelled: () => isClaudeEmptyOperationCancelled(operation, target),
    })
    if (!submitted) throw new ClaudeStartupPromptCancelledError()
    delete claudeEmptyDrafts.value[operation.draftKey]
    return true
  } catch (error) {
    if (
      error instanceof ClaudeStartupPromptCancelledError
      || isClaudeEmptyOperationCancelled(operation)
    ) return false
    const message = errorMessage(error)
    claudeEmptyError.value = message
    store.statusMessage = message
    return false
  } finally {
    finishClaudeEmptyOperation(operation)
  }
}

watch(() => [store.activeProjectId, store.activeSessionId, store.activeCliKind] as const, () => {
  const operation = activeClaudeEmptyOperation
  if (operation && !isClaudeEmptyOperationCancelled(operation)) return
  if (operation) {
    operation.cancelled = true
    finishClaudeEmptyOperation(operation)
  }
  claudeEmptyError.value = ''
}, { immediate: true })

onBeforeUnmount(() => {
  claudeEmptyDisposed = true
  if (activeClaudeEmptyOperation) activeClaudeEmptyOperation.cancelled = true
})

function basename(path: string): string {
  return path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path
}

function normalizeSeparators(path: string): string {
  return path.replace(/[\\/]+$/, '').replace(/\\/g, '/')
}

function isUnderProject(path: string, projectPath: string): boolean {
  const normalizedPath = normalizeSeparators(path).toLowerCase()
  const normalizedProject = normalizeSeparators(projectPath).toLowerCase()
  return normalizedPath === normalizedProject
    || normalizedPath.startsWith(normalizedProject + '/')
}

function relativeToProject(path: string, projectPath: string): string {
  const normalizedPath = normalizeSeparators(path)
  const normalizedProject = normalizeSeparators(projectPath)
  if (normalizedPath.toLowerCase() === normalizedProject.toLowerCase()) return ''
  return normalizedPath.slice(normalizedProject.length + 1)
}

function quotePath(text: string): string {
  const escaped = text.replace(/"/g, '\\"')
  return `"${escaped}"`
}

function formatDroppedPath(path: string): string {
  const project = store.activeProject
  if (!project) {
    return quotePath(path)
  }

  if (!isUnderProject(path, project.path)) {
    return quotePath(path)
  }

  const mode = claudeStore.projectDropPathMode
  const inner = mode === 'filename' ? basename(path) : relativeToProject(path, project.path)
  return quotePath(inner)
}

async function handleDroppedFile(path: string) {
  const tabId = activeTerminalId.value
  if (!tabId) {
    store.statusMessage = '没有可用的项目终端'
    return
  }

  const terminalStore = useTerminalStore()
  const tab = terminalStore.tabs.find((t) => t.id === tabId)
  if (!tab?.alive) {
    store.statusMessage = '当前项目终端未运行'
    return
  }

  const text = formatDroppedPath(path)
  try {
    await invoke('pty_write', { tabId, data: text })
  } catch (e) {
    store.statusMessage = `输入文件路径失败: ${e}`
  }
}

useTauriDrop((paths, position) => {
  dragOver.value = false
  if (!isInside(position, terminalRef.value)) return
  // Sidebar closed: the right 20% and top 20% zones belong to the sidebar-open drop.
  if (!store.sidebarOpen && isInSidebarDropZone(position, terminalRef.value)) return
  if (!store.sidebarOpen && isInTopSidebarDropZone(position, terminalRef.value)) return
  if (!paths.length) return

  // If a Claude conversation pane is visible, send files there as attachments.
  const activeConvPane = conversationPaneRef.value ?? conversationEmptyPaneRef.value
  if (isActiveClaudeSession.value && activeClaudeView.value === 'conversation' && activeConvPane) {
    activeConvPane.appendDroppedFiles(paths)
    return
  }
  if (showClaudeEmptyComposer.value && conversationEmptyPaneRef.value) {
    conversationEmptyPaneRef.value.appendDroppedFiles(paths)
    return
  }

  const path = paths[0]
  if (!path) return
  handleDroppedFile(path)
}, {
  onOver: (position) => {
    if (!store.sidebarOpen && isInSidebarDropZone(position, terminalRef.value)) {
      dragOver.value = false
      return
    }
    if (!store.sidebarOpen && isInTopSidebarDropZone(position, terminalRef.value)) {
      dragOver.value = false
      return
    }
    dragOver.value = isInside(position, terminalRef.value)
  },
  onLeave: () => {
    dragOver.value = false
  },
})
</script>

<style scoped>
.project-terminal {
  flex: 1;
  min-width: 0;
  position: relative;
  background: var(--terminal-bg);
  overflow: hidden;
}

.project-terminal--drag-over::after {
  content: '';
  position: absolute;
  inset: 0;
  border: 2px dashed var(--primary);
  pointer-events: none;
  z-index: 20;
}

.project-terminal__empty {
  position: absolute;
  inset: 0;
  display: grid;
  place-content: center;
  gap: 10px;
  color: var(--text-secondary);
  text-align: center;
}

.project-terminal__project-name {
  max-width: clamp(180px, 36vw, 360px);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 700;
  font-size: 30px;
  line-height: 1.4;
  color: var(--text-primary);
}

.project-terminal__empty-title {
  max-width: clamp(180px, 36vw, 360px);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 600;
  font-size: var(--font-size-base);
  line-height: 1.4;
  color: var(--text-primary);
}

.project-terminal__actions {
  justify-self: center;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
}

.project-terminal__action-btn {
  justify-self: center;
  width: auto;
  min-width: 88px;
  max-width: 128px;
  padding-inline: 12px;
}

</style>
