# macOS 本地版本与远程主干差异对比

> 生成时间：2026-07-17 13:09
> 对比基准：本地工作目录 vs 远程 `origin/main`（HEAD `b00b8e7`）
> 说明：当前 `.git` 已初始化，远程元数据已拉取，但尚未执行 `git pull` 或 `git commit`。本报告仅用于评估差异，不构成版本操作。

## 总体概况

当前 macOS 本地版本是在一个较旧的代码基线上进行了 macOS 特化改造，而远程 `main` 已经推进了多 CLI 支持、Codex/OpenCode 配置、迁移审计、测试体系等大量功能。两者差异极大，直接合并风险很高，建议按功能模块逐步回迁。

| 类别 | 数量 | 说明 |
|---|---|---|
| 本地删除的文件 | 65 | 远程存在、本地已删除 |
| 本地修改的文件 | 40 | 远程存在、本地内容已变更 |
| 本地新增的文件 | 29 | 本地新增、远程不存在 |
| 与远程一致的文件 | 31 | 内容未改动 |
| 总计涉及条目 | 165 | 含目录与文件 |

代码行数层面（排除构建产物目录）：

```
 105 files changed, 1206 insertions(+), 14402 deletions(-)
```

## 远程 main 最近提交

```
b00b8e7 (origin/main, origin/HEAD) feat: add model limits and terminal input handling
a436051 fix: update transparent app icon assets
1567437 chore: rename app and refresh packaging assets
f98c659 docs: refresh repository documentation
21162d4 feat: 支持多 CLI 配置与运行时
d749dac feat: add environment and Claude Code setup gates
3225311 Remove .svnignore — repo is Git, not SVN
5ee1fb5 Add Tauri backend (Rust)
b452189 Initial commit: claude-code-启动器 (Tauri + Vue 3 + Rust)
```

## 核心功能差距

### 1. 多 CLI 支持（Codex / OpenCode）

远程主干已支持 Claude、Codex、OpenCode 三种 CLI，但本地 macOS 版本删除了大量相关代码：

- `docs/阶段D-CodeX专属配置与运行时验证说明.md`
- `docs/阶段E-OpenCode配置调研与实现约定.md`
- `src-tauri/src/cli_capabilities.rs`
- `src-tauri/src/cli_contract.rs`
- `src-tauri/src/cli_migration.rs`
- `src-tauri/src/cli_runtime.rs`
- `src-tauri/src/codex_config.rs`
- `src-tauri/src/opencode_config.rs`
- `src-tauri/tests/fixtures/cli/codex-app-server-thread-list.sample.json`
- `src-tauri/tests/fixtures/cli/codex-capabilities-2026-07-14.json`
- `src-tauri/tests/fixtures/cli/codex-session-meta.sample.jsonl`
- `src-tauri/tests/fixtures/cli/opencode-capabilities-1.17.20.json`
- `src-tauri/tests/fixtures/cli/opencode-debug-scrap-global.sample.json`
- `src-tauri/tests/fixtures/cli/opencode-debug-scrap.sample.json`
- `src-tauri/tests/fixtures/cli/opencode-debug-scrap.schema.json`
- `src-tauri/tests/fixtures/cli/opencode-global-session-list.sample.json`
- `src-tauri/tests/fixtures/cli/opencode-session-list.sample.json`
- `src-tauri/tests/fixtures/cli/opencode-session-list.schema.json`
- `src/components/cli/CliCodexPanel.vue`
- `src/components/cli/CliOpencodePanel.vue`
- `src/components/codex/CodexConfigPanel.vue`
- `src/components/opencode/OpenCodeConfigPanel.vue`
- `src/stores/codexConfig.ts`
- `src/stores/opencodeConfig.ts`
- `src/utils/codexTerminalInput.ts`
- `src/utils/codexTerminalOutput.ts`
- `tests/codexTerminalInput.test.ts`
- `tests/codexTerminalOutput.test.ts`



### 2. 测试与契约体系

本地删除了测试文件、CLI 契约、迁移审计文档和测试夹具：

