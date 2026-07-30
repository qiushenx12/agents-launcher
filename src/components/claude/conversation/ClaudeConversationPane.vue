<template>
  <section
    class="claude-conversation"
    :class="{
      'has-terminal-prompt': terminalPrompt,
      'has-floating-topbar': workingActivity || !followLatest,
      'is-drag-over': isDragOver,
    }"
    @dragenter.prevent="isDragOver = true"
    @dragover.prevent="isDragOver = true"
    @dragleave.prevent="onSectionDragLeave"
    @drop.prevent.stop="onSectionDrop"
  >
    <div v-if="isDragOver" class="claude-drop-overlay" aria-hidden="true">
      <div class="claude-drop-overlay__inner">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="17 8 12 3 7 8"/><line x1="12" y1="3" x2="12" y2="15"/></svg>
        <span>拖入文件附加到消息</span>
      </div>
    </div>

    <span class="claude-conversation__copy-status" aria-live="polite" aria-atomic="true">
      {{ copyAnnouncement }}
    </span>
    <span class="claude-conversation__activity-status" aria-live="polite" aria-atomic="true">
      {{ activityAnnouncement || statusLabel }}
    </span>

    <div
      v-if="workspaceTrustPrompt"
      ref="workspaceTrustOverlayRef"
      class="workspace-trust-overlay"
      role="alertdialog"
      aria-modal="true"
      tabindex="-1"
      :aria-busy="workspaceTrustPending"
      :aria-labelledby="`workspace-trust-title-${composerDomId}`"
      :aria-describedby="`workspace-trust-description-${composerDomId}`"
      @keydown.esc.prevent="respondToWorkspaceTrust('cancel')"
      @keydown.tab="trapWorkspaceTrustFocus"
    >
      <section class="workspace-trust-dialog">
        <div class="workspace-trust-dialog__icon" aria-hidden="true">!</div>
        <div class="workspace-trust-dialog__content">
          <h2 :id="`workspace-trust-title-${composerDomId}`">是否信任此工作区？</h2>
          <p :id="`workspace-trust-description-${composerDomId}`">
            Claude Code 将能够读取、编辑并执行其中的文件。
          </p>
          <code>{{ workspaceTrustPrompt.path }}</code>
          <p v-if="workspaceTrustError" class="workspace-trust-dialog__error" role="alert">
            {{ workspaceTrustError }}
          </p>
        </div>
        <div class="workspace-trust-dialog__actions">
          <button
            ref="workspaceTrustCancelRef"
            class="btn btn-secondary"
            type="button"
            :disabled="workspaceTrustPending"
            @click="respondToWorkspaceTrust('cancel')"
          >
            {{ workspaceTrustPending && workspaceTrustAction === 'cancel' ? '正在退出…' : '不信任并退出' }}
          </button>
          <button
            ref="workspaceTrustConfirmRef"
            class="btn btn-primary"
            type="button"
            :disabled="workspaceTrustPending"
            @click="respondToWorkspaceTrust('confirm')"
          >
            {{ workspaceTrustPending && workspaceTrustAction === 'confirm' ? '正在继续…' : '信任并继续' }}
          </button>
        </div>
      </section>
    </div>

    <div
      v-if="selectionPrompt"
      ref="pluginInstallOverlayRef"
      class="workspace-trust-overlay plugin-install-overlay"
      :class="{ 'model-select-overlay': modelSwitchConfirmPrompt }"
      role="alertdialog"
      aria-modal="true"
      tabindex="-1"
      :aria-busy="pluginInstallPending"
      :aria-labelledby="`plugin-install-title-${composerDomId}`"
      :aria-describedby="`plugin-install-description-${composerDomId}`"
      @keydown="handleTerminalPromptKeydown"
    >
      <section class="workspace-trust-dialog plugin-install-dialog" :class="{ 'model-select-dialog': modelSwitchConfirmPrompt }">
        <div class="workspace-trust-dialog__icon plugin-install-dialog__icon" aria-hidden="true">
          <svg v-if="modelSwitchConfirmPrompt" viewBox="0 0 24 24" focusable="false">
            <path d="M4 7h13l-3-3" />
            <path d="m20 7-3-3" />
            <path d="M20 17H7l3 3" />
            <path d="m4 17 3 3" />
          </svg>
          <span v-else-if="planApprovalPrompt">✓</span>
          <span v-else>+</span>
        </div>
        <div class="workspace-trust-dialog__content">
          <h2 :id="`plugin-install-title-${composerDomId}`">
            {{ planApprovalPrompt ? '确认计划下一步' : modelSwitchConfirmPrompt ? '确认切换模型' : '是否安装此插件？' }}
          </h2>
          <p :id="`plugin-install-description-${composerDomId}`">
            {{ planApprovalPrompt
              ? 'Claude 已完成计划，请选择下一步。'
              : modelSwitchConfirmPrompt
                ? 'Claude Code 需要确认是否在当前会话中切换模型。'
                : 'Claude Code 请求安装以下插件，请选择一个选项继续。' }}
          </p>
          <code v-if="pluginInstallPrompt">{{ pluginInstallPrompt.pluginName }}</code>
          <p class="plugin-install-dialog__prompt">{{ selectionPrompt.prompt }}</p>
          <p v-if="pluginInstallError" class="workspace-trust-dialog__error" role="alert">
            {{ pluginInstallError }}
          </p>
        </div>
        <div class="workspace-trust-dialog__actions plugin-install-dialog__actions">
          <button
            v-for="(option, index) in selectionPrompt.options"
            :key="`${index}-${option}`"
            class="btn"
            :class="index === (modelSwitchKeyboardIndex ?? modelSwitchConfirmPrompt?.selectedIndex ?? planApprovalPrompt?.selectedIndex ?? 0) ? 'btn-primary' : 'btn-secondary'"
            type="button"
            :title="option"
            :disabled="pluginInstallPending"
            @click="respondToTerminalChoice(index)"
          >
            {{ pluginInstallPending && pluginInstallAction === index ? '处理中…' : option }}
          </button>
        </div>
      </section>
    </div>

    <div
      ref="historyRef"
      class="claude-conversation__history"
      @scroll="handleHistoryScroll"
      @click="handleHistoryClick"
    >
      <div v-if="state.loading" class="claude-conversation__empty">正在连接 Claude 会话…</div>
      <div v-else-if="!state.available" class="claude-conversation__empty">
        <div>当前 Claude 会话没有可用的结构化事件。</div>
        <button class="btn btn-secondary" @click="emit('showTerminal')">打开原始终端</button>
      </div>
      <div v-else-if="state.items.length === 0" class="claude-conversation__empty">
        <div class="claude-conversation__empty-prompt">{{ emptyConversationPrompt }}</div>
      </div>

      <template v-for="item in state.items" :key="item.id">
      <div
        v-if="shouldRenderConversationRow(item)"
        class="conversation-row"
        :class="`conversation-row--${item.kind}`"
      >
        <template v-if="isProcessGroupHeader(item)">
          <button
            class="conversation-process__summary"
            type="button"
            :aria-expanded="isProcessGroupOpen(item)"
            @click="toggleProcessGroup(item)"
          >
            <span>已处理 {{ processGroupDuration(item) }}</span>
            <span class="conversation-process__chevron" aria-hidden="true">&gt;</span>
          </button>
          <div class="conversation-process__divider" aria-hidden="true" />
        </template>
        <article
          v-if="shouldShowConversationItemContent(item)"
          class="conversation-item"
          :class="`conversation-item--${item.kind}`"
        >
        <template v-if="item.kind === 'user' || item.kind === 'assistant'">
          <div
            v-if="item.text"
            class="conversation-item__markdown"
            v-html="renderMarkdown(item.text)"
          />
        </template>

        <template v-else-if="isAskUserQuestionItem(item)">
          <div class="question-card" :class="{ 'is-complete': item.state !== 'waiting' }">
            <div class="question-card__header">
              <div>
                <strong>Claude 问题</strong>
                <span v-if="item.state === 'waiting' && !questionSubmitted[item.id]">请逐题选择</span>
              </div>
              <span class="question-card__state">
                {{ questionCardStateLabel(item) }}
              </span>
            </div>
            <div class="question-card__steps" aria-label="问题进度">
              <span
                v-for="(question, questionIndex) in askUserQuestions(item)"
                :key="`${item.id}-step-${questionIndex}`"
                class="question-card__step"
                :class="{
                  'is-active': questionIndex === activeQuestionIndex(item) && !questionSubmitted[item.id],
                  'is-complete': questionStepComplete(item, questionIndex),
                }"
              >
                <span aria-hidden="true">
                  {{ questionStepComplete(item, questionIndex) ? '✓' : questionIndex + 1 }}
                </span>
                {{ question.header || `问题 ${questionIndex + 1}` }}
              </span>
              <span
                class="question-card__step"
                :class="{ 'is-active': !!questionSubmitted[item.id], 'is-complete': item.state !== 'waiting' }"
              >
                <span aria-hidden="true">{{ item.state !== 'waiting' ? '✓' : '→' }}</span>
                提交
              </span>
            </div>
            <div
              v-for="{ question, questionIndex } in activeAskUserQuestions(item)"
              :key="`${item.id}-${questionIndex}`"
              class="question-card__question"
            >
              <div class="question-card__prompt">
                <span v-if="question.header" class="question-card__header-label">
                  {{ question.header }}
                </span>
                <p>{{ question.question }}</p>
              </div>
              <div class="question-card__options">
                <button
                  v-for="(option, optionIndex) in question.options"
                  :key="`${item.id}-${questionIndex}-${optionIndex}`"
                  class="question-option"
                  :class="{ 'is-selected': isQuestionOptionSelected(item, questionIndex, optionIndex) }"
                  type="button"
                  :disabled="!questionCanAnswer(item)"
                  @click="toggleQuestionOption(item, questionIndex, optionIndex)"
                >
                  <span class="question-option__marker" aria-hidden="true">
                    {{ isQuestionOptionSelected(item, questionIndex, optionIndex) ? '✓' : '' }}
                  </span>
                  <span class="question-option__content">
                    <strong>{{ option.label }}</strong>
                    <small v-if="option.description">{{ option.description }}</small>
                  </span>
                </button>
              </div>
              <label class="question-card__custom-answer">
                <span>其他回答</span>
                <textarea
                  :value="questionCustomAnswer(item, questionIndex)"
                  :disabled="!questionCanAnswer(item)"
                  rows="2"
                  placeholder="输入你的回答"
                  @input="setQuestionCustomAnswer(item, questionIndex, ($event.target as HTMLTextAreaElement).value)"
                  @keydown.shift.tab.stop
                />
              </label>
            </div>
            <div
              v-if="item.state !== 'waiting' || questionSubmitted[item.id]"
              class="question-card__complete-summary"
            >
              {{ item.state === 'failed' ? '问题回复失败' : questionSubmitted[item.id] && item.state === 'waiting'
                ? '回答已发送，正在等待 Claude 继续'
                : `已完成 ${askUserQuestions(item).length} 个问题` }}
            </div>
            <div v-if="item.state === 'waiting'" class="question-card__footer">
              <span v-if="questionErrors[item.id]" class="question-card__error" role="alert">
                {{ questionErrors[item.id] }}
              </span>
              <span v-else-if="questionSubmitted[item.id]" class="question-card__sent">
                已发送
              </span>
              <button
                class="btn btn-primary question-card__submit"
                type="button"
                :disabled="!questionAnswerReady(item) || !!questionSubmitting[item.id] || !!questionSubmitted[item.id]"
                @click="submitQuestionAnswers(item)"
              >
                {{ questionSubmitLabel(item) }}
              </button>
            </div>
          </div>
        </template>

        <template v-else-if="item.kind === 'tool'">
          <details class="tool-card" :open="isToolCardOpen(item)">
            <summary>
              <span class="tool-card__state" :class="`is-${item.state}`" />
              <span class="tool-card__heading">
                <span class="tool-card__name">{{ item.toolName }}</span>
                <code v-if="toolSummary(item)" class="tool-card__preview">
                  {{ toolSummary(item) }}
                </code>
              </span>
              <span class="tool-card__label">{{ toolStateLabel(item.state) }}</span>
              <span class="tool-card__toggle" aria-hidden="true">
                <span class="tool-card__toggle-label" />
                <span class="tool-card__chevron">›</span>
              </span>
            </summary>
            <div v-if="item.toolInput !== undefined" class="tool-card__section">
              <div class="tool-card__section-title">完整输入</div>
              <pre>{{ prettyValue(item.toolInput) }}</pre>
            </div>
            <div v-if="item.toolResult !== undefined" class="tool-card__section">
              <div class="tool-card__section-title">完整结果</div>
              <pre>{{ prettyValue(item.toolResult) }}</pre>
            </div>
          </details>
        </template>

        <template v-else-if="item.kind === 'permission'">
          <div class="permission-card">
            <div>
              <strong>需要在终端确认</strong>
              <p>{{ item.text }}</p>
            </div>
            <button class="btn btn-primary" @click="emit('showTerminal')">前往终端</button>
          </div>
        </template>

        <template v-else>
          <div class="status-card">{{ item.text }}</div>
        </template>
        </article>
        <div
          v-if="item.kind === 'assistant' && item.text && isFinalAssistantMessage(item)"
          class="assistant-message-actions"
        >
          <time>{{ formatTime(item.timestamp) }}</time>
          <button
            class="assistant-message-actions__copy"
            type="button"
            aria-label="复制回复"
            title="复制回复"
            @click="copyAssistantMessage(item.text, $event)"
          >
            <svg class="assistant-message-actions__copy-icon" aria-hidden="true" viewBox="0 0 24 24">
              <rect x="8" y="8" width="13" height="13" rx="2.5" />
              <path d="M16 8V5.5A2.5 2.5 0 0 0 13.5 3h-8A2.5 2.5 0 0 0 3 5.5v8A2.5 2.5 0 0 0 5.5 16H8" />
            </svg>
            <svg class="assistant-message-actions__check-icon" aria-hidden="true" viewBox="0 0 24 24">
              <path d="m5 12.5 4.2 4.2L19 7" />
            </svg>
            <svg class="assistant-message-actions__error-icon" aria-hidden="true" viewBox="0 0 24 24">
              <path d="M7 7l10 10M17 7 7 17" />
            </svg>
          </button>
        </div>
        <div v-if="item.kind === 'user' && item.text" class="user-message-actions">
          <time>{{ formatTime(item.timestamp) }}</time>
          <button
            class="user-message-actions__copy"
            type="button"
            aria-label="复制消息"
            title="复制消息"
            @click="copyUserMessage(item.text, $event)"
          >
            <svg class="user-message-actions__copy-icon" aria-hidden="true" viewBox="0 0 24 24">
              <rect x="8" y="8" width="13" height="13" rx="2.5" />
              <path d="M16 8V5.5A2.5 2.5 0 0 0 13.5 3h-8A2.5 2.5 0 0 0 3 5.5v8A2.5 2.5 0 0 0 5.5 16H8" />
            </svg>
            <svg class="user-message-actions__check-icon" aria-hidden="true" viewBox="0 0 24 24">
              <path d="m5 12.5 4.2 4.2L19 7" />
            </svg>
            <svg class="user-message-actions__error-icon" aria-hidden="true" viewBox="0 0 24 24">
              <path d="M7 7l10 10M17 7 7 17" />
            </svg>
          </button>
        </div>
      </div>
      </template>

    </div>

    <form class="claude-composer" @submit.prevent="submit">
      <div
        v-if="workingActivity || !followLatest"
        class="claude-composer__topbar"
      >
        <div
          v-if="workingActivity"
          class="claude-activity"
          aria-hidden="true"
        >
          <span class="claude-activity__spinner" aria-hidden="true">
            <span>·</span>
            <span>✢</span>
            <span>✽</span>
            <span>✻</span>
            <span>✶</span>
            <span>*</span>
          </span>
          <span class="claude-activity__label">{{ workingActivity.label }}…</span>
          <span v-if="hasActivityDetails" class="claude-activity__details">
            (<template v-if="activityElapsed">{{ activityElapsed }}</template>
            <template v-if="activityElapsed && activityTokenCount"> · </template>
            <template v-if="activityTokenCount">
              {{ activityTokenDirection }}
              <span class="claude-activity__token-count">{{ displayedTokenCount }}</span>
              tokens
            </template>
            <template v-if="(activityElapsed || activityTokenCount) && activityPhase"> · </template>
            <template v-if="activityPhase">{{ activityPhase }}</template>)
          </span>
        </div>
        <button
          v-if="!followLatest"
          class="claude-conversation__jump"
          type="button"
          aria-label="回到最新消息"
          title="回到最新消息"
          @click="resumeFollow"
        >
          <svg aria-hidden="true" focusable="false" viewBox="0 0 24 24">
            <path d="M12 4v14m-6-6 6 6 6-6" />
          </svg>
        </button>
      </div>
      <div
        v-if="state.queuedPrompts.length"
        class="claude-prompt-queue"
        aria-label="等待发送的消息"
        :aria-busy="state.queueActionPending"
      >
        <article
          v-for="(queuedPrompt, index) in state.queuedPrompts"
          :key="queuedPrompt.id"
          class="claude-prompt-queue__item"
        >
          <span class="claude-prompt-queue__index">{{ index + 1 }}</span>
          <span class="claude-prompt-queue__text" :title="queuedPrompt.text">
            {{ queuedPrompt.text }}
          </span>
          <span class="claude-prompt-queue__mode">
            {{ queuedPromptModeLabel(queuedPrompt) }}
          </span>
          <button
            class="claude-prompt-queue__action"
            type="button"
            :disabled="queuedPromptActionDisabled(queuedPrompt)"
            aria-label="立即插入"
            title="立即插入当前处理"
            @click="insertQueuedPromptNow(queuedPrompt.id)"
          >
            <svg aria-hidden="true" viewBox="0 0 24 24">
              <path d="M4 6v4a4 4 0 0 0 4 4h11" />
              <path d="m15 10 4 4-4 4" />
            </svg>
            <span>插入</span>
          </button>
          <button
            class="claude-prompt-queue__action claude-prompt-queue__action--withdraw"
            type="button"
            :disabled="queuedPromptActionDisabled(queuedPrompt)"
            aria-label="撤回到输入框"
            title="撤回到输入框"
            @click="withdrawQueuedPrompt(queuedPrompt.id)"
          >
            <svg aria-hidden="true" viewBox="0 0 24 24">
              <path d="m9 7-5 5 5 5" />
              <path d="M4 12h10a6 6 0 0 1 6 6" />
            </svg>
            <span>撤回</span>
          </button>
        </article>
      </div>
      <div class="claude-composer__input-area">
        <Transition name="claude-command-notice">
          <div
            v-if="commandNotice"
            class="claude-composer__command-notice"
            role="status"
            aria-live="polite"
          >
            {{ commandNotice }}
          </div>
        </Transition>
        <div
          v-if="slashCommandMenuOpen"
          ref="slashCommandMenuRef"
          :id="`${composerDomId}-slash-command-menu`"
          class="claude-composer__command-menu"
          role="listbox"
          aria-label="支持的斜杠命令"
        >
          <div class="claude-composer__command-menu-title">可用命令</div>
          <button
            v-for="(command, index) in filteredSlashCommands"
            :id="`${composerDomId}-slash-command-${index}`"
            :key="command.command"
            class="claude-composer__command-option"
            :class="{ 'is-selected': index === slashCommandIndex }"
            type="button"
            role="option"
            :aria-selected="index === slashCommandIndex"
            @mousedown.prevent
            @mouseenter="slashCommandIndex = index"
            @focus="slashCommandIndex = index"
            @click="selectSlashCommand(command.command)"
          >
            <code>{{ command.command }}</code>
            <span>{{ command.description }}</span>
            <small v-if="command.kind === 'skill'">skill</small>
          </button>
        </div>
        <div class="claude-composer__input-shell" :class="{ 'is-disabled': !canEditInput }">
        <div v-if="attachments.length" class="claude-composer__attachments">
          <ConversationAttachment
            v-for="(att, i) in attachments"
            :key="att.path"
            :path="att.path"
            :name="att.name"
            :is-image="att.isImage"
            :preview-url="att.previewUrl"
            @remove="removeAttachment(i)"
          />
        </div>
        <textarea
          ref="inputRef"
          v-model="prompt"
          :disabled="!canEditInput"
          :placeholder="inputPlaceholder"
          rows="2"
          :aria-controls="slashCommandMenuOpen ? `${composerDomId}-slash-command-menu` : undefined"
          :aria-activedescendant="slashCommandMenuOpen ? `${composerDomId}-slash-command-${slashCommandIndex}` : undefined"
          @keydown="handleComposerKeydown"
          @drop="(e) => e.preventDefault()"
          @paste="onComposerPaste"
        />
        <div class="claude-composer__actions">
          <button
            class="claude-composer__attach"
            type="button"
            :disabled="!canEditInput"
            aria-label="添加文件"
            title="添加文件"
            @click="pickFiles"
          >
            <svg aria-hidden="true" focusable="false" viewBox="0 0 24 24">
              <line x1="12" y1="5" x2="12" y2="19" />
              <line x1="5" y1="12" x2="19" y2="12" />
            </svg>
          </button>
          <span
            class="claude-composer__permission-mode"
            :class="`is-${permissionModeTone}`"
            title="Shift + Tab 切换权限模式"
            aria-live="polite"
          >
            {{ permissionModeLabel }}
          </span>
          <span class="claude-composer__actions-spacer" />
          <span
            v-if="contextUsage"
            class="claude-composer__context-usage"
            :class="`is-${contextUsageTone}`"
            :aria-label="contextUsageTitle"
            role="img"
            aria-live="polite"
          >
            <svg
              class="claude-composer__context-ring"
              viewBox="0 0 20 20"
              aria-hidden="true"
              focusable="false"
            >
              <circle class="claude-composer__context-ring-track" cx="10" cy="10" r="7" />
              <circle
                class="claude-composer__context-ring-value"
                cx="10"
                cy="10"
                r="7"
                transform="rotate(-90 10 10)"
                :stroke-dasharray="contextRingCircumference"
                :stroke-dashoffset="contextRingOffset"
              />
            </svg>
            <span class="claude-composer__context-tooltip" role="tooltip" aria-hidden="true">
              <span class="claude-composer__context-tooltip-main">{{ contextUsageTooltipMain }}</span>
            </span>
          </span>
          <div
            class="claude-composer__model-picker-shell"
          >
            <details
              ref="modelPickerRef"
              class="claude-composer__model-picker"
              :class="{ 'is-disabled': !canSelectModel || modelPreferencePending }"
              @toggle="handleModelPickerToggle"
            >
              <summary
                class="claude-composer__model-trigger"
                :aria-disabled="!canSelectModel || modelPreferencePending"
                :aria-expanded="modelPickerOpen"
                aria-label="选择 Claude 模型"
                title="选择 Claude 模型"
                @click="onModelPickerClick"
              >
                <span class="claude-composer__model-label">{{ currentModelLabel }}</span>
                <svg aria-hidden="true" focusable="false" viewBox="0 0 24 24">
                  <path d="m7 9 5 5 5-5" />
                </svg>
              </summary>
            </details>

            <div
              v-if="modelPickerOpen"
              ref="modelPickerPopoverRef"
              class="claude-composer__model-popover"
            >
              <div
                v-if="activeModelSubmenu === 'model'"
                class="claude-composer__model-submenu"
                role="listbox"
                aria-label="Claude 模型列表"
              >
                <button
                  v-for="model in modelOptions"
                  :key="model"
                  class="claude-composer__model-option"
                  :class="{ 'is-selected': model === baseModel }"
                  type="button"
                  :disabled="modelPreferencePending || model === baseModel"
                  role="option"
                  :aria-selected="model === baseModel"
                  @click="selectModel(model)"
                >
                  <span class="claude-composer__model-option-name">{{ model }}</span>
                  <svg v-if="model === baseModel" aria-hidden="true" focusable="false" viewBox="0 0 24 24">
                    <path d="m5 12.5 4.2 4.2L19 7" />
                  </svg>
                </button>
                <span v-if="modelOptions.length === 0" class="claude-composer__model-empty">
                  请先在 Claude 配置中获取模型
                </span>
              </div>
              <div
                v-else-if="activeModelSubmenu === 'effort'"
                class="claude-composer__model-submenu"
                role="listbox"
                aria-label="推理强度列表"
              >
                <button
                  v-for="effort in effortOptions"
                  :key="effort"
                  class="claude-composer__model-option"
                  :class="{ 'is-selected': effort === selectedEffort }"
                  type="button"
                  :disabled="modelPreferencePending || effort === selectedEffort"
                  role="option"
                  :aria-selected="effort === selectedEffort"
                  @click="selectEffort(effort)"
                >
                  <span class="claude-composer__model-option-name">{{ effort }}</span>
                  <svg v-if="effort === selectedEffort" aria-hidden="true" focusable="false" viewBox="0 0 24 24">
                    <path d="m5 12.5 4.2 4.2L19 7" />
                  </svg>
                </button>
              </div>
              <div
                v-else-if="activeModelSubmenu === 'context'"
                class="claude-composer__model-submenu"
                role="listbox"
                aria-label="上下文长度列表"
              >
                <button
                  v-for="context in contextOptions"
                  :key="context"
                  class="claude-composer__model-option"
                  :class="{ 'is-selected': context === selectedContext }"
                  type="button"
                  :disabled="modelPreferencePending || context === selectedContext"
                  role="option"
                  :aria-selected="context === selectedContext"
                  @click="selectContext(context)"
                >
                  <span class="claude-composer__model-option-name">{{ context }}</span>
                  <svg v-if="context === selectedContext" aria-hidden="true" focusable="false" viewBox="0 0 24 24">
                    <path d="m5 12.5 4.2 4.2L19 7" />
                  </svg>
                </button>
              </div>

              <div class="claude-composer__model-menu" aria-label="Claude 模型设置">
              <button
                class="claude-composer__model-group-trigger"
                :class="{ 'is-active': activeModelSubmenu === 'model' }"
                type="button"
                :aria-expanded="activeModelSubmenu === 'model'"
                @mouseenter="openModelSubmenu('model')"
                @click="openModelSubmenu('model')"
              >
                <span>模型选择</span>
                <span class="claude-composer__model-group-value">{{ baseModelLabel }}</span>
                <svg aria-hidden="true" focusable="false" viewBox="0 0 24 24">
                  <path d="m7 9 5 5 5-5" />
                </svg>
              </button>

              <button
                class="claude-composer__model-group-trigger"
                :class="{ 'is-active': activeModelSubmenu === 'effort' }"
                type="button"
                :aria-expanded="activeModelSubmenu === 'effort'"
                @mouseenter="openModelSubmenu('effort')"
                @click="openModelSubmenu('effort')"
              >
                <span>推理强度</span>
                <span class="claude-composer__model-group-value">{{ effortLabel }}</span>
                <svg aria-hidden="true" focusable="false" viewBox="0 0 24 24">
                  <path d="m7 9 5 5 5-5" />
                </svg>
              </button>

              <button
                class="claude-composer__model-group-trigger"
                :class="{ 'is-active': activeModelSubmenu === 'context' }"
                type="button"
                :aria-expanded="activeModelSubmenu === 'context'"
                @mouseenter="openModelSubmenu('context')"
                @click="openModelSubmenu('context')"
              >
                <span>上下文长度</span>
                <span class="claude-composer__model-group-value">{{ selectedContext }}</span>
                <svg aria-hidden="true" focusable="false" viewBox="0 0 24 24">
                  <path d="m7 9 5 5 5-5" />
                </svg>
              </button>

              <button
                class="claude-composer__model-reset"
                type="button"
                :disabled="modelPreferencePending"
                @click="resetModelPreferences"
                @mouseenter="activeModelSubmenu = null"
              >
                重置默认选项
              </button>
            </div>
          </div>
          </div>
          <button
            v-if="isWorking"
            class="claude-composer__send is-ready is-stop"
            :class="{ 'is-pending': stopPending }"
            type="button"
            :disabled="stopPending"
            aria-label="停止生成"
            title="停止生成"
            @click="stopRun"
          >
            <svg aria-hidden="true" focusable="false" viewBox="0 0 24 24">
              <rect x="7" y="7" width="10" height="10" rx="1" fill="currentColor" stroke="none" />
            </svg>
          </button>
          <button
            v-else
            class="claude-composer__send"
            :class="{ 'is-ready': canSubmit && !!(prompt.trim() || attachments.length) }"
            type="submit"
            :disabled="!canSubmit || !(prompt.trim() || attachments.length)"
            aria-label="发送消息"
            title="发送消息"
          >
            <svg aria-hidden="true" focusable="false" viewBox="0 0 24 24">
              <path d="M20 5v6a4 4 0 0 1-4 4H5" />
              <path d="m9 11-4 4 4 4" />
            </svg>
          </button>
        </div>
      </div>
      </div>
      <div v-if="submitError || externalError" class="claude-composer__footer">
        <span class="claude-composer__error">
          {{ submitError || externalError }}
        </span>
      </div>
    </form>
  </section>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { open as openFileDialog } from '@tauri-apps/plugin-dialog'
