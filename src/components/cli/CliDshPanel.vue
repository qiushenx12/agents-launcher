<template>
  <DshConfigPanel @left-width-change="emit('left-width-change', $event)" />
</template>

<script setup lang="ts">
import DshConfigPanel from '@/components/dsh/DshConfigPanel.vue'

// `sidebarCollapsed` is accepted for symmetry with the other CLI panels even
// though the dsh panel has no sidebar of its own.
defineProps<{
  sidebarCollapsed?: boolean
}>()
const emit = defineEmits<{
  (event: 'left-width-change', width: number): void
}>()
</script>
