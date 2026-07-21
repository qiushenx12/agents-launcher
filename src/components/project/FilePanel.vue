<template>
  <div class="file-panel">
    <div class="file-panel__toolbar">
      <span class="file-panel__title" :title="tab?.path">{{ tab?.dirty ? '● ' : '' }}{{ tab?.title }}</span>
      <div v-if="isMarkdown" class="file-panel__switch">
        <button :class="{ active: tab?.viewMode !== 'preview' }" @click="setMode('source')">源码</button>
        <button :class="{ active: tab?.viewMode === 'preview' }" @click="setMode('preview')">预览</button>
      </div>
      <span v-else class="file-panel__language">{{ tab?.language || 'text' }}</span>
    </div>
    <div
      v-show="showPreview"
      class="file-panel__preview markdown-body"
      v-html="renderedMarkdown"
    ></div>
    <div v-show="!showPreview" ref="editorRef" class="file-panel__editor" :class="{ 'file-panel__editor--md': isMarkdown }"></div>
  </div>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import MarkdownIt from 'markdown-it'
import Token from 'markdown-it/lib/token.mjs'
import hljs from 'highlight.js/lib/common'
import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection, highlightSpecialChars } from '@codemirror/view'
import { EditorState } from '@codemirror/state'
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands'
import { syntaxHighlighting, HighlightStyle, bracketMatching, foldGutter, LanguageDescription } from '@codemirror/language'
import { markdown, markdownLanguage } from '@codemirror/lang-markdown'
import { languages } from '@codemirror/language-data'
import { tags } from '@lezer/highlight'
import { useProjectStore } from '@/stores/project'

const props = defineProps<{
  tabId: string
}>()

const projectStore = useProjectStore()
const editorRef = ref<HTMLElement | null>(null)
const tab = computed(() => projectStore.visibleSidebarTabs.find((item) => item.id === props.tabId))
const isMarkdown = computed(() => tab.value?.language === 'markdown')
const showPreview = computed(() => isMarkdown.value && tab.value?.viewMode === 'preview')

function setMode(mode: 'source' | 'preview') {
  const current = tab.value
  if (current) projectStore.setFileViewMode(current.id, mode)
}

// ── Markdown preview ────────────────────────────────────────────────────────

function taskListsPlugin(md: MarkdownIt) {
  md.core.ruler.push('task-list', (state) => {
    for (let i = 0; i < state.tokens.length; i++) {
      const token = state.tokens[i]
      if (token.type !== 'inline' || !token.children) continue
      let inListItem = false
      for (let j = i - 1; j >= 0; j--) {
        const t = state.tokens[j]
        if (t.type === 'list_item_open') { inListItem = true; break }
        if (t.type === 'list_item_close' || t.type === 'bullet_list_close' || t.type === 'ordered_list_close') break
      }
      if (!inListItem) continue
      const first = token.children[0]
      if (first?.type !== 'text') continue
      const match = /^\[( |x|X)\]\s+/.exec(first.content)
      if (!match) continue
      const checkbox = new Token('html_inline', '', 0)
      checkbox.content = `<input type="checkbox" class="task-list-item-checkbox" disabled${match[1].toLowerCase() === 'x' ? ' checked' : ''}> `
      first.content = first.content.slice(match[0].length)
      token.children.splice(0, 1, checkbox, first)
    }
    return true
  })
}

const md: MarkdownIt = new MarkdownIt({
  html: false,
  linkify: true,
  highlight(code, lang) {
    if (lang && hljs.getLanguage(lang)) {
      try {
        return `<pre class="hljs"><code>${hljs.highlight(code, { language: lang, ignoreIllegals: true }).value}</code></pre>`
      } catch {
        // fall through to escaped output
      }
    }
    return `<pre class="hljs"><code>${md.utils.escapeHtml(code)}</code></pre>`
  },
}).use(taskListsPlugin)

const renderedMarkdown = computed(() => md.render(tab.value?.content ?? ''))

// ── CodeMirror editor ───────────────────────────────────────────────────────