- `tests/` 目录被整体删除
- `contracts/cli-contract.json` 被删除
- `src-tauri/fixtures/` 被整体删除
- 多份 `docs/阶段*-*.md` 迁移审计文档被删除

### 3. UI 与配置框架

远程增加了通用配置工作区（`ConfigWorkspace`、`ConfigStatusBanner`、`CliConfigPlaceholder` 等），本地仍使用早期 Claude 专用配置面板。

## 本地 macOS 特化改动

### 新增文件

本地为 macOS 增加的关键文件：

- `docs/macOS打包方案.md`
- `docs/macOS改写方案.md`
- `docs/macOS标题栏方案失败记录.md`
- `src-tauri/gen/schemas/macOS-schema.json`
- `src-tauri/icons/app.icns`
- `src-tauri/tauri.macos.conf.json`
- `src/composables/useDefaultShell.ts`
- `src/composables/usePlatform.ts`



### 修改的核心文件

以下文件在本地被修改，主要涉及 macOS 路径处理、PTY、窗口状态、启动器、项目管理等：

- `src-tauri/src/claude_launcher.rs`
- `src-tauri/src/config_store.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/src/main.rs`
- `src-tauri/src/model_fetcher.rs`
- `src-tauri/src/persistent_state.rs`
- `src-tauri/src/project_manager.rs`
- `src-tauri/src/pty/mod.rs`
- `src-tauri/src/pty/session.rs`
- `src-tauri/src/registry.rs`
- `src-tauri/src/session_manager.rs`
- `src-tauri/src/settings_manager.rs`
- `src-tauri/src/tab_cli.rs`
- `src-tauri/src/utils.rs`

### 删除的 Windows/macOS 通用文件

以下文件在本地被删除，但可能是通用能力，需要评估是否应恢复：

- `src-tauri/src/cli_capabilities.rs`
- `src-tauri/src/cli_contract.rs`
- `src-tauri/src/cli_migration.rs`
- `src-tauri/src/cli_runtime.rs`
- `src-tauri/src/codex_config.rs`
- `src-tauri/src/dependency_manager.rs`
- `src-tauri/src/file_transaction.rs`
- `src-tauri/src/opencode_config.rs`

## 详细清单

### 本地删除的文件（65 个）


#### Pinia 状态 (src/stores/)

- `src/stores/cliRuntime.ts`
- `src/stores/codexConfig.ts`
- `src/stores/configWorkspace.ts`
- `src/stores/opencodeConfig.ts`

#### Rust 后端 (src-tauri/src/)

- `src-tauri/src/cli_capabilities.rs`
- `src-tauri/src/cli_contract.rs`
- `src-tauri/src/cli_migration.rs`
- `src-tauri/src/cli_runtime.rs`
- `src-tauri/src/codex_config.rs`
- `src-tauri/src/dependency_manager.rs`
- `src-tauri/src/file_transaction.rs`
- `src-tauri/src/opencode_config.rs`

#### Tauri 配置与资源 (src-tauri/)

- `src-tauri/icons/16x16.png`
- `src-tauri/tests/fixtures/cli/codex-app-server-thread-list.sample.json`
- `src-tauri/tests/fixtures/cli/codex-capabilities-2026-07-14.json`
- `src-tauri/tests/fixtures/cli/codex-session-meta.sample.jsonl`
- `src-tauri/tests/fixtures/cli/opencode-capabilities-1.17.20.json`
- `src-tauri/tests/fixtures/cli/opencode-debug-scrap-global.sample.json`
- `src-tauri/tests/fixtures/cli/opencode-debug-scrap.sample.json`
- `src-tauri/tests/fixtures/cli/opencode-debug-scrap.schema.json`
- `src-tauri/tests/fixtures/cli/opencode-global-session-list.sample.json`
- `src-tauri/tests/fixtures/cli/opencode-session-list.sample.json`
- `src-tauri/tests/fixtures/cli/opencode-session-list.schema.json`
- `src-tauri/tests/fixtures/migration/interrupted-write.json`
- `src-tauri/tests/fixtures/migration/legacy-app-state-project-tab.json`
- `src-tauri/tests/fixtures/migration/legacy-env-configs-unknown-fields.json`
- `src-tauri/tests/fixtures/migration/legacy-projects-no-cli-kind.json`
- `src-tauri/tests/fixtures/migration/migrated-projects-default-claude.json`
- `src-tauri/tests/fixtures/migration/misclassified-cli-projects.json`

