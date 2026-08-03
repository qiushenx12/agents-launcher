<template>
  <div
    class="app-layout"
    :class="{
      'app-layout--mac-title-bar': usesNativeMacTitleBar,
      'app-layout--mac-fullscreen': usesNativeMacTitleBar && isMacFullscreen,
    }"
  >
    <!-- Custom title bar -->
    <header
      class="title-bar"
      :style="{
        '--project-nav-width': `${leftSidebarOpen ? sharedSidebarHeaderWidth : 45}px`,
      }"
      @mousedown="startTitleBarDrag"
      @dblclick="handleTitleBarDoubleClick"
    >
      <div
        class="title-bar__workspace-section"
        :class="{ 'title-bar__workspace-section--collapsed': !leftSidebarOpen }"
      >
        <span class="title-bar__sidebar-slot">
          <button
            class="title-bar__icon-btn"
            :class="{ active: leftSidebarOpen }"
            :title="leftSidebarToggleTitle"
            :aria-label="leftSidebarToggleTitle"
            :aria-pressed="leftSidebarOpen"
            :disabled="!appReady"
            data-tauri-drag-region="false"
            @click="toggleActiveLeftSidebar"
          >
            <span class="sidebar-toggle-icon sidebar-toggle-icon--left" aria-hidden="true"></span>
          </button>
        </span>
        <nav v-if="leftSidebarOpen" class="title-bar__mode-tabs" aria-label="工作区">
          <button
            class="title-bar__mode-tab"
            :class="{ active: workspaceMode === 'config' }"
            :disabled="!appReady"
            data-tauri-drag-region="false"
            @click="openConfigTab"
          >
            配置
            <span
              v-if="configWorkspaceStore.activeHasUnsavedChanges"
              class="title-bar__dirty"
              title="当前配置有未保存的修改"
              aria-label="当前配置有未保存的修改"
            ></span>
          </button>
          <button
            class="title-bar__mode-tab"
            :class="{ active: workspaceMode === 'project' }"
            :disabled="!appReady"
            data-tauri-drag-region="false"
            @click="openProjectTab"
          >
            项目
          </button>
        </nav>
      </div>

      <nav class="title-bar__tabs" aria-label="前端">
        <button
          v-for="kind in topBarStore.cliOrder"
          :key="kind"
          class="title-bar__tab"
          :class="{ active: activeCliKind === kind }"
          :disabled="!appReady"
          data-tauri-drag-region="false"
          @click="selectCliKind(kind)"
        >
          {{ CLI_DESCRIPTORS[kind].label }}
        </button>
      </nav>

      <div v-if="!usesNativeMacTitleBar" class="title-bar__controls" @dblclick.stop>
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
      <div v-show="workspaceMode === 'config'" class="app-panel">
        <ConfigWorkspace
          :sidebar-collapsed="!leftSidebarOpen"
          @left-width-change="sharedSidebarHeaderWidth = $event + 5"
        />
      </div>

      <!-- Terminal panel — always mounted to preserve state -->
      <div v-show="mainTab === 'terminal'" class="app-panel">
        <TerminalManager ref="terminalManagerRef" :launch-dir="activeLaunchDir" />
      </div>

      <!-- Shared CLI workspace — keep mounted while on the config tab so
           xterm instances (scrollback, mouse modes) survive tab switches. -->
      <div
        v-if="workspaceCliKind && workspaceCliStatus?.state === 'ready'"
        v-show="workspaceMode === 'project'"
        class="app-panel"
      >
        <ProjectPanel
          ref="projectPanelRef"
          :cli-kind="workspaceCliKind"
          @open-settings="toggleSettings($event)"
          @left-width-change="sharedSidebarHeaderWidth = $event + 5"
        />
      </div>

      <!-- Orchestration panel -->
      <div v-show="mainTab === 'orchestration'" class="app-panel">
        <OrchestrationManager />
      </div>
    </main>

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

    <div
      v-if="appReady && showSettings"
      class="settings-popover"
      :style="settingsPopoverStyle"
    >
      <div ref="settingsMenuRef" class="settings-dropdown__menu">
        <button class="settings-dropdown__group-trigger" :class="{ 'is-open': activeSettingsSubmenu === 'theme' }" type="button" :aria-expanded="activeSettingsSubmenu === 'theme'" @click="toggleSettingsSubmenu('theme', $event)">
          <span>主题</span>
          <span class="settings-dropdown__group-value">{{ theme === 'light' ? '浅色' : '深色' }}</span>
          <svg aria-hidden="true" focusable="false" viewBox="0 0 24 24"><path d="m7 9 5 5 5-5" /></svg>
        </button>
        <button class="settings-dropdown__group-trigger" :class="{ 'is-open': activeSettingsSubmenu === 'font-size' }" type="button" :aria-expanded="activeSettingsSubmenu === 'font-size'" @click="toggleSettingsSubmenu('font-size', $event)">
          <span>字体大小</span>
          <span class="settings-dropdown__group-value">终端 {{ terminalStore.fontSize }} · APP {{ appFontSize }}px</span>
          <svg aria-hidden="true" focusable="false" viewBox="0 0 24 24"><path d="m7 9 5 5 5-5" /></svg>
        </button>
        <button class="settings-dropdown__group-trigger" type="button" @click="openTopBarOrderModal">
          <span>前端入口排序</span>
          <span class="settings-dropdown__group-value">界面布局</span>
          <span class="settings-dropdown__chevron">›</span>
        </button>
        <button class="settings-dropdown__group-trigger" :class="{ 'is-open': activeSettingsSubmenu === 'project-drop-path' }" type="button" :aria-expanded="activeSettingsSubmenu === 'project-drop-path'" @click="toggleSettingsSubmenu('project-drop-path', $event)">
          <span>项目终端拖入文件</span>
          <span class="settings-dropdown__group-value">{{ claudeStore.projectDropPathMode === 'relative' ? '相对路径' : '仅文件名' }}</span>
          <svg aria-hidden="true" focusable="false" viewBox="0 0 24 24"><path d="m7 9 5 5 5-5" /></svg>
        </button>
        <button v-if="showClaudeSettings" class="settings-dropdown__group-trigger" :class="{ 'is-open': activeSettingsSubmenu === 'busy-input' }" type="button" :aria-expanded="activeSettingsSubmenu === 'busy-input'" @click="toggleSettingsSubmenu('busy-input', $event)">
          <span>Claude 运行中消息</span>
          <span class="settings-dropdown__group-value">{{ claudeObserverStore.busyInputMode === 'native' ? '执行间隙插入' : '完全停止后发送' }}</span>
          <svg aria-hidden="true" focusable="false" viewBox="0 0 24 24"><path d="m7 9 5 5 5-5" /></svg>
        </button>
        <button
          v-if="showClaudeViewSettings"
          class="settings-dropdown__group-trigger"
          :class="{ 'is-open': activeSettingsSubmenu === 'claude-view' }"
          type="button"
          :aria-expanded="activeSettingsSubmenu === 'claude-view'"
          @click="toggleSettingsSubmenu('claude-view', $event)"
        >
          <span>Claude 界面</span>
          <span class="settings-dropdown__group-value">{{ activeClaudeViewLabel }}</span>
          <svg aria-hidden="true" focusable="false" viewBox="0 0 24 24"><path d="m7 9 5 5 5-5" /></svg>
        </button>
        <button
          v-if="showClaudeSettings"
          class="settings-dropdown__group-trigger"
          type="button"
          role="switch"
          :aria-checked="claudeViewModeStore.logOutputEnabled"
          @click="toggleClaudeLogOutput"
        >
          <span>Claude 日志输出</span>
          <span class="settings-dropdown__group-value">
            {{ claudeViewModeStore.logOutputEnabled ? '已开启' : '已关闭' }}
          </span>
          <span
            class="settings-dropdown__switch"
            :class="{ 'is-on': claudeViewModeStore.logOutputEnabled }"
            aria-hidden="true"
          >
            <span />
          </span>
        </button>
      </div>

      <div
        v-if="activeSettingsSubmenu"
        class="settings-dropdown__submenu"
        :style="activeSettingsSubmenu === 'font-size'
          ? { bottom: `${settingsSubmenuBottom}px` }
          : { top: `${settingsSubmenuTop}px` }"
      >
        <div v-if="activeSettingsSubmenu === 'theme'" class="settings-dropdown__options" role="listbox" aria-label="主题">
          <button class="settings-dropdown__item" :class="{ active: theme === 'light' }" type="button" role="option" :aria-selected="theme === 'light'" @click="setTheme('light')"><span class="settings-dropdown__check">{{ theme === 'light' ? '✓' : '' }}</span>浅色</button>
          <button class="settings-dropdown__item" :class="{ active: theme === 'dark' }" type="button" role="option" :aria-selected="theme === 'dark'" @click="setTheme('dark')"><span class="settings-dropdown__check">{{ theme === 'dark' ? '✓' : '' }}</span>深色</button>
        </div>
        <div v-else-if="activeSettingsSubmenu === 'claude-view'" class="settings-dropdown__options" role="listbox" aria-label="Claude 界面">
          <button
            v-for="option in claudeViewOptions"
            :key="option.value"
            class="settings-dropdown__item"
            :class="{ active: activeClaudeView === option.value }"
            type="button"
            role="option"
            :aria-selected="activeClaudeView === option.value"
            @click="setClaudeView(option.value)"
          >
            <span class="settings-dropdown__check">{{ activeClaudeView === option.value ? '✓' : '' }}</span>
            <span class="settings-dropdown__item-label">{{ option.label }}</span>
            <span
              v-if="claudeViewModeStore.pendingRestartView === option.value"
              class="settings-dropdown__meta"
            >
              重启后
            </span>
          </button>
        </div>
        <div v-else-if="activeSettingsSubmenu === 'project-drop-path'" class="settings-dropdown__options" role="listbox" aria-label="项目终端拖入文件">
          <button class="settings-dropdown__item" :class="{ active: claudeStore.projectDropPathMode === 'relative' }" type="button" role="option" :aria-selected="claudeStore.projectDropPathMode === 'relative'" @click="setProjectDropPathMode('relative')"><span class="settings-dropdown__check">{{ claudeStore.projectDropPathMode === 'relative' ? '✓' : '' }}</span>相对路径</button>
          <button class="settings-dropdown__item" :class="{ active: claudeStore.projectDropPathMode === 'filename' }" type="button" role="option" :aria-selected="claudeStore.projectDropPathMode === 'filename'" @click="setProjectDropPathMode('filename')"><span class="settings-dropdown__check">{{ claudeStore.projectDropPathMode === 'filename' ? '✓' : '' }}</span>仅文件名</button>
        </div>
        <div v-else-if="activeSettingsSubmenu === 'busy-input'" class="settings-dropdown__options" role="listbox" aria-label="Claude 运行中消息">
          <button class="settings-dropdown__item settings-dropdown__item--described" :class="{ active: claudeObserverStore.busyInputMode === 'native' }" type="button" role="option" :aria-selected="claudeObserverStore.busyInputMode === 'native'" @click="setClaudeBusyInputMode('native')"><span class="settings-dropdown__check">{{ claudeObserverStore.busyInputMode === 'native' ? '✓' : '' }}</span><span class="settings-dropdown__item-copy"><span>执行间隙插入</span><small>交给 Claude Code 原生等待队列</small></span></button>
          <button class="settings-dropdown__item settings-dropdown__item--described" :class="{ active: claudeObserverStore.busyInputMode === 'after-stop' }" type="button" role="option" :aria-selected="claudeObserverStore.busyInputMode === 'after-stop'" @click="setClaudeBusyInputMode('after-stop')"><span class="settings-dropdown__check">{{ claudeObserverStore.busyInputMode === 'after-stop' ? '✓' : '' }}</span><span class="settings-dropdown__item-copy"><span>完全停止后发送</span><small>当前轮次结束后再提交</small></span></button>
        </div>
        <div v-else-if="activeSettingsSubmenu === 'font-size'" class="settings-dropdown__options settings-dropdown__options--font" aria-label="字体大小">
          <div class="settings-dropdown__font-row"><span>终端字体</span><div class="font-size-row"><button class="font-size-btn" type="button" :disabled="terminalStore.fontSize <= 6" aria-label="减小终端字体" @click="terminalStore.setFontSize(terminalStore.fontSize - 1)">−</button><span class="font-size-value">{{ terminalStore.fontSize }}</span><button class="font-size-btn" type="button" :disabled="terminalStore.fontSize >= 28" aria-label="增大终端字体" @click="terminalStore.setFontSize(terminalStore.fontSize + 1)">+</button></div></div>
          <div class="settings-dropdown__font-row"><span>APP 字体</span><div class="font-size-row"><button class="font-size-btn" type="button" :disabled="appFontSize <= APP_FONT_MIN" aria-label="减小 APP 字体" @click="setAppFontSize(-1)">−</button><span class="font-size-value">{{ appFontSize }}px</span><button class="font-size-btn" type="button" :disabled="appFontSize >= APP_FONT_MAX" aria-label="增大 APP 字体" @click="setAppFontSize(1)">+</button></div></div>
          <div class="settings-dropdown__font-row"><span>Markdown 字体</span><div class="font-size-row"><button class="font-size-btn" type="button" :disabled="mdFontSize <= MD_FONT_MIN" aria-label="减小 Markdown 字体" @click="setMdFontSize(mdFontSize - 1)">−</button><span class="font-size-value">{{ mdFontSize }}px</span><button class="font-size-btn" type="button" :disabled="mdFontSize >= MD_FONT_MAX" aria-label="增大 Markdown 字体" @click="setMdFontSize(mdFontSize + 1)">+</button></div></div>
        </div>
      </div>

    </div>

    <TopBarOrderModal
      :visible="topBarOrderModalOpen"
      @close="topBarOrderModalOpen = false"
    />
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
import TopBarOrderModal from './components/common/TopBarOrderModal.vue'
import { useClaudeStore } from './stores/claude'
import { useClaudeObserverStore } from './stores/claudeObserver'
import { useClaudeViewModeStore, type ClaudeView } from './stores/claudeViewMode'
import { useTerminalStore } from './stores/terminal'
import { useMarkdownFontSize, MD_FONT_MIN, MD_FONT_MAX } from './composables/useMarkdownFontSize'
import { useSettingsPopover } from './composables/useSettingsPopover'
import { useProjectStore } from './stores/project'
import { useCliRuntimeStore } from './stores/cliRuntime'
import { useConfigWorkspaceStore } from './stores/configWorkspace'
import { useTopBarStore } from './stores/topBar'
import { usePlatform } from './composables/usePlatform'
import {
  shouldStartTitleBarDrag,
  shouldToggleTitleBarMaximize,
} from './utils/windowTitleBar'
import {
  CLI_DESCRIPTORS,
  isCliKind,
  normalizePersistedMainTab,
  type CliKind,
  type MainTab,
} from './types/cli'
import type { ClaudeAgentEvent } from './types/claudeObserver'

