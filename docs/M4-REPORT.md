# M4 报告：门 G4（协议帧与 transcript 回放）

日期：2026-11-02 · 对应 `docs/MIGRATION-ROADMAP.md` 的 M4 与门 G4

## 一句话

**门 G4 过了**：`spec/transcripts/` 的 3 个录制逐帧通过；`o824 mock` 能直接替代内核驱动一个客户端——用录制自己的请求喂给它，吐出的帧与录制逐帧相同，而且 Go 和 Rust 两个 mock 的输出也彼此相同。

## 判据与证据

```
cargo fmt --all --check                       PASS
cargo clippy --all-targets -- -D warnings     PASS（0 warning）
cargo test --workspace                        PASS（237 个用例）
cargo build --release --bin o824              PASS
python scripts/cross-check.py                 transcripts 3/3（另含 doctor 8/8、plan 8/8、真机 3/3）
```

三向对照：`Go mock` vs `录制`、`Rust mock` vs `录制`、`Go` vs `Rust`。

## 做了什么

### `o824-protocol`：帧与序列号

LSP 风格的 `Content-Length: N\r\n\r\n<json>` 编解码（spec §9.1），带 64 MiB 上限、大小写不敏感的头部名、以及对"头部不可解析"与"流干净结束"的区分——前者意味着帧边界不可知，是会话终止的唯一传输层故障。

帧编解码只写一份，`mock` 用的就是内核这份（这正是 Go 注释里写的"第二个解析器会让两者漂移，而漂移的 mock 对第三方客户端毫无价值"）。

`Seq` 是会话级计数器：**每发出一个服务端消息就取一个号**，无论那条消息自己带不带 seq，所以编号在"响应带 seq、通知带 seq"的混排里依然连续。

### `o824-mock`：transcript 回放

- 解析 JSONL 并**按行分类**：有 `method` 和 `id` 是请求，有 `method` 无 `id` 是通知，有 `id` 且 `result`/`error` 恰有一个是响应。每个请求配上它之后、下一个请求之前的服务端消息。
- 回放时**重写两个值**：响应的 `id` 换成调用方的 id，所有 seq（顶层或通知的 `params` 里）重编号，所以客户端可以用任意 id、并断言 seq 单调。
- **按方法匹配而非按位置**：同一方法录了几次就用几次，用完后重复最后一个答案（开发期重发 `info`/`discover` 不会撞墙）；录制里没有的方法才是真错误：`-32601` + `data.code = E_UNSUPPORTED` + `data.supported` 列出覆盖的方法。
- `--delay` / `--fail-at` / `--protocol` 三个注入开关与 Go 一致；`--fail-at` 替换第 n 条消息但**不跳号**，注入的错误带 `data.code = E_MOCK_FAILURE`。

### `o824-cli mock`

`o824 mock --transcript <path> [--delay <ms>] [--fail-at <n>] [--protocol <v>]`，从 stdin 读帧、往 stdout 写帧。它和内核说同一种帧，所以客户端分不出区别——这正是 §19.5 的目的。

## 一个被契约钉住、容易写反的细节

`op.result` **在 `op.state(done)` 之前**（与设计文档 §19.1 的示例顺序相反）。transcript 里验的就是这个顺序：`plan-apply.jsonl` 的每个作业都先 `op.result` 再 `done`。`conformance/tests/transcripts.rs` 有一条专门的形状测试：响应把 `seq` 放顶层、通知把 `seq` 放 `params` 里、并且每个作业的 result 必须在 done 之前。

## 未完成

`diagnose`（诊断包 zip + 脱敏）留给 M5；CI 仍未在 GitHub 上跑过。

## 下一步

**M5**：`diagnose`（zip + 脱敏）、发布构建（单 exe、版本注入）、真机全流程验收，然后打 `v0.1.0-rs` 并归档 Go 仓库。
