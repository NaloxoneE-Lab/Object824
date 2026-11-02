# O824 Rust 迁移路线

本文回答三件事：**什么时候算迁完、按什么顺序、卡住了怎么办**。
为什么迁、crate 怎么切、有哪些坑，见 [`MIGRATION-RUST.md`](MIGRATION-RUST.md) 与 [`PORTING-NOTES.md`](PORTING-NOTES.md)。

- 决策日期：2026-11-01
- 参考实现：`D:\dsh work\Object 824`，tag `v0-go-reference`（冻结，可运行，当 oracle）
- 目标仓库：`D:\dsh work\Object 824\o824-rs`
- 团队假设：3 人（下文称 A / B / C），都熟 Rust，**都不熟 DSH 的内部细节**——所以文档和 golden 是唯一的事实源

---

## 一、迁移"完成"的定义

同时满足这五条才算完成，缺一条都不算：

| # | 定义 | 判据 |
|---|---|---|
| 1 | **行为等价** | 8 个 doctor golden + 8 个 plan golden 全绿 |
| 2 | **协议等价** | 3 个 transcript 逐帧通过 |
| 3 | **真机等价** | 实验室七种故障：`doctor → repair → doctor` 退出码与文件效果一致 |
| 4 | **可发布** | `cargo build --release` 出单个 `o824.exe`，在干净 Windows 上跑通六个子命令 |
| 5 | **可维护** | CI 全绿（fmt + clippy + test + golden + build），且真的在 GitHub 上跑过 |

**不包含**：P1 的剩余功能（发现算法补层、形态适配、共存检测）。那些在"迁移完成"之后直接在 Rust 侧做——**不要一边迁一边加功能**，那会让"行为等价"这个判据失效。

### P1 在建项（迁移之后，直接在 Rust 侧做）

| 项 | 状态 | 判据 |
|---|---|---|
| `o824-mock` npm 包（WASI，跨平台） | ✅ 2026-11-02 | `packages/o824-mock` 的 Node 用例全绿 + CI `mock-package` 在 Node 20/24 上绿 + 装 tarball 后能回放三个录制 |
| 发现算法补层（§7.7 第 1/4/5/6 层） | ✅ 2026-11-02 | 每层都有"找到 / 找不到"两侧用例（`cargo test -p o824-discovery`，24 个） |
| 形态适配（desktop / web，含 F11 版本漂移） | ✅ 2026-11-03 | 两种形态各自可发现（`cargo test -p o824-discovery`）；F11 在本机真机上报出 rc.3 vs rc.5 |
| 共存检测（并发实例、端口残留、版本漂移） | 🟡 | 第 7 层（孤儿 profile）与**版本漂移**已落地；并发实例与端口残留依赖 P2 的 start/stop |

设计见 [`docs/design/o824-mock-package.md`](design/o824-mock-package.md)，用户文档见
[`packages/o824-mock/README.md`](../packages/o824-mock/README.md)。

---

## 二、五个里程碑

```
第 1 周    M0 准备        workspace · CI · YAML spike · golden harness · errs/model/version · 第一棵树
第 2–4 周  M1 只读内核    discovery · 八棵树 · verify          → G1：8 个 doctor golden 全绿
第 4–6 周  M2 计划与快照  plan · snapshot                      → G2：8 个 plan golden 全绿
第 6–8 周  M3 执行与回滚  apply · cli 收口                      → G3：实验室七种故障 + CLI 输出与 Go 一致
第 8–10 周 M4 协议与 mock protocol · mock                       → G4：3 个 transcript 逐帧通过
第 10–11 周 M5 收口切换   diagnose · 发布构建 · 真机验收        → G5：v0.1.0-rs 发布，Go 仓库归档
```

合计约 **11 周**（70 人日 ÷ 3 人 + 依赖等待）。

### M0 · 准备（第 1 周）

| 任务 | 负责 | 验收 |
|---|---|---|
| workspace + CI | A | `cargo fmt/clippy/test/build` 全绿，CI 在 GitHub 上跑过 |
| **YAML spike** | B | 真机 1545 行 patch 文件：读入→原样写出逐字节相同；删一条只少那几行；加一个 map 键只多一行 |
| golden harness | C | 读 16 个 golden；故意改坏一个字段必须失败并打 diff |
| `errs` / `model` / `version` | A | 六个契约测试；真实 golden 反序列化再序列化逐字节相同 |
| 第一棵 `ok` 树 + asar | C | `discovery::validate` 判 `health=="ok"`、`version=="0.2.1-rc.3"` |

