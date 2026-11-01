# 第一周：目标与完成定义

这一周**不写业务逻辑**。目标是把"验收管线"立起来，让后面每一行 Rust 都有机器判据兜底。

## 本周四条产出

| # | 产出 | 完成的判据（可执行） |
|---|---|---|
| 1 | workspace 能构建 | `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && cargo build --release` 全绿 |
| 2 | golden harness 能用 | `cargo test -p o824-conformance` 读到 `spec/golden/` 的 16 个文件；故意改坏一个字段时测试失败并打印 diff |
| 3 | YAML spike 有结论 | `docs/week1/yaml-spike-result.md` 写下了"用哪个 crate、为什么、注释与顺序保不保得住" |
| 4 | 第一棵树能造 | `cargo test -p o824-fixtures` 造出 `ok` 树，`o824-discovery::validate` 判它 `health == "ok"`、`version == "0.2.1-rc.3"` |

四条都满足之前，**不要开始 `verify`**。第 2 条尤其重要：没有它，后面的移植只能靠"看起来对"。

## 为什么先做管线而不是先写代码

Go 版有 13.6k 行测试，但那些测试**测的是 Go 的实现**。Rust 版要证明的不是"我们写得像 Go"，而是"同一棵 fixture 树上，两个实现吐出同一份 JSON"。所以第一周要先把"同一份 JSON"这件事变成可执行的断言，之后每个 crate 才有真正的完成标准。

## 顺序建议

```
第 1 天   01-workspace.md    搭骨架 + CI
第 2-3 天 02-yaml-spike.md   YAML 选型（结论会改变 verify/plan/apply 的写法）
第 3-4 天 03-golden-harness.md  验收管线
第 4-5 天 04-first-crates.md    errs / model / version
第 5 天   05-fixtures.md        第一棵 ok 树 + asar
```

## 本周明确不做

- 不写 `verify` / `plan` / `apply` 的任何业务逻辑（第 2 周开始）。
- 不碰真机（`broken-dsh-lab` 的七种故障留到第 8 阶段）。
- 不做协议层（第 10 阶段）。
- 不追求"比 Go 更优雅"的重构。**这一周的唯一标准是等价**；等 golden 全过之后，再谈改进。
