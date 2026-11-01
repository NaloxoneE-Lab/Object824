# M0 完成报告（门 G0）

日期：2026-11-02 · 对应 `docs/MIGRATION-ROADMAP.md` 的 M0 与 `docs/week1/00-overview.md`

## 一句话

M0 的五个子任务全部做完，`cargo fmt + clippy + test + build --release` 全绿；**唯一没做到的是"CI 在 GitHub 上真跑过一次"**——仓库还没有远端，这一条需要推上去之后才算，见文末"未完成"。

## 门 G0 判据与证据

命令都在 `o824-rs/` 下执行，先 `source scripts/env.sh`（或 `. .\scripts\env.ps1`）。

| # | 判据 | 命令 | 结果 |
|---|---|---|---|
| 1 | 格式 | `cargo fmt --all --check` | 通过（无 diff） |
| 2 | 静态检查 | `cargo clippy --all-targets -- -D warnings` | 通过（0 warning） |
| 3 | 测试 | `cargo test --workspace` | **50 passed / 0 failed**，退出码 0 |
| 4 | 发布构建 | `cargo build --release --bin o824` | 通过，产出 `target/release/o824.exe` |

## 五个子任务

### 1 · workspace + CI

- 18 个 crate（`docs/MIGRATION-RUST.md` 说的 16 个，加上 `o824-version` 与 `o824-conformance`，理由见下），全部能构建。
- `rust-toolchain.toml` 固定 stable + rustfmt + clippy。
- `.github/workflows/ci.yml` 按 `week1/01` 原样写好：fmt → clippy → test → golden → release build。
- **依赖方向**由测试钉住，不是靠人记：`crates/o824-conformance/tests/dependencies.rs` 读每个 crate 的 `Cargo.toml`，禁止 `verify → plan`、`plan → apply`、`apply → cli`、`model → plan`，并要求 `errs` 不依赖任何内部 crate。

> **规格勘误**：`week1/01` 要求"`o824-model` / `o824-errs` 都不许依赖任何内部 crate"。这条在 Rust 侧做不到：`Finding` 里带着 `errs::Code`，所以 `model → errs` 是真实边（Go 版同样如此：`internal/model` import `internal/errs`）。测试改成"`errs` 零内部依赖、`model` 只能依赖 `errs`"，并在测试注释里写明原因。**这是修正规格，不是放宽规格**——除这一条外其余全部照原样执行。

### 2 · YAML spike

结论：**行级编辑**。三个候选的真实结果与证据见 [`week1/yaml-spike-result.md`](week1/yaml-spike-result.md)，要点：

- `marked-yaml` 0.8.0 连解析都过不了（真实文件顶层是序列，它要求 mapping），而且**没有 emitter**；
- `yaml-rust2` 的 `YamlEmitter` 能输出但把 3 条注释全部吃掉、1545 字节变 1331 字节；
- 行级编辑：原样写回逐字节相同；删 `opencode-go` 条目 `-9` 行且无任何 `+` 行；加 `allowBuilds` 键只多一行。

spike 代码留在 `spikes/yaml-spike/`（独立 workspace，`exclude` 出主图），可用 `cargo run --release` 复跑。

### 3 · golden harness

`cargo test -p o824-conformance` 读 `spec/golden/` 的 16 个文件，做到：

- 16 个文件都能解析成 `serde_json::Value`，并断言 key 顺序（`envelope/findings`、`id/…/fingerprint`）；
- 归一化按规格替换 `<ROOT>` / `<DATA>` / `<PLAN_ID>` / `<INSTALL_ID>` / `<TIMESTAMP>`，路径的**原生、JSON 转义、正斜杠**三种写法都覆盖；
- 失败时打印**带上下文行的 unified diff**，不是一句 "mismatch"；
- 有一条**故意改坏 `severity` 必须失败**的测试，断言 diff 里同时出现 `- "severity": "error",` 与 `+ "severity": "info",`；
- 重生成开关 `O824_UPDATE_GOLDEN=1` 已实现（`assert_golden_file`），并且有测试钉住"只在明确要求时才改写契约文件，否则差异必须失败"。