#### Vue 组件 (src/components/)

- `src/components/cli/CliClaudePanel.vue`
- `src/components/cli/CliCodexPanel.vue`
- `src/components/cli/CliOpencodePanel.vue`
- `src/components/codex/CodexConfigPanel.vue`
- `src/components/config/CliConfigPlaceholder.vue`
- `src/components/config/ConfigStatusBanner.vue`
- `src/components/config/ConfigWorkspace.vue`
- `src/components/config/ModelField.vue`
- `src/components/config/SecretField.vue`
- `src/components/opencode/OpenCodeConfigPanel.vue`

#### 前端其他 (src/)

- `src/types/cli.ts`

#### 前端工具 (src/utils/)

- `src/utils/codexTerminalInput.ts`
- `src/utils/codexTerminalOutput.ts`
- `src/utils/configSecurity.ts`

#### 契约文件 (contracts/)

- `contracts/cli-contract.json`

#### 文档 (docs/)

- `docs/三CLI入口与配置功能开发规划.md`
- `docs/依赖检测与安装开发文档.md`
- `docs/阶段A-基线契约与迁移审计.md`
- `docs/阶段B-工作区隔离验证说明.md`
- `docs/阶段C-配置框架与Claude兼容层验证说明.md`
- `docs/阶段D-CodeX专属配置与运行时验证说明.md`
- `docs/阶段E-OpenCode配置调研与实现约定.md`
- `docs/阶段F-最终回归与交付记录.md`

#### 根目录/其他

- `.gitignore`
- `AGENTS.md`
- `build.py`
- `dev.py`

#### 测试 (tests/)

- `tests/codexTerminalInput.test.ts`
- `tests/codexTerminalOutput.test.ts`

#### 美术资源 (art/)

- `art/icon-128x128.png`
- `art/icon-16x16.png`
- `art/icon-256x256.png`
- `art/icon-32x32.png`
- `art/icon-48x48.png`
- `art/icon-64x64.png`
- `art/icon.png`

### 本地修改的文件（40 个）


#### Pinia 状态 (src/stores/)

- `src/stores/claude.ts`
- `src/stores/project.ts`
- `src/stores/tabComm.ts`
- `src/stores/terminal.ts`

#### Rust 后端 (src-tauri/src/)

- `src-tauri/src/claude_launcher.rs`
- `src-tauri/src/config_store.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/src/main.rs`
- `src-tauri/src/model_fetcher.rs`
- `src-tauri/src/persistent_state.rs`
- `src-tauri/src/project_manager.rs`
- `src-tauri/src/pty/mod.rs`
- `src-tauri/src/pty/session.rs`
- `src-tauri/src/registry.rs`
- `src-tauri/src/session_manager.rs`
- `src-tauri/src/settings_manager.rs`
- `src-tauri/src/tab_cli.rs`
- `src-tauri/src/utils.rs`

#### Tauri 配置与资源 (src-tauri/)

- `src-tauri/Cargo.lock`
- `src-tauri/Cargo.toml`
- `src-tauri/build.rs`
- `src-tauri/icons/app.ico`
- `src-tauri/tauri.conf.json`

#### Vue Composables (src/composables/)

- `src/composables/useTauriDrop.ts`

#### Vue 组件 (src/components/)

- `src/components/claude/ConfigEditor.vue`
- `src/components/orchestration/OrchestrationManager.vue`
- `src/components/project/ProjectPanel.vue`
- `src/components/project/ProjectSidebar.vue`
- `src/components/project/ProjectTerminalArea.vue`
- `src/components/project/RightSidebar.vue`
- `src/components/terminal/TerminalManager.vue`
- `src/components/terminal/TerminalPane.vue`

#### 前端其他 (src/)

- `src/App.vue`
- `src/types/config.ts`
- `src/types/terminal.ts`

