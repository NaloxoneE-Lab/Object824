# desktop 形态：启动前必须先看进程表（缺陷修复，2026-11-06）

> 这不是一个新决定。`docs/P2-DECISIONS.md` §2 早就定了「没在跑就启动，在跑就报告那个 pid」，
> 并且写了「判定『在跑』用进程层（§7.7 第 4 层）」。本文记录的是：**这条决定在实现里只落地了
> 一半**，真机第一次按下去就露出来了。

## 1. 现象

真机 2026-11-06 12:55，用户 DSH 桌面端已在运行（12:52 启动，系统拉起，非 o824 启动）。
O824-HG 启动器点「启动」后，内核的作答是失败：

```
kernel: plan start for desktop (i3)
kernel: plan plan-20261106T045533Z-6d2617d0
kernel: apply -> 启动失败：desktop: the app exited (exit code: 0) right after starting;
                  see C:\Users\X_Cyanogen\AppData\Local\o824\logs\desktop-2026-11-06T045533Z-16736.log
```

那个日志文件是 **0 字节**：新进程还没来得及写任何东西就退了。

## 2. 为什么

DSH 桌面是单实例的。o824 拉起第二个进程，它把请求交给已有实例，自己以 0 退出。
`crates/o824-formproc/src/desktop.rs:103-116` 的就绪判据是「进程必须活过 `SETTLE`=1.5s」，
看到它退了就报 `Code::Internal`。

本该在更早一步挡住：`crates/o824-formproc/src/lifecycle.rs:80` 的

```rust
pub fn blocking_record<'a>(state: &'a State, home: &Path) -> Option<&'a ProcessRecord> {
    state.processes.values()
        .find(|record| same_home(&record.home, &home) && is_alive(record))
}
```

**只查 `state.processes`** —— 也就是 o824 自己写下的记录。用户从开始菜单起的那个实例不在
里面，于是 o824 认定「没在跑」，拉了一个，然后判据把一个正常的单实例交接当成了崩溃。

§2 写的「用进程层」并没有落地。`desktop.rs` 开头的注释（"start it if nothing is running,
otherwise report what is"）描述的是**应该有**的行为。

## 3. 影响

- 用户视角：程序明明开着，点「启动」得到的是一条红色失败。这比没有按钮更糟。
- 数据视角：`apply` 失败 → 计划没被消费 → 状态机里留下一次失败的启动记录。
- 这也是 `docs/P2-STATUS.md` 里「desktop 启停语义」那条待决项的**实证**：它不是语义没想清，
  是判定来源选窄了。

## 4. 决定

补齐 §2 的落地：**启动 desktop 之前，先看机器上有没有实例，而不只是看自己启动过什么。**

判定顺序：

1. `blocking_record(state, home)` 命中 → 直接报告它的 pid（现状不变，这条是对的）。
2. 否则做**进程层探测**：按镜像路径命中候选进程，再按「它服务的 home」筛。
3. 都没有 → 照旧 spawn。

## 5. 难点：怎么知道一个外部进程服务的是哪个 home

这是本次要定的事，不能含糊：

- 镜像路径（`image_of`）只能证明「是 DSH 桌面」，证明不了「是哪个 home」。
  同一台机器上桌面端只有一个安装，但 home 可以是 `~/.dsh` 或任何 `DSH_HOME`。
- 命令行参数里没有 home：`desktop.rs:67` 传的是**环境变量** `DSH_HOME`，不是 argv。
- 读另一个进程的环境块要 `NtQueryInformationProcess` + 读 PEB，权限与健壮性都是新风险。

**已拍板（2026-11-06，用户）：接受弱承诺。** 判定方法：镜像路径 + 启动时间早于本次请求。

接受它的一个直接后果，必须写在这里：**如果你在别的 home 上开着桌面端，这里的「启动」会被
当成「已在运行」。** 这就是 `desktop.rs` 已经在用的那种「stated rather than faked」——
弱承诺写进 CLI 输出，而不是假装知道 home。

判定与动作：

- 命中且只有一个候选 → 报告 pid，退出 0，不 spawn；
- 命中多个 → 报告「有 N 个实例在跑」并列出 pid，同样不 spawn；
- 一个都没有 → spawn。

