# Ponytail Review 优化任务清单

本清单基于对全代码库的过度工程化审查，目标是删除或简化不必要的复杂度。**只改复杂度，不动正确性、安全性和用户显式要求的功能。**

## 执行原则

1. 优先改影响范围小的纯内部代码。
2. 每个任务改完后运行静态检查：
   - 前端：`npx vue-tsc --noEmit`
   - 后端：`cd src-tauri && cargo check`
3. 不要引入新依赖，优先用标准库 / 已安装依赖 / 平台原生能力。
4. 改完后删除对应 TODO 项前的 `[ ]` 改为 `[x]`。

---

## Rust 后端

### persistent_state.rs
- [ ] **删除无用抽象**：`tool_state_mut` / `tool_state_ref` 接收 `_key` 但永远返回 `state.claude`，直接在内联调用处使用 `state.claude`。
  - 位置：`src-tauri/src/persistent_state.rs:239-245`
  - 预期收益：约 10 行

### settings_manager.rs
- [ ] **替换自定义 trait**：删除 `IntoObject` trait，用 `match v { Value::Object(m) => m, _ => Map::new() }` 或 `as_object_mut()` 在唯一调用处处理。
  - 位置：`src-tauri/src/settings_manager.rs:160-171`
  - 预期收益：约 15 行

### registry.rs
- [ ] **使用 winreg 自带编码**：删除手写的 `encode_reg_expand_sz`，改用 `key.set_value(var_name, &value)` 配合 `REG_EXPAND_SZ`。
  - 位置：`src-tauri/src/registry.rs:146-151`
  - 预期收益：约 8 行

### tab_cli.rs
- [ ] **合并重复 flag 解析函数**：将 `extract_flag_u32` / `extract_flag_u64` / `extract_flag_usize` 合并为 `extract_flag<T: FromStr>`。
  - 位置：`src-tauri/src/tab_cli.rs:159-172`
  - 预期收益：约 10 行
- [ ] **合并权限检查逻辑**：`check_permission` 与 `check_permission_inline` 逻辑重复，保留内联版本，测试中直接调用它。
  - 位置：`src-tauri/src/tab_cli.rs:177-207`
  - 预期收益：约 25 行

### pty/mod.rs
- [ ] **简化 pty_write**：当前 140+ 行的 Phase 1/2/Step A/B/C 可以压成一次锁内操作：追加缓冲区 → 扫描换行 → 执行 `tab-*` 命令 → 透传非命令字节。
  - 位置：`src-tauri/src/pty/mod.rs:230-372`
  - 预期收益：约 60 行

### model_fetcher.rs
- [ ] **删除死代码**：移除 `is_likely_ollama_url` 函数。
  - 位置：`src-tauri/src/model_fetcher.rs:8-14`
  - 预期收益：约 8 行

### claude_launcher.rs
- [ ] **简化环境变量展开**：`expand_env` 遍历全部 env vars 效率低，改为针对 `%LOCALAPPDATA%` / `%ProgramFiles%` 等已知变量直接用 `std::env::var`。
  - 位置：`src-tauri/src/claude_launcher.rs:30-41`
  - 预期收益：约 12 行

### session_manager.rs
- [ ] **抽离公共 JSONL 读取器**：`load_claude_sessions` 与 `load_claude_recent_projects` 的逐行解析逻辑几乎一样，提取 `read_history_lines()` 复用。
  - 位置：`src-tauri/src/session_manager.rs:105-208`
  - 预期收益：约 40 行

### config_store.rs
- [ ] **用 serde 反序列化代替手动构建**：删除嵌套循环构造 `HashMap<String, HashMap<String, String>>`，直接用 `serde_json::from_value`。
  - 位置：`src-tauri/src/config_store.rs:50-67`
  - 预期收益：约 15 行

---

## 前端

### App.vue
- [x] **移除刷新调度器**：`scheduleClaudeHistoryRefresh` 带有队列、timer、运行锁，其实直接调用 `refreshClaudeHistoryNow` 即可。
  - 位置：`src/App.vue:282-316`
  - 预期收益：约 30 行
- [x] **使用 Tauri 原生拖拽区域**：删除自定义的 `onTitleBarMouseDown` 拖拽阈值逻辑，改用 `data-tauri-drag-region`。（已验证通过）
  - 位置：`src/App.vue:196-234`
  - 预期收益：约 35 行

### stores/terminal.ts
- [ ] **~~简化监听器初始化~~ 不适用**：`listenerReadyPromise` 并非过度并发保护。改为纯布尔标志后，并发创建终端时会重复注册 `pty_output` / `pty_status` / `pty_title` 监听，导致 PTY 输出被 xterm.js 处理多次、光标跳到右下角。该 Promise 守卫是必要的，保留原实现。
  - 位置：`src/stores/terminal.ts:50-117`
  - 结论：不修改

### stores/claude.ts
- [ ] **压平设置加载的嵌套 try/catch**：三个设置项加载各自包一层 try/catch，改为 `await invoke(...).catch(() => default)`。
  - 位置：`src/stores/claude.ts:57-123`
  - 预期收益：约 30 行

### components/claude/ConfigEditor.vue
- [ ] **删除 vars 计算属性代理**：模板直接绑定 `store.editingConfig.vars.*`，删除 `vars` computed。
  - 位置：`src/components/claude/ConfigEditor.vue:145-164`
  - 预期收益：约 20 行

### components/claude/LaunchOptions.vue
- [ ] **用原生下拉替代自定义历史面板**：当前手写 `history-panel` + `focusout` 定时关闭，改为 `<select>` 或 `<datalist>`。
  - 位置：`src/components/claude/LaunchOptions.vue:72-90`
  - 预期收益：约 25 行

### components/project/RightSidebar.vue
- [ ] **拆分内联组件**：把 `ToolsPanel` / `FilePanel` / `TerminalPanel` / `BrowserPanel` 从 `defineComponent` + `h()` 内联写法拆成独立 `.vue` 文件。
  - 位置：`src/components/project/RightSidebar.vue:75-239`
  - 预期收益：约 150 行（减少内联组件样板代码）

### components/terminal/TerminalPane.vue
- [ ] **使用原生剪贴板 API**：删除 textarea + `document.execCommand('copy')` 的复制实现，改用 `navigator.clipboard.writeText(sel)`。
  - 位置：`src/components/terminal/TerminalPane.vue:119-137`
  - 预期收益：约 15 行

### components/claude/SessionList.vue
- [ ] **用 toLocaleString 替换手写日期格式化**：删除 `formatTs` 中的 `pad` 和手动拼接。
  - 位置：`src/components/claude/SessionList.vue:35-48`
  - 预期收益：约 10 行

### components/project/ProjectPanel.vue
- [ ] **统一分栏 composable**：`useResizablePanes` 和 `useResizableDivider` 功能重叠，保留一个，删除另一个。
  - 位置：`src/components/project/ProjectPanel.vue:48-130`
  - 预期收益：约 40 行

### 删除未使用文件
- [ ] **删除 SectionCard.vue**：没有任何地方导入或使用。
  - 路径：`src/components/common/SectionCard.vue`
  - 预期收益：约 25 行
- [ ] **删除 usePtyBridge.ts**：`usePtyBridge` 没有任何地方使用，`TerminalPane` 直接监听事件。
  - 路径：`src/composables/usePtyBridge.ts`
  - 预期收益：约 65 行

---

## 验证步骤（每个任务完成后）

```bash
# 前端类型检查
npx vue-tsc --noEmit

# 后端编译检查
cd src-tauri && cargo check
```

## 预计总收益

约 **600 行**代码可被删除或简化。
