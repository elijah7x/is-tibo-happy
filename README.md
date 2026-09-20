# is-tibo-happy

> Is Tibo happy? 打开 Codex 账号菜单看一眼就知道。

Codex 桌面版账号菜单里的一张小卡片：像素 Tibo 头像 + 心情 + 一行"下次重置什么时候"。

当 Tibo（[@thsottiaux](https://x.com/thsottiaux)）预告 Codex 用量重置，Tibo 就 **HAPPY**（红）；超过三天没消息，Tibo 就 **UNHAPPY**（灰）。

<!-- TODO: 演示 GIF：点开账号菜单 → 卡片出现 -->

**非官方玩具项目**，与 OpenAI、Tibo 本人均无关系。预告是 Tibo 对全体用户的公开表态，**不等于你个人账号的额度重置时刻**。

---

## 安装（一行命令，装完即生效）

```bash
curl -fsSL https://raw.githubusercontent.com/elijah7x/is-tibo-happy/main/install.sh | bash
```

中国大陆网络（走 jsDelivr）：

```bash
curl -fsSL https://cdn.jsdelivr.net/gh/elijah7x/is-tibo-happy@main/install.sh | bash
```

**需要**：macOS · Codex 桌面版（ChatGPT.app）· Node.js ≥ 22（没有的话 `brew install node`）。

装完会发生什么：
1. 如果 Codex 正在运行，它会**自动重启一次**（对话不丢）——这是唯一一次打扰
2. 打开左下角账号菜单，Tibo 就在你名字下面

不需要任何手动开启步骤。之后开机自启、Codex 重启后自动接上。

## 卸载（同样一行）

```bash
curl -fsSL https://raw.githubusercontent.com/elijah7x/is-tibo-happy/main/uninstall.sh | bash
```

会停掉后台服务、删掉所有文件、把 Codex 重启回普通模式。不留任何残余。

<details>
<summary>手动卸载（如果上面的命令不可用）</summary>

```bash
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.istibohappy.daemon.plist
rm -f ~/Library/LaunchAgents/com.istibohappy.daemon.plist
rm -rf ~/Library/Application\ Support/is-tibo-happy
# 然后退出并重新打开 Codex
```
</details>

---

## ⚠️ 装前须知

为了把卡片放进 Codex 界面，Codex（ChatGPT.app）会以 **调试模式**（`--remote-debugging-port=9333`）运行：

- 这个端口在 Codex 运行期间对**本机**开放——你电脑上的任何程序理论上都能借它读取 Codex 界面里的内容
- 只监听 127.0.0.1，外网和局域网**不可达**
- 卸载脚本会立刻把 Codex 重启回普通模式，端口随之关闭

如果你在意"本机程序互相隔离"这层安全模型，请不要安装。

## 它怎么工作

```
后台守护进程（LaunchAgent，空闲 ≈ 0% CPU，~40MB 内存）
  └─ 每 15 分钟拉一次公开重置数据（打开菜单时也会顺手刷新）
  └─ 通过 CDP 把一张卡片注入 Codex 的账号菜单（不修改 App 本体）
```

- **不改 App 文件**、不装浏览器扩展、不读你的对话
- 出网只有一件事：拉公开的重置预告 JSON，带自报家门的 User-Agent（`is-tibo-happy/0.1`）
- 你退出 Codex，它就安静等着；你打开 Codex，它才接上。绝不替你启动 Codex
- 卡片渲染零动画、零定时器，菜单关了就卸掉

### 数据从哪来

```
GitHub Actions 每 20 分钟抓一次 codex-reset.com → 存成 public/state.json
客户端：GitHub 镜像 → jsDelivr 镜像（大陆可达）→ 直连 codex-reset.com → 备用 codex-resets.com
```

源站只看到我们一个定时任务，装多少人都不会给它增加负担。四级降级、每级都有超时；拿不到数据时沿用最近一次结果（落盘缓存，重启也在），**连续 12 小时**拿不到才显示 OFFLINE。

### Tibo 什么时候 HAPPY

| Tibo 说了什么 | 卡片 |
|---|---|
| 给了准确时间 | HAPPY · `6 天后重置` / `3 小时后重置` / `即将重置` |
| 给了截止（"周二前"） | HAPPY · `最晚周二重置` |
| 暗示了一下（"coming Tuesday"） | HAPPY · `预计周二重置` |
| 刚重置过（三天内） | HAPPY · `刚刚重置` / `上次重置 2 天前` |
| 三天没消息 | UNHAPPY · `暂无重置预告` |
| 数据源连续 12h 拿不到 | OFFLINE · `数据不可用` |

星期几按**你的时区**换算（Tibo 的"周二"在北京可能是周三早上）。有预告永远 HAPPY，哪怕还要等六天。界面跟随 Codex 语言（中/英）。

## 常见问题

**菜单里没出现卡片？**
`cat ~/Library/Application\ Support/is-tibo-happy/daemon.log` 看最后几行。常见原因：Codex 刚重启还没连上（等 10 秒再开菜单）；Codex 不在 `/Applications` 或 `~/Applications`。

**Codex 更新后卡片消失了？**
守护进程会自动重连。如果界面改版导致找不到挂载点，日志里会有 `menu-unmatched`，欢迎开 issue。

**用 nvm / fnm 装的 Node？**
安装器会记住当前 Node 的绝对路径。你切换或删除那个版本，后台服务会停。建议 `brew install node` 装一个稳定的。

**我想看源码 / 参与**
运行时代码 5 个文件（`src/`，约 900 行），零依赖。`node --test 'test/*.test.mjs'` 跑 50 条规格测试；`designs/` 里有设计会审与三轮审计记录。

---

数据来源：[codex-reset.com](https://codex-reset.com)（主）、[codex-resets.com](https://codex-resets.com)（备）。感谢他们。