**门 G0**：五条全绿才能进 M1。YAML spike 的结论要写进 `docs/week1/yaml-spike-result.md`——它会改变 `verify`/`plan`/`apply` 的写法，晚发现等于返工。

### M1 · 只读内核（第 2–4 周）

| 任务 | 负责 | 依赖 |
|---|---|---|
| `execx`（argv 子进程） | A | M0 |
| `lock`（Windows 独占锁，E_BUSY） | A | M0 |
| `state`（原子状态文件） | A | M0 |
| `discovery`（注册表 / 固定卷 / asar 版本 / home 解析 / realpath 去重） | B | M0 |
| `fixtures` 八棵树 | C | 第一棵树（M0） |
| **`verify` 七个检查** | A+C | discovery、fixtures |

**门 G1（第一个真正的分水岭）**：8 个 `.doctor.json` 全绿。
在这之前**不要开始 `apply`**——`apply` 的每一步都建立在"检查是对的"之上。

### M2 · 计划与快照（第 4–6 周）

| 任务 | 负责 | 依赖 |
|---|---|---|
| `plan`（纯函数、15 分钟有效期、指纹、步骤映射） | B | G1 |
| `snapshot`（create/list/restore、保留 5 份、sha256 校验、原子 meta） | C | M0 |

**门 G2**：8 个 `.plan.json` 全绿。计划是纯函数，golden 还额外证明了**确定性**（同样的树，两次构建出同样的步骤）。

### M3 · 执行与回滚（第 6–8 周）

| 任务 | 负责 | 依赖 |
|---|---|---|
| `apply`（快照优先 → journal → 步骤 → 失败回滚 → 链接回放） | A | G2 |
| `apply` 的副本安装 + 目录交换（`--with-install`） | A | G2 |
| `cli` 收口（六子命令、退出码 0–8、`--json` 纯净、选择器） | B+C | 前面全部 |

**门 G3**：实验室七种故障全过，且 `discover` / `doctor --json` / `repair --dry-run --json` 的输出与 Go 版**逐字节一致**。

### M4 · 协议与 mock（第 8–10 周）

| 任务 | 负责 | 依赖 |
|---|---|---|
| `protocol`（LSP 帧、JSON-RPC、异步作业、plan/apply、发现缓存） | B | G3 |
| `mock`（transcript 回放、`--delay`/`--fail-at`/`--protocol`） | C | 帧编解码（可提前做） |

**门 G4**：3 个 transcript 逐帧通过；`o824 mock` 能替代内核驱动一个客户端。

### M5 · 收口与切换（第 10–11 周）

| 任务 | 负责 | 验收 |
|---|---|---|
| `diagnose`（zip + 脱敏） | C | 结构与 Go 版一致，默认不含绝对路径 |
| 发布构建（`-trimpath` 等价、版本注入、单 exe） | A | 干净 Windows 上跑通六子命令 |
| 真机全流程验收 | 全员 | 实验室七种故障 + `verify-f17.ps1` 级别的端到端 |
| 文档合并、Go 仓库归档 | B | README 指向 Rust 版；Go 仓库标记 archived |

**门 G5**：`v0.1.0-rs` 打 tag，Go 仓库归档。

---

## 三、并行怎么排（3 人）

```
周次   1    2    3    4    5    6    7    8    9   10   11
A    M0骨架  execx/lock/state  verify  ──  apply ──────  apply 发布
B    YAML   discovery ───────  plan  ──────  cli ──  protocol ──  文档
C    golden fixtures 八棵树 ──  verify  snapshot  cli  mock ──  diagnose
```

三条硬约束：

1. **G1 之前没人碰 `apply`**（A 在 G1 前做 execx/lock/state/verify）。
2. **`verify` 由 A+C 共同负责**，因为它最重（9 天）且是 G1 的瓶颈。
3. **`cli` 的骨架可以提前搭**（参数解析、退出码映射不依赖业务），但真正的收口必须等 `apply` 完成。

---

## 四、卡住了怎么办（回退策略）

