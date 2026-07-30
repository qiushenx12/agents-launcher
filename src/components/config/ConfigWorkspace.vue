<template>
  <div class="config-workspace">
    <div class="config-workspace__body">
      <CliClaudePanel
        v-show="workspaceStore.activeKind === 'claude'"
        :sidebar-collapsed="sidebarCollapsed"
        @left-width-change="emit('left-width-change', $event)"
      />
      <CliCodexPanel
        v-show="workspaceStore.activeKind === 'codex'"
        :sidebar-collapsed="sidebarCollapsed"
        @left-width-change="emit('left-width-change', $event)"
      />
      <CliOpencodePanel
        v-show="workspaceStore.activeKind === 'opencode'"
        :sidebar-collapsed="sidebarCollapsed"
        @left-width-change="emit('left-width-change', $event)"
      />
    </div>

    <Teleport to="body">
      <div
        v-if="workspaceStore.preflightVisible"
        class="preflight-dialog"
        role="presentation"
        @click.self="workspaceStore.closePreflight()"
      >
        <section
          class="preflight-dialog__card"
          role="dialog"
          aria-modal="true"
          aria-labelledby="preflight-dialog-title"
        >
          <header class="preflight-dialog__header">
            <div>
              <h2 id="preflight-dialog-title">{{ activeDescriptor.label }} 启动前检测</h2>
              <span>{{ activeDescriptor.configFormat.toUpperCase() }} 配置</span>
            </div>
            <button
              type="button"
              class="preflight-dialog__close"
              title="关闭"
              aria-label="关闭启动前检测"
              @click="workspaceStore.closePreflight()"
            >
              ×
            </button>
          </header>

          <ConfigStatusBanner
            :message="activeStatus?.message ?? `尚未检测 ${activeDescriptor.label}`"
            :tone="statusTone"
          />

          <div class="preflight-dialog__source">
            <span>配置来源</span>
            <p>{{ sourceDescription }}</p>
          </div>

          <div class="preflight-dialog__diagnostic">
            <span>诊断结果（已脱敏）</span>
            <pre>{{ safeDiagnostic }}</pre>
          </div>

          <footer class="preflight-dialog__actions">
            <button
              type="button"
              class="btn btn-secondary"
              :disabled="runtimeStore.checking[workspaceStore.activeKind]"
              @click="checkActive(true)"
            >
              {{ runtimeStore.checking[workspaceStore.activeKind] ? '检测中…' : '重新检测' }}
            </button>
            <button type="button" class="btn btn-primary" @click="workspaceStore.closePreflight()">
              完成
            </button>
          </footer>
        </section>
      </div>
    </Teleport>
  </div>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, watch } from 'vue'
import { CLI_DESCRIPTORS } from '@/types/cli'
import { useClaudeStore } from '@/stores/claude'
import { useCodexConfigStore } from '@/stores/codexConfig'
import { useOpencodeConfigStore } from '@/stores/opencodeConfig'
import { useCliRuntimeStore } from '@/stores/cliRuntime'
import { useConfigWorkspaceStore } from '@/stores/configWorkspace'
import { redactConfigRecord } from '@/utils/configSecurity'
import CliClaudePanel from '@/components/cli/CliClaudePanel.vue'
import CliCodexPanel from '@/components/cli/CliCodexPanel.vue'
import CliOpencodePanel from '@/components/cli/CliOpencodePanel.vue'
import { usePlatform } from '@/composables/usePlatform'
import ConfigStatusBanner from './ConfigStatusBanner.vue'

defineProps<{
  sidebarCollapsed?: boolean
}>()
const emit = defineEmits<{
  (event: 'left-width-change', width: number): void
}>()

