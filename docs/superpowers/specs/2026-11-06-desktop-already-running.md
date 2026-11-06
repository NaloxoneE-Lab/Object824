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