type DependencyName = 'node' | 'git'
type DependencyStatus = 'installed' | 'missing' | 'unsupported' | 'error'
type DependencyGateState = 'checking' | 'missing' | 'unsupported' | 'error' | 'installing' | 'restart_required' | 'ready'
const claudeViewOptions: Array<{ value: ClaudeView; label: string }> = [
  { value: 'conversation', label: '界面' },
  { value: 'terminal', label: '终端' },
]

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
const claudeObserverStore = useClaudeObserverStore()
const claudeViewModeStore = useClaudeViewModeStore()
const terminalStore = useTerminalStore()
const projectStore = useProjectStore()
const cliRuntimeStore = useCliRuntimeStore()
const configWorkspaceStore = useConfigWorkspaceStore()
const topBarStore = useTopBarStore()
const { isWindows, isMacOS } = usePlatform()
const usesNativeMacTitleBar = computed(() => isMacOS.value)
const terminalManagerRef = ref<InstanceType<typeof TerminalManager> | null>(null)
const projectPanelRef = ref<InstanceType<typeof ProjectPanel> | null>(null)
const topBarOrderModalOpen = ref(false)
const sharedSidebarHeaderWidth = ref(285)
const dependencyState = ref<DependencyGateState>('checking')
const dependencyResult = ref<DependencyCheckResult | null>(null)
const dependencyActionMessage = ref('')
const cliInstallHelpVisible = ref(false)
const cliWorkspacePreparation = ref<{ kind: CliKind; requestId: number } | null>(null)
let readyAppInitialized = false
let cliOpenRequestId = 0
const MIN_CLI_WORKSPACE_GATE_MS = 320

