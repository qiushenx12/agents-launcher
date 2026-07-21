import { computed, ref, watch } from 'vue'
import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { useClaudeStore } from './claude'
import { useCodexConfigStore } from './codexConfig'
import { useOpencodeConfigStore } from './opencodeConfig'
import { useTerminalStore } from './terminal'
import type { CliProfileRef, SessionEntry } from '@/types/config'
import { CLI_DESCRIPTORS, type CliKind } from '@/types/cli'
import { useCliRuntimeStore } from './cliRuntime'
import { getDefaultShell } from '@/composables/useDefaultShell'
import { usePlatform } from '@/composables/usePlatform'

export type TerminalStatus = 'off' | 'idle' | 'running'
export type SidebarTabType = 'tools' | 'file' | 'terminal' | 'browser'
export type SidebarPlacement = 'right' | 'top'
export type FileViewMode = 'source' | 'preview'
export type ProjectSortMode = 'manual' | 'time'

export interface Project {
  cliKind: CliKind
  id: string
  name: string
  path: string
  createdAt: number
  updatedAt: number
  order: number
  recentItems: RecentItem[]
}

export interface ProjectSession {
  cliKind: CliKind
  id: string
  projectId: string
  name: string
  claudeSessionId?: string
  nativeSessionId?: string
  launchMode?: 'new' | 'resume' | 'resume_picker'
  shell?: string[]
  cwd?: string
  env?: Record<string, string>
  profileRef?: CliProfileRef
  createdAt: number
  updatedAt: number
  order: number
}

export interface RecentItem {
  type: 'file' | 'browser' | 'terminal'
  name: string
  path?: string
  url?: string
  openedAt: number
}

export interface SidebarTab {
  cliKind: CliKind
  id: string
  type: SidebarTabType
  title: string
  path?: string
  url?: string
  browserHistory?: string[]
  browserHistoryIndex?: number
  browserRefreshKey?: number
  terminalId?: number
  content?: string
  dirty?: boolean
  viewMode?: FileViewMode
  language?: string
}

interface ProjectStoreFile {
  [key: string]: unknown
  projects: Project[]
  sessions: ProjectSession[]
  activeProjectId?: string | null
  activeSessionId?: string | null
  expandedProjectIds: string[]
  projectSortMode?: ProjectSortMode
  activeProjectIds?: Partial<Record<CliKind, string>>
  activeSessionIds?: Partial<Record<CliKind, string>>
}

interface OpenCodeProjectEntry {
  id: string
  worktree: string
}

interface OpenCodeProjectDiscovery {
  projects: OpenCodeProjectEntry[]
  warning?: string | null
}

interface OpenCodeSessionEntry {
  id: string
  title: string
  updated: number
  created: number
  projectId: string
  directory: string
}

interface CodexThreadEntry {
  id: string
  name?: string | null
  preview: string
  cwd: string
  createdAt: number
  updatedAt: number
}

interface CodexProjectEntry {
  worktree: string
  updatedAt: number
  sessionCount: number
}

interface CodexProjectDiscovery {
  projects: CodexProjectEntry[]
  warning?: string | null
}

const EMPTY_SELECTIONS: Record<CliKind, string | null> = {
  claude: null,
  codex: null,
  opencode: null,
}

function makeId(prefix: string) {
  const cryptoId = globalThis.crypto?.randomUUID?.()
  return cryptoId ? `${prefix}-${cryptoId}` : `${prefix}-${Date.now()}-${Math.random().toString(16).slice(2)}`
}

function now() {
  return Date.now()
}

function basename(path: string) {
  return path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path
}

function normalizeProjectPath(path: string) {
  return path.trim().replace(/[\\/]+$/, '')
}

function updateClaudeLaunchHistory(claudeStore: ReturnType<typeof useClaudeStore>, path: string) {
  // Update both the current launch dir and the in-memory history so the
  // config-side dropdown stays in sync immediately.
  claudeStore.launchDir = path
  claudeStore.launchDirHistory = [
    path,
    ...claudeStore.launchDirHistory.filter((d) => d.toLowerCase() !== path.toLowerCase()),
  ].slice(0, 20)
}

function extension(path: string) {
  const name = basename(path)
  const idx = name.lastIndexOf('.')
  return idx === -1 ? '' : name.slice(idx + 1).toLowerCase()
}

function languageFromPath(path: string) {
  const ext = extension(path)
  const map: Record<string, string> = {
    md: 'markdown',
    markdown: 'markdown',
    txt: 'text',
    log: 'text',
    env: 'env',
    js: 'javascript',
    ts: 'typescript',
    vue: 'vue',
    html: 'html',
    css: 'css',
    rs: 'rust',
    py: 'python',
    json: 'json',
    yaml: 'yaml',
    yml: 'yaml',
    toml: 'toml',
  }
  return map[ext] ?? ext
}

function defaultSessionName(existing: ProjectSession[]) {
  if (existing.length === 0) return '主终端'
  return `新会话 ${existing.length + 1}`
}

function historyTimestampToMs(ts: number) {
  if (!ts) return now()
  return ts > 1_000_000_000_000 ? ts : ts * 1000
}