const workspaceStore = useConfigWorkspaceStore()
const runtimeStore = useCliRuntimeStore()
const claudeStore = useClaudeStore()
const codexStore = useCodexConfigStore()
const opencodeStore = useOpencodeConfigStore()
const { isMacOS } = usePlatform()
const unregisterClaudeGuard = workspaceStore.registerDraftGuard('claude', {
  isDirty: () => claudeStore.isConfigDirty,
  discard: () => claudeStore.discardConfigChanges(),
})
const unregisterCodexGuard = workspaceStore.registerDraftGuard('codex', {
  isDirty: () => codexStore.isDirty,
  discard: () => codexStore.discardChanges(),
})
const unregisterOpencodeGuard = workspaceStore.registerDraftGuard('opencode', {
  isDirty: () => opencodeStore.isDirty,
  discard: () => opencodeStore.discardChanges(),
})
const activeDescriptor = computed(() => CLI_DESCRIPTORS[workspaceStore.activeKind])
const activeStatus = computed(() => runtimeStore.statuses[workspaceStore.activeKind])
const statusTone = computed<'info' | 'success' | 'warning' | 'error'>(() => {
  if (activeStatus.value?.state === 'ready') return 'success'
  if (activeStatus.value?.state === 'degraded') return 'warning'
  if (activeStatus.value?.state === 'blocked') return 'error'
  return 'info'
})

const sourceDescription = computed(() => {
  if (workspaceStore.activeKind === 'claude') {
    const settingsSource = claudeStore.settingsSourcePath || '~/.claude/settings.json'
    const compatibility = claudeStore.settingsUsingLegacyPath ? '（当前从历史路径兼容读取）' : ''
    const launcherPath = isMacOS.value
      ? '~/Library/Application Support/ClaudeEnvManager/env_configs.json'
      : '%APPDATA%\\ClaudeEnvManager\\env_configs.json'
    return `启动器方案：${launcherPath}；Claude 设置：${settingsSource}${compatibility}。`
  }
  if (workspaceStore.activeKind === 'codex') {
    const launcherPath = isMacOS.value
      ? '~/Library/Application Support/ClaudeEnvManager/codex/profiles.json'
      : '%APPDATA%\\ClaudeEnvManager\\codex\\profiles.json'
    return `全局配置：${codexStore.globalConfigPath || '~/.codex/config.toml'}（仅显式勾选时同步）；启动器方案：${codexStore.profilesPath || launcherPath}；auth.json 只读。`
  }
  return `唯一配置来源：${opencodeStore.globalConfigPath || '~/.config/opencode/opencode.jsonc'}；界面直接读取和保存该文件，只管理其中带 npm 的自定义 Provider，内置 Provider 保持不变。`
})

const appliedCodexProfile = computed(() => codexStore.activeProfile)
const globalCodexProfile = computed(() => (
  codexStore.profiles.find(profile => profile.id === codexStore.globalProfileId) ?? null
))

const safeDiagnostic = computed(() => JSON.stringify(redactConfigRecord({
  cliKind: workspaceStore.activeKind,
  profile: workspaceStore.activeKind === 'claude'
    ? claudeStore.activeProfileRef
    : workspaceStore.activeKind === 'codex'
      ? codexStore.activeProfileRef
      : null,
  runtime: {
    state: activeStatus.value?.state ?? 'not_checked',
    issueCode: activeStatus.value?.issueCode ?? null,
    version: activeStatus.value?.version ?? null,
    executablePath: activeStatus.value?.executablePath ?? null,
  },
  configFormat: activeDescriptor.value.configFormat,
  source: sourceDescription.value,
  codex: workspaceStore.activeKind === 'codex' ? {
    platform: codexStore.platform,
    secretStorageKind: codexStore.secretStorageKind,
    customGlobalSyncSupported: codexStore.customGlobalSyncSupported,
    customGlobalKeySyncSupported: codexStore.customGlobalKeySyncSupported,
    profileName: appliedCodexProfile.value?.name || null,
    managedProfileName: appliedCodexProfile.value?.managedProfileName || null,
    globalProfileId: codexStore.globalProfileId,
    globalProfileInSync: codexStore.globalProfileInSync,
    globalProfileName: globalCodexProfile.value?.name || null,
    authMode: appliedCodexProfile.value?.authMode || null,
    model: appliedCodexProfile.value?.model || null,
    reasoningEffort: appliedCodexProfile.value?.reasoningEffort || null,
    modelProvider: appliedCodexProfile.value?.authMode === 'official'
      ? 'openai'
      : appliedCodexProfile.value?.providerId || null,
    baseUrl: appliedCodexProfile.value?.authMode === 'official'
      ? appliedCodexProfile.value?.openaiBaseUrl || null
      : appliedCodexProfile.value?.baseUrl || null,
    wireApi: appliedCodexProfile.value?.authMode === 'custom'
      ? appliedCodexProfile.value?.wireApi
      : null,
    envKey: appliedCodexProfile.value?.authMode === 'custom'
      ? appliedCodexProfile.value?.envKey
      : null,
    storedApiKey: appliedCodexProfile.value?.hasStoredApiKey ?? false,
    globalConfigError: codexStore.globalConfigError,
    authStatus: {
      mode: codexStore.authStatus.mode,
      hasCredentials: codexStore.authStatus.hasCredentials,
      error: codexStore.authStatus.error,
    },
    precedence: 'CLI 参数 > 项目 .codex/config.toml > 启动器 --profile > 全局 ~/.codex/config.toml',
  } : null,
  opencode: workspaceStore.activeKind === 'opencode' ? {
    configPath: opencodeStore.globalConfigPath,
    providerIds: opencodeStore.editingConfig.providers.map(provider => provider.id),
    model: opencodeStore.editingConfig.model || null,
    smallModel: opencodeStore.editingConfig.smallModel || null,
    precedence: '全局 opencode.jsonc → 项目 opencode.json(c) → .opencode → 管理员配置',
    resolvedConfig: opencodeStore.resolvedPreview,
    previewError: opencodeStore.previewError || null,
    lastLaunchContext: opencodeStore.lastLaunchContext,
  } : null,
}), null, 2))

