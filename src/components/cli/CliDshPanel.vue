<template>
  <DshConfigPanel
    :sidebar-collapsed="sidebarCollapsed"
    @left-width-change="emit('left-width-change', $event)"
    @open-runtime="emit('open-runtime')"
  />
</template>

<script setup lang="ts">
import DshConfigPanel from '@/components/dsh/DshConfigPanel.vue'

// dsh 的配置面板自己带侧边栏（供应商清单），所以这里既转发折叠状态，也把
// 拖拽出来的宽度上报给工作区——与其它三个 CLI 面板同一套契约。
defineProps<{
  sidebarCollapsed?: boolean
}>()
const emit = defineEmits<{
  (event: 'left-width-change', width: number): void
  (event: 'open-runtime'): void
}>()
</script>
