#!/bin/zsh

# 双击启动 Agents Launcher 开发模式（npm run tauri dev）。
# 本窗口的终端日志用于排查 macOS 托盘 / Dock 恢复窗口问题：
# 重现问题后，请复制所有 [window-restore] 开头的日志行。

PROJECT_DIR="${0:A:h}"

pause_before_exit() {
  echo
  read -r "?按回车键关闭窗口……"
}

cd "$PROJECT_DIR" || {
  echo "无法进入项目目录：$PROJECT_DIR"
  pause_before_exit
  exit 1
}

# Finder 双击启动时 PATH 极简，补齐 node / npm / cargo 的常见安装位置。
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
# 本机 cargo 可能只在 rustup toolchain 目录里（没有 ~/.cargo/bin shim）。
for toolchain_bin in "$HOME"/.rustup/toolchains/*/bin(N); do
  export PATH="$toolchain_bin:$PATH"
done
# nvm 安装的 node。
for nvm_bin in "$HOME"/.nvm/versions/node/*/bin(N); do
  export PATH="$nvm_bin:$PATH"
done

missing=0
for cmd in node npm cargo; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "未找到 $cmd。请先在终端里确认它能正常运行。"
    missing=1
  fi
done
if (( missing )); then
  pause_before_exit
  exit 1
fi

echo "========================================"
echo " Agents Launcher 开发模式（诊断版）"
echo "========================================"
echo "项目目录：$PROJECT_DIR"
echo
echo "操作步骤："
echo "  1. 开启「最小化到托盘」"
echo "  2. 点窗口红色关闭按钮"
echo "  3. 依次点击 Dock 图标、托盘图标、托盘菜单「打开 Agents Launcher」"
echo "  4. 回到本窗口，复制所有 [window-restore] 开头的日志"
echo "========================================"
echo

npm run tauri dev

echo
echo "========================================"
echo "开发模式已退出。"
pause_before_exit