async function checkActive(force = false) {
  await runtimeStore.check(workspaceStore.activeKind, force)
  if (workspaceStore.activeKind === 'opencode') {
    await opencodeStore.ensureLoaded().catch(() => {})
    await opencodeStore.previewCurrent().catch(() => {})
  }
}

watch(() => workspaceStore.preflightVisible, (visible) => {
  if (visible) checkActive(true).catch(() => {})
})

onBeforeUnmount(() => {
  unregisterClaudeGuard()
  unregisterCodexGuard()
  unregisterOpencodeGuard()
  workspaceStore.closePreflight()
})
</script>

<style scoped>
.config-workspace {
  height: 100%;
  display: flex;
  flex-direction: column;
  min-height: 0;
  background: var(--app-bg-gradient);
}

.config-workspace__body {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}

.config-workspace__body > * {
  height: 100%;
}

.preflight-dialog {
  position: fixed;
  inset: 0;
  z-index: 1200;
  padding: 24px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.38);
  backdrop-filter: blur(2px);
}

.preflight-dialog__card {
  width: min(680px, 100%);
  max-height: min(720px, calc(100vh - 48px));
  padding: 18px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  overflow-y: auto;
  border: 1px solid var(--separator);
  border-radius: 12px;
  background: var(--bg);
  box-shadow: 0 18px 60px rgba(0, 0, 0, 0.28);
}

.preflight-dialog__header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}

.preflight-dialog__header h2 {
  margin: 0 0 4px;
  font-size: 17px;
}

.preflight-dialog__header span,
.preflight-dialog__source span,
.preflight-dialog__diagnostic span {
  color: var(--text-secondary);
  font-size: var(--font-size-small);
}

.preflight-dialog__close {
  width: 30px;
  height: 30px;
  flex: 0 0 auto;
  border: 0;
  border-radius: 50%;
  color: var(--text-secondary);
  background: var(--tab-bg);
  cursor: pointer;
  font-size: 20px;
  line-height: 1;
}

.preflight-dialog__close:hover {
  color: var(--text-primary);
}

.preflight-dialog__source,
.preflight-dialog__diagnostic {
  padding: 10px 12px;
  border: 1px solid var(--separator);
  border-radius: var(--radius-sm);
  background: var(--card);
}

.preflight-dialog__source p {
  margin-top: 4px;
  color: var(--text-secondary);
  font-size: var(--font-size-small);
  line-height: 1.55;
  overflow-wrap: anywhere;
}

.preflight-dialog__diagnostic pre {
  max-height: 260px;
  margin-top: 7px;
  overflow: auto;
  color: var(--text-primary);
  font-family: var(--font-mono);
  font-size: var(--font-size-small);
  line-height: 1.5;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.preflight-dialog__actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
</style>