import { invoke } from '@tauri-apps/api/core'
import { useClaudeObserverStore } from '@/stores/claudeObserver'
import { useClaudeStore } from '@/stores/claude'
import { useProjectStore } from '@/stores/project'
import { resolveClaudeWorkspaceTrustAction } from '@/utils/claudeWorkspaceTrust'
import type {
  ClaudeConversationItem,
  ClaudeConversationState,
  ClaudeQueuedPrompt,
} from '@/types/claudeObserver'
import { copyTextToClipboard } from '@/utils/clipboard'
import {
  getClaudeConversationScroll,
  saveClaudeConversationScroll,
} from '@/utils/claudeConversationScroll'
import { createClaudeMarkdownRenderer } from '@/utils/claudeMarkdown'
import { getClaudeComposerTextareaMetrics } from '@/utils/claudeComposerSizing'
import { CLAUDE_PROMPT_QUEUE_LIMIT } from '@/utils/claudePromptQueue'
import {
  filterClaudeSlashCommands,
  setClaudeSkillCommands,
  validateClaudeSlashCommand,
} from '@/utils/claudeSlashCommands'
import {
  parseClaudeAskUserQuestions,
  type ClaudeAskUserQuestion,
} from '@/utils/claudeQuestion'
import { summarizeClaudeTool } from '@/utils/claudeToolPresentation'
import {
  MAX_CONVERSATION_IMAGE_ATTACHMENTS,
  arrayBufferToBase64,
  basename,
  imageMimeToExtension,
  isImagePath,
  partitionPathsByImageLimit,
  formatConversationDropPath,
} from '@/utils/claudeFileDrop'
import ConversationAttachment from './ConversationAttachment.vue'

interface FileAttachment {
  path: string
  name: string
  isImage: boolean
  previewUrl: string | null
}

const props = defineProps<{
  tabId?: number | null
  projectId: string
  projectName: string
  sessionId: string
  startupMode?: boolean
  startupPending?: boolean
  initialPermissionMode?: ClaudeDefaultPermissionMode
  externalError?: string
  sessionDraft?: string
  sessionAttachmentPaths?: string[]
  clearSessionDraft?: (projectId: string, sessionId: string) => void
  restoreSessionDraft?: (projectId: string, sessionId: string, prompt: string) => void
  submitStartupPrompt?: (prompt: string) => Promise<boolean>
}>()
const emit = defineEmits<{
  showTerminal: []
  'update:initialPermissionMode': [mode: ClaudeDefaultPermissionMode]
  'update:sessionDraft': [prompt: string]
  'update:sessionAttachmentPaths': [paths: string[]]
}>()
const observerStore = useClaudeObserverStore()
const claudeStore = useClaudeStore()
const projectStore = useProjectStore()
const historyRef = ref<HTMLElement | null>(null)
const inputRef = ref<HTMLTextAreaElement | null>(null)
const slashCommandMenuRef = ref<HTMLElement | null>(null)
const modelPickerRef = ref<HTMLDetailsElement | null>(null)
const modelPickerPopoverRef = ref<HTMLElement | null>(null)
const workspaceTrustOverlayRef = ref<HTMLElement | null>(null)
const workspaceTrustCancelRef = ref<HTMLButtonElement | null>(null)
const workspaceTrustConfirmRef = ref<HTMLButtonElement | null>(null)
const pluginInstallOverlayRef = ref<HTMLElement | null>(null)
const prompt = ref(props.sessionDraft ?? '')
const submitError = ref('')
const stopPending = ref(false)
type ClaudeDefaultPermissionMode = 'bypassPermissions' | 'auto' | 'default' | 'acceptEdits' | 'plan'
const startupPermissionMode = ref<ClaudeDefaultPermissionMode>('auto')
let startupPermissionModeWrite: Promise<void> = Promise.resolve()
let startupPermissionModeRevision = 0
const followLatest = ref(true)
const copyAnnouncement = ref('')
const workspaceTrustPending = ref(false)
const workspaceTrustAction = ref<'confirm' | 'cancel' | null>(null)
const workspaceTrustError = ref('')
const pluginInstallPending = ref(false)
const pluginInstallAction = ref<number | null>(null)
const pluginInstallError = ref('')
const modelSwitchKeyboardIndex = ref<number | null>(null)
let terminalModelActionQueue: Promise<void> = Promise.resolve()
const slashCommandIndex = ref(0)
const slashCommandMenuDismissed = ref(false)
const commandNotice = ref('')
let commandNoticeTimer: number | undefined
const isDragOver = ref(false)
const attachments = ref<FileAttachment[]>([])
const questionSelections = ref<Record<string, number[]>>({})
const questionCustomAnswers = ref<Record<string, string>>({})
const questionActiveIndexes = ref<Record<string, number>>({})
const questionSubmitting = ref<Record<string, boolean>>({})
const questionSubmitted = ref<Record<string, boolean>>({})
const questionErrors = ref<Record<string, string>>({})
const modelPickerOpen = ref(false)
const modelPreferencePending = ref(false)
type ModelSubmenu = 'model' | 'effort' | 'context'
const activeModelSubmenu = ref<ModelSubmenu | null>(null)

const activeProject = computed(() =>
  projectStore.projects.find((p) => p.id === props.projectId) ?? null,
)
const projectPath = computed(() => activeProject.value?.path ?? null)
interface ClaudeSkillEntry {
  name: string
  description: string
}
const slashCommandCatalogRevision = ref(0)
let skillLoadSequence = 0

async function refreshClaudeSlashCommands() {
  const sequence = ++skillLoadSequence
  try {
    const skills = await invoke<ClaudeSkillEntry[]>('list_claude_skills', {
      projectPath: projectPath.value,
    })
    if (sequence !== skillLoadSequence) return
    setClaudeSkillCommands(skills)
  } catch {
    if (sequence !== skillLoadSequence) return
    setClaudeSkillCommands([])
  } finally {
    if (sequence === skillLoadSequence) slashCommandCatalogRevision.value += 1
  }
}
const dropPathMode = computed(() => claudeStore.projectDropPathMode ?? 'relative')

function buildAttachment(path: string): FileAttachment {
  const name = basename(path)
  const isImage = isImagePath(path)
  return { path, name, isImage, previewUrl: null }
}

async function loadImagePreview(att: FileAttachment) {
  if (!att.isImage) return
  try {
    att.previewUrl = await invoke<string>('read_image_base64', { path: att.path })
  } catch {
    // preview stays null
  }
}

function currentImageCount() {
  return attachments.value.filter((a) => a.isImage).length
}

function addDroppedPaths(paths: string[]) {
  for (const path of paths) {
    if (!attachments.value.some((a) => a.path === path)) {
      const att = buildAttachment(path)
      attachments.value.push(att)
      void loadImagePreview(att)
    }
  }
}

function addDroppedPathsWithImageLimit(paths: string[]) {
  const { accepted, rejected } = partitionPathsByImageLimit(paths, currentImageCount())
  if (accepted.length) addDroppedPaths(accepted)
  if (rejected.length) {
    submitError.value = `最多同时添加 ${MAX_CONVERSATION_IMAGE_ATTACHMENTS} 张图片`
  }
}

function onSectionDragLeave(event: DragEvent) {
  const related = event.relatedTarget
  if (related instanceof Node && (event.currentTarget as Element)?.contains(related)) return
  isDragOver.value = false
}

function onSectionDrop(event: DragEvent) {
  isDragOver.value = false
  const files = event.dataTransfer?.files
  if (!files || files.length === 0) return
  const paths: string[] = []
  for (let i = 0; i < files.length; i++) {
    const f = files[i] as File & { path?: string }
    if (f.path) paths.push(f.path)
  }
  if (paths.length) addDroppedPathsWithImageLimit(paths)
}

function appendDroppedFiles(paths: string[]) {
  addDroppedPaths(paths)
}

function removeAttachment(index: number) {
  attachments.value.splice(index, 1)
}

async function pickFiles() {
  try {
    const selected = await openFileDialog({ multiple: true })
    if (!selected) return
    const paths = Array.isArray(selected) ? selected : [selected]
    addDroppedPathsWithImageLimit(paths)
  } catch {
    // user cancelled
  }
}

