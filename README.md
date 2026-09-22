# is-tibo-happy

**English** | [简体中文](README.zh-CN.md)

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

**Requires**: macOS · Codex desktop (ChatGPT.app). No Node, no dependencies — one native binary (Apple Silicon + Intel universal).

That's it. If Codex is running, Tibo attaches to it **without restarting** — no chats lost, no dialogs, nothing interrupted. If Codex isn't running it just shows up next time you open it. Auto-starts on login, re-attaches whenever Codex restarts.

## Update

The daemon checks for a new release once a day and swaps itself in (sha256-verified, self-tested, restarted by launchd). To opt out, set `ITH_NO_UPDATE=1` or re-run with `--no-update` in the plist. Manual check: `~/Library/Application\ Support/is-tibo-happy/is-tibo-happy update`.

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

To draw inside Codex, the daemon briefly opens the app's built-in Node inspector (`SIGUSR1` → `127.0.0.1:9229`) and injects the card through Electron's own `webContents` API:

- The inspector is loopback-only and **closed right after each attach** (a few hundred ms per cycle) — no debug port is left listening
- During that window any local process could reach it — same loopback trust boundary as any local tool
- On Codex builds without the inspector handler it falls back to launching with `--remote-debugging-port=9333` (loopback-only, stays open while Codex runs); uninstalling relaunches Codex normally either way

If local-process isolation matters to you, don't install.

## How it works

```
LaunchAgent daemon (idle ≈ 0% CPU, ~15 MB)
  └─ fetches public reset data every 15 min (and on menu open)
  └─ attaches via SIGUSR1 → Node inspector → webContents.executeJavaScript
     — injects the card, closes the inspector; app files untouched
```

- No app patching, no browser extension, no reading your chats
- Only outbound traffic: public reset endpoints + a daily release check, with a self-declared UA (`is-tibo-happy/0.2`)
- You quit Codex → it waits quietly; you reopen it → it attaches. It never launches Codex for you
- The widget has zero timers, zero animation, zero network calls; unmounts when the menu closes

### Data path

```
GitHub Actions mirrors the source every 20 min → public/state.json
client: betteropc.com → GitHub raw → jsDelivr (China-reachable) → codex-reset.com → codex-resets.com
```

The source sites only ever see one polite cron job. Four fallback tiers, all timeout-bounded. The last good state is cached on disk and survives restarts — OFFLINE only appears after **12 hours** without any data.

### When is Tibo happy

| Tibo says | Card shows |
|---|---|
| An exact time | HAPPY · `reset in 6 days` / `reset in ~3h` / `reset imminent` |
| A deadline ("by Tuesday") | HAPPY · `reset by Tuesday` |
| A tease ("coming Tuesday") | HAPPY · `reset expected Tuesday` |
| Teased time passed, unconfirmed | HAPPY · `reset imminent` |
| It just reset (≤3 days) | HAPPY · `just reset` / `last reset 2d ago` |
| Nothing for 3+ days | UNHAPPY · `no reset news` |
| No data for 12+ hours | OFFLINE · `data unavailable` |

Weekdays are converted to **your timezone** (his "Tuesday" may be Wednesday morning in Beijing). Any signal means HAPPY, even six days out — and a tease stays HAPPY until it lands or is clearly missed (a 36h late-grace window); announced resets almost never slip, they just arrive early or late. Card text follows the Codex UI language (中文 / English).

## FAQ

**No card in the menu?**
`tail ~/Library/Application\ Support/is-tibo-happy/daemon.log`. Usually Codex just restarted (wait ~10s) or the app isn't in `/Applications` / `~/Applications`.

**Card vanished after a Codex update?**
The daemon re-attaches automatically. If an UI redesign breaks menu detection, the log shows `menu-unmatched` — please open an issue.

**Hack on it?**
Runtime is a single Rust binary (`native/`, ~1600 lines); the injected card stays JS (`src/widget.js`). `cargo test --manifest-path native/Cargo.toml` runs 85 spec tests. Windows/Linux ports welcome — see `install.sh` for what needs reimplementing.

---

Data: [betteropc.com](https://betteropc.com/ai-products/reset-signals/codex) (primary) · [codex-reset.com](https://codex-reset.com) · [codex-resets.com](https://codex-resets.com) (fallbacks) — thanks for the public data.
