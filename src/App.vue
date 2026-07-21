<template>
  <div class="app-layout">
    <!-- Custom title bar -->
    <header class="title-bar" data-tauri-drag-region @dblclick="toggleMaximize">
      <div class="title-bar__left">
        <button
          class="title-bar__icon-btn"
          :title="leftSidebarToggleTitle"
          :aria-label="leftSidebarToggleTitle"
          :disabled="!appReady"
          data-tauri-drag-region="false"
          @click="toggleActiveLeftSidebar"
          @dblclick.stop
        >
          <span class="sidebar-toggle-icon sidebar-toggle-icon--left" aria-hidden="true"></span>
        </button>
      </div>

      <nav class="title-bar__tabs" @dblclick.stop>
        <button
          class="title-bar__tab"
          :class="{ active: mainTab === 'config' }"
          :disabled="!appReady"
          data-tauri-drag-region="false"
          @click="openConfigTab"
        >
          配置
        </button>
        <button
          class="title-bar__tab"
          :class="{ active: mainTab === 'claude' }"
          :disabled="!appReady"
          data-tauri-drag-region="false"
          @click="openClaudeTab"
        >
          {{ CLI_DESCRIPTORS.claude.label }}
        </button>
        <button
          class="title-bar__tab"
          :class="{ active: mainTab === 'codex' }"
          :disabled="!appReady"
          data-tauri-drag-region="false"
          @click="openCodexTab"
        >
          {{ CLI_DESCRIPTORS.codex.label }}
        </button>
        <button
          class="title-bar__tab"
          :class="{ active: mainTab === 'opencode' }"
          :disabled="!appReady"
          data-tauri-drag-region="false"
          @click="openOpencodeTab"
        >
          {{ CLI_DESCRIPTORS.opencode.label }}
        </button>
      </nav>

      <div class="title-bar__controls" @dblclick.stop>
        <button
          class="title-bar__control"
          data-tauri-drag-region="false"
          title="最小化"
          aria-label="最小化"
          @click="minimizeWindow"
        >
          <span class="title-bar__window-icon title-bar__window-icon--minimize" aria-hidden="true"></span>
        </button>
        <button
          class="title-bar__control"
          data-tauri-drag-region="false"
          title="最大化/还原"
          aria-label="最大化/还原"
          @click="toggleMaximize"
        >
          <span
            class="title-bar__window-icon"
            :class="isMaximized ? 'title-bar__window-icon--restore' : 'title-bar__window-icon--maximize'"
            aria-hidden="true"
          ></span>
        </button>
        <button
          class="title-bar__control title-bar__control--close"
          data-tauri-drag-region="false"
          title="关闭"
          aria-label="关闭"
          @click="closeWindow"
        >
          <span class="title-bar__window-icon title-bar__window-icon--close" aria-hidden="true"></span>
        </button>
      </div>
    </header>

    <!-- Content area -->
    <main v-if="appReady" class="app-content">
      <!-- Config panels -->
      <div v-show="mainTab === 'config'" class="app-panel">
        <ConfigWorkspace :sidebar-collapsed="configSidebarCollapsed" />
      </div>

      <!-- Terminal panel — always mounted to preserve state -->
      <div v-show="mainTab === 'terminal'" class="app-panel">
        <TerminalManager ref="terminalManagerRef" :launch-dir="activeLaunchDir" />
      </div>

      <!-- Shared CLI workspace — keep mounted while on the config tab so
           xterm instances (scrollback, mouse modes) survive tab switches. -->
      <div
        v-if="workspaceCliKind && workspaceCliStatus?.state === 'ready'"
        v-show="activeCliKind === mainTab"
        class="app-panel"
      >
        <ProjectPanel
          :cli-kind="workspaceCliKind"
          @open-settings="showSettings = !showSettings"
        />
      </div>

      <!-- Orchestration panel -->
      <div v-show="mainTab === 'orchestration'" class="app-panel">
        <OrchestrationManager />
      </div>
    </main>

    <!-- Status bar -->
    <StatusBar v-if="appReady" :items="statusItems" />

    <div
      v-if="dependencyState !== 'ready'"
      class="dependency-gate"
      role="alert"
      aria-live="polite"
    >
      <section class="dependency-gate__card">
        <div class="dependency-gate__icon" aria-hidden="true">
          {{ dependencyGateIcon }}
        </div>
        <h1>{{ dependencyGateTitle }}</h1>
        <p class="dependency-gate__description">{{ dependencyGateMessage }}</p>
        <p v-if="dependencyResult?.version" class="dependency-gate__detail">
          当前版本：{{ dependencyResult.version }}
        </p>
        <p v-if="dependencyActionMessage" class="dependency-gate__feedback">
          {{ dependencyActionMessage }}
        </p>

        <div v-if="dependencyState === 'checking' || dependencyState === 'installing'" class="dependency-gate__progress">
          <span class="dependency-gate__spinner" aria-hidden="true"></span>
          {{ dependencyState === 'installing' ? '请等待安装命令完成。' : '正在检查系统环境。' }}
        </div>

        <div v-else-if="dependencyState === 'restart_required'" class="dependency-gate__actions">
          <button class="dependency-gate__button dependency-gate__button--primary" @click="closeWindow">
            关闭应用
          </button>
        </div>

        <div v-else class="dependency-gate__actions">
          <button class="dependency-gate__button dependency-gate__button--secondary" @click="openDependencyWebsite">
            前往官网下载
          </button>
          <button
            v-if="canInstallDependency"
            class="dependency-gate__button dependency-gate__button--primary"
            @click="installActiveDependency"
          >
            通过 winget 安装
          </button>
          <button
            v-if="dependencyState === 'error'"
            class="dependency-gate__button dependency-gate__button--secondary"
            @click="retryDependencyCheck"
          >
            重新检测
          </button>
          <button class="dependency-gate__button dependency-gate__button--link" @click="requestRestartAfterManualInstall">
            我已完成手动安装
          </button>
        </div>
        <p v-if="dependencyState !== 'checking' && dependencyState !== 'installing'" class="dependency-gate__hint">
          安装完成后请关闭并重新打开应用；当前进程不会自动更新系统 PATH。
        </p>
      </section>
    </div>

    <div
      v-if="cliGateVisible"
      class="dependency-gate project-claude-gate"
      role="alert"
      aria-live="polite"
    >
      <section class="dependency-gate__card">
        <div class="dependency-gate__icon" aria-hidden="true">
          {{ cliGateChecking ? '⏳' : '✦' }}
        </div>
        <h1>{{ cliGateTitle }}</h1>
        <p class="dependency-gate__description">
          {{ cliGateDescription }}
        </p>
        <p v-if="activeCliStatus?.version" class="dependency-gate__detail">
          当前版本：{{ activeCliStatus.version }}
        </p>
        <p v-if="activeCliStatus?.executablePath" class="dependency-gate__detail">
          可执行文件：{{ activeCliStatus.executablePath }}
        </p>

        <div v-if="cliGateChecking" class="dependency-gate__progress">
          <span class="dependency-gate__spinner" aria-hidden="true"></span>
          {{ cliGateProgressText }}
        </div>

        <div v-else class="dependency-gate__actions">
          <button class="dependency-gate__button dependency-gate__button--primary" @click="cliInstallHelpVisible = !cliInstallHelpVisible">
            安装说明
          </button>
          <button
            class="dependency-gate__button dependency-gate__button--secondary"
            @click="retryCliGateCheck"
          >
            重新检测
          </button>
          <button class="dependency-gate__button dependency-gate__button--link" @click="openConfigTab">
            返回配置
          </button>
        </div>
        <p v-if="cliInstallHelpVisible" class="dependency-gate__hint">
          {{ cliInstallHint }}
        </p>
        <p v-else-if="!cliGateChecking" class="dependency-gate__hint">
          完成安装或权限修复后，点击“重新检测”。只会重新检查 {{ activeCliLabel }}。
        </p>
      </section>
    </div>

    <div v-if="appReady && showSettings" class="settings-popover">
      <div class="settings-dropdown__section">主题</div>
      <button
        class="settings-dropdown__item"
        :class="{ active: theme === 'light' }"
        @click="setTheme('light')"
      >
        <span class="settings-dropdown__check" v-if="theme === 'light'">✓</span>
        <span class="settings-dropdown__check" v-else></span>
        浅色
      </button>
      <button
        class="settings-dropdown__item"
        :class="{ active: theme === 'dark' }"
        @click="setTheme('dark')"
      >
        <span class="settings-dropdown__check" v-if="theme === 'dark'">✓</span>
        <span class="settings-dropdown__check" v-else></span>
        深色
      </button>

      <div class="settings-dropdown__section">项目终端拖入文件</div>
      <button
        class="settings-dropdown__item"
        :class="{ active: claudeStore.projectDropPathMode === 'relative' }"
        @click="setProjectDropPathMode('relative')"
      >
        <span class="settings-dropdown__check" v-if="claudeStore.projectDropPathMode === 'relative'">✓</span>
        <span class="settings-dropdown__check" v-else></span>
        相对路径
      </button>
      <button
        class="settings-dropdown__item"
        :class="{ active: claudeStore.projectDropPathMode === 'filename' }"
        @click="setProjectDropPathMode('filename')"
      >
        <span class="settings-dropdown__check" v-if="claudeStore.projectDropPathMode === 'filename'">✓</span>
        <span class="settings-dropdown__check" v-else></span>
        仅文件名
      </button>

      <div class="settings-dropdown__section">终端字体大小</div>
      <div class="settings-dropdown__item font-size-row">
        <button
          class="font-size-btn"
          :disabled="terminalStore.fontSize <= 6"
          @click="terminalStore.setFontSize(terminalStore.fontSize - 1)"
        >−</button>
        <span class="font-size-value">{{ terminalStore.fontSize }}</span>
        <button
          class="font-size-btn"
          :disabled="terminalStore.fontSize >= 28"
          @click="terminalStore.setFontSize(terminalStore.fontSize + 1)"
        >+</button>
      </div>

      <div class="settings-dropdown__section">APP 字体大小</div>
      <div class="settings-dropdown__item font-size-row">
        <button
          class="font-size-btn"
          :disabled="appFontSize <= APP_FONT_MIN"
          @click="setAppFontSize(-1)"
        >−</button>
        <span class="font-size-value">{{ appFontSize }}px</span>
        <button
          class="font-size-btn"
          :disabled="appFontSize >= APP_FONT_MAX"
          @click="setAppFontSize(1)"
        >+</button>
      </div>

      <div class="settings-dropdown__section">Markdown 字体大小</div>
      <div class="settings-dropdown__item font-size-row">
        <button
          class="font-size-btn"
          :disabled="mdFontSize <= MD_FONT_MIN"
          @click="setMdFontSize(mdFontSize - 1)"
        >−</button>
        <span class="font-size-value">{{ mdFontSize }}px</span>
        <button
          class="font-size-btn"
          :disabled="mdFontSize >= MD_FONT_MAX"
          @click="setMdFontSize(mdFontSize + 1)"
        >+</button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, nextTick, onMounted, onBeforeUnmount, watch } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { open } from '@tauri-apps/plugin-shell'
