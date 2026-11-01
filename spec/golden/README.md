# spec/golden — 跨实现一致性契约

这个目录是 O824 内核的**跨实现契约**：任何实现（Go、Rust，或其他语言）只要在同样的 fixture 树上运行，就必须产出与这里逐字节相同（归一化之后）的 JSON。换句话说，**移植实现跑通这组文件，就算与参考实现一致**。

每个 fixture kind 两个文件：

| 文件 | 内容 |
| --- | --- |
| `<kind>.doctor.json` | `o824 doctor --json` 对这个树输出的 findings 信封（`dsh-doctor/v1`） |
| `<kind>.plan.json` | `o824 repair --dry-run --json` 对这个树输出的修复计划 |

覆盖的 kind 与 `internal/fixtures` 一致：`ok`、`broken-link`、`missing-bundle`、`dup-loader`、`parse-error`、`half-install`、`dangling-bridge`、`git-dep`。

## 运行

```bat
go test ./internal/cli -run TestGolden -count=1
```

测试在进程内用 `fixtures.Build` 现场建树，再通过 `cli.Run` 分别跑 `doctor --root <root> --home <root> --json` 和 `repair --root <root> --home <root> --dry-run --json`，归一化后与这里的文件比较。每个 kind 会在两棵各自新建的树上跑两遍：两遍之间只要有一个绝对路径、时间戳或随机 id 漏过归一化，测试就会以“两次运行不一致”失败，而不会变成偶发。失败时打印 expected/actual 的 unified diff。

`half-install` 的未完成 journal 位于安装树里，而 doctor 从 data root 读取 journal，所以测试把该 journal 复制进一个临时 data root，并用 `O824_DATA_ROOT` 指过去。

## 重新生成

```bat
go test ./internal/cli -run TestGolden -update
```

`-update` 会用当前内核的输出重写这些文件（LF、两空格缩进、末尾一个换行）。只有在确认行为变更是**有意的契约变更**时才重新生成，并在提交说明里写清改了哪条契约——否则你只是把回归固化成新的期望值。

## 归一化规则

JSON 里只有下面这些值被替换成占位符，其余字节（包括 key 顺序，即 Go 结构体字段顺序）原样保留：

| 占位符 | 被替换的值 |
| --- | --- |
| `<ROOT>` | fixture 的绝对根路径（fixture 的 DSH home 与它相同），作为剩余路径的前缀出现时也只替换前缀，路径后缀保留 |
| `<DATA>` | data root 的绝对路径（`O824_DATA_ROOT`），同样只替换前缀 |
| `<PLAN_ID>` | 计划的 `id`（含生成时刻的摘要） |
| `<TIMESTAMP>` | `createdAt` 与 `expiresAt` |
| `<INSTALL_ID>` | `--root` 推导出的 `local-<hash>` 安装 id：它是绝对根路径的哈希，本身就不可能机器无关；显式 `--install` 传入的 id 不会被替换 |

`seq`、`params`、`repair`、`fingerprint` 以及任何 finding 字段都**不做**归一化：它们正是移植实现必须复现的部分。`steps` 为空时 Go 输出 `"steps": null`，这也属于契约，不要“顺手修正”为 `[]`。

退出码不在文件里：`doctor` 有 finding 退出 1、干净退出 0，`repair --dry-run` 退出 0，这些由测试一并钉住。

## 与 spec/transcripts 的关系

`spec/transcripts/` 钉住的是 JSON-RPC 协议帧的时序与形状；本目录钉住的是 `doctor`/`repair --dry-run` 两个 CLI 文档的完整内容。两者合起来构成移植时的机器可检材料。
