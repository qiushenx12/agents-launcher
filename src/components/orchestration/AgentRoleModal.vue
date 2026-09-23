<script setup lang="ts">
import { useTabCommStore } from '@/stores/tabComm'

const tabComm = useTabCommStore()
</script>

<template>
  <div class="modal-overlay" @click.self="tabComm.closeRoleConfig">
    <div class="role-modal">
      <div class="role-modal__header">Agent 角色配置</div>

      <div class="role-modal__body">
        <div class="role-field">
          <label class="role-field__label">角色名称</label>
          <input
            type="text"
            class="role-field__input"
            v-model="tabComm.roleName"
            placeholder="例如：前端架构师"
          />
        </div>

        <div class="role-field">
          <label class="role-field__label">角色描述</label>
          <input
            type="text"
            class="role-field__input"
            v-model="tabComm.roleDescription"
            placeholder="负责组件设计和代码审查"
          />
        </div>

        <div class="role-field">
          <label class="role-field__label">系统提示词</label>
          <textarea
            class="role-field__textarea"
            v-model="tabComm.roleSystemPrompt"
            rows="6"
            placeholder="你是一位资深前端架构师，擅长 Vue/React 组件设计..."
          />
        </div>
      </div>

      <div class="role-modal__footer">
        <button class="btn btn-secondary" @click="tabComm.closeRoleConfig">取消</button>
        <button class="btn btn-primary" @click="tabComm.saveRole">保存</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.modal-overlay {
  position: fixed;
  inset: 0;
  background: var(--overlay);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}

.role-modal {
  background: var(--card);
  border-radius: var(--radius);
  width: 480px;
  max-height: 80vh;
  overflow-y: auto;
  box-shadow: var(--modal-shadow);
}

.role-modal__header {
  padding: 16px 20px;
  border-bottom: 1px solid var(--separator);
  font-size: var(--font-size-title);
  font-weight: 600;
  color: var(--text-primary);
}

.role-modal__body {
  padding: 16px 20px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.role-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.role-field__label {
  font-size: var(--font-size-small);
  font-weight: 600;
  color: var(--text-secondary);
}

.role-field__input,
.role-field__textarea {
  padding: 8px 10px;
  background: var(--bg);
  border: 1px solid var(--separator);
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: var(--font-size-base);
  outline: none;
  transition: border-color 0.12s ease;
  resize: vertical;
}

.role-field__input:focus,
.role-field__textarea:focus {
  border-color: var(--primary);
}

.role-field__textarea {
  font-family: inherit;
  line-height: 1.5;
  min-height: 100px;
}

.role-modal__footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 20px;
  border-top: 1px solid var(--separator);
}
</style>