import ConfigWorkspace from './components/config/ConfigWorkspace.vue'
import TerminalManager from './components/terminal/TerminalManager.vue'
import ProjectPanel from './components/project/ProjectPanel.vue'
import OrchestrationManager from './components/orchestration/OrchestrationManager.vue'
import StatusBar from './components/common/StatusBar.vue'
import { useClaudeStore } from './stores/claude'
import { useTerminalStore } from './stores/terminal'
import { useMarkdownFontSize, MD_FONT_MIN, MD_FONT_MAX } from './composables/useMarkdownFontSize'
import { useSettingsPopover } from './composables/useSettingsPopover'
import { useProjectStore } from './stores/project'
import { useCliRuntimeStore } from './stores/cliRuntime'
import { useConfigWorkspaceStore } from './stores/configWorkspace'
import { usePlatform } from './composables/usePlatform'
import {
  CLI_DESCRIPTORS,
  isCliKind,
  normalizePersistedMainTab,
  type CliKind,
  type MainTab,
} from './types/cli'

type DependencyName = 'node' | 'git'
type DependencyStatus = 'installed' | 'missing' | 'unsupported' | 'error'
type DependencyGateState = 'checking' | 'missing' | 'unsupported' | 'error' | 'installing' | 'restart_required' | 'ready'

