<script setup lang="ts">
/**
 * dsh 版本选择列表。
 *
 * 后端只返回「有哪些版本 + 当前固定的是哪个」，勾选与确定都在这里完成：
 * 列表是倒序（最新在上），当前版本所在行显示勾选状态，底部常驻显示当前版本号。
 *
 * 为什么不是「查到 latest 就提示更新」：dsh 的发版是分批的，最新版本经常正是
 * 装不上的那个（配套包还没发完）。用户真正要回答的是「我该跑哪个版本」，所以
 * 需要看到完整列表并能往回挑，而不是被推着往最新走。
 */
import { computed, nextTick, ref, watch } from 'vue'
import { useDshConfigStore } from '../../stores/dshConfig'

const store = useDshConfigStore()

const versions = computed(() => store.versionPickerList?.versions ?? [])
const pinned = computed(() => store.versionPickerList?.pinned ?? null)
const latest = computed(() => store.versionPickerList?.latest ?? null)
const loading = computed(() => store.updatingVersion && !store.versionPickerList)

const listEl = ref<HTMLElement | null>(null)

/**
 * 列表很长（dsh 的版本号已经排到几十个），而面板高度有上限，当前版本所在的
 * 那一行默认可能落在可视区之外 —— 那样「当前版本」这个标记就等于没显示。
 * 列表到达后把它滚到中间。
 */
function revealPinned() {
  const list = listEl.value
  const target = pinned.value
  if (!list || !target) return
  const row = list.querySelector<HTMLElement>(`[data-version="${target}"]`)
  if (!row) return
  // 手算 scrollTop 而不是 scrollIntoView：后者会连带滚动祖先容器，而这里只有
  // 列表自己是滚动区。`.version-list` 设了 position: relative，所以 offsetTop
  // 就是相对列表顶部的距离。越界值由浏览器自行夹取。
  list.scrollTop = row.offsetTop - (list.clientHeight - row.offsetHeight) / 2
}

watch(versions, () => { void nextTick(revealPinned) }, { immediate: true })

/** 确定按钮只在真的选了「另一个」版本时才可点。 */
const canConfirm = computed(() => (
  !!store.versionPickerChoice && store.versionPickerChoice !== pinned.value
))

function rowState(version: string) {
  return {
    'version-row--selected': store.versionPickerChoice === version,
    'version-row--pinned': pinned.value === version,
  }
}
</script>

<template>
  <div class="version-dialog" role="dialog" aria-modal="true" aria-label="选择 dsh 版本">
    <div class="version-dialog__panel">
      <header class="version-dialog__head">
        <h3>选择 dsh 版本</h3>
        <button class="version-dialog__close" type="button" @click="store.closeVersionPicker()">
          &times;
        </button>
      </header>

      <p v-if="loading" class="version-dialog__hint">正在获取可用版本…</p>
      <p v-else-if="store.versionPickerError" class="version-dialog__error">
        {{ store.versionPickerError }}
      </p>
      <p v-else-if="versions.length === 0" class="version-dialog__hint">没有查到可用版本。</p>

      <ul v-else ref="listEl" class="version-list">
        <li
          v-for="version in versions"
          :key="version"
          class="version-row"
          :class="rowState(version)"
          :data-version="version"
          role="button"
          tabindex="0"
          @click="store.chooseVersion(version)"
          @keydown.enter.prevent="store.chooseVersion(version)"
          @keydown.space.prevent="store.chooseVersion(version)"
        >
          <span class="version-row__check" aria-hidden="true">
            <template v-if="store.versionPickerChoice === version">✓</template>
          </span>
          <span class="version-row__label">v{{ version }}</span>
          <span v-if="latest === version" class="version-row__tag">最新</span>
          <span v-if="pinned === version" class="version-row__tag version-row__tag--current">
            当前版本
          </span>
        </li>
      </ul>

      <footer class="version-dialog__foot">
        <div class="version-dialog__foot-info">
          <span class="version-dialog__current">
            当前版本：{{ pinned ? `v${pinned}` : '未固定' }}
          </span>
          <span v-if="store.versionPickerChoice && store.versionPickerChoice !== pinned" class="version-dialog__next">
            将切换到 v{{ store.versionPickerChoice }}
          </span>
        </div>
        <div class="version-dialog__actions">
          <button class="btn btn-secondary" type="button" @click="store.closeVersionPicker()">
            取消
          </button>
          <button
            class="btn btn-primary"
            type="button"
            :disabled="!canConfirm || store.updatingVersion"
            @click="store.confirmVersionChoice()"
          >
            {{ store.updatingVersion ? '正在保存…' : '确定' }}
          </button>
        </div>
      </footer>
    </div>
  </div>
