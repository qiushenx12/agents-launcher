<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { CodexSessionIssue } from '@/types/config'
import { useProjectStore } from '@/stores/project'

const props = defineProps<{
  issues: CodexSessionIssue[]
  /** apply：切换配置被拦截的场景，提供"仍然切换"出口；workspace：工作区横幅入口 */
  mode: 'workspace' | 'apply'
}>()

const emit = defineEmits<{
  (event: 'close'): void
  (event: 'continue-apply'): void
}>()

const projectStore = useProjectStore()

const localIssues = ref<CodexSessionIssue[]>([...props.issues])
watch(() => props.issues, (issues) => {
  localIssues.value = [...issues]
})

const repairingThreadId = ref<string | null>(null)
const repairErrors = ref<Record<string, string>>({})
const repairedThreadIds = ref<Set<string>>(new Set())

interface IssueGroup {
  threadId: string
  projectName: string
  preview: string
  cwd: string
  repairable: boolean
  issues: CodexSessionIssue[]
}

const groups = computed<IssueGroup[]>(() => {
  const map = new Map<string, IssueGroup>()
  for (const issue of localIssues.value) {
    const existing = map.get(issue.threadId)
    if (existing) {
      existing.issues.push(issue)
      existing.repairable = existing.repairable || issue.repairable
    } else {
      map.set(issue.threadId, {
        threadId: issue.threadId,
        projectName: issue.projectName,
        preview: issue.preview,
        cwd: issue.cwd,
        repairable: issue.repairable,
        issues: [issue],
      })
    }
  }
  return [...map.values()]
})

const remainingRepairable = computed(() =>
  groups.value.filter(group => group.repairable).length,
)

function kindLabel(kind: string): string {
  switch (kind) {
    case 'orphaned_tail': return '分页断链'
    case 'parent_missing': return '父页缺失'
    case 'base_beyond_end': return '挂接点越界'
    case 'writer_newer_than_cli': return 'CLI 版本过旧'
    default: return kind
  }
}

