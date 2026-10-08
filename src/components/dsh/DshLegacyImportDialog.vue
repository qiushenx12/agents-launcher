<script setup lang="ts">
/**
 * 「settings.yaml.imported 里有供应商没进新配置」的一键恢复弹窗。
 *
 * dsh 0.2.0 的一次性导入先改名再逐节导入，被装配校验拒绝的节（典型：自定义
 * 路由缺 api）只留在 .imported 里——数据没丢，但界面上什么都没有。后端读取时
 * 把这些残留扫进 document.legacyImport；本弹窗列出候选、说明各自的障碍，
 * 确认后由后端把原始块逐字节移植进当前布局文档（含启动器不编辑的字段）。
 *
 * 只在「有东西可恢复」时才会出现；用户本次会话关掉后不再自动弹（store 里的
 * legacyImportDismissed），恢复成功且候选清零后自然不再出现。
 */
import { computed } from 'vue'
import { useDshModelsStore } from '@/stores/dshModels'

const store = useDshModelsStore()

const info = computed(() => store.legacyImport)
const restorable = computed(() =>
  (info.value?.candidates ?? []).filter((item) => item.problems.length === 0))
const blocked = computed(() =>
  (info.value?.candidates ?? []).filter((item) => item.problems.length > 0))

/** 有未写入的草稿改动时先说清：恢复后的静默重读会丢弃它们。 */
const hasDirtyDrafts = computed(() => store.providers.some((draft) => store.isProviderDirty(draft)))
</script>

<template>
  <div class="import-dialog" role="dialog" aria-modal="true" aria-label="恢复供应商">
    <div class="import-dialog__panel">
      <header class="import-dialog__head">
        <h3>恢复被丢弃的供应商</h3>
        <button class="import-dialog__close" type="button" @click="store.dismissLegacyImport()">
          &times;
        </button>
      </header>

      <div class="import-dialog__body">
        <p class="import-dialog__intro">
          升级到 dsh 0.2.0 时的一次性导入没有带走它们（缺必填字段会被整段拒绝），
          数据还在 <code>settings.yaml.imported</code> 里。
        </p>

        <p v-if="info?.parseError" class="import-dialog__error">
          残留文件解析失败，无法自动恢复：{{ info.parseError }}
        </p>

        <ul v-else class="import-list">
          <li v-for="item in restorable" :key="item.id" class="import-row">
            <span class="import-row__name">{{ item.displayName || item.id }}</span>
            <span class="import-row__meta">{{ item.id }} · {{ item.models.length }} 个模型</span>
          </li>
          <li v-for="item in blocked" :key="item.id" class="import-row import-row--blocked">
            <span class="import-row__name">{{ item.displayName || item.id }}</span>
            <span class="import-row__meta">{{ item.id }} · {{ item.models.length }} 个模型</span>
            <span class="import-row__problem">
              需先手工补齐：{{ item.problems.join('；') }}
            </span>
          </li>
        </ul>

        <p v-if="hasDirtyDrafts" class="import-dialog__warning">
          当前有未写入的修改，恢复后的重新读取会丢弃它们。
        </p>
      </div>

      <footer class="import-dialog__foot">
        <span v-if="store.needsRestartToApply" class="import-dialog__note">
          恢复后需重启 dsh 服务生效。
        </span>
        <div class="import-dialog__actions">
          <button class="btn btn-secondary" type="button" @click="store.dismissLegacyImport()">
            取消
          </button>
          <button
            class="btn btn-primary"
            type="button"
            :disabled="restorable.length === 0 || store.restoringImport"
            @click="store.restoreImportedProviders()"
          >
            {{ store.restoringImport ? '正在恢复…' : `恢复 ${restorable.length} 个供应商` }}
          </button>
        </div>
      </footer>
    </div>
  </div>
</template>

<style scoped>
/*
 * 令牌只取 `src/assets/styles/theme.css` 里真实存在的名字（--card / --separator /
 * --text-primary / --warning / --radius-lg / --modal-shadow …）；max-height 落在
 * 视口单位上（父级由内容撑开，百分比解析不出结果）。
 */
.import-dialog {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
  padding: 16px;
}

.import-dialog__panel {
  display: flex;
  flex-direction: column;
  width: min(520px, 100%);
  max-height: min(560px, calc(100vh - 32px));
  padding: 16px;
  border: 1px solid var(--separator);
  border-radius: var(--radius-lg);
  background: var(--card);
  box-shadow: var(--modal-shadow);
}

.import-dialog__head {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 12px;
}

.import-dialog__head h3 {
  margin: 0;
  font-size: 14px;
  font-weight: 500;
  color: var(--text-primary);
}

.import-dialog__close {
  border: 0;
  background: transparent;
  color: var(--text-secondary);
  font-size: 18px;
  line-height: 1;
  cursor: pointer;
}

.import-dialog__body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.import-dialog__intro {
  margin: 0 0 12px;
  font-size: 13px;
  line-height: 1.6;
  color: var(--text-secondary);
}

.import-dialog__intro code { overflow-wrap: anywhere; }

.import-dialog__error {
  margin: 0 0 12px;
  font-size: 13px;
  line-height: 1.6;
  color: var(--danger);
}

.import-dialog__warning {
  margin: 12px 0 0;
  font-size: 12px;
  line-height: 1.6;
  color: var(--warning);
}

.import-list {
  margin: 0;
  padding: 0;
  list-style: none;
}

.import-row {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 4px 8px;
  padding: 7px 8px;
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: 13px;
}

.import-row:hover { background: var(--tab-bg); }

.import-row__name { font-weight: 500; }

.import-row__meta {
  color: var(--text-secondary);
  font-size: 12px;
  font-family: var(--font-mono);
}

.import-row__problem {
  flex-basis: 100%;
  color: var(--warning);
  font-size: 12px;
  line-height: 1.5;
}

.import-dialog__foot {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-top: 12px;
  padding-top: 12px;
  border-top: 1px solid var(--separator);
}

.import-dialog__note {
  font-size: 12px;
  color: var(--text-secondary);
}

.import-dialog__actions {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
  margin-left: auto;
}
</style>