const syntaxStyle = HighlightStyle.define([
  { tag: tags.keyword, color: 'var(--syn-keyword)' },
  { tag: [tags.string, tags.regexp], color: 'var(--syn-string)' },
  { tag: [tags.comment, tags.quote], color: 'var(--syn-comment)', fontStyle: 'italic' },
  { tag: [tags.number, tags.bool, tags.null, tags.atom], color: 'var(--syn-number)' },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName)], color: 'var(--syn-function)' },
  { tag: [tags.typeName, tags.className, tags.tagName], color: 'var(--syn-type)' },
  { tag: [tags.propertyName, tags.attributeName], color: 'var(--syn-attr)' },
  { tag: [tags.operator, tags.punctuation], color: 'var(--syn-operator)' },
  { tag: tags.heading1, color: 'var(--syn-heading)', fontWeight: '700', fontSize: '1.5em' },
  { tag: tags.heading2, color: 'var(--syn-heading)', fontWeight: '700', fontSize: '1.3em' },
  { tag: tags.heading3, color: 'var(--syn-heading)', fontWeight: '700', fontSize: '1.15em' },
  { tag: [tags.heading4, tags.heading5, tags.heading6], color: 'var(--syn-heading)', fontWeight: '700' },
  { tag: tags.link, color: 'var(--syn-link)', textDecoration: 'underline' },
  { tag: tags.url, color: 'var(--syn-link)' },
  { tag: tags.emphasis, fontStyle: 'italic' },
  { tag: tags.strong, fontWeight: '700' },
  { tag: tags.strikethrough, textDecoration: 'line-through' },
  { tag: tags.monospace, color: 'var(--syn-string)', backgroundColor: 'var(--syn-inline-code-bg)', borderRadius: '3px' },
  { tag: tags.processingInstruction, color: 'var(--syn-comment)' },
])

const cmTheme = EditorView.theme({
  '&': {
    height: '100%',
    backgroundColor: 'var(--card)',
    color: 'var(--text-primary)',
    fontSize: 'var(--md-font-size, 13px)',
  },
  '.cm-scroller': {
    fontFamily: 'var(--font-mono)',
    lineHeight: '1.65',
    overflow: 'auto',
  },
  '.cm-content': {
    padding: '10px 0',
    caretColor: 'var(--primary)',
  },
  '.cm-line': {
    padding: '0 12px',
  },
  '.cm-gutters': {
    backgroundColor: 'var(--card)',
    color: 'var(--text-secondary)',
    border: 'none',
    borderRight: '1px solid var(--separator)',
  },
  '.cm-lineNumbers .cm-gutterElement': {
    minWidth: '36px',
    padding: '0 8px 0 12px',
  },
  '.cm-activeLine': {
    backgroundColor: 'var(--editor-active-line)',
  },
  '.cm-activeLineGutter': {
    backgroundColor: 'var(--editor-active-line)',
    color: 'var(--text-primary)',
  },
  '&.cm-focused': {
    outline: 'none',
  },
  '.cm-selectionBackground, &.cm-focused .cm-selectionBackground': {
    backgroundColor: 'var(--editor-selection) !important',
  },
  '.cm-cursor': {
    borderLeftColor: 'var(--text-primary)',
  },
  '.cm-foldGutter': {
    color: 'var(--text-secondary)',
  },
  '.cm-tooltip': {
    backgroundColor: 'var(--card)',
    color: 'var(--text-primary)',
    border: '1px solid var(--separator)',
  },
  '.cm-matchingBracket': {
    backgroundColor: 'var(--editor-bracket)',
    outline: 'none',
  },
})

async function languageExtension(filename: string) {
  const isMd = /\.(md|markdown)$/i.test(filename)
  if (isMd) {
    return markdown({ base: markdownLanguage, codeLanguages: languages })
  }
  const desc = LanguageDescription.matchFilename(languages, filename)
  if (!desc) return []
  try {
    return await desc.load()
  } catch {
    return []
  }
}

let editor: EditorView | null = null
let applyingExternalChange = false

async function createEditor() {
  if (!editorRef.value) return
  const current = tab.value
  const filename = current?.path ?? current?.title ?? ''
  const lang = await languageExtension(filename)
  const isMd = /\.(md|markdown)$/i.test(filename)
  if (!editorRef.value) return
  editor = new EditorView({
    parent: editorRef.value,
    state: EditorState.create({
      doc: current?.content ?? '',
      extensions: [
        lineNumbers(),
        highlightActiveLineGutter(),
        highlightSpecialChars(),
        history(),
        foldGutter(),
        drawSelection(),
        highlightActiveLine(),
        bracketMatching(),
        keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
        lang,
        ...(isMd ? [EditorView.lineWrapping] : []),
        cmTheme,
        syntaxHighlighting(syntaxStyle),
        EditorView.updateListener.of((update) => {
          if (!update.docChanged || applyingExternalChange) return
          const id = tab.value?.id
          if (id) projectStore.updateFileContent(id, update.state.doc.toString())
        }),
      ],
    }),
  })
}

