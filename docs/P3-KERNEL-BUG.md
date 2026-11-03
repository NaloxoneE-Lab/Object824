# 内核 bug：作业事件到不了真实客户端（已修，2026-11-03）

**状态**：已修复。根因是 `main` 把全局 stdout 锁持有到进程结束。

## 症状

`o824 kernel --stdio` 收到 `o824.doctor` 后，主线程把 `{jobId}` 回复写进 stdout，然后作业线程
一个事件都发不出来。客户端只看得到 jobId，永远等不到 `op.state` / `op.result`。

## 根因

`crates/o824/src/main.rs`：

```rust
let stdout = std::io::stdout();
let mut stdout = stdout.lock();          // ← 全局 stdout 锁，持有到 main 返回
let code = o824_cli::run(&args, &mut stdout, &mut stderr);
```

`kernel` 子命令**不使用**这个 writer，而是自己建一个 `Box::new(std::io::stdout())` 交给 `serve`。
`std::io::Stdout` 的锁是**进程级全局互斥量**：主线程已经握着它，任何其他线程再调
`stdout().write_all(...)` 就会永远等下去。

于是：主线程写回复正常（重入锁），作业线程写事件全部阻塞。

## 为什么进程内测试看不见

`crates/o824-kernel` 的 11 个用例直接调 `serve`，写端是 `SharedWriter`（一个 `Vec<u8>`），
**从不经过 `main`**，所以那把锁根本没被持有。P1 里唯一的真二进制测试只验证了"EOF 后退出"，
没有发起任何作业。

## 修法

```rust
let mut stdout = std::io::stdout();   // 句柄，不是锁
let mut stderr = std::io::stderr();
```

每次写入自己取锁、写完释放，主线程不再长期占锁。

## 回归测试

`crates/o824/tests/kernel_stdio.rs`：

```rust
#[test]
fn a_job_over_real_pipes_reports_its_result_and_terminates()
```

它用**真二进制 + 真管道**发一次 `o824.doctor`，断言收到 `op.result`（带 doctor 信封）与终态
`done`。把 `main` 的修复撤掉再跑，这条用例在 87 秒后判 FAILED——**它能抓住这个 bug**。

`crates/o824-bridge`（GUI 的客户端）的 8 个用例也全部转正，其中 doctor / plan / 未知 id 三条
走的就是这条路径。

## 排查过程中被排除的（留档，免得下次重走）

| 猜测 | 怎么排除的 |
|---|---|
| 工作线程没启动 | 让它在第一步写文件：文件出现了 |
| 卡在 discovery | 插桩显示卡在**第一次写 stdout**，`resolve_install` 之前 |
| 内核自己的输出互斥锁 | 绕开它直接写 `std::io::stdout()`，依然阻塞 |
| 双线程写 stdout 不可行 | 独立小程序（含 cargo 构建、python 管道父进程）正常 |
| 沙箱禁子线程 | 同进程内工作线程写文件正常 |
| 是 stderr 缓冲吞了日志 | 换成写文件后日志可见 |

**最后一条线索是"主线程已经写过一次、然后阻塞在读 stdin"这个组合**——正是它让全局锁被占住。

## 教训

判据要选在**用户会走的地方**。内核的每个方法都应该有一次真管道往返；进程内测试写得再密，
也测不到"锁被别人一直握着"这类只有真实进程结构才有的问题。