</template>

<style scoped>
/*
 * 颜色一律取 `src/assets/styles/theme.css` 里真实存在的令牌（`--card` /
 * `--text-primary` / `--separator` / `--primary` …）。这里曾经写过一套
 * `--color-background-primary` 之类的名字，那套令牌在本项目里并不存在，而
 * `var()` 又没带兜底值 —— 于是整条 background / border 声明在计算期被判无效，
 * 面板直接变成透明的，只剩文字浮在应用界面上。
 */
.version-dialog {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  /* 撑满固定遮罩，使面板的高度上限有一个确定的参照物。 */
  height: 100%;
  padding: 16px;
}

.version-dialog__panel {
  display: flex;
  flex-direction: column;
  width: min(420px, 100%);
  /*
   * 上限必须落在视口单位上。父级（.version-dialog）的高度由内容撑开，是
   * 不确定值，百分比在这里解析不出结果 —— 整条 max-height 会被判无效，
   * 列表于是一路溢出到窗口外面，连底部的按钮都推出去了。
   */
  max-height: min(560px, calc(100vh - 32px));
  padding: 16px;
  border: 1px solid var(--separator);
  border-radius: var(--radius-lg);
  background: var(--card);
  box-shadow: 0 12px 32px rgba(0, 0, 0, 0.32);
}

.version-dialog__head {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 12px;
}

.version-dialog__head h3 {
  margin: 0;
  font-size: 14px;
  font-weight: 500;
  color: var(--text-primary);
}

.version-dialog__close {
  border: 0;
  background: transparent;
  color: var(--text-secondary);
  font-size: 18px;
  line-height: 1;
  cursor: pointer;
}

.version-dialog__hint,
.version-dialog__error {
  margin: 0 0 12px;
  font-size: 13px;
  line-height: 1.6;
}

.version-dialog__hint { color: var(--text-secondary); }
.version-dialog__error { color: var(--danger); }

/* 唯一的滚动区：撑满标题与页脚之间的剩余高度，越界时内部滚动。 */
.version-list {
  flex: 1;
  min-height: 0;
  margin: 0;
  padding: 0 4px 0 0;
  overflow-y: auto;
  position: relative;
  list-style: none;
}

.version-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 8px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-size: 13px;
  color: var(--text-primary);
  user-select: none;
}

.version-row:hover { background: var(--tab-bg); }

/* 选中态用实心主色，与供应商清单 / 侧栏条目保持一致。 */
.version-row--selected {
  color: #fff;
  background: var(--primary);
}

.version-row--selected:hover { background: var(--primary-hover); }

.version-row__check {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 14px;
  color: var(--primary);
}

.version-row__label {
  font-family: var(--font-mono);
  font-size: 13px;
}

.version-row__tag {
  padding: 1px 6px;
  border-radius: 999px;
  background: var(--tab-bg);
  color: var(--text-secondary);
  font-size: 11px;
}

.version-row__tag--current {
  color: var(--success, #22c55e);
  background: color-mix(in srgb, var(--success, #22c55e) 14%, transparent);
}

/* 选中行整行翻成主色底，行内的勾与标签也得跟着翻成浅色才读得出来。 */
.version-row--selected .version-row__check { color: #fff; }

.version-row--selected .version-row__tag {
  color: #fff;
  background: rgba(255, 255, 255, 0.2);
}

.version-dialog__foot {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-top: 12px;
  padding-top: 12px;
  border-top: 1px solid var(--separator);
}

.version-dialog__foot-info {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.version-dialog__current {
  font-size: 13px;
  color: var(--text-primary);
}

.version-dialog__next {
  font-size: 12px;
  color: var(--primary);
}

.version-dialog__actions {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
}
</style>
