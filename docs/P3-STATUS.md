# P3 状态（GUI）

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
2. **安装入口只做到"引导"**：主按钮在"还没装"时给出三条官方途径（Windows 安装包、两个 npm
   包）与可复制的命令，**不代跑** `npm i -g`（§20 的原话："web/tui 只引导、不代装"）。
   **没能截图验证**：这台机器上永远有安装，`--root` 指向不存在的目录时内核仍会给出一个空安装，
   所以"还没装"的首屏在本机造不出来。要验证得给发现层加一个"零安装"的测试注入点。
3. **启动/停止按钮**（§21.5 的 start/stop 已经是 plan action，窗口还没接）。
4. **单实例：已做**。第二个窗口不再打开——它找到标题为 `O824` 的已有窗口，提到前面，然后退出。
   先试过命名互斥体，在这台机器上**每次都报"已存在"**（`GetLastError` 是线程局部槽，任何
   Win32 调用都会改它，而我在检查前写了日志），追它比这个守卫本身更贵。用窗口本身作证据，
   简单且可观察。
5. **托盘：已做**（显示窗口 / 检查更新 / 退出；左键点图标也显示窗口）。**自动更新只做了安全的
   那半**：菜单里"检查更新"打开 GitHub 发布页，**不替换自身二进制**——本产品不买代码签名证书，
   一个能自更新的未签名 exe 是启动器最不该做的事。真正的自动更新需要先解决签名与更新源。

## 开发时怎么跑

```bash
source scripts/env.sh          # 自包含 gnu 工具链（给主工作区）
source scripts/env-msvc.sh     # MSVC + SDK（给 GUI，Tauri 需要）
export RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc
cd apps/o824-gui && cargo build --release
bash scripts/gui-shot.sh out.png 6     # 构建→启动→截图，并拒绝截到旧进程
```

测试用的两个开关（**只在开发/验证时用**）：

- `O824_ROOT=<fixture 树>`：让窗口对着合成树跑，不影响真实安装；
- `O824_GUI_AUTOREPAIR=hold|slow|1`：自动走一遍修复流程（`hold` 停在确认面板供拍照）。

## 构建期两个坑（都踩过）

1. 前端资源是**编译期嵌入**的，`build.rs` 里必须声明 `dist/` 为输入，否则改完页面跑的还是旧的；
2. **运行中的进程占着 exe**，`cargo build` 会静默保留旧二进制——改完先杀进程。