const appReady = computed(() => dependencyState.value === 'ready')
const workspaceMode = computed<'config' | 'project' | 'other'>(() => {
  if (mainTab.value === 'config') return 'config'
  return isCliKind(mainTab.value) ? 'project' : 'other'
})
const activeCliKind = computed<CliKind>(() => configWorkspaceStore.activeKind)
const activeCliStatus = computed(() => cliRuntimeStore.statuses[activeCliKind.value] ?? null)
// Keep the selected frontend stable while configuration replaces the project
// workspace. This lets both modes show the same CLI without a second switcher.
const workspaceCliKind = computed<CliKind>(() => activeCliKind.value)
const workspaceCliStatus = computed(() => cliRuntimeStore.statuses[workspaceCliKind.value] ?? null)
const activeCliLabel = computed(() => CLI_DESCRIPTORS[activeCliKind.value].label)
const settingsCliKind = computed<CliKind | null>(() => {
  if (workspaceMode.value === 'config') return activeCliKind.value
  return isCliKind(mainTab.value) ? mainTab.value : null
})
const showClaudeSettings = computed(() => settingsCliKind.value === 'claude')
const showClaudeViewSettings = computed(() => showClaudeSettings.value)
const activeClaudeView = computed<ClaudeView>(() => (
  claudeViewModeStore.runtimeView
))
function claudeViewLabel(view: ClaudeView) {
  return claudeViewOptions.find(option => option.value === view)?.label ?? '界面'
}
const activeClaudeViewLabel = computed(() => {
  const current = claudeViewLabel(activeClaudeView.value)
  const pending = claudeViewModeStore.pendingRestartView
  return pending ? `${current} · 重启后${claudeViewLabel(pending)}` : current
})
const cliWorkspacePreparing = computed(() => workspaceMode.value === 'project'
  && cliWorkspacePreparation.value?.kind === activeCliKind.value)
