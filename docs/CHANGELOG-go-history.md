# 变更说明：P0 内核合并之后的这一批（2026-11-01）

基线：`d55cd35`（13 个任务的 P0 内核合并进 `main`）。
本说明覆盖 `d55cd35..118d62a` 的 19 个提交、37 个文件、+4486/−465 行。

分四类：**实验室发现的真 bug**、**真机验证推翻的判据**、**新增能力**、**文档与 CI**。
最后两节是**已验证**与**未验证**的清单——未验证的部分不是"应该没问题"，是"还没证据"。

---

## 1. 实验室发现的真 bug（都已在真机配置副本上复现并修复）

### 1.1 `pnpm-workspace.yaml` 读错了目录（`4a8c984`）

pnpm 的白名单在 `profiles\<profile>\pnpm-workspace.yaml`，代码读的是 `<home>\pnpm-workspace.yaml`。
后果有两层：

- 装了 git 插件的真实安装会**误报** F17（读不到白名单）；
- `repair` 会**新建**一个 `<home>\pnpm-workspace.yaml`，pnpm 根本不看它——doctor 变绿，问题还在。

修复：F17、F16 的白名单检查、`allow-builds` 步骤（改为每个 profile 一步）、快照范围全部改到 profile 相对路径；fixtures 跟着搬家。

### 1.2 F01 会朝"不是插件的目录"重建链接（`2aa261a`）

`rebuild-link` 原先只要目标目录存在就重建。而 F01 的定义就是"目标在、但里面没有 `package.json`"——重建到同一处只是把故障复制一遍，还白做一次快照和链接替换。
修复：`Build` 只在目标里真有 `package.json` 时才发这一步；否则只报 finding 并给提示。

### 1.3 `repair` 丢了 F16 的下一步提示（`2aa261a`）

"修完重新体检"那次改动把 `E_PROFILE_PARSE` 的提示分支弄丢了，用户看到一条 finding 却不知道要先建快照。已补回，F01 也加了对应提示。

### 1.4 junction 替换没有可验证的证据（`c59ac92`）

新增 `internal/cli/exe_test.go`：**构建真二进制**、在 F01 树上跑 `repair`、再断言"目标目录里的文件能通过链接读到"。
这条测试的意义不只是防回归——它确立了"Win32 调用必须在真 exe 里验证，进程内测试不算数"。

---

## 2. 真机验证推翻的判据（结论是"这条检查没有对应的真实故障"）

### 2.1 F03 降级为提示（`73290bc` + `5989fe2`）

在真机 DSH 0.2.1-rc.3 上实测：

- `cordis.patch.yml` 的 `id` 指的是 **loader entry id**，不是 bundle 包名；指向不存在的 entry 只打 `patch: entry "X" not found` 警告，**进程照常启动**；
- 用 `insert` 造出真正重复的 entry id，`--dump-config` 合成阶段不报错，**真启动（web profile，服务正常监听）也不失败**：指向不存在的模块只得到 `2 entries did not activate` 警告，指向同一个真实模块完全静默；
- `app.asar` 里根本没有 `duplicate loader entry id` 这个字符串。

结论：原判据（patch 行 id 与 bundles 求交集）不成立，而且**会误删用户写的覆盖配置**。
处理：F03 改成 `severity: warning`、去掉 `repair`、`plan` 不再生成 `dedupe-loader`；设计文档 §18.2/§18.3/§19.1 同步修订。P0 的端到端验收用例从 `dup-loader` 换成 `git-dep`（F17）。

### 2.2 F17 的修复本身是错的（`41e37f6` + `ad9a75b` + `118d62a`）

用真 pnpm 11.28.5 实测：

| 我们原来的写法 | 真 pnpm 的反应 |
|---|---|
| `allowBuilds: [pkg]`（list） | 硬报错 `The allowBuilds field should be an object, but got array` |
| `onlyBuiltDependencies: [pkg]` | v11 已移除该键，无效 |
| `allowBuilds: {pkg: true}`（裸包名） | 能被解析，但 **git 依赖仍然不批**——文档明确：裸包名永远批不下 git/tarball 依赖 |

也就是说：**原来的"修复"会让一个本来能用的 workspace 文件变成 pnpm 一读就报错**，比不修更糟。
正确写法（已实测通过）：map 形式 + **仓库键** `"name@git+https://host/org/repo.git": true`（不带 `#ref`，带 commit 也认）。
另外 `pnpm rebuild` / `rebuild --pending` **不会**重跑被拦下的构建，只有 `install` 会。

处理：

