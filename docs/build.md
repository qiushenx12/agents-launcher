# Agents Launcher 构建与发布指南

本文档说明 Windows、macOS 双平台打包、版本状态记录，以及通过 Git 和 GitHub CLI 发布安装包的流程。

## 环境要求

共同环境：

- Node.js 18 或更高版本
- npm
- Python 3.10 或更高版本
- Rust stable
- Git
- GitHub CLI（使用命令行发布 GitHub Release 时需要）

Windows 额外需要：

- Rust MSVC 工具链
- 包含“使用 C++ 的桌面开发”工作负载的 Visual Studio Build Tools
- WebView2

macOS 额外需要：

- Xcode Command Line Tools
- 对应架构的 Rust 工具链
- 正式对外分发时需要准备 Apple 代码签名和公证配置

macOS 安装包必须在 Mac 上完成实际构建和测试。本项目的自动化检查不能在 Windows 上验证 macOS 原生产物。

## 安装依赖

```powershell
npm install
```

Rust 依赖会在首次编译时自动下载。

## 开发模式

```powershell
npm run tauri dev
```

## 构建检查

发布前建议执行：

```powershell
npm run build
python -m unittest discover -s tests -p "test_build_version.py" -v
cargo test --manifest-path src-tauri/Cargo.toml
```

## 平台级版本状态

根目录的 `version.json` 是版本状态的唯一来源，默认要求 Windows 和 macOS 均通过测试：

```json
{
  "schemaVersion": 3,
  "currentVersion": "1.0.0",
  "platforms": {
    "windows": {
      "status": "pending",
      "architecture": null,
      "testedAt": null,
      "artifacts": []
    },
    "macos": {
      "status": "pending",
      "architecture": null,
      "testedAt": null,
      "artifacts": []
    }
  },
  "releases": []
}
```

状态规则：

- `pending`：该平台尚未完成一次打包。
- `passed`：该平台安装包已成功构建并归档。
- 平台各记各的，**互不为前提**：任一台打包成功即记录，不需要另一个平台先通过。
- 任一平台重新运行打包时，只会把该平台重置为 `pending`，其它平台状态保持不变。
- **同一版本号可以反复出包**：打包成功后刷新 `releases[]` 里那条记录的平台快照（产物换成新的），`recordedAt` 仍是首次记录的时间，也不会追加第二条。
- **版本号只有一个控制点：打包开始时的输入。** 直接回车沿用 `currentVersion`；输入更高的版本号则改用新版本，并把两个平台记录重置为 `pending`（开新版本）。没有基于隐藏状态的自动递增。
- 旧版 `schemaVersion: 1` / `2` 会由 `build.py` 自动迁移；迁移会丢弃已废弃的 `published` 与 `requiredPlatforms`。

## 使用 build.py 打包

Windows 和 macOS 均使用同一个入口：

```powershell
python build.py
```

Mac 用户也可以在 Finder 中双击根目录的 `build-macos.command`。该文件负责检查 Python、Node.js、Rust 和 Xcode Command Line Tools，随后调用同一个 `build.py`，因此版本号、平台状态、测试确认和产物归档规则完全一致：

```bash
./build-macos.command
```

脚本会自动识别当前系统：

- Windows：生成 NSIS `.exe` 安装包。**打包开始时会询问版本号**，两种输入：
  - 输入**更高**的版本号 → 本次使用它，两个平台记录都重置为 `pending`（开新版本）；
  - 输入**与当前相同**的版本号、或**直接回车** → 沿用该版本，另一个平台的记录保持不变（重出）。
- macOS：生成 `.app` 和 `.dmg`，版本记录使用 `.dmg`。**不询问版本号**，始终跟随仓库里 Windows 已定好的 `currentVersion`——因此 Mac 打包前必须先 `git pull` 到 Windows 打包并提交后的提交。
- 其它系统：拒绝正式打包。

脚本还会把当前版本号同步到：

