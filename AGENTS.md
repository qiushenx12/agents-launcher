# AGENTS.md

This file provides repository-specific guidance for coding agents working on Agents Launcher.

## Project Overview

Agents Launcher is a Windows desktop workspace for **Claude Code**, **Codex**, and **OpenCode**. It combines CLI-specific configuration profiles, project and session discovery, embedded PTY terminals, file tools, inter-tab communication, and local orchestration features.

- **Frontend:** Vue 3 Composition API, Pinia, TypeScript, Vite, xterm.js
- **Backend:** Rust and Tauri 2 commands/events
- **Target platform:** Windows only
- **Package manager:** npm
- **Application data:** `%APPDATA%\ClaudeEnvManager\`

The backend depends on Windows APIs through the `winreg` and `windows` crates. Do not assume that the Rust project can compile or run on another platform.

## Working Commands

Install dependencies after a fresh checkout:

```powershell
npm install
```

Development:

```powershell
# Full Tauri application with Vite and Rust hot reload
npm run tauri dev

# Frontend-only Vite server
npm run dev
```

Build and static checks:

```powershell
# TypeScript check plus frontend production build
npm run build

# TypeScript check only
npx vue-tsc --noEmit

# Rust compile check
cargo check --manifest-path src-tauri/Cargo.toml

# Full Windows production build and NSIS bundle
npm run tauri build
```

Tests:

```powershell
# Frontend terminal-output tests; requires Node.js 22 or newer
node --test tests/codexTerminalOutput.test.ts

# Rust unit tests
cargo test --manifest-path src-tauri/Cargo.toml
```

There is no lint script configured.

## Architecture

### Frontend

`src/main.ts` mounts `src/App.vue`. The root component coordinates these main areas:

- Shared configuration workspace for Claude Code, Codex, and OpenCode
- CLI-specific project workspaces and session terminals
- Standalone multi-tab terminal manager
- Multi-agent orchestration manager
- Global dependency and CLI capability gates

Important frontend directories:

- `src/components/config/` - shared configuration workspace and reusable fields
- `src/components/claude/` - Claude Code configuration and launch controls
- `src/components/codex/` - Codex profile editor
- `src/components/opencode/` - OpenCode provider and profile editor
- `src/components/project/` - project tree, sessions, terminal area, and file tools
- `src/components/terminal/` - xterm.js panes, terminal tabs, snapshots, and permissions
- `src/components/orchestration/` - agent roles and orchestration presets
- `src/stores/` - Pinia state for profiles, CLI runtimes, projects, terminals, and tab communication
- `src/types/cli.ts` - shared CLI kinds, descriptors, and main-tab contracts

Components call Rust with Tauri `invoke()` and receive PTY output through Tauri events. Keep privileged filesystem, process, credential, and registry operations in Rust rather than implementing them in the frontend.

### Backend

Tauri commands are registered in `src-tauri/src/lib.rs`. Major module groups are:

- `cli_contract`, `cli_capabilities`, `cli_runtime` - shared CLI types, detection, capability validation, and native session discovery
- `codex_config`, `opencode_config`, `config_store` - CLI profile and configuration management
- `cli_migration`, `file_transaction` - migrations, atomic writes, verified backups, and recovery
- `project_manager`, `session_manager` - project metadata, sessions, recent items, and text files
- `pty/` - PTY creation, input/output, resize, title parsing, and process lifecycle
- `tab_cli` - inter-tab commands, permissions, snapshots, and orchestration presets
- `persistent_state`, `settings_manager` - window, pane, font, profile, and launch state
- `registry`, `claude_launcher`, `model_fetcher` - Windows integration, Claude launch, environment application, and model discovery

### CLI Isolation

Claude Code, Codex, and OpenCode share UI and project abstractions, but their runtime state and configuration must remain isolated by `CliKind`.

- Never infer a CLI from a display label or project name.
- Persist and query active profiles using both CLI kind and profile ID.
- Preserve native session identifiers without converting them into another CLI's format.
- Run capability checks against only the selected CLI executable.
- Unknown legacy records default to Claude Code only through the explicit migration layer.

### Persistence and Security

- Persist application state through dedicated Tauri commands.
- Preserve unknown fields when rewriting supported external configuration files.
- Use `file_transaction` helpers for atomic writes, verified backups, and rollback.
- Never log, serialize into diagnostics, or expose API keys and tokens to the frontend unnecessarily.
- Codex and OpenCode managed secrets use Windows DPAPI where supported.
- Keep migration logic idempotent and cover legacy or interrupted-write cases with fixtures.

## Keyboard Shortcuts

Shortcuts are defined in `src/App.vue`:

| Context | Shortcut | Action |
| --- | --- | --- |
| Terminal | `Ctrl + W` | Close the active terminal tab |
| Terminal | `Ctrl + Tab` | Switch terminal tabs |
| Project | `Ctrl + T` | Create a project session |
| Project | `Ctrl + W` | Close the active project session terminal |
| Project | `Ctrl + Tab` | Switch project sessions |
| Project | `Ctrl + P` | Open a file in the sidebar |
| Project | `Ctrl + S` | Save the active sidebar file |
| Project | `Ctrl + Shift + B` | Toggle the tool sidebar |

## Build Outputs

- Frontend bundle: `dist/`
- Rust build artifacts: `src-tauri/target/`
- NSIS installer: `src-tauri/target/release/bundle/nsis/`

These paths and `node_modules/` are ignored by Git. Do not add generated build output or local CLI credentials to commits.

## Workflow Guidelines

- Preserve unrelated local changes in a dirty worktree.
- Use `rg` or `rg --files` for repository searches when available.
- Keep Tauri command names and frontend request/response types synchronized.
- Add or update Rust fixtures when changing CLI contracts, migrations, or native output adapters.
- Run checks in proportion to the change. For cross-layer work, run `npm run build`, the Node terminal-output tests when relevant, and `cargo test`.
- Do not launch the Vite server, a browser, or the Tauri application for verification. Use static checks and automated tests, then ask the user to perform interactive validation.
- This repository has no configured formatter or linter scripts; avoid unrelated formatting churn.

## Documentation and Git

- Ask the user for explicit confirmation before staging or committing any change under `docs/`.
- Ask the user for explicit confirmation again before pushing a commit that contains changes under `docs/` to a remote repository.
- Development-stage documents, plans, reviews, drafts, progress notes, and temporary records must remain local and must never be committed.
- Only `docs/build.md` and `docs/依赖检测与安装开发文档.md` may remain tracked in `docs/`; changes to either file still require the confirmations above.
