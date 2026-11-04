# P3 状态（GUI）

> **2026-11-05：窗口外观已被 P4 扩展的"发行版板"替换**，见
> [`P4-GUI-BOARD.md`](P4-GUI-BOARD.md) 与
> [`superpowers/specs/2026-11-05-p4-gui-board-design.md`](superpowers/specs/2026-11-05-p4-gui-board-design.md)。
> 本文下面的能力清单仍然成立；"列表在主区"这一条已不成立（列表进了"全部安装"弹层）。

> 判据来源：`docs/design/2026-10-28-dsh-launcher-design.md` §7.4、§20。

## 已经能用的

`apps/o824-gui`（Tauri 2 + WebView2，**独立 workspace**，用 MSVC 构建）：

| 能力 | 判据 / 证据 |
|---|---|
| 起窗口 | release 版双击可开；`%LOCALAPPDATA%` 拒绝建目录时回退到程序目录或 `%TEMP%` |
| 列出安装 | `list` 287–650 ms（`discover`），三种形态都列（web / tui / desktop） |
| 首屏三句话 | 由 `o824-present` 产出，§20.1 的三条路径（没装 / 可以启动 / 起不来） |
| 点开一条看详情 | 每条 finding 显示 `headline / reason / next_step`，全部是人话 |
| **一键修复** | plan → 确认（含备份提示与步骤）→ apply → 重新诊断；在 fixture 上端到端验过 |
| 信任声明 | 常驻"非官方 · 未签名 · 仓库地址"（§20.6） |
| 中英双语 | 词条在 `crates/o824-present`，测试保证两种语言都有、且不含内部术语 |

## 关键设计

- **窗口是内核的客户端**（§7.5）：Rust 侧 spawn `o824 kernel --stdio`，前端一行协议都没有。
- **诊断按需**：一次 `doctor` 要 3.7–5.4 秒（遍历每个 profile 的 `node_modules`）。多个安装时
  不预先诊断，点了才算。
- **apply 只传计划 id**：内核保存了它写的计划，窗口确认的是内核记录的东西，而不是窗口自己拼的
  步骤。
- **文案在 `o824-present`**：`describe()` 把检查编号翻译成句子，`step_name()` 把计划步骤类型
  翻译成人话；测试禁止 7 个内部术语出现在任何界面字符串里。

## 已知缺口（按重要性）

1. ~~"还原"按钮~~ **已做，而且是真的能撤销**。没有走"让快照覆盖插件目录"那条路（一个 profile
   几万文件、上 GB），而是让 **`apply` 把每个被替换的链接记进 journal**（`replaced`），
   `rollback` 再按它反向重放——恢复的正是这次修复动过的东西，不是"整棵树回到某时刻"。
   判据：fixture 上 `repair → rollback` 之后 finding 回来了（`a_repair_that_rebuilt_a_link_is_undone_by_the_rollback`）。
   局限：只能撤销最近一次 apply；journal 里 `kind: other`（原本是真实文件/目录）的条目不动，
   因为无法重建，猜比不动更糟。
2. **安装入口：引导，不代装**，并且**已经截图验过**（2026-11-04）。§20 的原话是"web/tui 只
   引导、不代装"，所以主按钮给三条官方途径与可复制的命令。空态本身在本机造不出来（永远有安装），
   用 `O824_GUI_EMPTY=1 O824_GUI_GUIDE=1` 打开 `?empty=1&guide=1` 渲染验证——这是测试开关，
   不是产品路径。
3. ~~启动/停止按钮~~ **已接**：主按钮按自己的标签执行（`FirstUse::action()` 同时给出句子和
   动词，测试保证两者不会不一致），web 形态启动后给出可点开的地址。
4. **单实例：已做**。第二个窗口不再打开——它找到标题为 `O824` 的已有窗口，提到前面，然后退出。
   先试过命名互斥体，在这台机器上**每次都报"已存在"**（`GetLastError` 是线程局部槽，任何
   Win32 调用都会改它，而我在检查前写了日志），追它比这个守卫本身更贵。用窗口本身作证据，
   简单且可观察。
5. **托盘：已恢复**（2026-11-04 深夜）。之前删掉它是因为图标从未出现，而根因不是代码：
   DSH 工作区里的文件带 **Low 强制完整性标签**，从这种文件启动的进程以 Low IL 运行，而通知区域是
   Medium——UIPI 不允许低完整性进程注册，于是 `Shell_NotifyIcon` 返回 `ACCESS_DENIED`。同一个 exe
   放到 `%LOCALAPPDATA%` 或 D 盘别处就正常。**用 `o824 install` 装到
   `%LOCALAPPDATA%\o824` 再从那里运行**，不要从构建目录直接跑。详见 `docs/TRAY-INVESTIGATION.md`。