interface DependencyCheckResult {
  dependency: DependencyName
  status: DependencyStatus
  path: string | null
  version: string | null
  message: string
}

interface DependencyInstallResult {
  dependency: DependencyName
  displayName: string
  message: string
}

const mainTab = ref<MainTab>('config')
const claudeStore = useClaudeStore()
const terminalStore = useTerminalStore()
const projectStore = useProjectStore()
const cliRuntimeStore = useCliRuntimeStore()
const configWorkspaceStore = useConfigWorkspaceStore()
const { isWindows, isMacOS } = usePlatform()
const terminalManagerRef = ref<InstanceType<typeof TerminalManager> | null>(null)
const configSidebarCollapsed = ref(false)
const dependencyState = ref<DependencyGateState>('checking')
const dependencyResult = ref<DependencyCheckResult | null>(null)
const dependencyActionMessage = ref('')
const cliInstallHelpVisible = ref(false)
const cliWorkspacePreparation = ref<{ kind: CliKind; requestId: number } | null>(null)
let readyAppInitialized = false
let cliOpenRequestId = 0
const MIN_CLI_WORKSPACE_GATE_MS = 320

const appReady = computed(() => dependencyState.value === 'ready')
const activeCliKind = computed<CliKind | null>(() => isCliKind(mainTab.value) ? mainTab.value : null)
const activeCliStatus = computed(() => activeCliKind.value
  ? cliRuntimeStore.statuses[activeCliKind.value]
  : null)
// The workspace panel must stay mounted on non-CLI tabs (e.g. config);
// fall back to the kind persisted in the project store.
const workspaceCliKind = computed<CliKind>(() => activeCliKind.value ?? projectStore.activeCliKind)
const workspaceCliStatus = computed(() => cliRuntimeStore.statuses[workspaceCliKind.value] ?? null)
const activeCliLabel = computed(() => activeCliKind.value
  ? CLI_DESCRIPTORS[activeCliKind.value].label
  : '')
const cliWorkspacePreparing = computed(() => !!activeCliKind.value
  && cliWorkspacePreparation.value?.kind === activeCliKind.value)
const cliGateVisible = computed(() => appReady.value
  && !!activeCliKind.value
  && (cliWorkspacePreparing.value || activeCliStatus.value?.state !== 'ready'))
const cliGateChecking = computed(() => cliWorkspacePreparing.value
  || !activeCliStatus.value
  || activeCliStatus.value.state === 'checking')
const cliGateTitle = computed(() => {
  if (!activeCliStatus.value || activeCliStatus.value.state === 'checking') {
    return `正在检查 ${activeCliLabel.value}`
  }
  if (cliWorkspacePreparing.value) return `正在整理 ${activeCliLabel.value} 工作区`
  if (activeCliStatus.value?.issueCode === 'executable_missing') return `未检测到 ${activeCliLabel.value}`
  return `${activeCliLabel.value} 暂不可用`
})
const cliGateDescription = computed(() => cliWorkspacePreparing.value
  && activeCliStatus.value?.state === 'ready'
  ? '正在同步项目和历史会话，并完成项目列表排序。'
  : activeCliStatus.value?.message ?? '')
const cliGateProgressText = computed(() => cliWorkspacePreparing.value
  && activeCliStatus.value?.state === 'ready'
  ? `正在整理 ${activeCliLabel.value} 的项目与会话。`
  : `正在检查 ${activeCliLabel.value}。`)