- 检测：git/tarball 依赖必须用仓库键才算批准，裸包名不算；旧 list 形式不批准但也不让 doctor 崩；finding 新增 `Params["key"]`。
- 修复：写 map 形式、必要时把旧 list 就地迁移成 map、然后跑 `plugin --profile <p> install`（不是 rebuild）。
- 该动作会动 `node_modules`，快照救不回来，所以放在 `--with-install` 后面，默认不做。（2026-11-01 已解决：F17 改走 §7.2 的暂存重装，不再原地 install。）

---

## 3. 新增能力

### 3.1 自动快照（`a9045b3` + `8f2df0a`）

- `snapshot.CreateIfChanged`：当前 scope 的 `rel -> sha256` 与最新一份快照一致就什么都不写，否则按原流程原子写入。
- `snapshot create --if-changed`、`doctor --save-snapshot`（**只在零 finding 时**拍，坏树的快照比没有更糟；不带 flag 时 doctor 严格只读）。
- 效果：F16（profile 写坏）从"没辙"变成"有检查点就能自动修"。实验室闭环已验证：干净树拍快照 → 写坏 `cordis.yml` → `repair --yes` → 恢复原样、doctor 干净。

### 3.2 F17 补全：`--with-install`（`2c73428` + `ce02468`）

`repair --with-install` 在写完白名单后追加一步 `install-packages`：用 argv 调宿主
`DeepSeek Harness.exe --expose-internals <cli.js> plugin --profile <p> install`，
环境变量 `ELECTRON_RUN_AS_NODE=1`，10 分钟超时，失败时把 pnpm 的 stderr 尾部写进 journal；
失败会回滚白名单，并尽量恢复 `node_modules` 的链接表。
这条路径已被 §7.2 取代：F17 现在也走“副本里 install + 两次改名交换”的 `reinstall-profile`，原地 install 的步骤已删除，
“包内被构建脚本改写的文件救不了”这条限制随之消失（见 §7.4 “已解决”）。

### 3.3 A 组收尾（`8a30a30` + `0e9a54d` + `519bdeb`）

- `o824 --version`（版本/提交号/构建时间走 `-ldflags -X` 注入）；
- 中文 `README.md`（六条命令、退出码表、findings 表、已知限制）；
- `diagnose --out <zip>` 诊断包（默认脱敏，zip 原子写，`--json` 只输出 manifest）；
- GitHub Actions CI（windows-latest：vet → test → build）。

---

## 4. 已验证（有命令输出为证）

| 项 | 证据 |
|---|---|
| 真机健康树零误报 | `o824 doctor --install i1` → `no findings`，退出 0 |
| F03 在真机不成立 | `dsh --dump-config` / 真启动 web profile 均不失败（见 §2.1） |
| F17 的键格式 | 本地 git 仓库 `git+file://` 装包 → `IGNORED_BUILDS` → 写仓库键 → `install` → `postinstall: Done`、`built.txt` 生成 |
| F17 的 map 形式 | 写 list 硬报错；写 map 通过（同上） |
| 自动快照闭环 | 拍快照 → 写坏 → `repair` 恢复 → `doctor` 干净 |
| F01 不再无效重建 | 实验室 `f01`：`repair` 拒绝、提示重装、文件一个字节没动 |
| 全量测试 | `go vet ./...` 干净；13 个包 `go test ./... -count=1` 全绿 |

## 5. 未验证（**没有证据**，不要当成"已通过"）

1. **经 o824 端到端跑通 F17 的构建重跑**：行为本身已实测——写 `allowBuilds: {"pkg@git+<url>": true}` 后跑 install 会重新评估审批并得到 `postinstall: Done`；但“写白名单 → 副本里 `install --frozen-lockfile` → pnpm 跑掉被批准的脚本 → 交换”这一整条还没有在真机上跑过。`reinstall-profile` 本身只用真 pnpm 验过 F02 形态（§7.3）。原来的 `install-packages` 尝试因进程树/沙箱原因未证实，已随 §7.2 删除。
2. **真实 git 插件**（GitHub 上的真包）：只用了本地 `git+file://` 仓库，没有走过真正的 `git+https://` 拉取、没有走 codeload 路径。
3. **F03 在其它 DSH 版本**：只测了 0.2.1-rc.3。判据是否在别的版本成立，未知。
4. **F16/F20 在真实故障上**：只用实验室树验证过；没有真的把一台机器升级打断再恢复。
5. **锁的跨用户互斥**：仍是 per-data-root（`%LOCALAPPDATA%`），同一台机器不同 Windows 用户之间不互斥——这是已知的 spec 偏离，未修。
6. **`diagnose` 包在真实故障上的可读性**：只验证了结构、脱敏和原子写，没验证"维护者拿到它能不能定位问题"。
7. **CI**：workflow 写好了，但仓库没有远端，**从未真正跑过**。

