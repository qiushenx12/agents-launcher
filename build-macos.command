#!/bin/zsh

# Finder launches .command files with an unpredictable working directory.
# Resolve all project paths from this script's own location.
PROJECT_DIR="${0:A:h}"

pause_before_exit() {
  echo
  read -r "?按回车键关闭窗口……"
}

fail() {
  echo
  echo "macOS 打包未开始：$1"
  pause_before_exit
  exit 1
}

require_command() {
  local command_name="$1"
  local install_hint="$2"
  command -v "$command_name" >/dev/null 2>&1 || fail "未找到 $command_name。$install_hint"
}

cd "$PROJECT_DIR" || fail "无法进入项目目录：$PROJECT_DIR"

echo "========================================"
echo " Agents Launcher macOS 构建与版本记录"
echo "========================================"
echo "项目目录：$PROJECT_DIR"
echo

[[ -f "$PROJECT_DIR/build.py" ]] || fail "未找到 build.py。"

require_command python3 "请安装 Python 3.10 或更高版本。"
require_command node "请安装 Node.js 18 或更高版本。"
require_command npm "请确认 Node.js 和 npm 已正确安装。"
require_command rustc "请从 https://rustup.rs 安装 Rust。"
require_command cargo "请从 https://rustup.rs 安装 Rust。"
require_command xcode-select "请先执行 xcode-select --install。"

if ! xcode-select -p >/dev/null 2>&1; then
  fail "未检测到 Xcode Command Line Tools，请先执行 xcode-select --install。"
fi

if ! python3 -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 9) else 1)'; then
  fail "当前 Python 版本为 $(python3 --version 2>/dev/null)，需要 Python 3.9 或更高版本。"
fi

NODE_MAJOR="$(node -p 'Number(process.versions.node.split(".")[0])' 2>/dev/null)"
case "$NODE_MAJOR" in
  ''|*[!0-9]*)
    fail "无法识别当前 Node.js 版本：$(node --version 2>/dev/null)"
    ;;
esac
if (( NODE_MAJOR < 18 )); then
  fail "当前 Node.js 版本为 $(node --version 2>/dev/null)，需要 Node.js 18 或更高版本。"
fi

case "$(uname -m)" in
  arm64)
    MAC_ARCHITECTURE="Apple Silicon (arm64)"
    ;;
  x86_64)
    MAC_ARCHITECTURE="Intel (x86_64)"
    ;;
  *)
    fail "暂不支持当前处理器架构：$(uname -m)"
    ;;
esac

echo "Python：$(python3 --version)"
echo "Node.js：$(node --version)"
echo "Rust：$(rustc --version)"
echo "架构：$MAC_ARCHITECTURE"
echo
echo "接下来由 build.py 统一处理："
echo "  - 读取并同步当前版本号"
echo "  - 生成 macOS .app 和 .dmg"
echo "  - 记录 macOS 测试状态和产物"
echo "  - Windows、macOS 均通过后发布当前版本"
echo

# build.py normally pauses on errors. The wrapper owns the final pause so a
# Finder-launched Terminal window only asks once before closing.
AGENTS_LAUNCHER_COMMAND_WRAPPER=1 python3 "$PROJECT_DIR/build.py"
BUILD_EXIT_CODE=$?

echo
if (( BUILD_EXIT_CODE == 0 )); then
  echo "macOS 打包流程已结束。请核对上方版本状态和产物路径。"
else
  echo "macOS 打包脚本异常退出，退出码：$BUILD_EXIT_CODE"
fi

pause_before_exit
exit "$BUILD_EXIT_CODE"
