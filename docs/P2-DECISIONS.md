# P2 待决事项：已定（2026-11-03）

三条待决事项在 2026-11-03 由用户拍板。下面是"当初为什么这么选"的记录；当前进度的真相
来源是 `docs/P2-STATUS.md`。

## 1. 启停走进 plan/apply —— 选 A

**决定**：`o824.plan` 接受 `action: repair|start|stop`，`apply` 执行；CLI 的 `start`/`stop`
改成"建计划 + 立即 apply"，显式命令本身就是确认。

**落地**：`crates/o824-plan/src/process.rs`（计划）、`crates/o824-apply/src/process.rs`（执行）、
`crates/o824-formproc/src/lifecycle.rs`（判定与守卫）。协议层因此**没有** `o824.start`，
符合 §21.5 的原话。

## 2. desktop 形态的启停 —— 选 A

**决定**：没在跑就启动，在跑就报告那个 pid。**不接受**第二个窗口：`desktop` profile 被 DSH
独占，第二份会与用户自己开着的那个冲突。

**落地**：`crates/o824-formproc/src/desktop.rs`。判定"在跑"用进程层（§7.7 第 4 层）；
`o824 start --install desktop` 在发现已有实例时直接报告 pid 并以 0 退出。

**明确的弱承诺**：桌面应用不打印 URL，所以它的"就绪"只能是"起来一会儿还活着"，而不是
"窗口已经在屏幕上"——代码里写清了，没有假装。

## 3. tui 形态

**决定**：不在本构建里启动或停止，返回 `E_UNSUPPORTED` 并说明理由（TUI 需要在终端里跑，
内核 detached 启动出来的会是一个没人能输入文字的进程）。发现与诊断照常。

## 4. F03 的判据 —— 重定（经过三次）

真机测试依次推翻了两个判据，最后剩下能由文件自身证明的那条：

1. ~~patch 行的 id 与 bundle 同名~~ —— 那是**正常的覆盖行为**，patch 文件就是干这个的。
2. ~~patch 行的 id 不在 bundles 里~~ —— bundle 提供的 entry 名与包名不同，真实 profile 上误报 8 条。
3. **同一个 id 被定义两次**（一行 `id`+`name`，或 `insert:` 元素）→ DSH 静默挂载两个 entry。
   这是文件自身能证明的，也是唯一一种"你以为在跑 A，其实 A 和 B 都在跑"的情况。

判定：warning，永不自动修——哪个是用户想要的，内核不可能知道。

**同时修正 F16**：旧规则要求每个 patch 行都有顶层 `id`，而真机 DSH 0.2.1-rc.5 接受
`- insert: [{id, name}]`（合成进配置、exit 0），用户自己的 profile 就是这么挂 MCP 的。
规则改成"要么有字符串 `id`，要么是 `insert:` 序列"。

**两个验收脚本都相应更新**：cross-check 把 F03/F16 列为"已重定"（和 F11 列为"新增"一样），
lab-compare 把两侧的这两条 finding 去掉并按剩余 findings 重算计数与退出码——分歧被观察并
解释，而不是把场景豁免掉。
