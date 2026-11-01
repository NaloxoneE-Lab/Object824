# 迁移到 Rust：方案与验收

**决策（2026-11-01）**：完全重写，语言 Rust。理由：尚未发布，没有用户、没有兼容包袱，此刻迁移代价最低。

本文是**为什么**与**怎么切**；**什么时候、按什么顺序、卡住了怎么办**见 [`MIGRATION-ROADMAP.md`](MIGRATION-ROADMAP.md)。

本文是给 Rust 团队的施工图。**Go 实现从今天起冻结为"参考实现 / oracle"**：不再加功能，但必须保持可编译可运行，用来在两边行为不一致时判定谁对。

---

## 0. 三条不可谈判的验收

任何阶段、任何 crate，只要这三条不满足就不算完成：

| # | 判据 | 位置 | 说明 |
|---|---|---|---|
| 1 | **Golden 一致** | `spec/golden/*.json`（16 个） | 八棵 fixture 树上 `doctor --json` 与 `plan --json`，规范化后逐字节相等 |
| 2 | **Transcript 一致** | `spec/transcripts/*.jsonl`（3 个） | mock 回放的帧序列逐帧相等（`id`/`seq` 按规则重写） |
| 3 | **实验室一致** | `broken-dsh-lab/`（Windows 真机） | 七种故障的 `doctor → repair → doctor` 退出码与文件效果一致 |

前两条在仓库里、可自动化；第三条需要一台装了 DSH 的 Windows 机器。

**如果 Rust 实现和 golden 不一致，先假定 Rust 错**——除非能证明 golden 本身是错的（比如它记录了某个 Go 的实现细节）。这时先改 golden 并在提交信息里说明为什么，再让两边都跟上。

---

## 1. 仓库策略（建议）

**新仓库 `o824-rs`，Go 仓库冻结并打 tag。**

```
o824-rs/
├── Cargo.toml              workspace
├── crates/
│   ├── o824-errs/          错误码
│   ├── o824-model/         findings 信封
│   ├── o824-execx/         argv 子进程
│   ├── o824-lock/          Windows 独占锁
│   ├── o824-state/         原子状态文件
│   ├── o824-discovery/     注册表/卷/asar
│   ├── o824-fixtures/      合成安装树（dev-dependency）
│   ├── o824-verify/        七个检查
│   ├── o824-plan/          纯函数计划
│   ├── o824-snapshot/      快照
│   ├── o824-apply/         执行 + 回滚
│   ├── o824-protocol/      LSP 帧 + JSON-RPC + 作业
│   ├── o824-mock/          transcript 回放
│   ├── o824-diagnose/      诊断包
│   ├── o824-cli/           子命令
│   └── o824/               bin（一个 exe，多角色）
├── spec/                   从 Go 仓库复制：golden + transcripts
├── docs/                   PORTING-NOTES.md + 设计文档 + 本文
└── tests/                  跨 crate 的验收测试（golden/transcript/lab 驱动）
```

为什么不放同一个仓库：两套实现共存时，`cargo test` 和 `go test` 会各自以为自己是对的；分开之后，"新仓库必须复现旧仓库的 golden"这条关系是单向的、可判定的。

`spec/` 用 **git subtree** 引入（不是 submodule），这样 Rust 仓库自带契约、不依赖外部 checkout；Go 仓库那边改了 golden 再 subtree pull。

Go 仓库冻结动作：

```bash
git tag -a v0-go-reference -m "Go 参考实现，冻结；Rust 重写期间作为 oracle"
```

---

## 2. 迁移顺序

按依赖图自底向上。每个阶段结束时**必须**跑通对应验收，再进下一阶段。

