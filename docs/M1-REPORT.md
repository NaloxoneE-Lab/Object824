# M1 报告：门 G1（8 个 `.doctor.json` 全绿）

日期：2026-11-02 · 对应 `docs/MIGRATION-ROADMAP.md` 的 M1 与门 G1

## 一句话

**门 G1 过了**：八棵 fixture 树上 `doctor --json` 的输出与 `spec/golden/*.doctor.json` 逐字节相同，而且**被测真机交叉验证过**——同一棵树上 Go oracle、Rust 实现、golden 三者一致。M1 清单里 `execx`/`lock` 和 `discovery` 的机器扫描仍未做，但它们不是 G1 的判据。

## 判据与证据

```
cargo fmt --all --check                                  PASS
cargo clippy --all-targets -- -D warnings                PASS（0 warning）
cargo test --workspace                                   PASS（109 个用例）
cargo build --release --bin o824                         PASS
```

三条独立的 golden 证据：

| # | 证据 | 结果 |
|---|---|---|
| 1 | `cargo test -p o824-conformance --test doctor_golden` | 8/8 kind 与 golden 逐字节相同 |
| 2 | 同一棵树上 `Go o824.exe doctor --json` vs `Rust o824.exe doctor --json` | 8/8 kind 完全一致 |
| 3 | **Rust 造的树**交给 **Go oracle** 跑，再与 golden 比 | 8/8 kind 一致 —— 这同时证明 fixture builder 本身是忠实的 |

第 3 条是这次加的：golden 原本是从 Go 自己的 fixture 生成的，如果 Rust 的 fixture builder 造出了另一棵树，"Rust 对 golden"就只是自说自话。把 Rust 造的树交给 oracle，才算把这条链接上。

复跑方式（`o824-rs/` 下）：

```bash
cargo build --release --bin o824
python scripts/cross-check.py
```

脚本会现场造八棵树、两边各跑一遍、三向比对；任一项不一致就打印三份文档并退出 1。

真机上（`D:\Users\X_Cyanogen\AppData\Local\Programs\DeepSeek Harness` + `C:\Users\X_Cyanogen\.dsh`）两者输出也完全一致：`no findings`，退出码 0。

## 做了什么

### 新增 crate：`o824-yaml`

**为什么值得单独一个 crate**：F16 的 `params.line` 是契约的一部分，而它来自参考实现的 `gopkg.in/yaml.v3`——那是 libyaml 的 Go 移植。任何"另一个 YAML 库"都会给出不同的行号，甚至不同的接受/拒绝判断：

| 输入 | `yaml.v3`（Go） | `yaml-rust2` | `unsafe-libyaml`（本 crate） |
|---|---|---|---|
| `- id: dsh-base\n  enabled: [true, false\n` | `line 1` | line 4 | **line 1** ✓ |
| `x: 1\ny: [1,\n 2\n` | `line 1` | line 4 | **line 1** ✓ |
| `a: [1,\n 2,\n 3\n` | `line 3` | line 4 | **line 3** ✓ |
| `a: 1\n b: 2\n` | `line 2` | line 3 | **line 2** ✓ |
| `a:\n\t- 1\n`（Tab 缩进） | 拒绝 | **接受**（错） | **拒绝** ✓ |

`line 1` 对第 2 行那个未闭合的 `[` 看起来是错的，但它是 `yaml.v3` 的真实输出：它打印的是 `context_mark.line`（0 基、不加一），只有 scanner 错误才加一。**契约就是契约**，所以这里用 `unsafe-libyaml`（libyaml 的 C→Rust 转写）+ 复刻 `yaml.v3` 的 `fail()` 规则，换来逐字节一致，而不是一个"看起来更合理"的行号。

这个 crate 是整个工作区里唯一允许 `unsafe` 的地方（一行注释对应一处 `unsafe` 块），其余 18 个 crate 仍是 `#![forbid(unsafe_code)]`。

