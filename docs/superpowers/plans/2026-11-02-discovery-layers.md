# 发现算法补层 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 §7.7 的发现算法补到七层：现在只有第 2（注册表）和第 3（固定卷）层，本计划补第 1、4、5、6 层，并把第 7 层（孤儿 profile）变成一条 finding。

**Architecture:** 每层产出一组候选根（`SourceRoot`），按**优先级顺序**进 `sorted_unique_roots`：同一条真实路径被多层发现时，**最先出现的那层赢**，并决定报告时的拼写。所有层都由 `Options` 注入，所以每一层都能在本机用一个临时树验证，不需要真的装 DSH。

**Tech Stack:** Rust（`o824-discovery`，新增依赖 `o824-state`）、`windows` crate 0.58（Toolhelp32 + QueryFullProcessImageNameW、COM `IShellLinkW`）。

**Spec:** `docs/design/2026-10-28-dsh-launcher-design.md` §7.7（层表与两个实测坑）、§18.2（F 编号）

## Global Constraints

- MSRV **1.89**；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 干净。
- `o824-discovery` 保持 `#![deny(unsafe_code)]`：Windows API 只在 `machine` 模块内，该模块已有 `#[allow(unsafe_code)]`。
- 发现**永不阻塞主流程**：第 6 层有硬时间预算（默认 400 ms）与条目上限（默认 5000）。
- 候选去重仍是 realpath + 大小写不敏感（`dedupe_key`）；报告顺序仍按路径排序，`i1`/`i2` 的编号必须可重现。
- 任何一层的失败（注册表读不到、COM 初始化失败、进程打不开）都**跳过该层**，不让整个 discover 失败。
- 新的 check 编号必须进设计文档 §18.2 的表，错误码必须唯一。

## Task 1: 第 1 层 —— state.json 记住的路径

**Files:**
- Modify: `crates/o824-discovery/src/machine.rs`（`Options`、`candidate_roots`、`standard_roots`、`SourceRoot`）
- Modify: `crates/o824-discovery/Cargo.toml`（加 `o824-state`）
- Test: `crates/o824-discovery/src/machine.rs` 的 `mod tests`

**Interfaces:**
- Produces:
  - `pub enum Source { State, Registry, Volume, Process, Shortcut, Scan, Explicit }`（`SourceRoot.source`）
  - `pub struct Layers { pub state: bool, pub registry: bool, pub volumes: bool, pub process: bool, pub shortcut: bool, pub scan: bool }`，`Default` 全 `true`
  - `Options { roots, extra_dirs, home, layers: Layers, state_path: Option<PathBuf>, shortcut_dirs: Vec<PathBuf>, scan_dirs: Vec<PathBuf> }`
- 层顺序（也是优先级）：state → registry → volumes → process → shortcut → scan，最后是 `extra_dirs`（`Source::Explicit`）。

- [ ] **Step 1: 写失败的测试**（`machine.rs` 内）

`a_root_remembered_in_the_state_file_is_found_without_the_registry`：建 `TempDir` 当 data root，写 `state.json` 含 `{"installs":{"x":{"root":"<fixture>"}}}`；`Options{ layers: Layers{ registry:false, volumes:false, process:false, shortcut:false, scan:false, ..default }, state_path: Some(data_root.join("state.json")), ..default }`；断言 `candidate_roots` 的第一项 `root == fixture` 且 `source == Source::State`，且 `discover` 得到的 install 版本正确。

`a_state_root_and_a_registry_root_collapse_to_one`：同一路径同时由 state 与 `roots: Some(vec![...])` 给出时，只保留一项且 `source == Source::Explicit`（显式优先于记忆）。

- [ ] **Step 2: 跑测试确认失败**：`cargo test -p o824-discovery` → 编译失败（`Source`/`Layers` 不存在）。

- [ ] **Step 3: 实现**：`SourceRoot` 加 `source: Source`；`sorted_unique_roots` 保留首个并记录其 source；`state_roots(path)` 读 `o824_state::load` 的 `installs` 里的 `root`；`standard_roots(layers, state_path)` 按上面的顺序拼。

- [ ] **Step 4: 跑测试确认通过**：`cargo test -p o824-discovery`。

- [ ] **Step 5: 提交**：`git commit -m "discovery: remember the installation the state file names"`

## Task 2: 第 4 层 —— 运行中进程的 image path

**Files:**
- Modify: `crates/o824-discovery/src/machine.rs`
- Test: 同上

**Interfaces:**
- Produces: `pub fn running_host_roots() -> Vec<String>`（每个进程的可执行文件目录，文件名大小写不敏感等于 `EXE_NAME`）与 `pub fn host_processes() -> Vec<RunningHost>`，`RunningHost { pid: u32, image: String }`（第 4 层给后面的共存检测留证据）。

- [ ] **Step 1: 写失败的测试**

`a_running_host_process_contributes_its_directory`：把 `C:\Windows\System32\cmd.exe` 复制成 `<tmp>\fake\DeepSeek Harness.exe`，`spawn` 它并让它睡 30 秒（`cmd /c timeout 30`）；轮询 `running_host_roots()` 直到出现 `<tmp>\fake`（最多 5 秒）；杀掉子进程；断言 `host_processes()` 里该 pid 的 image 以 `<tmp>\fake` 结尾。

- [ ] **Step 2: 跑测试确认失败**（函数不存在）。

