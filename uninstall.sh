#!/bin/bash
# is-tibo-happy 卸载 —— curl -fsSL <url>/uninstall.sh | bash
set -euo pipefail

NAME=is-tibo-happy
DEST="$HOME/Library/Application Support/$NAME"
PLIST="$HOME/Library/LaunchAgents/com.istibohappy.daemon.plist"

launchctl bootout "gui/$(id -u)" "$PLIST" 2>/dev/null || true
rm -f "$PLIST"
rm -rf "$DEST"

# 老版本可能给在跑的 Codex 留了调试端口：只提示，绝不杀/不重启宿主进程
APP=""
for p in "/Applications/ChatGPT.app" "$HOME/Applications/ChatGPT.app"; do
  [ -d "$p" ] && APP="$p" && break
done
if [ -n "$APP" ] && ps -axo pid=,command= | grep -F "$APP/Contents/MacOS/ChatGPT" | grep -q -- "--remote-debugging-port"; then
  echo "⚠ Codex 仍在运行且带着旧版本留下的调试端口——退出并重新打开 Codex 一次即可关闭。"
fi

echo "✓ 已卸载（后台服务停止 + 文件移除）。"