const cliInstallHint = computed(() => {
  const restart = isMacOS.value ? '；安装后请完全退出并重新打开应用' : ''
  if (activeCliKind.value === 'claude') return `npm 安装命令：npm install -g @anthropic-ai/claude-code${restart}`
  if (activeCliKind.value === 'codex') {
    return isMacOS.value
      ? `Homebrew：brew install --cask codex；或 npm install -g @openai/codex${restart}`
      : 'npm 安装命令：npm install -g @openai/codex'
  }
  return isMacOS.value
    ? `Homebrew：brew install anomalyco/tap/opencode；或 npm install -g opencode-ai${restart}`
    : 'npm 安装命令：npm install -g opencode-ai'
})
const activeDependencyName = computed(() => dependencyResult.value?.dependency === 'git' ? 'Git' : 'Node.js')
const canInstallDependency = computed(() => {
  return isWindows.value
    && (dependencyState.value === 'missing' || dependencyState.value === 'unsupported')
})
const dependencyGateTitle = computed(() => {
  if (dependencyState.value === 'checking') return '正在检查运行环境'
  if (dependencyState.value === 'installing') return `正在安装 ${activeDependencyName.value}`
  if (dependencyState.value === 'restart_required') return '安装完成，请重启应用'
  if (dependencyState.value === 'unsupported') return `${activeDependencyName.value} 版本不兼容`
  if (dependencyState.value === 'error') return `${activeDependencyName.value} 检测失败`
  return `未检测到 ${activeDependencyName.value}`
})
const dependencyGateIcon = computed(() => {
  if (dependencyState.value === 'checking' || dependencyState.value === 'installing') return '⏳'
  if (dependencyState.value === 'restart_required') return '↻'
  if (dependencyState.value === 'error') return '⚠'
  return dependencyResult.value?.dependency === 'git' ? '◆' : '⬢'
})
const dependencyGateMessage = computed(() => {
  if (dependencyState.value === 'checking') return 'Claude Code 启动器正在依次检查 Node.js 和 Git。'
  if (dependencyState.value === 'installing') return `正在通过 winget 安装 ${activeDependencyName.value}。`
  if (dependencyState.value === 'restart_required') {
    return dependencyActionMessage.value || '系统环境已变更。请关闭并重新打开应用后继续。'
  }
  return dependencyResult.value?.message || '无法确定依赖状态，请重试或手动安装。'
})

const leftSidebarToggleTitle = computed(() => {
  if (isCliKind(mainTab.value)) return '折叠/展开项目边栏'
  if (mainTab.value === 'config') return '折叠/展开配置边栏'
  return '折叠/展开左侧边栏'
})

function toggleActiveLeftSidebar() {
  if (!appReady.value) return
  if (isCliKind(mainTab.value)) {
    projectStore.toggleLeftSidebarCollapsed()
    return
  }
  if (mainTab.value === 'config') {
    configSidebarCollapsed.value = !configSidebarCollapsed.value
  }
}

function openConfigTab() {
  mainTab.value = 'config'
}

// ── Window controls ────────────────────────────────────────────────────────
const win = getCurrentWindow()
const isMaximized = ref(false)

async function minimizeWindow() {
  await win.minimize().catch(() => {})
}

async function toggleMaximize() {
  await win.toggleMaximize().catch(() => {})
  isMaximized.value = await win.isMaximized().catch(() => false)
}

async function closeWindow() {
  // Let the registered onCloseRequested handler perform the actual cleanup.
  await win.close().catch(() => {})
}

function setBlockedDependency(result: DependencyCheckResult) {
  dependencyResult.value = result
  dependencyActionMessage.value = ''
  dependencyState.value = result.status === 'unsupported'
    ? 'unsupported'
    : result.status === 'error'
      ? 'error'
      : 'missing'
}

async function runDependencyCheck() {
  dependencyState.value = 'checking'
  dependencyResult.value = null
  dependencyActionMessage.value = ''

  let nodeResult: DependencyCheckResult
  try {
    nodeResult = await invoke<DependencyCheckResult>('check_node_dependency')
  } catch (error) {
    setBlockedDependency({
      dependency: 'node',
      status: 'error',
      path: null,
      version: null,
      message: `无法检查 Node.js：${String(error)}`,
    })
    return
  }

  if (nodeResult.status !== 'installed') {
    setBlockedDependency(nodeResult)
    return
  }

  let gitResult: DependencyCheckResult
  try {
    gitResult = await invoke<DependencyCheckResult>('check_git_dependency')
  } catch (error) {
    setBlockedDependency({
      dependency: 'git',
      status: 'error',
      path: null,
      version: null,
      message: `无法检查 Git：${String(error)}`,
    })
    return
  }

  if (gitResult.status !== 'installed') {
    setBlockedDependency(gitResult)
    return
  }

  dependencyState.value = 'ready'
  await initializeReadyApp()
}

async function retryDependencyCheck() {
  await runDependencyCheck()
}

async function openDependencyWebsite() {
  const url = dependencyResult.value?.dependency === 'git'
    ? 'https://git-scm.com/downloads'
    : 'https://nodejs.org/en/download'
  try {
    await open(url)
    dependencyActionMessage.value = '已在默认浏览器中打开下载页面。'
  } catch (error) {
    dependencyActionMessage.value = `无法打开下载页面：${String(error)}`
  }
}

async function installActiveDependency() {
  const dependency = dependencyResult.value?.dependency
  if (!dependency || !canInstallDependency.value) return

  const displayName = dependency === 'git' ? 'Git' : 'Node.js LTS'
  const packageId = dependency === 'git' ? 'Git.Git' : 'OpenJS.NodeJS.LTS'
  const confirmed = window.confirm(
    `将通过 winget 安装 ${displayName}（${packageId}）。\n\n继续即表示同意 winget 源和该软件包的许可协议；安装程序可能请求管理员授权。`
  )
  if (!confirmed) return

  const previousState = dependencyState.value
  dependencyState.value = 'installing'
  dependencyActionMessage.value = ''
  try {
    const result = await invoke<DependencyInstallResult>('install_dependency_via_winget', { dependency })
    dependencyActionMessage.value = result.message
    dependencyState.value = 'restart_required'
  } catch (error) {
    dependencyActionMessage.value = `安装失败：${String(error)}`
    dependencyState.value = previousState
  }
}

