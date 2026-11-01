# o824-rs

O824 内核的 Rust 实现。**Go 版（`D:\dsh work\Object 824`，tag `v0-go-reference`）是参考实现/oracle**：它已冻结，不再加功能，但保持可编译可运行，用来在两边行为不一致时判定谁对。

## 这是什么

一个 Windows 命令行内核：发现本机的 DeepSeek Harness 安装、诊断它为什么起不来、在改动任何文件之前自动快照、修不好可以回滚。它对 UI 说 JSON-RPC（stdio），对人类说英文兜底文案 + 错误码，人话由 UI 渲染。

## 先读什么

| 顺序 | 文件 | 为什么 |
|---|---|---|
| 1 | [`docs/PORTING-NOTES.md`](docs/PORTING-NOTES.md) | **必读**。九条用真机调试换来的结论，漏一条就会重踩 |
| 2 | [`docs/MIGRATION-RUST.md`](docs/MIGRATION-RUST.md) | 施工图：crate 布局、11 个阶段、每阶段验收、八个风险 |
| 3 | [`docs/design/2026-10-28-dsh-launcher-design.md`](docs/design/2026-10-28-dsh-launcher-design.md) | 设计规范（唯一权威） |
| 4 | [`docs/week1/`](docs/week1/) | 第一周的规格级材料 |

## 三条机器判据（移植算不算成功的唯一标准）

| # | 判据 | 位置 |
|---|---|---|
| 1 | Golden：八棵 fixture 树上 `doctor --json` / `plan --json` 的 JSON 逐字节相同 | `spec/golden/` |
| 2 | Transcript：mock 回放的帧序列逐帧相同 | `spec/transcripts/` |
| 3 | 实验室：七种真机故障的 `doctor → repair → doctor` 退出码与文件效果相同 | `lab/`（需 Windows + DSH） |

前两条在仓库里、可自动化。**第 1 条是第一周就要能跑的**。

## 目录

```
crates/          workspace（16 个 crate，见 docs/MIGRATION-RUST.md）
spec/            golden + transcripts（跨实现契约，从 Go 仓库复制）
docs/            设计、迁移方案、移植笔记、第一周规格
lab/             真机实验室脚本（PowerShell，与语言无关，可直接复用）
```

## 构建

```bash
cargo build --release --bin o824
cargo test --workspace
```

目标产物：单个静态 `o824.exe`（Windows x64），无 CGO 等价物、不依赖外部 DLL。

## 与 Go 版的关系

- 行为不一致时：**先假定 Rust 错**，除非能证明 golden 本身是错的。
- golden 需要变更时：在提交信息里写清改了哪条契约，并同步回 Go 仓库（它是 oracle）。
- 不在本仓库修 Go 代码；Go 仓库只作为参考保留。