#### 根目录/其他

- `BUILD.md`
- `README.md`
- `index.html`
- `package-lock.json`
- `package.json`

### 本地新增的文件（29 个）


#### Rust 后端 (src-tauri/src/)

- `src-tauri/src/env_applier.rs`

#### Tauri 配置与资源 (src-tauri/)

- `src-tauri/.DS_Store`
- `src-tauri/gen/schemas/macOS-schema.json`
- `src-tauri/icons/app.icns`
- `src-tauri/icons/icon.png`
- `src-tauri/target/`
- `src-tauri/tauri.macos.conf.json`

#### Vue Composables (src/composables/)

- `src/composables/useDefaultShell.ts`
- `src/composables/usePlatform.ts`

#### Vue 组件 (src/components/)

- `src/components/claude/ModelField.vue`

#### 文档 (docs/)

- `docs/UI线框图制作规范.md`
- `docs/UI线框图_项目功能规划.html`
- `docs/编排功能设计文档.md`
- `docs/跨标签页通信系统.md`
- `docs/项目功能开发文档.md`
- `docs/macOS打包方案.md`
- `docs/macOS改写方案.md`
- `docs/macOS标题栏方案失败记录.md`

#### 根目录/其他

- `截图/`
- `.DS_Store`
- `.claude/`
- `.svn/`
- `.svnignore`
- `CLAUDE.md`
- `build.bat`
- `claude-code-2-1-112/`
- `dev.bat`
- `dist/`
- `node_modules/`

### 与远程一致的文件（31 个）


#### Tauri 配置与资源 (src-tauri/)

- `src-tauri/capabilities/default.json`
- `src-tauri/gen/schemas/acl-manifests.json`
- `src-tauri/gen/schemas/capabilities.json`
- `src-tauri/gen/schemas/desktop-schema.json`
- `src-tauri/gen/schemas/windows-schema.json`

#### Vue Composables (src/composables/)

- `src/composables/useDragReorder.ts`
- `src/composables/usePtyBridge.ts`
- `src/composables/useResizableDivider.ts`
- `src/composables/useResizablePanes.ts`

#### Vue 组件 (src/components/)

- `src/components/claude/ClaudePanel.vue`
- `src/components/claude/ConfigList.vue`
- `src/components/claude/LaunchOptions.vue`
- `src/components/claude/SessionList.vue`
- `src/components/claude/TokenField.vue`
- `src/components/common/SectionCard.vue`
- `src/components/common/StatusBar.vue`
- `src/components/common/ToastNotification.vue`
- `src/components/orchestration/AgentRoleModal.vue`
- `src/components/orchestration/PresetManager.vue`
- `src/components/project/ModuleToolbar.vue`
- `src/components/terminal/SnapshotManager.vue`
- `src/components/terminal/TabPermissionModal.vue`
- `src/components/terminal/TerminalTab.vue`

#### 前端其他 (src/)

- `src/assets/styles/components.css`
- `src/assets/styles/theme.css`
- `src/env.d.ts`
- `src/main.ts`
- `src/types/orchestration.ts`

#### 根目录/其他

- `tsconfig.json`
- `tsconfig.node.json`
- `vite.config.ts`

## 后续合并建议

1. **先补回测试与契约文件**：`tests/`、`contracts/`、`fixtures/` 是主干质量保障，应优先恢复。
2. **按 CLI 类型拆分模块**：将 Codex/OpenCode 支持做成可选模块，macOS 版本初期可只启用 Claude，但保留扩展结构。
3. **用平台条件编译隔离差异**：Rust 中继续使用 `#[cfg(target_os = "macos")]`；前端用运行时平台检测，而不是删除其他平台代码。
4. **补全 `.gitignore`**：当前 `.gitignore` 被删除，导致 `node_modules/`、`dist/`、`src-tauri/target/`、`.DS_Store` 等被跟踪，需恢复并补充 macOS/构建产物规则。
5. **建议基于远程 main 新建 mac 适配分支**：将当前 mac 修改整理成一组 patch，rebase 到最新 `origin/main` 上，而不是在当前旧基线上直接合并。
