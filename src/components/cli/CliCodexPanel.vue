<template>
  <CodexConfigPanel
    :sidebar-collapsed="sidebarCollapsed"
    @left-width-change="emit('left-width-change', $event)"
  />
</template>

<script setup lang="ts">
import CodexConfigPanel from '@/components/codex/CodexConfigPanel.vue'

defineProps<{
  sidebarCollapsed?: boolean
}>()
const emit = defineEmits<{
  (event: 'left-width-change', width: number): void
}>()
</script>
