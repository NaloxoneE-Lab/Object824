# M5 报告：门 G5（诊断包、发布构建、全流程验收）

日期：2026-11-02 · 对应 `docs/MIGRATION-ROADMAP.md` 的 M5 与门 G5

## 一句话

**门 G5 过了**：`diagnose` 落地、发布构建带版本注入、六加一个子命令在真机上全流程跑通，G0–G4 的判据在最终版本上重新验证过一次。`v0.1.0-rs` 已打 tag。

## 判据与证据

```
cargo fmt --all --check                       PASS
cargo clippy --all-targets -- -D warnings     PASS（0 warning）
cargo test --workspace                        PASS（248 个用例）
pwsh -File scripts/release.ps1 -Version 0.1.0  PASS  → o824 0.1.0 (commit …, built …) windows/amd64
python scripts/cross-check.py                 doctor 8/8 · plan 8/8 · transcripts 3/3 · 真机 3/3
python scripts/lab-compare.py                 6/6 场景一致
```

## 做了什么

### `o824-diagnose`：诊断包

一个 zip，内容**按白名单**取，不遍历安装树：

| 条目 | 内容 |
|---|---|
| `manifest.json` | 时间、工具版本、安装指纹、`redacted` 标记、条目清单 |
| `version.json` | 版本、commit、构建时间、工具链、平台 |
| `system.json` | OS 版本与 build、架构、data root、剩余空间 |
| `doctor.json` | 完整的 findings 信封 |
| `profile/<名>/<文件>` | 每个 profile 的 5 个设置文件 |
| `journal/<文件>.json` | 最新 10 个 journal |

**默认脱敏**：安装根、DSH home、`%USERPROFILE%` 分别换成 `<ROOT>`、`<HOME>`、`<USERPROFILE>`，并且同时替换**原始拼写、正斜杠拼写、JSON 转义拼写**——JSON 文档里的路径是双反斜杠，只替换一种会漏。替换按长度从长到短排序，否则 `%USERPROFILE%` 会把安装根的前缀先啃掉。

`--include-paths` 关掉脱敏，并把 `redacted: false` 写进 manifest——维护者需要真实布局时用，但**这个选择会被记录**。

实测：真机树上生成的包，zip 原文里搜不到 `X_Cyanogen` / `broken-dsh-lab`。

### 发布构建

`scripts/release.ps1` 把 `O824_VERSION` / `O824_COMMIT` / `O824_BUILD_TIME` / `O824_RUSTC_VERSION` 注入后构建——和 Go 的 `-ldflags -X` 同一个位置，只是 Rust 侧用 `option_env!` 在编译期读：

```
o824 0.1.0 (commit f4c7751, built 2026-11-01T19:59:48Z) windows/amd64
```

## 真机全流程（七个入口）

| 入口 | 结果 |
|---|---|
| `discover`（JSON 与表格） | 与 Go **逐字节相同** |
| `doctor --install i1 --json` | 与 Go 逐字节相同 |
| `repair --install i1 --dry-run --json` | 归一化后与 Go 相同 |
| `snapshot create` / `list` | 5 个设置文件入快照，无 node_modules |
| `rollback --latest` | 复原字节；先验快照内 sha256，再验写回后的 |
| `diagnose --out` | 9 个条目，脱敏通过 |
| `mock --transcript` | 3 个录制逐帧一致（见 G4） |

**一个过程中发现并修掉的真 bug**：`rollback` 把 `install.root`（应用目录）当成恢复目标，而快照覆盖的是 DSH home。快照的 `restore` 会拒绝"两边都已知且不一致"的请求（这是 C1 的护栏），所以真机上是退出 5 而不是写错地方——护栏挡住了它，但 CLI 传错了参数。修成 `installScope`（home 优先），实验室树上复验通过。

## 迁移完成的定义（对照路线图 §一）

| # | 定义 | 状态 |
|---|---|---|
| 1 | 行为等价：8 doctor + 8 plan golden 全绿 | ✅ |
| 2 | 协议等价：3 个 transcript 逐帧通过 | ✅ |
| 3 | 真机等价：实验室故障 `doctor → repair → doctor` 一致 | ✅ 6/6 场景（实验室提供的全部场景） |
| 4 | 可发布：单 exe，干净 Windows 上六个子命令 | ✅ 本机实测；**"干净 Windows"未验**（见下） |
| 5 | 可维护：CI 全绿且真在 GitHub 跑过 | ⚠️ **CI 从未在 GitHub 上跑过**（仓库没有远端） |

## 补记（v0.1.1）：`--with-install` 已实现并真机验证

原先拒绝的 `reinstall-profile` 现在真的执行，判据由 `scripts/with-install-check.ps1` 钉住（真实 app + 临时 home，跑宿主自带的 pnpm）：

```
doctor reports F01                 PASS
the plan is one reinstall step     PASS
the repair applies                 PASS
the profile diagnoses clean        PASS
a dependency tree was swapped in   PASS   (84 top-level packages)
no staging copy is left behind     PASS
no old tree is left behind         PASS
the journal records done           PASS
the journal records the step note  PASS
```

实现按 Go 的顺序：**空间预检 → 复制 profile 到 `<profile>.o824-stage`（跳过 node_modules 和宿主的点目录）→ 用宿主 exe 以 `ELECTRON_RUN_AS_NODE` 跑它自带的 pnpm `install --frozen-lockfile` → 要求副本里真的出现非空 `node_modules` → 两次改名交换 → 清理**。任一步失败都在交换之前，真实 profile 一个字节都没动；交换后的清理失败只报告，不回滚（为了收拾干净而重新弄坏 profile 是错的取舍）。

两个实现细节值得记：
- 子进程有 **15 分钟**上限（`execx` 新增了带 deadline 的 runner：边读管道边等，超时就杀，避免 pnpm 挂死修复流程）；
- `--frozen-lockfile` **只在副本里真有 lockfile 时**才加（新 profile 没有 lockfile 是合法的，pnpm 会拒绝这个flag）。

## 明确未做

1. **CI 没有远端可跑**。`.github/workflows/ci.yml` 就位，但仓库至今没有 GitHub 远端。这是唯一一条从 G0 悬到现在的判据。
3. **"干净 Windows"冒烟**：本机就是唯一验证过的机器（Windows，无 MSVC，`windows-gnu` 宿主）。
4. **P1 剩余项**（发现算法补层、形态适配、共存检测）：按路线图，迁移完成之后才做。

**这四条都不影响 G0–G4 的机器判据**，但第 1 条是路线图写明的"迁移完成"定义之一，所以这里如实标成未完成，而不是用"CI 文件写好了"糊过去。