const cliGateVisible = computed(() => appReady.value
  && workspaceMode.value === 'project'
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
  return '折叠/展开项目边栏'
})

const leftSidebarOpen = computed(() => !projectStore.leftSidebarCollapsed)

function toggleActiveLeftSidebar() {
  if (!appReady.value) return
  projectStore.toggleLeftSidebarCollapsed()
}

function openConfigTab() {
  mainTab.value = 'config'
}

async function openProjectTab() {
  if (workspaceMode.value === 'project') return
  await openCliTab(activeCliKind.value)
}

async function selectCliKind(kind: CliKind) {
  if (kind === activeCliKind.value && workspaceMode.value !== 'other') return

  if (workspaceMode.value === 'config') {
    if (!(await configWorkspaceStore.selectKind(kind))) return
    projectStore.setActiveCliKind(kind)
    await cliRuntimeStore.check(kind)
    return
  }

  await openCliTab(kind)
}

// ── Window controls ────────────────────────────────────────────────────────
const win = getCurrentWindow()
const isMaximized = ref(false)
const isMacFullscreen = ref(false)
const macFullscreenTransitioning = ref(false)
const MAC_TITLE_BAR_TRANSITION_MS = 260
let macFullscreenSyncTimer: ReturnType<typeof setTimeout> | null = null
let unlistenMacFullscreenRequest: (() => void) | undefined
let unlistenWindowResized: (() => void) | undefined

