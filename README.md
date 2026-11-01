# o824-rs

O824 内核的 Rust 实现。**Go 版（`D:\dsh work\Object 824`，tag `v0-go-reference`）是参考实现/oracle**：它已冻结，不再加功能，但保持可编译可运行，用来在两边行为不一致时判定谁对。

## 这是什么

一个 Windows 命令行内核：发现本机的 DeepSeek Harness 安装、诊断它为什么起不来、在改动任何文件之前自动快照、修不好可以回滚。它对 UI 说 JSON-RPC（stdio），对人类说英文兜底文案 + 错误码，人话由 UI 渲染。

> **如果你是被派来接手这个重构的 agent**：先读工作区根的 `..\HANDOFF.md`（`D:\dsh work\Object 824\HANDOFF.md`），它按顺序告诉你该读什么、第一步做什么、以及本机没有 Rust 工具链这件事。

## 先读什么

| 顺序 | 文件 | 为什么 |
|---|---|---|
| 0 | [`docs/M3-REPORT.md`](docs/M3-REPORT.md) | **现在在哪**：门 G3（实验室六场景 + CLI 收口） |
| 0.2 | [`docs/M2-REPORT.md`](docs/M2-REPORT.md) | 门 G2（8 个 plan golden 全绿，含确定性） |
| 0.2 | [`docs/M1-REPORT.md`](docs/M1-REPORT.md) | 门 G1 的判据与证据 |
| 0.5 | [`docs/M0-COMPLETION.md`](docs/M0-COMPLETION.md) | 门 G0 的判据、证据、以及还没做到的那一条 |
| 1 | [`docs/PORTING-NOTES.md`](docs/PORTING-NOTES.md) | **必读**。九条用真机调试换来的结论，漏一条就会重踩 |
| 2 | [`docs/MIGRATION-ROADMAP.md`](docs/MIGRATION-ROADMAP.md) | **路线图**：5 个里程碑、门 G0–G5、11 周排期、卡住了怎么办 |
| 3 | [`docs/MIGRATION-RUST.md`](docs/MIGRATION-RUST.md) | 施工图：crate 布局、每阶段验收、八个风险 |
| 4 | [`docs/design/2026-10-28-dsh-launcher-design.md`](docs/design/2026-10-28-dsh-launcher-design.md) | 设计规范（唯一权威） |
| 5 | [`docs/week1/`](docs/week1/) | 第一周的规格级材料（含 YAML spike 结论） |

## 三条机器判据（移植算不算成功的唯一标准）

| # | 判据 | 位置 |
|---|---|---|
| 1 | Golden：八棵 fixture 树上 `doctor --json` / `plan --json` 的 JSON 逐字节相同 | `spec/golden/` |
| 2 | Transcript：mock 回放的帧序列逐帧相同 | `spec/transcripts/` |
| 3 | 实验室：七种真机故障的 `doctor → repair → doctor` 退出码与文件效果相同 | `lab/`（需 Windows + DSH） |

前两条在仓库里、可自动化。**第 1 条是第一周就要能跑的**。

## 目录

```
crates/          workspace（19 个 crate，见 docs/M1-REPORT.md 的说明）
spec/            golden + transcripts（跨实现契约，从 Go 仓库复制）
docs/            设计、迁移方案、移植笔记、第一周规格、M0 报告
lab/             真机实验室脚本（PowerShell，与语言无关，可直接复用）
scripts/         env.sh / env.ps1：激活仓库内自包含的 Rust 工具链
spikes/          一次性选型实验（yaml-spike），独立 workspace，不进发布图
```

## 构建

```bash
cargo build --release --bin o824
cargo test --workspace
```

目标产物：单个静态 `o824.exe`（Windows x64），无 CGO 等价物、不依赖外部 DLL。

### 本机工具链（自包含，只装在 `o824-rs` 里）

这台机器没有 Visual Studio 构建工具，所以本地工具链是 `x86_64-pc-windows-gnu`，而且**全部装在仓库内**：`rustup`、cargo home、以及编译缓存都在 `.toolchain/`（git-ignore）。激活：

```powershell
. .\scripts\env.ps1      # PowerShell
```
```bash
source scripts/env.sh    # bash
```

脚本做四件事：设置 `CARGO_HOME` / `RUSTUP_HOME` 到 `.toolchain/`、把 `cargo` 加进 `PATH`、把 `TMP`/`TMPDIR` 指到仓库内（链接器写不了 `%TEMP%`）、打印 cargo 版本。

链接用 Rust 工具链自带的 **`rust-lld`**（见 `.cargo/config.toml`），不需要 MinGW、不需要 C 编译器。这一点是必要的：MinGW 的 gcc 在**路径含空格**时会崩（本仓库路径是 `D:\dsh work\Object 824\...`），而 `rust-lld` 不会。

CI（`.github/workflows/ci.yml`）跑的是 `windows-latest` 的 MSVC 宿主，与本地 GNU 宿主产物不同，但 `cargo test` 的语义一致。


## 与 Go 版的关系

- 行为不一致时：**先假定 Rust 错**，除非能证明 golden 本身是错的。
- golden 需要变更时：在提交信息里写清改了哪条契约，并同步回 Go 仓库（它是 oracle）。
- 不在本仓库修 Go 代码；Go 仓库只作为参考保留。
