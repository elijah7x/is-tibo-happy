# is-tibo-happy

[English](README.md) | **简体中文**

住在 Codex 桌面版账号菜单里的小伙伴：像素 Tibo 头像、他的心情、还有一行"下次重置什么时候"。

当 Tibo（[@thsottiaux](https://x.com/thsottiaux)）预告 Codex 用量重置，Tibo 就 **HAPPY**（红）；三天没消息，他变 **UNHAPPY**（灰）。

**非官方粉丝项目**——与 OpenAI、Tibo 本人均无关。预告是 Tibo 对全体用户的公开表态，**不是你个人额度的重置时刻**。

## 安装

```bash
curl -fsSL https://raw.githubusercontent.com/elijah7x/is-tibo-happy/main/install.sh | bash
```

国内网络（走 jsDelivr）：

```bash
curl -fsSL https://cdn.jsdelivr.net/gh/elijah7x/is-tibo-happy@main/install.sh | bash
```

**需要**：macOS · Codex 桌面版（ChatGPT.app）· Node.js ≥ 22（没有就 `brew install node`）。

装完即生效：如果 Codex 正在运行，它会**重启一次**（对话不丢）——这是唯一一次打扰。之后 Tibo 就在左下角账号菜单、你名字的下面。开机自启，Codex 重启自动接上，没有任何手动步骤。

## 更新

再跑一次上面的安装命令即可——脚本是幂等的，只会替换文件并就地重启守护进程。（特意没有做自动更新。）

## 卸载

```bash
curl -fsSL https://raw.githubusercontent.com/elijah7x/is-tibo-happy/main/uninstall.sh | bash
```

停掉后台服务、删掉所有文件、把 Codex 重启回普通模式（调试端口随之立刻关闭）。不留残余。

<details>
<summary>手动卸载</summary>

```bash
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.istibohappy.daemon.plist
rm -f ~/Library/LaunchAgents/com.istibohappy.daemon.plist
rm -rf ~/Library/Application\ Support/is-tibo-happy
# 然后退出并重新打开 Codex
```
</details>

## ⚠️ 装前须知

为了把卡片画进 Codex 界面，ChatGPT.app 会以 `--remote-debugging-port=9333` 调试模式运行：

- 这个端口在 Codex 运行期间对**本机所有程序**可达——理论上你电脑上的其它程序能借它读取 Codex 界面内容
- 只监听 127.0.0.1，局域网和互联网**不可达**
- 卸载时会把 Codex 重启回普通模式，端口立刻关闭

如果你在意"本机程序互相隔离"这层安全模型，请不要安装。

## 它怎么工作

```
后台守护进程（LaunchAgent，空闲 ≈ 0% CPU，约 50MB 内存）
  └─ 每 15 分钟拉一次公开重置数据（打开菜单时也顺手刷新）
  └─ 通过 CDP 往账号菜单注入一张卡片——不改 App 本体
```

- 不改 App 文件、不装浏览器扩展、不读你的对话
- 出网只有一件事：拉公开的重置预告 JSON，带自报家门的 User-Agent（`is-tibo-happy/0.1`）
- 你退出 Codex，它就安静等着；你打开，它才接上。绝不替你启动 Codex
- 卡片零定时器、零动画、零网络请求，菜单关了就卸掉

### 数据从哪来

```
GitHub Actions 每 20 分钟抓一次源站 → 存成 public/state.json
客户端：betteropc.com → GitHub raw → jsDelivr（国内可达）→ codex-reset.com → codex-resets.com
```

源站永远只看到这一个定时任务，装多少人都不会给它增加负担。四级降级、每级都有超时。最近一次好结果会落盘缓存、重启也在——**连续 12 小时**拿不到数据才显示 OFFLINE。

### Tibo 什么时候开心

| Tibo 说了什么 | 卡片 |
|---|---|
| 给了准确时间 | HAPPY · `6 天后重置` / `3 小时后重置` / `即将重置` |
| 给了截止（"周二前"） | HAPPY · `最晚周二重置` |
| 暗示了一下（"coming Tuesday"） | HAPPY · `预计周二重置` |
| 预告时点过了、未确认 | HAPPY · `随时重置` |
| 刚重置过（三天内） | HAPPY · `刚刚重置` / `上次重置 2 天前` |
| 三天没消息 | UNHAPPY · `暂无重置预告` |
| 连续 12h 拿不到数据 | OFFLINE · `数据不可用` |

星期几按**你的时区**换算（Tibo 的"周二"在北京可能是周三早上）。只要有预告就是 HAPPY，哪怕还要等六天——预告也很少跳票，只会早来或迟到：过了预告时点还有 36 小时迟到宽限，确认落地后落到"刚刚重置"。卡片语言跟随 Codex 界面（中/英）。

## 常见问题

**菜单里没出现卡片？**
`tail ~/Library/Application\ Support/is-tibo-happy/daemon.log` 看最后几行。常见原因：Codex 刚重启还没连上（等 10 秒再开菜单）；Codex 不在 `/Applications` 或 `~/Applications`。

**Codex 更新后卡片消失了？**
守护进程会自动重连。如果界面改版导致找不到挂载点，日志里会有 `menu-unmatched`，欢迎开 issue。

**用 nvm / fnm / volta 装的 Node？**
安装器会记住当前 Node 的绝对路径。你切换或删除那个版本，后台服务会停。建议 `brew install node` 装一个稳定的。

**想看源码 / 移植？**
运行时代码 5 个文件（`src/`，约 900 行），零依赖。`node --test 'test/*.test.mjs'` 跑 50 条规格测试。Windows/Linux 欢迎 fork 移植——`install.sh` 里就是要重新实现的部分。

---

数据来源：[betteropc.com](https://betteropc.com/ai-products/reset-signals/codex)（主）、[codex-reset.com](https://codex-reset.com)、[codex-resets.com](https://codex-resets.com)（备）。感谢他们。