| 卡在哪 | 现象 | 处理 |
|---|---|---|
| **YAML 保注释**（M0） | 三个库都过不了 round-trip | 退到"行级编辑"：解析目标条目的行区间，用文本编辑删/插。这条路 100% 保真，代价是写一个"YAML 条目→行号"映射（我们的两个场景结构都很简单） |
| **golden 对不上但看不出为什么**（M1/M2） | 差异只有几个字节 | 先跑 Go 版对同一棵树取输出，逐字符 diff；**先假定 Rust 错**。若确实是 golden 记录了 Go 的实现细节，改 golden 并在提交信息里写明理由 |
| **`apply` 的回滚测不出来**（M3） | 失败路径没有可观测差异 | 用 Go 版的 `apply_test.go` 当场景清单：逐条翻译成 Rust 测试，不要自己发明场景 |
| **整个迁移严重超期**（>14 周） | 进度落后于 Go 版可用性 | **Go 版仍是可发布状态**（tag `v0-go-reference`）。这时应当停下来评估：是继续投人，还是把 Rust 版降级为"P5 的第二实现"，Go 版继续做主线 |

**最后一条是这次迁移最重要的保险**：Go 版没有删、没有坏，任何时候都能发。

---

## 五、进度怎么记

在仓库根放一张表（每个 crate 一行），每次合并前更新：

| crate | 里程碑 | 状态 | 负责人 | 验收通过 |
|---|---|---|---|---|
| `errs` `model` `version` | M0 | ✅ | | G0 |
| `conformance`（golden/依赖图） | M0 | ✅ | | G0 |
| `yaml`（libyaml 包装，见 `M1-REPORT.md`） | M1 | ✅ | | G1 |
| `execx` `lock` | M1 | ✅ 跨进程锁有两进程测试 | | |
| `state` | M1 | ✅ data root + 原子 state.json | | |
| `discovery` | M1 | ✅ 注册表 + 固定卷 + realpath 去重；真机 `discover` 与 Go 逐字节相同 | | |
| `fixtures` | M1 | ✅ 八棵树 + junction/symlink，已被 Go oracle 交叉验证 | | G1 |
| `verify` | M1 | ✅ 七个检查 | | **G1** |
| `plan` | M2 | ✅ 步骤映射 + TTL + id + 指纹 | | **G2** |
| `snapshot` | M2 | ✅ create/create_if_changed/list/restore，保留 5 份 | | |
| `apply` | M3 | ✅ 原生 junction、行级白名单、快照回滚、journal、副本安装（`--with-install`，v0.1.1 补） | | **G3** |
| `cli` | M3 | ✅ `discover`/`doctor`/`repair`/`snapshot`/`rollback`，退出码 0–8，与 Go 逐字相同 | | **G3** |
| `protocol` `mock` | M4 | ✅ 帧编解码 + transcript 回放，3 个录制逐帧一致 | | **G4** |
| `diagnose` | M5 | ✅ zip + 默认脱敏，真机验证无绝对路径残留 | | **G5** |

状态只有三种：⬜ 未开始 / 🟡 进行中 / ✅ 门已过。**"写完了但门没过"不算 ✅。**

> **2026-11-02：迁移完成。** G0–G5 全部通过，`--with-install`（v0.1.1）已实现并真机验收，
> CI 在 GitHub 上真跑过（[run 41392699018](https://github.com/NaloxoneE-Lab/Object824/actions/runs/41392699018)，
> 八步全过）。仓库：<https://github.com/NaloxoneE-Lab/Object824>。详见 `M5-REPORT.md`。

> 表里少一行 `o824`（bin）——它永远只是把参数转给 `o824-cli`，没有自己的里程碑。
> 另：`MIGRATION-RUST.md` 列了 16 个 crate，实际是 **19 个**：多了 `o824-version`（`week1/04`）、`o824-conformance`（`week1/03`，只含测试）和 `o824-yaml`（M1 加的，理由见 `M1-REPORT.md`）。这不是漂移，是两份文档的口径差，以仓库为准。


---

## 六、迁移完成后

1. Go 仓库打 `archived` 标记，README 首行指向 `o824-rs`。
2. `spec/`（golden + transcripts）以 Rust 仓库为准；Go 仓库只读。
3. 回到路线图：P1 剩余项（发现算法补层、形态适配、共存检测）在 Rust 侧直接做。
4. 第 12 周起可以做 P3（Tauri GUI）——**Rust 团队做这件事会比 Go 团队顺**，因为壳和内核同语言。