- [ ] **Step 3: 实现**：`CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS)` → `Process32FirstW/NextW` 取 pid；对每个 pid `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `QueryFullProcessImageNameW`（打不开就跳过）；文件名匹配 `EXE_NAME` 者取其父目录。`discover` 在 `layers.process` 打开时把它们并入。

- [ ] **Step 4: 跑测试确认通过**。

- [ ] **Step 5: 提交**：`git commit -m "discovery: a running installation is a source of truth"`

## Task 3: 第 5 层 —— 快捷方式

**Files:**
- Modify: `crates/o824-discovery/src/machine.rs`
- Test: 同上

**Interfaces:**
- Produces: `pub fn shortcut_roots(dirs: &[PathBuf]) -> Vec<String>`；`default_shortcut_dirs() -> Vec<PathBuf>`（桌面 + 开始菜单，用户级与全局）。

- [ ] **Step 1: 写失败的测试**

`a_shortcut_pointing_at_a_root_is_followed`：用 COM 在 `TempDir` 里**创建**一个 `.lnk` 指向 fixture 根（测试辅助 `fn write_shortcut(link: &Path, target: &Path)`，用 `IShellLinkW` + `IPersistFile::Save`）；然后 `Options{ layers: only shortcut, shortcut_dirs: vec![tmp] }`，断言候选里出现 fixture 根且 `source == Source::Shortcut`。

- [ ] **Step 2: 跑测试确认失败**。

- [ ] **Step 3: 实现**：`CoInitializeEx(COINIT_APARTMENTTHREADED)`（已初始化则忽略 `RPC_E_CHANGED_MODE`）→ `CoCreateInstance(&ShellLink)` → `IPersistFile::Load` → `GetPath`；以 `.lnk` 结尾的文件才读；目标文件必须存在（`GetPath` 拿到的是 exe 或目录，取父目录/自身）；每个目录读一层，不递归。

- [ ] **Step 4: 跑测试确认通过**。

- [ ] **Step 5: 提交**：`git commit -m "discovery: follow a shortcut to the installation it names"`

## Task 4: 第 6 层 —— 受限扫盘

**Files:**
- Modify: `crates/o824-discovery/src/machine.rs`
- Test: 同上

**Interfaces:**
- Produces: `pub fn scan_roots(dirs: &[PathBuf], budget: Duration, max_entries: usize) -> Vec<String>`；`default_scan_dirs() -> Vec<PathBuf>`（各固定卷的 `Program Files`、`Program Files (x86)`、`%LOCALAPPDATA%\Programs`）。判据：某目录同时含 `EXE_NAME` 与 `resources\app.asar`。

- [ ] **Step 1: 写失败的测试**

`a_custom_install_directory_is_found_by_the_bounded_scan`：造 `<tmp>\Vendor\DeepSeek Harness\{DeepSeek Harness.exe, resources\app.asar}`；`scan_roots(&[tmp], 400ms, 5000)` 必须含该目录。
`the_scan_stops_at_the_entry_budget`：造 50 个空目录，`max_entries: 5`，断言不 panic 且返回空（预算耗尽即停）。

- [ ] **Step 2: 跑测试确认失败**。

- [ ] **Step 3: 实现**：显式栈的广度优先，深度上限 3；每进入一个目录先检查 `Instant::now() >= deadline` 与 `visited >= max_entries`；不可读目录跳过。

- [ ] **Step 4: 跑测试确认通过**。

- [ ] **Step 5: 提交**：`git commit -m "discovery: a bounded scan for installations in unexpected places"`

## Task 5: 第 7 层 —— 孤儿 profile 成为一条 finding

**Files:**
- Modify: `crates/o824-discovery/src/lib.rs`（`pub fn orphan_profiles(home: &Path, installs: &[Install]) -> Vec<String>`）
- Modify: `crates/o824-verify/src/lib.rs`（新检查 `F27` / `E_ORPHAN_PROFILE`）
- Modify: `docs/design/2026-10-28-dsh-launcher-design.md` §18.2 表（新增一行）
- Test: `crates/o824-verify/src/lib.rs` 与 `crates/o824-discovery/src/lib.rs`

**Interfaces:**
- Produces: `orphan_profiles` 返回 `profiles\*` 里存在、但**没有任何 install** 时被视为孤儿的 profile 名（`installs` 非空时返回空）。
- `verify` 增加 `CheckId::F27`，`code == "E_ORPHAN_PROFILE"`，`severity == warning`，`category == "environment"`，`params.profiles` 是名字数组。

- [ ] **Step 1: 写失败的测试**

`an_orphan_profile_is_reported_when_no_install_exists`：DSH home 里造 `profiles\desktop`，install 列表为空 → 断言 finding `F27` 且 `params.profiles == ["desktop"]`。
`no_orphan_when_an_install_exists`：同样的 home 但给一个 install → 断言没有 `F27`。

- [ ] **Step 2: 跑测试确认失败**。

- [ ] **Step 3: 实现** 并在设计文档 §18.2 补 `F27` 一行（症状：有 profile 无安装；判据：profile 目录存在且无任何 install；可修性：C，只诊断；错误码 `E_ORPHAN_PROFILE`）。

- [ ] **Step 4: 跑测试确认通过**，并跑 `cargo test --workspace`（golden 不应变化：八棵 fixture 树都有 install）。

- [ ] **Step 5: 提交**：`git commit -m "discovery: a profile without an installation is a finding, not a guess"`

## 自检

- §7.7 第 1/4/5/6/7 层各有一个任务；第 2/3 层已存在。
- "两个实测坑"（Publisher 为空、DisplayName 带版本号）第 2 层已经处理，本计划不重复。
- 每层的测试都只依赖临时目录与本机已有程序，不需要真装 DSH。