let pastedImageSequence = 0

async function onComposerPaste(event: ClipboardEvent) {
  const items = event.clipboardData?.items
  if (!items) return
  const imageItems = Array.from(items).filter((item) => item.type.startsWith('image/'))
  if (imageItems.length === 0) return
  event.preventDefault()

  const remaining = Math.max(0, MAX_CONVERSATION_IMAGE_ATTACHMENTS - currentImageCount())
  const acceptedItems = imageItems.slice(0, remaining)
  if (imageItems.length > acceptedItems.length) {
    submitError.value = `最多同时添加 ${MAX_CONVERSATION_IMAGE_ATTACHMENTS} 张图片`
  }

  for (const item of acceptedItems) {
    const extension = imageMimeToExtension(item.type)
    if (!extension) continue
    const file = item.getAsFile()
    if (!file) continue
    try {
      const buffer = await file.arrayBuffer()
      const dataBase64 = arrayBufferToBase64(buffer)
      const path = await invoke<string>('save_pasted_image', { dataBase64, extension })
      if (attachments.value.some((a) => a.path === path)) continue
      attachments.value.push({
        path,
        name: `粘贴图片${++pastedImageSequence}.${extension}`,
        isImage: true,
        previewUrl: `data:${item.type};base64,${dataBase64}`,
      })
    } catch (error) {
      submitError.value = error instanceof Error ? error.message : String(error)
    }
  }
}

const EMPTY_PROMPT_TEMPLATES = [
  (projectName: string) => `我们在 ${projectName} 中构建什么？`,
  (projectName: string) => `我们应该在 ${projectName} 中做些什么？`,
  (projectName: string) => `想给 ${projectName} 加些什么新功能？`,
  (projectName: string) => `让我们在 ${projectName} 中继续创造吧！`,
]
const emptyPromptTemplate = EMPTY_PROMPT_TEMPLATES[
  Math.floor(Math.random() * EMPTY_PROMPT_TEMPLATES.length)
]
const emptyConversationPrompt = computed(() => emptyPromptTemplate(props.projectName))

const markdown = createClaudeMarkdownRenderer()
const copyFeedbackTimers = new WeakMap<HTMLButtonElement, number>()
const copyFeedbackTimerIds = new Set<number>()
let copyAnnouncementTimer: number | undefined
let copyAnnouncementSequence = 0
let disposed = false
let scrollInitialized = false
let scrollLoadGeneration = 0
let workspaceTrustPreviousFocus: HTMLElement | null = null
let restoreComposerFocusWhenReady = false
let composerResizeObserver: ResizeObserver | null = null
let observedComposerWidth: number | null = null
const composerDomId = computed(() => props.tabId ?? props.sessionId.replace(/[^a-zA-Z0-9_-]/g, '_'))
const state = computed<ClaudeConversationState>(() => {
  if (props.startupMode) {
    return {
      tabId: 0,
      statusRevision: 0,
      available: true,
      active: true,
      sessionReady: true,
      runState: props.startupPending ? 'starting' : 'idle',
      items: [],
      terminalLog: '',
      loading: false,
      activityStatus: undefined,
      compactCompletionRevision: 0,
      queuedPrompts: [],
      queueActionPending: false,
    }
  }
  const tabId = props.tabId ?? 0
  return observerStore.states[tabId] ?? {
    tabId,
    statusRevision: 0,
    available: false,
    active: true,
    sessionReady: false,
    runState: 'starting',
    items: [],
    terminalLog: '',
    loading: true,
    activityStatus: undefined,
    compactCompletionRevision: 0,
    queuedPrompts: [],
    queueActionPending: false,
  }
})

type ConversationProcessGroup = {
  id: string
  itemIds: Set<string>
  firstItemId: string
  startedAt: string
  endedAt: string
  turn: number
}

const processGroups = computed<ConversationProcessGroup[]>(() => {
  const groups: ConversationProcessGroup[] = []
  let activeGroup: ConversationProcessGroup | null = null
  let turn = 0
  let turnStartedAt: string | null = null

  for (let index = 0; index < state.value.items.length; index++) {
    const item = state.value.items[index]
    if (item.kind === 'user') {
      turn += 1
      turnStartedAt = item.timestamp
      activeGroup = null
      continue
    }
    if (item.kind !== 'tool' || isAskUserQuestionItem(item)) {
      activeGroup = null
      continue
    }
    if (!activeGroup || activeGroup.turn !== turn) {
      activeGroup = {
        id: `process-${item.id}`,
        itemIds: new Set(),
        firstItemId: item.id,
        startedAt: turnStartedAt ?? item.timestamp,
        endedAt: item.timestamp,
        turn,
      }
      groups.push(activeGroup)
    }
    activeGroup.itemIds.add(item.id)
    activeGroup.endedAt = item.timestamp
  }

  for (const group of groups) {
    const groupItemIds = [...group.itemIds]
    const lastItemId = groupItemIds[groupItemIds.length - 1]
    const lastItemIndex = state.value.items.findIndex(item => item.id === lastItemId)
    const nextAssistant = state.value.items.slice(lastItemIndex + 1)
      .find(item => item.kind === 'assistant')
    if (nextAssistant) group.endedAt = nextAssistant.timestamp
  }
  return groups
})
const expandedProcessGroupIds = ref<Set<string>>(new Set())

function processGroupForItem(item: ClaudeConversationItem) {
  return processGroups.value.find(group => group.itemIds.has(item.id))
}

function isActiveProcessGroup(group: ConversationProcessGroup) {
  const latestUserIndex = state.value.items.map(item => item.kind).lastIndexOf('user')
  const latestTurn = state.value.items
    .slice(0, latestUserIndex + 1)
    .filter(item => item.kind === 'user')
    .length
  return group.turn === latestTurn && (
    state.value.runState === 'working' || state.value.runState === 'permission'
  )
}

function isProcessGroupOpen(item: ClaudeConversationItem) {
  const group = processGroupForItem(item)
  return !!group && (
    isActiveProcessGroup(group) || expandedProcessGroupIds.value.has(group.id)
  )
}

function isProcessGroupHeader(item: ClaudeConversationItem) {
  const group = processGroupForItem(item)
  return !!group && !isActiveProcessGroup(group) && item.id === group.firstItemId
}

function shouldRenderConversationRow(item: ClaudeConversationItem) {
  const group = processGroupForItem(item)
  return !group || isProcessGroupOpen(item) || item.id === group.firstItemId
}

function shouldShowConversationItemContent(item: ClaudeConversationItem) {
  const group = processGroupForItem(item)
  return !group || isProcessGroupOpen(item)
}

function isToolCardOpen(item: ClaudeConversationItem) {
  return isProcessGroupOpen(item) || item.state === 'running' || item.state === 'failed'
}

function toggleProcessGroup(item: ClaudeConversationItem) {
  const group = processGroupForItem(item)
  if (!group || isActiveProcessGroup(group)) return
  const next = new Set(expandedProcessGroupIds.value)
  if (next.has(group.id)) next.delete(group.id)
  else next.add(group.id)
  expandedProcessGroupIds.value = next
}

function processGroupDuration(item: ClaudeConversationItem) {
  const group = processGroupForItem(item)
  if (!group) return ''
  const startedAt = new Date(group.startedAt).getTime()
  const endedAt = new Date(group.endedAt).getTime()
  const durationSeconds = Number.isFinite(startedAt) && Number.isFinite(endedAt)
    ? Math.max(1, Math.round((endedAt - startedAt) / 1_000))
    : 1
  const minutes = Math.floor(durationSeconds / 60)
  const seconds = durationSeconds % 60
  return minutes > 0 ? `${minutes}m ${seconds}s` : `${seconds}s`
}

const workspaceTrustPrompt = computed(() => (
  state.value.terminalPrompt?.kind === 'workspaceTrust'
    ? state.value.terminalPrompt
    : null
))
const pluginInstallPrompt = computed(() => (
  state.value.terminalPrompt?.kind === 'pluginInstall'
    ? state.value.terminalPrompt
    : null
))
const modelSwitchConfirmPrompt = computed(() => (
  state.value.terminalPrompt?.kind === 'modelSwitchConfirm'
    ? state.value.terminalPrompt
    : null
))
const planApprovalPrompt = computed(() => (
  state.value.terminalPrompt?.kind === 'planApproval'
    ? state.value.terminalPrompt
    : null
))
const selectionPrompt = computed(() => (
  pluginInstallPrompt.value ?? modelSwitchConfirmPrompt.value ?? planApprovalPrompt.value
))
const terminalPrompt = computed(() => !!state.value.terminalPrompt)

const pendingQuestion = computed(() => state.value.items.some(item => (
  isAskUserQuestionItem(item) && item.state === 'waiting'
)))

const statusLabel = computed(() => {
  if (pendingQuestion.value) return 'Claude is waiting for your choices'
  switch (state.value.runState) {
    case 'idle': return '等待输入'
    case 'working': return 'Claude 正在处理'
    case 'permission': return '等待终端确认'
    case 'stopped': return '会话已结束'
    default: return '正在连接'
  }
})

const workingActivity = computed(() => {
  if (state.value.runState !== 'working') return null
  const activity = state.value.activityStatus
  if (!activity) return { label: 'Thinking' }
  const label = activity.label.replace(/[….]+$/u, '')
  return {
    label: /^compacting conversation$/i.test(label) ? '正在压缩上下文' : label || 'Thinking',
    elapsed: activity.elapsed,
    tokenDirection: activity.tokenDirection,
    tokenCount: activity.tokenCount,
    phase: activity.phase,
  }
})

const activityElapsed = computed(() => workingActivity.value?.elapsed ?? '')
const activityTokenDirection = computed(() => workingActivity.value?.tokenDirection ?? '')
const activityTokenCount = computed(() => workingActivity.value?.tokenCount ?? '')
const activityPhase = computed(() => workingActivity.value?.phase ?? '')
const hasActivityDetails = computed(() => !!(
  activityElapsed.value || activityTokenCount.value || activityPhase.value
))

type TokenCountFormat = {
  multiplier: number
  fractionDigits: number
  suffix: string
  useGrouping: boolean
}

function parseTokenCount(value: string): { value: number, format: TokenCountFormat } | null {
  const match = value.trim().match(/^([\d,]+(?:\.\d+)?)([kKmM]?)$/)
  if (!match) return null
  const numeric = Number(match[1].replaceAll(',', ''))
  if (!Number.isFinite(numeric)) return null
  const suffix = match[2]
  const multiplier = suffix.toLowerCase() === 'k' ? 1_000 : suffix.toLowerCase() === 'm' ? 1_000_000 : 1
  return {
    value: numeric * multiplier,
    format: {
      multiplier,
      fractionDigits: match[1].split('.')[1]?.length ?? 0,
      suffix,
      useGrouping: match[1].includes(','),
    },
  }
}

function formatTokenCount(value: number, format: TokenCountFormat) {
  const scaled = value / format.multiplier
  if (format.suffix) return `${scaled.toFixed(format.fractionDigits)}${format.suffix}`
  if (format.fractionDigits > 0) return scaled.toFixed(format.fractionDigits)
  const rounded = Math.round(scaled)
  return format.useGrouping ? rounded.toLocaleString('en-US') : String(rounded)
}

const animatedTokenValue = ref<number | null>(null)
const animatedTokenFormat = ref<TokenCountFormat | null>(null)
const displayedTokenCount = computed(() => {
  const tokenCount = activityTokenCount.value
  if (!tokenCount || animatedTokenValue.value === null || !animatedTokenFormat.value) return tokenCount
  return formatTokenCount(animatedTokenValue.value, animatedTokenFormat.value)
})
let tokenAnimationFrame: number | undefined

function stopTokenAnimation() {
  if (tokenAnimationFrame === undefined) return
  window.cancelAnimationFrame(tokenAnimationFrame)
  tokenAnimationFrame = undefined
}

function animateTokenCount(nextTokenCount: string) {
  const parsed = parseTokenCount(nextTokenCount)
  stopTokenAnimation()
  if (!parsed) {
    animatedTokenValue.value = null
    animatedTokenFormat.value = null
    return
  }

  const start = animatedTokenValue.value
  animatedTokenFormat.value = parsed.format
  if (
    start === null
    || window.matchMedia('(prefers-reduced-motion: reduce)').matches
    || start === parsed.value
  ) {
    animatedTokenValue.value = parsed.value
    return
  }

  const startedAt = performance.now()
  const duration = 420
  const tick = (now: number) => {
    const progress = Math.min(1, (now - startedAt) / duration)
    const easedProgress = 1 - (1 - progress) ** 3
    animatedTokenValue.value = start + (parsed.value - start) * easedProgress
    if (progress < 1) tokenAnimationFrame = window.requestAnimationFrame(tick)
    else tokenAnimationFrame = undefined
  }
  tokenAnimationFrame = window.requestAnimationFrame(tick)
}

const activityAnnouncement = computed(() => (
  workingActivity.value?.label === '正在压缩上下文'
    ? 'Claude 正在压缩上下文'
    : state.value.runState === 'working' ? 'Claude 正在处理' : ''
))

const canEditInput = computed(() =>
  state.value.available
  && state.value.active
  && state.value.sessionReady
  && (state.value.runState === 'idle' || state.value.runState === 'working')
  && !state.value.terminalPrompt
  && !props.startupPending
)

const canSubmit = computed(() => (
  canEditInput.value
  && (
    state.value.runState !== 'working'
    || state.value.queuedPrompts.length < CLAUDE_PROMPT_QUEUE_LIMIT
  )
))

const filteredSlashCommands = computed(() => {
  slashCommandCatalogRevision.value
  return filterClaudeSlashCommands(prompt.value)
})
const slashCommandMenuOpen = computed(() => (
  canEditInput.value
  && !slashCommandMenuDismissed.value
  && filteredSlashCommands.value.length > 0
))
const isWorking = computed(() => state.value.runState === 'working')
const STARTUP_PERMISSION_MODE_LABELS: Record<ClaudeDefaultPermissionMode, string> = {
  bypassPermissions: '⏵⏵ bypass permissions',
  auto: '⏵⏵ auto mode',
  default: '⏸ manual mode',
  acceptEdits: '⏵⏵ accept edits',
  plan: '⏸ plan mode',
}
const STARTUP_PERMISSION_MODE_CYCLE: ClaudeDefaultPermissionMode[] = [
  'bypassPermissions',
  'auto',
  'default',
  'acceptEdits',
  'plan',
]
function permissionModeFromLabel(label: string | undefined | null): ClaudeDefaultPermissionMode | null {
  const normalized = label?.toLowerCase() ?? ''
  if (normalized.includes('bypass permissions')) return 'bypassPermissions'
  if (normalized.includes('auto mode')) return 'auto'
  if (normalized.includes('manual mode') || normalized.includes('default mode')) return 'default'
  if (normalized.includes('auto-accept edits') || normalized.includes('accept edits')) return 'acceptEdits'
  if (normalized.includes('plan mode')) return 'plan'
  return null
}
const permissionModeLabel = computed(() => {
  if (props.startupMode) {
    return STARTUP_PERMISSION_MODE_LABELS[startupPermissionMode.value] ?? STARTUP_PERMISSION_MODE_LABELS.auto
  }
  const observedLabel = state.value.permissionMode?.trim()
  const observedMode = permissionModeFromLabel(observedLabel)
  if (state.value.pendingPermissionMode) {
    return STARTUP_PERMISSION_MODE_LABELS[state.value.pendingPermissionMode]
  }
  if (observedMode) return observedLabel!
  return STARTUP_PERMISSION_MODE_LABELS[props.initialPermissionMode ?? 'auto']
})
const permissionModeTone = computed(() => {
  const mode = permissionModeLabel.value.toLowerCase()
  if (mode.includes('bypass permissions')) return 'bypass'
  if (mode.includes('auto mode')) return 'auto'
  if (mode.includes('accept edits')) return 'accept-edits'
  if (mode.includes('plan mode')) return 'plan'
  return 'manual'
})

function handleSlashCommandMenuOutsidePointerDown(event: PointerEvent) {
  if (!slashCommandMenuOpen.value) return
  const target = event.target
  if (!(target instanceof Node) || slashCommandMenuRef.value?.contains(target)) return
  slashCommandMenuDismissed.value = true
}

function showCommandNotice(message: string) {
  commandNotice.value = message
  if (commandNoticeTimer !== undefined) window.clearTimeout(commandNoticeTimer)
  commandNoticeTimer = window.setTimeout(() => {
    commandNotice.value = ''
    commandNoticeTimer = undefined
  }, 2_000)
}

async function selectSlashCommand(command: string) {
  prompt.value = command
  slashCommandIndex.value = 0
  await nextTick()
  resizeComposerToContent()
  await submit()
}

function scrollSelectedSlashCommandIntoView() {
  void nextTick(() => {
    const selected = slashCommandMenuRef.value
      ?.querySelector<HTMLElement>('.claude-composer__command-option.is-selected')
    selected?.scrollIntoView({ block: 'nearest' })
  })
}

function handleComposerKeydown(event: KeyboardEvent) {
  if (event.isComposing) return

  if (
    event.key === 'Tab'
    && event.shiftKey
    && !event.altKey
    && !event.ctrlKey
    && !event.metaKey
  ) {
    event.preventDefault()
    if (canEditInput.value && (props.startupMode || (props.tabId !== null && props.tabId !== undefined))) {
      void cyclePermissionMode()
    }
    return
  }

  if (
    slashCommandMenuOpen.value
    && !event.altKey
    && !event.ctrlKey
    && !event.metaKey
    && !event.shiftKey
  ) {
    if (event.key === 'ArrowUp' || event.key === 'ArrowDown') {
      event.preventDefault()
      const count = filteredSlashCommands.value.length
      const step = event.key === 'ArrowUp' ? -1 : 1
      slashCommandIndex.value = (slashCommandIndex.value + step + count) % count
      scrollSelectedSlashCommandIntoView()
      return
    }
    if (event.key === 'Enter') {
      event.preventDefault()
      const selected = filteredSlashCommands.value[slashCommandIndex.value]
      if (selected) void selectSlashCommand(selected.command)
      return
    }
  }

  if (
    event.key === 'Enter'
    && !event.altKey
    && !event.ctrlKey
    && !event.metaKey
    && !event.shiftKey
  ) {
    event.preventDefault()
    void submit()
  }
}

const configuredModel = computed(() => (
  claudeStore.editingConfig.vars['ANTHROPIC_MODEL']?.trim() ?? ''
))

const effortOptions = ['auto', 'low', 'medium', 'high', 'xhigh', 'max']
const contextOptions = ['200k', '1m'] as const
type ContextLength = typeof contextOptions[number]

