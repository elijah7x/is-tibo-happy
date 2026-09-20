#!/bin/bash
# is-tibo-happy 一键安装
#   全球: curl -fsSL https://raw.githubusercontent.com/elijah7x/is-tibo-happy/main/install.sh | bash
#   境内: curl -fsSL https://cdn.jsdelivr.net/gh/elijah7x/is-tibo-happy@main/install.sh | bash
set -euo pipefail

NAME=is-tibo-happy
REPO=elijah7x/is-tibo-happy
REF=main
DEST="$HOME/Library/Application Support/$NAME"
PLIST="$HOME/Library/LaunchAgents/com.istibohappy.daemon.plist"
BASES=(
  "https://raw.githubusercontent.com/$REPO/$REF"
  "https://cdn.jsdelivr.net/gh/$REPO@$REF"
)
FILES=(
  src/is-tibo-happy.mjs src/cdp.mjs src/net.mjs src/state.mjs src/widget.js
  assets/avatar/avatar.json
)

dl() {  # dl <repo相对路径> <本地路径>：逐源尝试
  local rel="$1" out="$2" b
  for b in "${BASES[@]}"; do
    curl -fsSL --connect-timeout 8 --max-time 60 "$b/$rel" -o "$out" 2>/dev/null && return 0
  done
  echo "下载失败: $rel（所有源都不可达，检查网络后重试）" >&2
  exit 1
}

[ "$(uname)" = "Darwin" ] || { echo "仅支持 macOS"; exit 1; }
APP=""
for p in "/Applications/ChatGPT.app" "$HOME/Applications/ChatGPT.app"; do
  [ -d "$p" ] && APP="$p" && break
done
[ -n "$APP" ] || { echo "未找到 ChatGPT.app（Codex 桌面版宿主，/Applications 或 ~/Applications）"; exit 1; }

NODE="$(command -v node || true)"
if [ -z "$NODE" ]; then
  for p in /opt/homebrew/bin/node /usr/local/bin/node; do [ -x "$p" ] && NODE="$p" && break; done
fi
[ -n "$NODE" ] || { echo "需要 Node.js ≥ 22，请先安装（brew install node）"; exit 1; }
"$NODE" -e 'process.exit(typeof WebSocket === "function" ? 0 : 1)' 2>/dev/null \
  || { echo "Node.js 版本过旧（需要 ≥ 22 提供全局 WebSocket），当前: $("$NODE" -v)"; exit 1; }
if [[ "$NODE" =~ /\.nvm/|/fnm/|/volta/ ]]; then
  echo "⚠ 检测到版本管理器安装的 Node（$NODE），切换/删除该版本会导致后台服务停止；建议 brew install node"
fi

echo "▸ 下载运行文件 → $DEST"
mkdir -p "$DEST/src" "$DEST/assets/avatar"
for f in "${FILES[@]}"; do
  dl "$f" "$DEST/$f"
  case "$f" in
    *.mjs|*.js) "$NODE" --check "$DEST/$f" >/dev/null 2>&1 || { echo "下载的文件损坏: $f"; exit 1; } ;;
    *.json) "$NODE" -e 'JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"))' "$DEST/$f" >/dev/null 2>&1 || { echo "下载的文件损坏: $f"; exit 1; } ;;
  esac
done
[ -f "$PLIST" ] || touch "$DEST/.first-run"   # 仅首次安装授权拉起 App；重装不打扰

echo "▸ 注册并启动后台服务（LaunchAgent）"
cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.istibohappy.daemon</string>
  <key>ProgramArguments</key>
  <array><string>$NODE</string><string>$DEST/src/is-tibo-happy.mjs</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ThrottleInterval</key><integer>30</integer>
  <key>StandardOutPath</key><string>$DEST/daemon.log</string>
  <key>StandardErrorPath</key><string>$DEST/daemon.log</string>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>$(dirname "$NODE"):/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string></dict>
</dict></plist>
EOF
launchctl bootout "gui/$(id -u)" "$PLIST" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$PLIST"
launchctl print "gui/$(id -u)/com.istibohappy.daemon" >/dev/null 2>&1 \
  || { echo "后台服务启动失败，查看 $DEST/daemon.log"; exit 1; }

cat <<'DONE'

✓ 安装完成，后台服务已启动，无需任何手动操作。

  若 Codex（ChatGPT.app）正在运行，它将被自动重启一次以挂载调试端口
  —— 对话内容不会丢失，这是唯一一次打扰。

  打开侧栏底部账号菜单即可看到 Tibo。
  卸载: curl -fsSL https://raw.githubusercontent.com/elijah7x/is-tibo-happy/main/uninstall.sh | bash
DONE
