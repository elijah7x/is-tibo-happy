#!/bin/bash
# is-tibo-happy 卸载 —— curl -fsSL <url>/uninstall.sh | bash
set -euo pipefail

NAME=is-tibo-happy
DEST="$HOME/Library/Application Support/$NAME"
PLIST="$HOME/Library/LaunchAgents/com.istibohappy.daemon.plist"

launchctl bootout "gui/$(id -u)" "$PLIST" 2>/dev/null || true
rm -f "$PLIST"
rm -rf "$DEST"

# 端口 9333 随 App 进程存活——卸载即退出并以正常模式重启，立即关闭暴露面
APP=""
for p in "/Applications/ChatGPT.app" "$HOME/Applications/ChatGPT.app"; do
  [ -d "$p" ] && APP="$p" && break
done
PORT_CLOSED=1
if [ -n "$APP" ] && pgrep -f "$APP/Contents/MacOS/ChatGPT" >/dev/null; then
  echo "▸ 重启 Codex 以关闭调试端口（对话记录不会丢失）"
  osascript -e 'quit app "ChatGPT"' 2>/dev/null || true
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    pgrep -f "$APP/Contents/MacOS/ChatGPT" >/dev/null || break
    sleep 1
  done
  if pgrep -f "$APP/Contents/MacOS/ChatGPT" >/dev/null; then
    # osascript 被拒/挂起等：别谎报端口已关
    PORT_CLOSED=0
    echo "⚠ Codex 未能自动退出——请手动退出一次再打开，调试端口才会关闭"
  else
    open -a "ChatGPT" 2>/dev/null || true
  fi
fi

if [ "$PORT_CLOSED" = 1 ]; then
  echo "✓ 已卸载（后台服务停止 + 文件移除 + 调试端口已关闭）。"
else
  echo "✓ 已卸载（后台服务停止 + 文件移除；调试端口待 Codex 手动重启后关闭）。"
fi