function waitForMacTitleBarTransition() {
  if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
    return Promise.resolve()
  }
  return new Promise<void>((resolve) => {
    setTimeout(resolve, MAC_TITLE_BAR_TRANSITION_MS)
  })
}

async function syncMacFullscreenState() {
  if (!usesNativeMacTitleBar.value) return
  const fullscreen = await win.isFullscreen().catch(() => isMacFullscreen.value)
  isMacFullscreen.value = fullscreen
  isMaximized.value = fullscreen
  macFullscreenTransitioning.value = false
}

function scheduleMacFullscreenSync(delay = 140) {
  if (macFullscreenSyncTimer !== null) {
    clearTimeout(macFullscreenSyncTimer)
  }
  macFullscreenSyncTimer = setTimeout(() => {
    macFullscreenSyncTimer = null
    syncMacFullscreenState().catch(() => {})
  }, delay)
}

async function requestAnimatedFullscreenToggle(knownFullscreen?: boolean) {
  if (!usesNativeMacTitleBar.value || macFullscreenTransitioning.value) return

  const fullscreen = knownFullscreen
    ?? await win.isFullscreen().catch(() => isMacFullscreen.value)
  macFullscreenTransitioning.value = true

  if (fullscreen) {
    // While the webview is still stable in fullscreen, first move all title-bar
    // content back to its traffic-light-safe position. Only then ask AppKit to
    // leave fullscreen, so the returning native buttons never overlap it.
    isMacFullscreen.value = false
    await nextTick()
    await waitForMacTitleBarTransition()
  }

  try {
    await invoke('toggle_animated_fullscreen')
    // `onResized` normally fires once the native transition completes. Keep a
    // fallback for Reduce Motion and macOS versions that omit the final event.
    scheduleMacFullscreenSync(1400)
  } catch (error) {
    macFullscreenTransitioning.value = false
    await syncMacFullscreenState()
    console.error('Failed to toggle native macOS fullscreen:', error)
  }
}

function startTitleBarDrag(event: MouseEvent) {
  if (!shouldStartTitleBarDrag(event)) return
  void win.startDragging().catch(() => {})
}

function handleTitleBarDoubleClick(event: MouseEvent) {
  if (!shouldToggleTitleBarMaximize(event)) return
  void toggleMaximize()
}

async function minimizeWindow() {
  await win.minimize().catch(() => {})
}