// Sync external content changes (e.g. file reopened) into the editor.
watch(() => tab.value?.content, (content) => {
  if (!editor || content === undefined) return
  if (content === editor.state.doc.toString()) return
  applyingExternalChange = true
  editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: content } })
  applyingExternalChange = false
})

onMounted(createEditor)
onBeforeUnmount(() => {
  editor?.destroy()
  editor = null
})
</script>

<style scoped>
.file-panel {
  --syn-keyword: #AF00DB;
  --syn-string: #A31515;
  --syn-comment: #008000;
  --syn-number: #098658;
  --syn-function: #795E26;
  --syn-type: #267F99;
  --syn-attr: #E50000;
  --syn-operator: #1D1D1F;
  --syn-heading: #0000FF;
  --syn-link: #007AFF;
  --syn-inline-code-bg: rgba(27, 31, 35, 0.06);
  --editor-active-line: rgba(0, 0, 0, 0.045);
  --editor-selection: rgba(0, 122, 255, 0.22);
  --editor-bracket: rgba(0, 122, 255, 0.18);
  height: 100%;
  display: flex;
  flex-direction: column;
  min-height: 0;
}

[data-theme="dark"] .file-panel {
  --syn-keyword: #C586C0;
  --syn-string: #CE9178;
  --syn-comment: #6A9955;
  --syn-number: #B5CEA8;
  --syn-function: #DCDCAA;
  --syn-type: #4EC9B0;
  --syn-attr: #9CDCFE;
  --syn-operator: #D4D4D4;
  --syn-heading: #569CD6;
  --syn-link: #4DA3FF;
  --syn-inline-code-bg: rgba(255, 255, 255, 0.08);
  --editor-active-line: rgba(255, 255, 255, 0.05);
  --editor-selection: rgba(10, 132, 255, 0.35);
  --editor-bracket: rgba(10, 132, 255, 0.3);
}

.file-panel__toolbar {
  height: 38px;
  flex: 0 0 38px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 0 10px;
  border-bottom: 1px solid var(--separator);
  color: var(--text-secondary);
  font-size: var(--font-size-small);
}

