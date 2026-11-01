# 02 · YAML 选型 Spike（本周最重要的一个决定）

## 为什么先做它

内核有两处**必须改写用户的 YAML，同时保住注释和顺序**：

1. `dedupe-loader`：删掉 `cordis.patch.yml` 里重复的 loader 条目；
2. `allow-builds`：在 `pnpm-workspace.yaml` 的 `allowBuilds` map 里加一个键。

Go 版用的是 `gopkg.in/yaml.v3` 的 **Node 树**（保留注释、顺序、空行）。Rust 这边如果选错库，两个后果：

- 改写后注释全丢 → 用户打开文件发现"我的注释没了"，这是产品事故；
- 或者只能整份重写 → 同样是事故。

所以这一项**必须在写 `verify`/`plan`/`apply` 之前有结论**。

## 实验

用真机的 profile 文件做（不要用合成的简化样本）：

```
输入：C:\Users\X_Cyanogen\.dsh\profiles\desktop\cordis.patch.yml   （1545 行，带注释与空行）
输入：C:\Users\X_Cyanogen\.dsh\profiles\desktop\pnpm-workspace.yaml（带注释）
```

三个操作，每个都要过：

| # | 操作 | 通过标准 |
|---|---|---|
| A | 读入 → 原样写出 | 输出与输入**逐字节相同**（含注释、空行、缩进、行尾） |
| B | 读入 → 删除 `- id: opencode-go` 那一条（含它的嵌套 config）→ 写出 | `diff` 只显示被删的那几行；其它注释、顺序、缩进全不变 |
| C | 读入 → 在 `allowBuilds` map 里加 `"foo@git+https://example.com/x.git": true` → 写出 | 若 `allowBuilds` 不存在则追加到文件末尾；存在则只多一行；其余不变 |

操作 B 和 C 正是 `dedupe-loader` 与 `allow-builds` 要做的事——**这不是玩具实验，是真实需求的最小切片**。

## 候选库

| crate | 保留注释 | 保留顺序 | 维护状态 | 备注 |
|---|---|---|---|---|
| `serde_yaml` | ❌ | 部分 | **已停维护**（2024 归档） | 不能用 |
| `yaml-rust2` | 部分（node API 能拿到注释 token，但要自己重建） | ✅ | 活跃 | 需要写"节点树编辑 + 回写"的适配层 |
| `marked-yaml` | ✅（设计目标就是 round-trip） | ✅ | 活跃但用户少 | 最贴近需求，风险是生态小、边界情况未知 |
| `saphyr` | 继承 `yaml-rust2` 的定位 | ✅ | 活跃 | 同 yaml-rust2 |
| `serde_yml` | ❌ | 部分 | 是 `serde_yaml` 的 fork | 同样不适合"保注释改写" |

## 判定顺序

1. 先试 **`marked-yaml`**：如果 A/B/C 三个操作全过，选它（它就是为这件事设计的）。
2. 不过就试 **`yaml-rust2`** + 自己写适配层：A 很可能过不了（注释要自己挂回去），那就退一步——**只对"改写"这个场景做行级操作**：解析出目标条目的**行区间**，用文本编辑删/插，不重排整个文件。这条路能 100% 保住一切，代价是要自己写"YAML 条目到行号"的映射（可控，因为我们的两个场景结构都很简单：顶层数组的条目、以及一个 map 的追加）。
3. 两条都不行，**明确记录**，并在 `verify`/`plan` 设计评审时把这个约束提出来（可能的退路：改写前先把原文件快照，写坏可回滚——但不能因此丢掉"保注释"的承诺）。

## 结论要写成什么

新建 `docs/week1/yaml-spike-result.md`，内容必须包含：

```markdown
# YAML spike 结论
选择：<crate 名 或 "行级编辑">
A（round-trip 逐字节）: PASS/FAIL，证据：<命令 + 输出摘要>
B（删除一个条目）: PASS/FAIL，证据：<diff 行数>
C（allowBuilds 加键）: PASS/FAIL，证据：<diff 行数>
已知限制：<例如 "块标量 | 内部的注释会丢" —— 有就写，没有写"未发现">
对 verify/plan/apply 的影响：<一句话>
```

## 一个反例（供参考）

Go 版踩过的坑：`allowBuilds` 早先写成**数组**，pnpm 11 直接报错；后来改成 map。如果 Rust 侧的 YAML 库在写 map 时把 key 加了引号（`"foo": true` vs `foo: true`），pnpm 仍然接受，但**golden 会不一样**——因为 golden 比的是内核输出的 JSON，不是 YAML 文件本身，所以这条不影响验收，但会影响"用户看到的文件长什么样"。记录在结论里即可。