async function toggleMaximize() {
  if (usesNativeMacTitleBar.value) {
    await requestAnimatedFullscreenToggle()
    return
  }
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
const {
  showSettings,
  settingsAnchorLeft,
  settingsAnchorBottom,
  settingsMenuMaxHeight,
  toggleSettings,
  updateSettingsAnchor,
} = useSettingsPopover()
const settingsPopoverStyle = computed(() => ({
  left: `${settingsAnchorLeft.value}px`,
  bottom: `${settingsAnchorBottom.value}px`,
  '--settings-menu-max-height': `${settingsMenuMaxHeight.value}px`,
}))
const theme = ref<'light' | 'dark'>('light')
type SettingsSubmenu = 'theme' | 'claude-view' | 'project-drop-path' | 'busy-input' | 'font-size'
const activeSettingsSubmenu = ref<SettingsSubmenu | null>(null)
const settingsMenuRef = ref<HTMLElement | null>(null)
const settingsSubmenuTop = ref(0)
const settingsSubmenuBottom = ref(0)

function toggleSettingsSubmenu(submenu: SettingsSubmenu, event: MouseEvent) {
  if (activeSettingsSubmenu.value === submenu) {
    activeSettingsSubmenu.value = null
    return
  }

  const trigger = event.currentTarget as HTMLElement
  const menu = settingsMenuRef.value
  if (menu) {
    const menuBounds = menu.getBoundingClientRect()
    const triggerBounds = trigger.getBoundingClientRect()
    settingsSubmenuTop.value = triggerBounds.top - menuBounds.top
    settingsSubmenuBottom.value = menuBounds.bottom - triggerBounds.bottom
  } else {
    settingsSubmenuTop.value = 0
    settingsSubmenuBottom.value = 0
  }
  activeSettingsSubmenu.value = submenu
}

watch(showSettings, (visible) => {
  if (!visible) activeSettingsSubmenu.value = null
})

watch(showClaudeSettings, (visible) => {
  if (
    !visible
    && (activeSettingsSubmenu.value === 'claude-view'
      || activeSettingsSubmenu.value === 'busy-input')
  ) {
    activeSettingsSubmenu.value = null
  }
})

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

async function setClaudeView(view: ClaudeView) {
  if (!showClaudeViewSettings.value) return
  try {
    await claudeViewModeStore.save(view)
    if (!claudeViewModeStore.structuredCaptureEnabled && view !== 'terminal') {
      projectStore.statusMessage = `已设置为${claudeViewLabel(view)}界面模式；请关闭并重新启动 App 后激活相关功能。`
    } else if (projectPanelRef.value?.showClaudeViewControls === true) {
      await projectPanelRef.value.selectClaudeView(view)
    } else {
      claudeViewModeStore.setRuntimeView(view)
    }
    showSettings.value = false
  } catch (error) {
    projectStore.statusMessage = `Claude 启动视图保存失败：${String(error)}`
  }
}

async function toggleClaudeLogOutput() {
  const enabled = !claudeViewModeStore.logOutputEnabled
  try {
    await claudeViewModeStore.setLogOutputEnabled(enabled)
    projectStore.statusMessage = `Claude 日志输出已${enabled ? '开启' : '关闭'}。`
  } catch (error) {
    projectStore.statusMessage = `Claude 日志输出设置保存失败：${String(error)}`
  }
}

function setProjectDropPathMode(mode: 'filename' | 'relative') {
  claudeStore.projectDropPathMode = mode
  showSettings.value = false
}

async function setClaudeBusyInputMode(mode: 'native' | 'after-stop') {
  try {
    await claudeObserverStore.setBusyInputMode(mode)
    showSettings.value = false
  } catch (error) {
    console.error('Failed to save Claude busy input mode:', error)
  }
}

function openTopBarOrderModal() {
  showSettings.value = false
  topBarOrderModalOpen.value = true
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

function onWindowResize() {
  updateSettingsAnchor()
}

async function openCliTab(kind: CliKind, forceCheck = false) {
  if (!appReady.value || cliRuntimeStore.checking[kind]) return
  if (workspaceMode.value === 'config') {
    if (kind === activeCliKind.value) {
      if (!(await configWorkspaceStore.confirmDiscardActiveChanges(`进入 ${CLI_DESCRIPTORS[kind].label} 项目`))) {
        return
      }
    } else if (!(await configWorkspaceStore.selectKind(kind))) {
      return
    }
  } else if (kind !== activeCliKind.value && !(await configWorkspaceStore.selectKind(kind))) {
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

async function retryCliGateCheck() {
  await openCliTab(activeCliKind.value, true)
}

const activeLaunchDir = computed(() => claudeStore.launchDir)
let unlistenClaudeHistory: (() => void) | undefined
let unlistenClaudeSessionLifecycle: (() => void) | undefined
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
    if (isCliKind(tab)) {
      await configWorkspaceStore.selectKind(tab)
      mainTab.value = tab
    } else if (tab === 'config') {
      mainTab.value = 'config'
    } else if (tab === 'terminal' || tab === 'orchestration') {
      mainTab.value = 'config'
    }
    projectStore.setActiveCliKind(activeCliKind.value)
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

  if (workspaceMode.value === 'project') {
    await openCliTab(activeCliKind.value)
  }
}

onMounted(async () => {
  loadTheme()
  loadAppFontSize()
  await claudeViewModeStore.load()
  await claudeObserverStore.loadBusyInputMode()
  await topBarStore.loadOrder()
  await loadWindowState()
  await loadLastMainTab()
  if (usesNativeMacTitleBar.value) {
    isMacFullscreen.value = await win.isFullscreen().catch(() => false)
    isMaximized.value = isMacFullscreen.value
  } else {
    isMaximized.value = await win.isMaximized().catch(() => false)
  }

  window.addEventListener('keydown', onKeyDown)
  window.addEventListener('resize', onWindowResize)
  document.addEventListener('click', onDocumentClick)
  unlistenClaudeSessionLifecycle = await listen<ClaudeAgentEvent>('claude_agent_event', (event) => {
    projectStore.handleClaudeSessionLifecycleEvent(event.payload).catch((error) => {
      console.error('Failed to synchronize Claude session after /clear:', error)
    })
  }).catch(() => undefined)
  unlistenClaudeHistory = await listen('claude_history_changed', () => {
    if (!appReady.value) return
    refreshClaudeHistory().catch((error) => {
      console.error('Failed to refresh Claude history:', error)
    })
  }).catch(() => undefined)

  unlistenMacFullscreenRequest = await listen<boolean>(
    'macos-fullscreen-toggle-requested',
    ({ payload }) => {
      requestAnimatedFullscreenToggle(payload).catch(() => {})
    },
  ).catch(() => undefined)

  // Query only after resizing has settled. This tracks native fullscreen
  // completion without accumulating an IPC request for every resize frame.
  unlistenWindowResized = await win.onResized(() => {
    if (usesNativeMacTitleBar.value) {
      scheduleMacFullscreenSync()
    }
  }).catch(() => undefined)

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
    unlistenWindowResized?.()
    unlistenWindowResized = undefined
    unlistenMacFullscreenRequest?.()
    unlistenMacFullscreenRequest = undefined
    await win.destroy()
  })

  await runDependencyCheck()
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeyDown)
  window.removeEventListener('resize', onWindowResize)
  document.removeEventListener('click', onDocumentClick)
  unlistenClaudeSessionLifecycle?.()
  unlistenClaudeHistory?.()
  unlistenMacFullscreenRequest?.()
  unlistenWindowResized?.()
  if (macFullscreenSyncTimer !== null) {
    clearTimeout(macFullscreenSyncTimer)
    macFullscreenSyncTimer = null
  }
  saveWindowState()
})
</script>