function requestRestartAfterManualInstall() {
  dependencyActionMessage.value = '请关闭应用，完成安装后重新打开。应用重启后会重新检查环境。'
  dependencyState.value = 'restart_required'
}

// ── Theme ──────────────────────────────────────────────────────────────────
const { showSettings } = useSettingsPopover()
const theme = ref<'light' | 'dark'>('light')

function applyTheme(t: 'light' | 'dark') {
  theme.value = t
  document.documentElement.setAttribute('data-theme', t)
  localStorage.setItem('app-theme', t)
  invoke('set_titlebar_theme', { dark: t === 'dark' }).catch(() => {
    // ignore on non-Windows platforms or dev browser
  })
}

function setTheme(t: 'light' | 'dark') {
  applyTheme(t)
  showSettings.value = false
}

function setProjectDropPathMode(mode: 'filename' | 'relative') {
  claudeStore.projectDropPathMode = mode
  showSettings.value = false
}

// ── App font size ──────────────────────────────────────────────────────────
const APP_FONT_MIN = 10
const APP_FONT_MAX = 18
const appFontSize = ref(13)
const { fontSize: mdFontSize, setFontSize: setMdFontSize } = useMarkdownFontSize()

function applyAppFontSize(size: number) {
  const clamped = Math.max(APP_FONT_MIN, Math.min(APP_FONT_MAX, size))
  appFontSize.value = clamped
  document.documentElement.style.setProperty('--font-size-base', `${clamped}px`)
  document.documentElement.style.setProperty('--font-size-title', `${clamped + 1}px`)
  document.documentElement.style.setProperty('--font-size-small', `${clamped - 1}px`)
  localStorage.setItem('app-font-size', String(clamped))
}

function setAppFontSize(delta: number) {
  applyAppFontSize(appFontSize.value + delta)
}

function loadAppFontSize() {
  const saved = parseInt(localStorage.getItem('app-font-size') ?? '', 10)
  if (!isNaN(saved)) {
    applyAppFontSize(saved)
  }
}

function loadTheme() {
  const saved = localStorage.getItem('app-theme') as 'light' | 'dark' | null
  if (saved === 'dark' || saved === 'light') {
    applyTheme(saved)
  }
}

function onDocumentClick(e: MouseEvent) {
  const target = e.target as HTMLElement
  if (!target.closest('.settings-popover') && !target.closest('.settings-entry')) {
    showSettings.value = false
  }
}

async function openCliTab(kind: CliKind, forceCheck = false) {
  if (!appReady.value || cliRuntimeStore.checking[kind]) return
  if (mainTab.value === 'config'
    && !(await configWorkspaceStore.confirmDiscardActiveChanges(`进入 ${CLI_DESCRIPTORS[kind].label} 工作区`))) {
    return
  }
  const requestId = ++cliOpenRequestId
  const gateStartedAt = performance.now()
  mainTab.value = kind
  projectStore.setActiveCliKind(kind)
  cliInstallHelpVisible.value = false
  cliWorkspacePreparation.value = { kind, requestId }
  let workspaceReady = false

  try {
    const status = await cliRuntimeStore.check(kind, forceCheck)
    if (requestId !== cliOpenRequestId || mainTab.value !== kind) return
    if (status.state !== 'ready') return

    await projectStore.prepareCliWorkspace(kind)
    if (requestId !== cliOpenRequestId || mainTab.value !== kind) return

    // Project/session discovery mutates the list several times. Keep it covered
    // until Vue has rendered the final computed ordering behind the gate.
    await nextTick()
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
    if (requestId !== cliOpenRequestId || mainTab.value !== kind) return

    const remainingGateTime = MIN_CLI_WORKSPACE_GATE_MS - (performance.now() - gateStartedAt)
    if (remainingGateTime > 0) {
      await new Promise<void>((resolve) => setTimeout(resolve, remainingGateTime))
    }
    if (requestId !== cliOpenRequestId || mainTab.value !== kind) return
    workspaceReady = true
  } finally {
    if (cliWorkspacePreparation.value?.requestId === requestId) {
      cliWorkspacePreparation.value = null
      if (workspaceReady) {
        await nextTick()
        terminalStore.triggerRefit()
      }
    }
  }
}

async function openClaudeTab() {
  await openCliTab('claude')
}

async function openCodexTab() {
  await openCliTab('codex')
}

async function openOpencodeTab() {
  await openCliTab('opencode')
}

async function retryCliGateCheck() {
  if (!activeCliKind.value) return
  await openCliTab(activeCliKind.value, true)
}

const activeLaunchDir = computed(() => claudeStore.launchDir)
let unlistenClaudeHistory: (() => void) | undefined
let refreshingClaudeHistory = false

async function refreshClaudeHistory() {
  if (!appReady.value) return
  if (refreshingClaudeHistory) return
  refreshingClaudeHistory = true
  try {
    await Promise.all([
      claudeStore.loadRecentProjects(),
      claudeStore.loadSessions({ resetDisplayCount: false }),
    ])
    await projectStore.refreshClaudeHistory()
  } catch (error) {
    console.error('Failed to refresh Claude history:', error)
  } finally {
    refreshingClaudeHistory = false
  }
}

// Switch to terminal view when the Claude store requests it
watch(() => claudeStore.switchToTerminal, async (val) => {
  if (val && appReady.value) {
    if (mainTab.value === 'config'
      && !(await configWorkspaceStore.confirmDiscardActiveChanges('进入终端'))) {
      claudeStore.switchToTerminal = false
      return
    }
    mainTab.value = 'terminal'
    claudeStore.switchToTerminal = false
  }
})

