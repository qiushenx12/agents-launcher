import { defineComponent, h } from 'vue'

export default defineComponent({
  name: 'AsyncPanelLoading',
  setup() {
    return () => h('div', {
      class: 'async-panel-loading',
      role: 'status',
      'aria-label': '正在加载',
    }, [
      h('span', {
        class: 'async-panel-loading__spinner',
        'aria-hidden': 'true',
      }),
    ])
  },
})