function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${bytes} B`
}

function formatOrphanRange(issue: CodexSessionIssue): string {
  if (!issue.orphanedFirstAt || !issue.orphanedLastAt) return ''
  const first = new Date(issue.orphanedFirstAt)
  const last = new Date(issue.orphanedLastAt)
  if (Number.isNaN(first.getTime()) || Number.isNaN(last.getTime())) return ''
  const fmt = (value: Date) =>
    `${value.getMonth() + 1}-${value.getDate()} ${String(value.getHours()).padStart(2, '0')}:${String(value.getMinutes()).padStart(2, '0')}`
  return `${fmt(first)} ~ ${fmt(last)}`
}

async function repair(group: IssueGroup) {
  if (repairingThreadId.value) return
  repairingThreadId.value = group.threadId
  delete repairErrors.value[group.threadId]
  try {
    await projectStore.repairCodexSessionChain(group.threadId)
    repairedThreadIds.value = new Set([...repairedThreadIds.value, group.threadId])
    localIssues.value = localIssues.value.filter(
      issue => !(issue.threadId === group.threadId && issue.repairable),
    )
  } catch (error) {
    repairErrors.value = { ...repairErrors.value, [group.threadId]: String(error) }
  } finally {
    repairingThreadId.value = null
  }
}
</script>

<template>
  <div class="modal-overlay" @click.self="emit('close')">
    <div class="session-issues">
      <div class="session-issues__header">
        <h3>Codex 会话完整性</h3>
        <button class="session-issues__close" @click="emit('close')">&times;</button>
      </div>

      <div class="session-issues__body">
        <p class="session-issues__intro">
          Codex 0.149+ 的分页会话在翻页时可能产生断链：父页尾部的一段记录不在任何页链上，
          重启 Codex 桌面端后这些聊天内容会显示不出来（数据本身仍在磁盘上）。
          修复会把页链合并为完整单文件，原始文件自动备份。
          <strong>修复前请完全退出 Codex 桌面端与 VS Code。</strong>
        </p>

        <div
          v-for="group in groups"
          :key="group.threadId"
          class="issue-card"
        >
          <div class="issue-card__info">
            <div class="issue-card__title">
              {{ group.preview || group.projectName || group.threadId }}
            </div>
            <div class="issue-card__meta">{{ group.cwd }}</div>
            <div
              v-for="(issue, index) in group.issues"
              :key="index"
              class="issue-card__detail"
            >
              <span class="issue-card__kind" :class="{ 'issue-card__kind--advisory': !issue.repairable }">
                {{ kindLabel(issue.kind) }}
              </span>
              <template v-if="issue.kind === 'orphaned_tail'">
                孤立段 {{ issue.orphanedRecords }} 条记录（{{ formatBytes(issue.orphanedBytes) }}，
                含 {{ issue.orphanedUserMessages }} 条用户消息<template v-if="formatOrphanRange(issue)">，{{ formatOrphanRange(issue) }}</template>）
              </template>
              <template v-else>
                {{ issue.message }}
              </template>
            </div>
            <div v-if="repairErrors[group.threadId]" class="issue-card__error">
              {{ repairErrors[group.threadId] }}
            </div>
          </div>
          <div class="issue-card__actions">
            <button
              v-if="group.repairable"
              class="btn btn-primary"
              :disabled="repairingThreadId !== null"
              @click="repair(group)"
            >
              {{ repairingThreadId === group.threadId ? '修复中…' : '修复此会话' }}
            </button>
          </div>
        </div>

        <div v-if="groups.length === 0" class="session-issues__empty">
          没有待处理的会话问题了。
        </div>
      </div>

      <div class="session-issues__footer">
        <template v-if="mode === 'apply'">
          <span class="session-issues__hint">
            {{ remainingRepairable > 0 ? '修复全部断链后请重新点击"应用"。' : '断链已全部修复，请重新点击"应用"完成切换。' }}
          </span>
          <button class="btn btn-secondary" @click="emit('close')">取消切换</button>
          <button class="btn btn-secondary" @click="emit('continue-apply')">仍然切换</button>
        </template>
        <template v-else>
          <button class="btn btn-secondary" @click="emit('close')">关闭</button>
        </template>
      </div>
    </div>
  </div>
</template>

<style scoped>
.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}

.session-issues {
  background: var(--bg-primary, #fff);
  color: var(--text-primary, #1f2328);
  border-radius: 8px;
  width: 640px;
  max-width: 92vw;
  max-height: 80vh;
  display: flex;
  flex-direction: column;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.24);
}

.session-issues__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 18px;
  border-bottom: 1px solid var(--border-color, #e2e5ea);
}

.session-issues__header h3 {
  margin: 0;
  font-size: 15px;
}

.session-issues__close {
  border: none;
  background: transparent;
  font-size: 20px;
  cursor: pointer;
  color: inherit;
  line-height: 1;
}

.session-issues__body {
  padding: 14px 18px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.session-issues__intro {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--text-secondary, #5b6470);
}

.issue-card {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  border: 1px solid var(--border-color, #e2e5ea);
  border-radius: 6px;
  padding: 10px 12px;
}

.issue-card__info {
  flex: 1;
  min-width: 0;
}

.issue-card__title {
  font-size: 13px;
  font-weight: 600;
  word-break: break-all;
}

.issue-card__meta {
  font-size: 11px;
  color: var(--text-secondary, #5b6470);
  word-break: break-all;
  margin-top: 2px;
}

.issue-card__detail {
  font-size: 12px;
  margin-top: 6px;
  line-height: 1.6;
}

.issue-card__kind {
  display: inline-block;
  font-size: 11px;
  padding: 0 6px;
  margin-right: 6px;
  border-radius: 3px;
  background: rgba(210, 120, 20, 0.16);
  color: #b05f00;
}

.issue-card__kind--advisory {
  background: rgba(80, 120, 200, 0.14);
  color: #3366b8;
}

.issue-card__error {
  font-size: 12px;
  color: #c0392b;
  margin-top: 6px;
  line-height: 1.6;
}

.issue-card__actions {
  flex-shrink: 0;
}

.session-issues__empty {
  font-size: 13px;
  color: var(--text-secondary, #5b6470);
  text-align: center;
  padding: 12px 0;
}

.session-issues__footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 10px;
  padding: 12px 18px;
  border-top: 1px solid var(--border-color, #e2e5ea);
}

.session-issues__hint {
  flex: 1;
  font-size: 12px;
  color: var(--text-secondary, #5b6470);
}
</style>