function normalizeFsPath(path: string) {
  return path.trim().replace(/[\\/]+$/, '').replace(/\//g, '\\').toLowerCase()
}

function sessionDisplayName(entry: SessionEntry) {
  return entry.display.replace(/\n/g, ' ').replace(/\r/g, '').trim() || entry.id
}

function normalizeSessionName(name: string) {
  return name.replace(/\n/g, ' ').replace(/\r/g, '').trim()
}

function isDefaultProjectSessionName(name: string) {
  const normalized = normalizeSessionName(name)
  return normalized === '主终端' || /^新会话\s+\d+$/.test(normalized)
}

const HISTORY_BIND_GRACE_MS = 30_000

function projectSessionIdForClaude(projectId: string, claudeSessionId: string) {
  return `session-claude-${projectId}-${claudeSessionId}`.replace(/[^a-zA-Z0-9_-]/g, '_')
}

function browserTitle(url: string) {
  try {
    const parsed = new URL(url)
    return parsed.hostname || '浏览器'
  } catch {
    return basename(url.replace(/\/$/, '')) || '浏览器'
  }
}

export const useProjectStore = defineStore('project', () => {
  const activeCliKind = ref<CliKind>('claude')
  const projects = ref<Project[]>([])
  const sessions = ref<ProjectSession[]>([])
  const expandedProjectIds = ref<Set<string>>(new Set())
  const activeProjectId = ref<string | null>(null)
  const activeSessionId = ref<string | null>(null)
  const activeProjectIds = ref<Record<CliKind, string | null>>({ ...EMPTY_SELECTIONS })
  const activeSessionIds = ref<Record<CliKind, string | null>>({ ...EMPTY_SELECTIONS })
  const projectSortMode = ref<ProjectSortMode>('manual')
  const sidebarOpen = ref(false)
  const sidebarPlacement = ref<SidebarPlacement>('right')
  const leftSidebarCollapsed = ref(false)
  const sidebarTabs = ref<SidebarTab[]>([])
  const activeSidebarTabId = ref<string | null>(null)
  const statusMessage = ref('')

  // Auto-dismiss status messages so stale toasts never linger across tabs.
  let statusMessageTimer: ReturnType<typeof setTimeout> | null = null
  watch(statusMessage, (message) => {
    if (statusMessageTimer) {
      clearTimeout(statusMessageTimer)
      statusMessageTimer = null
    }
    if (!message) return
    statusMessageTimer = setTimeout(() => {
      statusMessageTimer = null
      statusMessage.value = ''
    }, 3000)
  })

  const sessionTerminalIds = ref<Record<string, number>>({})
  const persistedRootExtras = ref<Record<string, unknown>>({})
  let projectsLoaded = false
  let loadPromise: Promise<void> | null = null

  const visibleProjects = computed(() =>
    projects.value.filter((project) => project.cliKind === activeCliKind.value)
  )

  const visibleSessions = computed(() =>
    sessions.value.filter((session) => session.cliKind === activeCliKind.value)
  )

  const visibleSidebarTabs = computed(() =>
    sidebarTabs.value.filter((tab) => tab.cliKind === activeCliKind.value)
  )

  const activeProject = computed(() =>
    visibleProjects.value.find((p) => p.id === activeProjectId.value) ?? null
  )

  const sessionsOfActiveProject = computed(() =>
    visibleSessions.value
      .filter((s) => s.projectId === activeProjectId.value)
      .sort((a, b) => a.order - b.order)
  )

  const activeSession = computed(() =>
    visibleSessions.value.find((s) => s.id === activeSessionId.value && s.projectId === activeProjectId.value) ?? null
  )

  const activeSidebarTab = computed(() =>
    visibleSidebarTabs.value.find((tab) => tab.id === activeSidebarTabId.value) ?? null
  )

  const recentItemsOfActiveProject = computed(() =>
    [...(activeProject.value?.recentItems ?? [])].sort((a, b) => b.openedAt - a.openedAt)
  )

  async function persist() {
    rememberActiveSelection()
    const data: ProjectStoreFile = {
      ...persistedRootExtras.value,
      projects: projects.value,
      sessions: sessions.value,
      activeProjectId: activeProjectId.value,
      activeSessionId: activeSessionId.value,
      expandedProjectIds: [...expandedProjectIds.value],
      projectSortMode: projectSortMode.value,
      activeProjectIds: compactSelections(activeProjectIds.value),
      activeSessionIds: compactSelections(activeSessionIds.value),
    }
    await invoke('save_projects', { data })
  }

  function compactSelections(values: Record<CliKind, string | null>) {
    return Object.fromEntries(
      Object.entries(values).filter((entry): entry is [CliKind, string] => !!entry[1]),
    ) as Partial<Record<CliKind, string>>
  }

  function rememberActiveSelection() {
    activeProjectIds.value[activeCliKind.value] = activeProjectId.value
    activeSessionIds.value[activeCliKind.value] = activeSessionId.value
  }

  function setActiveCliKind(kind: CliKind) {
    if (activeCliKind.value === kind) {
      normalizeActiveState()
      return
    }
    rememberActiveSelection()
    statusMessage.value = ''
    activeCliKind.value = kind
    activeProjectId.value = activeProjectIds.value[kind]
    activeSessionId.value = activeSessionIds.value[kind]
    const firstSidebarTab = sidebarTabs.value.find((tab) => tab.cliKind === kind)
    activeSidebarTabId.value = firstSidebarTab?.id ?? null
    sidebarOpen.value = !!firstSidebarTab && sidebarOpen.value
    normalizeActiveState()
  }

  function normalizeActiveState() {
    if (activeProjectId.value && !visibleProjects.value.some((p) => p.id === activeProjectId.value)) {
      activeProjectId.value = null
    }
    if (!activeProjectId.value && visibleProjects.value.length > 0) {
      activeProjectId.value = [...visibleProjects.value].sort((a, b) => a.order - b.order)[0].id
    }
    if (activeProjectId.value) {
      expandedProjectIds.value.add(activeProjectId.value)
    }

    if (
      activeSessionId.value
      && !visibleSessions.value.some((s) => s.id === activeSessionId.value && s.projectId === activeProjectId.value)
    ) {
      activeSessionId.value = null
    }
    if (!activeSessionId.value) {
      activeSessionId.value = sessionsOfActiveProject.value[0]?.id ?? null
    }
    rememberActiveSelection()
  }

  function findProjectByPath(path: string, cliKind: CliKind = activeCliKind.value) {
    const normalizedPath = normalizeFsPath(path)
    return projects.value.find((project) =>
      project.cliKind === cliKind
      && normalizeFsPath(project.path) === normalizedPath
    ) ?? null
  }

  function mergeRecentProjectPaths(
    paths: string[],
    options: { prepend?: boolean; cliKind?: CliKind } = {},
  ) {
    const cliKind = options.cliKind ?? 'claude'
    const incoming: Project[] = []
    const existingForKind = projects.value.filter((project) => project.cliKind === cliKind)

    for (const path of paths) {
      if (!path) continue
      const normalized = normalizeProjectPath(path)
      if (!normalized) continue
      const normalizedKey = normalizeFsPath(normalized)

      if (
        existingForKind.some((project) => normalizeFsPath(project.path) === normalizedKey)
        || incoming.some((project) => normalizeFsPath(project.path) === normalizedKey)
      ) {
        continue
      }

      const ts = now()
      incoming.push({
        cliKind,
        id: makeId('project'),
        name: basename(normalized),
        path: normalized,
        createdAt: ts,
        updatedAt: ts,
        order: options.prepend ? incoming.length : existingForKind.length + incoming.length,
        recentItems: [],
      })
    }

    if (incoming.length === 0) return false

    if (options.prepend) {
      existingForKind.forEach((project) => {
        project.order += incoming.length
      })
      projects.value.push(...incoming)
    } else {
      projects.value.push(...incoming)
    }

    return true
  }

  function uniqueSessionId(preferredId: string) {
    if (!sessions.value.some((s) => s.id === preferredId)) return preferredId
    return makeId('session')
  }

  function updateSessionTerminalTitle(sessionId: string, title: string) {
    const tabId = sessionTerminalIds.value[sessionId]
    if (tabId) useTerminalStore().updateTabTitle(tabId, title)
  }

  function sessionHasLiveTerminal(sessionId: string) {
    const tabId = sessionTerminalIds.value[sessionId]
    if (!tabId) return false
    const tab = useTerminalStore().tabs.find((item) => item.id === tabId)
    return !!tab?.alive
  }

  function canBindHistoryEntryToLocalSession(session: ProjectSession, timestamp: number) {
    return !session.claudeSessionId
      && sessionHasLiveTerminal(session.id)
      && timestamp + HISTORY_BIND_GRACE_MS >= session.createdAt
  }

  function findLocalSessionForNewHistoryEntry(projectSessions: ProjectSession[], timestamp: number) {
    const candidates = projectSessions
      .filter((session) => canBindHistoryEntryToLocalSession(session, timestamp))
      .sort((a, b) => b.createdAt - a.createdAt)

    return candidates.find((session) => session.id === activeSessionId.value) ?? candidates[0] ?? null
  }

  function removeUnopenedDuplicateSession(projectSessions: ProjectSession[], duplicate: ProjectSession, replacement: ProjectSession) {
    if (duplicate.id === replacement.id) return false
    if (sessionTerminalIds.value[duplicate.id]) return false

    sessions.value = sessions.value.filter((session) => session.id !== duplicate.id)
    const index = projectSessions.findIndex((session) => session.id === duplicate.id)
    if (index !== -1) projectSessions.splice(index, 1)
    if (activeSessionId.value === duplicate.id) {
      activeSessionId.value = replacement.id
    }
    return true
  }

  function pruneClaudeSessionsForProject(projectId: string) {
    sessions.value = sessions.value.filter((session) => {
      if (session.projectId !== projectId) return true
      if (!session.claudeSessionId) return true
      return !!sessionTerminalIds.value[session.id]
    })
  }

  async function syncProjectSessionsFromClaude(projectId: string) {
    const project = projects.value.find((p) => p.id === projectId && p.cliKind === 'claude')
    if (!project) return false

    let recent: SessionEntry[] = []
    try {
      recent = await invoke<SessionEntry[]>('load_claude_sessions', {
        targetDir: project.path,
      }) ?? []
    } catch {
      return false
    }

    if (recent.length === 0) return false

    let changed = false
    const projectSessions = sessions.value.filter((s) => s.projectId === projectId && s.cliKind === 'claude')
    const reusableLocalSessions = projectSessions.filter(
      (s) => !s.claudeSessionId && !sessionTerminalIds.value[s.id],
    )
    const usedReusableIds = new Set<string>()

    for (const [index, entry] of recent.entries()) {
      const displayName = sessionDisplayName(entry)
      const timestamp = historyTimestampToMs(entry.ts)
      const historySession = projectSessions.find((s) => s.claudeSessionId === entry.id)
      let session = historySession

      if (index === 0) {
        const localSession = findLocalSessionForNewHistoryEntry(projectSessions, timestamp)
        if (localSession && (!historySession || !sessionHasLiveTerminal(historySession.id))) {
          if (historySession && removeUnopenedDuplicateSession(projectSessions, historySession, localSession)) {
            changed = true
          }
          session = localSession
        }
      }

      if (!session) {
        session = reusableLocalSessions.find(
          (s) => !usedReusableIds.has(s.id) && normalizeSessionName(s.name) === displayName,
        )
      }

      if (!session && index === 0) {
        session = reusableLocalSessions.find(
          (s) => !usedReusableIds.has(s.id) && isDefaultProjectSessionName(s.name),
        )
      }

      if (!session) {
        session = {
          cliKind: 'claude',
          id: uniqueSessionId(projectSessionIdForClaude(projectId, entry.id)),
          projectId,
          name: displayName,
          claudeSessionId: entry.id,
          nativeSessionId: entry.id,
          createdAt: timestamp,
          updatedAt: timestamp,
          order: index,
        }
        sessions.value.push(session)
        projectSessions.push(session)
        changed = true
      } else {
        usedReusableIds.add(session.id)
        if (session.name !== displayName) {
          session.name = displayName
          updateSessionTerminalTitle(session.id, displayName)
          changed = true
        }
        if (session.claudeSessionId !== entry.id) {
          session.claudeSessionId = entry.id
          changed = true
        }
        if (session.nativeSessionId !== entry.id) {
          session.nativeSessionId = entry.id
          changed = true
        }
        if (session.updatedAt !== timestamp) {
          session.updatedAt = timestamp
          changed = true
        }
        if (session.order !== index) {
          session.order = index
          changed = true
        }
      }
    }

    const recentIds = new Set(recent.map((entry) => entry.id))
    const trailingSessions = sessions.value
      .filter((s) =>
        s.cliKind === 'claude'
        && s.projectId === projectId
        && !recentIds.has(s.nativeSessionId ?? s.claudeSessionId ?? '')
      )
      .sort((a, b) => a.order - b.order)

    trailingSessions.forEach((session, index) => {
      const nextOrder = recent.length + index
      if (session.order !== nextOrder) {
        session.order = nextOrder
        changed = true
      }
    })

    return changed
  }

  function ensureProjectHasSession(projectId: string) {
    const project = projects.value.find((item) => item.id === projectId)
    if (!project) return
    if (sessions.value.some((s) => s.projectId === projectId && s.cliKind === project.cliKind)) return
    createSessionRecord(projectId, '主终端', { activate: false })
  }

  async function syncProjectHistory(project: Project) {
    if (project.cliKind === 'claude') return syncProjectSessionsFromClaude(project.id)
    if (project.cliKind === 'codex') return syncProjectSessionsFromCodex(project.id)
    if (project.cliKind === 'opencode') return syncProjectSessionsFromOpenCode(project.id)
    return false
  }

  async function syncProjectSessionsFromCodex(projectId: string) {
    const project = projects.value.find((item) => item.id === projectId && item.cliKind === 'codex')
    if (!project) return false

    let recent: CodexThreadEntry[]
    try {
      recent = await invoke<CodexThreadEntry[]>('list_codex_threads', {
        projectPath: project.path,
        maxCount: 200,
      })
    } catch (error) {
      statusMessage.value = `CodeX 真实会话读取失败，可继续使用新会话或原生恢复：${String(error)}`
      return false
    }
    return applyCodexThreadEntries(projectId, recent)
  }

  function applyCodexThreadEntries(projectId: string, recent: CodexThreadEntry[]) {
    let changed = false
    const current = sessions.value.filter(
      (session) => session.cliKind === 'codex' && session.projectId === projectId,
    )
    const recentIds = new Set(recent.map((entry) => entry.id))
    for (const [index, entry] of recent.entries()) {
      const createdAt = historyTimestampToMs(entry.createdAt)
      const updatedAt = historyTimestampToMs(entry.updatedAt)
      const nextName = normalizeSessionName(entry.name || entry.preview).slice(0, 120)
        || `CodeX 会话 ${entry.id.slice(0, 8)}`
      let session = current.find((item) => item.nativeSessionId === entry.id)
      if (!session) {
        session = {
          cliKind: 'codex',
          id: uniqueSessionId(`session-codex-${entry.id}`),
          projectId,
          name: nextName,
          nativeSessionId: entry.id,
          launchMode: 'resume',
          createdAt,
          updatedAt,
          order: index,
        }
        sessions.value.push(session)
        current.push(session)
        changed = true
      } else {
        if (session.name !== nextName) {
          session.name = nextName
          updateSessionTerminalTitle(session.id, nextName)
          changed = true
        }
        if (session.updatedAt !== updatedAt || session.createdAt !== createdAt || session.order !== index) {
          session.createdAt = createdAt
          session.updatedAt = updatedAt
          session.order = index
          changed = true
        }
        if (session.launchMode !== 'resume') {
          session.launchMode = 'resume'
          changed = true
        }
      }
    }

    for (const session of current) {
      if (!session.nativeSessionId || recentIds.has(session.nativeSessionId)) continue
      if (sessionHasLiveTerminal(session.id)) continue
      sessions.value = sessions.value.filter((item) => item.id !== session.id)
      changed = true
    }

    const localSessions = sessions.value
      .filter((session) =>
        session.cliKind === 'codex'
        && session.projectId === projectId
        && !session.nativeSessionId
      )
      .sort((a, b) => a.order - b.order)
    localSessions.forEach((session, index) => {
      const nextOrder = recent.length + index
      if (session.order !== nextOrder) {
        session.order = nextOrder
        changed = true
      }
    })
    return changed
  }

  async function discoverCodexProjects() {
    let discovery: CodexProjectDiscovery
    try {
      discovery = await invoke<CodexProjectDiscovery>('discover_codex_projects')
    } catch (error) {
      statusMessage.value = `CodeX 项目发现不可用，可手动选择目录：${String(error)}`
      return false
    }
    if (discovery.warning) {
      statusMessage.value = `${discovery.warning}；其余项目仍可使用。`
    }
    const paths = discovery.projects
      .map((entry) => entry.worktree)
      .filter((path) => !!path)
    return mergeRecentProjectPaths(paths, { cliKind: 'codex' })
  }

  async function syncProjectSessionsFromOpenCode(projectId: string) {
    const project = projects.value.find((item) => item.id === projectId && item.cliKind === 'opencode')
    if (!project) return false

    let recent: OpenCodeSessionEntry[]
    try {
      recent = await invoke<OpenCodeSessionEntry[]>('list_opencode_sessions', {
        projectPath: project.path,
        maxCount: 200,
      })
    } catch (error) {
      statusMessage.value = `OpenCode 历史会话不可用：${String(error)}`
      return false
    }
    return applyOpenCodeSessionEntries(projectId, recent)
  }

  function applyOpenCodeSessionEntries(projectId: string, recent: OpenCodeSessionEntry[]) {
    let changed = false
    const current = sessions.value.filter(
      (session) => session.cliKind === 'opencode' && session.projectId === projectId,
    )
    const recentIds = new Set(recent.map((entry) => entry.id))
    const bindCandidates = current
      .filter((session) => !session.nativeSessionId && sessionHasLiveTerminal(session.id))
      .sort((a, b) => b.createdAt - a.createdAt)
    const boundLocalIds = new Set<string>()
    for (const [index, entry] of recent.entries()) {
      let session = current.find((item) => item.nativeSessionId === entry.id)
      let freshlyBound = false
      if (!session) {
        const candidates = bindCandidates.filter(
          (item) => !boundLocalIds.has(item.id) && entry.created + HISTORY_BIND_GRACE_MS >= item.createdAt,
        )
        const candidate = candidates.find((item) => item.id === activeSessionId.value) ?? candidates[0] ?? null
        if (candidate) {
          session = candidate
          boundLocalIds.add(candidate.id)
          session.nativeSessionId = entry.id
          session.launchMode = 'resume'
          freshlyBound = true
          changed = true
        }
      }
      if (!session) {
        session = {
          cliKind: 'opencode',
          id: uniqueSessionId(`session-opencode-${entry.id}`),
          projectId,
          name: normalizeSessionName(entry.title) || entry.id,
          nativeSessionId: entry.id,
          launchMode: 'resume',
          createdAt: entry.created,
          updatedAt: entry.updated,
          order: index,
        }
        sessions.value.push(session)
        current.push(session)
        changed = true
      } else {
        const nextName = normalizeSessionName(entry.title) || entry.id
        const renamable = !freshlyBound || isDefaultProjectSessionName(session.name)
        if (renamable && session.name !== nextName) {
          session.name = nextName
          updateSessionTerminalTitle(session.id, nextName)
          changed = true
        }
        if (session.updatedAt !== entry.updated || session.order !== index) {
          session.updatedAt = entry.updated
          session.order = index
          changed = true
        }
      }
    }

    for (const session of current) {
      if (!session.nativeSessionId || recentIds.has(session.nativeSessionId)) continue
      if (sessionTerminalIds.value[session.id]) continue
      sessions.value = sessions.value.filter((item) => item.id !== session.id)
      changed = true
    }
    return changed
  }

  // Re-sync OpenCode session names shortly after naming completes, instead of
  // only on workspace re-entry. Polls lightly: the CLI is only invoked when a
  // session is still unnamed (title pending) or terminal output has settled
  // since the last sync.
  let lastOpenCodeNameSync = 0

  async function pollOpenCodeSessionNames() {
    if (activeCliKind.value !== 'opencode') return
    const terminalStore = useTerminalStore()
    const now = Date.now()
    const projectIds = new Set<string>()
    for (const session of sessions.value) {
      if (session.cliKind !== 'opencode') continue
      const tabId = sessionTerminalIds.value[session.id]
      if (!tabId) continue
      if (
        session.nativeSessionId
        && (session.name === session.nativeSessionId || isDefaultProjectSessionName(session.name))
      ) {
        projectIds.add(session.projectId)
        continue
      }
      const lastOutput = terminalStore.outputActivity[tabId] ?? 0
      if (lastOutput > lastOpenCodeNameSync && now - lastOutput > 2000) {
        projectIds.add(session.projectId)
      }
    }
    if (projectIds.size === 0) return
    lastOpenCodeNameSync = now
    let changed = false
    for (const projectId of projectIds) {
      if (await syncProjectSessionsFromOpenCode(projectId)) changed = true
      historySyncedAt[`opencode:${projectId}`] = Date.now()
    }
    if (changed) await persist()
  }

  const opencodeNamePollTimer = setInterval(() => {
    pollOpenCodeSessionNames().catch(() => {})
  }, 4000)
  void opencodeNamePollTimer

  async function discoverOpenCodeProjects() {
    let discovery: OpenCodeProjectDiscovery
    try {
      discovery = await invoke<OpenCodeProjectDiscovery>('discover_opencode_projects')
    } catch (error) {
      statusMessage.value = `OpenCode 项目发现不可用：${String(error)}`
      return false
    }
    if (discovery.warning) {
      statusMessage.value = `${discovery.warning}；其余项目仍可使用。`
    }
    const paths = discovery.projects
      .map((entry) => entry.worktree)
      .filter((path) => path && path !== '/' && (path.startsWith('/') || /^[a-zA-Z]:[\\/]/.test(path)))
    return mergeRecentProjectPaths(paths, { prepend: true, cliKind: 'opencode' })
  }

  // Earlier builds keyed project dedupe on exact path strings, so the same
  // directory could be persisted twice with different separator styles
  // (e.g. "D:/work/demo" from OpenCode discovery vs "D:\work\demo").
  // Merge those duplicates once on load, keeping the first record.
  function dedupeDuplicateProjects() {
    const keptByKey = new Map<string, Project>()
    const redirected = new Map<string, Project>()
    for (const project of projects.value) {
      const key = `${project.cliKind}:${normalizeFsPath(project.path)}`
      const kept = keptByKey.get(key)
      if (!kept) {
        keptByKey.set(key, project)
        continue
      }
      redirected.set(project.id, kept)
      const existingItems = new Set(
        kept.recentItems.map((item) => `${item.type}:${item.path ?? item.url ?? item.name}`),
      )
      for (const item of project.recentItems) {
        const itemKey = `${item.type}:${item.path ?? item.url ?? item.name}`
        if (existingItems.has(itemKey)) continue
        existingItems.add(itemKey)
        kept.recentItems.push(item)
      }
      kept.createdAt = Math.min(kept.createdAt, project.createdAt)
      kept.updatedAt = Math.max(kept.updatedAt, project.updatedAt)
    }
    if (redirected.size === 0) return false

    projects.value = projects.value.filter((project) => !redirected.has(project.id))
    for (const session of sessions.value) {
      const kept = redirected.get(session.projectId)
      if (kept) session.projectId = kept.id
    }
    for (const kind of Object.keys(activeProjectIds.value) as CliKind[]) {
      const activeId = activeProjectIds.value[kind]
      const kept = activeId ? redirected.get(activeId) : undefined
      if (kept) activeProjectIds.value[kind] = kept.id
    }
    const activeRedirect = activeProjectId.value ? redirected.get(activeProjectId.value) : undefined
    if (activeRedirect) activeProjectId.value = activeRedirect.id
    expandedProjectIds.value = new Set(
      [...expandedProjectIds.value].map((id) => redirected.get(id)?.id ?? id),
    )
    return true
  }

  async function loadProjects() {
    if (projectsLoaded) {
      normalizeActiveState()
      return
    }
    if (loadPromise) return loadPromise
    loadPromise = (async () => {
      try {
      // Load the config-side launch directory history first so we can seed the
      // project list if it is empty.
      const claudeStore = useClaudeStore()
      await claudeStore.loadRecentProjects()

      const data = await invoke<ProjectStoreFile>('load_projects')
      const knownRootKeys = new Set([
        'projects', 'sessions', 'activeProjectId', 'activeSessionId',
        'expandedProjectIds', 'projectSortMode', 'activeProjectIds', 'activeSessionIds',
      ])
      persistedRootExtras.value = Object.fromEntries(
        Object.entries(data).filter(([key]) => !knownRootKeys.has(key)),
      )
      projects.value = (data.projects ?? []).map((project) => ({
        ...project,
        cliKind: project.cliKind ?? 'claude',
      }))
      sessions.value = (data.sessions ?? []).map((session) => ({
        ...session,
        cliKind: session.cliKind ?? 'claude',
        nativeSessionId: session.nativeSessionId ?? session.claudeSessionId,
      }))
      expandedProjectIds.value = new Set(data.expandedProjectIds ?? [])
      activeProjectIds.value = {
        ...EMPTY_SELECTIONS,
        ...(data.activeProjectIds ?? {}),
        claude: data.activeProjectIds?.claude ?? data.activeProjectId ?? null,
      }
      activeSessionIds.value = {
        ...EMPTY_SELECTIONS,
        ...(data.activeSessionIds ?? {}),
        claude: data.activeSessionIds?.claude ?? data.activeSessionId ?? null,
      }
      activeProjectId.value = activeProjectIds.value[activeCliKind.value]
      activeSessionId.value = activeSessionIds.value[activeCliKind.value]
      projectSortMode.value = data.projectSortMode === 'time' ? 'time' : 'manual'

      dedupeDuplicateProjects()

      // Merge recent directories from the config-side launch directory history.
      // This keeps the project list in sync with Claude's own history even when
      // no projects have been explicitly added in this module. Most recent
      // directories appear first on initial load.
      mergeRecentProjectPaths(claudeStore.launchDirHistory, { prepend: true, cliKind: 'claude' })

      for (const project of projects.value.filter((item) => item.cliKind === 'claude')) {
        await syncProjectSessionsFromClaude(project.id)
        ensureProjectHasSession(project.id)
      }

      normalizeActiveState()
      await persist()
      projectsLoaded = true
      } catch (e) {
        statusMessage.value = `加载项目失败：${e}`
      } finally {
        loadPromise = null
      }
    })()
    return loadPromise
  }

  // Per-project history sync runs a CLI child process, so switching tabs used
  // to cost one spawn per project, sequentially. Syncs now run concurrently
  // and results are considered fresh for HISTORY_SYNC_TTL; manual refresh and
  // targeted name polling bypass the TTL.
  const HISTORY_SYNC_TTL = 60_000
  const HISTORY_SYNC_CONCURRENCY = 4
  const historySyncedAt: Record<string, number> = {}
  const discoverySyncedAt: Partial<Record<CliKind, number>> = {}

  async function discoverCliProjects(kind: 'codex' | 'opencode', force: boolean) {
    const now = Date.now()
    if (!force && now - (discoverySyncedAt[kind] ?? 0) <= HISTORY_SYNC_TTL) return false
    const changed = kind === 'codex'
      ? await discoverCodexProjects()
      : await discoverOpenCodeProjects()
    discoverySyncedAt[kind] = now
    return changed
  }

  async function syncCliSessions(
    kind: CliKind,
    syncOne: (projectId: string) => Promise<boolean>,
    options?: { force?: boolean; projectIds?: string[] },
  ) {
    let changed = false
    const now = Date.now()
    const targets = projects.value.filter((item) => item.cliKind === kind
      && (!options?.projectIds || options.projectIds.includes(item.id)))
    const queue = options?.force
      ? [...targets]
      : targets.filter((item) => now - (historySyncedAt[`${kind}:${item.id}`] ?? 0) > HISTORY_SYNC_TTL)
    const workers = Array.from(
      { length: Math.min(HISTORY_SYNC_CONCURRENCY, queue.length) },
      async () => {
        for (let project = queue.shift(); project; project = queue.shift()) {
          if (await syncOne(project.id)) changed = true
          historySyncedAt[`${kind}:${project.id}`] = Date.now()
          const count = sessions.value.length
          ensureProjectHasSession(project.id)
          if (sessions.value.length !== count) changed = true
        }
      },
    )
    await Promise.all(workers)
    normalizeActiveState()
    if (changed) await persist()
    return changed
  }

  // Batched history sync fetches every native session in one backend call
  // (single app-server handshake for CodeX, one local DB read for OpenCode)
  // and distributes entries to projects by path. Falls back to per-project
  // syncing when the batched command is unavailable.
  const batchSyncedAt: Partial<Record<CliKind, number>> = {}

  async function syncCliSessionsBatched(kind: 'codex' | 'opencode', options?: { force?: boolean }) {
    const now = Date.now()
    if (!options?.force && now - (batchSyncedAt[kind] ?? 0) <= HISTORY_SYNC_TTL) return false

    let grouped: Map<string, (CodexThreadEntry | OpenCodeSessionEntry)[]>
    try {
      const all = kind === 'codex'
        ? await invoke<CodexThreadEntry[]>('list_all_codex_threads', {
          maxCount: 500,
          force: options?.force ?? false,
        })
        : await invoke<OpenCodeSessionEntry[]>('list_all_opencode_sessions', { maxCount: 500 })
      grouped = new Map()
      for (const entry of all) {
        const path = kind === 'codex'
          ? (entry as CodexThreadEntry).cwd
          : (entry as OpenCodeSessionEntry).directory
        const key = normalizeFsPath(path)
        if (!key) continue
        const bucket = grouped.get(key)
        if (bucket) bucket.push(entry)
        else grouped.set(key, [entry])
      }
    } catch {
      return kind === 'codex'
        ? syncCliSessions('codex', syncProjectSessionsFromCodex, { force: options?.force })
        : syncCliSessions('opencode', syncProjectSessionsFromOpenCode, { force: options?.force })
    }

    let changed = false
    const targets = projects.value.filter((item) => item.cliKind === kind)
    for (const project of targets) {
      const entries = grouped.get(normalizeFsPath(project.path)) ?? []
      const applied = kind === 'codex'
        ? applyCodexThreadEntries(project.id, entries as CodexThreadEntry[])
        : applyOpenCodeSessionEntries(project.id, entries as OpenCodeSessionEntry[])
      if (applied) changed = true
      historySyncedAt[`${kind}:${project.id}`] = now
      const count = sessions.value.length
      ensureProjectHasSession(project.id)
      if (sessions.value.length !== count) changed = true
    }
    batchSyncedAt[kind] = now
    normalizeActiveState()
    if (changed) await persist()
    return changed
  }

  async function prepareCliWorkspace(kind: CliKind) {
    if (activeCliKind.value !== kind) setActiveCliKind(kind)
    await loadProjects()
    // A slower CLI probe/load must not switch the workspace back after the
    // user has already selected another CLI entry.
    if (activeCliKind.value !== kind) return
    if (kind === 'codex') {
      await discoverCliProjects('codex', false)
      await syncCliSessionsBatched('codex')
      return
    }
    if (kind === 'opencode') {
      await discoverCliProjects('opencode', false)
      await syncCliSessionsBatched('opencode')
    }
  }

  async function refreshActiveCliHistory() {
    if (activeCliKind.value === 'claude') return refreshClaudeHistory()
    if (activeCliKind.value === 'codex') {
      await discoverCliProjects('codex', true)
      return syncCliSessionsBatched('codex', { force: true })
    }
    if (activeCliKind.value === 'opencode') {
      await discoverCliProjects('opencode', true)
      return syncCliSessionsBatched('opencode', { force: true })
    }
    return false
  }

  async function refreshClaudeHistory() {
    const claudeStore = useClaudeStore()
    let changed = mergeRecentProjectPaths(claudeStore.launchDirHistory, { cliKind: 'claude' })

    for (const project of projects.value.filter((item) => item.cliKind === 'claude')) {
      if (await syncProjectSessionsFromClaude(project.id)) {
        changed = true
      }

      const sessionCount = sessions.value.length
      ensureProjectHasSession(project.id)
      if (sessions.value.length !== sessionCount) {
        changed = true
      }
    }

    const launchProject = claudeStore.launchDir
      ? findProjectByPath(claudeStore.launchDir, 'claude')
      : null
    if (launchProject && await syncProjectSessionsFromClaude(launchProject.id)) {
      changed = true
    }

    if (changed) {
      normalizeActiveState()
      await persist()
    }

    return changed
  }

  async function pickAndAddProject() {
    const selected = await open({
      directory: true,
      multiple: false,
      title: `选择 ${CLI_DESCRIPTORS[activeCliKind.value].label} 项目目录`,
    })
    if (typeof selected === 'string') {
      await addProject(selected)
    }
  }

  async function addProject(path: string, name?: string) {
    const normalizedPath = normalizeProjectPath(path)
    const normalizedKey = normalizeFsPath(normalizedPath)
    const existing = projects.value.find(
      (p) => p.cliKind === activeCliKind.value
        && normalizeFsPath(p.path) === normalizedKey,
    )
    if (existing) {
      activeProjectId.value = existing.id
      expandedProjectIds.value.add(existing.id)
      await syncProjectHistory(existing)
      normalizeActiveState()
      await persist()
      return existing
    }

    const ts = now()
    const project: Project = {
      cliKind: activeCliKind.value,
      id: makeId('project'),
      name: name?.trim() || basename(normalizedPath),
      path: normalizedPath,
      createdAt: ts,
      updatedAt: ts,
      order: visibleProjects.value.length,
      recentItems: [],
    }
    projects.value.push(project)
    activeProjectId.value = project.id
    expandedProjectIds.value.add(project.id)

    if (project.cliKind === 'claude') {
      const claudeStore = useClaudeStore()
      updateClaudeLaunchHistory(claudeStore, project.path)
      await claudeStore.saveLaunchDir().catch(() => {})
    }

    await syncProjectHistory(project)
    ensureProjectHasSession(project.id)
    normalizeActiveState()

    await persist()
    return project
  }

  async function launchClaudeFromConfig(
    claudeSessionId?: string,
  ) {
    await prepareCliWorkspace('claude')
    const claudeStore = useClaudeStore()
    const launchPath = normalizeProjectPath(claudeStore.launchDir)

    if (!launchPath) {
      claudeStore.statusMessage = '启动目录为空，请先设置启动目录'
      return null
    }

    const project = await addProject(launchPath)
    await syncProjectSessionsFromClaude(project.id)

    let session: ProjectSession | undefined
    if (claudeSessionId) {
      const entry = claudeStore.sessions.find((item) => item.id === claudeSessionId)
      session = sessions.value.find(
        (item) => item.projectId === project.id && item.claudeSessionId === claudeSessionId,
      )

      const timestamp = entry ? historyTimestampToMs(entry.ts) : now()
      const displayName = entry ? sessionDisplayName(entry) : '恢复会话'
      if (!session) {
        session = createSessionRecord(project.id, displayName, {
          id: uniqueSessionId(projectSessionIdForClaude(project.id, claudeSessionId)),
          claudeSessionId,
          createdAt: timestamp,
          updatedAt: now(),
        })
      } else {
        session.name = displayName
        session.updatedAt = now()
        updateSessionTerminalTitle(session.id, displayName)
      }
    } else {
      session = sessions.value.find(
        (item) =>
          item.projectId === project.id
          && !item.claudeSessionId
          && !sessionTerminalIds.value[item.id]
          && isDefaultProjectSessionName(item.name),
      )

      if (session) {
        session.updatedAt = now()
      } else {
        session = createSessionRecord(project.id)
      }
    }

    activeProjectId.value = project.id
    expandedProjectIds.value.add(project.id)
    leftSidebarCollapsed.value = false
    activeSessionId.value = session.id
    // Claude Code always uses the configuration that is active when the PTY
    // starts. A project session is history, not a snapshot of an old provider.
    session.profileRef = undefined
    await persist()
    await ensureSessionTerminal(session.id)
    return session
  }

  async function removeProject(projectId: string) {
    const terminalStore = useTerminalStore()
    const relatedSessionIds = sessions.value.filter((s) => s.projectId === projectId).map((s) => s.id)
    for (const id of relatedSessionIds) {
      const tabId = sessionTerminalIds.value[id]
      if (tabId) await terminalStore.closeTab(tabId)
      delete sessionTerminalIds.value[id]
    }
    projects.value = projects.value.filter((p) => p.id !== projectId)
    sessions.value = sessions.value.filter((s) => s.projectId !== projectId)
    expandedProjectIds.value.delete(projectId)
    if (activeProjectId.value === projectId) {
      activeProjectId.value = visibleProjects.value.find((project) => project.id !== projectId)?.id ?? null
    }
    activeSessionId.value = sessionsOfActiveProject.value[0]?.id ?? null
    await persist()
  }

  async function renameProject(projectId: string, name: string) {
    const trimmed = name.trim()
    if (!trimmed) return
    const project = projects.value.find((p) => p.id === projectId)
    if (!project) return
    project.name = trimmed
    project.updatedAt = now()
    await persist()
  }

  async function replaceProjectPath(projectId: string, path: string, name?: string) {
    const project = projects.value.find((p) => p.id === projectId)
    if (!project) return null
    project.path = path
    project.name = name?.trim() || basename(path)
    project.updatedAt = now()
    activeProjectId.value = project.id
    expandedProjectIds.value.add(project.id)
    if (project.cliKind === 'claude') pruneClaudeSessionsForProject(project.id)
    await syncProjectHistory(project)
    ensureProjectHasSession(project.id)
    normalizeActiveState()
    await persist()
    return project
  }

  async function toggleProjectExpanded(projectId: string) {
    if (expandedProjectIds.value.has(projectId)) {
      expandedProjectIds.value.delete(projectId)
    } else {
      expandedProjectIds.value.add(projectId)
    }
    await persist()
  }

  function toggleLeftSidebarCollapsed() {
    leftSidebarCollapsed.value = !leftSidebarCollapsed.value
  }

  async function setProjectSortMode(mode: ProjectSortMode) {
    projectSortMode.value = mode
    await persist()
  }

  async function reorderProjects(projectIds: string[]) {
    if (projectIds.length === 0) return
    const orderById = new Map(projectIds.map((id, index) => [id, index]))
    projects.value.forEach((project) => {
      const order = orderById.get(project.id)
      if (order !== undefined) project.order = order
    })
    projectSortMode.value = 'manual'
    await persist()
  }

  async function activateProject(projectId: string) {
    activeProjectId.value = projectId
    expandedProjectIds.value.add(projectId)

    const project = projects.value.find((p) => p.id === projectId)
    if (project) {
      if (project.cliKind === 'claude') {
        const claudeStore = useClaudeStore()
        updateClaudeLaunchHistory(claudeStore, project.path)
        await claudeStore.saveLaunchDir().catch(() => {})
        await claudeStore.loadSessions().catch(() => {})
      }
      await syncProjectHistory(project)
    }

    normalizeActiveState()
    await persist()
  }

  function createSessionRecord(
    projectId: string,
    name?: string,
    options: {
      id?: string
      claudeSessionId?: string
      nativeSessionId?: string
      launchMode?: ProjectSession['launchMode']
      activate?: boolean
      createdAt?: number
      updatedAt?: number
      order?: number
      profileRef?: CliProfileRef
    } = {},
  ) {
    const projectSessions = sessions.value.filter((s) => s.projectId === projectId)
    const project = projects.value.find((item) => item.id === projectId)
    if (!project) throw new Error(`项目不存在: ${projectId}`)
    const ts = options.createdAt ?? now()
    const session: ProjectSession = {
      cliKind: project.cliKind,
      id: options.id ?? makeId('session'),
      projectId,
      name: name?.trim() || defaultSessionName(projectSessions),
      claudeSessionId: options.claudeSessionId,
      nativeSessionId: options.nativeSessionId ?? options.claudeSessionId,
      launchMode: options.launchMode ?? (options.nativeSessionId || options.claudeSessionId ? 'resume' : 'new'),
      profileRef: options.profileRef,
      createdAt: ts,
      updatedAt: options.updatedAt ?? ts,
      order: options.order ?? projectSessions.length,
    }
    sessions.value.push(session)
    if (options.activate !== false) {
      activeProjectId.value = projectId
      expandedProjectIds.value.add(projectId)
      activeSessionId.value = session.id
    }
    return session
  }

  async function createSession(projectId = activeProjectId.value, name?: string) {
    if (!projectId) return null
    const project = projects.value.find(
      (item) => item.id === projectId && item.cliKind === activeCliKind.value,
    )
    if (!project) return null

    const session = createSessionRecord(projectId, name)
    await persist()
    await ensureSessionTerminal(session.id)
    return session
  }

  async function resumeCodexSession(projectId = activeProjectId.value) {
    if (!projectId) return null
    const project = projects.value.find(
      (item) => item.id === projectId && item.cliKind === 'codex',
    )
    if (!project) return null
    const session = createSessionRecord(projectId, 'CodeX 原生恢复', {
      launchMode: 'resume_picker',
    })
    await persist()
    await ensureSessionTerminal(session.id)
    return session
  }

  async function pickAndResumeCodexSession() {
    if (activeCliKind.value !== 'codex') return null
    const selected = await open({
      directory: true,
      multiple: false,
      title: '选择 CodeX 项目目录并打开原生恢复',
    })
    if (typeof selected !== 'string') return null
    const project = await addProject(selected)
    return resumeCodexSession(project.id)
  }

  async function activateSession(sessionId: string) {
    const session = sessions.value.find(
      (item) => item.id === sessionId && item.cliKind === activeCliKind.value,
    )
    if (!session) return
    activeProjectId.value = session.projectId
    expandedProjectIds.value.add(session.projectId)
    activeSessionId.value = session.id
    await persist()
    if (getSessionStatus(session.id) === 'off') {
      await ensureSessionTerminal(session.id)
    }
  }

  async function renameSession(sessionId: string, name: string) {
    const trimmed = name.trim()
    if (!trimmed) return
    const session = sessions.value.find((s) => s.id === sessionId)
    if (!session) return
    session.name = trimmed
    session.updatedAt = now()
    updateSessionTerminalTitle(sessionId, trimmed)
    await persist()
  }

  async function removeSession(sessionId: string) {
    await closeSessionTerminal(sessionId)
    sessions.value = sessions.value.filter((s) => s.id !== sessionId)
    delete sessionTerminalIds.value[sessionId]
    if (activeSessionId.value === sessionId) {
      activeSessionId.value = sessionsOfActiveProject.value[0]?.id ?? null
    }
    await persist()
  }

  function getSessionStatus(sessionId: string): TerminalStatus {
    const terminalStore = useTerminalStore()
    const tabId = sessionTerminalIds.value[sessionId]
    if (!tabId) return 'off'
    const tab = terminalStore.tabs.find((t) => t.id === tabId)
    if (!tab || !tab.alive) return 'off'
    return tab.active ? 'running' : 'idle'
  }

  async function ensureSessionTerminal(sessionId: string) {
    const session = sessions.value.find((s) => s.id === sessionId)
    if (!session) return null
    const project = projects.value.find((p) => p.id === session.projectId)
    if (!project) return null
    if (project.cliKind !== session.cliKind) {
      statusMessage.value = '项目与会话的 CLI 类型不一致，已阻止启动。'
      return null
    }

    const terminalStore = useTerminalStore()
    const existingId = sessionTerminalIds.value[sessionId]
    const existing = terminalStore.tabs.find((t) => t.id === existingId)
    if (existing && existing.alive) return existing.id

    const runtimeStore = useCliRuntimeStore()
    const runtimeStatus = await runtimeStore.check(session.cliKind)
    if (runtimeStatus.state !== 'ready') {
      statusMessage.value = runtimeStatus.message
      return null
    }
    const envVars: Record<string, string> = {}
    let cmd: string[]
    if (session.cliKind === 'claude') {
      const claudeStore = useClaudeStore()
      const profileVars = claudeStore.editingConfig.vars
      for (const [key, value] of Object.entries(profileVars)) {
        if (value) envVars[key] = value
      }
      const args: string[] = []
      if (claudeStore.skipPermissions) args.push('--dangerously-skip-permissions')
      const nativeSessionId = session.nativeSessionId ?? session.claudeSessionId
      if (nativeSessionId) args.push('-r', nativeSessionId)
      cmd = [runtimeStore.executable('claude'), ...args]
    } else if (session.cliKind === 'codex') {
      const codexStore = useCodexConfigStore()
      const { isWindows } = usePlatform()
      const args: string[] = ['--no-alt-screen']
      // This is a Windows ConPTY/IME workaround. Unix PTYs must keep Codex's
      // normal paste heuristics and receive their UTF-8 input untouched.
      if (isWindows.value) args.push('-c', 'disable_paste_burst=true')
      try {
        await codexStore.ensureLoaded()
        if (codexStore.globalConfigError) {
          statusMessage.value = codexStore.globalConfigError
          return null
        }
        const activeProfileRef = codexStore.activeProfileRef
        if (activeProfileRef) {
          const context = await codexStore.resolveLaunchContext(activeProfileRef.profileId)
          args.push('--profile', context.managedProfileName)
          Object.assign(envVars, context.envVars)
        }
      } catch (error) {
        statusMessage.value = `CodeX 配置无法用于启动：${error}`
        return null
      }
      args.push('-C', project.path)
      if (session.nativeSessionId) {
        args.push('resume', session.nativeSessionId)
      } else if (session.launchMode === 'resume_picker') {
        args.push('resume')
      }
      cmd = [runtimeStore.executable('codex'), ...args]
    } else {
      const args: string[] = []
      const opencodeStore = useOpencodeConfigStore()
      try {
        await opencodeStore.resolveLaunchContext(
          session.cwd || project.path,
        )
      } catch (error) {
        statusMessage.value = `OpenCode 当前配置获取失败，已阻止启动：${error}`
        return null
      }
      if (session.nativeSessionId) args.push('--session', session.nativeSessionId)
      cmd = [runtimeStore.executable('opencode'), ...args]
    }

    const tabId = await terminalStore.createTab(cmd, envVars, session.cwd || project.path, session.name, {
      scope: 'project',
      projectSessionId: session.id,
      activate: false,
      cliKind: session.cliKind,
    })
    sessionTerminalIds.value[session.id] = tabId
    return tabId
  }

  async function closeSessionTerminal(sessionId = activeSessionId.value) {
    if (!sessionId) return
    const terminalStore = useTerminalStore()
    const tabId = sessionTerminalIds.value[sessionId]
    if (tabId) {
      await terminalStore.closeTab(tabId)
      delete sessionTerminalIds.value[sessionId]
    }
  }

  function openSidebar(defaultTab: SidebarTabType = 'tools', placement: SidebarPlacement = 'right') {
    sidebarPlacement.value = placement
    sidebarOpen.value = true
    if (visibleSidebarTabs.value.length === 0 && defaultTab === 'tools') {
      const tab = makeSidebarTab(defaultTab)
      sidebarTabs.value.push(tab)
      activeSidebarTabId.value = tab.id
    } else if (!activeSidebarTabId.value) {
      activeSidebarTabId.value = visibleSidebarTabs.value[0]?.id ?? null
    }
  }

  function closeSidebar() {
    sidebarOpen.value = false
  }

  function makeSidebarTab(type: SidebarTabType, payload?: string): SidebarTab {
    const titles: Record<SidebarTabType, string> = {
      tools: '工具',
      file: payload ? basename(payload) : '文件',
      terminal: '终端',
      browser: payload || '浏览器',
    }
    return {
      cliKind: activeCliKind.value,
      id: makeId('sidebar'),
      type,
      title: titles[type],
      path: type === 'file' ? payload : undefined,
      url: type === 'browser' ? payload : undefined,
      browserHistory: type === 'browser' && payload ? [payload] : undefined,
      browserHistoryIndex: type === 'browser' && payload ? 0 : undefined,
      browserRefreshKey: type === 'browser' ? 0 : undefined,
      dirty: false,
      viewMode: 'source',
    }
  }

  async function openSidebarTab(type: SidebarTabType, payload?: string) {
    if (type === 'file' && payload) {
      await openFile(payload)
      return
    }
    sidebarOpen.value = true
    const tab = makeSidebarTab(type, payload)
    sidebarTabs.value.push(tab)
    activeSidebarTabId.value = tab.id
    if (type === 'terminal') {
      await createSidebarTerminal(tab.id)
    }
    if (type === 'browser' && payload) {
      recordRecentItem('browser', payload, payload)
    }
  }

  async function closeSidebarTab(tabId: string) {
    const tab = sidebarTabs.value.find((t) => t.id === tabId)
    if (!tab) return
    if (tab.dirty) {
      const confirmed = window.confirm(`${tab.title} 尚未保存，确定关闭吗？`)
      if (!confirmed) return
    }
    if (tab.terminalId) {
      await useTerminalStore().closeTab(tab.terminalId)
    }
    sidebarTabs.value = sidebarTabs.value.filter((t) => t.id !== tabId)
    if (activeSidebarTabId.value === tabId) {
      activeSidebarTabId.value = visibleSidebarTabs.value.find((item) => item.id !== tabId)?.id ?? null
    }
  }

  async function openFile(path?: string, placement?: SidebarPlacement) {
    let filePath = path
    if (!filePath) {
      const selected = await open({ multiple: false, title: '选择文件' })
      if (typeof selected !== 'string') return
      filePath = selected
    }

    if (placement) sidebarPlacement.value = placement
    sidebarOpen.value = true
    const existing = visibleSidebarTabs.value.find((tab) => tab.type === 'file' && tab.path === filePath)
    if (existing) {
      activeSidebarTabId.value = existing.id
      return
    }

    try {
      const content = await invoke<string>('read_text_file', { path: filePath })
      const tab: SidebarTab = {
        ...makeSidebarTab('file', filePath),
        content,
        language: languageFromPath(filePath),
      }
      sidebarTabs.value.push(tab)
      activeSidebarTabId.value = tab.id
      recordRecentItem('file', tab.title, filePath)
    } catch (e) {
      statusMessage.value = String(e)
      window.setTimeout(() => {
        if (statusMessage.value === String(e)) statusMessage.value = ''
      }, 2600)
    }
  }

  async function saveFile(tabId = activeSidebarTabId.value ?? '') {
    const tab = sidebarTabs.value.find((item) => item.id === tabId)
    if (!tab || tab.type !== 'file' || !tab.path) return
    await invoke('save_text_file', { path: tab.path, content: tab.content ?? '' })
    tab.dirty = false
  }

  function updateFileContent(tabId: string, content: string) {
    const tab = sidebarTabs.value.find((item) => item.id === tabId)
    if (!tab || tab.type !== 'file') return
    tab.content = content
    tab.dirty = true
  }

  function setFileViewMode(tabId: string, mode: FileViewMode) {
    const tab = sidebarTabs.value.find((item) => item.id === tabId)
    if (tab) tab.viewMode = mode
  }

  async function createSidebarTerminal(tabId: string) {
    const tab = sidebarTabs.value.find((item) => item.id === tabId)
    if (!tab || tab.terminalId) return
    const project = activeProject.value
    const terminalId = await useTerminalStore().createTab(
      getDefaultShell(),
      {},
      project?.path ?? null,
      tab.title,
      {
        scope: 'sidebar',
        sidebarTabId: tab.id,
        activate: false,
        cliKind: tab.cliKind,
      },
    )
    tab.terminalId = terminalId
    recordRecentItem('terminal', tab.title, undefined)
  }

  function updateBrowserUrl(tabId: string, url: string, options: { replace?: boolean; record?: boolean } = {}) {
    const tab = sidebarTabs.value.find((item) => item.id === tabId)
    if (!tab || tab.type !== 'browser') return
    tab.url = url
    tab.title = url ? browserTitle(url) : '浏览器'
    tab.browserRefreshKey = (tab.browserRefreshKey ?? 0) + 1

    if (url) {
      const history = tab.browserHistory ?? []
      const currentIndex = tab.browserHistoryIndex ?? history.length - 1
      if (options.replace && currentIndex >= 0) {
        history[currentIndex] = url
        tab.browserHistory = history
        tab.browserHistoryIndex = currentIndex
      } else if (history[currentIndex] !== url) {
        const nextHistory = history.slice(0, currentIndex + 1)
        nextHistory.push(url)
        tab.browserHistory = nextHistory
        tab.browserHistoryIndex = nextHistory.length - 1
      }
    }

    if (url && options.record !== false) recordRecentItem('browser', url, url)
  }

  function goBrowserBack(tabId: string) {
    const tab = sidebarTabs.value.find((item) => item.id === tabId)
    if (!tab || tab.type !== 'browser') return
    const history = tab.browserHistory ?? []
    const currentIndex = tab.browserHistoryIndex ?? history.length - 1
    if (currentIndex <= 0) return
    const nextIndex = currentIndex - 1
    tab.browserHistoryIndex = nextIndex
    updateBrowserUrl(tabId, history[nextIndex], { replace: true, record: true })
  }

  function goBrowserForward(tabId: string) {
    const tab = sidebarTabs.value.find((item) => item.id === tabId)
    if (!tab || tab.type !== 'browser') return
    const history = tab.browserHistory ?? []
    const currentIndex = tab.browserHistoryIndex ?? history.length - 1
    if (currentIndex >= history.length - 1) return
    const nextIndex = currentIndex + 1
    tab.browserHistoryIndex = nextIndex
    updateBrowserUrl(tabId, history[nextIndex], { replace: true, record: true })
  }

  function refreshBrowser(tabId: string) {
    const tab = sidebarTabs.value.find((item) => item.id === tabId)
    if (!tab || tab.type !== 'browser') return
    tab.browserRefreshKey = (tab.browserRefreshKey ?? 0) + 1
  }

  function recordRecentItem(type: RecentItem['type'], name: string, payload?: string) {
    const project = activeProject.value
    if (!project) return
    const item: RecentItem = {
      type,
      name,
      path: type === 'file' ? payload : undefined,
      url: type === 'browser' ? payload : undefined,
      openedAt: now(),
    }
    project.recentItems = [
      item,
      ...project.recentItems.filter((existing) => {
        if (type === 'file') return existing.path !== item.path
        if (type === 'browser') return existing.url !== item.url
        return existing.name !== item.name || existing.type !== item.type
      }),
    ].slice(0, 20)
    project.updatedAt = now()
    persist().catch(() => {})
  }

  async function openRecent(item: RecentItem) {
    if (item.type === 'file' && item.path) {
      await openFile(item.path)
    } else if (item.type === 'browser' && item.url) {
      await openSidebarTab('browser', item.url)
    } else if (item.type === 'terminal') {
      await openSidebarTab('terminal')
    }
  }

  return {
    activeCliKind,
    projects,
    sessions,
    visibleProjects,
    visibleSessions,
    visibleSidebarTabs,
    expandedProjectIds,
    activeProjectId,
    activeSessionId,
    projectSortMode,
    sidebarOpen,
    sidebarPlacement,
    leftSidebarCollapsed,
    sidebarTabs,
    activeSidebarTabId,
    statusMessage,
    sessionTerminalIds,
    activeProject,
    sessionsOfActiveProject,
    activeSession,
    activeSidebarTab,
    recentItemsOfActiveProject,
    loadProjects,
    prepareCliWorkspace,
    setActiveCliKind,
    refreshActiveCliHistory,
    refreshClaudeHistory,
    pickAndAddProject,
    addProject,
    removeProject,
    renameProject,
    replaceProjectPath,
    toggleProjectExpanded,
    toggleLeftSidebarCollapsed,
    setProjectSortMode,
    reorderProjects,
    activateProject,
    launchClaudeFromConfig,
    createSession,
    resumeCodexSession,
    pickAndResumeCodexSession,
    activateSession,
    renameSession,
    removeSession,
    getSessionStatus,
    ensureSessionTerminal,
    closeSessionTerminal,
    openSidebar,
    closeSidebar,
    openSidebarTab,
    closeSidebarTab,
    openFile,
    saveFile,
    updateFileContent,
    setFileViewMode,
    createSidebarTerminal,
    updateBrowserUrl,
    goBrowserBack,
    goBrowserForward,
    refreshBrowser,
    recordRecentItem,
    openRecent,
  }
})
