<template>
  <div class="conv-attachment" :title="path">
    <div class="conv-attachment__preview">
      <img
        v-if="isImage && previewUrl"
        :src="previewUrl"
        class="conv-attachment__thumbnail"
        draggable="false"
        alt=""
      />
      <span v-else class="conv-attachment__icon" aria-hidden="true">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
          <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
          <polyline points="14,2 14,8 20,8" />
        </svg>
      </span>
    </div>
    <span class="conv-attachment__name">{{ name }}</span>
    <button
      class="conv-attachment__remove"
      type="button"
      :aria-label="`移除 ${name}`"
      @click="$emit('remove')"
    >
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
        <line x1="18" y1="6" x2="6" y2="18" />
        <line x1="6" y1="6" x2="18" y2="18" />
      </svg>
    </button>
  </div>
</template>

<script setup lang="ts">
defineProps<{
  path: string
  name: string
  isImage: boolean
  previewUrl: string | null
}>()
defineEmits<{ remove: [] }>()
</script>

<style scoped>
.conv-attachment {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  max-width: 180px;
  padding: 4px 6px 4px 5px;
  border: 1px solid var(--claude-composer-border);
  border-radius: 8px;
  background: var(--claude-composer-bg);
  font-size: 12px;
  line-height: 1.4;
  color: var(--text-primary);
  cursor: default;
  vertical-align: middle;
}

.conv-attachment__preview {
  flex: 0 0 auto;
  width: 28px;
  height: 28px;
  border-radius: 4px;
  overflow: hidden;
  display: grid;
  place-items: center;
  background: color-mix(in srgb, var(--claude-field-bg) 80%, transparent);
}

.conv-attachment__thumbnail {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.conv-attachment__icon {
  width: 16px;
  height: 16px;
  color: var(--text-secondary);
}

.conv-attachment__icon svg {
  width: 100%;
  height: 100%;
}

.conv-attachment__name {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.conv-attachment__remove {
  flex: 0 0 auto;
  display: grid;
  place-items: center;
  width: 18px;
  height: 18px;
  padding: 0;
  border: 0;
  border-radius: 4px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  opacity: 0.7;
}

.conv-attachment__remove:hover {
  opacity: 1;
  background: color-mix(in srgb, var(--claude-field-bg) 70%, var(--danger, #f85149) 30%);
  color: var(--danger, #f85149);
}

.conv-attachment__remove svg {
  width: 12px;
  height: 12px;
}
</style>
