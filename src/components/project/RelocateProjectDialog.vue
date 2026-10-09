<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import type { PathKind } from '@/types/config'
import { useProjectStore } from '@/stores/project'

/*
 * 「重新指向新路径」弹窗：项目文件夹被移动/改名后，把应用内记录和 Claude
 * Code 原生数据（~/.claude/projects 转写、history.jsonl、.claude.json）一起
 * 迁到新目录。数据链路见 stores/project.ts 的 relocateProject 注释。
 */
const props = defineProps<{
  projectId: string
}>()

const emit = defineEmits<{
  (event: 'close'): void
}>()

const store = useProjectStore()

const project = computed(
  () => store.projects.find((item) => item.id === props.projectId) ?? null,
)

const newPath = ref('')
const newPathKind = ref<PathKind | 'checking' | 'error'>('checking')
const submitting = ref(false)
const errorMessage = ref('')

watch(project, (value) => {
  // 默认把旧路径的父目录填进输入框，方便原地改名的场景直接编辑。
  if (value && !newPath.value) {
    newPath.value = value.path
  }
}, { immediate: true })

let inspectTimer: ReturnType<typeof setTimeout> | null = null
watch(newPath, (path) => {
  if (inspectTimer) clearTimeout(inspectTimer)
  const value = path.trim()
  if (!value) {
    newPathKind.value = 'missing'
    return
  }
  newPathKind.value = 'checking'
  inspectTimer = setTimeout(async () => {
    try {
      newPathKind.value = await invoke<PathKind>('path_kind', { path: value })
    } catch {
      newPathKind.value = 'error'
    }
  }, 300)
})

