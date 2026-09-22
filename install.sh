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
REL="https://github.com/$REPO/releases/latest/download"

[ "$(uname)" = "Darwin" ] || { echo "仅支持 macOS"; exit 1; }
APP=""
for p in "/Applications/ChatGPT.app" "$HOME/Applications/ChatGPT.app"; do
  [ -d "$p" ] && APP="$p" && break
done
[ -n "$APP" ] || { echo "未找到 ChatGPT.app（Codex 桌面版宿主，/Applications 或 ~/Applications）"; exit 1; }

echo "▸ 下载二进制 → ${DEST}（universal2，sha256 校验后自检）"
mkdir -p "$DEST"
curl -fsSL --connect-timeout 8 --max-time 300 "$REL/$NAME" -o "$DEST/$NAME" \
  || { echo "二进制下载失败（GitHub Releases 不可达——境内网络可先开代理再重试）"; exit 1; }
curl -fsSL --connect-timeout 8 --max-time 30 "$REL/SHA256SUMS" -o "$DEST/SHA256SUMS" \
  || { echo "校验文件下载失败"; exit 1; }
( cd "$DEST" && grep " $NAME\$" SHA256SUMS | shasum -a 256 -c - >/dev/null ) \
  || { echo "二进制校验失败（下载被篡改或损坏），已中止"; rm -f "$DEST/$NAME"; exit 1; }
chmod 755 "$DEST/$NAME"
"$DEST/$NAME" --selftest || { echo "二进制自检失败"; exit 1; }
[ -f "$PLIST" ] || touch "$DEST/.first-run"   # 仅首次安装授权拉起 App；重装不打扰

echo "▸ 注册并启动后台服务（LaunchAgent）"
mkdir -p "$(dirname "$PLIST")"   # 全新账户可能还没有 LaunchAgents 目录
cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.istibohappy.daemon</string>
  <key>ProgramArguments</key>
  <array><string>$DEST/$NAME</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ThrottleInterval</key><integer>30</integer>
  <key>StandardOutPath</key><string>$DEST/daemon.log</string>
  <key>StandardErrorPath</key><string>$DEST/daemon.log</string>
</dict></plist>
EOF
launchctl bootout "gui/$(id -u)" "$PLIST" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$PLIST"
launchctl print "gui/$(id -u)/com.istibohappy.daemon" >/dev/null 2>&1 \
  || { echo "后台服务启动失败，查看 $DEST/daemon.log"; exit 1; }

cat <<'DONE'

✓ 安装完成，后台服务已启动，无需任何手动操作。

  若 Codex（ChatGPT.app）正在运行，它会被静默重启一次以挂载调试端口
  —— 对话内容不会丢失，这是唯一一次打扰。

  打开侧栏底部账号菜单即可看到 Tibo。
  卸载: curl -fsSL https://raw.githubusercontent.com/elijah7x/is-tibo-happy/main/uninstall.sh | bash
DONE