### `o824-verify`：七个检查

| 检查 | 实现文件 | 覆盖的 golden |
|---|---|---|
| F01/F02 链接断裂/缺失 | `link.rs` | broken-link、missing-bundle |
| F04 桥缺失 | `link.rs` | dangling-bridge |
| F03 重复 loader 条目（advisory） | `loader.rs` | dup-loader |
| F16 profile 文件不可解析 | `profile.rs` | parse-error |
| F17 pnpm 构建白名单 | `pnpm.rs` | git-dep |
| F20 未完成的安装 | `journal.rs` | half-install |

几个容易写错、这次刻意钉住的点：

- **F17 的 key 不是包名**：`dshmarket@git+https://github.com/deepseek-ai/dshmarket.git`（去掉 `#ref`，空格写 `%20`）。测试里专门有一条"裸包名永远批不下来"。
- **F03 没有 `repair`**：判据在 0.2.1-rc.3 上不成立（PORTING-NOTES §1），所以它是 `warning` 且不驱动任何自动修改。测试断言 `repair.is_none()`。
- **`..` 要清理**：`link:../../vendor/plugins/x` 拼出来的路径含 `..`，Go 的 `filepath.Join` 会清理，Rust 的 `Path::join` 不会——不清理的话 dangling-bridge 的 `path` 参数就和 golden 差一截。
- **junction 判定**：用 `MetadataExt::file_attributes()` 的 `FILE_ATTRIBUTE_REPARSE_POINT` 位，不靠 `is_symlink()`（junction 同时是目录和 reparse point）。
- **params 顺序**：Go 的 map 序列化按 key 排序，而 `serde_json` 保留插入顺序，所以参数按 key 排序后再插入（`verify::params`），否则 golden 立刻红。

### 就地修掉的工具链缺口

`tempfile` 3.27 依赖 `getrandom` 0.4，后者的 Windows 支持用 `raw-dylib` 链接，`windows-gnu` 宿主会去找 `dlltool`——本机没有（链接用的是工具链自带的 `rust-lld`，没有 MinGW）。为 20 行功能装回 600 MB MinGW 不划算，于是**把 `tempfile` 从依赖里去掉**，在 `o824-fixtures` 里实现了一个 `TempDir`（`{prefix}{pid}-{seq}-{nanos}`，Drop 删除）。现在整个工作区没有需要 C 工具链的依赖。

> 记一笔：将来若有依赖真的需要 `raw-dylib`（例如某些 `windows-sys` API），要么补一个 `dlltool`，要么改走 MSVC 宿主。M3 之前值得先确认一次。

### `o824-cli`：`doctor` 子命令

`o824 doctor --root <p> [--home <p>] [--profile <n>] [--json]`，退出码 0（干净）/1（有 finding）/2（命令行错）。人话输出与 Go 逐字相同（`no findings`、`F17  error  E_PNPM_BUILD_BLOCKED  key=… package=…`）。

为了让测试能指定 data root（进程级环境变量在并行测试里会打架），`doctor::run_with(..., data_root)` 是给测试用的入口，`doctor::run` 走环境变量。

## M1 的后半段：机器层

上面是门 G1 的部分。G1 过完之后补齐了 M1 清单里剩下的四个 crate，以及让它们能被验证的 `discover` 子命令。

### `o824-execx`：唯一的子进程出口

只要 `program + argv`，不要命令字符串：路径里有空格、引号、非 ASCII 都原样交给子进程。**非零退出是数据不是错误**（`code` 有值、`error` 为 `None`），stdout/stderr 分两个缓冲返回，不合并。子进程用 `CREATE_NO_WINDOW` 启动，GUI 进程里不会闪黑框。

测试里用**测试二进制自己**当子进程，这一条同时验证了"程序路径含空格"（本仓库路径就有）和"参数含空格与非 ASCII"（`--skip "no such test 测试"` 若被拆开，harness 会以非零退出）。

