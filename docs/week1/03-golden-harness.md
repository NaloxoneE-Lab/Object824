# 03 · Golden Harness（验收管线）

目标：`cargo test -p o824-conformance` 能读 `spec/golden/` 的 16 个文件，把当前实现的输出归一化后逐字节比对。

**这一项做完，之后每个 crate 才有真正的完成标准。** 没有它，"移植"只能靠人眼看。

## 输入

```
spec/golden/<kind>.doctor.json      kind ∈ ok, broken-link, missing-bundle, dup-loader,
spec/golden/<kind>.plan.json              parse-error, half-install, dangling-bridge, git-dep
```

文件内容：两空格缩进、LF、末尾一个换行；字段顺序按 Go 结构体顺序（**所以 `serde_json` 必须开 `preserve_order`**）。

## 归一化规则（必须与 Go 版逐条一致）

对每条 JSON 值做**文本级替换**，顺序敏感（长的先替换）：

| 替换 | 值 | 说明 |
|---|---|---|
| fixture 根目录的绝对路径 | `<ROOT>` | 含 JSON 转义后的形式（`D:\\x\\y`）与正斜杠形式 |
| data root 的绝对路径 | `<DATA>` | 只有 `half-install` 会用到 |
| plan 的 `id` | `<PLAN_ID>` | 形如 `plan-20261101T121041Z-fce66dc1` |
| plan 的 `createdAt` / `expiresAt` | `<TIMESTAMP>` | RFC3339Nano |
| plan 的 `installId` | `<INSTALL_ID>` | `--root` 选择时是 `local-<sha256(root)>`，与机器相关 |

**只替换这些**。`seq`、`params`、`repair`、`fingerprint`、`fallbackMessage` 一个字都不许动——它们正是要验的东西。

一个容易漏的点：Go 的 `json.Marshal` 会把路径里的 `\` 转义成 `\\`。归一化必须同时处理"原始形式"和"转义形式"，否则 Windows 上永远对不上。

## 测试 API（建议）

```rust
// crates/o824-conformance/src/lib.rs
pub struct GoldenCase {
    pub kind: &'static str,
    pub doctor: serde_json::Value,   // 归一化后的期望值
    pub plan: serde_json::Value,
}

pub fn load_golden(dir: &Path, kind: &str) -> GoldenCase;
pub fn normalize(value: &mut serde_json::Value, root: &Path, data: Option<&Path>,
                 plan_id: &str, install_id: &str, created: &str, expires: &str);
pub fn assert_golden(expected: &serde_json::Value, actual: &serde_json::Value, label: &str);
```

`assert_golden` 失败时要打印**逐行的 unified diff**（`similar` crate 或自己写 LCS），不要只打 "mismatch"。

## 测试流程（每个 kind）

```
1. fixtures::build(kind) -> root                 （第 5 天才有；在此之前用桩）
2. doctor = run_cli(["doctor", "--root", root, "--home", root, "--json"])
3. plan   = run_cli(["repair", "--root", root, "--home", root, "--dry-run", "--json"])
4. normalize(doctor), normalize(plan)
5. assert_golden(spec/golden/<kind>.doctor.json, doctor)
   assert_golden(spec/golden/<kind>.plan.json,   plan)
6. 用一棵**新建的**树再跑一遍，断言两次归一化结果完全相同（抓"漏掉的随机性"）
```

第 6 步是 Go 版的做法，很值钱：它把"偶发"变成"必现"。

`half-install` 特殊：未完成的 journal 在树里，而 `doctor` 从 data root 读 journal。所以要把 journal 复制进一个临时 data root，并把该路径替换成 `<DATA>`。

## 重新生成

Go 版用 `-update` 开关。Rust 版建议用环境变量：

```bash
UPDATE_GOLDEN=1 cargo test -p o824-conformance
```

**规则同 Go 版**：只有在确认这是**有意的契约变更**时才重新生成，并在提交信息里写清改了哪条契约。否则你只是把回归固化成新的期望值。

## 在 `verify` 存在之前怎么用

第 1 周还没有 `verify`，所以 harness 先对**假实现**工作：

- `o824-conformance` 里放一个 `stub` 模块，直接返回 golden 文件里的 JSON；
- 断言 `assert_golden(golden, golden)` 通过——证明归一化和 diff 管线本身是通的；
- 再写一个测试，故意改一个字段，断言 `assert_golden` **失败**且 diff 里能看到那一行。

这两条测试就是"管线可信"的证明。第 5 阶段接上真的 `verify` 时，把 stub 换成真实实现即可。

## 完成判据

- [ ] 16 个 golden 文件都能被解析成 `serde_json::Value`（字段顺序保留）。
- [ ] `assert_golden` 在相等时静默通过、不等时打印带上下文的 diff。
- [ ] 归一化对 Windows 路径的两种形式（原始 `\`、转义 `\\`）都有效。
- [ ] 有一条"故意改坏一个字段必须失败"的测试。
- [ ] `cargo test -p o824-conformance` 在 CI 上跑过。
