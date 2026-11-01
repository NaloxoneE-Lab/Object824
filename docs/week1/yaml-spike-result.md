# YAML spike 结论

结论日期：2026-11-02 · 对应 `docs/week1/02-yaml-spike.md` · 复现：`cd spikes/yaml-spike && cargo run --release`

## 结论

**选择：行级编辑（line-level editing）。**

不做"解析整棵 YAML 树再打印回来"，而是只解析出**目标条目的行区间**，用纯文本删/插。这样才能 100% 保住注释、空行、缩进、行尾与键的顺序——因为我们根本不去重新生成那些行。

## 实验输入（真机 profile）

| 文件 | 实测大小 |
|---|---|
| `C:\Users\X_Cyanogen\.dsh\profiles\desktop\cordis.patch.yml` | **1545 字节 / 55 行**（07-28 起未变） |
| `C:\Users\X_Cyanogen\.dsh\profiles\desktop\pnpm-workspace.yaml` | 536 字节 / 18 行 |

> 更正一处规格笔误：`02-yaml-spike.md` 写的是"1545 **行**"，实测 1545 是**字节数**，行数是 55。spike 仍按真实文件跑，结论不受影响，但后面按"1545 行"估算工作量会离谱。

## 三个操作

| # | 操作 | 通过标准 | 结果 |
|---|---|---|---|
| A | 读入 → 原样写出 | 逐字节相同 | **PASS**：1545 vs 1545 字节，`cmp` 相等 |
| B | 删除 `- id: opencode-go`（含嵌套 config） | diff 只显示被删的那几行 | **PASS**：`-9` 行，且 diff 里**没有任何 `+` 行**（只有删除） |
| C1 | `allowBuilds` 不存在时追加 | 追加到文件末尾 | **PASS**：`+3` 行（空行分隔符 + `allowBuilds:` + 一个键），无删除 |
| C2 | `allowBuilds` 存在时加一个键 | 只多一行 | **PASS**：`+1` 行，插在 map 末尾而不是文件末尾 |

C1 的 3 行里有一行是**有意加的空行分隔符**（文件原本以列表项结尾，直接贴 `allowBuilds:` 虽然合法但难读）。真正属于语义的只有 2 行。

## 候选库的实测证据

| crate | 结论 | 证据（同一份真机文件） |
|---|---|---|
| `marked-yaml` 0.8.0 | **不可用** | 解析直接失败：`4:1: Top level must be a mapping`。而且它**根本没有 emitter**：docs.rs 上 `Node` 只实现 `Debug`/`PartialEq`/`From<…>`/`TryFrom<Yaml>`，没有任何 `Display` 或 serde `Serialize` 实现，官方文档自己也写着"Currently this library only supports parsing YAML from strings" |
| `yaml-rust2` 0.10.4 | **不可用**（作为改写器） | 解析 OK，`YamlEmitter` 输出 **1331 字节 vs 输入 1545 字节**，注释保留 **0 / 3**，键顺序与引号风格也被重写 → A 直接 FAIL |
| 自制行级编辑 | **采用** | A/B/C1/C2 全 PASS（见上表） |

补充：`serde_yaml` 已于 2024 归档、`serde_yml` 是它的 fork，两者都不保留注释，本来就不在候选里。

## 已知限制（写清楚，别等踩到才发现）

1. **块标量**（`config: |` / `>` 里的多行文本）中如果出现**行首 `- `**，会被误当成新条目。目前两个真实场景里都没有块标量；将来若遇到，先按"扫描到块标量就把它的内容整体吞掉"补规则，并加一条测试。
2. **多文档**（`---` 分隔）不支持：只处理单文档文件。
3. **锚点/别名**（`&a` / `*a`）不解析也不改写：行级编辑不碰它们，所以"不改"是安全的；但如果某天需要按锚点定位，要另做。
4. 条目定位用"行首 `- `"，因此**要求缩进风格稳定**（顶层条目不缩进）。真实 profile 满足这一点；`pnpm-workspace.yaml` 只需要定位 `allowBuilds:` 这一行和它的子块，同样简单。
5. 行尾：编辑保留原文件的换行风格（`split_inclusive('\n')` 原样拼回），不强制 CRLF/LF 统一。

## 对 `verify` / `plan` / `apply` 的影响

- **`verify` 只读**：用真正的 YAML 解析器（`yaml-rust2` 即可）判断"能不能解析、第几行坏"，不需要保真，`parse-error`（F16）要求的就是行号。
- **`plan` 产出的是"改动指令"而不是"新文件"**：删 `[start,end)` 行、在第 N 行后插一行。这让计划天然确定（同树同计划 → golden 的 `steps` 稳定），也让指纹只依赖行内容。
- **`apply` 执行的是行编辑**：写前快照、写后 `diff` 校验，失败回滚。因为不重排文件，"修完把用户注释吃掉"这类事故在结构上就不可能发生。
- **`dedupe-loader` 必须先重扫**：一次 apply 可能删多条，每次删完都要重新计算行区间（行号会平移）。这条要作为 `apply` 的测试用例。

## 复现方式

```bash
cd o824-rs/spikes/yaml-spike
source ../../scripts/env.sh     # 或 . .\..\..\scripts\env.ps1
cargo run --release
```

可用 `O824_PATCH_YML` / `O824_WORKSPACE_YAML` 指向别的 profile 副本。spike 是独立 workspace（`exclude` 出主 workspace），它的依赖不会进发布图的依赖集合。
