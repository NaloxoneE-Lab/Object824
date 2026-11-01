# 04 · 前三个 crate：`errs` / `model` / `version`

这三个没有业务逻辑，但它们定义了**所有 JSON 的形状**。做对了，后面每个 crate 的验收都能直接用 golden；做错了，后面全要返工。

## `o824-errs`

```rust
pub struct Code(&'static str);      // 或 #[derive] 的 enum + as_str()
pub struct Error {
    pub code: Code,
    pub fallback: String,           // 英文兜底文案
    pub params: serde_json::Map<String, serde_json::Value>,
}
impl std::fmt::Display for Error { /* 输出 fallback，fallback 为空时输出 code */ }
impl std::error::Error for Error {}
```

**错误码必须一字不差**（它们是协议的一部分，golden 里会出现）：

```
E_USAGE  E_PERMISSION  E_STATE_DRIFT  E_NETWORK  E_BUSY  E_INTERNAL
E_INSTALL_DIR  E_LINK_BROKEN  E_BUNDLE_MISSING  E_BRIDGE_DANGLING
E_DUP_LOADER  E_PROFILE_PARSE  E_PNPM_BUILD_BLOCKED  E_HALF_INSTALL
E_CONFIRMATION_REQUIRED  E_DISK_FULL  E_PLAN_UNKNOWN  E_PLAN_EXPIRED
E_PROTOCOL_PARSE  E_PROTOCOL_INVALID  E_PROTOCOL_MISMATCH  E_UNSUPPORTED
E_MOCK_FAILURE
```

**退出码映射**（`o824-cli` 用，测试要钉住）：

| 错误码 | 退出码 |
|---|---|
| 无错误 | 0 |
| `E_USAGE`、`E_INSTALL_DIR` | 2 |
| `E_CONFIRMATION_REQUIRED` | 3 |
| `E_PERMISSION` | 4 |
| `E_STATE_DRIFT` | 5 |
| `E_NETWORK` | 6 |
| `E_BUSY` | 7 |
| 其它任何 `Error` / 非 `Error` | 8 |

注意 `E_INSTALL_DIR → 2`（它是"路径给错了"，属用法族），不是 8。

## `o824-model`

```rust
pub struct Finding {
    pub check_id: String,                  // "F01"…"F20"
    pub severity: Severity,                // "error" | "warning" | "info"
    pub code: errs::Code,
    pub category: Category,                // 九种，见下
    pub params: serde_json::Map<String, Value>,   // omitempty
    pub fallback_message: String,          // omitempty
    pub detail: String,                    // omitempty
    pub repair: Option<Repair>,            // omitempty
}
pub struct Repair { pub kind: String, pub plan_hint: String }
pub struct Envelope { pub envelope: String, pub findings: Vec<Finding> }
pub struct Install {
    pub id: String, pub kind: String, pub root: String, pub version: String,
    pub profiles: Vec<String>, pub health: String, pub home: String,   // home: omitempty
}
```

**JSON 字段名**（少一个字母 golden 就红）：

| 结构体 | 字段 |
|---|---|
| `Finding` | `checkId` `severity` `code` `category` `params` `fallbackMessage` `detail` `repair` |
| `Repair` | `kind` `planHint` |
| `Envelope` | `envelope` `findings` |
| `Install` | `installId` `kind` `root` `version` `profiles` `health` `home` |

**`Category` 九值**（顺序即 §19.3 的顺序）：`install` `config` `plugin` `data` `env` `security` `network` `permission` `internal`。

**`NewEnvelope` 的语义**：`envelope` 恒为 `"dsh-doctor/v1"`；`findings` 为 `null` 时**必须输出 `[]`**（不是 `null`）。同理 `Install.profiles` 永远输出数组。

> 这条是 Go 版踩过的坑：空计划一度输出 `"steps": null`，因为 Go 的 nil slice 序列化成 null。golden 是跨语言契约，**任何语言的"零值"差异都必须被消掉**，否则移植方要模仿对方的实现细节。

## `o824-version`

```rust
pub const VERSION: &str = option_env!("O824_VERSION").unwrap_or("0.1.0-dev");
pub const COMMIT: &str  = option_env!("O824_COMMIT").unwrap_or("");
pub fn string() -> String;  // "o824 <version> (commit <c>, built <t>) <os>/<arch>"
```

`--version` 输出到 stdout、退出 0；`--help` 输出到 stdout、退出 0。

## 要移植的测试

| 测试 | 断言什么 |
|---|---|
| `envelope_shape` | `NewEnvelope` 的 JSON 是 `{"envelope":"dsh-doctor/v1","findings":[...]}` |
| `finding_omits_empty_detail` | 空的 `detail`/`params`/`repair` 不出现在 JSON 里 |
| `finding_json_contract` | 八个字段名逐个断言（防止改名） |
| `new_envelope_normalises_nil` | nil findings → `[]` |
| `exit_code_map` | 上表逐行 |
| `install_json_contract` | 六个/七个字段名与类型 |

## 完成判据

- [ ] 六个测试全过。
- [ ] 用一个真实的 golden 文件（比如 `git-dep.doctor.json`）反序列化成 `Envelope` 再序列化，**逐字节相同**（这一步同时验证了字段顺序与 `preserve_order`）。