| 阶段 | crate | 验收 | 估时 |
|---|---|---|---|
| 1 | `errs` `model` `version` | 单元测试；findings JSON 与 golden 里的 finding 逐字段相同 | 2 天 |
| 2 | `execx` `lock` `state` | 单元测试（含两进程争锁） | 4 天 |
| 3 | `discovery` | `discover --json` 对真机输出与 Go 版相同；asar 版本解析对真 asar 有效 | 5 天 |
| 4 | `fixtures` | 八棵树能被 `discovery::validate` 判为 ok / `E_INSTALL_DIR` | 5 天 |
| 5 | `verify` | **golden 的 8 个 `.doctor.json` 全部通过** | 9 天 |
| 6 | `plan` | **golden 的 8 个 `.plan.json` 全部通过** | 5 天 |
| 7 | `snapshot` | create/list/restore 单元测试 + 保留 5 份 | 6 天 |
| 8 | `apply` | 实验室七种故障的 repair 效果一致 | 10 天 |
| 9 | `cli` | 六个子命令、退出码 0–8、`--json` 纯净；`discover/doctor/repair --dry-run` 与 Go 版逐字节一致 | 9 天 |
| 10 | `protocol` `mock` | **3 个 transcript 逐帧通过** | 12 天 |
| 11 | `diagnose` | zip 结构 + 脱敏 | 3 天 |

合计约 70 人日。三人并行时，1–2 可并行、3–4 可并行、5 必须等 3/4、6 等 5、8 等 7。

**第 5 阶段是第一个真正的分水岭**：golden 的 doctor 输出全过，说明七个检查的语义被完整复现了。在那之前不要碰 `apply`。

---

## 3. 八个必须知道的风险（都有对应的护栏）

1. **把 Go 的实现细节当契约**。已经踩过一次：空计划输出 `"steps": null`（Go 的 nil slice），已改成 `[]`。看到 golden 里有奇怪形状，先问"这是行为还是语言的偶然"。
2. **重新发明 F03**。判据不成立（见 PORTING-NOTES §1）。照抄现有的"warning 提示"，别"顺手实现"。
3. **pnpm 11 的四条规则**（PORTING-NOTES §2）。任何一条漏掉，修复就是假的。
4. **就地改 `node_modules`**。必须副本 + 交换（PORTING-NOTES §3）。
5. **YAML 保注释的编辑**。Go 用 `yaml.v3` 的 Node 树；Rust 的 `serde_yaml` 已停维护，需要挑一个能保留注释与顺序的库（`marked-yaml` 或 `yaml-rust2` 的 node API），**这是一个真实的选型风险**，建议第 1 周就做 spike。
6. **并发验证**。Rust 编译期能挡住数据竞争，但"作业终态保证""取消后不得再发 done"这类**时序**语义只能靠测试。用 transcript 测。
7. **Windows API 的坑**（PORTING-NOTES §5）：junction 的 substitute name 拼写、reparse 判定、`cmd /c` 的元字符、BOM、相对链接目标。
8. **测试基础设施别省**。Go 版 13.6k 行测试里有大量"这个故障该报什么"的断言，它们是**语义说明书**。Rust 版可以少写重复的，但 `verify` 和 `apply` 两个 crate 的测试量不该明显低于 Go 版。

---

## 4. 第一周建议

1. **搭 workspace + CI**：`cargo fmt --check`、`cargo clippy -D warnings`、`cargo test`、`cargo build --release`（Windows x64），并加一条"跑 golden 对比"的 job。
2. **YAML 库 spike**（风险 5）：拿真机的 `cordis.patch.yml`（1545 行、带注释）做"读入 → 改一个 key → 写出"，看注释和顺序能否保住。这一项结论会决定 `verify`/`plan`/`apply` 的实现方式。
3. **`errs` + `model` + `fixtures` 的第一棵树**：先让"能造出一棵 ok 树"跑通，因为后面所有测试都靠它。
4. **跑一次 oracle 对照**：在本机跑 `o824.exe doctor --root <fixture> --json` 拿到 golden，再用 Rust 侧读同一份 golden 做断言——**先把验收管线跑通，再写业务代码**。

---

## 5. Go 仓库的定位

- **冻结**：不再加功能（P1 剩余项——发现算法补层、形态适配、共存检测——直接在 Rust 侧做）。
- **保留可运行**：本机 `go build` 仍能出 exe，作为 oracle。
- **不再修 bug**，除非它同时是 Rust 侧的验收障碍（此时先修 golden 或 oracle，让两边对齐）。
- 文档（设计文档、PORTING-NOTES、本文、CHANGELOG）随新仓库走，Go 仓库保留历史。