.file-panel__title {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-panel__switch {
  display: inline-flex;
  flex: 0 0 auto;
  padding: 2px;
  border: 1px solid var(--separator);
  border-radius: var(--radius-sm);
  background: var(--bg);
}

.file-panel__switch button {
  border: 0;
  border-radius: 4px;
  padding: 3px 8px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
}

.file-panel__switch button.active {
  background: var(--card);
  color: var(--primary);
}

.file-panel__language {
  flex: 0 0 auto;
  font-family: var(--font-mono);
}

.file-panel__editor {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}

.file-panel__editor :deep(.cm-editor) {
  height: 100%;
}

[data-theme="dark"] .file-panel__editor--md :deep(.cm-editor) {
  background: var(--bg);
}

.file-panel__preview {
  flex: 1;
  min-height: 0;
  overflow: auto;
  overflow-x: hidden;
  padding: 16px 18px;
  color: var(--text-primary);
  font-size: var(--md-font-size, 13px);
  line-height: 1.7;
  word-wrap: break-word;
  overflow-wrap: break-word;
}

[data-theme="dark"] .file-panel__preview {
  background: var(--bg);
}

/* ── Markdown preview (GitHub / VSCode style, theme aware) ─────────────── */

.markdown-body :deep(h1),
.markdown-body :deep(h2) {
  margin: 20px 0 12px;
  padding-bottom: 6px;
  border-bottom: 1px solid var(--separator);
  line-height: 1.3;
}

.markdown-body :deep(h1) { font-size: 1.6em; }
.markdown-body :deep(h2) { font-size: 1.35em; }

.markdown-body :deep(h3),
.markdown-body :deep(h4),
.markdown-body :deep(h5),
.markdown-body :deep(h6) {
  margin: 16px 0 8px;
  line-height: 1.35;
}

.markdown-body :deep(h3) { font-size: 1.15em; }
.markdown-body :deep(h4) { font-size: 1.05em; }

.markdown-body :deep(h1:first-child),
.markdown-body :deep(h2:first-child),
.markdown-body :deep(h3:first-child) {
  margin-top: 0;
}

.markdown-body :deep(p) {
  margin: 0 0 10px;
}

.markdown-body :deep(a) {
  color: var(--primary);
  text-decoration: none;
  overflow-wrap: anywhere;
}

.markdown-body :deep(a:hover) {
  text-decoration: underline;
}

.markdown-body :deep(ul),
.markdown-body :deep(ol) {
  margin: 0 0 10px;
  padding-left: 26px;
}

.markdown-body :deep(li) {
  margin: 3px 0;
}

.markdown-body :deep(li > ul),
.markdown-body :deep(li > ol) {
  margin-bottom: 0;
}

.markdown-body :deep(.task-list-item-checkbox) {
  margin: 0 6px 0 -20px;
  vertical-align: middle;
  accent-color: var(--primary);
}

.markdown-body :deep(blockquote) {
  margin: 0 0 10px;
  padding: 2px 14px;
  border-left: 3px solid var(--primary);
  background: var(--bg);
  color: var(--text-secondary);
  border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
}

.markdown-body :deep(blockquote p:last-child) {
  margin-bottom: 0;
}

.markdown-body :deep(code) {
  padding: 2px 5px;
  border-radius: 4px;
  background: var(--syn-inline-code-bg);
  font-family: var(--font-mono);
  font-size: 0.92em;
  overflow-wrap: anywhere;
}

.markdown-body :deep(pre) {
  margin: 0 0 12px;
  padding: 12px 14px;
  border: 1px solid var(--separator);
  border-radius: var(--radius-sm);
  background: var(--bg);
  overflow-x: hidden;
  overflow-y: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.markdown-body :deep(pre code) {
  padding: 0;
  background: transparent;
  font-size: 0.92em;
  line-height: 1.6;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.markdown-body :deep(table) {
  margin: 0 0 12px;
  border-collapse: collapse;
  display: block;
  overflow: auto;
}

.markdown-body :deep(th),
.markdown-body :deep(td) {
  padding: 6px 12px;
  border: 1px solid var(--separator);
  overflow-wrap: break-word;
}

.markdown-body :deep(th) {
  background: var(--bg);
  font-weight: 700;
}

.markdown-body :deep(tr:nth-child(even)) {
  background: var(--bg);
}

.markdown-body :deep(hr) {
  margin: 18px 0;
  border: 0;
  border-top: 1px solid var(--separator);
}

.markdown-body :deep(img) {
  max-width: 100%;
}

/* ── highlight.js token colors (theme aware) ───────────────────────────── */

.markdown-body :deep(.hljs-keyword),
.markdown-body :deep(.hljs-selector-tag),
.markdown-body :deep(.hljs-meta .hljs-keyword) {
  color: var(--syn-keyword);
}

.markdown-body :deep(.hljs-string),
.markdown-body :deep(.hljs-regexp),
.markdown-body :deep(.hljs-meta .hljs-string) {
  color: var(--syn-string);
}

.markdown-body :deep(.hljs-comment),
.markdown-body :deep(.hljs-quote) {
  color: var(--syn-comment);
  font-style: italic;
}

.markdown-body :deep(.hljs-number),
.markdown-body :deep(.hljs-literal) {
  color: var(--syn-number);
}

.markdown-body :deep(.hljs-title),
.markdown-body :deep(.hljs-title.function_),
.markdown-body :deep(.hljs-section) {
  color: var(--syn-function);
}

.markdown-body :deep(.hljs-title.class_),
.markdown-body :deep(.hljs-type),
.markdown-body :deep(.hljs-built_in) {
  color: var(--syn-type);
}

.markdown-body :deep(.hljs-attr),
.markdown-body :deep(.hljs-attribute),
.markdown-body :deep(.hljs-variable),
.markdown-body :deep(.hljs-template-variable),
.markdown-body :deep(.hljs-name) {
  color: var(--syn-attr);
}

.markdown-body :deep(.hljs-tag),
.markdown-body :deep(.hljs-operator),
.markdown-body :deep(.hljs-punctuation) {
  color: var(--syn-operator);
}

.markdown-body :deep(.hljs-symbol),
.markdown-body :deep(.hljs-bullet),
.markdown-body :deep(.hljs-link) {
  color: var(--syn-link);
}

.markdown-body :deep(.hljs-emphasis) { font-style: italic; }
.markdown-body :deep(.hljs-strong) { font-weight: 700; }

.markdown-body :deep(.hljs-addition) {
  color: var(--success);
  background: rgba(52, 199, 89, 0.1);
}

.markdown-body :deep(.hljs-deletion) {
  color: var(--danger);
  background: rgba(255, 59, 48, 0.1);
}
</style>