### `o824-lock`：跨进程独占锁

`File::try_lock`（标准库，1.89 起稳定）→ `Err(WouldBlock)` 时返回 `E_BUSY`，并带上 `Params["pid"]` 指向持锁进程。pid 写在 `o824.lock.pid` 旁边（锁文件本身被持有期间读不了）。

有一条**两进程测试**：父测试起一个子进程（同一个测试二进制，`--exact tests::lock_holder_child`）持锁并打一行 `locked`，父进程读到之后自己 `acquire`，断言 `E_BUSY` 且 `pid` 等于子进程 pid。

> **与 Go 的一处差异（已记录在 crate 文档里）**：Go 用 `CreateFileW` 的 share mode 0，连"同一进程第二次打开"都拒绝；`LockFileEx` 只拒绝另一个进程的锁。跨进程语义一致（契约要的就是这个），差别只出现在同一进程同时取两次锁——内核不会这么做。换来的是零 `unsafe`、不引 `windows` API。

### `o824-state`：原子状态文件

`data_root` 的规则与 Go 一致（`O824_DATA_ROOT` → `%LOCALAPPDATA%\o824` → `<home>\o824`），state.json 的写入走"同目录临时文件 → `sync_all` → `fs::rename` 覆盖"。读的时候把 Go 写的 `"installs": null` 当空表（Go 的 nil map 值），坏 JSON 报 `E_STATE_DRIFT` 而不是静默当空。

### `o824-discovery`：机器扫描

补上注册表（HKCU → HKLM → WOW6432Node，`DisplayName` 以 `DeepSeek Harness` 开头就算）、固定卷（`GetLogicalDrives` + `GetDriveTypeW`）、realpath 去重、排序后编号 `i1…iN`。

这是**整个工作区第二次使用 `unsafe`**（第一次是 `o824-yaml` 的 libyaml 包装），而且是紧挨着 Windows API 的 `#[cfg(windows)]` 块；crate 从 `#![forbid(unsafe_code)]` 改成 `#![deny(unsafe_code)]`，只给 `machine` 模块 `#[allow(unsafe_code)]`。`windows` crate 用的是 0.58——它的 `windows-targets` 还带预编译 import lib，不会像 `windows-sys` 0.61 那样要 `dlltool`。

### `o824 discover`（顺带，因为验收要用）

`discover [--json] [--root] [--home] [--install]`，行为与 Go 逐字一致，连表格的列宽（每列最宽 + 2 空格）都一样。

**真机结果**（本机）：

```
$ o824 discover
id  kind     version     root                                                         health
i1  desktop  0.2.1-rc.3  D:\Users\X_Cyanogen\AppData\Local\Programs\DeepSeek Harness  ok
```

`discover --json`、`discover`（表格）、`doctor --install i1 --json` 三者与 Go **逐字节相同**，`doctor --install nope` 的报错文本与退出码 2 也相同。这些都进了 `scripts/cross-check.py` 的 "real machine" 段。

## 仍然未完成

1. **CI 仍未在 GitHub 上跑过**（仓库还没有远端）。这是唯一一条从 M0 悬到现在的 G0 判据。
2. `MIGRATION-RUST.md` 要求的 `discovery` 验收是"`discover --json` 对真机输出与 Go 版相同"——这条**已经满足并被脚本钉住**；但它只在有真安装的机器上能跑，CI 上会 SKIP。
3. MSRV 从 1.80 提到 **1.89**（`File::try_lock`）。`week1/01` 的示例片段里写的是 1.80，以仓库为准。

## 下一步

M1 做完了。按路线图进 **M2**：`plan`（纯函数、15 分钟有效期、指纹、步骤映射）与 `snapshot`（create/list/restore、保留 5 份、sha256 校验）。门 G2 是 8 个 `.plan.json` 全绿。

**G1 已过**，所以"`verify` 的语义被完整复现"这句话现在是有机器判据的——这才是能开始 `plan` 的前提。
