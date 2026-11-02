# o824-mock npm 包 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 macOS/Linux 上的 UI 开发者用 `npm i -D <release-tarball>` + `npx o824-mock` 就能对着录制开发界面，跑的是与内核同源的那一份回放实现。

**Architecture:** 一份 Rust 代码（`o824_mock::Transcript::replay`）编出两个产物：宿主 exe 与 `wasm32-wasip1` 模块。npm 包 = wasm + 一层只负责 stdio/预开目录/退出码的 launcher + 三个内置录制。launcher 里没有任何协议逻辑，所以不存在"第二份实现"。

**Tech Stack:** Rust（`o824-mock` crate，无新依赖）、Node ≥ 20 的 `node:wasi`（preview1）、`node --test`（无第三方测试依赖）、PowerShell 构建脚本、GitHub Actions。

**Spec:** `docs/design/o824-mock-package.md`（已批准，2026-11-02）

## Global Constraints

- MSRV **1.89**；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 必须干净。
- **不新增 Rust 依赖**：`o824-mock` 现有依赖（errs / model / protocol / serde）足够。
- **不新增 npm 依赖**：测试用 Node 内置的 `node --test`、`node:child_process`、`node:test`。
- npm 包 `engines.node` = `>=20`；launcher 必须在 Node 20 与 24 上都能跑。
- 帧上限 **64 MiB**（`o824_protocol::MAX_FRAME_SIZE`），transcript 单行上限 **8 MiB**（`o824_mock` 内部常量）。
- wasm 产物**不入库**；`packages/o824-mock/o824-mock.wasm` 与 `dist/` 进 `.gitignore`。
- 包版本必须等于发布的 tag（`v0.1.2` → `0.1.2`）。
- 退出码沿用 CLI 契约：`0` 正常、`2` 用法、`8` 内部/transcript 不可读。

## Review Focus

规格没写、但一个人真用起来最可能踩的五件事，每件都在对应任务里有一条测试钉住：

1. **带空格或非 ASCII 的 transcript 路径**（Windows 路径几乎必然带空格）：launcher 必须原样传递，全程不经过 shell。→ Task 2
2. **从别的目录传相对路径**：解析必须相对调用者的 CWD，不是包目录。→ Task 2
3. **客户端提前关闭管道 / 杀掉 mock**：不能让进程挂住或吐栈。→ Task 2
4. **一行超过 64 KiB 的 transcript**（plan 结果带指纹与步骤表）：不能被行缓冲截断。→ Task 1
5. **Node < 20 或没有 WASI**：要一句人话 + 退出码 2，不是一段栈。→ Task 2

---

### Task 1: `o824-mock` 独立入口（宿主 exe）

**Files:**
- Create: `crates/o824-mock/src/bin/o824-mock.rs`（最小验证时已写，本次补测试并定稿）
- Modify: `crates/o824-mock/Cargo.toml`（加 `[[bin]]`）
- Test: `crates/o824-mock/tests/bin.rs`

**Interfaces:**
- Consumes: `o824_mock::{Transcript, Options}`、`o824_protocol::{read_frame, write_frame}`。
- Produces: 一个可执行文件 `o824-mock`，参数 `--transcript <path> [--delay <ms>] [--fail-at <n>] [--protocol <ver>]`；stdin 读帧、stdout 写帧；退出码 0/2/8。

- [ ] **Step 1: 写失败的测试** `crates/o824-mock/tests/bin.rs`