// Built-in Claude launches from the config panel now live inside Claude Code.
watch(() => claudeStore.switchToProject, async (val) => {
  if (val && appReady.value) {
    claudeStore.switchToProject = false
    await openClaudeTab()
  }
})

// Ensure terminal panes re-fit when switching from config back to terminal tab.
// The ResizeObserver skips fits when the container is hidden (0×0), so we need
// an explicit signal when the panel becomes visible again.
watch(mainTab, (tab) => {
  showSettings.value = false
  if (tab === 'terminal') {
    terminalStore.triggerRefit()
  }
  if (isCliKind(tab)) {
    projectStore.setActiveCliKind(tab)
    terminalStore.triggerRefit()
  }
  invoke('save_last_active_main_tab', { tab }).catch(() => {})
})

// ── Status bar ─────────────────────────────────────────────────────────────
const claudeConfigDir = ref<string>('')

const statusItems = computed(() => {
  if (mainTab.value === 'terminal') {
    return [
      { label: '模式', value: '终端' },
      { label: '标签页', value: String(terminalStore.terminalTabs.length) },
    ]
  }
  if (activeCliKind.value) {
    return [
      { label: '模式', value: CLI_DESCRIPTORS[activeCliKind.value].label },
      { label: '项目', value: projectStore.activeProject?.name ?? '未选择' },
      { label: '会话', value: projectStore.activeSession?.name ?? '未选择' },
    ]
  }
  if (mainTab.value === 'orchestration') {
    return [
      { label: '模式', value: '编排' },
      { label: '标签页', value: String(terminalStore.tabs.length) },
    ]
  }
  return [
    { label: '工具', value: CLI_DESCRIPTORS.claude.label },
    ...(claudeStore.claudeExePath
      ? [{ label: '路径', value: claudeStore.claudeExePath }]
      : [{ label: '状态', value: '未安装' }]),
    ...(claudeConfigDir.value
      ? [{ label: '配置目录', value: claudeConfigDir.value }]
      : []),
  ]
})

// ── Window size persistence ────────────────────────────────────────────────
interface WindowState {
  width?: number
  height?: number
  x?: number
  y?: number
}

async function loadWindowState() {
  try {
    const state = await invoke<WindowState>('load_window_state')
    const win = getCurrentWindow()
    if (state && state.width && state.height) {
      const { LogicalSize } = await import('@tauri-apps/api/dpi')
      await win.setSize(new LogicalSize(state.width, state.height))
    }
    if (state && state.x !== undefined && state.y !== undefined) {
      const { LogicalPosition } = await import('@tauri-apps/api/dpi')
      await win.setPosition(new LogicalPosition(state.x, state.y))
    }
  } catch {
    // use defaults from tauri.conf.json
  }
}

async function saveWindowState() {
  try {
    const win = getCurrentWindow()
    const size = await win.innerSize()       // physical pixels
    const pos = await win.outerPosition()    // physical pixels
    const scale = await win.scaleFactor()    // e.g. 1.25 on 125% DPI
    await invoke('save_window_state', {
      state: {
        width: size.width / scale,           // store as logical pixels
        height: size.height / scale,
        x: pos.x / scale,
        y: pos.y / scale,
      },
    })
  } catch {
    // ignore
  }
}

async function loadLastMainTab() {
  try {
    const savedTab = await invoke<string>('load_last_active_main_tab')
    const tab = normalizePersistedMainTab(savedTab)
    if (tab === 'claude' || tab === 'codex' || tab === 'opencode' || tab === 'config') {
      mainTab.value = tab
    } else if (tab === 'terminal' || tab === 'orchestration') {
      mainTab.value = 'config'
    }
  } catch {
    // keep default
  }
}

function cycleProjectSession() {
  const sessions = projectStore.sessionsOfActiveProject
  if (sessions.length <= 1) return
  const currentIdx = sessions.findIndex((session) => session.id === projectStore.activeSessionId)
  const nextIdx = (currentIdx + 1) % sessions.length
  projectStore.activateSession(sessions[nextIdx].id)
}

// ── Global keyboard shortcuts ──────────────────────────────────────────────
function onKeyDown(e: KeyboardEvent) {
  if (!appReady.value) return
  const primaryModifier = isMacOS.value ? e.metaKey : e.ctrlKey
  if (!primaryModifier) return

  if (isCliKind(mainTab.value)) {
    if (activeCliStatus.value?.state !== 'ready') return
    if (e.key === 't' || e.key === 'T') {
      e.preventDefault()
      projectStore.createSession()
      return
    }

    if (e.key === 'w' || e.key === 'W') {
      e.preventDefault()
      projectStore.closeSessionTerminal()
      return
    }

    if (e.key === 'Tab') {
      e.preventDefault()
      cycleProjectSession()
      return
    }

    if (e.key === 'p' || e.key === 'P') {
      e.preventDefault()
      projectStore.openFile()
      return
    }

    if (e.key === 's' || e.key === 'S') {
      e.preventDefault()
      projectStore.saveFile()
      return
    }

    if (e.shiftKey && (e.key === 'b' || e.key === 'B')) {
      e.preventDefault()
      projectStore.sidebarOpen ? projectStore.closeSidebar() : projectStore.openSidebar('tools')
      return
    }
  }

  if (e.key === 'w' || e.key === 'W') {
    e.preventDefault()
    if (mainTab.value === 'terminal' && terminalStore.activeTabId !== null) {
      terminalStore.closeTab(terminalStore.activeTabId)
    }
    return
  }

  if (e.key === 'Tab') {
    e.preventDefault()
    if (mainTab.value === 'terminal' && terminalStore.terminalTabs.length > 1) {
      const ids = terminalStore.terminalTabs.map(t => t.id)
      const currentIdx = ids.indexOf(terminalStore.activeTabId ?? -1)
      const nextIdx = (currentIdx + 1) % ids.length
      terminalStore.activateTab(ids[nextIdx])
    }
    return
  }
}