## 6. 仍然成立的老限制

- 只支持 Windows x64。
- 七个检查都能诊断，但能自动修的只有：F17（写白名单；`--with-install` 时走暂存重装真跑构建）、F16（有快照时）、F04（清单能唯一定位时）。
- F01/F02 默认只诊断：不加 `--with-install` 时不会自动修，自动重建插件闭包（不依赖用户开启安装）仍是 P1。
- 快照不含 `node_modules`（按 §16.1），所以只靠快照回滚不了的修复不能默认做；`--with-install` 的重装不在其列——它在副本里装、成功后才两次改名交换，失败时原树不动（见 §7）。
- 发现算法只做了注册表 + 固定卷 + 显式路径（spec §7.7 的第 1/4/5/6 层未做）。

---

## 7. 追加：F01/F02 的"重装依赖"修复（`378c41e` + `ef97a1d` + `b5ca13d`）

### 7.1 设计文档补了 §15.1.1

诊断不得依赖宿主（宿主正是坏的），但**修复可以委托宿主重新物化依赖**：只在 `apply` 的修复步骤里、必须用户显式开启（`--with-install`）、argv 直调宿主 pnpm、工作目录是副本、成功后才交换。

### 7.2 实现

- `plan` 新增 `reinstall-profile` 步骤：`--with-install` 时，F01/F02 的 finding 映射成"每个 profile 一步"，替代原来那个"重建到同一目标"的 `rebuild-link`；不开这个 flag 时行为完全不变。
- `apply` 的 `reinstallProfile`：**预检空间**（2× node_modules + 256 MiB，不够报 `E_DISK_FULL`）→ 建 `<profile>.o824-stage`（复制除 `node_modules` 和点目录外的一切，junction 重建不跟随）→ 在副本里跑 `pnpm install --frozen-lockfile`（15 分钟超时，stderr 尾部进 journal）→ 校验副本里有 `node_modules` → **两次改名交换**（旧树先变 `.o824-old`，成功后删；任何一步失败都改回来）。
- 失败时原 `node_modules` 一个字节不动——§16.1 之所以不快照 `node_modules`，正是因为它可以由 lockfile 重建，这条现在兑现了。
- `plan`：`--with-install` 时 F17 也走同一条暂存路径——先 `allow-builds`（写白名单）再一步 `reinstall-profile`，不再有 `install-packages`。同一 profile 的多个 F17、以及同 profile 的 F01/F02，共用这一步（按 profile 去重）；不开 flag 时 F17 仍只写白名单，行为不变。白名单写入必须排在暂存安装之前，副本才带得上刚批准的 `pnpm-workspace.yaml`——`plan` 测试钉住这个顺序。
- `apply`：`install-packages` 步骤及其 spawn 路径删除，连带只为它存在的 `node_modules` 链接表保护；F17 的重跑构建改由 `reinstallProfile` 的 `install --frozen-lockfile` 完成（副本会带上白名单，pnpm 因此重新评估审批并跑掉被拦的脚本）。

### 7.3 端到端验证（**这次是真跑通的**）

实验室树：真实 profile 的五个配置文件 + 一条指向已删除目录的 `node_modules/dshmarket` junction。

```
$ o824 doctor --root "<真机 app>" --home "<lab home>"
F02  error  E_BUNDLE_MISSING  path=profiles\desktop\node_modules\dshmarket target=..\gone

$ o824 repair … --yes --with-install
applied 1 step(s)
  reinstall-profile  …\home\profiles\desktop        exit=0，14 秒

$ o824 doctor …            → no findings，exit=0
node_modules 顶层条目：80（与真实 profile 一致）
dshmarket 现在是真包目录（LICENSE / README…），不再是悬空 junction
快照：修复前自动留下 20261101T102909Z-f525
```

14 秒（比预估的 35 秒更快）的原因是 pnpm store 预热且放在同一块盘上；冷 store 的机器就是网络时间。

### 7.4 这条验证覆盖了什么、没覆盖什么

