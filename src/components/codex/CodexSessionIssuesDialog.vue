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
/*
 * 颜色一律取 `src/assets/styles/theme.css` 里真实存在的令牌。
 *
 * 这里原来写的是 `--bg-primary` / `--border-color`，那两个名字在本项目里
 * 并不存在，而每一处都带了浅色兜底值（`#fff` / `#e2e5ea`）—— 于是深色主题下
 * 弹窗照样是白底黑字、边框是浅灰，只有按钮跟着主题变了，整块面板像贴在
 * 界面上的外来窗口。带兜底的 `var()` 不会在 `tests/styleTokenContract.test.ts`
 * 的通则里报错（那一条只钉「不带兜底又没定义」），所以名字写错能一直藏着。
 *
 * 兜底值在这里也一并去掉：留着就等于给下一次改名留了条静默失效的路。令牌名
 * 写错应该在测试里立刻暴露，而不是等到肉眼看见白面板。
 */
.modal-overlay {
  position: fixed;
  inset: 0;
  background: var(--overlay);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}

.session-issues {
  background: var(--card);
  color: var(--text-primary);
  border: 1px solid var(--separator);
  border-radius: var(--radius-lg);
  width: 640px;
  max-width: 92vw;
  /* 上限落在视口单位上：父级是 flex 容器，百分比在这里解析不出确定值，
     整条 max-height 会被判无效，正文溢出到窗口外面。 */
  max-height: min(600px, calc(100vh - 32px));
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: var(--modal-shadow);
}

.session-issues__header {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: space-between;
  padding: 14px 18px;
  border-bottom: 1px solid var(--separator);
}

.session-issues__header h3 {
  margin: 0;
  font-size: 15px;
  font-weight: 600;
  color: var(--text-primary);
}

.session-issues__close {
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

.session-issues__close:hover {
  color: var(--danger);
}

.session-issues__body {
  padding: 14px 18px;
  overflow-y: auto;
  /* 唯一的滚动区：不给 flex 子项 min-height: 0，它就不肯收缩，自己的滚动条
     也就跟着失效。 */
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.session-issues__intro {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--text-secondary);
}

/* 正文里的警示句跟着主题走：浅色主题用深一档的橙（浅底上才够对比度），
   深色主题直接取亮橙。 */
.session-issues__intro strong {
  color: var(--warning-strong);
}

.issue-card {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  border: 1px solid var(--separator);
  border-radius: var(--radius-sm);
  padding: 10px 12px;
  background: var(--bg);
}

.issue-card__info {
  flex: 1;
  min-width: 0;
}

.issue-card__title {
  font-size: 13px;
  font-weight: 600;
  word-break: break-all;
  color: var(--text-primary);
}

.issue-card__meta {
  font-size: 11px;
  color: var(--text-secondary);
  word-break: break-all;
  margin-top: 2px;
}

.issue-card__detail {
  font-size: 12px;
  margin-top: 6px;
  line-height: 1.6;
  color: var(--text-primary);
}

.issue-card__kind {
  display: inline-block;
  font-size: 11px;
  padding: 0 6px;
  margin-right: 6px;
  border-radius: 3px;
  color: var(--warning);
  background: color-mix(in srgb, var(--warning) 16%, transparent);
}

.issue-card__kind--advisory {
  color: var(--text-secondary);
  background: var(--tab-bg);
}

.issue-card__error {
  font-size: 12px;
  color: var(--danger);
  margin-top: 6px;
  line-height: 1.6;
}

.issue-card__actions {
  flex-shrink: 0;
}

.session-issues__empty {
  font-size: 13px;
  color: var(--text-secondary);
  text-align: center;
  padding: 12px 0;
}

.session-issues__footer {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: flex-end;
  gap: 10px;
  padding: 12px 18px;
  border-top: 1px solid var(--separator);
}

.session-issues__hint {
  flex: 1;
  font-size: 12px;
  color: var(--text-secondary);
}
</style>