// ── Lifecycle ──────────────────────────────────────────────────────────────
async function initializeReadyApp() {
  if (readyAppInitialized) return
  readyAppInitialized = true

  try {
    claudeConfigDir.value = await invoke<string>('get_claude_config_dir')
  } catch {
    // status bar can omit the directory when the app-data location is unavailable
  }

  if (isCliKind(mainTab.value)) {
    await openCliTab(mainTab.value)
  }
}

onMounted(async () => {
  loadTheme()
  loadAppFontSize()
  await loadWindowState()
  await loadLastMainTab()
  isMaximized.value = await win.isMaximized().catch(() => false)

  window.addEventListener('keydown', onKeyDown)
  document.addEventListener('click', onDocumentClick)
  unlistenClaudeHistory = await listen('claude_history_changed', () => {
    if (!appReady.value) return
    refreshClaudeHistory().catch((error) => {
      console.error('Failed to refresh Claude history:', error)
    })
  }).catch(() => undefined)

  // NOTE: Removed `onResized` handler — it fired on every resize event
  // (including drag), accumulating IPC calls that blocked the event loop.
  // The maximize icon still updates correctly via toggleMaximize().
  let unlisten = () => {}

  // Save window state on close, then explicitly close the window.
  // In Tauri v2, registering onCloseRequested prevents the default close —
  // we must call win.close() ourselves after finishing async work.
  win.onCloseRequested(async (event) => {
    event.preventDefault()
    if (mainTab.value === 'config'
      && !(await configWorkspaceStore.confirmDiscardActiveChanges('关闭应用'))) {
      return
    }
    try {
      await invoke('save_last_active_main_tab', { tab: mainTab.value })
      await saveWindowState()
    } catch (e) {
      console.error('Failed to save window state on close:', e)
    }
    unlisten()
    await win.destroy()
  })

  await runDependencyCheck()
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeyDown)
  document.removeEventListener('click', onDocumentClick)
  unlistenClaudeHistory?.()
  saveWindowState()
})
</script>

<style scoped>
.app-layout {
  display: flex;
  flex-direction: column;
  height: 100%;
  background-color: var(--bg);
  position: relative;
}

.app-nav {
  display: none;
}

.title-bar {
  flex-shrink: 0;
  height: 38px;
  display: flex;
  align-items: center;
  justify-content: flex-start;
  gap: 6px;
  padding: 0 8px;
  background-color: var(--card);
  border-bottom: 1px solid var(--separator);
  user-select: none;
}

.title-bar__left {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
}

.title-bar__left:empty {
  display: none;
}

.title-bar__tabs {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  justify-content: flex-start;
  gap: 4px;
}

.title-bar__tab {
  height: 28px;
  padding: 0 14px;
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  font-family: var(--font-base);
  font-size: var(--font-size-base);
  cursor: pointer;
  transition: background-color 0.12s ease, color 0.12s ease;
}

.title-bar__tab:hover {
  background-color: var(--tab-bg);
  color: var(--text-primary);
}

.title-bar__tab.active {
  background-color: var(--primary);
  color: #fff;
  font-weight: 600;
}

.title-bar__icon-btn {
  width: 28px;
  height: 28px;
  display: grid;
  place-items: center;
  border: 1px solid var(--separator);
  border-radius: var(--radius-sm);
  background: var(--bg);
  color: var(--text-secondary);
  font-size: 14px;
  cursor: pointer;
}

.title-bar__icon-btn:hover {
  background-color: var(--tab-bg);
  color: var(--text-primary);
}

.title-bar__icon-btn:disabled,
.title-bar__tab:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.title-bar__controls {
  flex: 0 0 auto;
  margin-left: auto;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 4px;
}

.title-bar__control {
  width: 28px;
  height: 28px;
  display: grid;
  place-items: center;
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background-color 0.12s ease, color 0.12s ease;
}

.title-bar__window-icon {
  position: relative;
  width: 14px;
  height: 14px;
  display: block;
  color: currentColor;
}

.title-bar__window-icon--minimize::before {
  content: '';
  position: absolute;
  left: 1px;
  right: 1px;
  top: 7px;
  height: 1.5px;
  border-radius: 999px;
  background: currentColor;
}

.title-bar__window-icon--maximize::before {
  content: '';
  position: absolute;
  inset: 2px;
  border: 1.5px solid currentColor;
  border-radius: 1px;
}

.title-bar__window-icon--restore::before,
.title-bar__window-icon--restore::after {
  content: '';
  position: absolute;
  width: 9px;
  height: 9px;
  border: 1.5px solid currentColor;
  border-radius: 1px;
}

.title-bar__window-icon--restore::before {
  right: 1px;
  top: 1px;
}

.title-bar__window-icon--restore::after {
  left: 1px;
  bottom: 1px;
}

.title-bar__window-icon--close::before,
.title-bar__window-icon--close::after {
  content: '';
  position: absolute;
  left: 1px;
  right: 1px;
  top: 6px;
  height: 1.5px;
  border-radius: 999px;
  background: currentColor;
}

.title-bar__window-icon--close::before {
  transform: rotate(45deg);
}