- `package.json`
- `package-lock.json`
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock`

打包成功且找到安装包后，脚本会直接把当前平台记为 `passed` 并归档产物，**不再需要手动输入确认**。

例如 Windows 首先打包完成时：

```text
currentVersion: 1.0.0
windows: passed
macos: pending
```

之后 Mac 使用相同版本完成打包：

```text
currentVersion: 1.0.0
windows: passed
macos: passed
```

版本号由打包开始时的输入决定，不会自行递增。输入更高版本号时，上一档的进位规则为：

```text
1.0.0 -> 1.0.1
1.0.8 -> 1.0.9
1.0.9 -> 1.1.0
```

注意：`build.py` 与 `build-macos.command` 不会自动创建 Git Tag，也不会自动上传 GitHub Release；远程发布仍需执行本文后面的 Git 和 `gh` 命令。

## 平台操作速查

| 操作项 | Windows | macOS |
| --- | --- | --- |
| 推荐入口 | `python build.py` | Finder 双击 `build-macos.command`，或执行 `./build-macos.command` |
| 构建类型 | NSIS `.exe` | `.app` 和 `.dmg` |
| 通过判定 | 打包成功即通过 | 打包成功即通过 |
| 状态字段 | `platforms.windows` | `platforms.macos` |
| 测试归档 | `src-tauri/release-bundle/nsis/` | `src-tauri/release-bundle/dmg/` |
| GitHub 附件 | `Agents Launcher_<版本>_x64-setup.exe` | `Agents Launcher_<版本>_<架构>.dmg` |

打包通过是平台各自的判定，互不影响：

- Windows 通过不会代替 Mac 打包。
- Mac 通过不会重新确认 Windows 产物。
- 打包失败的平台保持 `pending`。
- 已通过的平台重新打包时会先回到 `pending`，直到本次打包成功才恢复为 `passed`。
- 最后一个必需平台通过时，脚本才写入版本级发布记录。

## Mac 端完整打包流程

Mac 操作者可以直接按以下步骤执行。

### 1. 拉取 Windows 平台状态

```bash
git pull --ff-only
```

检查当前版本和两个平台的状态：

```bash
python3 -c 'import json; s=json.load(open("version.json")); print("version:", s["currentVersion"]); print("windows:", s["platforms"]["windows"]["status"]); print("macos:", s["platforms"]["macos"]["status"])'
```

正常的接力状态通常应为：

```text
version: 1.0.0
windows: passed
macos: pending
```

Mac 可以独立构建并记录自己的结果，不需要 Windows 先通过。

### 2. 运行 Mac 打包入口

在 Finder 中双击 `build-macos.command`，或在终端执行：

```bash
./build-macos.command
```

该入口会检查 Python、Node.js、Rust 和 Xcode Command Line Tools，然后调用 `build.py`。不要绕过脚本直接修改 `version.json` 中的 macOS 状态。

### 3. 测试 DMG

构建完成后脚本会自动把 macOS 记为 `passed` 并归档 DMG。建议抽查：

- DMG 可以正常挂载。
- 应用可以复制到 Applications 并启动。
- 应用显示的版本号与 `currentVersion` 一致。
- 主要功能和终端功能在当前 Mac 架构上可用。
- 如用于外部分发，签名和公证结果符合要求。

如果发现问题，修复后重新运行 `build-macos.command` 即可（本次打包已记为 `passed`，但下一次成功的打包会覆盖产物路径与 `testedAt`，无需手动改 `version.json`）。

### 4. 核对最终状态

```bash
python3 -c 'import json; s=json.load(open("version.json")); print(json.dumps({"version": s["currentVersion"], "platforms": s["platforms"]}, ensure_ascii=False, indent=2))'
```

两个平台都打过包后，应该看到：

```text
windows.status = passed
macos.status = passed
```

同时应存在归档后的 DMG：

```bash
ls -lh src-tauri/release-bundle/dmg/
```

### 5. 提交 Mac 测试及最终发布记录

先检查修改，不要提交 `.app`、`.dmg`、`target` 或其它本地文件：

```bash
git status --short
git diff -- version.json
```

提交版本状态及打包过程中同步的版本配置：

```bash
git add version.json package.json package-lock.json \
  src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "release: record macOS v1.0.0"
git push origin main
```

提交前应将示例中的 `1.0.0` 替换为 `currentVersion`。如果还有其它待发布源码修改，应先检查后明确加入，不要直接使用 `git add -A`。

### 6. 创建 Tag 或上传 Mac 安装包

如果 Mac 是最后完成的平台，可以在最终状态提交后创建唯一 Tag：

```bash
git tag -a v1.0.0 -m "Agents Launcher v1.0.0"
git push origin v1.0.0
```

若 Windows 已创建同一 Tag 的草稿 Release，Mac 只需要上传 DMG：

```bash
gh release upload v1.0.0 \
  "src-tauri/release-bundle/dmg/Agents Launcher_1.0.0_aarch64.dmg"
