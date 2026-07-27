<template>
  <ClaudePanel
    :sidebar-collapsed="sidebarCollapsed"
    @left-width-change="emit('left-width-change', $event)"
  />
</template>

<script setup lang="ts">
import ClaudePanel from '@/components/claude/ClaudePanel.vue'

defineProps<{
  sidebarCollapsed?: boolean
}>()
const emit = defineEmits<{
  (event: 'left-width-change', width: number): void
}>()
</script>