.title-bar__window-icon--close::after {
  transform: rotate(-45deg);
}

.title-bar__control:hover {
  background-color: var(--tab-bg);
  color: var(--text-primary);
}

.title-bar__control--close:hover {
  background-color: var(--danger);
  color: #fff;
}

.settings-popover {
  position: absolute;
  left: 8px;
  bottom: 34px;
  min-width: 140px;
  background-color: var(--card);
  border-radius: var(--radius);
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.15), 0 0 0 1px var(--separator);
  padding: 6px;
  z-index: 1000;
  animation: dropdown-in 0.12s ease;
}

[data-theme="dark"] .settings-popover {
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4), 0 0 0 1px var(--separator);
}

@keyframes dropdown-in {
  from {
    opacity: 0;
    transform: translateY(-4px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}

.settings-dropdown__section {
  padding: 4px 8px;
  font-size: var(--font-size-small);
  font-weight: 600;
  color: var(--text-secondary);
  user-select: none;
}

.settings-dropdown__item {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 5px 8px;
  font-family: var(--font-base);
  font-size: var(--font-size-base);
  color: var(--text-primary);
  background-color: transparent;
  border: none;
  border-radius: var(--radius-sm);
  cursor: pointer;
  transition: background-color 0.12s ease;
  text-align: left;
}

.settings-dropdown__item:hover {
  background-color: var(--tab-bg);
}

.settings-dropdown__item.active {
  color: var(--primary);
}

.settings-dropdown__check {
  display: inline-block;
  width: 16px;
  text-align: center;
  font-weight: 600;
}

.font-size-row {
  justify-content: space-between;
}

.font-size-btn {
  width: 24px;
  height: 24px;
  display: grid;
  place-items: center;
  border: 1px solid var(--separator);
  border-radius: var(--radius-sm);
  background: var(--bg);
  color: var(--text-primary);
  font-size: var(--font-size-base);
  cursor: pointer;
  transition: background-color 0.12s ease;
}

.font-size-btn:hover:not(:disabled) {
  background-color: var(--tab-bg);
}

.font-size-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.font-size-value {
  min-width: 24px;
  text-align: center;
  font-weight: 600;
}

.app-content {
  flex: 1;
  overflow: hidden;
  position: relative;
}

.app-panel {
  position: absolute;
  inset: 0;
  overflow: auto;
}

.dependency-gate {
  position: absolute;
  top: 38px;
  right: 0;
  bottom: 0;
  left: 0;
  z-index: 2000;
  display: grid;
  place-items: center;
  padding: 28px;
  background:
    radial-gradient(circle at top, color-mix(in srgb, var(--primary) 12%, transparent), transparent 44%),
    var(--bg);
}

.dependency-gate__card {
  width: min(100%, 560px);
  padding: 36px;
  border: 1px solid var(--separator);
  border-radius: var(--radius);
  background-color: var(--card);
  box-shadow: 0 16px 44px rgba(0, 0, 0, 0.18);
  text-align: center;
}

.dependency-gate__icon {
  display: grid;
  width: 58px;
  height: 58px;
  margin: 0 auto 16px;
  place-items: center;
  border-radius: 50%;
  background-color: var(--tab-bg);
  color: var(--primary);
  font-size: 28px;
}

.dependency-gate h1 {
  margin: 0 0 12px;
  color: var(--text-primary);
  font-size: 22px;
}

.dependency-gate__description,
.dependency-gate__detail,
.dependency-gate__feedback,
.dependency-gate__hint {
  margin: 0;
  color: var(--text-secondary);
  font-size: var(--font-size-base);
  line-height: 1.65;
}

.dependency-gate__detail,
.dependency-gate__feedback {
  margin-top: 8px;
}

.dependency-gate__feedback {
  color: var(--primary);
}

.dependency-gate__actions,
.dependency-gate__progress {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: center;
  gap: 10px;
  margin-top: 24px;
}

.dependency-gate__progress {
  color: var(--text-secondary);
}

.dependency-gate__button {
  min-height: 34px;
  padding: 0 14px;
  border: 1px solid transparent;
  border-radius: var(--radius-sm);
  font-family: var(--font-base);
  font-size: var(--font-size-base);
  cursor: pointer;
}

.dependency-gate__button--primary {
  border-color: var(--primary);
  background-color: var(--primary);
  color: #fff;
}

.dependency-gate__button--secondary {
  border-color: var(--separator);
  background-color: var(--bg);
  color: var(--text-primary);
}

.dependency-gate__button--link {
  width: 100%;
  border-color: transparent;
  background: transparent;
  color: var(--primary);
}

.dependency-gate__button:hover {
  filter: brightness(1.06);
}

.installing-dots {
  display: inline-flex;
  width: 1.2em;
  margin-left: 1px;
  justify-content: flex-start;
}

.installing-dots span {
  animation: installing-dot-pulse 1.2s infinite ease-in-out;
  opacity: 0.25;
}

.installing-dots span:nth-child(2) {
  animation-delay: 0.15s;
}

.installing-dots span:nth-child(3) {
  animation-delay: 0.3s;
}

@keyframes installing-dot-pulse {
  0%,
  75%,
  100% {
    opacity: 0.25;
  }
  35% {
    opacity: 1;
  }
}

.dependency-gate__hint {
  margin-top: 20px;
  font-size: var(--font-size-small);
}

.dependency-gate__spinner {
  width: 16px;
  height: 16px;
  border: 2px solid var(--separator);
  border-top-color: var(--primary);
  border-radius: 50%;
  animation: dependency-spin 0.8s linear infinite;
}

@keyframes dependency-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
