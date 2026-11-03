# 内核 bug：作业事件到不了真实客户端（P3 的阻塞项）

**状态**：已定位到可复现的最小现场，未修复。发现于 2026-11-03，做 P3 的 GUI 桥接层时。

## 症状

`o824 kernel --stdio` 收到 `o824.doctor` 后，**主线程**把 `{jobId}` 回复写进 stdout，然后作业线程
一个事件都发不出来。客户端只看得到 jobId，永远等不到 `op.state` / `op.result`。

## 最小复现

```bash
# 一帧请求，stdin 给文件
python - <<'PY'
import json
body = json.dumps({"jsonrpc":"2.0","id":1,"method":"o824.doctor","params":{"installId":"i1"}}).encode()
open(".toolchain/doctor_abs.bin","wb").write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
PY

./target/debug/o824.exe kernel --stdio \
  --root "$(pwd)/.toolchain/f03-tree3" --home "$(pwd)/.toolchain/f03-tree3" \
  --data-root "$(pwd)/.toolchain/f03-tree3/data" < .toolchain/doctor_abs.bin
```

输出只有一帧：

```
Content-Length: 56

{"jsonrpc":"2.0","id":1,"seq":1,"result":{"jobId":"j1"}}
```

用管道（stdin 保持打开）也一样：内核活着，但不再产出任何帧。

## 已经排除的

| 猜测 | 排除方式 |
|---|---|
| `eprintln!`/stderr 从子线程丢失 | 换成写文件后确认：**工作线程确实跑起来了**（写出了一个文件） |
| 作业本身慢或卡在 discovery | 在 `doctor()` 里插桩，卡点在工作线程**第一次写 stdout** 处，`resolve_install` 之前 |
| 我们的 output 互斥锁被占 | 插桩显示 `write: locked` 之后卡在 `write_all`；换成**绕开互斥锁**、直接从工作线程写 `std::io::stdout()`，**依然到不了** |
| 双线程写 stdout 本身不可行 | 独立最小程序（`Arc<Mutex<Box<dyn Write>>>` 装 `stdout`，主线程写一次后 join 工作线程）**正常**；主线程持有 `StdinLock` 时也正常 |
| 沙箱禁止子线程 | 同一二进制里工作线程能写文件；独立的线程探测程序正常 |

## 剩下的可能

工作线程写 stdout 时，**主线程正阻塞在 stdin 的 `read_line` 上**——这是内核与我的最小复现唯一的
结构差异（复现里主线程在 `join`，不在读管道）。Rust 的 `Stdout`/`Stdin` 是两个独立的全局锁，
理论上不该互相阻塞；但在 Windows 上两者最终都指向同一批控制台/管道句柄，某种情况下写端会被
读端挡住。

**下一步的验证方向**（给下一个接手的人）：

1. 在主线程读 stdin 的同时，让工作线程写 stdout 的最小复现（我还没跑这一个组合）。
2. 把 `write_message` 的输出从 `Box<Stdout>` 换成**原始句柄**（`File::from_raw_handle(GetStdHandle)`），绕过 Rust 的全局 stdout 锁，看是否仍然阻塞。
3. 若确认为 Rust stdio 锁问题，改法很可能是：内核**只用一个线程写 stdout**（专用写线程 + 通道），
   这也回到我最初删掉的那个设计。

## 影响范围

- **P3 的 GUI 桥接层**（`o824-bridge` 的三个作业用例标了 `#[ignore = "KERNEL-JOBS-OVER-PIPES…"]`）。
- **第三方 UI**：任何通过 `o824 kernel --stdio` 发起 doctor/plan/apply 的真实客户端都会挂。
- **不受影响**：CLI 的直接命令（`o824 doctor` 等不开内核服务）、`o824.info`/`o824.discover`
  （同步路径，主线程直接写）、11 个内核进程内测试（用 `SharedWriter`，不是进程管道）。

## 为什么以前没发现

P1 的内核验收是"进程内测试 + 一个只验证 EOF 退出的真二进制测试"。进程内测试的写端是
`Vec<u8>`，不是管道；那个真二进制测试没有发起任何作业。**判据选在了看得见的地方，而不是
用户会走的地方。** 这一条应当补进验收清单：内核的每个方法都要有一次**真管道**往返。