- ✅ 覆盖：F02 → `reinstall-profile` → 真 pnpm 安装 → 交换 → 复检干净，整条链路。
- ✅ 覆盖：失败路径由 16 个单元测试钉住（stub 的 install runner、空 stage、空间不足、两次改名失败回滚），原树逐字节不变。
- ✅ 覆盖：F17 也走这条暂存路径——`allow-builds` 后接 `reinstall-profile`，每个 profile 一步，与 F01/F02 去重；顺序（白名单先写、暂存安装在后）由 `plan` 测试固定，`apply` 测试用 stub 安装证明副本带上了审批、交换后标记文件落在 profile 里。
- ❌ 没覆盖：冷 store / 真网络（用的是本地 store 副本 + `git+file://` 级别的离线条件）。
- ❌ 没覆盖：F01（链接目标存在但不是包）的真机形态——逻辑与 F02 共用同一条路径，但没有单独造过真机样本。
- ❌ 没覆盖：F17 的构建重跑没有在真机上端到端跑过（写白名单 → 副本 install → pnpm 跑掉被批准的脚本）；行为本身已实测，见 §5 第 1 条。

**已解决：原地 install 的不可回滚。** 原限制是 `install-packages` 在真实 profile 里原地跑 pnpm：`node_modules` 不在快照范围内，失败时白名单能回滚，构建脚本已经写进自己包里的内容却救不回来。现在 `--with-install` 不再原地 install：F17 与 F01/F02 共用 `reinstall-profile`，先在 `<profile>.o824-stage` 里装、校验通过后才用两次改名换入，任何失败原 `node_modules` 一个字节不动、副本直接丢掉。成功的那次 install 仍然会跑包自己的脚本（这正是目的），可逆性来自“副本 + 交换”而不是快照。


---

## 8. F17 全链路打通，以及它暴露的五个 bug（`303b591` `0f358b4` `bf6e92e` `fb0d8d9`）

### 8.1 F17 也改走副本 + 交换

`--with-install` 不再有"就地 install"这条路径：F17 的计划变成 `[allow-builds, reinstall-profile]`，和 F01/F02 共用同一条安全路径。整条链上**没有任何动作会就地改 `node_modules`**，所以 §16.1 的回滚承诺对依赖树也成立。

### 8.2 真机跑出来的五个 bug（全部已修）

| # | 现象 | 根因 | 修法 |
|---|---|---|---|
| 1 | `ERR_PNPM_NO_LOCKFILE` | 新 profile 本来就没有 `pnpm-lock.yaml`，而我们固定加 `--frozen-lockfile` | 只在副本里真有 lockfile 时才冻结 |
| 2 | `ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY` | pnpm 清空 `node_modules` 前要确认，无 TTY 直接中止（跨卷 store 时会触发） | 固定加 `--config.confirmModulesPurge=false`；副本本来也没有 `node_modules` |
| 3 | 用户只看到 `exited with code 1` | pnpm 把报错写在 **stdout**，我们只取 stderr | stderr 为空时回退到 stdout 尾部 |
| 4 | 白名单写了却不生效 | 路径含空格时 pnpm 的键是 `git+file:///D:/dsh%20work/...`，我们写字面空格 | `approvalKeys` 用 `net/url` 规范化 |
| 5 | 带 BOM 的 `package.json` 被报 F16 | 我们直接喂给 JSON 解析器 | 读 profile 文件时剥掉前导 BOM |

### 8.3 端到端证据

```
$ o824 doctor …                 → F17，key=f17-verify-probe@git+file:///D:/dsh%20work/…
$ o824 repair … --with-install  → exit=0
    allow-builds       …\pnpm-workspace.yaml
    reinstall-profile  …\profiles\f17verify
$ cat …\node_modules\f17-verify-probe\built.txt → built      ← postinstall 真的跑了
$ o824 doctor …                 → no findings
```

可重复执行：`D:\dsh work\broken-dsh-lab\verify-f17.ps1`（自带临时 home、临时 store，不碰真实 `~/.dsh`）。

### 8.4 §7.4 的未验证清单更新

- ~~由 o824 驱动 F17 的 install 步骤~~ → **已验证**（见 8.3）。
- 仍成立：真 `git+https://`（GitHub）拉取没跑过，只测了 `git+file://`；跨卷 store 已在本机复现并修掉，但用户的 C:/C: 同卷路径没单独测。

---

## 9. 协议层 C1/C2（`7618bbe` `f8ef0df` `0f0f902` `286e8a3`）

### 9.1 已落地

- `o824 kernel --stdio`：LSP 帧（`Content-Length` 按字节）、JSON-RPC 2.0、会话内全局单调 `seq`、stderr 只走诊断。
- `o824.info`（版本/能力/平台）、`o824.discover`（真机安装列表）。
- 异步作业：`o824.doctor` 立即回 `jobId`，事件 `op.state(running)` → `op.progress` → `op.result` → `op.state(done)`；`o824.job` 幂等补取；`o824.cancel` 先答 `{cancellable,reason}` 再决定；stdin EOF 取消在跑的作业并有界等待。
- 错误码：`-32700` / `-32600` / `-32601`+`E_UNSUPPORTED` / `-32603`。