<style scoped>
.app-layout {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--app-bg-gradient);
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
  padding: 0 8px 0 0;
  background: var(--card-bg-gradient);
  user-select: none;
}

.title-bar__workspace-section {
  width: var(--project-nav-width, 219px);
  flex: 0 0 var(--project-nav-width, 219px);
  align-self: stretch;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 8px;
  border-right: 1px solid var(--separator);
  background: var(--app-bg-gradient);
  background-attachment: fixed;
}

/* The native macOS title bar reserves space for the traffic lights by adding
   left padding. Compensate the workspace section width so its right divider
   stays aligned with the content sidebar divider below. Keep the collapsed
   toggle area intact because it is still visible beside the traffic lights. */
.app-layout--mac-title-bar .title-bar__workspace-section:not(.title-bar__workspace-section--collapsed) {
  width: calc(var(--project-nav-width, 219px) - 80px);
  flex-basis: calc(var(--project-nav-width, 219px) - 80px);
}

.app-layout--mac-title-bar.app-layout--mac-fullscreen .title-bar__workspace-section:not(.title-bar__workspace-section--collapsed) {
  width: calc(var(--project-nav-width, 219px) - 8px);
  flex-basis: calc(var(--project-nav-width, 219px) - 8px);
}

.app-layout--mac-title-bar .title-bar__workspace-section--collapsed {
  width: 45px;
  flex-basis: 45px;
}

.title-bar__workspace-section--collapsed {
  gap: 0;
}

.title-bar__sidebar-slot {
  width: 28px;
  height: 28px;
  flex: 0 0 28px;
}

.title-bar__mode-tabs {
  min-width: 0;
}

.app-layout--mac-title-bar .title-bar {
  position: relative;
  padding-left: 80px;
  transition: padding-left 0.26s cubic-bezier(0.4, 0, 0.2, 1);
}

/* Make the traffic-light area background consistent with the left sidebar */
.app-layout--mac-title-bar .title-bar::before {
  content: '';
  position: absolute;
  top: 0;
  left: 0;
  width: 80px;
  height: 100%;
  pointer-events: none;
  background: var(--app-bg-gradient);
  background-attachment: fixed;
}

.app-layout--mac-title-bar.app-layout--mac-fullscreen .title-bar {
  padding-left: 8px;
}

.app-layout--mac-title-bar.app-layout--mac-fullscreen .title-bar::before {
  width: 8px;
}

@media (prefers-reduced-motion: reduce) {
  .app-layout--mac-title-bar .title-bar {
    transition: none;
  }
}

.title-bar__left {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 3px;
}

.title-bar__mode-tab {
  position: relative;
  height: 28px;
  min-width: 44px;
  padding: 0 9px;
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  font-family: var(--font-base);
  font-size: var(--font-size-base);
  cursor: pointer;
}

.title-bar__mode-tab:hover {
  background-color: var(--tab-bg);
  color: var(--text-primary);
}

