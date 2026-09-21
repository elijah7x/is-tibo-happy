# is-tibo-happy

A tiny companion that lives inside the Codex desktop profile menu: pixel-art Tibo, his mood, and one line about the next reset.

When Tibo ([@thsottiaux](https://x.com/thsottiaux)) teases a Codex usage reset, Tibo is **HAPPY** (red). After three days of silence he turns **UNHAPPY** (gray).

<!-- TODO: demo screenshot / gif -->

**Unofficial fan project** — not affiliated with OpenAI or Tibo. Reset teases are public statements about the global schedule, **not your personal quota reset time**.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/elijah7x/is-tibo-happy/main/install.sh | bash
```

China-friendly mirror (jsDelivr):

```bash
curl -fsSL https://cdn.jsdelivr.net/gh/elijah7x/is-tibo-happy@main/install.sh | bash
```

**Requires**: macOS · Codex desktop (ChatGPT.app) · Node.js ≥ 22 (`brew install node`).

That's it. If Codex is running it restarts **once** (no chats lost); then Tibo sits under your name in the bottom-left account menu. Auto-starts on login, re-attaches when Codex restarts.

## Uninstall

```bash
curl -fsSL https://raw.githubusercontent.com/elijah7x/is-tibo-happy/main/uninstall.sh | bash
```

Stops the daemon, deletes every file, and relaunches Codex in normal mode (this also closes the debug port immediately). Nothing left behind.

<details>
<summary>Manual uninstall</summary>

```bash
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.istibohappy.daemon.plist
rm -f ~/Library/LaunchAgents/com.istibohappy.daemon.plist
rm -rf ~/Library/Application\ Support/is-tibo-happy
# then quit and reopen Codex
```
</details>

## ⚠️ Before you install

To draw inside Codex, ChatGPT.app runs with `--remote-debugging-port=9333`:

- The port is reachable by **any local process** while Codex runs — in theory another program on your Mac could read the Codex UI through it
- Loopback (127.0.0.1) only — never exposed to LAN or the internet
- Uninstalling relaunches Codex normally, closing the port right away

If local-process isolation matters to you, don't install.

## How it works

```
LaunchAgent daemon (idle ≈ 0% CPU, ~40 MB)
  └─ fetches public reset data every 15 min (and on menu open)
  └─ injects one card into the profile menu via CDP — app files untouched
```

- No app patching, no browser extension, no reading your chats
- Only outbound traffic: public reset endpoints with a self-declared UA (`is-tibo-happy/0.1`)
- You quit Codex → it waits quietly; you reopen it → it attaches. It never launches Codex for you
- The widget has zero timers, zero animation, zero network calls; unmounts when the menu closes

### Data path

```
GitHub Actions mirrors the source every 20 min → public/state.json
client: GitHub raw → jsDelivr (China-reachable) → codex-reset.com → codex-resets.com
```

The source sites only ever see one polite cron job. Four fallback tiers, all timeout-bounded. The last good state is cached on disk and survives restarts — OFFLINE only appears after **12 hours** without any data.

### When is Tibo happy

| Tibo says | Card shows |
|---|---|
| An exact time | HAPPY · `reset in 6 days` / `reset in ~3h` / `reset imminent` |
| A deadline ("by Tuesday") | HAPPY · `reset by Tuesday` |
| A tease ("coming Tuesday") | HAPPY · `reset expected Tuesday` |
| It just reset (≤3 days) | HAPPY · `just reset` / `last reset 2d ago` |
| Nothing for 3+ days | UNHAPPY · `no reset news` |
| No data for 12+ hours | OFFLINE · `data unavailable` |

Weekdays are converted to **your timezone** (his "Tuesday" may be Wednesday morning in Beijing). Any signal means HAPPY, even six days out. Card text follows the Codex UI language (中文 / English).

## FAQ

**No card in the menu?**
`tail ~/Library/Application\ Support/is-tibo-happy/daemon.log`. Usually Codex just restarted (wait ~10s) or the app isn't in `/Applications` / `~/Applications`.

**Card vanished after a Codex update?**
The daemon re-attaches automatically. If an UI redesign breaks menu detection, the log shows `menu-unmatched` — please open an issue.

**Node installed via nvm/fnm/volta?**
The installer records the current Node absolute path. Deleting or switching that version stops the daemon. `brew install node` is the sturdy option.

**Hack on it?**
Runtime is 5 dependency-free files in `src/` (~900 lines). `node --test 'test/*.test.mjs'` runs 50 spec tests. Windows/Linux ports welcome — see `install.sh` for what needs reimplementing.

---

## 中文速览

Codex 桌面版左下角账号菜单里的一张小卡片：像素 Tibo 头像 + 心情（红 = 有重置预告，灰 = 三天没消息）+ 一行重置时间。非官方玩具项目，与 OpenAI、Tibo 无关；显示的是全局预告，**不是你个人额度的重置时刻**。

**安装**（需要 macOS + Codex 桌面版 + Node ≥ 22）：跑上面第二条 jsDelivr 命令即可，装完即生效（Codex 会重启一次）。**卸载**：跑 `uninstall.sh` 那行命令，干净无残留。

**装前须知**：为实现注入，Codex 会以调试端口 9333 运行，本机程序理论上可借它读取界面内容（仅本机，外网不可达）。介意请勿装。

---

Data: [codex-reset.com](https://codex-reset.com) (primary) · [codex-resets.com](https://codex-resets.com) (backup) — thanks for the public data.