用 `env!("CARGO_BIN_EXE_o824-mock")` 拿到被测二进制，写四条：
（a）`the_bundled_transcripts_replay_frame_for_frame`：对 `spec/transcripts/*.jsonl` 三个文件，把录制里的请求帧喂进去，断言产出的帧与录制里的服务端消息逐帧相等。
（b）`a_usage_error_exits_two`：不带 `--transcript`，断言 `code() == Some(2)` 且 stderr 含 `--transcript`。
（c）`an_unreadable_transcript_exits_eight`：`--transcript Z:\nope.jsonl`，断言 `code() == Some(8)`。
（d）`a_line_longer_than_the_line_cap_is_reported_not_truncated`：写入一条 9 MiB 的合法 JSON 行做 transcript，断言退出码 8、stderr 含 `8 MiB`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p o824-mock --test bin`
Expected: 编译失败或 FAIL（bin 尚未进 Cargo.toml 的显式目标 / 断言不符）

- [ ] **Step 3: 定稿 bin**

`crates/o824-mock/src/bin/o824-mock.rs`：解析参数（未知参数 → 2；`--transcript` 缺失 → 2），`Transcript::load` 失败 → 8，`replay` 失败 → 8，其余原样返回。`Cargo.toml` 加：

```toml
[[bin]]
name = "o824-mock"
path = "src/bin/o824-mock.rs"
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p o824-mock`
Expected: 全绿，包括原有 10 条库测试

- [ ] **Step 5: 提交**

```bash
git add crates/o824-mock
git commit -m "mock: a standalone entry point the package can ship"
```

---

### Task 2: launcher 的纯逻辑（`lib/launcher.mjs`）

**Files:**
- Create: `packages/o824-mock/lib/launcher.mjs`
- Create: `packages/o824-mock/package.json`
- Test: `packages/o824-mock/test/launcher.test.mjs`

**Interfaces:**
- Produces:
  - `export const BUNDLED_TRANSCRIPTS = ['doctor-clean', 'doctor-findings', 'plan-apply']`
  - `export class LauncherError extends Error { constructor(message, exitCode = 2) }`
  - `export function describeTranscript(argv, packageDir) -> { argv: string[], preopen: string }`，抛 `LauncherError`
  - `export function requireNode20(version) -> null | string`（返回一句人话，或 null 表示合格）

- [ ] **Step 1: 写失败的测试** `packages/o824-mock/test/launcher.test.mjs`

```js
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { describeTranscript, requireNode20, LauncherError } from '../lib/launcher.mjs';

const pkg = '/pkg';

test('a bundled name maps into the package transcripts', () => {
  const r = describeTranscript(['--transcript', 'doctor-clean'], pkg);
  assert.deepEqual(r.argv, ['--transcript', '/t/doctor-clean.jsonl']);
  assert.equal(r.preopen, '/pkg/transcripts');
});

test('an absolute path is preopened from its own directory', () => {
  const r = describeTranscript(['--transcript', '/work/session.jsonl'], pkg);
  assert.deepEqual(r.argv, ['--transcript', '/t/session.jsonl']);
  assert.equal(r.preopen, '/work');
});

test('a relative path resolves against the caller working directory', () => {
  const r = describeTranscript(['--transcript', 'spec/transcripts/x.jsonl'], pkg);
  assert.equal(r.preopen, resolve(process.cwd(), 'spec/transcripts'));
});

test('a path with spaces and non-ascii survives untouched', () => {
  const r = describeTranscript(['--transcript', 'D:/开发 目录/session.jsonl'], pkg);
  assert.equal(r.preopen, resolve('D:/开发 目录'));
  assert.equal(r.argv[1], '/t/session.jsonl');
});

test('other flags pass through unchanged', () => {
  const r = describeTranscript(['--transcript', 'doctor-clean', '--delay', '100'], pkg);
  assert.deepEqual(r.argv, ['--transcript', '/t/doctor-clean.jsonl', '--delay', '100']);
});

test('an unknown name is refused here, not by the guest', () => {
  assert.throws(() => describeTranscript(['--transcript', 'nope'], pkg), (e) =>
    e instanceof LauncherError && e.exitCode === 2 && /doctor-clean/.test(e.message));
});

test('a missing --transcript is a usage error', () => {
  assert.throws(() => describeTranscript([], pkg), (e) => e.exitCode === 2);
});

