# M3 报告：门 G3（执行、回滚、CLI 收口、实验室）

日期：2026-11-02 · 对应 `docs/MIGRATION-ROADMAP.md` 的 M3 与门 G3

## 一句话

**门 G3 过了**：真机实验室的六个场景（`f03`/`f01`/`f02`/`f16`/`f17`/`f20`）上，`doctor → repair --yes → doctor` 的退出码、输出文本、以及**树的语义状态**都与 Go 完全一致；`discover`/`doctor`/`repair --dry-run` 的逐字节对照继续全绿。

## 判据与证据

```
cargo fmt --all --check                       PASS
cargo clippy --all-targets -- -D warnings     PASS（0 warning）
cargo test --workspace                        PASS（209 个用例）
cargo build --release --bin o824              PASS
python scripts/lab-compare.py                 6/6 场景一致
python scripts/cross-check.py                 doctor 8/8 + plan 8/8 + 真机 3/3
```

实验室对照比较四类东西：**退出码**、**输出文本**、**app 树**、**home 树**。其中 YAML 文件按**解析后的文档**比较，不按字节——理由见下。

## 做了什么

### `o824-apply`：执行、回滚、journal

顺序不可换，每一步都是下一步的前提：**锁 → 计划过期 → 指纹复验 → 快照 → journal(applying) → 步骤 → 失败则回滚 → journal(done/failed)**。

- **junction 用原生 `FSCTL_SET_REPARSE_POINT`**，不用 `cmd /c mklink`：命令行会给 cmd.exe 一次重新解释 `&`/`%` 的机会（PORTING-NOTES §5）。缓冲区布局（`REPARSE_DATA_BUFFER` + substitute/print name）有单元测试逐字段钉住，另有真机测试建出 junction 并穿透读文件。
- **先建旁路、再换名**：新 junction 在 `<path>.o824-new-<n>` 建好、`FSCTL` 成功后才删旧的、改名过去。建不出来时旧链接原封不动。
- **回滚要能"删除它创建的东西"**：快照只能还原已存在的文件，所以执行前记录每个步骤可能创建的路径是否存在；回滚时删掉新建的、还原被替换的链接（按记录的 target 重建，包括悬空链接）。
- journal 在第一次写之前落盘；失败时写 `failed` + `error`，只有回滚自己也失败才写 `rollbackError`——**回滚的失败绝不替换掉本该报告的那个错误**。

### `allow-builds`：行级改写 pnpm 白名单

M0 的 spike 结论在这里兑现：不重新生成整棵树，只改目标行。三种形状都处理：已有 map 只加键；`allowBuilds:` 空值当空 map；**v11 之前序列在就地迁移成 map**（并记进 journal 的 `detail`）。显式 `false` 被翻成 `true`（那正是要修的故障）。

> 与 Go 的差异：Go 用 yaml.v3 的 Node 树重新输出，Rust 行级改写。**同一份文档，字节可能不同，语义相同**——这正是实验室脚本按解析结果比较 YAML 的原因。选择理由见 `docs/week1/yaml-spike-result.md`。

### `o824-cli`：repair 真执行 + snapshot + rollback

- `repair` 非 `--dry-run` 现在真的执行：非交互会话没有 `--yes` 就 `E_CONFIRMATION_REQUIRED`（退出码 3）且一个字节都不写；执行完**重新诊断**，还有 finding 就退出 1（"执行完的摘要"不算证据）。
- `snapshot create|list`、`rollback --latest|--to <id>` 落地，都走同一把机器锁 + journal。

## 实验室抓出来的两个真 bug

这两个都是**只在真机上才暴露**的、判据没覆盖到的差异：

1. **`rel_path` 语义不对**。它只处理"在 root 之下"，而 Go 的 `filepath.Rel` 会算出带 `..` 的相对路径。F01 的链接目标在 DSH home 旁边时，Go 报 `target=..\vendor\dsh-base-empty`，Rust 报绝对路径。已按 `filepath.Rel` 重写（公共前缀 + `..` + 余下部分）。
2. **`--root`/`--home` 没有归一化**。实验室传的路径含 `..`（`...\Object 824\..\broken-dsh-lab\f0x\home`），Go 的 `filepath.Join` 会清理，Rust 的 `Path::join` 不会——于是同一条链接被算成 `..\..\..\..\..\broken-dsh-lab\...`。现在在选择安装时统一 `clean`。

**这两条都说明：golden 覆盖不到的地方，实验室才是判据。** 它们不会被 `cargo test` 抓到，只会被真机抓到。

## 明确的未做（都有清晰的失败方式，不是静默的）

1. **`--with-install` / `reinstall-profile`**：需要在副本里跑宿主自带的 pnpm、校验后两次改名交换。这一步要几分钟和几百 MB，且会执行包自己的构建脚本。现在 `repair --with-install` 直接返回 `E_UNSUPPORTED` 并说明原因，`apply` 遇到该步骤也拒绝。
2. **`dedupe-loader` 步骤**：没有任何 P0 finding 会产生它（F03 是 advisory），所以内核拒绝执行而不是带一个没人走的分支。
3. `diagnose` / `mock` / `protocol` 三个 crate 仍是空壳——那是 M4/M5。

## 下一步

**M4**：`protocol`（LSP 帧、JSON-RPC、异步作业、plan/apply、发现缓存）与 `mock`（transcript 回放）。门 G4 是 `spec/transcripts/` 里 3 个 transcript **逐帧**通过。