```

不要在 Mac 上创建第二个 `v1.0.0-macos` Tag，也不要创建独立的 macOS Release。

## 构建产物

| 平台 | 临时构建产物 | 已测试归档 |
| --- | --- | --- |
| Windows | `src-tauri/target/release/bundle/nsis/` | `src-tauri/release-bundle/nsis/` |
| macOS | `src-tauri/target/release/bundle/dmg/` | `src-tauri/release-bundle/dmg/` |

安装包名称示例：

```text
Agents Launcher_1.0.0_x64-setup.exe
Agents Launcher_1.0.0_aarch64.dmg
```

`target` 和 `release-bundle` 均不会提交到 Git。`build.py` 会保护和恢复历史安装包，但正式发布文件仍应上传至 GitHub Release 或其它长期存储位置。

## 两台电脑协作打包

两个平台必须使用相同版本号和同一份业务源码。**版本号以 Windows 为准**，推荐顺序如下：

1. 提交并推送待发布源码，确认 `currentVersion` 正确。
2. Windows 拉取该提交，运行 `python build.py`——开始时确认或输入新版本号，打包成功即记录通过。
3. 提交并推送 Windows 平台在 `version.json` 中的记录。
4. Mac 拉取最新提交，确认仍是同一版本号（Mac 端不会询问也不会修改版本号）。
5. Mac 运行 `python build.py` 或 `build-macos.command`，打包成功即记录通过。
6. 提交版本记录。
7. 创建唯一的版本 Tag，并将 Windows、macOS 安装包上传到同一个 GitHub Release。

平台状态提交只改变版本记录，不应夹带业务源码修改。如果平台测试期间修改了业务源码，两个平台都应重新打包和测试。

## 使用 Git 标记版本

确认 `version.json` 中两个平台均为 `passed`：

```powershell
$versionState = Get-Content version.json -Raw | ConvertFrom-Json
$versionState.currentVersion
$versionState.platforms
```

检查并提交最终发布记录：

```powershell
git status --short
git diff
git add <本次需要发布的源码和版本记录>
git commit -m "release: v1.0.0"
```

创建并推送唯一 Tag：

```powershell
git tag -a v1.0.0 -m "Agents Launcher v1.0.0"
git push origin main
git push origin v1.0.0
```

不要分别创建 `v1.0.0-windows` 和 `v1.0.0-macos`。同一个应用版本只使用一个 Tag。

## 使用 GitHub CLI 发布双平台安装包

### 检查登录和仓库

```powershell
git remote get-url origin
gh auth status
gh repo view
```

未登录时执行：

```powershell
gh auth login
```

### 两个安装包位于同一台电脑

```powershell
gh release create v1.0.0 `
  "路径/Agents Launcher_1.0.0_x64-setup.exe" `
  "路径/Agents Launcher_1.0.0_aarch64.dmg" `
  --title "Agents Launcher v1.0.0" `
  --generate-notes `
  --fail-on-no-commits `
  --verify-tag
```

### 安装包分别位于 Windows 和 Mac

先在其中一台电脑创建草稿 Release 并上传本机产物：

```powershell
gh release create v1.0.0 `
  "src-tauri/release-bundle/nsis/Agents Launcher_1.0.0_x64-setup.exe" `
  --title "Agents Launcher v1.0.0" `
  --generate-notes `
  --verify-tag `
  --draft
```

在 Mac 上向同一个草稿上传 DMG：

```bash
gh release upload v1.0.0 \
  "src-tauri/release-bundle/dmg/Agents Launcher_1.0.0_aarch64.dmg"
```

确认两个安装包都存在：

```powershell
gh release view v1.0.0
```

最后发布草稿：

```powershell
gh release edit v1.0.0 --draft=false
```

不要使用 `--clobber` 覆盖已经发布的同名安装包。发现安装包错误时，应停止发布并重新完成对应平台测试。

## 常见问题

### Windows 已通过，Mac 打包失败会递增版本吗？

不会。版本保持不变，Windows 继续为 `passed`，macOS 保持 `pending`。修复后在 Mac 上重新打包即可。

### 为什么 Mac 打包时没有版本号提问？

版本号以 Windows 为准。Mac 端只做兼容性验证与补打包，始终使用仓库里 Windows 已定的 `currentVersion`。如果 Mac 上看到的版本号不对，先 `git pull` 到 Windows 打包并提交后的提交再打包。

### Windows 已通过后又重新打包会怎样？

Windows 会先重置为 `pending`，本次打包成功后再次记为 `passed`，产物路径与 `passedAt` 会被覆盖。

### 能否先发布 Windows，之后再向正式 Release 增加 Mac？

技术上可以追加附件，但不符合当前版本状态规则。推荐等两个平台均通过后一次发布，或先创建草稿 Release，上传齐全后再正式发布。

### 同一个版本能否包含 Intel 和 Apple Silicon 两个 Mac 包？

GitHub Release 可以包含多个不同文件名的 DMG。当前状态按 `macos` 平台整体记录；如果以后要求 Intel 和 Apple Silicon 必须分别通过，需要再将平台键细分为 `macos-x64` 和 `macos-arm64`。
