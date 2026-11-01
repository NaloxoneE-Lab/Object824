# 01 · Workspace 骨架与 CI

目标：`cargo test` 能跑，CI 能跑，crate 之间的依赖方向从第一天就定死。

## 目录

```
o824-rs/
├── Cargo.toml               # workspace
├── rust-toolchain.toml
├── crates/
│   ├── o824/                # bin：一个 exe，多角色
│   ├── o824-cli/            # 子命令解析、退出码、人话渲染
│   ├── o824-protocol/       # LSP 帧 + JSON-RPC + 作业
│   ├── o824-mock/           # transcript 回放
│   ├── o824-apply/          # 执行 + 回滚
│   ├── o824-plan/           # 纯函数计划
│   ├── o824-snapshot/       # 快照
│   ├── o824-verify/         # 七个检查
│   ├── o824-fixtures/       # 合成安装树（仅 dev）
│   ├── o824-discovery/      # 注册表/卷/asar
│   ├── o824-diagnose/       # 诊断包
│   ├── o824-state/          # 原子状态文件
│   ├── o824-lock/           # Windows 独占锁
│   ├── o824-execx/          # argv 子进程
│   ├── o824-model/          # findings 信封
│   ├── o824-errs/           # 错误码
│   └── o824-conformance/    # 只含测试：golden + transcript 断言
└── spec/                    # golden + transcripts（从 Go 仓库复制）
```

## `Cargo.toml`（workspace）

```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
edition = "2021"
rust-version = "1.80"
license = "UNLICENSED"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["preserve_order"] }
thiserror = "1"
windows = { version = "0.58", features = [
  "Win32_Foundation",
  "Win32_Storage_FileSystem",
  "Win32_System_IO",
  "Win32_System_Registry",
  "Win32_System_Threading",
  "Win32_System_Diagnostics_Debug",
] }
sha2 = "0.10"
zip = { version = "2", default-features = false, features = ["deflate"] }
```

两个要点：

- `serde_json` 必须开 **`preserve_order`**。Go 的结构体字段顺序是契约的一部分（golden 是逐字节比的），而 `serde_json` 默认用 `BTreeMap` 会按字母序排 key，**golden 立刻全红**。
- `windows` crate 的 feature 列表按需增减，但 `Win32_Storage_FileSystem`（CreateFile、reparse point、GetDiskFreeSpaceEx）和 `Win32_System_Registry` 一定有。

## 依赖方向（与 Go 版一致，不许出现环）

```
o824 ─→ o824-cli ─┬→ o824-protocol ─→ o824-mock
                  ├→ o824-discovery ─→ o824-state, o824-lock
                  ├→ o824-verify
                  ├→ o824-plan
                  ├→ o824-apply ─→ o824-snapshot, o824-execx
                  └→ o824-diagnose
       全部 ─→ o824-model, o824-errs
```

**硬规则**：

- `o824-verify` 不许依赖 `o824-plan`（检查不知道修复存在）。
- `o824-plan` 不许依赖 `o824-apply`（计划不知道执行存在）。
- `o824-apply` 不许依赖 `o824-cli`（内核不知道人话存在）。
- `o824-model` / `o824-errs` 不许依赖任何内部 crate。

CI 加一条检查依赖方向的方式：在 `o824-conformance` 里读各 crate 的 `Cargo.toml`，断言上面这几条禁止边不存在（简单粗暴但有效）。

## `rust-toolchain.toml`

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

## CI（`.github/workflows/ci.yml`）

```yaml
name: ci
on: [push, pull_request]

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

jobs:
  windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test --workspace --all-features
      - name: Golden conformance
        run: cargo test -p o824-conformance -- --nocapture
      - run: cargo build --release --bin o824
```

## 完成判据

- [ ] `cargo build --workspace` 通过（哪怕 crate 里只有 `pub fn placeholder()`）。
- [ ] `cargo clippy --all-targets -- -D warnings` 干净。
- [ ] CI 在 GitHub 上真跑过一次（这是 Go 版留下的债，别在新仓库重演）。
- [ ] 依赖方向的断言测试存在且通过。
