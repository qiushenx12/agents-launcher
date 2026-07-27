<template>
  <OpenCodeConfigPanel
    :sidebar-collapsed="sidebarCollapsed"
    @left-width-change="emit('left-width-change', $event)"
  />
</template>

<script setup lang="ts">
import OpenCodeConfigPanel from '@/components/opencode/OpenCodeConfigPanel.vue'

defineProps<{
  sidebarCollapsed?: boolean
}>()
const emit = defineEmits<{
  (event: 'left-width-change', width: number): void
}>()
</script>