const sameAsOld = computed(() => {
  if (!project.value) return false
  const trim = (p: string) => p.trim().replace(/[\\/]+$/, '').replace(/\//g, '\\').toLowerCase()
  return trim(newPath.value) === trim(project.value.path)
})

const canSubmit = computed(() =>
  !!project.value
  && newPathKind.value === 'directory'
  && !sameAsOld.value
  && !submitting.value,
)

const statusText = computed(() => {
  switch (newPathKind.value) {
    case 'checking': return '检查路径中…'
    case 'directory': return sameAsOld.value ? '新路径与当前路径相同' : '目录存在，可以使用'
    case 'file': return '这是一个文件，不是目录'
    case 'missing': return '路径不存在（请先创建或移动好文件夹）'
    // macOS TCC 未授权 ~/Desktop、~/Documents 等时目录是在的，只是读不到，
    // 必须和「不存在」区分开，否则用户会以为自己填错了路径。
    case 'denied': return '没有访问权限（路径存在，但系统拒绝本应用读取该目录）'
    // Windows：U 盘拔出 / 网络驱动器断开。目录没被删，重连即可，不该迁移。
    case 'unreachable': return '盘符或网络位置当前不可达（请接回后重试，不要迁移）'
    case 'error': return '无法识别该路径'
    default: return ''
  }
})

async function pickDirectory() {
  const selected = await open({
    directory: true,
    multiple: false,
    title: '选择项目的新目录',
  })
  if (typeof selected === 'string') {
    newPath.value = selected
  }
}

async function submit() {
  if (!project.value || !canSubmit.value) return
  submitting.value = true
  errorMessage.value = ''
  try {
    // relocateProject 在原生迁移失败时抛错（本地记录保持不动）；成功时
    // 非致命提示由 store 的 statusMessage 展示，弹窗直接关闭。
    await store.relocateProject(project.value.id, newPath.value.trim())
    emit('close')
  } catch (error) {
    errorMessage.value = `迁移失败：${String(error)}`
  } finally {
    submitting.value = false
  }
}
</script>

<template>
  <div class="modal-overlay" @click.self="emit('close')">
    <div class="relocate">
      <div class="relocate__header">
        <h3>重新指向新路径</h3>
        <button class="relocate__close" @click="emit('close')">&times;</button>
      </div>

      <div class="relocate__body">
        <p v-if="project" class="relocate__intro">
          项目「{{ project.name }}」当前记录的路径是
          <code>{{ project.path }}</code>。
          选择或输入文件夹的新位置后，会同时更新应用内的项目记录和 Claude Code
          的原生数据（会话转写、prompt 历史、项目配置条目），原有会话在新目录下可以继续
          <code>--resume</code>。
        </p>

        <label class="relocate__field">
          <span class="relocate__label">新路径</span>
          <div class="relocate__input-row">
            <input
              v-model="newPath"
              class="relocate__input"
              type="text"
              spellcheck="false"
              placeholder="例如 D:\Project\my-app-new"
            />
            <button class="btn btn-secondary" type="button" @click="pickDirectory">浏览…</button>
          </div>
          <span
            class="relocate__status"
            :class="{
              'relocate__status--ok': newPathKind === 'directory' && !sameAsOld,
              'relocate__status--bad': newPathKind !== 'checking' && newPathKind !== 'directory',
            }"
          >
            {{ statusText }}
          </span>
        </label>

        <div v-if="errorMessage" class="relocate__error">{{ errorMessage }}</div>

        <p v-if="project?.cliKind === 'claude' && project.wsl" class="relocate__note">
          这是 WSL 项目：只会更新应用内的路径，WSL 发行版内的 Claude 原生数据不会迁移。
        </p>
      </div>

      <div class="relocate__footer">
        <button class="btn btn-secondary" @click="emit('close')">取消</button>
        <button class="btn btn-primary" :disabled="!canSubmit" @click="submit">
          {{ submitting ? '迁移中…' : '迁移到新路径' }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 颜色一律取 theme.css 里真实存在的令牌，不带浅色兜底值（参见
   CodexSessionIssuesDialog.vue 的同款注释）。 */
.modal-overlay {
  position: fixed;
  inset: 0;
  background: var(--overlay);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}

.relocate {
  background: var(--card);
  color: var(--text-primary);
  border: 1px solid var(--separator);
  border-radius: var(--radius-lg);
  width: 560px;
  max-width: 92vw;
  max-height: min(520px, calc(100vh - 32px));
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: var(--modal-shadow);
}

.relocate__header {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: space-between;
  padding: 14px 18px;
  border-bottom: 1px solid var(--separator);
}

.relocate__header h3 {
  margin: 0;
  font-size: 15px;
  font-weight: 600;
  color: var(--text-primary);
}

.relocate__close {
  border: none;
  background: transparent;
  font-size: 20px;
  padding: 0 4px;
  cursor: pointer;
  color: var(--text-secondary);
  line-height: 1;
  border-radius: 4px;
  transition: color 0.12s ease;
}

.relocate__close:hover {
  color: var(--danger);
}

.relocate__body {
  padding: 14px 18px;
  overflow-y: auto;
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.relocate__intro {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--text-secondary);
}

.relocate__intro code {
  font-family: var(--font-mono);
  font-size: 11px;
  word-break: break-all;
}

.relocate__field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.relocate__label {
  font-size: var(--font-size-small);
  color: var(--text-secondary);
}

.relocate__input-row {
  display: flex;
  gap: 8px;
  align-items: center;
}

.relocate__input {
  flex: 1;
  min-width: 0;
  padding: 7px 9px;
  border: 1px solid var(--input-border);
  border-radius: var(--radius-sm);
  background: var(--input-bg);
  color: var(--text-primary);
  font-family: var(--font-mono);
  font-size: var(--font-size-small);
}

.relocate__input:focus {
  outline: none;
  border-color: var(--input-focus-border);
}

.relocate__status {
  font-size: var(--font-size-small);
  color: var(--text-secondary);
}

.relocate__status--ok {
  color: var(--success);
}

.relocate__status--bad {
  color: var(--danger);
}

.relocate__error {
  padding: 8px 10px;
  border: 1px solid var(--danger);
  border-radius: var(--radius-sm);
  background: rgba(255, 59, 48, 0.08);
  color: var(--danger);
  font-size: var(--font-size-small);
  line-height: 1.6;
  word-break: break-all;
}

.relocate__note {
  margin: 0;
  padding: 8px 10px;
  border: 1px solid var(--warning-border);
  border-radius: var(--radius-sm);
  background: var(--warning-surface);
  color: var(--warning);
  font-size: var(--font-size-small);
  line-height: 1.6;
}

.relocate__footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 18px;
  border-top: 1px solid var(--separator);
  flex-shrink: 0;
}
</style>