function stripContextSuffix(model: string) {
  return model.trim().replace(/\[1m\]$/i, '').trim()
}

function composeModelName(model: string, context: ContextLength) {
  const base = stripContextSuffix(model)
  return context === '1m' && base ? `${base}[1m]` : base
}

const baseModel = computed(() => stripContextSuffix(configuredModel.value))

const baseModelLabel = computed(() => baseModel.value || '默认模型')

const currentModelLabel = computed(() => (
  state.value.currentModel?.trim() || configuredModel.value || '默认模型'
))

const contextUsage = computed(() => props.startupMode ? undefined : state.value.contextUsage)

const contextUsageTone = computed(() => {
  const percentage = contextUsage.value?.usedPercentage ?? 0
  if (percentage >= 85) return 'critical'
  if (percentage >= 70) return 'warning'
  return 'normal'
})

function formatContextTokens(value: number) {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1).replace(/\.0$/, '')}M`
  if (value >= 1_000) return `${(value / 1_000).toFixed(1).replace(/\.0$/, '')}k`
  return String(value)
}

const contextUsageTitle = computed(() => {
  const usage = contextUsage.value
  if (!usage) return ''
  const details = usage.usedTokens != null && usage.contextWindowSize != null
    ? `已使用 ${formatContextTokens(usage.usedTokens)} / ${formatContextTokens(usage.contextWindowSize)}`
    : `已使用 ${usage.usedPercentage}%`
  const source = usage.source === 'native' ? 'Claude 原生数据' : '根据会话记录估算'
  return `${details}，剩余 ${usage.remainingPercentage}% · ${source}`
})

const CONTEXT_RING_RADIUS = 7
const contextRingCircumference = 2 * Math.PI * CONTEXT_RING_RADIUS

const contextRingOffset = computed(() => {
  const percentage = Math.min(100, Math.max(0, contextUsage.value?.usedPercentage ?? 0))
  return contextRingCircumference * (1 - percentage / 100)
})

const contextUsageTooltipMain = computed(() => {
  const usage = contextUsage.value
  if (!usage) return ''
  if (usage.usedTokens != null && usage.contextWindowSize != null) {
    const used = formatContextTokens(usage.usedTokens)
    const total = formatContextTokens(usage.contextWindowSize)
    return `已用${usage.usedPercentage}% ${used}/${total}`
  }
  return `已用${usage.usedPercentage}%`
})

const selectedEffort = computed(() => {
  const effort = claudeStore.editingConfig.vars['CLAUDE_CODE_EFFORT_LEVEL']?.trim() ?? ''
  return effortOptions.includes(effort) ? effort : 'auto'
})

const effortLabel = computed(() => selectedEffort.value)

const selectedContext = computed<ContextLength>(() => (
  state.value.currentContext
  ?? (/\[1m\]$/i.test(configuredModel.value) ? '1m' : '200k')
))

const modelOptions = computed(() => Array.from(new Set([
  ...(baseModel.value ? [baseModel.value] : []),
  ...claudeStore.availableModels.map(stripContextSuffix),
].filter(Boolean))))

const canSelectModel = computed(() => (
  canEditInput.value
  && state.value.runState === 'idle'
  && !pendingQuestion.value
))

function onModelPickerClick(event: MouseEvent) {
  if (canSelectModel.value && !modelPreferencePending.value) return
  event.preventDefault()
}

async function handleModelPickerToggle() {
  modelPickerOpen.value = modelPickerRef.value?.open ?? false
  if (!modelPickerOpen.value) activeModelSubmenu.value = null
  if (!modelPickerOpen.value || claudeStore.availableModels.length || claudeStore.modelsFetching) return
  await claudeStore.fetchModels()
}

function openModelSubmenu(submenu: ModelSubmenu) {
  if (!canSelectModel.value || modelPreferencePending.value) return
  activeModelSubmenu.value = submenu
}

function closeModelPicker() {
  if (!modelPickerOpen.value && activeModelSubmenu.value === null) return
  modelPickerRef.value?.removeAttribute('open')
  modelPickerOpen.value = false
  activeModelSubmenu.value = null
}

function handleModelPickerOutsidePointerDown(event: PointerEvent) {
  const target = event.target
  if (!(target instanceof Node)) return
  if (modelPickerRef.value?.contains(target) || modelPickerPopoverRef.value?.contains(target)) return
  closeModelPicker()
}

async function persistModelPreferences(
  model: string,
  effort: string,
  context: ContextLength,
): Promise<boolean> {
  if (!canSelectModel.value || modelPreferencePending.value) return false

  const previousModel = claudeStore.editingConfig.vars['ANTHROPIC_MODEL'] ?? ''
  const previousEffort = claudeStore.editingConfig.vars['CLAUDE_CODE_EFFORT_LEVEL'] ?? ''
  const nextModel = composeModelName(model, context)
  if (nextModel === previousModel && effort === previousEffort) return true

  modelPreferencePending.value = true
  submitError.value = ''
  try {
    if (nextModel) claudeStore.editingConfig.vars['ANTHROPIC_MODEL'] = nextModel
    else delete claudeStore.editingConfig.vars['ANTHROPIC_MODEL']
    if (effort) claudeStore.editingConfig.vars['CLAUDE_CODE_EFFORT_LEVEL'] = effort
    else delete claudeStore.editingConfig.vars['CLAUDE_CODE_EFFORT_LEVEL']

    const saved = await claudeStore.saveConfig()
    if (!saved) {
      if (previousModel) claudeStore.editingConfig.vars['ANTHROPIC_MODEL'] = previousModel
      else delete claudeStore.editingConfig.vars['ANTHROPIC_MODEL']
      if (previousEffort) claudeStore.editingConfig.vars['CLAUDE_CODE_EFFORT_LEVEL'] = previousEffort
      else delete claudeStore.editingConfig.vars['CLAUDE_CODE_EFFORT_LEVEL']
      submitError.value = '模型设置保存失败，请先在 Claude 配置中选择一个默认配置'
      return false
    }

    if (
      nextModel !== previousModel
      && props.tabId !== null
      && props.tabId !== undefined
    ) {
      await observerStore.changeModel(props.tabId, nextModel)
    }
    activeModelSubmenu.value = null
    return true
  } catch (error) {
    submitError.value = error instanceof Error ? error.message : String(error)
    return false
  } finally {
    modelPreferencePending.value = false
  }
}

async function selectModel(model: string) {
  const nextBaseModel = stripContextSuffix(model)
  if (!nextBaseModel || nextBaseModel === baseModel.value) return
  await persistModelPreferences(nextBaseModel, selectedEffort.value, selectedContext.value)
}

async function selectEffort(effort: string) {
  if (!effort || effort === selectedEffort.value) return
  await persistModelPreferences(baseModel.value, effort, selectedContext.value)
}

async function selectContext(context: ContextLength) {
  if (context === selectedContext.value) return
  await persistModelPreferences(baseModel.value, selectedEffort.value, context)
}

async function resetModelPreferences() {
  await persistModelPreferences(baseModel.value, 'auto', '200k')
}

const inputPlaceholder = computed(() => {
  if (pendingQuestion.value) return 'Select Claude questions above and submit your choices'
  if (props.startupPending) return '正在等待 Claude 启动并发送消息…'
  if (state.value.runState === 'permission') return '请先在原始终端中完成确认'
  if (
    state.value.runState === 'working'
    && state.value.queuedPrompts.length >= CLAUDE_PROMPT_QUEUE_LIMIT
  ) return `等待队列已满（最多 ${CLAUDE_PROMPT_QUEUE_LIMIT} 条）`
  if (state.value.runState === 'working') return '输入消息，发送后加入等待队列…'
  if (!state.value.available) return '结构化通道不可用，请切换到原始终端'
  if (!state.value.active) return '会话已经结束'
  if (!state.value.sessionReady || state.value.runState === 'starting') return 'Claude 正在启动…'
  return '向 Claude 发送消息…'
})

function renderMarkdown(text: string) {
  return markdown.render(text)
}

type CopyFeedbackMessages = {
  defaultLabel: string
  copiedAnnouncement: string
  failedAnnouncement: string
}

const CODE_COPY_FEEDBACK: CopyFeedbackMessages = {
  defaultLabel: '复制代码',
  copiedAnnouncement: '代码已复制到剪贴板',
  failedAnnouncement: '代码复制失败',
}

const MESSAGE_COPY_FEEDBACK: CopyFeedbackMessages = {
  defaultLabel: '复制消息',
  copiedAnnouncement: '消息已复制到剪贴板',
  failedAnnouncement: '消息复制失败',
}

async function cyclePermissionMode() {
  if (!canEditInput.value) return
  submitError.value = ''
  try {
    if (props.startupMode) {
      const currentIndex = STARTUP_PERMISSION_MODE_CYCLE.indexOf(startupPermissionMode.value)
      const nextMode = STARTUP_PERMISSION_MODE_CYCLE[
        (currentIndex + 1) % STARTUP_PERMISSION_MODE_CYCLE.length
      ] ?? 'auto'
      startupPermissionModeRevision += 1
      startupPermissionMode.value = nextMode
      emit('update:initialPermissionMode', nextMode)
      startupPermissionModeWrite = startupPermissionModeWrite
        .catch(() => {})
        .then(() => observerStore.saveDefaultPermissionMode(nextMode))
      void startupPermissionModeWrite.catch((error) => {
        if (!disposed) {
          submitError.value = error instanceof Error ? error.message : String(error)
        }
      })
      return
    }
    if (props.tabId === null || props.tabId === undefined) return
    const currentMode = state.value.pendingPermissionMode
      ?? permissionModeFromLabel(state.value.permissionMode)
      ?? props.initialPermissionMode
      ?? 'auto'
    const currentIndex = STARTUP_PERMISSION_MODE_CYCLE.indexOf(currentMode)
    const nextMode = STARTUP_PERMISSION_MODE_CYCLE[
      (currentIndex + 1) % STARTUP_PERMISSION_MODE_CYCLE.length
    ] ?? 'auto'
    await observerStore.cyclePermissionMode(props.tabId, nextMode)
  } catch (error) {
    submitError.value = error instanceof Error ? error.message : String(error)
  }
}

async function loadStartupPermissionMode() {
  if (!props.startupMode) return
  const revision = startupPermissionModeRevision
  try {
    const mode = await observerStore.loadDefaultPermissionMode()
    // 配置读取可能在用户第一次 Shift + Tab 之后才返回；此时不能覆盖用户的新选择。
    if (revision === startupPermissionModeRevision) {
      startupPermissionMode.value = mode
      emit('update:initialPermissionMode', mode)
    }
  } catch (error) {
    submitError.value = error instanceof Error ? error.message : String(error)
  }
}

const ASSISTANT_COPY_FEEDBACK: CopyFeedbackMessages = {
  defaultLabel: '复制回复',
  copiedAnnouncement: '回复已复制到剪贴板',
  failedAnnouncement: '回复复制失败',
}

function showCopyFeedback(
  button: HTMLButtonElement,
  state: 'copied' | 'failed',
  messages: CopyFeedbackMessages = CODE_COPY_FEEDBACK,
) {
  const previousTimer = copyFeedbackTimers.get(button)
  if (previousTimer !== undefined) {
    window.clearTimeout(previousTimer)
    copyFeedbackTimerIds.delete(previousTimer)
  }

  const copied = state === 'copied'
  const label = copied ? '已复制' : '复制失败'
  button.classList.toggle('is-copied', copied)
  button.classList.toggle('is-failed', !copied)
  button.setAttribute('aria-label', label)
  button.title = label

  const announcementSequence = ++copyAnnouncementSequence
  copyAnnouncement.value = ''
  void nextTick(() => {
    if (announcementSequence === copyAnnouncementSequence) {
      copyAnnouncement.value = copied ? messages.copiedAnnouncement : messages.failedAnnouncement
    }
  })
  if (copyAnnouncementTimer !== undefined) window.clearTimeout(copyAnnouncementTimer)
  copyAnnouncementTimer = window.setTimeout(() => {
    if (announcementSequence === copyAnnouncementSequence) copyAnnouncement.value = ''
    copyAnnouncementTimer = undefined
  }, copied ? 1600 : 2200)

  const timer = window.setTimeout(() => {
    copyFeedbackTimerIds.delete(timer)
    copyFeedbackTimers.delete(button)
    if (!button.isConnected) return
    button.classList.remove('is-copied', 'is-failed')
    button.setAttribute('aria-label', messages.defaultLabel)
    button.title = messages.defaultLabel
  }, copied ? 1600 : 2200)
  copyFeedbackTimers.set(button, timer)
  copyFeedbackTimerIds.add(timer)
}

async function handleHistoryClick(event: MouseEvent) {
  if (!(event.target instanceof Element)) return
  const button = event.target.closest<HTMLButtonElement>('.conversation-code-block__copy')
  if (!button || !historyRef.value?.contains(button)) return

  const code = button.closest('.conversation-code-block')
    ?.querySelector<HTMLElement>('pre code')
    ?.textContent
  if (code === undefined || code === null) return

  button.setAttribute('aria-busy', 'true')
  try {
    await copyTextToClipboard(code)
    if (!disposed && button.isConnected) showCopyFeedback(button, 'copied')
  } catch {
    if (!disposed && button.isConnected) showCopyFeedback(button, 'failed')
  } finally {
    button.removeAttribute('aria-busy')
    if (!disposed && button.isConnected) button.focus({ preventScroll: true })
  }
}

async function copyUserMessage(text: string, event: MouseEvent) {
  await copyConversationMessage(text, event, MESSAGE_COPY_FEEDBACK)
}

async function copyAssistantMessage(text: string, event: MouseEvent) {
  await copyConversationMessage(text, event, ASSISTANT_COPY_FEEDBACK)
}

async function copyConversationMessage(
  text: string,
  event: MouseEvent,
  messages: CopyFeedbackMessages,
) {
  if (!(event.currentTarget instanceof HTMLButtonElement)) return
  const button = event.currentTarget
  button.setAttribute('aria-busy', 'true')
  try {
    await copyTextToClipboard(text)
    if (!disposed && button.isConnected) {
      showCopyFeedback(button, 'copied', messages)
    }
  } catch {
    if (!disposed && button.isConnected) {
      showCopyFeedback(button, 'failed', messages)
    }
  } finally {
    button.removeAttribute('aria-busy')
    if (!disposed && button.isConnected) button.focus({ preventScroll: true })
  }
}

function formatTime(timestamp: string) {
  const date = new Date(timestamp)
  return Number.isNaN(date.getTime())
    ? ''
    : date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
}

function askUserQuestions(item: ClaudeConversationItem): ClaudeAskUserQuestion[] {
  return parseClaudeAskUserQuestions(item.toolName, item.toolInput) ?? []
}

function isAskUserQuestionItem(item: ClaudeConversationItem) {
  return askUserQuestions(item).length > 0
}

function activeQuestionIndex(item: ClaudeConversationItem) {
  const questions = askUserQuestions(item)
  const requested = questionActiveIndexes.value[item.id] ?? 0
  return Math.min(Math.max(requested, 0), questions.length)
}

function activeAskUserQuestions(item: ClaudeConversationItem) {
  if (item.state !== 'waiting' || questionSubmitted.value[item.id]) return []
  const questionIndex = activeQuestionIndex(item)
  const question = askUserQuestions(item)[questionIndex]
  return question ? [{ question, questionIndex }] : []
}

function questionStepComplete(item: ClaudeConversationItem, questionIndex: number) {
  return item.state !== 'waiting'
    || !!questionSubmitted.value[item.id]
    || questionIndex < activeQuestionIndex(item)
}

function questionSelectionKey(item: ClaudeConversationItem, questionIndex: number) {
  return `${item.id}:${questionIndex}`
}

function questionCustomAnswer(item: ClaudeConversationItem, questionIndex: number) {
  return questionCustomAnswers.value[questionSelectionKey(item, questionIndex)] ?? ''
}

function setQuestionCustomAnswer(
  item: ClaudeConversationItem,
  questionIndex: number,
  value: string,
) {
  if (!questionCanAnswer(item) || questionIndex !== activeQuestionIndex(item)) return
  const key = questionSelectionKey(item, questionIndex)
  questionCustomAnswers.value = { ...questionCustomAnswers.value, [key]: value }
  if (value.trim()) {
    questionSelections.value = { ...questionSelections.value, [key]: [] }
  }
  questionErrors.value = { ...questionErrors.value, [item.id]: '' }
}

function selectedQuestionOptions(item: ClaudeConversationItem, questionIndex: number) {
  return questionSelections.value[questionSelectionKey(item, questionIndex)] ?? []
}

function isQuestionOptionSelected(
  item: ClaudeConversationItem,
  questionIndex: number,
  optionIndex: number,
) {
  return selectedQuestionOptions(item, questionIndex).includes(optionIndex)
}

function questionCanAnswer(item: ClaudeConversationItem) {
  return item.state === 'waiting'
    && !questionSubmitting.value[item.id]
    && !questionSubmitted.value[item.id]
    && props.tabId !== null
    && props.tabId !== undefined
    && state.value.available
    && state.value.active
    && state.value.runState === 'permission'
}

function toggleQuestionOption(
  item: ClaudeConversationItem,
  questionIndex: number,
  optionIndex: number,
) {
  if (!questionCanAnswer(item) || questionIndex !== activeQuestionIndex(item)) return
  const question = askUserQuestions(item)[questionIndex]
  if (!question) return
  const key = questionSelectionKey(item, questionIndex)
  const current = selectedQuestionOptions(item, questionIndex)
  const next = question.multiSelect
    ? current.includes(optionIndex)
      ? current.filter(index => index !== optionIndex)
      : [...current, optionIndex].sort((left, right) => left - right)
    : [optionIndex]
  questionSelections.value = { ...questionSelections.value, [key]: next }
  questionCustomAnswers.value = { ...questionCustomAnswers.value, [key]: '' }
  questionErrors.value = { ...questionErrors.value, [item.id]: '' }
}

function questionAnswerReady(item: ClaudeConversationItem) {
  const questionIndex = activeQuestionIndex(item)
  return !!askUserQuestions(item)[questionIndex] && (
    selectedQuestionOptions(item, questionIndex).length > 0
    || !!questionCustomAnswer(item, questionIndex).trim()
  )
}

function questionCardStateLabel(item: ClaudeConversationItem) {
  if (questionSubmitting.value[item.id]) return '正在提交'
  if (questionSubmitted.value[item.id] && item.state === 'waiting') return '等待 Claude'
  if (item.state !== 'waiting') return toolStateLabel(item.state)
  const questions = askUserQuestions(item)
  if (questions.length > 0) {
    return `第 ${Math.min(activeQuestionIndex(item) + 1, questions.length)} / ${questions.length} 题`
  }
  return toolStateLabel(item.state)
}

function questionSubmitLabel(item: ClaudeConversationItem) {
  if (questionSubmitting.value[item.id]) return '正在提交…'
  const questions = askUserQuestions(item)
  return activeQuestionIndex(item) < questions.length - 1 ? '下一题' : '提交回答'
}

async function submitQuestionAnswers(item: ClaudeConversationItem) {
  if (
    props.tabId === null
    || props.tabId === undefined
    || !questionCanAnswer(item)
    || !questionAnswerReady(item)
  ) return

  const questions = askUserQuestions(item)
  const questionIndex = activeQuestionIndex(item)
  const question = questions[questionIndex]
  if (!question) return
  const answer = {
    selectedOptions: [...selectedQuestionOptions(item, questionIndex)],
    customText: questionCustomAnswer(item, questionIndex),
  }
  questionSubmitting.value = { ...questionSubmitting.value, [item.id]: true }
  questionErrors.value = { ...questionErrors.value, [item.id]: '' }
  try {
    await observerStore.respondToAskUserQuestion(props.tabId, question, answer)
    const nextQuestionIndex = questionIndex + 1
    questionActiveIndexes.value = {
      ...questionActiveIndexes.value,
      [item.id]: nextQuestionIndex,
    }
    if (nextQuestionIndex >= questions.length) {
      questionSubmitted.value = { ...questionSubmitted.value, [item.id]: true }
    }
  } catch (error) {
    questionErrors.value = {
      ...questionErrors.value,
      [item.id]: error instanceof Error ? error.message : String(error),
    }
  } finally {
    questionSubmitting.value = { ...questionSubmitting.value, [item.id]: false }
  }
}

function toolStateLabel(stateValue: ClaudeConversationItem['state']) {
  switch (stateValue) {
    case 'running': return '运行中'
    case 'success': return '已完成'
    case 'failed': return '失败'
    default: return '等待中'
  }
}

function toolSummary(item: ClaudeConversationItem) {
  return summarizeClaudeTool(item.toolName, item.toolInput)
}

function prettyValue(value: unknown) {
  if (typeof value === 'string') return value
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}

function resizeComposerToContent() {
  const input = inputRef.value
  if (!input) return

  input.style.overflowY = 'hidden'
  input.style.height = 'auto'
  const lineHeight = Number.parseFloat(window.getComputedStyle(input).lineHeight)
  const metrics = getClaudeComposerTextareaMetrics(input.scrollHeight, lineHeight)
  input.style.height = `${metrics.height}px`
  input.style.overflowY = metrics.overflowY
  if (scrollInitialized && followLatest.value) void scrollToLatest()
}

function restoreDraftForSession(projectId: string, sessionId: string, restored: string) {
  if (props.restoreSessionDraft) {
    props.restoreSessionDraft(projectId, sessionId, restored)
    return
  }
  if (props.projectId === projectId && props.sessionId === sessionId) {
    prompt.value = prompt.value.trim() ? `${restored}\n${prompt.value}` : restored
  }
}

function observeComposerWidth() {
  const input = inputRef.value
  if (!input || typeof ResizeObserver === 'undefined') return
  composerResizeObserver?.disconnect()
  observedComposerWidth = null
  composerResizeObserver = new ResizeObserver(([entry]) => {
    if (!entry) return
    const width = entry.contentRect.width
    if (observedComposerWidth !== null && Math.abs(width - observedComposerWidth) < 0.5) return
    observedComposerWidth = width
    resizeComposerToContent()
  })
  composerResizeObserver.observe(input)
}

function handleHistoryScroll() {
  const element = historyRef.value
  if (!element || !scrollInitialized) return
  followLatest.value = element.scrollHeight - element.scrollTop - element.clientHeight < 80
}

function isFinalAssistantMessage(item: ClaudeConversationItem) {
  const itemIndex = state.value.items.findIndex(candidate => candidate.id === item.id)
  if (itemIndex < 0) return false
  for (const laterItem of state.value.items.slice(itemIndex + 1)) {
    if (laterItem.kind === 'user') return true
    if (laterItem.kind === 'assistant') return false
  }
  return true
}

function refreshSlashCommandsOnWindowFocus() {
  void refreshClaudeSlashCommands()
}

async function scrollToLatest() {
  await nextTick()
  const element = historyRef.value
  if (element && scrollInitialized && followLatest.value) element.scrollTop = element.scrollHeight
}

function saveCurrentScroll(sessionId: string) {
  const element = historyRef.value
  if (element && scrollInitialized) saveClaudeConversationScroll(sessionId, element.scrollTop)
}

async function initializeHistory(tabId: number, sessionId: string) {
  const generation = ++scrollLoadGeneration
  const savedScrollTop = getClaudeConversationScroll(sessionId)
  scrollInitialized = false
  followLatest.value = savedScrollTop === undefined

  await observerStore.loadSnapshot(tabId)
  await nextTick()
  if (
    disposed
    || generation !== scrollLoadGeneration
    || tabId !== props.tabId
    || sessionId !== props.sessionId
  ) return

  const element = historyRef.value
  if (!element) return
  if (savedScrollTop === undefined) {
    element.scrollTop = element.scrollHeight
  } else {
    const maxScrollTop = Math.max(0, element.scrollHeight - element.clientHeight)
    element.scrollTop = Math.min(savedScrollTop, maxScrollTop)
  }

  followLatest.value = element.scrollHeight - element.scrollTop - element.clientHeight < 80
  scrollInitialized = true
  if (canEditInput.value) inputRef.value?.focus()
}

function resumeFollow() {
  followLatest.value = true
  void scrollToLatest()
}

async function submit() {
  if (!(prompt.value.trim() || attachments.value.length) || !canEditInput.value) return
  if (validateClaudeSlashCommand(prompt.value).kind === 'unsupported') {
    showCommandNotice('暂不支持该命令')
    return
  }
  if (
    state.value.runState === 'working'
    && state.value.queuedPrompts.length >= CLAUDE_PROMPT_QUEUE_LIMIT
  ) {
    submitError.value = `等待队列最多保留 ${CLAUDE_PROMPT_QUEUE_LIMIT} 条消息`
    return
  }
  const attachedPaths = attachments.value.map((a) =>
    formatConversationDropPath(a.path, projectPath.value, dropPathMode.value),
  )
  const submitted = attachedPaths.length
    ? attachedPaths.join(' ') + (prompt.value.trim() ? '\n' + prompt.value : '')
    : prompt.value
  const pendingAttachments = [...attachments.value]
  if (props.startupMode) {
    const submitStartupPrompt = props.submitStartupPrompt
    if (!submitStartupPrompt) return
    submitError.value = ''
    try {
      // 确保新会话读取到刚通过 Shift + Tab 选定的默认权限模式。
      await startupPermissionModeWrite
      await submitStartupPrompt(submitted)
      attachments.value = []
    } catch (error) {
      if (!disposed) submitError.value = String(error)
    }
    return
  }
  if (props.tabId === null || props.tabId === undefined) return
  const submittedTabId = props.tabId
  const submittedProjectId = props.projectId
  const submittedSessionId = props.sessionId
  submitError.value = ''
  prompt.value = ''
  attachments.value = []
  void pendingAttachments
  await nextTick()
  resizeComposerToContent()
  try {
    const accepted = state.value.runState === 'working'
      ? await observerStore.queuePrompt(submittedTabId, submitted)
      : await observerStore.submitPrompt(submittedTabId, submitted)
    if (!accepted) {
      restoreDraftForSession(submittedProjectId, submittedSessionId, submitted)
      return
    }
    followLatest.value = true
  } catch (error) {
    const submittedState = observerStore.states[submittedTabId]
    if (
      submittedState?.available
      && submittedState.active
    ) {
      restoreDraftForSession(submittedProjectId, submittedSessionId, submitted)
      if (
        !disposed
        && props.projectId === submittedProjectId
        && props.sessionId === submittedSessionId
      ) {
        await nextTick()
        resizeComposerToContent()
      }
    }
    submitError.value = String(error)
  }
}

async function stopRun() {
  if (stopPending.value || !isWorking.value || props.tabId === null || props.tabId === undefined) return

  stopPending.value = true
  submitError.value = ''
  try {
    const restoredPrompt = await observerStore.interruptRun(props.tabId)
    if (restoredPrompt !== undefined) {
      prompt.value = restoredPrompt + prompt.value
      await nextTick()
      resizeComposerToContent()
      inputRef.value?.focus({ preventScroll: true })
    }
  } catch (error) {
    submitError.value = error instanceof Error ? error.message : String(error)
  } finally {
    stopPending.value = false
  }
}

function queuedPromptModeLabel(queuedPrompt: ClaudeQueuedPrompt) {
  if (queuedPrompt.delivery === 'sending') return '处理中'
  if (queuedPrompt.mode === 'native') {
    return queuedPrompt.delivery === 'native' ? '等待执行间隙' : '原生队列'
  }
  return '完成后发送'
}

function queuedPromptActionDisabled(queuedPrompt: ClaudeQueuedPrompt) {
  if (
    state.value.queueActionPending
    || state.value.queuedPrompts.some(item => item.delivery === 'sending')
    || !state.value.available
    || !state.value.active
    || !!state.value.terminalPrompt
    || (state.value.runState !== 'working' && state.value.runState !== 'idle')
  ) return true
  return queuedPrompt.delivery === 'native' && state.value.runState !== 'working'
}

async function withdrawQueuedPrompt(queuedPromptId: string) {
  if (props.tabId === null || props.tabId === undefined) return
  const tabId = props.tabId
  const projectId = props.projectId
  const sessionId = props.sessionId
  submitError.value = ''
  try {
    const restored = await observerStore.withdrawQueuedPrompt(tabId, queuedPromptId)
    restoreDraftForSession(projectId, sessionId, restored)
    if (
      !disposed
      && props.tabId === tabId
      && props.projectId === projectId
      && props.sessionId === sessionId
    ) {
      await nextTick()
      resizeComposerToContent()
      inputRef.value?.focus({ preventScroll: true })
    }
  } catch (error) {
    submitError.value = String(error)
  }
}

async function insertQueuedPromptNow(queuedPromptId: string) {
  if (props.tabId === null || props.tabId === undefined) return
  submitError.value = ''
  try {
    await observerStore.insertQueuedPromptNow(props.tabId, queuedPromptId)
    followLatest.value = true
  } catch (error) {
    submitError.value = String(error)
  }
}

async function respondToWorkspaceTrust(action: 'confirm' | 'cancel') {
  if (
    workspaceTrustPending.value
    || props.tabId === null
    || props.tabId === undefined
  ) return

  workspaceTrustPending.value = true
  workspaceTrustAction.value = action
  workspaceTrustError.value = ''
  void nextTick(() => workspaceTrustOverlayRef.value?.focus({ preventScroll: true }))
  try {
    const resolution = resolveClaudeWorkspaceTrustAction(action)
    if (resolution.kind === 'reject-and-close-terminal') {
      await projectStore.closeSessionTerminal(props.sessionId)
      workspaceTrustPending.value = false
      workspaceTrustAction.value = null
      return
    }
    await observerStore.respondToTerminalPrompt(props.tabId, 'confirm')
  } catch (error) {
    workspaceTrustPending.value = false
    workspaceTrustAction.value = null
    workspaceTrustError.value = error instanceof Error ? error.message : String(error)
  }
}

async function respondToTerminalChoice(index: number) {
  if (modelSwitchConfirmPrompt.value) {
    if (index >= 0) modelSwitchKeyboardIndex.value = index
    queueTerminalModelAction(index >= 0 ? index : 'cancel')
    return
  }

  if (
    pluginInstallPending.value
    || props.tabId === null
    || props.tabId === undefined
  ) return

  pluginInstallPending.value = true
  pluginInstallAction.value = index >= 0 ? index : null
  pluginInstallError.value = ''
  pluginInstallOverlayRef.value?.focus({ preventScroll: true })
  try {
    await observerStore.respondToTerminalPrompt(props.tabId, index)
  } catch (error) {
    pluginInstallPending.value = false
    pluginInstallAction.value = null
    pluginInstallError.value = error instanceof Error ? error.message : String(error)
  }
}

function handleTerminalPromptKeydown(event: KeyboardEvent) {
  if (!selectionPrompt.value) return

  if (!modelSwitchConfirmPrompt.value) {
    if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      void respondToTerminalChoice(-1)
    } else if (event.key === 'Tab') {
      trapPluginInstallFocus(event)
    }
    return
  }

  event.preventDefault()
  event.stopPropagation()
  event.stopImmediatePropagation()
  if (pluginInstallPending.value) return

  if (event.key === 'Escape') {
    queueTerminalModelAction('cancel')
    return
  }

  if (event.key === 'ArrowUp' || event.key === 'ArrowDown') {
    const optionCount = modelSwitchConfirmPrompt.value.options.length
    if (optionCount === 0) return
    const currentIndex = modelSwitchKeyboardIndex.value
      ?? modelSwitchConfirmPrompt.value.selectedIndex
    const step = event.key === 'ArrowUp' ? -1 : 1
    modelSwitchKeyboardIndex.value = (currentIndex + step + optionCount) % optionCount
    queueTerminalModelAction(event.key === 'ArrowUp' ? 'up' : 'down')
    return
  }

  if (event.key === 'Enter') {
    queueTerminalModelAction('confirm')
  }
}

function queueTerminalModelAction(action: 'up' | 'down' | 'confirm' | 'cancel' | number) {
  if (
    props.tabId === null
    || props.tabId === undefined
    || !modelSwitchConfirmPrompt.value
  ) return

  const closesPrompt = action === 'confirm' || action === 'cancel' || typeof action === 'number'
  if (closesPrompt) {
    pluginInstallPending.value = true
    pluginInstallAction.value = typeof action === 'number'
      ? action
      : modelSwitchKeyboardIndex.value
  }
  const tabId = props.tabId
  terminalModelActionQueue = terminalModelActionQueue
    .catch(() => {})
    .then(async () => {
      if (!modelSwitchConfirmPrompt.value) return
      pluginInstallError.value = ''
      await observerStore.respondToTerminalPrompt(tabId, action)
    })
    .catch((error) => {
      modelSwitchKeyboardIndex.value = modelSwitchConfirmPrompt.value?.selectedIndex ?? null
      pluginInstallError.value = error instanceof Error ? error.message : String(error)
    })
    .finally(() => {
      if (closesPrompt && modelSwitchConfirmPrompt.value) {
        pluginInstallPending.value = false
        pluginInstallAction.value = null
      }
    })
}

function handleGlobalModelSwitchKeydown(event: KeyboardEvent) {
  if (modelSwitchConfirmPrompt.value) handleTerminalPromptKeydown(event)
}

function trapPluginInstallFocus(event: KeyboardEvent) {
  const overlay = pluginInstallOverlayRef.value
  if (!overlay) return
  const targets = Array.from(overlay.querySelectorAll('button'))
    .filter((element): element is HTMLButtonElement => !element.disabled)
  if (targets.length === 0) {
    event.preventDefault()
    overlay.focus({ preventScroll: true })
    return
  }

  const activeIndex = targets.findIndex(element => element === document.activeElement)
  const nextIndex = event.shiftKey
    ? (activeIndex <= 0 ? targets.length - 1 : activeIndex - 1)
    : (activeIndex < 0 || activeIndex === targets.length - 1 ? 0 : activeIndex + 1)
  event.preventDefault()
  targets[nextIndex].focus({ preventScroll: true })
}

function trapWorkspaceTrustFocus(event: KeyboardEvent) {
  const targets = [workspaceTrustCancelRef.value, workspaceTrustConfirmRef.value]
    .filter((element): element is HTMLButtonElement => !!element && !element.disabled)
  if (targets.length === 0) {
    event.preventDefault()
    workspaceTrustOverlayRef.value?.focus({ preventScroll: true })
    return
  }

  const activeIndex = targets.findIndex(element => element === document.activeElement)
  let nextIndex: number
  if (event.shiftKey) {
    nextIndex = activeIndex <= 0 ? targets.length - 1 : activeIndex - 1
  } else {
    nextIndex = activeIndex < 0 || activeIndex === targets.length - 1 ? 0 : activeIndex + 1
  }
  event.preventDefault()
  targets[nextIndex].focus({ preventScroll: true })
}

watch(
  () => state.value.items.map(item => `${item.id}:${item.text?.length ?? 0}:${item.state ?? ''}`).join('|'),
  () => {
    if (scrollInitialized) void scrollToLatest()
  },
)

watch(() => !!workingActivity.value, () => {
  if (scrollInitialized && followLatest.value) void scrollToLatest()
})

watch(() => state.value.runState, (nextState, previousState) => {
  if (
    (previousState === 'working' || previousState === 'permission')
    && nextState !== 'working'
    && nextState !== 'permission'
  ) {
    expandedProcessGroupIds.value = new Set()
  }
})

watch(activityTokenCount, (tokenCount) => {
  if (tokenCount) animateTokenCount(tokenCount)
  else {
    stopTokenAnimation()
    animatedTokenValue.value = null
    animatedTokenFormat.value = null
  }
}, { immediate: true })

watch(() => state.value.compactCompletionRevision, (revision, previousRevision) => {
  if (revision > previousRevision) showCommandNotice('已完成上下文压缩')
})

watch(() => state.value.queuedPrompts.length, () => {
  if (scrollInitialized && followLatest.value) void scrollToLatest()
})

watch(() => props.sessionDraft, (sessionDraft) => {
  const nextDraft = sessionDraft ?? ''
  if (prompt.value !== nextDraft) prompt.value = nextDraft
})

watch(() => props.sessionAttachmentPaths, (paths) => {
  const current = attachments.value.map((a) => a.path)
  const incoming = paths ?? []
  if (current.join('\0') === incoming.join('\0')) return
  attachments.value = []
  addDroppedPaths(incoming)
})

watch(prompt, (draft) => {
  slashCommandMenuDismissed.value = false
  if (draft !== (props.sessionDraft ?? '')) emit('update:sessionDraft', draft)
  void nextTick(resizeComposerToContent)
})

watch(
  () => filteredSlashCommands.value.map(command => command.command).join('\u0000'),
  () => { slashCommandIndex.value = 0 },
)

watch(attachments, (list) => {
  const paths = list.map((a) => a.path)
  const saved = props.sessionAttachmentPaths
  if (!saved || saved.join('\0') !== paths.join('\0')) {
    emit('update:sessionAttachmentPaths', paths)
  }
}, { deep: true })

watch(() => workspaceTrustPrompt.value
  ? `${workspaceTrustPrompt.value.kind}\u0000${workspaceTrustPrompt.value.path}`
  : '', (promptKey) => {
  workspaceTrustPending.value = false
  workspaceTrustAction.value = null
  workspaceTrustError.value = ''
  if (promptKey) {
    restoreComposerFocusWhenReady = false
    workspaceTrustPreviousFocus = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null
    void nextTick(() => workspaceTrustConfirmRef.value?.focus({ preventScroll: true }))
    return
  }

  if (state.value.terminalPrompt) return

  const previousFocus = workspaceTrustPreviousFocus
  workspaceTrustPreviousFocus = null
  void nextTick(() => {
    if (previousFocus?.isConnected && !previousFocus.hasAttribute('disabled')) {
      previousFocus.focus({ preventScroll: true })
    } else if (canEditInput.value) {
      inputRef.value?.focus({ preventScroll: true })
    } else {
      restoreComposerFocusWhenReady = true
    }
  })
}, { immediate: true })

watch(() => selectionPrompt.value
  ? `${selectionPrompt.value.kind}\u0000${selectionPrompt.value.options.join('\u0000')}`
  : '', (promptKey) => {
  pluginInstallPending.value = false
  pluginInstallAction.value = null
  pluginInstallError.value = ''
  modelSwitchKeyboardIndex.value = null
  if (promptKey) {
    modelSwitchKeyboardIndex.value = modelSwitchConfirmPrompt.value?.selectedIndex ?? null
    restoreComposerFocusWhenReady = false
    void nextTick(() => pluginInstallOverlayRef.value?.focus({ preventScroll: true }))
    return
  }

  void nextTick(() => {
    if (canEditInput.value) {
      inputRef.value?.focus({ preventScroll: true })
    } else {
      restoreComposerFocusWhenReady = true
    }
  })
}, { immediate: true })

watch(canEditInput, (enabled) => {
  if (
    !enabled
    || !restoreComposerFocusWhenReady
    || workspaceTrustPrompt.value
    || pluginInstallPrompt.value
  ) return
  restoreComposerFocusWhenReady = false
  void nextTick(() => inputRef.value?.focus({ preventScroll: true }))
})

watch(() => [props.tabId, props.sessionId, props.startupMode] as const, ([tabId, sessionId, startupMode], [, previousSessionId]) => {
  if (startupMode) {
    scrollInitialized = true
    submitError.value = ''
    void loadStartupPermissionMode()
    void nextTick(() => inputRef.value?.focus())
    return
  }
  if (tabId === null || tabId === undefined) return
  saveCurrentScroll(previousSessionId)
  prompt.value = props.sessionDraft ?? ''
  attachments.value = []
  const savedPaths = props.sessionAttachmentPaths ?? []
  if (savedPaths.length) addDroppedPaths(savedPaths)
  submitError.value = ''
  void initializeHistory(tabId, sessionId)
})

watch(projectPath, () => {
  void refreshClaudeSlashCommands()
}, { immediate: true })

onMounted(() => {
  document.addEventListener('pointerdown', handleModelPickerOutsidePointerDown)
  document.addEventListener('pointerdown', handleSlashCommandMenuOutsidePointerDown)
  document.addEventListener('keydown', handleGlobalModelSwitchKeydown, true)
  window.addEventListener('focus', refreshSlashCommandsOnWindowFocus)
  if (props.startupMode) {
    scrollInitialized = true
    void loadStartupPermissionMode()
    void nextTick(() => inputRef.value?.focus())
  } else if (props.tabId !== null && props.tabId !== undefined) {
    void initializeHistory(props.tabId, props.sessionId)
  }
  void nextTick(() => {
    resizeComposerToContent()
    observeComposerWidth()
  })
})

onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', handleModelPickerOutsidePointerDown)
  document.removeEventListener('pointerdown', handleSlashCommandMenuOutsidePointerDown)
  document.removeEventListener('keydown', handleGlobalModelSwitchKeydown, true)
  window.removeEventListener('focus', refreshSlashCommandsOnWindowFocus)
  if (!props.startupMode) saveCurrentScroll(props.sessionId)
  disposed = true
  if (copyAnnouncementTimer !== undefined) window.clearTimeout(copyAnnouncementTimer)
  stopTokenAnimation()
  if (commandNoticeTimer !== undefined) window.clearTimeout(commandNoticeTimer)
  for (const timer of copyFeedbackTimerIds) window.clearTimeout(timer)
  copyFeedbackTimerIds.clear()
  composerResizeObserver?.disconnect()
  composerResizeObserver = null
})

defineExpose({ appendDroppedFiles })
</script>

<style scoped>
.claude-conversation {
  --claude-content-width: min(720px, 100%);
  --claude-content-font-size: var(--font-size-title, 14px);
  --claude-meta-font-size: var(--font-size-small, 12px);
  --claude-content-line-height: 1.55;
  --claude-item-gap: 16px;
  --claude-block-padding: 12px;
  --claude-page-bg: var(--card, #fff);
  --claude-surface-bg: var(--bg);
  --claude-elevated-bg: var(--card);
  --claude-border-color: var(--separator);
  --claude-field-bg: var(--input-bg);
  --claude-field-border: var(--input-border);
  --claude-input-surface: color-mix(in srgb, var(--claude-field-bg) 96%, var(--primary) 4%);
  --claude-copy-bg: rgba(255, 255, 255, 0.94);
  --claude-copy-hover-bg: color-mix(in srgb, var(--primary) 10%, #fff);
  --claude-copy-border: var(--input-border);
  --claude-copy-shadow: rgba(0, 0, 0, 0.12);
  --claude-jump-bg: #e8ebef;
  --claude-jump-hover-bg: #dde3ea;
  --claude-jump-border: #c9cfd7;
  --claude-jump-shadow: rgba(0, 0, 0, 0.16);
  --claude-shadow-soft: rgba(0, 0, 0, 0.14);
  --claude-shadow-medium: rgba(0, 0, 0, 0.18);
  --claude-composer-bg: #fff;
  --claude-composer-border: #666b72;
  --claude-composer-border-hover: #41464d;
  --claude-composer-border-focus: #2d3137;
  --claude-composer-ring: rgba(28, 31, 35, 0.12);
  --claude-send-bg: #5c6168;
  --claude-send-ready-bg: #23262a;
  --claude-send-color: #fff;
  --claude-activity-color: #b94f2b;
  --claude-activity-bg: rgba(255, 255, 255, 0.96);
  --claude-activity-border: #d6d9de;
  --claude-activity-shadow: rgba(0, 0, 0, 0.14);
  --claude-table-border: #cbd1d8;
  --claude-table-bg: #fff;
  --claude-table-header-bg: #f1f3f5;
  --claude-table-stripe-bg: #f8f9fa;
  --claude-queue-bg: #f3f4f6;
  --claude-queue-border: #d7dbe1;
  --claude-queue-hover-bg: #e9edf2;
  position: absolute;
  inset: 0;
  z-index: 4;
  display: flex;
  flex-direction: column;
  color: var(--text-primary);
  background: var(--claude-page-bg);
  font-size: var(--claude-content-font-size);
  line-height: var(--claude-content-line-height);
}

.claude-conversation.has-terminal-prompt {
  z-index: 30;
}

[data-theme="dark"] .claude-conversation {
  --claude-page-bg: #1b1f24;
  --claude-surface-bg: #252a31;
  --claude-elevated-bg: #2b3139;
  --claude-border-color: #373e48;
  --claude-field-bg: #2a3038;
  --claude-field-border: #48515d;
  --claude-copy-bg: #303741;
  --claude-copy-hover-bg: #37414d;
  --claude-copy-border: #48515d;
  --claude-copy-shadow: rgba(0, 0, 0, 0.34);
  --claude-jump-bg: #292e35;
  --claude-jump-hover-bg: #343b44;
  --claude-jump-border: #414954;
  --claude-jump-shadow: rgba(0, 0, 0, 0.36);
  --claude-shadow-soft: rgba(0, 0, 0, 0.28);
  --claude-shadow-medium: rgba(0, 0, 0, 0.36);
  --claude-composer-bg: #30353c;
  --claude-composer-border: #525a65;
  --claude-composer-border-hover: #626c78;
  --claude-composer-border-focus: #747e8a;
  --claude-composer-ring: rgba(210, 216, 224, 0.10);
  --claude-send-bg: #c6c9ce;
  --claude-send-ready-bg: #f4f5f6;
  --claude-send-color: #17191c;
  --claude-activity-color: #e7835f;
  --claude-activity-bg: rgba(37, 42, 49, 0.96);
  --claude-activity-border: #414954;
  --claude-activity-shadow: rgba(0, 0, 0, 0.32);
  --claude-table-border: #48515d;
  --claude-table-bg: #22272d;
  --claude-table-header-bg: #2c323a;
  --claude-table-stripe-bg: #272c33;
  --claude-queue-bg: #2a3037;
  --claude-queue-border: #444c57;
  --claude-queue-hover-bg: #343b44;
}

.tool-card__state {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #8b8b8b;
  flex: 0 0 auto;
}

.tool-card__state.is-success { background: #3fb950; }
.tool-card__state.is-running { background: #d29922; }
.tool-card__state.is-waiting { background: #58a6ff; }
.tool-card__state.is-failed { background: #f85149; }
.claude-conversation__copy-status,
.claude-conversation__activity-status {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}

.workspace-trust-overlay {
  position: absolute;
  z-index: 20;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 20px;
  background: color-mix(in srgb, var(--claude-page-bg) 72%, transparent);
  backdrop-filter: blur(2px);
}

.workspace-trust-dialog {
  width: min(440px, 100%);
  max-height: 100%;
  padding: 20px;
  overflow: auto;
  border: 1px solid var(--claude-field-border);
  border-radius: 14px;
  background: var(--claude-elevated-bg);
  box-shadow: 0 18px 48px var(--claude-shadow-medium);
}

.workspace-trust-dialog__icon {
  display: grid;
  place-items: center;
  width: 32px;
  height: 32px;
  margin-bottom: 14px;
  border-radius: 50%;
  color: #fff;
  background: #d29922;
  font-size: 18px;
  font-weight: 700;
}

.plugin-install-dialog__icon {
  background: #58a6ff;
}

.plugin-install-dialog__icon svg {
  width: 21px;
  height: 21px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.9;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.plugin-install-dialog__prompt {
  max-height: 96px;
  overflow: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  font-size: 13px;
}

.plugin-install-dialog__actions {
  justify-content: stretch;
}

.plugin-install-dialog__actions .btn {
  flex: 1 1 180px;
  min-width: 0;
  text-align: left;
}

.model-select-dialog {
  width: max-content;
  min-width: min(560px, 100%);
  max-width: min(1000px, calc(100vw - 40px));
}

.model-select-dialog .plugin-install-dialog__actions {
  flex-direction: column;
  align-items: stretch;
}

.model-select-dialog .plugin-install-dialog__actions .btn {
  width: 100%;
  flex: 0 0 auto;
  overflow-x: auto;
  text-overflow: clip;
  white-space: nowrap;
}

.workspace-trust-dialog__content h2 {
  margin: 0;
  font-size: 18px;
  line-height: 1.4;
}

.workspace-trust-dialog__content p {
  margin: 8px 0 0;
  color: var(--text-secondary);
}

.workspace-trust-dialog__content code {
  display: block;
  margin-top: 14px;
  padding: 10px 12px;
  overflow-wrap: anywhere;
  border: 1px solid var(--claude-border-color);
  border-radius: 8px;
  color: var(--text-primary);
  background: var(--claude-surface-bg);
  font-family: "Cascadia Code", Consolas, monospace;
  font-size: 13px;
}

.workspace-trust-dialog__content .workspace-trust-dialog__error {
  color: var(--danger, #f85149);
}

.workspace-trust-dialog__actions {
  display: flex;
  flex-wrap: wrap;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 18px;
}

@media (max-width: 420px) {
  .workspace-trust-overlay {
    padding: 12px;
  }

  .workspace-trust-dialog {
    padding: 16px;
  }

  .workspace-trust-dialog__actions {
    flex-direction: column;
  }

  .workspace-trust-dialog__actions .btn {
    width: 100%;
  }
}

.claude-conversation__history {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  scrollbar-gutter: stable;
  padding: 24px clamp(18px, 6vw, 84px);
  scroll-behavior: auto;
}

.claude-conversation.has-floating-topbar .claude-conversation__history {
  padding-bottom: 72px;
}

.claude-conversation__empty {
  min-height: 100%;
  display: grid;
  place-content: center;
  justify-items: center;
  gap: 12px;
  color: var(--text-secondary);
  text-align: center;
}

.claude-conversation__empty-prompt {
  max-width: var(--claude-content-width);
  overflow-wrap: anywhere;
  color: var(--text-primary);
  font-size: clamp(20px, 2vw, 26px);
  font-weight: 500;
  line-height: 1.4;
}

.conversation-row {
  width: var(--claude-content-width);
  margin: 0 auto var(--claude-item-gap);
}

.conversation-process__summary {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  padding: 2px 0;
  border: 0;
  color: var(--text-secondary);
  background: transparent;
  cursor: pointer;
  font: inherit;
  font-size: var(--claude-meta-font-size);
  line-height: 1.5;
}

.conversation-process__summary:hover {
  color: var(--text-primary);
}

.conversation-process__summary:focus-visible {
  outline: 2px solid var(--primary);
  outline-offset: 3px;
}

.conversation-process__chevron {
  display: inline-block;
  font-size: 15px;
  line-height: 1;
  transition: transform 140ms ease;
}

.conversation-process__summary[aria-expanded="true"] .conversation-process__chevron {
  transform: rotate(90deg);
}

.conversation-process__divider {
  height: 1px;
  margin: 6px 0 12px;
  background: var(--claude-border-color);
}

.conversation-item {
  width: 100%;
}

.conversation-item--user {
  width: fit-content;
  max-width: 70%;
  margin-left: auto;
  padding: var(--claude-block-padding) 16px;
  border: 1px solid var(--claude-field-border);
  border-radius: 14px 14px 4px 14px;
  background: var(--claude-input-surface);
}

.user-message-actions {
  width: fit-content;
  max-width: 70%;
  min-height: 24px;
  margin: 5px 0 0 auto;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  color: var(--text-secondary);
  font-size: var(--claude-meta-font-size);
  line-height: 1;
  opacity: 0;
  pointer-events: none;
  transition: opacity 120ms ease;
}

.conversation-row--user:hover .user-message-actions,
.conversation-row--user:focus-within .user-message-actions {
  opacity: 1;
  pointer-events: auto;
}

.user-message-actions__copy {
  display: grid;
  width: 24px;
  height: 24px;
  padding: 0;
  place-items: center;
  border: 0;
  border-radius: 6px;
  color: var(--text-secondary);
  background: transparent;
  cursor: pointer;
}

.user-message-actions__copy:hover {
  color: var(--text-primary);
  background: color-mix(in srgb, var(--claude-field-bg) 82%, var(--primary) 18%);
}

.user-message-actions__copy:focus-visible {
  outline: 2px solid var(--primary);
  outline-offset: 1px;
}

.user-message-actions__copy svg {
  width: 17px;
  height: 17px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.8;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.user-message-actions__check-icon,
.user-message-actions__error-icon {
  display: none;
}

.user-message-actions__copy.is-copied {
  color: var(--success);
}

.user-message-actions__copy.is-copied .user-message-actions__copy-icon {
  display: none;
}

.user-message-actions__copy.is-copied .user-message-actions__check-icon {
  display: block;
}

.user-message-actions__copy.is-failed {
  color: var(--danger);
}

.user-message-actions__copy.is-failed .user-message-actions__copy-icon {
  display: none;
}

.user-message-actions__copy.is-failed .user-message-actions__error-icon {
  display: block;
}

@media (hover: none) {
  .user-message-actions,
  .assistant-message-actions {
    opacity: 1;
    pointer-events: auto;
  }
}

.conversation-item--assistant {
  padding: 0;
}

.assistant-message-actions {
  min-height: 20px;
  margin-top: 5px;
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--text-secondary);
  font-size: var(--claude-meta-font-size);
  line-height: 1;
  opacity: 0;
  pointer-events: none;
  transition: opacity 120ms ease;
}

.conversation-row--assistant:hover .assistant-message-actions,
.conversation-row--assistant:focus-within .assistant-message-actions {
  opacity: 1;
  pointer-events: auto;
}

.assistant-message-actions__copy {
  display: grid;
  width: 24px;
  height: 24px;
  padding: 0;
  place-items: center;
  border: 0;
  border-radius: 6px;
  color: var(--text-secondary);
  background: transparent;
  cursor: pointer;
}

.assistant-message-actions__copy:hover {
  color: var(--text-primary);
  background: color-mix(in srgb, var(--claude-field-bg) 82%, var(--primary) 18%);
}

.assistant-message-actions__copy:focus-visible {
  outline: 2px solid var(--primary);
  outline-offset: 1px;
}

.assistant-message-actions__copy svg {
  width: 17px;
  height: 17px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.8;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.assistant-message-actions__check-icon,
.assistant-message-actions__error-icon {
  display: none;
}

.assistant-message-actions__copy.is-copied {
  color: var(--success);
}

.assistant-message-actions__copy.is-copied .assistant-message-actions__copy-icon {
  display: none;
}

.assistant-message-actions__copy.is-copied .assistant-message-actions__check-icon,
.assistant-message-actions__copy.is-failed .assistant-message-actions__error-icon {
  display: block;
}

.assistant-message-actions__copy.is-failed {
  color: var(--danger);
}

.assistant-message-actions__copy.is-failed .assistant-message-actions__copy-icon {
  display: none;
}

.conversation-item__markdown {
  font-size: var(--claude-content-font-size);
  line-height: var(--claude-content-line-height);
  overflow-wrap: anywhere;
}

.conversation-item__markdown :deep(p + p) { margin-top: 10px; }
.conversation-item__markdown :deep(ul),
.conversation-item__markdown :deep(ol) {
  margin: 10px 0;
  padding-left: 24px;
}
.conversation-item__markdown :deep(li + li) { margin-top: 4px; }
.conversation-item__markdown :deep(h1),
.conversation-item__markdown :deep(h2),
.conversation-item__markdown :deep(h3),
.conversation-item__markdown :deep(h4) {
  margin: 14px 0 8px;
  font-size: 1em;
  line-height: var(--claude-content-line-height);
}
.conversation-item__markdown > :deep(:first-child) { margin-top: 0; }
.conversation-item__markdown > :deep(:last-child) { margin-bottom: 0; }
.conversation-item__markdown :deep(.conversation-table-wrap) {
  max-width: 100%;
  margin: 12px 0;
  overflow-x: auto;
  border-radius: 8px;
  scrollbar-width: thin;
}
.conversation-item__markdown :deep(.conversation-table-wrap table) {
  width: 100%;
  min-width: 100%;
  border: 1px solid var(--claude-table-border);
  border-collapse: separate;
  border-spacing: 0;
  border-radius: 8px;
  overflow: hidden;
  background: var(--claude-table-bg);
}
.conversation-item__markdown :deep(.conversation-table-wrap th),
.conversation-item__markdown :deep(.conversation-table-wrap td) {
  padding: 9px 12px;
  border-right: 1px solid var(--claude-table-border);
  border-bottom: 1px solid var(--claude-table-border);
  text-align: left;
  vertical-align: top;
}
.conversation-item__markdown :deep(.conversation-table-wrap th) {
  background: var(--claude-table-header-bg);
  font-weight: 650;
}
.conversation-item__markdown :deep(.conversation-table-wrap tbody tr:nth-child(even) td) {
  background: var(--claude-table-stripe-bg);
}
.conversation-item__markdown :deep(.conversation-table-wrap tr > :last-child) {
  border-right: 0;
}
.conversation-item__markdown :deep(.conversation-table-wrap tbody tr:last-child td) {
  border-bottom: 0;
}
.conversation-item__markdown :deep(.conversation-code-block) {
  position: relative;
  margin: 10px 0;
}
.conversation-item__markdown :deep(.conversation-code-block pre) {
  margin: 0;
  overflow: auto;
  padding: var(--claude-block-padding) 52px var(--claude-block-padding) var(--claude-block-padding);
  border-radius: 8px;
  background: var(--claude-surface-bg);
}
.conversation-item__markdown :deep(.conversation-code-block__copy) {
  position: absolute;
  top: 8px;
  right: 8px;
  z-index: 1;
  display: grid;
  place-items: center;
  width: 34px;
  height: 34px;
  padding: 0;
  border: 1px solid var(--claude-copy-border);
  border-radius: 9px;
  color: var(--text-secondary);
  background: var(--claude-copy-bg);
  box-shadow: 0 3px 10px var(--claude-copy-shadow);
  cursor: pointer;
  transition:
    color 140ms ease,
    border-color 140ms ease,
    background-color 140ms ease,
    box-shadow 140ms ease,
    transform 140ms ease;
}
.conversation-item__markdown :deep(.conversation-code-block__copy:hover) {
  color: var(--text-primary);
  border-color: color-mix(in srgb, var(--claude-copy-border) 45%, var(--primary));
  background: var(--claude-copy-hover-bg);
  transform: translateY(-1px);
}
.conversation-item__markdown :deep(.conversation-code-block__copy:focus-visible) {
  outline: 2px solid var(--primary);
  outline-offset: 2px;
}
.conversation-item__markdown :deep(.conversation-code-block__copy svg) {
  width: 18px;
  height: 18px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.8;
  stroke-linecap: round;
  stroke-linejoin: round;
}
.conversation-item__markdown :deep(.conversation-code-block__check-icon),
.conversation-item__markdown :deep(.conversation-code-block__error-icon) { display: none; }
.conversation-item__markdown :deep(.conversation-code-block__copy.is-copied) {
  color: var(--success);
  border-color: color-mix(in srgb, var(--success) 55%, var(--claude-copy-border));
}
.conversation-item__markdown :deep(.conversation-code-block__copy.is-copied .conversation-code-block__copy-icon) { display: none; }
.conversation-item__markdown :deep(.conversation-code-block__copy.is-copied .conversation-code-block__check-icon) { display: block; }
.conversation-item__markdown :deep(.conversation-code-block__copy.is-failed) {
  color: var(--danger);
  border-color: color-mix(in srgb, var(--danger) 55%, var(--claude-copy-border));
}
.conversation-item__markdown :deep(.conversation-code-block__copy.is-failed .conversation-code-block__copy-icon) { display: none; }
.conversation-item__markdown :deep(.conversation-code-block__copy.is-failed .conversation-code-block__error-icon) { display: block; }
.conversation-item__markdown :deep(code) {
  font-family: "Cascadia Code", Consolas, monospace;
  font-size: inherit;
}

.tool-card {
  border: 1px solid var(--claude-border-color);
  border-radius: 9px;
  background: var(--claude-surface-bg);
  overflow: hidden;
}

.tool-card summary {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: var(--claude-block-padding) 14px;
  cursor: pointer;
  user-select: none;
  font-size: var(--claude-content-font-size);
  line-height: var(--claude-content-line-height);
}

.tool-card summary::-webkit-details-marker { display: none; }
.tool-card summary::marker { content: ""; }
.tool-card__heading {
  min-width: 0;
  flex: 1 1 auto;
  display: flex;
  align-items: baseline;
  gap: 10px;
}
.tool-card__name { flex: 0 0 auto; font-weight: 600; }
.tool-card__preview {
  min-width: 0;
  overflow: hidden;
  color: var(--text-secondary);
  font-family: "Cascadia Code", Consolas, monospace;
  font-size: var(--claude-content-font-size);
  font-weight: 400;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.tool-card__label { flex: 0 0 auto; color: var(--text-secondary); font-size: var(--claude-meta-font-size); }
.tool-card__toggle {
  flex: 0 0 auto;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--text-secondary);
  font-size: var(--claude-meta-font-size);
}
.tool-card__toggle-label::before { content: "展开"; }
.tool-card[open] .tool-card__toggle-label::before { content: "收起"; }
.tool-card__chevron {
  display: inline-block;
  font-size: 18px;
  line-height: 1;
  transition: transform 120ms ease;
}
.tool-card[open] .tool-card__chevron { transform: rotate(90deg); }
.tool-card__section {
  padding: var(--claude-block-padding) 14px;
  border-top: 1px solid var(--claude-border-color);
}
.tool-card__section-title {
  margin-bottom: 8px;
  color: var(--text-secondary);
  font-size: var(--claude-meta-font-size);
  line-height: 1.5;
}
.tool-card pre {
  max-height: 260px;
  margin: 0;
  padding: var(--claude-block-padding);
  overflow: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  border-radius: 6px;
  color: var(--text-primary);
  background: var(--claude-elevated-bg);
  font-family: "Cascadia Code", Consolas, monospace;
  font-size: var(--claude-content-font-size);
  line-height: var(--claude-content-line-height);
}

.question-card {
  padding: 14px;
  border: 1px solid color-mix(in srgb, #58a6ff 52%, var(--claude-border-color));
  border-radius: 10px;
  background: color-mix(in srgb, #58a6ff 8%, var(--claude-surface-bg));
}

.question-card.is-complete {
  border-color: var(--claude-border-color);
  background: var(--claude-surface-bg);
}

.question-card__header,
.question-card__footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.question-card__header > div {
  display: flex;
  align-items: baseline;
  gap: 9px;
  min-width: 0;
}

.question-card__header > div > span,
.question-card__state {
  color: var(--text-secondary);
  font-size: var(--claude-meta-font-size);
}

.question-card__steps {
  display: flex;
  gap: 7px;
  margin-top: 12px;
  overflow-x: auto;
  scrollbar-width: thin;
}

.question-card__step {
  display: inline-flex;
  flex: 0 0 auto;
  gap: 5px;
  align-items: center;
  padding: 4px 8px;
  border: 1px solid var(--claude-field-border);
  border-radius: 999px;
  color: var(--text-secondary);
  font-size: var(--claude-meta-font-size);
  white-space: nowrap;
}

.question-card__step.is-active {
  border-color: var(--primary);
  color: var(--primary);
  background: color-mix(in srgb, var(--primary) 10%, transparent);
}

.question-card__step.is-complete {
  border-color: color-mix(in srgb, var(--success, #3fb950) 45%, var(--claude-field-border));
  color: var(--success, #3fb950);
}

.question-card__question {
  margin-top: 16px;
}

.question-card__prompt p {
  margin: 5px 0 10px;
  color: var(--text-primary);
  font-weight: 600;
}

.question-card__header-label {
  display: inline-block;
  color: var(--text-secondary);
  font-size: var(--claude-meta-font-size);
  letter-spacing: 0.03em;
  text-transform: uppercase;
}

.question-card__options {
  display: grid;
  gap: 7px;
}

.question-option {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  width: 100%;
  padding: 10px 12px;
  border: 1px solid var(--claude-field-border);
  border-radius: 8px;
  color: var(--text-primary);
  background: var(--claude-field-bg);
  font: inherit;
  text-align: left;
  cursor: pointer;
}

.question-option:hover:not(:disabled) {
  border-color: color-mix(in srgb, var(--primary) 70%, var(--claude-field-border));
  background: color-mix(in srgb, var(--primary) 9%, var(--claude-field-bg));
}

.question-option.is-selected {
  border-color: var(--primary);
  background: color-mix(in srgb, var(--primary) 14%, var(--claude-field-bg));
}

.question-option:disabled {
  cursor: default;
  opacity: 0.72;
}

.question-option__marker {
  display: grid;
  flex: 0 0 18px;
  width: 18px;
  height: 18px;
  place-items: center;
  border: 1px solid var(--claude-field-border);
  border-radius: 5px;
  color: var(--primary);
  font-size: 13px;
  font-weight: 700;
  line-height: 1;
}

.question-option.is-selected .question-option__marker {
  border-color: var(--primary);
  background: var(--primary);
  color: #fff;
}

.question-option__content {
  display: grid;
  gap: 3px;
  min-width: 0;
}

.question-option__content small {
  color: var(--text-secondary);
  font-size: var(--claude-meta-font-size);
  line-height: 1.45;
}

.question-card__custom-answer {
  display: grid;
  gap: 6px;
  margin-top: 9px;
  color: var(--text-secondary);
  font-size: var(--claude-meta-font-size);
}

.question-card__custom-answer textarea {
  width: 100%;
  min-height: 56px;
  padding: 8px 10px;
  resize: vertical;
  border: 1px solid var(--claude-field-border);
  border-radius: 8px;
  color: var(--text-primary);
  background: var(--claude-field-bg);
  font: inherit;
  line-height: 1.45;
}

.question-card__custom-answer textarea:focus {
  outline: none;
  border-color: var(--primary);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--primary) 22%, transparent);
}

.question-card__custom-answer textarea:disabled {
  cursor: not-allowed;
  opacity: 0.72;
}

.question-card__complete-summary {
  margin-top: 16px;
  color: var(--text-secondary);
  font-size: var(--claude-content-font-size);
}

.question-card__footer {
  margin-top: 14px;
}

.question-card__error {
  min-width: 0;
  overflow-wrap: anywhere;
  color: var(--danger, #f85149);
  font-size: var(--claude-meta-font-size);
}

.question-card__sent {
  color: var(--success, #3fb950);
  font-size: var(--claude-meta-font-size);
}

.question-card__submit {
  flex: 0 0 auto;
}

.permission-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: var(--claude-block-padding) 14px;
  border: 1px solid color-mix(in srgb, #58a6ff 50%, transparent);
  border-radius: 9px;
  background: color-mix(in srgb, #58a6ff 10%, var(--claude-elevated-bg));
}
.permission-card p { margin: 8px 0 0; color: var(--text-secondary); }
.status-card {
  padding: var(--claude-block-padding) 0;
  color: var(--text-secondary);
  font-size: var(--claude-content-font-size);
  text-align: center;
}

.claude-conversation__jump {
  grid-column: 3;
  grid-row: 1;
  justify-self: end;
  z-index: 2;
  display: grid;
  place-items: center;
  width: 48px;
  height: 48px;
  padding: 0;
  border: 1px solid var(--claude-jump-border);
  border-radius: 50%;
  color: var(--text-primary);
  background: var(--claude-jump-bg);
  box-shadow: 0 5px 18px var(--claude-jump-shadow);
  cursor: pointer;
  pointer-events: auto;
}

.claude-conversation__jump:hover {
  background: var(--claude-jump-hover-bg);
}

.claude-conversation__jump:active {
  transform: translateY(1px);
}

.claude-conversation__jump:focus-visible {
  outline: 2px solid var(--primary);
  outline-offset: 3px;
}

.claude-conversation__jump svg {
  width: 26px;
  height: 26px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.8;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.claude-composer {
  position: relative;
  flex: 0 0 auto;
  padding: var(--claude-block-padding) clamp(18px, 6vw, 84px);
  border-top: 0;
  background: transparent;
}

/* Fade the last lines of history into the fixed page color.  This layer is
   attached to the composer so it stays above the scrolling content while
   leaving the conversation background itself completely flat. */
.claude-composer::before {
  content: '';
  position: absolute;
  top: -72px;
  right: 0;
  left: 0;
  z-index: 1;
  height: 72px;
  pointer-events: none;
  background: linear-gradient(to bottom, transparent 0%, var(--claude-page-bg) 100%);
}

.claude-composer__topbar {
  position: absolute;
  left: 50%;
  bottom: calc(100% + 8px);
  z-index: 4;
  width: var(--claude-content-width);
  min-height: 46px;
  margin: 0;
  display: grid;
  grid-template-columns: 48px minmax(0, 1fr) 48px;
  align-items: center;
  pointer-events: none;
  transform: translateX(-50%);
}

.claude-activity {
  grid-column: 2;
  grid-row: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 7px;
  justify-self: center;
  width: max-content;
  max-width: 100%;
  padding: 7px 12px;
  overflow: hidden;
  border: 1px solid var(--claude-activity-border);
  border-radius: 999px;
  background: var(--claude-activity-bg);
  box-shadow: 0 5px 18px var(--claude-activity-shadow);
  backdrop-filter: blur(8px);
  font-family: "Cascadia Code", Consolas, monospace;
  font-size: var(--claude-content-font-size);
  line-height: 1.5;
  white-space: nowrap;
}

.claude-activity__spinner {
  position: relative;
  flex: 0 0 auto;
  width: 17px;
  height: 17px;
  color: var(--claude-activity-color);
  font-size: 17px;
  line-height: 1;
}

.claude-activity__spinner > span {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  opacity: 0;
  animation: claude-activity-frame 720ms steps(1, end) infinite;
}

.claude-activity__spinner > span:nth-child(2) { animation-delay: 120ms; }
.claude-activity__spinner > span:nth-child(3) { animation-delay: 240ms; }
.claude-activity__spinner > span:nth-child(4) { animation-delay: 360ms; }
.claude-activity__spinner > span:nth-child(5) { animation-delay: 480ms; }
.claude-activity__spinner > span:nth-child(6) { animation-delay: 600ms; }

.claude-activity__label {
  flex: 0 0 auto;
  color: var(--claude-activity-color);
  font-weight: 600;
}

.claude-activity__details {
  min-width: 0;
  overflow: hidden;
  color: var(--text-secondary);
  text-overflow: ellipsis;
}

.claude-activity__token-count {
  display: inline-block;
  font-variant-numeric: tabular-nums;
}

.claude-prompt-queue {
  display: grid;
  position: relative;
  z-index: 1;
  gap: 6px;
  width: var(--claude-content-width);
  max-height: 194px;
  margin: 0 auto 10px;
  overflow-y: auto;
  scrollbar-width: thin;
}

.claude-prompt-queue__item {
  display: grid;
  grid-template-columns: 22px minmax(0, 1fr) auto auto auto;
  gap: 8px;
  min-height: 42px;
  padding: 7px 9px;
  align-items: center;
  border: 1px solid var(--claude-queue-border);
  border-radius: 11px;
  background: var(--claude-queue-bg);
}

.claude-prompt-queue__index {
  display: grid;
  place-items: center;
  width: 20px;
  height: 20px;
  border-radius: 50%;
  color: var(--text-secondary);
  background: color-mix(in srgb, var(--claude-queue-border) 58%, transparent);
  font-size: var(--claude-meta-font-size);
  font-variant-numeric: tabular-nums;
}

.claude-prompt-queue__text {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.claude-prompt-queue__mode {
  color: var(--text-secondary);
  font-size: var(--claude-meta-font-size);
  white-space: nowrap;
}

.claude-prompt-queue__action {
  display: inline-flex;
  gap: 4px;
  min-height: 28px;
  padding: 3px 7px;
  align-items: center;
  border: 0;
  border-radius: 7px;
  color: var(--text-secondary);
  background: transparent;
  font: inherit;
  font-size: var(--claude-meta-font-size);
  cursor: pointer;
}

.claude-prompt-queue__action:hover:not(:disabled) {
  color: var(--text-primary);
  background: var(--claude-queue-hover-bg);
}

.claude-prompt-queue__action:focus-visible {
  outline: 2px solid var(--primary);
  outline-offset: 1px;
}

.claude-prompt-queue__action:disabled {
  cursor: wait;
  opacity: 0.48;
}

.claude-prompt-queue__action svg {
  width: 16px;
  height: 16px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.8;
  stroke-linecap: round;
  stroke-linejoin: round;
}

@keyframes claude-activity-frame {
  0%, 16.66% { opacity: 1; }
  16.67%, 100% { opacity: 0; }
}

@media (max-width: 680px) {
  .claude-prompt-queue__item {
    grid-template-columns: 22px minmax(0, 1fr) auto auto;
  }

  .claude-prompt-queue__mode {
    display: none;
  }

  .claude-prompt-queue__action span {
    display: none;
  }

  .claude-prompt-queue__action {
    width: 28px;
    padding: 0;
    justify-content: center;
  }
}

.claude-composer__input-area {
  position: relative;
  z-index: 1;
  width: var(--claude-content-width);
  margin: 0 auto;
}

.claude-composer__command-menu {
  position: absolute;
  right: 0;
  bottom: calc(100% + 10px);
  left: 0;
  z-index: 7;
  max-height: min(320px, 46vh);
  padding: 6px;
  overflow-y: auto;
  border: 1px solid var(--claude-composer-border);
  border-radius: 12px;
  background: var(--card);
  box-shadow: 0 14px 34px var(--claude-shadow-medium);
}

.claude-composer__command-menu-title {
  padding: 5px 9px 7px;
  color: var(--text-secondary);
  font-size: var(--claude-meta-font-size);
}

.claude-composer__command-option {
  display: grid;
  width: 100%;
  grid-template-columns: minmax(132px, auto) minmax(0, 1fr) auto;
  gap: 12px;
  min-height: 40px;
  padding: 8px 10px;
  align-items: center;
  border: 0;
  border-radius: 8px;
  color: var(--text-primary);
  background: transparent;
  font: inherit;
  text-align: left;
  cursor: pointer;
}

.claude-composer__command-option:hover,
.claude-composer__command-option.is-selected {
  background: color-mix(in srgb, var(--primary) 12%, var(--bg));
}

.claude-composer__command-option:focus-visible {
  outline: 2px solid var(--primary);
  outline-offset: -2px;
}

.claude-composer__command-option code {
  overflow: hidden;
  color: var(--primary);
  background: transparent;
  font-family: "Cascadia Code", Consolas, monospace;
  font-size: var(--claude-content-font-size);
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.claude-composer__command-option > span {
  min-width: 0;
  overflow: hidden;
  color: var(--text-secondary);
  font-size: var(--claude-meta-font-size);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.claude-composer__command-option small {
  padding: 2px 6px;
  border-radius: 999px;
  color: var(--text-secondary);
  background: color-mix(in srgb, var(--claude-composer-border) 58%, transparent);
  font-size: 10px;
  line-height: 1.4;
  text-transform: uppercase;
}

.claude-composer__command-notice {
  position: absolute;
  bottom: calc(100% + 12px);
  left: 50%;
  z-index: 8;
  padding: 7px 12px;
  border: 1px solid color-mix(in srgb, #f85149 48%, var(--claude-composer-border));
  border-radius: 999px;
  color: #f85149;
  background: var(--card);
  box-shadow: 0 8px 24px var(--claude-shadow-medium);
  font-size: var(--claude-meta-font-size);
  white-space: nowrap;
  transform: translateX(-50%);
}

.claude-command-notice-enter-active,
.claude-command-notice-leave-active {
  transition: opacity 140ms ease, transform 140ms ease;
}

.claude-command-notice-enter-from,
.claude-command-notice-leave-to {
  opacity: 0;
  transform: translate(-50%, 5px);
}

.claude-composer__input-shell {
  width: 100%;
  margin: 0;
  padding: var(--claude-block-padding) 8px 8px 14px;
  border: 1px solid var(--claude-composer-border);
  border-radius: 14px;
  background: var(--claude-composer-bg);
  box-shadow:
    0 8px 20px var(--claude-shadow-soft),
    0 2px 5px rgba(0, 0, 0, 0.10);
  transform: translateY(-2px);
  transition:
    border-color 140ms ease,
    background-color 140ms ease,
    box-shadow 140ms ease,
    transform 140ms ease;
}

.claude-composer textarea {
  display: block;
  width: 100%;
  height: 3.1em;
  min-height: 3.1em;
  max-height: 13.95em;
  margin: 0;
  padding: 0 6px 0 0;
  resize: none;
  overflow-y: hidden;
  border: 0;
  color: var(--text-primary);
  background: transparent;
  box-shadow: none;
  font: inherit;
  font-size: var(--claude-content-font-size);
  line-height: var(--claude-content-line-height);
  outline: none;
  scrollbar-width: thin;
  scrollbar-color: var(--claude-composer-border-hover) transparent;
}

.claude-composer__input-shell:hover:not(.is-disabled):not(:focus-within) {
  border-color: var(--claude-composer-border-hover);
  box-shadow:
    0 10px 24px var(--claude-shadow-medium),
    0 3px 7px rgba(0, 0, 0, 0.11);
}

.claude-composer__input-shell:focus-within {
  border-color: var(--claude-composer-border-focus);
  background: var(--claude-composer-bg);
  box-shadow:
    0 12px 28px var(--claude-shadow-medium),
    0 3px 8px rgba(0, 0, 0, 0.12),
    0 0 0 2px var(--claude-composer-ring);
  transform: translateY(-3px);
}

.claude-composer__input-shell.is-disabled {
  opacity: 0.65;
  box-shadow: none;
  transform: none;
}

.claude-composer textarea:disabled { cursor: not-allowed; }

.claude-composer__actions {
  display: flex;
  height: 40px;
  padding-top: 8px;
  align-items: flex-end;
  justify-content: space-between;
}

.claude-composer__actions-spacer {
  flex: 1;
}

.claude-composer__context-usage {
  position: relative;
  display: inline-flex;
  flex: 0 0 auto;
  width: 32px;
  height: 32px;
  align-items: center;
  justify-content: center;
  margin-right: 6px;
  border-radius: 8px;
  color: var(--text-secondary);
  line-height: 1;
  user-select: none;
}

.claude-composer__context-usage:hover {
  background: color-mix(in srgb, var(--claude-field-bg) 64%, transparent);
}

.claude-composer__context-usage.is-warning {
  color: #a57500;
}

.claude-composer__context-usage.is-critical {
  color: var(--danger);
}

[data-theme="dark"] .claude-composer__context-usage.is-warning {
  color: #f0c85a;
}

.claude-composer__context-ring {
  width: 18px;
  height: 18px;
  display: block;
}

.claude-composer__context-ring-track {
  fill: none;
  stroke: color-mix(in srgb, currentColor 22%, transparent);
  stroke-width: 2.5;
}

.claude-composer__context-ring-value {
  fill: none;
  stroke: currentColor;
  stroke-width: 2.5;
  stroke-linecap: round;
  transition: stroke-dashoffset 0.35s ease;
}

.claude-composer__context-tooltip {
  position: absolute;
  bottom: calc(100% + 8px);
  left: 50%;
  z-index: 40;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 6px 10px;
  border: 1px solid color-mix(in srgb, var(--claude-composer-border) 72%, transparent);
  border-radius: 8px;
  background: var(--claude-field-bg);
  box-shadow: 0 6px 20px rgb(0 0 0 / 0.16);
  color: var(--text-primary);
  font-size: var(--claude-meta-font-size);
  white-space: nowrap;
  opacity: 0;
  visibility: hidden;
  transform: translateX(-50%) translateY(3px);
  transition: opacity 0.12s ease, transform 0.12s ease, visibility 0.12s;
  pointer-events: none;
}

.claude-composer__context-usage:hover .claude-composer__context-tooltip {
  opacity: 1;
  visibility: visible;
  transform: translateX(-50%) translateY(0);
}

.claude-composer__context-tooltip-main {
  font-variant-numeric: tabular-nums;
  font-weight: 600;
}

.claude-composer__permission-mode {
  display: inline-flex;
  min-width: 0;
  max-width: min(150px, 25vw);
  height: 32px;
  align-items: center;
  margin-left: 8px;
  padding: 0 8px;
  overflow: hidden;
  border: 1px solid color-mix(in srgb, var(--claude-composer-border) 72%, transparent);
  border-radius: 8px;
  color: #747b84;
  background: color-mix(in srgb, var(--claude-field-bg) 64%, transparent);
  font-size: var(--claude-meta-font-size);
  line-height: 1;
  text-overflow: ellipsis;
  white-space: nowrap;
  user-select: none;
}

.claude-composer__permission-mode.is-bypass {
  max-width: min(178px, 32vw);
  padding-right: 14px;
  color: #c51d71;
}

.claude-composer__permission-mode.is-auto { color: #a57500; }
.claude-composer__permission-mode.is-manual { color: #747b84; }
.claude-composer__permission-mode.is-accept-edits { color: #8250b8; }
.claude-composer__permission-mode.is-plan { color: #087f83; }

[data-theme="dark"] .claude-composer__permission-mode.is-bypass { color: #ff73ad; }
[data-theme="dark"] .claude-composer__permission-mode.is-auto { color: #f0c85a; }
[data-theme="dark"] .claude-composer__permission-mode.is-manual { color: #aab1ba; }
[data-theme="dark"] .claude-composer__permission-mode.is-accept-edits { color: #c792ea; }
[data-theme="dark"] .claude-composer__permission-mode.is-plan { color: #55d3ca; }

.claude-composer__model-picker-shell {
  position: relative;
  flex: 0 1 auto;
  min-width: 0;
}

.claude-composer__model-picker {
  position: relative;
  flex: 0 1 auto;
  min-width: 0;
  margin-right: 8px;
}

.claude-composer__model-picker[open] {
  z-index: 5;
}

.claude-composer__model-popover {
  position: absolute;
  right: 0;
  bottom: calc(100% + 7px);
  z-index: 7;
  display: flex;
  align-items: flex-end;
  gap: 7px;
  width: max-content;
  max-width: calc(100vw - 16px);
}

.claude-composer__model-trigger {
  display: flex;
  max-width: min(260px, 30vw);
  height: 32px;
  align-items: center;
  gap: 5px;
  padding: 0 8px;
  overflow: hidden;
  border: 1px solid transparent;
  border-radius: 8px;
  color: var(--text-secondary);
  font-size: var(--claude-meta-font-size);
  list-style: none;
  cursor: pointer;
  user-select: none;
  transition:
    border-color 140ms ease,
    background-color 140ms ease,
    color 140ms ease;
}

.claude-composer__model-trigger::-webkit-details-marker {
  display: none;
}

.claude-composer__model-trigger:hover {
  border-color: var(--input-border);
  color: var(--text-primary);
  background: color-mix(in srgb, var(--input-bg) 78%, transparent);
}

.claude-composer__model-picker[open] .claude-composer__model-trigger {
  border-color: var(--input-border);
  color: var(--text-primary);
  background: var(--input-bg);
}

.claude-composer__model-picker.is-disabled .claude-composer__model-trigger {
  cursor: not-allowed;
  opacity: 0.55;
}

.claude-composer__model-label {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.claude-composer__model-trigger svg {
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

.claude-composer__model-picker[open] .claude-composer__model-trigger svg {
  transform: rotate(180deg);
}

.claude-composer__model-menu {
  position: static;
  display: flex;
  width: max-content;
  min-width: 210px;
  max-width: min(420px, calc(100vw - 16px));
  max-height: min(420px, 65vh);
  flex-direction: column;
  padding: 5px;
  overflow-y: auto;
  border: 1px solid var(--input-border);
  border-radius: 10px;
  background: var(--card);
  box-shadow: 0 10px 26px rgba(0, 0, 0, 0.18);
}

.claude-composer__model-submenu {
  position: static;
  display: flex;
  width: max-content;
  min-width: 210px;
  max-width: min(420px, calc(100vw - 16px));
  max-height: min(300px, 50vh);
  flex-direction: column;
  padding: 5px;
  overflow-y: auto;
  border: 1px solid var(--input-border);
  border-radius: 10px;
  background: var(--card);
  box-shadow: 0 10px 26px rgba(0, 0, 0, 0.18);
}

.claude-composer__model-group {
  border-bottom: 1px solid var(--separator);
}

.claude-composer__model-group-trigger {
  display: flex;
  width: 100%;
  min-height: 34px;
  align-items: center;
  gap: 8px;
  padding: 7px 8px;
  border: 0;
  color: var(--text-primary);
  background: transparent;
  font: inherit;
  font-size: var(--claude-meta-font-size);
  list-style: none;
  cursor: pointer;
  text-align: left;
  user-select: none;
}

.claude-composer__model-group-trigger > span:first-child {
  flex: 0 0 auto;
  white-space: nowrap;
}

.claude-composer__model-group-trigger::-webkit-details-marker {
  display: none;
}

.claude-composer__model-group-trigger:hover {
  border-radius: 7px;
  background: var(--bg);
}

.claude-composer__model-group-value {
  flex: 1 1 auto;
  min-width: 0;
  margin-left: auto;
  overflow: hidden;
  color: var(--text-secondary);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.claude-composer__model-group-trigger svg {
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

.claude-composer__model-group-trigger.is-active svg {
  transform: rotate(180deg);
}

.claude-composer__model-options {
  padding: 0 5px 5px;
}

.claude-composer__model-option {
  display: flex;
  width: 100%;
  min-width: 0;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 8px 9px;
  border: 0;
  border-radius: 7px;
  color: var(--text-primary);
  background: transparent;
  font: inherit;
  font-size: var(--claude-meta-font-size);
  text-align: left;
  cursor: pointer;
}

.claude-composer__model-option:hover:not(:disabled) {
  background: var(--bg);
}

.claude-composer__model-option.is-selected {
  color: var(--primary);
  font-weight: 600;
}

.claude-composer__model-option:disabled {
  cursor: default;
}

.claude-composer__model-option-name {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.claude-composer__model-option svg {
  flex: 0 0 15px;
  width: 15px;
  height: 15px;
  fill: none;
  stroke: currentColor;
  stroke-linecap: round;
  stroke-linejoin: round;
  stroke-width: 2;
}

.claude-composer__model-empty {
  padding: 9px;
  color: var(--text-secondary);
  font-size: var(--claude-meta-font-size);
  line-height: 1.4;
}

.claude-composer__model-reset {
  width: 100%;
  margin-top: 5px;
  padding: 8px 9px;
  border: 0;
  border-radius: 7px;
  color: var(--text-secondary);
  background: transparent;
  font: inherit;
  font-size: var(--claude-meta-font-size);
  text-align: left;
  cursor: pointer;
}

.claude-composer__model-reset:hover:not(:disabled) {
  color: var(--text-primary);
  background: var(--bg);
}

.claude-composer__model-reset:disabled {
  cursor: default;
  opacity: 0.55;
}

@media (prefers-reduced-motion: reduce) {
  .claude-composer__input-shell,
  .claude-composer__send,
  .conversation-item__markdown :deep(.conversation-code-block__copy) { transition: none; }

  .claude-activity__spinner > span { animation: none; }
  .claude-activity__spinner > span:not(:nth-child(4)) { display: none; }
  .claude-activity__spinner > span:nth-child(4) { opacity: 1; }
}

@media (forced-colors: active) {
  .claude-conversation__jump {
    border-color: ButtonText;
    color: ButtonText;
    background: Canvas;
  }

  .claude-activity__spinner,
  .claude-activity__label { color: Highlight; }

  .conversation-item__markdown :deep(.conversation-code-block__copy) {
    border-color: ButtonText;
    color: ButtonText;
    background: Canvas;
  }

  .conversation-item__markdown :deep(.conversation-table-wrap table),
  .conversation-item__markdown :deep(.conversation-table-wrap th),
  .conversation-item__markdown :deep(.conversation-table-wrap td) {
    border-color: CanvasText;
  }

  .claude-prompt-queue__item,
  .claude-prompt-queue__action {
    border: 1px solid CanvasText;
  }

  .claude-conversation__jump:focus-visible,
  .claude-composer__model-trigger:focus-visible,
  .claude-composer__send:focus-visible,
  .conversation-item__markdown :deep(.conversation-code-block__copy:focus-visible),
  .claude-composer__input-shell:focus-within {
    outline: 2px solid Highlight;
    outline-offset: 2px;
  }
}

@media (max-width: 560px) {
  .claude-composer__command-option {
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 4px 8px;
  }

  .claude-composer__command-option > span {
    grid-column: 1 / -1;
  }

  .claude-composer__command-option small {
    grid-column: 2;
    grid-row: 1;
  }

  .claude-composer__model-trigger {
    max-width: min(170px, 35vw);
  }

  .claude-composer__model-menu {
    right: -2px;
    max-width: min(300px, 80vw);
  }
}

.claude-composer__send {
  display: grid;
  flex: 0 0 32px;
  width: 32px;
  height: 32px;
  padding: 0;
  place-items: center;
  border: 0;
  border-radius: 9px;
  color: var(--claude-send-color);
  background: var(--claude-send-bg);
  box-shadow: 0 2px 7px rgba(0, 0, 0, 0.18);
  cursor: not-allowed;
  opacity: 0.82;
  transition:
    background-color 140ms ease,
    box-shadow 140ms ease,
    opacity 140ms ease,
    transform 140ms ease;
}

.claude-composer__send.is-ready {
  background: var(--claude-send-ready-bg);
  box-shadow: 0 3px 9px rgba(0, 0, 0, 0.24);
  cursor: pointer;
  opacity: 1;
}

.claude-composer__send.is-ready:hover {
  transform: translateY(-1px);
}

.claude-composer__send.is-ready:active {
  transform: translateY(0);
}

.claude-composer__send.is-stop.is-pending {
  cursor: wait;
  opacity: 0.82;
}

.claude-composer__send:focus-visible {
  outline: 2px solid var(--text-primary);
  outline-offset: 2px;
}

.claude-composer__send svg {
  width: 18px;
  height: 18px;
  fill: none;
  stroke: currentColor;
  stroke-width: 2;
  stroke-linecap: round;
  stroke-linejoin: round;
}

@media (forced-colors: active) {
  .claude-composer__send,
  .claude-composer__send.is-ready {
    border: 1px solid ButtonText;
    color: ButtonText;
    background: ButtonFace;
    opacity: 1;
  }

  .claude-composer__send:focus-visible {
    outline: 2px solid Highlight;
    outline-offset: 2px;
  }
}

.claude-composer__footer {
  position: relative;
  z-index: 1;
  width: var(--claude-content-width);
  margin: 7px auto 0;
  display: flex;
  align-items: center;
  color: var(--text-secondary);
  font-size: var(--claude-meta-font-size);
  line-height: 1.5;
}

.claude-composer__error { color: #f85149; }

.claude-drop-overlay {
  position: absolute;
  inset: 0;
  z-index: 50;
  display: grid;
  place-items: center;
  background: color-mix(in srgb, var(--claude-page-bg) 80%, transparent);
  backdrop-filter: blur(2px);
  pointer-events: none;
}

.claude-drop-overlay__inner {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  padding: 28px 40px;
  border: 2px dashed var(--primary, #58a6ff);
  border-radius: 16px;
  color: var(--primary, #58a6ff);
  background: color-mix(in srgb, var(--primary, #58a6ff) 8%, var(--claude-elevated-bg));
  font-size: 15px;
  font-weight: 500;
}

.claude-drop-overlay__inner svg {
  width: 36px;
  height: 36px;
}

.claude-conversation.is-drag-over {
  outline: 2px dashed var(--primary, #58a6ff);
  outline-offset: -2px;
}

.claude-composer__attachments {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 0 6px 8px 0;
}

.claude-composer__attach {
  display: grid;
  flex: 0 0 32px;
  width: 32px;
  height: 32px;
  padding: 0;
  place-items: center;
  border: 1px solid var(--claude-composer-border);
  border-radius: 9px;
  color: var(--text-secondary);
  background: transparent;
  cursor: pointer;
  transition: color 140ms ease, border-color 140ms ease, background-color 140ms ease;
}

.claude-composer__attach:hover:not(:disabled) {
  color: var(--text-primary);
  border-color: var(--claude-composer-border-hover);
  background: color-mix(in srgb, var(--claude-field-bg) 60%, transparent);
}

.claude-composer__attach:focus-visible {
  outline: 2px solid var(--primary);
  outline-offset: 2px;
}

.claude-composer__attach:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.claude-composer__attach svg {
  width: 16px;
  height: 16px;
  fill: none;
  stroke: currentColor;
  stroke-width: 2;
  stroke-linecap: round;
}
</style>
