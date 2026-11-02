# P2-1 启停（web 形态先做）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `o824 start | stop | status | logs` 让 web 形态能被**真正**起、停、看日志，并且**拒绝**在同一个 DSH home 上再起一个实例（§21.4 的第 1 条风险）。

**Architecture:** `start` 用 `dsh --profile web --port <n> --no-open` 起一个**分离**的子进程，stdout/stderr 重定向到 `<data_root>\logs\`；把 `{pid, port, home, image, startedAt, logPath}` 记进 state。`stop` 先**验证**记录的 pid 仍是当初那个进程（image + 创建时间），再杀。`status` 读记录并探活。`logs` 从记录里的路径读尾部。

**Tech Stack:** Rust；`o824-execx` 的分离启动（新增）、`o824-state` 的记录、`windows` crate 的 `GetProcessTimes`（pid 复用防护）。不新增第三方依赖。

**Spec:** `docs/design/2026-10-28-dsh-launcher-design.md` §21.2（start/stop/status/logs 是形态适配器的操作）、§21.3（web 的端口与进程残留）、§21.4（并发实例）、§21.5（`start` 返回 `{pid, port, url}`）、§12（P2）

## Global Constraints

- **不动用户的真实 home**：所有测试用 `DSH_HOME` 指向临时目录。真实 `~/.dsh` 里的会话是用户的数据，起第二个实例去碰它是我们自己在制造 §21.4 警告的那种危险。
- `start` 必须**分离**：CLI 退出后服务仍在跑；没有守护进程化就不叫 start。
- **URL 里的 token 是秘密**：`start` 的应答可以带 URL，但**持久化的记录里只存端口与日志路径**；需要 URL 时从日志里读。
- `stop` 只杀指纹匹配的进程：image 相同 + 创建时间相同。pid 会被复用，只比 pid 是错的。
- 端口冲突不是错误：`--port 0` 让系统挑；用户指定的端口被占时返回 `E_PORT_IN_USE`。
- 没有 `dsh` 可执行文件时，测试**跳过**而不是失败（CI 上没有 DSH）。

## Task 1: state 里的进程记录

**Files:** `crates/o824-state/src/lib.rs`、测试同文件

**Interfaces:**
- `pub struct ProcessRecord { install_id, pid: u32, port: u16, home, image, started_at: Timestamp, log_path }`
- `State.processes: BTreeMap<String, ProcessRecord>`（键 = install id），`null` 读作空（与 `installs` 同一个规则）。
- 兼容：老 `state.json`（只有 `installs`）必须照常读出，且写回时不多出 `"processes": null`。

- [ ] **Step 1: 写失败的测试**：老状态文件（无 `processes`）能读；写入记录后往返；`"processes": null` 读作空。
- [ ] **Step 2–4**：失败 → 实现 → 通过。
- [ ] **Step 5: 提交**。

## Task 2: 分离启动与就绪检测

**Files:** `crates/o824-execx/src/detached.rs`、`crates/o824-webform/src/lib.rs`（新 crate）、测试

**Interfaces:**
- `execx::spawn_detached(program, args, dir, env, log_path) -> Result<u32, Error>`（返回 pid；`CREATE_NO_WINDOW | DETACHED_PROCESS`，stdout/stderr 指向同一日志文件）
- `webform::start(install, home, data_root, port: Option<u16>, timeout) -> Result<Started, Error>`，`Started { pid, port, log_path }`
- 就绪判据：日志里出现 `dsh web: http://…:PORT/`，或超时 → 杀掉子进程并返回 `E_TIMEOUT`（不让半起的进程留在后台）。

- [ ] **Step 1: 写失败的测试**：用真实的 `dsh`（不存在则跳过）在临时 home 上起一次，断言返回的 port > 0 且日志里有那一行；再断言超时路径会杀掉子进程（用一个永远不打印的假程序）。
- [ ] **Step 2–5**：实现、通过、提交。

## Task 3: start / stop / status / logs 子命令

**Files:** `crates/o824-cli/src/{start,stop,status,logs}.rs`、`lib.rs`

- [ ] **Step 1: 写失败的测试**：`start` 后 `status` 显示 running 与端口；`stop` 后 `status` 显示没有；同一 home 上第二次 `start` 被拒（`E_CONCURRENT_INSTANCE`）；`logs` 打出日志尾部。
- [ ] **Step 2–5**：实现、通过、提交。

## Task 4: 并发实例守卫（§21.4 第 1 条）

- [ ] 「同一 home 已有活实例 → 拒绝启动」是 Task 3 的一部分；这里补**跨形态**：桌面端正在跑（进程层已能看见）时，`start --profile web` 要**警告**而不是静默（共享 `~/.dsh` 写同一份 sessions）。
- [ ] 判据：起一个假 `DeepSeek Harness.exe`（复制 cmd.exe）+ 一个 web 实例，`status` 同时列出两者并给出警告。

## 自检

- §21.5 的 `start` 返回 `{pid, port, url}`：url 从日志提取，不落盘。
- §21.3 的"要能识别这个 node 是我们上次起的"：靠记录里的 image + 创建时间。
- tui 形态不在本计划内：本机没有那个应用，对着文档里的包名实现等于编造证据。