test('node older than 20 is refused with a sentence', () => {
  assert.match(requireNode20('18.19.0'), /Node 20/);
  assert.equal(requireNode20('20.0.0'), null);
  assert.equal(requireNode20('24.8.0'), null);
});
```

- [ ] **Step 2: 跑测试确认失败**

Run: `node --test packages/o824-mock/test`
Expected: FAIL — `Cannot find module '../lib/launcher.mjs'`

- [ ] **Step 3: 实现 `packages/o824-mock/lib/launcher.mjs`**

规则：`--transcript` 的值以 `.jsonl` 结尾或含 `/`/`\` → `resolve()` 后预开其目录；否则必须是 `BUNDLED_TRANSCRIPTS` 里的名字（不认识就抛，消息里列出可用名）。其余 argv 原样保留。`package.json` 的 `name` = `o824-mock`、`bin` 指 `bin/o824-mock.mjs`、`engines.node` = `>=20`、`type` = `module`、`files` = `["bin","lib","o824-mock.wasm","transcripts","README.md"]`。

- [ ] **Step 4: 跑测试确认通过**

Run: `node --test packages/o824-mock/test`
Expected: 8 个用例全绿

- [ ] **Step 5: 提交**

```bash
git add packages/o824-mock
git commit -m "mock: the launcher's path rules, as pure functions with tests"
```

---

### Task 3: launcher 入口与端到端回放

**Files:**
- Create: `packages/o824-mock/bin/o824-mock.mjs`
- Test: `packages/o824-mock/test/replay.test.mjs`

**Interfaces:**
- Consumes: `describeTranscript`、`requireNode20`、`LauncherError`（Task 2）。
- Produces: 可执行入口；`node packages/o824-mock/bin/o824-mock.mjs --transcript <name|path>` 在 stdout 吐帧，退出码 0/2/8。

- [ ] **Step 1: 写失败的测试** `packages/o824-mock/test/replay.test.mjs`

用一个 `run(args, payload)` 帮助函数 spawn `process.execPath` 跑入口，返回 `{status, stdout, stderr}`；一个 `frames(buffer)` 帮助函数按 Content-Length 切帧。断言：

```js
test('every bundled transcript replays frame for frame', () => { /* 三个文件逐个比对录制里的服务端消息 */ });
test('fail-at replaces one message and keeps the sequence gapless', () => { /* 第 3 条是 -32603 / E_MOCK_FAILURE，seq 连续 */ });
test('an unsupported method is answered with -32601 naming what is supported', () => {});
test('a closed stdout does not crash the process', () => { /* 传不足的请求后立刻 end 输入，断言能正常退出 0 */ });
test('the wasm and the host binary agree', () => { /* 若 dist 里没有宿主 exe 则 skip 并说明 */ });
```

- [ ] **Step 2: 跑测试确认失败**

Run: `node --test packages/o824-mock/test`
Expected: replay 用例 FAIL（入口不存在或 wasm 不存在）

- [ ] **Step 3: 实现入口**

`bin/o824-mock.mjs`：`--help` 打印用法；读 `process.versions.node` 过 `requireNode20`（不合格 → stderr 一句话 + exit 2）；`describeTranscript(process.argv.slice(2), packageDir)`；`new WASI({ version: 'preview1', args: ['o824-mock', ...argv], env: process.env, preopens: { '/t': preopen }, returnOnExit: true })`；`wasi.start(instance)` 的返回值作为退出码；`LauncherError` 打印 message 并用其 `exitCode` 退出；wasm 缺失时一句话 + exit 8。只静音 `WASI is an experimental feature` 这一条警告。

- [ ] **Step 4: 跑测试确认通过**

Run: `pwsh -File scripts/build-mock-package.ps1 -Version 0.0.0-test` 然后 `node --test packages/o824-mock/test`
Expected: 全绿（此步依赖 Task 4 的脚本；若尚未写，先手工把 wasm 与 transcripts 放进包目录再跑）

- [ ] **Step 5: 提交**

```bash
git add packages/o824-mock
git commit -m "mock: the package entry point, proven against the recordings"
```

---

### Task 4: 构建与打包脚本

**Files:**
- Create: `scripts/build-mock-package.ps1`
- Modify: `.gitignore`（忽略 `packages/o824-mock/o824-mock.wasm`、`packages/o824-mock/transcripts/`、`dist/`）
- Test: 脚本自身的断言 + Task 3 的端到端用例

**Interfaces:**
- Produces: `dist/o824-mock-<ver>.tgz`、`dist/o824-mock.exe`、`dist/SHA256SUMS`；脚本以非 0 退出表示任一步失败。

- [ ] **Step 1: 写失败的测试**

在 `packages/o824-mock/test/package.test.mjs` 里写一条：跑完构建后 `npm pack --dry-run --json` 的输出里必须含 `o824-mock.wasm`、`bin/o824-mock.mjs`、`lib/launcher.mjs` 与三个 `transcripts/*.jsonl`，且 `package.json` 的 version 等于传入的版本；wasm 缺失时打包必须失败（用临时删掉 wasm 再跑一次来断言）。

- [ ] **Step 2: 跑测试确认失败**

Run: `node --test packages/o824-mock/test`
Expected: FAIL（脚本不存在）

- [ ] **Step 3: 实现脚本**

`scripts/build-mock-package.ps1 -Version <ver>`：
1. `cargo build -p o824-mock --bin o824-mock --release`，把 exe 复制到 `dist/o824-mock.exe`；
2. `cargo build -p o824-mock --bin o824-mock --target wasm32-wasip1 --release`，复制 wasm 到包目录；
3. 复制 `spec/transcripts/*.jsonl` 到 `packages/o824-mock/transcripts/`；
4. 改 `package.json` 的 `version`；
5. 断言 wasm 存在（不存在就 `throw`，**不产出坏包**）；
6. `npm pack --pack-destination dist`；
7. 写 `dist/SHA256SUMS`（覆盖 tgz 与 exe）；
8. 打印下一步（建 Release 拖哪两个文件、tag 命令）。

- [ ] **Step 4: 跑一遍并验证产物**

Run: `pwsh -File scripts/build-mock-package.ps1 -Version 0.1.2`
Expected: `dist/` 里三个文件；`node --test packages/o824-mock/test` 全绿

- [ ] **Step 5: 提交**

```bash
git add scripts/build-mock-package.ps1 .gitignore packages/o824-mock
git commit -m "mock: one script builds the tarball, the exe and the checksums"
```

---

### Task 5: CI job（Node 20 与 24 双版本）

**Files:**
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: Task 4 的脚本与 Task 3 的测试。
- Produces: job `mock-package`；失败时用与 `cargo test` 那一步相同的方式把失败用例名打成 annotation。

- [ ] **Step 1: 加 job**

`mock-package`：matrix `node: [20, 24]`，步骤为 checkout → setup-node → rust-toolchain（带 `wasm32-wasip1` target）→ `pwsh scripts/build-mock-package.ps1 -Version 0.0.0-ci` → `node --test packages/o824-mock/test` → 失败时把失败用例名打成 `::error::`（复用现有写法）。

- [ ] **Step 2: 本地验证 workflow 语法**

Run: `python -c "import yaml;d=yaml.safe_load(open('.github/workflows/ci.yml'));print(list(d['jobs']))"`
Expected: 打印出两个 job 名

- [ ] **Step 3: 推送并观察**

Run: `git push origin main`，然后用 checks API 确认 `mock-package` 的 Node 20 与 24 两格都 success。

- [ ] **Step 4: 提交（若推送前还需修）**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: build and test the mock package on node 20 and 24"
```

---

### Task 6: 文档与发布清单

**Files:**
- Modify: `packages/o824-mock/README.md`（新建）
- Modify: `README.md`（加"用 mock 开发 UI"一节）
- Modify: `docs/design/2026-10-28-dsh-launcher-design.md`（§19.5 措辞）
- Create: `dist/RELEASE.md`（由脚本生成，或脚本打印）

**Interfaces:**
- Consumes: 前五个任务的产物名与安装命令。
- Produces: 一份人能照做的发布清单。

- [ ] **Step 1: 写包 README**

必须包含：三条安装方式（Release tarball / 将来 registry / 仓库内 `file:`）、三个内置录制的名字、`--delay/--fail-at/--protocol` 的说明、那段 20 行的 Node 客户端示例、以及"同一段客户端代码把命令换成 `o824` 就能对着真内核跑"这句。

- [ ] **Step 2: 改主 README 与设计文档 §19.5**

§19.5 的那句"`npx o824-mock`"改成两条实际可用的安装方式，并注明跨平台由 WASI 实现。

- [ ] **Step 3: 验证文档里的命令真能跑**

Run: 在一个空目录里 `npm i -D <dist 里的 tgz>`，然后 `npx o824-mock --transcript doctor-clean` 喂请求，断言 7 帧。
Expected: 帧与录制一致

- [ ] **Step 4: 提交**

```bash
git add packages/o824-mock/README.md README.md docs/design
git commit -m "docs: how a UI developer installs and drives the mock"
```

---

## 自检

**Spec 覆盖**：§2 形状 → Task 1/2/3；§3 launcher 契约 → Task 2（规则）+ Task 3（接线）；§4 不漂移 → Task 5（三方比对）；§5 构建 → Task 4；§6 发布 → Task 4（产物）+ Task 6（清单）；§7 取舍 → `.gitignore` 与 Task 4 Step 3 的"不出坏包"断言；§8 文档 → Task 6；§9 判据 → Task 3/4/5/6 的验证步骤。

**未覆盖**：§10 的"Release 由用户上传"不是代码任务，落在 Task 4 Step 3 第 8 项的打印清单里。

**比例**：六个任务对应六个可独立验收的产物；没有把实现写成转录。