.title-bar__mode-tab.active {
  background-color: var(--primary);
  color: #fff;
  font-weight: 600;
}

.title-bar__dirty {
  position: absolute;
  top: 5px;
  right: 5px;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: #ff9500;
}

.title-bar__tabs {
  min-width: 0;
  flex: 1;
  align-self: stretch;
  display: flex;
  align-items: center;
  justify-content: flex-start;
  gap: 4px;
  padding-left: 8px;
  border-bottom: 1px solid var(--separator);
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

.title-bar__icon-btn.active {
  color: var(--primary);
  background: rgba(0, 122, 255, 0.08);
}

.title-bar__icon-btn:disabled,
.title-bar__mode-tab:disabled,
.title-bar__tab:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.title-bar__controls {
  flex: 0 0 auto;
  align-self: stretch;
  margin-left: auto;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 4px;
  border-bottom: 1px solid var(--separator);
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
  position: fixed;
  z-index: 1000;
  animation: dropdown-in 0.12s ease;
}

.settings-dropdown__menu,
.settings-dropdown__submenu {
  background-color: var(--card);
  border: 1px solid var(--input-border);
  border-radius: 10px;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.15);
}

[data-theme="dark"] .settings-dropdown__menu,
[data-theme="dark"] .settings-dropdown__submenu {
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4), 0 0 0 1px var(--separator);
}

.settings-dropdown__menu {
  display: flex;
  width: 270px;
  max-height: var(--settings-menu-max-height, calc(100vh - 44px));
  flex-direction: column;
  padding: 6px;
  overflow-y: auto;
}

.settings-dropdown__submenu {
  position: absolute;
  left: calc(100% + 7px);
  display: flex;
  width: max-content;
  min-width: 230px;
  max-width: min(360px, calc(100vw - 301px));
  max-height: min(420px, calc(100vh - 44px));
  flex-direction: column;
  padding: 5px;
  overflow-y: auto;
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

.settings-dropdown__group-trigger {
  display: flex;
  width: 100%;
  min-height: 34px;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  border: 0;
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  background: transparent;
  font-family: var(--font-base);
  font-size: var(--font-size-base);
  text-align: left;
  cursor: pointer;
  transition: background-color 0.12s ease;
}

.settings-dropdown__group-trigger:hover,
.settings-dropdown__group-trigger.is-open {
  background-color: var(--tab-bg);
}

.settings-dropdown__group-value {
  flex: 1;
  min-width: 0;
  margin-left: auto;
  overflow: hidden;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  text-align: right;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.settings-dropdown__switch {
  display: flex;
  width: 28px;
  height: 16px;
  flex: 0 0 auto;
  align-items: center;
  padding: 2px;
  border-radius: 999px;
  background: var(--separator);
  transition: background-color 0.15s ease;
}

.settings-dropdown__switch > span {
  width: 12px;
  height: 12px;
  border-radius: 50%;
  background: var(--app-bg-gradient);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.25);
  transition: transform 0.15s ease;
}

.settings-dropdown__switch.is-on {
  background: var(--primary);
}

.settings-dropdown__switch.is-on > span {
  transform: translateX(12px);
}

.settings-dropdown__options {
  display: flex;
  flex-direction: column;
  gap: 1px;
  width: 100%;
}

.settings-dropdown__options--font {
  gap: 3px;
}

.settings-dropdown__item:hover {
  background-color: var(--tab-bg);
}

.settings-dropdown__item.active {
  color: var(--primary);
}

.settings-dropdown__meta {
  margin-left: auto;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  white-space: nowrap;
}

.settings-dropdown__check {
  display: inline-block;
  width: 16px;
  text-align: center;
  font-weight: 600;
}

.settings-dropdown__item-label {
  flex: 1;
  min-width: 0;
}

.settings-dropdown__item--described {
  align-items: flex-start;
}

.settings-dropdown__item-copy {
  display: grid;
  min-width: 0;
  gap: 1px;
}

.settings-dropdown__item-copy small {
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  font-weight: 400;
}

.settings-dropdown__chevron {
  flex: 0 0 auto;
  color: var(--text-secondary);
  font-size: 18px;
  line-height: 1;
}

.settings-dropdown__group-trigger svg {
  flex: 0 0 13px;
  width: 13px;
  height: 13px;
  fill: none;
  stroke: currentColor;
  stroke-linecap: round;
  stroke-linejoin: round;
  stroke-width: 1.8;
  transition: transform 140ms ease;
}

.settings-dropdown__group-trigger.is-open svg {
  transform: rotate(180deg);
}

.settings-dropdown__font-row {
  display: flex;
  min-height: 30px;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 4px 8px;
  color: var(--text-primary);
  font-size: var(--font-size-base);
}

.font-size-row {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 6px;
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
  background: var(--app-bg-gradient);
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
