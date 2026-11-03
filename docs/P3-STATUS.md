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

1. **"还原"按钮没做，而且不该先做**。协议层现在有了 `action: "rollback"`（§21.5 的 action
   列表本来就有），`plan` 会生成 `restore-snapshot` 步骤、`apply` 能执行——但**快照只覆盖设置
   文件，不覆盖 `node_modules`**（设计文档明写）。而我们的修复大多动的正是插件目录，所以
   "还原"按钮按下去并不会撤销刚才那次修复。一个不会还原的还原按钮比没有更糟，所以先不摆。
   要做的话得先让快照覆盖插件目录，那是快照层的设计变更。
2. **没有安装入口**（§20.1 第一条路径"安装 DSH"）。当前只能列已有的。
3. **启动/停止按钮**（§21.5 的 start/stop 已经是 plan action，窗口还没接）。
4. 托盘、单实例、自动更新（P3 之外）。

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