### 9.2 裁定：`op.result` 在 `op.state(done)` **之前**

§19.1 的示例 transcript 是 `done` 在前、`result` 在后。实现选择了**反过来**（result 先、done 后），理由：

- 一个照抄 §19.1 的客户端会在收到 `done` 时停止读取，那样就永远拿不到结果——示例本身是坏的；
- 反过来之后，任何客户端都能"收到 done 时结果已经在手上"；
- 这不是规范性条款（§19.2 规定的是 progress/cancel 语义），且 `o824.job` 提供了幂等补取，所以即使客户端漏掉事件也能恢复。

### 9.3 已知待办

- `o824.doctor` 带 `installId` 时仍会重跑一次全盘发现（实测 ~5 秒）。UI 每次体检等 5 秒不可接受，下一次协议切片里要加"会话内缓存上一次 discover 结果"。
- `go test -race` 在本机跑不了（没有 cgo/gcc），并发正确性靠互斥顺序审查 + 20 次重复运行验证；**建议在 CI 上加 `-race`**。

### 9.4 C3 落地与它抓到的一个 bug

`o824.plan` / `o824.apply` 已可用（作业化，带 `confirm`、`E_PLAN_UNKNOWN`、`E_PLAN_EXPIRED`、`idempotencyKey` 去重、`capabilities` 增加 `plan`/`apply`），并加了**会话内发现缓存**（30 秒 TTL，带 `--root` 时完全跳过发现，实测 plan 从 ~5 秒降到 0.1 秒）。

协议冒烟测试抓到一个真 bug（`4231e10`）：`o824.plan` 只给 `root` 时失败于 `E_USAGE: plan: the installation has no id`，而 CLI 的 `repair --root` 是好的——因为**两个选择器只有其中一个**会推导 `local-<hash>` 这个 id。已让 `selectInstallUsing` 和 `resolveInstall` 推导同一份 id，并加了回归测试。

修复后实测（真机 lab 树，走协议）：

```
plan   → RESULT steps: ['allow-builds']   expiresAt 15 分钟后
apply  → running snapshot → progress snapshot 0/1 → running apply → progress apply 0/1 → 1/1
       → 文件真的被改了：allowBuilds: dshmarket-git-test@git+https://…: true
```

### 9.5 C4：mock 内核（`b0fafd6` `8a44c6e` `8268d70`）

- `internal/mock`：JSONL transcript 回放引擎（`Load` 带行号报错、按方法回放、`id`/`seq` 重写、`Delay`/`FailAt`/`Protocol`、未覆盖方法回 `-32601` + 支持列表）。
- `o824 mock --transcript <path> [--delay ms] [--fail-at N] [--protocol ver]`，与内核**共用同一套帧编解码**（`internal/protocol/frame.go`）。
- `spec/transcripts/` 三份手写 transcript：`doctor-clean`、`doctor-findings`（一条 F17 error + 一条 F03 warning）、`plan-apply`（两步计划 + 快照/应用阶段）。
- `internal/mock/conformance_test.go`：一致性骨架——用 transcript 自身当客户端回放并逐帧比对，外加"真内核与 mock 的 `o824.info` 字段一致"。

实测：

```
$ o824 mock --transcript spec/transcripts/doctor-findings.jsonl
  info/discover/doctor 三个响应按客户端 id 回放，doctor 走 running→progress→result→done
  未覆盖方法 → -32601 + E_UNSUPPORTED + ["o824.info","o824.discover","o824.doctor"]

$ … --fail-at 3
  第 3 条服务端消息变成 -32603 + E_MOCK_FAILURE（failAt:3, method:"o824.doctor"）
```

至此 §19.5 的承诺兑现：**不装 DSH、不碰坏机器，第三方 UI 也能开发并测试全部界面**（含错误路径）。

### 9.6 P1 协议层状态

| 切片 | 内容 | 状态 |
|---|---|---|
| C1 | LSP 帧 + `info` + `discover` | ✅ |
| C2 | 异步作业 + `doctor` + `job` + `cancel` | ✅ |
| C3 | `plan` + `apply`（确认/过期/幂等）+ 发现缓存 | ✅ |
| C4 | `o824-mock` + 一致性骨架 + `spec/transcripts` | ✅ |

P1 里仍未做：desktop/web 两种形态的完整适配（含 F11 版本漂移、并发实例/端口残留/版本漂移的共存检测）、发现算法补层（§7.7 第 1/4/5/6 层）、`npx o824-mock` 的 Node 包装（规范里写的是 npm 形态，我们是 Go 单 exe，需要单独决定是否提供）。