一个在写 harness 时才发现的契约事实：**8 个 `doctor` golden 里一个绝对路径都没有**（finding 的 `params.path` 是 profile 相对路径），只有 8 个 `plan` golden 需要归一化。测试把这一点钉成 `with_placeholders == 8`。

### 4 · `errs` / `model` / `version`

- `errs`：23 个错误码的字符串逐个断言（含 `E_MOCK_FAILURE`，它只出现在协议层但规格列了它）；未知码反序列化报错而不是被吞掉。
- `model`：`Envelope` / `Finding` / `Repair` / `Install` 的字段名与顺序逐条断言；空 `params`/`detail`/`repair` 不出现；`findings` 永远是数组。
- `version`：`o824 <version>[ (commit …, built …)] <os>/<arch>`，零注入时没有空括号；架构拼写按参考实现的 `amd64`/`arm64` 映射（Rust 说 `x86_64`）。
- **真实 golden 往返**：8 个 `.doctor.json` 反序列化成 `Envelope` 再序列化，**逐字节相同**——这条同时验了字段顺序与 `preserve_order`。
- 退出码映射在 `o824-cli` 里钉住：`E_USAGE`/`E_INSTALL_DIR → 2`、`E_CONFIRMATION_REQUIRED → 3`、`E_PERMISSION → 4`、`E_STATE_DRIFT → 5`、`E_NETWORK → 6`、`E_BUSY → 7`、其余 `8`。

### 5 · 第一棵 `ok` 树

- `o824-fixtures` 能造**全部八种** kind（不是只有 `ok`），`ok` 树被判 `health == "ok"`、`version == "0.2.1-rc.3"`。
- 树里 `<root>\version` 故意写 Electron 的 `44.7.0`，测试断言 `validate` 报的是 asar 里的 `0.2.1-rc.3`——"从 version 文件读版本"这种错实现过不了。
- `node_modules` 条目是**真链接**（symlink，失败退 junction），测试用 `canonicalize` 断言它确实指向 `vendor/`，不是复制出来的目录。
- asar 按 `week1/05` 的字节格式写，读端走的是 `discovery::parse_asar_version` 的真实代码路径。

## 环境决策（会影响后面每个人）

**工具链只装在 `o824-rs/.toolchain/`，不写 `C:\`、不改 PATH、不改注册表。**

安装过程踩了两件事，都记下来免得重踩：

1. `rsproxy.cn` 在这次会话里 TCP 拒绝连接；`mirrors.tuna.tsinghua.edu.cn/rustup` 与 `mirrors.ustc.edu.cn/rust-static` 可用（实测 1 MB/s 级），本次用 TUNA。
2. 本机没有 Visual Studio 构建工具，只能走 `x86_64-pc-windows-gnu`。而 **MinGW 的 gcc 在路径含空格时必崩**（`ld` 把 `D:/dsh work/...` 拆成多个参数），本仓库路径正好带空格。解法不是搬家，而是用 Rust 工具链自带的 **`rust-lld`** 链接（`.cargo/config.toml` 里一行 `linker = "rust-lld"`）——不需要 MinGW、不需要 C 编译器，也顺带省掉 949 MB。验证过 `cargo build`/`test`/`build --release` 全部正常。

## 未完成 / 需要下一轮接手

1. **CI 没在 GitHub 上跑过**：`.github/workflows/ci.yml` 写好了，但仓库没有远端。这条按 `week1/01` 属于 G0 判据，**如实记为未完成**，不是"看起来完成"。
2. `cargo test` 的 50 个用例里，`verify`/`plan`/`apply`/`protocol`/`mock`/`diagnose`/`execx`/`lock`/`state` 还是空壳，只有 `MILESTONE` 常量。
3. `discovery` 只实现了 `validate` / asar 版本 / home 解析 / profile 列举；注册表扫描、固定卷枚举、realpath 去重（M1）。
4. `cli` 只有 `--version` / `--help` 与退出码映射；六子命令在 M3。

## 下一步

M1 的第一件事是 `verify`：**先让 8 个 `.doctor.json` 全绿**，再谈别的。`fixtures` 的八棵树已经就位，`verify` 可以直接吃它们。