代价说清楚：如果用户在**别的 home** 上开着桌面端，这条会把「启动」当成「已在运行」。
这正是 `desktop.rs` 已经在用的那种「stated rather than faked」——把弱承诺写进代码注释和
CLI 输出里，而不是假装知道 home。

如果这个代价不可接受，那就得走 PEB 那条路，工作量是另一个量级，且要单独评估。

## 6. 测试（`crates/o824-formproc`）

| 用例 | 期望 |
|---|---|
| state 里有活记录 | 报告该 pid，不 spawn（现状回归） |
| state 空，进程表里有 DSH 桌面 | 报告 pid，不 spawn，exit 0 |
| state 空，进程表里有 2 个 | 报告 2 个 pid，不 spawn |
| state 空，进程表里没有 | 照旧 spawn 并走 `SETTLE` |
| state 里有**陈旧**记录（pid 已死） | 忽略记录，继续走进程层 |

进程表探测要可注入（trait 或闭包），否则这四个用例在 CI 上不可测。

## 7. 明确不做

- 不判断窗口是否在屏幕上（桌面应用不打印 URL，这个弱承诺保持）。
- 不动 web / tui 形态。
- 不改协议：仍然只有 plan/apply，没有 `o824.start`。
- 不在本会话改代码。

## 8. 顺带记录（不属于本缺陷）

O824-HG 侧（`D:\dsh work\O824-HG`）在这条链路上完全正常：`click_main_btn` → `plan` → `apply`
→ 进度行 → 结果回显都跑通了。修完这里，那边不需要动。

---

## 9. 施工单（给下一个会话，2026-11-06）

已经核过的事实，不必再查：

- `crates/o824-formproc/Cargo.toml` **已经依赖** `o824-discovery`，所以进程层可以直接用，不用新增依赖边。
- `o824_discovery::EXE_NAME == "DeepSeek Harness.exe"`（`crates/o824-discovery/src/lib.rs:61`），
  `o824_discovery::host_processes() -> Vec<RunningHost{pid, image}>`（`src/processes.rs:25`）
  返回**全部**该镜像名的进程，已带完整 image 路径。这正是桌面形态要的东西。
- `lifecycle::start` 的唯一调用点是 `crates/o824-apply/src/process.rs:55`。
- 现有错误码 `Code::ConcurrentInstance`（`E_CONCURRENT_INSTANCE`）语义是"同一 home 上已有实例"，
  **不要**拿它表示"已在运行且成功"——那是成功路径，不是错误。
- `StopOutcome`（`lifecycle.rs:67`）已经确立了"显式枚举结果"的写法，照它办。

### 9.1 `crates/o824-formproc/src/desktop.rs`

在函数**最开头**（创建日志文件之前——否则每次启动都会留下一个 0 字节的
`desktop-*.log`，这次事故里那个空文件就是这么来的）加探测：

```rust
/// A running desktop app whose image is `exe`. The weak promise: image path only,
/// so an instance serving a different DSH home looks like this one.
pub fn running_instance(exe: &Path) -> Option<u32> {
    let want = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    o824_discovery::host_processes()
        .into_iter()
        .find(|host| {
            let have = Path::new(&host.image);
            have.canonicalize().map(|p| p == want).unwrap_or(false)
        })
        .map(|host| host.pid)
}
```

并给 `start` 加一个可注入的探测参数（否则 9.3 的用例在 CI 上没法测）：

```rust
pub fn start(
    exe: &Path, args: &[String], home: &Path, data_root: &Path,
    on_spawned: &mut dyn FnMut(&ProcessRecord) -> Result<(), Error>,
) -> Result<StartOutcome, Error> {
    start_with(exe, args, home, data_root, on_spawned, &running_instance)
}

pub fn start_with(
    exe: &Path, args: &[String], home: &Path, data_root: &Path,
    on_spawned: &mut dyn FnMut(&ProcessRecord) -> Result<(), Error>,
    probe: &dyn Fn(&Path) -> Option<u32>,
) -> Result<StartOutcome, Error> {
    if let Some(pid) = probe(exe) {
        return Ok(StartOutcome::AlreadyRunning(pid));
    }
    // …原有实现原样保留，最后 Ok(StartOutcome::Started(Started{…}))
}
```

### 9.2 `crates/o824-formproc/src/lifecycle.rs`

```rust
/// What a start produced.
#[derive(Clone, Debug)]
pub enum StartOutcome {
    /// We spawned it.
    Started(Started),
    /// Something was already running and we did not spawn anything.
    AlreadyRunning(u32),
}
```

