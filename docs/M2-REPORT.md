# M2 报告：门 G2（8 个 `.plan.json` 全绿）

日期：2026-11-02 · 对应 `docs/MIGRATION-ROADMAP.md` 的 M2 与门 G2

## 一句话

**门 G2 过了**：八棵 fixture 树上 `repair --dry-run --json` 的输出与 `spec/golden/*.plan.json` 逐字节相同，且与 Go oracle 三向一致（Go vs golden、Rust vs golden、Go vs Rust）。计划是纯函数，所以还额外钉住了**确定性**：同一棵树、同一时刻，两次构建出的计划完全相同。

## 判据与证据

```
cargo fmt --all --check                                  PASS
cargo clippy --all-targets -- -D warnings                PASS（0 warning）
cargo test --workspace                                   PASS（181 个用例）
cargo build --release --bin o824                         PASS
python scripts/cross-check.py                            doctor 8/8 + plan 8/8 + 真机 3/3
```

## 做了什么

### `o824-model::time`：RFC 3339 时间类型

计划和快照文档里的 `createdAt` / `expiresAt` 是 Go `time.Time` 的 JSON 拼写：**纳秒精度、尾部零裁掉、零小数就不写小数点**。这个拼写是契约的一部分，所以自己写了个 30 行的 UTC 时间类型（civil-from-days + 反向），不引日历库。`2026-11-01T12:10:41.5123456Z` 与 `2026-11-01T12:10:41Z` 两个拼写都有测试钉住。

### `o824-snapshot`：快照存储

`create` / `create_if_changed` / `list` / `restore`，布局与 Go 一致（`<dataRoot>\snapshots\<installID>\<ID>\{files\,meta.json}`）：

- 只捕获 §16.1 允许修复触碰的文件（每个 profile 的 5 个文件 + 安装根的 `version`），**绝不**碰 `node_modules`；
- 每个副本都对着源算 sha256，`meta.json` **最后**写，所以中途崩掉留下的目录不会被 `list` 当成快照；
- 保留最新 5 份；`create_if_changed` 用同一套文件清单和哈希集合比较，没有变化就不写目录；
- `restore` 先验快照内的字节、写完再验目标字节，任何一步对不上都是 `E_STATE_DRIFT`；快照记录的 scope root 与调用方给的 root 不一致时**拒绝**，而不是往应用目录里写 `profiles\`。

### `o824-plan`：纯函数计划

步骤映射按 P0 目录的"可修复性"一列：F01→`rebuild-link`（且目标必须仍是个带 `package.json` 的目录）、F04→`rebuild-bridge`（且 profile manifest 必须指名那个包）、F16→`restore-snapshot`（且必须有快照）、F17→`allow-builds`；F02/F03/F20 与其余全部**不产生步骤**。

计划 id 是 `plan-<UTC 时间>-<哈希前 8 位>`，指纹是"这一步会改写的每个文件的 rel → sha256"。`test_a_blocked_build_gets_one_allow_builds_step_and_a_fingerprint` 直接断言指纹等于 golden 里那个 `e7987c11…`——一条测试同时钉住了 fixture 字节、哈希算法和路径拼写。

### `o824-cli`：`repair --dry-run`

`repair --root/--home/--profile/--install [--dry-run] [--json] [--with-install]`。`--dry-run` 打印计划（人话或 JSON）；不带 `--dry-run` 时明确返回 `E_UNSUPPORTED`（退出码 8）并提示"执行在 M3"，**不会**假装修好了什么。安装选择逻辑抽到了 `cli::install::select`，`discover`/`doctor`/`repair` 共用，三者对 `--root`/`--home`/`--install` 的含义不会再分叉。

## 一个差点踩进去的坑：**oracle 二进制比源码旧**

第一次跑 plan 的三向对照时，Go 和 Rust 全部 FAIL，而 Go 的输出是 `"steps": null` —— 和 golden 的 `[]` 相反。原因不是实现错了：仓库里那个 `o824.exe` 是 2026-11-01 20:41 构建的，而 `internal/plan/plan.go:416` 那段"nil slice 归一成 `[]`"的修复比它新。**用当前源码重建 oracle 之后，8/8 全过。**

> 教训写进 `scripts/cross-check.py` 的文档里：对照之前先重建 oracle。判据是 golden，不是那个 exe。

## 未完成

1. **执行**（`apply`）还没做，所以 `repair` 不带 `--dry-run` 时返回 `E_UNSUPPORTED`。这是 M3 的主体。
2. `snapshot` 子命令（create/list/restore 的命令行）还没做；快照 crate 本身可用。
3. CI 仍未在 GitHub 上跑过。

## 下一步

**M3**：`apply`（快照 → journal → 步骤 → 失败回滚 → 链接回放）、`--with-install` 的副本安装与目录交换、以及 `cli` 收口（六个子命令、退出码 0–8、`--json` 纯净）。门 G3 是三件事：实验室七种故障、`discover`/`doctor --json`/`repair --dry-run --json` 与 Go 逐字节一致（前两项已达成）、以及最终的真机验收。