`lifecycle::start` 的返回类型改为 `Result<StartOutcome, Error>`：

- `Form::Web` → 包一层 `StartOutcome::Started`
- `Form::Desktop` → 直接返回 `desktop::start(...)` 的结果

### 9.3 `crates/o824-apply/src/process.rs:55`

```rust
let outcome = lifecycle::start(form, &executable, &home, data_root, port, &mut |record| { … })?;
let StartOutcome::Started(started) = outcome else {
    // Nothing was started, so nothing is recorded: writing a record for a pid we did
    // not spawn would make `stop` kill a process o824 does not own.
    return Ok(());
};
```

（`use` 行改成 `use o824_formproc::lifecycle::{self, Form, StartOutcome, StopOutcome};`）

### 9.4 CLI：不写记录之后别误伤

`crates/o824-cli/src/process_cmd.rs:235` 在 apply 之后从 state 里找记录，找不到就报
`"o824 start: the plan ran but recorded no process"`。走 apply 的 `AlreadyRunning` 分支时
**没有记录**，会撞上这条。改法（改动最小、不动协议）：

```rust
let started = …原有查找…;
let Some(started) = started else {
    // apply may have found an instance it did not start; report that one instead.
    let exe = …desktop 可执行文件路径（与 plan 用的同一个）…;
    if let Some(pid) = o824_formproc::desktop::running_instance(&exe) {
        if flags.flag("json") {
            let _ = writeln!(stdout, r#"{{"status":"already-running","pid":{pid}}}"#);
        } else {
            let _ = writeln!(stdout, "o824 start: desktop 已在运行 (pid {pid})");
        }
        return EXIT_OK;
    }
    …原来的报错…
};
```

退出码 0，符合 §2 的"报告那个 pid 并以 0 退出"。

### 9.5 测试（`crates/o824-formproc/src/desktop.rs` 的 `#[cfg(test)]`）

probe 是注入的，所以四个用例不需要真进程：

| 用例 | 做法 | 断言 |
|---|---|---|
| 已有实例 | `probe` 返回 `Some(4242)` | `Ok(AlreadyRunning(4242))`，且 `logs/` 下**没有**新文件 |
| 没有实例 | `probe` 返回 `None`，`exe` 指向一个临时假 exe | 走到 spawn 路径（可用一个立刻退出的 `cmd` 桩） |
| 两个实例 | `running_instance` 只取第一个；另加单测覆盖排序稳定性 | 返回稳定 pid |
| 镜像不同 | `probe` 比较路径时用 `canonicalize` | 不同路径不命中 |

`running_instance` 本身另写一个 `#[ignore]` 的真机用例（本机开着 DSH 桌面时才跑）。

### 9.6 验收

```bash
source scripts/env.sh && source scripts/env-msvc.sh
cargo +stable-x86_64-pc-windows-msvc test -p o824-formproc -p o824-apply
cargo +stable-x86_64-pc-windows-msvc clippy --all-targets -- -D warnings
# 真机：DSH 桌面开着时
o824 start --install <desktop 的 id> --json   # → {"status":"already-running","pid":N}，exit 0
# 关掉 DSH 桌面再跑一次                        # → 真正拉起，仍按 SETTLE 判就绪
```

### 9.7 不要做的事

- 不要改 `o824-kernel` 的协议（仍然只有 plan/apply）。
- 不要把 `AlreadyRunning` 做成 `Err`——它是成功。
- 不要为了这个改动去读别的进程的环境块（PEB）；§5 已经拍板接受弱承诺。
- 不要动 web / tui 两个形态。

### 9.8 改完之后 O824-HG 侧要不要动

不用。界面上 `apply` 成功后会显示"已启动 desktop"；如果想更准确，可在
`D:\dsh work\O824-HG` 的 `src/kernel.rs` 里把 apply 结果里的 `alreadyRunning` 读出来，
把 `sub_tip` 改成"已在运行"。那是另一个仓库的独立小改动，可选。


---

## 10. 施工结果（2026-11-06，同一会话完成）

按 §9 的施工单做完了，另外发现一件文档没预料到的事。

### 10.1 做了什么

| 位置 | 改动 |
|---|---|
| `crates/o824-formproc/src/desktop.rs` | 新增 `running_instance(exe)`（镜像路径匹配）、`earliest(pids, created)`（按创建时间取最早）、`StartOutcome{Started, AlreadyRunning}`；`start` 变薄壳，`start_with(..., probe)` 接收注入的探测 |
| `crates/o824-formproc/src/lifecycle.rs` | 新增 `lifecycle::StartOutcome{Started, AlreadyRunning}`；`start` 返回它，web 形态包成 `Started`，desktop 形态透传 |
| `crates/o824-apply/src/process.rs` | `let StartOutcome::Started(started) = outcome else { return Ok(()) };` —— **不写记录** |
| `crates/o824-cli/src/process_cmd.rs` | 记录找不到时，回落到 `desktop::running_instance`，报 `already-running` 并退 0 |
| `scripts/kernel-start-check.ps1` | 新增：直接驱动内核协议（discover → plan → apply），就是窗口走的那条路 |
| `scripts/lab-compare.py` | 顺手：`pwsh` 改成可用 `O824_PS` 覆盖（本机没装 PowerShell 7，只有 5.1） |

探测放在**创建日志文件之前**：这正是原来那个 0 字节 `desktop-*.log` 的来源。

### 10.2 文档没预料到的一点：Electron 的多个进程

§5 假设「按镜像路径命中候选进程」得到的就是那个桌面实例。真机上有 **7 个进程共用同一个镜像**（主进程 + 渲染器 + GPU + utility），照 §9.1 的 `.find(...)` 取第一个，报出来的可能是渲染进程的 pid——一个用户拿去 `stop` 会失败的数字。

处理：镜像筛完之后**取创建时间最早的那个**（Electron 的主进程先起）。`earliest()` 单独成函数以便测试；读不到创建时间时退回第一个候选，而不是假装没有实例。

真机验证（DSH 桌面开着）：

```
running_instance(D:\...\Programs\DeepSeek Harness\DeepSeek Harness.exe) = Some(16444)
```

### 10.3 真机验收（§9.6 那两条）

用 `scripts/kernel-start-check.ps1` 走**协议**（不是 CLI 的预检），因为 GUI/kernel 才是出事的那条路：

```
kernel: C:\Users\X_Cyanogen\AppData\Local\o824\o824.exe
desktop install: i3  version 0.2.1-rc.3
plan: plan-20261106T084508Z-7fbc3cda  steps: start-desktop
the already-running desktop was accepted, not reported as a crash      exit=0
```

修复前这条链的答复是 `启动失败：desktop: the app exited (exit code: 0) right after starting`。

> 脚本必须从**安装位置**运行：从构建目录起的内核是 Low IL，写不了 `%LOCALAPPDATA%\o824` 下的计划文件
> （`cannot write the plan: 拒绝访问`）。这不是本缺陷的一部分，是同一条完整性规则。

### 10.4 测试

新增 5 个（`cargo test -p o824-formproc`）：

- `an_existing_instance_is_reported_and_nothing_is_started`：probe 返回 `Some(4242)` → `AlreadyRunning(4242)`，且 `logs/` 下**没有**新文件；
- `with_nothing_running_the_check_under_test_is_the_probe_itself`：probe 被调用一次，且传 `None` 时走到了 spawn（失败于"不是程序"，正是预期）；
- `the_earliest_process_wins`：三个 pid 取创建时间最早的；读不到时间时退回第一个；
- `a_path_that_differs_does_not_count_as_the_same_image`：不同路径不互相命中；
- `the_real_probe_finds_a_running_desktop`（`#[ignore]`，真机跑）。

工作区：**419 通过 / 0 失败**；clippy 干净；`cross-check.py` 22 PASS；`lab-compare.py` 6/6。

### 10.5 与 §9 的两处偏差，说明理由

1. **JSON 字段用 `alreadyRunning`，不是 §9.4 草稿里的 `"status":"already-running"`**。CLI 顶部的预检早就在发
   `alreadyRunning: true`，`D:\dsh work\O824-HG` 也是按这个键读的（§9.8）。为一个新分支再发明一个字段，会
   让同一个客户端要处理两种形状。
2. **多候选时不报「有 N 个实例」**（§5 的第四条）。Electron 一次启动就是多个进程，报 N 会把"一个实例"
   说成"多个实例"。改为报最早那个进程的 pid，并在注释里写明为什么。
