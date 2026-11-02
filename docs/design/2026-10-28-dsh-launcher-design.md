# Object 824 — DSH Launcher 设计文档

- **项目代号：Object 824**
- 日期：2026-10-28（2026-10-31 增补受管的三种 DSH 客户端形态与首次安装，见 §21 / §22）
- 状态：**已批准（2026-10-31）**
- **协议名：O824**（曾用 DLP，因与"数据防泄漏"撞名而弃用）。方法前缀 `o824.`、二进制 `o824.exe`、环境变量前缀 `O824_`。
- **许可证：MIT**（代码与规范同一许可）。**对外产品名暂不定**，正式版发布前再定。
- **平台：v1 只做 Windows x64**；macOS / Linux 与 ARM64 在后续版本加入（§23）。
- 一句话：一个**能在 DSH 起不来时仍然可用**的 Windows 启动器，内核与 UI 以进程边界分离，契约公开成规范，供社区实现自己的界面。

---

## 1. 意图与为什么

DSH 桌面端自己会更新（Electron 主进程里的 `electron-updater`），插件管理也有宿主自带的插件管理器。所以"再做一个启动器"如果只是把这两件事重做一遍，没有存在理由。

真正的空档是**故障态**：更新中途断电/被杀、文件被安全软件拦、装了两份、`cordis.patch.yml` 写坏、`node_modules` 装了一半、想回滚到上一版、机器上根本没有可用安装。这些场景里宿主进程起不来，而插件正是由宿主加载的——所以插件方案在原理上就够不到它们。

作者进一步要求：内核与 UI 分离，且这套契约要公开，让别人能接自己的界面。于是交付物是**两件东西**：一份协议规范（O824）+ 一个参考实现（内核 + 人类 CLI + 一个 GUI）。

## 2. 目标

- **G1 故障态可修复**：在 DSH 完全无法启动的机器上，能发现安装、判断故障类型、修复到可启动。
- **G2 本体生命周期**：安装、更新、回滚 DSH 本体；诊断与修复 profile（配置层、依赖层）。
- **G2a 覆盖全部 profile**：`desktop`、`web`、`tui`、`headless` 一等公民，不把 profile 当特例。两种安装形态都要覆盖：桌面端（应用目录 + `~/.dsh/profiles`）与 npm 全局安装的 `dsh web`（包目录 + 各自的 profile）。
- **G3 契约公开**：内核与 UI 之间是进程级契约，规范可独立阅读，第三方不读参考实现源码也能写 UI。
- **G4 可分发**：启动器自身单文件、无运行时依赖、可自更新（**不做 Authenticode 签名**，见决策 15）。

## 3. 非目标

- 不复刻宿主内的插件市场 / 设置页（宿主已有 `dsh-plugin-manager`、`dsh-plugin-market`、`dsh-client-ui-settings-plugins`）。启动器只处理"宿主起不来时"的那部分。
- 不做远程管理、多机、服务器批量（v1 不考虑）。
- 不内置、不镜像分发 DeepSeek 的二进制。
- **不做发行版**：不内置 node/dsh 运行时、不随包塞精选插件、不打包整合包。这是与 dsh_desktop / DSH-Launcher 的根本路线差异（见 §17）。
- 不做 DSH 内部插件形态的等价物（见 §1 的鸡生蛋问题）。

## 4. 反目标（做到也算失败）

- 与官方 `electron-updater` 抢同一个安装目录，导致两边互相覆盖。
- 日常修复需要管理员权限。
- 用户为了装启动器得先装 Node / .NET / Rust 运行时。
- 规范需要读 20 页才能写第一个 UI。
- 修复失败时只给一句无法行动的话（"出错了，请重试"）。
- 使用《明日方舟：终末地》的立绘、logo、字体等素材（版式可借鉴，素材必须原创或使用 DSH 自有品牌资源）。

## 5. 已验证的现状（本机实测，2026-10-27/28）

| 事实 | 证据 |
|---|---|
| 桌面端是 Electron 壳 + Node 模式 host + cordis 插件树 | `DeepSeek Harness.exe`，`version` = 44.7.0 |
| 应用装在用户目录，无需 UAC 即可改文件 | `%LOCALAPPDATA%\Programs\DeepSeek Harness\` |
| 前端资源打包在 asar 中 | `resources\app.asar`（182 MB，带 SHA256 完整性块）；原生模块在 `app.asar.unpacked` |
| 宿主运行时版本 | `@deepseek-ai/dsh-desktop-host` 0.2.1-rc.3 |
| 有无头 CLI 入口 | `resources\runtime\cli\bin\dsh.cmd` → `ELECTRON_RUN_AS_NODE=1` + `app.asar\dsh\node_modules\@deepseek-ai\dsh-desktop-host\lib\cli.js` → `runCli` from `@deepseek-ai/dsh/lib/bin.js` |
| CLI 能力 | `dsh [--profile <name>]`；`dsh plugin --profile <n> <pnpm args>`（转发 pnpm）；`--from-default-profile`、`--patch`、`--dump-config`、`--dump-config-schema`、`--dump-default-config` |
| profile 布局 | `~/.dsh/profiles/desktop/`：`package.json`（`dsh.profile.bundles` = 插件清单）、`cordis.yml`（空数组，根）、`cordis.patch.yml`（用户覆盖层）、`pnpm-lock.yaml`、`node_modules`（`nodeLinker: hoisted`） |
| 应用更新源 | `resources\app-update.yml`：generic，`https://download.deepseek.com/dsh-desk/feeds/win-x64/`，channel `nightly`，发布者 `CN=Hangzhou DeepSeek Artificial Intelligence Co., Ltd.` |
| 更新由 Electron 主进程驱动 | 主进程 bundle 中出现 `electron-updater`、`checkForUpdates`、`quitAndInstall` |
| 宿主内置插件管理 | 包名：`dsh-plugin-manager`、`dsh-plugin-market`、`dsh-host-plugin-inventory`、`dsh-client-ui-plugin-manager` |
| 第三方插件可做到的边界 | `dshmarket` 通过 spawn `dsh` CLI / pnpm 完成安装、升级、备份、诊断、重启；但它明确无法修改桌面端 profile（"the market cannot change the desktop profile through the command line"） |
| 目标机运行时 | WebView2 155.x 已存在；系统自带 .NET Framework 4.8；**未**安装 .NET SDK / Rust 工具链；Node v24 仅开发机有 |

## 6. 约束

- 平台：Windows 10/11 x64 优先（arm64 后续）。
- 安装位置是 per-user，但**文件可能被正在运行的 DSH 占用**——替换前必须确认进程已退出。
- 官方更新器仍在，二者必须分工：**启动器负责"修复/降级/换频道/离线安装"，官方更新器负责"日常在线升级"**。不许同时写同一目录。
- 分发包不含 DeepSeek 二进制；一律从官方 feed 下载并验证发布者签名。
- SmartScreen：**不购买代码签名证书（2026-10-31 决定）**，我们的 exe 始终未签名，首次下载会触发"未知发布者"提示。防篡改改由**自管 ed25519 密钥签名的发布清单 + 逐文件 sha256** 承担；下载页与安装器首屏必须写清核对方法。注意：DSH 官方安装包仍是官方签名的，那条校验不能省。
- 网络：下载源可能不可达/被限速；必须有超时、重试、断点续传、镜像配置。
- 法务：参见反目标最后一条。

## 7. 已决策的方案与取舍

### 7.1 契约传输：stdio 会话（选项 A）

UI 拉起 `o824-kernel` 子进程，双向 JSON-RPC 走 stdin/stdout，进程生命周期 = 一次会话，framing 沿用 LSP 的 `Content-Length`。

> **修订（2026-10-31）**：内核要管理的对象是三种 DSH 客户端形态（§21），但这不改变传输选择——内核仍只讲 stdio。被管理的 `web` 形态自己会开端口，那是**被管理对象**的端口，不是内核的。

否决 B（本地守护 + HTTP/WS）：端口冲突、鉴权、单实例、防火墙弹窗，全是给"多客户端/远程"付的税，而场景不需要。
否决 C（双轨分层）：v1 就要写两套传输，测试矩阵翻倍。但**保留分层思想**写入规范：正文只定义语义层，传输绑定先只发 stdio profile。
B 的优势（谁能 spawn 谁才有句柄、天然单实例、可录制回放）是决定性的：修复工具不该额外开网络面。

### 7.2 输出格式：内核只出严格 JSON；人话由随包 `o824` CLI 渲染

- 内核只有一种输出：严格 JSON（含 `protocolVersion`、未知字段必须忽略）。
- `o824` 是**附加工具**，消费内核 JSON 并渲染文本；它在规范之外，无兼容承诺。
- 只读操作允许宽松文本展示——**这里的"文本"指 `o824` CLI 的渲染，不是内核的输出**：内核始终只给 JSON（§19.3）；**变更操作必须结构化请求 + plan/apply**（见 7.3）。
- 否决"内核双输出"：两套渲染、两套测试、text 一定会被依赖却不在规范内。

### 7.3 变更模型：plan / apply 两阶段

1. `plan` 返回结构化计划：将改哪些文件、删哪些目录、版本变化、所需空间、预计耗时、风险项。
2. UI 渲染给人看并确认。
3. `apply` 只接受 `planId` + 幂等键，**不接受"直接执行字符串"**；**计划有效期 15 分钟**，过期需重新 plan。

理由：变更不可逆且代价不对称；审计日志必须记录"用户确认了什么"；进程被杀/网络断是常态，需要幂等重放。

### 7.4 GUI 技术栈：Tauri 2 + WebView2（默认）

- 观感需求（大图、圆角、毛玻璃、hover 动效）是 CSS 原生能力，XAML 要逐条手搓。
- 壳体积 3–8 MB，自带上行更新器，NSIS 打包可用。
- **唯一风险（WebView2 缺失）由架构兜住**：内核是独立 exe，GUI 起不来时 `o824 doctor` 仍可用；GUI 启动失败要弹原生 MessageBox 指向 CLI。
- 备选 WPF（.NET Framework 4.8，零依赖、单 exe）：仅在"零依赖"优先级压过"界面迭代速度"时采用；代价是自己造 UI 库/动画（PCL2 走的就是这条路：VB.NET 2.22 MB + 自研 UI 库与动画模块）。

### 7.5 三层分离：进程 / SDK / 可选 shim

**第一层（承重墙）：内核永远是一个独立进程，规范面只有 stdio JSON-RPC。**

`o824.exe kernel --stdio` 启动后在 stdin/stdout 上传 JSON-RPC；**stderr 只留给内核自己的崩溃日志，永不承载协议**（FFmpeg 的教训：stdout 一旦要放数据，日志就只能挤去 stderr）。

为什么这条是承重墙，四个理由各对应一种真实事故：

| 理由 | 不这么做的后果 |
|---|---|
| 崩溃隔离 | 内核解析 YAML、跑 pnpm、替换上 GB 文件，都会崩；进程内崩了 UI 跟着死，且死在 `apply` 中途——留下半改的 DSH，比修之前更糟 |
| 版本错配可检测 | 三个角色一个 exe，UI 与内核永远同版本；第三方 UI 配任意内核靠 `o824.info` 协商能力，而不是靠崩溃发现 |
| 信任边界 | 启动器干的是删目录、改配置的活；"谁 spawn 谁才有管道句柄"是这条链上唯一的天然授权，不需要 token 或端口鉴权 |
| 前向兼容 | JSON 未知字段忽略，永远能加字段；C 结构体改一个字段就是 ABI 断裂 |

代价只有进程启动 40–120 ms/会话——一次修复要几十秒，占比千分之一。

进程生命周期约定（规范的一部分）：UI 用带 `CREATE_NO_WINDOW` 的方式 spawn，并把内核放进 **Job Object（KILL_ON_JOB_CLOSE）**，确保 UI 被杀时内核不会变孤儿；内核必须在该管道 EOF 后自行退出，不留后台进程；`stderr` 上的内容只用于诊断，不进协议。

**第二层：SDK 把"调用方便"补回来。**

每个主流生态一个薄客户端（`@o824/client`、`O824.Client`、`o824-client` Rust），约三百行，封装的是同一个 stdio JSON-RPC：

```ts
const o824 = await O824.connect();                     // 内部 spawn o824.exe kernel --stdio
o824.on("progress", e => ui.setProgress(e.phase, e.progress));
const plan = await o824.plan({ action: "repair", profile: "desktop" });
await ui.confirm(plan);                               // 确认是 UI 的事，SDK 绝不代劳
await o824.apply(plan.id, { idempotencyKey: crypto.randomUUID() });
```

SDK 的边界要写死，它和规范同等重要：

- **必须做**：分帧、请求/响应关联、事件回调、取消传播、进程退出检测与重启提示、能力协商、由 JSON Schema 生成的类型。**2026-11-03 实施**：`o824-sdk`（TypeScript）落地了前六项 + 手写类型；"由 JSON Schema 生成"要等协议 schema 存在，在此之前由**契约测试**（解析 `spec/transcripts` 每一行）保证类型不漂移。
- **绝不做**：业务判断（"该不该修"）、替用户确认、自动重试变更类请求、把本地状态当真相。
- **出错时**：把内核的结构化错误原样抛上来（`code` / `userAction` / `retryable`），不做"友好化"改写——改写正是我们要消灭的碎片化。

每个 SDK 配同一组契约测试（相同 transcript 喂给所有语言，断言行为一致）。成本约三天一门语言。

**第三层（可选、非规范性）：进程内 shim。**

只给嵌入方准备，三个 C 函数，**传的仍是同一套 JSON**：

```c
int32_t o824_open(const char* opts_json, uint64_t* out_handle);
int32_t o824_call(uint64_t handle, const char* request_json, char** out_response_json);
int32_t o824_close(uint64_t handle);
```

跨语言依然便宜（三个函数谁都能 FFI），前向兼容保住（还是 JSON），规范只有一份。失去的只有崩溃隔离，所以：**标注"仅为嵌入方准备，参考 UI 不得使用"**，放在规范的附录里，不参与一致性测试，v0.1 不实现。

明确**不做**的那种"原生 API"：`o824_apply(plan_id, flags)`、`o824_repair_profile(const wchar_t*)`。一旦这么设计，每加字段、每改语义就是 ABI 版本爆炸，各语言绑定各写一套，**规范退化成一个 C 头文件**。

**贯穿原则：自己的 GUI 必须走公开协议。**

Tauri 界面只依赖 `o824-client` SDK，绝不直接链内核 crate。CI 用固定 transcript 回放，验证"UI 发出的每条请求都在规范里"；UI 每遇到一次协议不足，先改规范再改 UI。参考实现绕过契约，契约就只剩第三方在踩坑，半年后必烂。

**语言选择**：内核用 Rust 或 Go，由实现者定。硬约束：单个 exe、无需用户安装运行时、可交叉编译、便于社区审计。
参考实现用其中一种；**另一种语言留给社区**——这本身就是对规范的第一次真实检验。

### 7.6 内核的分发形态：一个带清单签名的 zip，内含一个多角色 exe

```
o824-0.1.0-win-x64.zip
├─ o824.exe          kernel / CLI / updater 三合一（按子命令分派）
├─ o824-gui.exe      Tauri 界面（可独立发布）
├─ manifest.json    版本、协议版本、逐文件 sha256
└─ LICENSES.txt
```

- 单 exe 承担三个角色：`o824 kernel --stdio`（协议模式）、`o824 doctor`（人类 CLI）、`o824 --apply-update <包>`（自更新），避免"内核与 CLI 版本错配"——修复工具上的版本错配会变成灾难。
- 与"版本化目录 + 指针"的自更新模型天然一致：更新 = 解压新目录。
- **不用 MSIX**：MSIX 有文件/注册表虚拟化，一个需要替换别的程序文件的修复工具会被它架空。
- 便携版就是同一个 zip，解压即用。
- 校验：`manifest.json` 逐文件 sha256 + **发布清单的 ed25519 签名（自管密钥，离线保管）**；不做 Authenticode（决策见 §23 第 15 条）。安装器只做"解压 + 建快捷方式 + 写 HKCU 卸载键"。
- 体积预算：内核 3–6 MB，GUI 5–8 MB + 资源。

### 7.7 发现算法：DSH 的安装位置（从最可信到兜底，每层都要验证）

| 层 | 来源 | 备注 |
|---|---|---|
| 1 | `state.json` 记住的上次路径 / `--install-dir` | 用户显式指定优先 |
| 2 | 卸载注册表 HKCU → HKLM → WOW6432Node | 本机实测：`HKCU\...\Uninstall\1bf39983-50d0-5fe0-9ef4-cece76f67c5e`，`DisplayName="DeepSeek Harness 0.2.1-rc.3"`，`InstallLocation="D:\Users\X_Cyanogen\AppData\Local\Programs\DeepSeek Harness"` |
| 3 | 遍历**所有固定卷**的 `\Users\<user>\AppData\Local\Programs\DeepSeek Harness` | 不能只查 `%LOCALAPPDATA%`：本机 `%LOCALAPPDATA%` 在 C:，安装却在 D: |
| 4 | 运行中进程的 image path | 活证据，顺带得到"是否在运行" |
| 5 | 开始菜单 / 桌面 `.lnk` 解析 | 用户可能从快捷方式启动 |
| 6 | 受限扫盘（白名单目录 + 超时） | 兜底，永不阻塞主流程 |
| 7 | `~/.dsh/profiles/*` 反推 | 有 profile 无安装 → 报告"孤儿 profile"，不猜测。**2026-11-02 实施**：不是一个新的 F 编号，而是 `o824 discover` 在"整机没有任何安装、但 home 里有 profile"时打到 stderr 的一行提示——F 编号是**每个安装**的检查，而这里恰恰没有安装可以挂靠 |

两个实测坑：**注册表 `Publisher` 是空的**（不能靠它识别安装）；`DisplayName` 带版本号（匹配要用前缀，不能用等值）。

候选验证三件套（缺一即标记 damaged）：`DeepSeek Harness.exe` 存在、`resources\app.asar` 存在、能从 `version` 文件或 asar 内 `package.json` 读出宿主版本。去重靠 realpath 归一（符号链接、8.3 短名、大小写）。

`dsh web` 形态走另一条链：`npm prefix -g`（本机 `C:\Users\X_Cyanogen\AppData\Roaming\npm`）+ `node_modules\@deepseek-ai\dsh`；pnpm 全局安装则查 pnpm 的 global dir。



### 7.8 启动器自己的安装位置与自更新

**安装位置（per-user；默认值固定，但用户可选）**

```
<用户选择的根>\                     程序，版本化目录（默认 %LOCALAPPDATA%\Programs\o824\）
├─ app-0.1.0\                      o824-gui.exe / o824-kernel.exe / o824.exe
├─ app-0.1.1\
└─ current                         文本指针（或 junction）指向当前版本

%LOCALAPPDATA%\o824\                数据（不随程序目录走）
├─ state.json  journal\  plans\  snapshots\  logs\
└─ cache\packages\                 下载缓存（安装包，可几百 MB）
```

**路径由用户在安装时选择**：默认 `%LOCALAPPDATA%\Programs\o824\`，也可以装到别的盘（例如 `D:\Apps\o824\`）。安装器把选择结果写到两处——HKCU 卸载键的 `InstallLocation` 与 `%LOCALAPPDATA%\o824\state.json`；**自更新读这两处，不猜路径**。

四条硬规则：

1. **绝不放进 DSH 的安装目录**，也不共用它的卸载键——否则修 DSH、卸载 DSH 会连坐把启动器带走。
2. 默认不装 `Program Files`；用户坚持要装时，明确提示"每次更新都会要 UAC"，并把默认焦点放在"换一个目录"。
3. 自己的卸载键写在 HKCU；卸载只删自己，**不动 `~/.dsh`**；数据目录（`%LOCALAPPDATA%\o824\`）**卸载时询问，默认保留**。
4. **程序目录与数据目录分离**：数据永远在 `%LOCALAPPDATA%\o824\`，不跟着程序目录走——否则用户把程序装到移动盘/网络盘时，状态与备份会一起消失。

路径校验沿用 §22.3 的那张表（可写、非嵌套、非 UNC、长度、空间），但**不需要** DSH 那套"目录已有内容"的提示——启动器不会和别的软件抢目录。

自更新时若记录的路径已不可写或被删除 → `E_SELF_UPDATE_BLOCKED`，提示"重新运行安装器并选择新的安装位置"，**不尝试自动搬到别处**。

选 per-user 与 DSH 自身保持一致（它也是 per-user），多用户机器各管各的。数据目录可用 `--data-root` / `O824_DATA_ROOT` 迁到别的盘——安装包缓存体积大，用户可能想放到 D:。

**自更新：两段式（Windows 上运行中的 exe 不能覆盖自己）**

1. 主进程检查更新（默认启动时检查、**不静默安装**，需用户确认；不收集任何设备标识）。
2. 下载到 `cache\packages\`，校验**发布清单的 ed25519 签名 + 逐文件 sha256**（不做 Authenticode，见决策 15）。
3. 主进程 spawn 一个 updater 副本（`o824-gui.exe --apply-update <包> --wait-pid <pid>`）后退出。
4. updater 等父进程退出 → 解压到新的 `app-<ver>\` → **原子地切换 `current` 指针** → 重启启动器 → 自删。
5. 保留上一个版本，`o824 self-update --rollback` 可退回。

不做"直接覆盖 exe"：文件锁定、被杀软拦、写一半断电都会留下半坏状态；版本化目录 + 指针切换意味着最坏情况只是回到旧版本。

约束：

- **不自动降级**，除非显式 `--allow-downgrade`。
- 自更新与"修复 DSH"共用同一把机器级锁——任何 `apply` 进行中都拒绝更新，反之亦然。
- 启动器只有几 MB，**全量更新**，不做差分（差分是用复杂度换一点流量，不值）。
- 更新源与 DSH 的 feed 完全独立，**托管在 GitHub Releases**（支持配置镜像以应对可达性）；**发布签名密钥必须固定且离线保管**——未签名分发时，这份清单签名就是唯一的可信锚点；代价是 SmartScreen 提示会长期存在，不假装能靠"信誉累积"消掉。
- 离线/逃生口：`o824 self-update --from <文件>`，给"更新源不可达"和"启动器自己坏了"用。
- 便携版：若检测到自己不在 `app-<ver>` 布局里，自更新降级为"提示下载新版本"，不尝试替换。
- GUI 与内核**同一版本一起打包**，各自上报版本；第三方 UI 只依赖 `o824.info` 的能力协商，不靠版本号硬匹配。


## 8. 架构

```
Object 824（桌面 GUI / CLI）
      │  只依赖 SDK，spawn 内核
      ▼
o824-kernel（单 exe，零网络面）
      ├─ 形态适配器
      │    ├─ desktop ──→ 官方桌面应用（asar / 官方 updater / 进程锁）
      │    ├─ web     ──→ dsh web（npm 全局包 / 端口 / 浏览器）
      │    └─ tui     ──→ dsh-tui（npm 包 / 真实终端 / peer 版本）
      ├─ discovery   跨形态发现安装与部署
      ├─ verify      按形态做健康检查（复用 §18 的 F 编号）
      ├─ plan        生成变更计划（纯函数：输入现状 + 目标 → 计划）
      ├─ apply       执行计划（文件替换、调用 pnpm、写审计日志）
      ├─ state       %LOCALAPPDATA%\o824\（state.json / plans / logs / backups）
      ├─ proc        进程探测、优雅退出、启动目标并回传 pid/端口/退出码
      └─ selfupdate  启动器自身更新
```

关键点：`plan` 与 `apply` 分离，plan 不产生副作用，可反复调用、可 dry-run、可被第三方 UI 预览。**内核不监听任何端口**——它管的是别人的服务，自己不该再开一个。

## 9. 协议草案 O824 0.1

> 本节是方法清单与传输的索引。若干细节已被 **§19** 修订并定死（全部操作异步化、`seq` 语义、`category` 枚举、文案归属、版本协商、退出码），冲突时以 §19 为准。

**9.1 传输（规范性）**

- 仅 stdio：`o824.exe kernel --stdio`，帧格式沿用 LSP：`Content-Length: N\r\n\r\n<json>`。
- `stderr` 保留给内核自身的诊断，**不得承载协议消息**；客户端必须持续读取 stderr，否则管道写满会把内核卡死。
- 一个会话一个客户端；会话 = 子进程生命周期。
- 服务端每条消息带单调递增 `seq`，同一 `seq` 之前的事件顺序有保证。
- 内核发现 stdout 写阻塞超过阈值（客户端不读），必须进入安全状态：先落 journal，能安全中止则中止，否则跑完当前原子步骤后退出——**不得无限阻塞在写操作上**。
- 父管道 EOF → 内核必须自行退出（事先落 journal）。

**9.2 版本与能力协商**

- `o824.info` 返回 `protocolVersion`（语义化 主.次.补丁）、`impl`（名称 + 版本）、`capabilities[]`（如 `plan.apply`、`snapshot.rollback`、`profile.web`）。
- 客户端只允许调用 `capabilities` 里声明过的能力。
- 未知方法：JSON-RPC `-32601` + O824 错误码 `E_UNSUPPORTED`（附 `minVersion`，说明从哪个协议版本起支持）。
- 0.1 期间只许加法：加字段、加方法、加错误码；删改语义必须升主版本。

**9.3 方法（v0.1 最小集）**

| 方法 | 说明 |
|---|---|
| `o824.info` | 返回 `protocolVersion`、`impl`、`capabilities[]`、`platform` |
| `o824.discover` | 返回安装列表（路径、版本、频道、profile 列表、健康摘要） |
| `o824.doctor` | 只读诊断，返回 findings（严重度、错误码、可行动建议） |
| `o824.plan` | 输入目标动作与参数，返回 plan 对象（含 `planId`、有效期、影响面） |
| `o824.apply` | 输入 `planId` + `idempotencyKey` + 确认令牌，执行并返回 job id |
| `o824.cancel` | 取消正在执行的 job（尽力而为，返回是否真的可取消） |
| `o824.job` | 查询 job 状态与最终结果（幂等） |
| `o824.logs.tail` | 拉取审计/操作日志（带游标） |

通知（server → client）：`op.progress`、`op.state`、`op.log`、`plan.expired`。

错误对象统一形状：`{code, category, message, userAction, retryable, details}`。`code` 是枚举（如 `E_DISK_FULL`、`E_FILE_LOCKED`、`E_SIGNATURE_MISMATCH`、`E_PROFILE_PARSE`、`E_NETWORK`、`E_PERMISSION`），`userAction` 是一句可执行的建议，`category` 供 UI 决定展示方式。

`o824.doctor` 返回的 findings 信封**兼容生态里已有的 `dsh-doctor/v1`**（见 §17.2 的 dsh-win32），扩展字段放独立命名空间（`o824.` 前缀）。理由：那个格式已经被一个 143 星的独立工具和它的用户群使用，统一比自创便宜。完整的错误码清单见 §18.2 每一行的"错误码"列。

约束：未知字段必须忽略；新增方法只增不改；破坏性变更必须升 `protocolVersion` 主版本。

## 10. `o824` CLI（人类界面）

```
o824 discover            列出安装
o824 doctor [--fix-hint] 只读诊断，输出人话 + 错误码
o824 install --version X
o824 repair --profile desktop
o824 rollback --to <version>
o824 plugins list
o824 logs
o824 spec-check          JSON Schema + golden transcript 自检
```

`o824` 的每个动作都要先打印 plan 摘要并等确认（`--yes` 仅跳过交互，不跳过 plan）。

## 11. 一致性测试

- `spec/schema/*.json`：JSON Schema。
- `spec/transcripts/*.jsonl`：golden transcript（输入 → 期望事件序列），比对时忽略时间戳与进度抖动。
- `o824 spec-check`：任一新实现可自测，CI 跑同一套。
- **`spec/fixtures/` 合成安装树**：正常 / F01 坏链 / F02 缺 bundle / F03 重复 loader / F17 构建白名单 / F20 半装。修复逻辑的回归测试全部对着它们跑，不依赖真机。
- **`o824-mock`**：按 transcript 回放的内核替身，第三方 UI 无需真机即可开发（§19.5）。
- **SDK 契约测试**：同一组 transcript 喂给每个语言的 SDK，断言（请求序列、错误映射、取消行为）一致。

## 12. 分阶段

| 阶段 | 内容 | 完成标准 |
|---|---|---|
| P0 | 内核 CLI：discover / doctor / repair / snapshot / rollback（无协议，纯文本），profile 泛化；`doctor` 覆盖 §18.3 的 F01–F04 / F16 / F17 / F20 | 能修好一台"profile 写坏"的机器，且修之前自动快照、修坏了能回滚 |
| P1 | 协议 0.1（stdio）+ **desktop / web 两种形态**的 discover / doctor / repair + `o824` CLI + TS SDK + 一致性测试骨架 + `spec/fixtures` 与 `o824-mock` | 桌面端与 web 形态的 F01–F04 / F11 / F16 / F17 / F20 都能诊断并修复；不装 DSH 也能开发 UI |
| P2 | **tui 形态** + 三形态的 start / stop / logs + 共存检测（并发实例、端口残留、版本漂移） | 三种形态都能被发现、启动、停止；共存风险有明确警告而不是默默放过 |
| P3 | 桌面 GUI（Tauri，**复用 P1 的 SDK 与 plan 渲染**）；§20 的十条 UX 硬约束全部落地 | 图形界面完成一次修复与一次安装；§20.1 的三条首次使用路径都能走通 |
| P4 | 安装器、**发布清单签名（ed25519）**、自更新、崩溃恢复 | 陌生人能装、能升级启动器自己，且能自行核对清单签名 |
| P5 | 公开 0.1 规范 + 文档 + 第二个（社区）实现 | 出现一个作者之外的可用实现 |

**为什么先 desktop + web**：这两类承载了绝大多数"起不来"的故障（asar 损坏、进程锁、npm 包损坏、端口冲突），而 `tui` 是客户端、故障面窄，放 P2 补齐。**桌面 GUI 排到 P3**：它最贵，而协议可以先用 CLI 验证——先做最贵的壳、协议却没被真用过，是最常见的死法。

**P5 之后（暂不排期，由 §17.3 第 5 条得出）**：多实例并行、插件矩阵、整合包导入导出。社区最买账的是这些，但它们是"发行版"能力，先不抢 P0；路线图里留位置，避免被评价为"功能少"。

## 13. 风险与未验证项

| 项 | 处理 |
|---|---|
| 官方更新器与启动器抢目录 | 不依赖"探测更新器"（探不准）：靠 plan 绑定状态指纹、apply 前重校验；若尽力探测到更新器进程，额外拒绝一次并说明原因 |
| 官方安装包的静默参数未知（NSIS `/S` 未验证） | P0 期间实测，失败则回退到"GUI 交互安装 + 启动器只做前置检查" |
| 桌面端 profile 是否可被 CLI 管理（CLI 里有 `manageDesktopProfile` 参数，含义未确认） | P0 验证；若不可，插件级修复降级为"依赖重建 + 配置校验"，重置仍必须是用户显式动作 |
| 回滚缺少旧版本包（feed 是否保留历史版本未知） | 启动器本地保留 **2 个**旧版本元数据与安装包，缓存上限 **2 GB**；降级提示见 §22.3.1 |
| **不购买代码签名证书**（2026-10-31 决定） | 始终以未签名分发；防篡改靠自管 ed25519 签名的发布清单 + 逐文件 sha256；下载页与安装器首屏主动说明"未签名，可能出现 SmartScreen 提示"，并给核对方法（不劝关安全软件） |
| 规范冷启动（没人实现） | P4 之前不承诺 1.0；先自己写两个 UI（Tauri + CLI）证明契约可被独立实现 |
| `dsh web` 的安装形态与桌面端完全不同（npm 全局包，无应用目录、无 asar） | P0 就要打通两条发现路径；布局描述符按"形态 + 版本"两个维度索引 |
| 快照与 `node_modules` 体积（profile 471 MB） | 只快照文件清单与设置；依赖回滚靠 lockfile 重建，回滚耗时可接受但要在 plan 里如实估算 |
| 杀软/EDR 误拦自更新（解压写 exe、切换指针、自删） | updater 用标准 exe 替换流程、发布密钥稳定；失败时保留旧版本并给出明确错误码 `E_SELF_UPDATE_BLOCKED`，引导用户手动下载 |
| SmartScreen 首次拦截 | 未签名是既定事实：靠发布清单签名 + 稳定下载地址降低"被换包"风险；文档与首屏说明"这是未签名程序"，给 sha256 核对步骤，不劝用户关 SmartScreen |
| 自更新与修复并发 | 共用同一把机器锁；拿不到锁就是 `E_BUSY`，双方都不得强行继续 |

## 14. 留给实现者的决定

内核语言（Rust/Go）、GUI 框架细节与状态管理、状态文件格式、日志格式与轮转、i18n 方案、CI 与发布流水线、证书采购、遥测（默认关闭）。

## 15. 评审后补充的决定（2026-10-28，一次独立评审提出）

评审者独立通读了本文并检查了本机，指出的问题与据此定下的规则：

**15.1 内核必须自带结构校验，不依赖装好的宿主。**
所有现成的校验器（`--dump-config`、`--dump-config-schema`、`--from-default-profile`、模板）都跑在待验证的那份安装里——G1 的故障态下它们正是死的。所以 `doctor` 不能是宿主 CLI 的薄包装：内核自己解析 `package.json` / `cordis.yml` / `cordis.patch.yml` / `pnpm-lock.yaml`，自己判断 bundle 是否在 `node_modules` 里、锁文件是否与清单一致。宿主 CLI 只在它还能跑时作为**附加证据**。`E_PROFILE_PARSE` 一类的错误码由 O824 定义，继承宿主语义。

**15.1.1 诊断不得依赖宿主，修复可以委托宿主重新物化依赖（2026-11-01 补充）。**
上面那条约束的是**诊断**：判断"哪里坏了"不能靠跑宿主，因为宿主正是坏的。**修复**是另一件事：把丢失的包重新放到磁盘上，只有包管理器能做，而宿主自带的 pnpm 就是这台机器上唯一知道这份 lockfile 该怎么解的东西。规则：

- 委托只发生在 `apply` 的修复步骤里，且**必须用户显式开启**（`repair --with-install`），默认不动 `node_modules`。
- 委托的形式是 argv 直调宿主的 pnpm（`DeepSeek Harness.exe --expose-internals <pnpm.mjs> install --frozen-lockfile`），不经 shell；工作目录是**副本**，不是真实 profile。
- 副本里装成功、校验通过后才做一次目录交换；交换前原 `node_modules` 一个字节没动，失败就把副本丢掉。这样"修复可回滚"的承诺对 `node_modules` 也成立（§16.1 之所以不快照 `node_modules`，正是因为它可以由 lockfile 重建）。
- 诊断仍然只看内核自己解析出来的事实；修复结果是否成立，靠修复后重新体检（§17.4）来证明，不靠 pnpm 的退出码自证。

**15.2 安装路径不假设、不发明。**
本机就有反例：安装在 `D:`，profile 在 `C:`（`%LOCALAPPDATA%`），两卷分离且无 junction。规则：发现阶段扫卸载注册表 + 所有盘的常见目录，**不假设同卷**；状态与备份放 `%LOCALAPPDATA%\o824\`，跨卷时用"复制 + 校验"而非硬链接/原子换名。启动器**不替用户决定**安装路径：默认值取官方安装器默认，但用户可在 plan 阶段改（§22.3）；启动器自己不发明路径。重复安装只报告，卸载交给官方 uninstaller，启动器不删目录。

**15.3 中断与并发语义必须进 0.1。**（0.1 只许加不许改，这两条不能后补）
- `apply` 开始前写 journal（目标、步骤、当前阶段、计划指纹）；管道断开或进程被杀后，新会话读 journal 报 `op.interrupted`，给出"继续/回滚"两个动作，不自行决定。
- 机器级锁：一个内核独占写权，第二个实例得到 `E_BUSY`（附持有者 pid 与会话信息），不做队列。**作用域是整机（per-machine）**——同一台机器上不同的 Windows 用户也互斥，因为 DSH 的安装与 `~/.dsh` 之外的文件可能被两边同时动。

**15.4 plan 绑定状态指纹。**
`plan` 时对"安装文件 + profile 关键文件"取指纹，`apply` 前重新校验；不一致返回 `E_STATE_DRIFT` 并要求重新 plan。官方更新器是否正在跑**无法可靠探测**（只有一个缓存目录和一个无版本元数据的安装包），因此不靠"探测更新器"来保安全，靠指纹校验。

**15.5 版本支持与降级。**
内核内置"布局描述符"（按 DSH 版本区间给出 profile 结构与校验规则）。遇到未知版本：只做结构校验 + 恢复到模板，不做版本特化修复，并明确告知"该版本未经验证"。描述符随内核自更新下发，规范里把它定义为实现细节（不属于协议）。

**15.6 取消"换频道"。**
`app-update.yml` 在安装目录内、归官方所有，改它下次更新会被覆盖，且不属于我们的授权面。G2 的"换频道"改为"安装指定版本"（整包安装 + 本地缓存），频道切换留给官方客户端。

## 16. 作者确认的三项（2026-10-28）

**16.1 修复承诺 = 强承诺，用户的设置与插件不能丢。**
任何**可能影响启动**的操作之前，内核自动生成快照——触发条件是：安装/删除插件、更新或回滚 DSH、修复 profile。用户也可以随时手动创建快照。失败或中断可一键回滚。快照范围（小而够用）：`package.json`、`cordis.patch.yml`、`cordis.yml`、`pnpm-lock.yaml`、安装清单（版本 + 文件哈希索引）。**不**快照 `node_modules`（数百 MB，且可由 lockfile 重建）。默认保留最近 **5 个**快照（可调），超限按时间淘汰。
"重置到模板"是独立动作，**永远不作为自动修复手段**，且必须由用户显式选择。
因此 `plan` 必须包含 `snapshot` 步骤，`apply` 的第一个可回滚检查点就是"快照已落盘且校验通过"。

**16.2 全 profile 覆盖。**
`desktop` / `web` / `tui` / `headless` 从第一天起就是同一套代码路径，差异只体现在布局描述符。注意 **profile 名与形态名同形不同义**：`profiles/tui` 是 dsh 自带的 profile，`tui` 形态是第三方应用 dsh-TUI，它用的是**另一个 profile**——`profiles/dsh-tui`（首次运行自动初始化）。两者名字像，但不是一回事。两种安装形态：桌面端（`Programs\DeepSeek Harness` + `~/.dsh/profiles`）与 npm 全局 `dsh web`（全局包目录 + 同名 profile 体系）。"DSH 起不来"对 `dsh web` 的含义是：npm 包损坏、版本冲突、profile 不可解析、端口/依赖故障。

**16.3 版本认识范围。**
内核内置布局描述符，认识**当前版本 + 前 2 个 minor（共 3 条线）**；遇到未知版本**降级**为"只做结构校验 + 恢复到模板"，并在结果里明确标注"该版本未经验证"。描述符属于实现细节，不进协议。

## 17. 竞品与差异（2026-10-29 调研）

调研方法：本机 `web_search` 不可用（无 API key），数据取自 GitHub API 的仓库元数据 / Release 资产 / Issue 原文，以及 npm registry 搜索与 jsDelivr 上的 README。星数与下载量为调研当日快照。

### 17.1 启动器：三个流派

| 项目 | 规模 | 技术栈 | 定位 |
|---|---|---|---|
| [DSHA](https://github.com/DSH-APP/DSHA) | 2543★ | Java/Kotlin | 安卓：proot 跑 Ubuntu + DSH，ADB 直连 |
| [DSH-X](https://github.com/yyh-001/DSH-X) | 766★ | Electron/JS | 启动项与多实例、版本管理、插件与 `.dspack` 整合包、MCP/技能 |
| [dsh_desktop](https://github.com/myYangyunfan/dsh_desktop) | 735★ | Tauri 2 + Rust + 内置 Node | 34 个精选插件随包交付、零环境依赖、离线可用 |
| [DSH-Launcher](https://github.com/MarcoG-h/DSH-Launcher) | 271★ | Electron/TS | 多实例管理中枢、整合包、插件矩阵、安全审计 |
| [Ruler4396/dsh-launcher](https://github.com/Ruler4396/dsh-launcher) | **已停更** | 原生 + WebView2 | 1.4 MB 极简壳、`--diagnose` 诊断包 |

共性：启动、版本安装切换、插件管理/市场、主题、托盘、自更新。分化在三件事：谁内置运行时（安装包 100–190 MB）、谁做多实例、谁做整合包。

### 17.2 维修：全是零件，没有整机

| 项目 | 形态 | 覆盖 | 局限 |
|---|---|---|---|
| [dsh-win32](https://github.com/sjh9714/dsh-win32) 143★ | `npx` 独立 CLI | `setup/doctor/fix/verify`，JSON 信封 `dsh-doctor/v1`，DSH 起不来时也能停用旧插件 | 只修 Windows 的 PowerShell/沙箱/koffi/预设；不管安装、版本、profile 依赖 |
| [dsh-repair](https://github.com/gezi-wen/dsh-repair) 753 下载/月 | 插件 + 技能 + 6 个脚本 | 起不来的排查路径、依赖桥重建、升级预演 | **插件形态：起不来时它就不存在** |
| [@anweat/dsh-substrate](https://github.com/anweat/dsh-substrate) 881 下载/月 | 插件 | 诊断插件冲突，引导外部修复 | 只诊断 |
| [dsh-session-surgeon](https://github.com/xiaoshenming/dsh-session-surgeon) 13★ | 插件 | 修打不开的会话文件 | 单点 |
| [dsh-network-settings](https://github.com/kanneiren/dsh-network-settings) 100★ | 插件 | 网络诊断与安全修复 | 单点 |

### 17.3 六条启发

1. **需求被验证。** 一个只做 Windows 修复的 CLI 有 143 星；一个只带技能的修复包月下载 753 次。"坏了怎么办"不是臆想出来的痛点。
2. **空档被确认，但话要说准。** 不是"没人想到修"，而是**没人把它做成体外的一等公民**：除 dsh-win32 外全是插件，正好掉进"起不来时插件不存在"的死路；dsh-win32 虽然在体外，范围却只有 Windows 环境适配。
3. **反面教材印证了"不打包"。** dsh_desktop 的内置整合包自己制造了 `duplicate loader entry` → 插件树整体加载失败 → 退出码 1（其 PR #31 有完整根因分析）。发行版把启动器本身变成了新的故障源。
4. **三样现成设计直接借用**：`dsh-doctor/v1` 的 JSON 信封；`fix` 的"只修能证明安全的、修完再验一次"；`--diagnose` 的一键诊断包。另有 `.dspack` 的 manifest 结构、supervisor 探活与崩溃环重启，记入 P2 之后的参考。
5. **需求排序要修正。** 社区最买账的是多实例、插件矩阵、整合包（DSH-X 与 MarcoG 靠这些拿星）。它们不该抢 P0，但路线图里必须有位置，否则用户会觉得"功能少"。
6. **"只做壳"会被官方吃掉。** Ruler4396 在官方桌面端发布后直接停更。护城河只能是官方没有动机做的部分——**故障态**。

### 17.4 由此写死的约束

- `o824.doctor` 的 findings 信封**兼容 `dsh-doctor/v1`**，扩展字段放独立命名空间；不自创格式。
- **不做发行版**：不内置 DSH、不随包插件、不改内核。这与 dsh_desktop / MarcoG 的路线相反，是定位而不是能力缺失。
- P0 的三个靶心由 17.2 的空白反推得出：**安装损坏、版本回滚、profile 依赖重建**。
- 修复类动作必须遵守"可证明安全 + 修后复验"，与 plan/apply 同源。

## 18. 故障模式目录（2026-10-29）

### 18.1 来源与方法

官方仓库 `deepseek-ai/deepseek-harness` **关闭了 Issues**（26.5 万星、3.2 万 fork，`has_issues=false`），故障报告散落在 Discussions，且只能通过 GraphQL 认证检索。**DSH 的故障知识没有集中沉淀**——这既是生态空白，也是本目录存在的理由。

因此本目录的输入是：`dsh-repair` 的 SKILL.md（作者从真实事故沉淀的"症状 → 嫌疑层"速查表与铁律）、`dsh-win32` 的 Windows 失败清单、第三方启动器的 Issue/PR（如 dsh_desktop #124/#148/#177 与 PR #31）、以及本机实测（插件管理器操作日志、沙箱执行器故障）。

每条都标注**判据**（怎么确认，而不是猜）和**可修性**。可修性三档：**A 能自动修**、**B 需重装或回滚**、**C 只能诊断并给出可执行指引**。

### 18.2 目录

**A. 启动期崩溃（readiness 之前）——用户看到的是"双击没反应"**

| ID | 症状 | 根因与判据 | 可修性 | 错误码 |
|---|---|---|---|---|
| F01 | `ERR_MODULE_NOT_FOUND` 指向插件内部依赖 | 搬家/清理后 junction **穿透损坏**：`readlink` 有值、目标存在，但 `statSync(link + '/package.json')` 失败。判据：穿透可达性测试，不采信 readlink | A（原地重建为 junction） | `E_LINK_BROKEN` |
| F02 | `cannot resolve profile bundle "X"` | profile 的 `node_modules` 链接穿透损坏或缺失。判据：逐个 bundle 做穿透验证 | A | `E_BUNDLE_MISSING` |
| F03 | 插件树加载异常（**症状未证实**，2026-11-01 实测修订） | 实测 DSH 0.2.1-rc.3：`cordis.patch.yml` 的 `id` 指的是 loader entry id（`timer`、`dsh-ops/shell` 这类），**不是 bundle 包名**；指向不存在的 entry 只打 `patch: entry "X" not found` 警告并继续，进程照常启动。用 `insert` 造出真正重复的 entry id，`--dump-config` 合成阶段不报错，**真启动（web profile，服务正常监听）也不失败**：重复 id 指向不存在的模块时只得到 `2 entries did not activate` 警告，指向同一个真实模块时完全静默。`app.asar` 里没有 `duplicate loader entry id` 这个字符串。原判据"patch 行 id 与 bundles 求交集"**不成立** | P0：**只提示**（severity `warning`，不自动修）；判据待重定（P1） | `E_DUP_LOADER` |
| F04 | 树外插件（`link:`）导致起不来 | 依赖桥悬空。**一条桥断 = 整个 DSH 崩在 readiness 之前**，不是单插件失效。判据：桥存在性 + 指向当前闭包 + 穿透可达。**2026-11-01 实测补充**：宿主的 bundle 都打在 `app.asar` 里（`app.asar.unpacked` 只有 koffi / fontkit / libreoffice-kit 这类 native 依赖），junction 指不进 asar，所以"重建桥"只在 profile 自己的 `node_modules` 已有该包时可行；否则等价于重装插件 | A（有目标才重建；否则只诊断，重装属 P1） | `E_BRIDGE_DANGLING` |
| F05 | 升级/重装报 EPERM / EBUSY | 运行中的实例锁着原生模块（实测 `koffi.node`）。判据：以共享模式 None 独占打开该 `.node` | B（先停实例再重装） | `E_NATIVE_LOCKED` |
| F06 | 双击无反应 | asar / 安装文件损坏（下载中断、杀软隔离）。判据：asar 头可解析 + 完整性块校验 + 关键文件存在 | B（重装或回滚） | `E_ASAR_CORRUPT` |

**B. 起得来但功能死（"活着但残废"）——最难查**

| ID | 症状 | 根因与判据 | 可修性 | 错误码 |
|---|---|---|---|---|
| F07 | 模型页缺提供方、按钮死 | 插件 `apply()` 在运行时抛异常 → 静默死亡；schema 只校形状。判据：合成 ctx 直调 `apply()` | C | `E_PLUGIN_APPLY` |
| F08 | 配置写了某模型，页面没有 | 适配器**静默丢弃**目录里没有的 id。判据：`listModels(provider)` 实际公布清单 vs 配置清单 | C | `E_MODEL_DROPPED` |
| F09 | 日志干净、功能没了 | loader 报错被默认日志级别吞掉；桌面壳**吞 stderr**（只在崩溃时展示尾部）。判据：绕开壳直跑 `bin.js`，stdout/stderr 双留档 | C（诊断即价值） | `E_LOG_SWALLOWED` |
| F10 | 升级后行为变了、没人改配置 | `settings.yaml` 被客户端启动后**程序化重写**（实测升级后 25 秒被整体重写）。判据：快照对比 mtime 与内容哈希 | C | `E_SETTINGS_REWRITTEN` |
| F11 | 升级后插件全挂 | **判据（2026-11-03 实施修订）**：同一台机器上**多个形态的 DSH 版本不一致**（它们共用 `~/.dsh`，各自带的运行时不同）。本机实测即为该故障：desktop `0.2.1-rc.3` vs 全局 npm 包 `0.2.1-rc.5`。原判据"两棵 node_modules 版本表 diff"在实践中不可用——profile 里没有 core 包，插件自己的 `node_modules` 只有用户装的插件 | A（版本配平，尚未实现） | `E_VERSION_DRIFT` |
| F12 | 某插件界面白屏 | client 半身缺 react（0.1.5 起宿主不再随包发）。判据：client 包的 peer 与宿主实际提供比对 | A（补 peer） | `E_CLIENT_PEER` |
| F13 | 技能列表少了某个技能 | frontmatter 的 description 含 `: ` → YAML 解析失败 → **技能静默消失**。判据：解析 frontmatter 并比对技能目录 | A | `E_SKILL_FRONTMATTER` |

**C. 数据层**

| ID | 症状 | 根因与判据 | 可修性 | 错误码 |
|---|---|---|---|---|
| F14 | 会话看起来是空的 | 会话是**追加式多帧 zstd**，Node 的 `zstdDecompressSync` 只解第一帧（只拿到约 191 字节头）。判据：帧计数，必要时用外部 `zstd` 全解 | C | `E_SESSION_FRAME` |
| F15 | 回退旧版读不了会话 | 会话格式升级不可降级（0.1.5 起 V3）。判据：读会话头里的格式版本 | C（回滚前必须警告） | `E_SESSION_FORMAT` |
| F16 | 配置解析失败 / 写不进 | profile 的 YAML 写坏、磁盘满、权限不足。判据：解析器 + 磁盘余量 + 可写性 | A（语法类）/ C（资源类） | `E_PROFILE_PARSE` / `E_DISK_FULL` / `E_PERMISSION` |

**D. 安装 / 升级 / 插件管理**

| ID | 症状 | 根因与判据 | 可修性 | 错误码 |
|---|---|---|---|---|
| F17 | 从 GitHub 直装插件失败 | 包需要执行构建脚本但没被批准（本机日志实测，dshmarket@1.70.2）。**pnpm 11 语义（2026-11-01 实测修订）**：`pnpm-workspace.yaml` 的 `allowBuilds` 是 **map**（写成数组会硬报错 `The allowBuilds field should be an object, but got array`）；`onlyBuiltDependencies` 等旧键在 v11 已移除；**git/tarball 依赖用裸包名永远批不下来**，必须写 `name@git+https://host/org/repo.git`（不带 `#ref`）或 `name@<url>#<commit>`；`pnpm rebuild` 不会重跑被拦下的构建，只有 install 会 | A（写 map 形式的仓库键后重跑安装） | `E_PNPM_BUILD_BLOCKED` |
| F18 | 本地路径插件装不上 | `dsh plugin add` 拆分含空格的本地路径（upstream #2485）。判据：读回安装后的包身份，而不是只看命令成功 | A（规范化路径） | `E_PLUGIN_PATH` |
| F19 | 脚本在 Node ≥24 报 EINVAL | `spawnSync` 直接调 `.cmd`（CVE-2024-27980 补丁）。判据：检查调用形态 | A（过 `cmd.exe` 或直调 `npm-cli.js`） | `E_SPAWN_CMD` |
| F20 | 升级中断 → 半装状态 | 断电/被杀/写一半。判据：journal 存在且无终态记录；tui 形态另看 `~/.dsh-tui/restart.log` | B（回滚或重装） | `E_HALF_INSTALL` |
| F23 | dsh-TUI 拒绝启动，提示 Node 版本 | dsh-TUI 要求 Node `^22.19 \|\| >=24`，Node 23 与预发布在加载应用前被拦 | A（换 Node） | `E_NODE_RANGE` |
| F24 | 插件装了但原生模块缺失 | pnpm ≥11 默认拦构建脚本（`ERR_PNPM_IGNORED_BUILDS`），或跳过了异平台原生包 | A（允许构建脚本后重装） | `E_PNPM_IGNORED_BUILDS` |
| F25 | TUI 更新后起不来 | `~/.dsh-tui/restart.log` 有未完成记录，安装器可能留下半装 | B（重跑 `dsh-tui update`，或 `safe --rescue` 建干净 profile） | `E_UPDATE_INTERRUPTED` |
| F26 | 安装路径不可用 | 目标目录不可写、落在受保护目录、是网络/可移动盘、或路径过长。判据：plan 阶段预检（§22.3），不是 apply 时才发现 | A（换路径） | `E_INSTALL_DIR` |

**E. 环境与执行层**

| ID | 症状 | 根因与判据 | 可修性 | 错误码 |
|---|---|---|---|---|
| F21 | 所有工具调用直接失败 | 沙箱执行器不可用——本机实测 `windows-acl-run: --temp is not an existing directory` → `EXECUTOR_UNAVAILABLE`。判据：临时目录存在性 + 执行器自检 | C（DSH 内部故障，非安装故障） | `E_SANDBOX_RUNTIME` |
| F22 | 装不上 / 打不开 | 杀软隔离、WebView2 被删、SmartScreen 拦截。判据：进程与文件存在性 + WebView2 注册表项 | C | `E_BLOCKED_BY_SECURITY` |

### 18.3 与 `doctor` 的对应

- 每条 F 项对应一个 check id（`F01`…`F22`），`o824.doctor` 的 findings 用同一编号，第三方 UI 可以按编号本地化文案。
- **P0 必须实现**：F01、F02、F03、F04、F16、F17、F20（这七条覆盖了"起不来"的绝大多数）。其中 F03 自 2026-11-01 起降级为"只提示"：实测判据不成立（见 §18.2），重定判据前不做自动修。
- **P0 只诊断不修**：F05、F06、F11、F14、F15（需要重装/回滚或外部工具）。
- **P1 补**：F07–F13（"活着但残废"这一档需要合成 apply 测试等较重的诊断手段）。
- F21、F22 归入"环境故障"，只诊断并明确告知"这不是 DSH 安装的问题"，避免用户白折腾。
- **F23–F25 属于 `tui` 形态**，随 P2 覆盖；其中 F25 的判据（`restart.log`）也可以用来检测 F20 的 tui 版本。

### 18.4 规范里的位置

本目录作为规范的**非规范性附录**（informative）：实现可以选择只做其中一部分，但**检查编号与错误码必须一致**——这样同一个故障在任何 UI 上都叫同一个名字，用户换工具不用重新学一遍。

## 19. 接口契约补充（面向 UI 开发者与脚本用户）

这一节是 §9 的修订与补全。凡与 §9 冲突，以本节为准（§9 仍是方法清单的索引）。

### 19.1 最小 transcript：UI 开发者的第一个小时

UI 开发者应该能照抄这段就跑起来。四条消息走完"我是谁 → 有哪些安装 → 体检 → 拿到结论"：

```
→ {"jsonrpc":"2.0","id":1,"method":"o824.info","params":{"clientProtocol":"0.1.0"}}
← {"jsonrpc":"2.0","id":1,"result":{
     "protocolVersion":"0.1.0","minClientProtocol":"0.1.0",
     "impl":{"name":"o824","version":"0.1.0"},
     "capabilities":["discover","doctor","plan","apply","snapshot","rollback","logs"],
     "platform":{"os":"windows","arch":"x64"}}}

→ {"jsonrpc":"2.0","id":2,"method":"o824.discover"}
← {"jsonrpc":"2.0","id":2,"result":{"installs":[
     {"installId":"i1","kind":"desktop","root":"D:\\...\\DeepSeek Harness",
      "version":"0.2.1-rc.3","profiles":["desktop"],"health":"degraded"}]}}

→ {"jsonrpc":"2.0","id":3,"method":"o824.doctor","params":{"installId":"i1","profile":"desktop"}}
← {"jsonrpc":"2.0","id":3,"result":{"jobId":"j1"}}          ← 立即返回，结果走事件
← {"jsonrpc":"2.0","method":"op.state","params":{"seq":1,"jobId":"j1","state":"running","phase":"scan"}}
← {"jsonrpc":"2.0","method":"op.progress","params":{"seq":2,"jobId":"j1","phase":"scan","done":3,"total":7,"indeterminate":false}}
← {"jsonrpc":"2.0","method":"op.state","params":{"seq":3,"jobId":"j1","state":"done"}}
← {"jsonrpc":"2.0","method":"op.result","params":{"seq":4,"jobId":"j1",
     "envelope":"dsh-doctor/v1",
     "findings":[{"checkId":"F03","severity":"warning","code":"E_DUP_LOADER","category":"config",
                  "params":{"duplicates":1},"fallbackMessage":"Duplicate plugin entry"}]}}
```

**修订 1：所有操作都是异步作业。** 包括 `doctor`——它可能要扫几百个目录。统一成"返回 `jobId`，结果走 `op.result`"，UI 只需一条状态机路径；`o824.job` 可按 `jobId` 幂等补取结果，用于 UI 重连。

**修订 2：`seq` 是会话内全局单调**，不是每个 job 各自计数。UI 用它做事件排序与去重，不必理解并发。

### 19.2 进度与取消（定死，不留解释空间）

- 进度用 `{done, total, indeterminate}`；`total` 未知时 `indeterminate: true`，此时 `done` 仅供参考。
- 同一 job 内 `done` **单调不减**；`total` 只允许在阶段切换时变化（变大或变小都允许，但必须同时发 `op.state` 说明阶段变了）。
- **plan 有效期 15 分钟**；过期后 `apply` 返回 `E_PLAN_EXPIRED`，并主动发 `plan.expired` 通知，UI 必须重新 plan 而不是重试 apply。
- 阶段枚举固定：`scan | download | verify | snapshot | install | apply | rebuild | rediscover | health | restart`。首次安装（§22）会依次出现 `download → verify → snapshot → install → rediscover → health`；修复类操作不出现 `install`，更新类不出现 `install` 但可能出现 `rebuild`。
- **取消保证终态**：`o824.cancel` 之后，UI 一定能收到 `op.state{state:"cancelled"|"failed"|"done"}` 之一；内核不得让 job 停在 running。若不可取消，立即返回 `{cancellable:false, reason}`，且**不得**随后发出 cancelled。
- 一个会话可并行多个 job，靠 `jobId` 区分；写操作仍受机器级锁约束（§15.3）。

### 19.3 `category` 枚举与文案归属

`category` 取值固定为九种，UI 据此决定图标、颜色与是否阻断：
`install | config | plugin | data | env | security | network | permission | internal`

**文案归属**（修订 §9 的 `message` 必填）：

- 内核**不返回面向用户的人类文案**。`fallbackMessage` 只是英文兜底，保证没有翻译的 UI 也不会显示"未知错误"。
- 所有面向用户的话由 UI 按 `code` + `params` 渲染，词条表随规范提供：`spec/i18n/en.json`、`spec/i18n/zh-CN.json`。
- 诊断原文放 `detail`（可选、可能很长），只给"复制诊断信息"用，不直接上屏。

### 19.4 协议版本协商

- 客户端在 `o824.info` 里带 `clientProtocol`；内核返回 `protocolVersion` 与 `minClientProtocol`。
- 客户端可要求最低内核版本（`params.minServerProtocol`）。不满足时内核返回 `E_PROTOCOL_TOO_OLD`（附 `supported` 区间），**不得**降级继续。
- 规则：主版本相同即兼容；次版本只增；补丁号不参与协商。
- 客户端主版本高于内核 → `E_PROTOCOL_MISMATCH`，UI 应提示"启动器太旧，请升级"。

### 19.5 mock kernel 与 fixtures（开发基础设施）

没有这两样，第三方 UI 只能在真机上碰运气：

- `o824-mock`：按 `spec/transcripts/*.jsonl` 回放的内核替身，支持 `--transcript`、`--delay`、`--fail-at <seq>`（在第 N 条注入错误）、`--protocol <ver>`。UI 开发者就能开发全部界面，不需要 Windows、不需要坏掉的 DSH。

**2026-11-02 落实**：`o824-mock` 实现为 npm 包，内含内核同一份回放代码编出的
`wasm32-wasip1` 模块 + 一层只管 stdio 的 launcher，因此"不需要 Windows"是真的（WASI 跨平台）。
安装方式两条：GitHub Release 的 tarball（`npm i -D <release-url>`，不需要 npm 账号），
或将来发布到 registry 后用 `npx o824-mock`。设计与取舍见
`docs/design/o824-mock-package.md`，用户文档见 `packages/o824-mock/README.md`。
- `spec/fixtures/`：**合成安装树**（每个几百 KB 的假树，不含任何真实二进制）——正常树、F01 坏链、F02 缺 bundle、F03 重复 loader、F17 构建白名单、F20 半装。内核的修复逻辑对着这些跑回归测试。

### 19.6 CLI 契约（脚本用户）

- **退出码**：`0` 成功；`1` 发现问题（doctor 有 findings）；`2` 用法错误；`3` 需要确认但处于非交互；`4` 权限不足；`5` 状态漂移；`6` 网络；`7` 被锁；`8` 内部错误。
- `o824 <cmd> --json` 输出内核原始 JSON（不经过人话渲染）；`--json` 时 stdout 只有 JSON，日志一律走 stderr。
- 环境变量：`O824_DATA_ROOT`、`O824_HOME`（覆盖 `DSH_HOME`）、`O824_LOG_LEVEL`、`O824_OFFLINE`、`O824_MIRROR`、`O824_NO_COLOR`。
- 所有写操作命令都接受 `--dry-run`，等价于"只出 plan 不 apply"。

### 19.7 诊断包（求助路径的技术面）

`o824 diagnose --out <file.zip>` 固定包含：版本与安装指纹、doctor findings、最近 N 条操作日志、profile 的关键文件、系统信息（OS/架构/区域/杀软名称）。

**默认不含**：会话内容、凭据/API key、用户目录的绝对路径（做占位符替换）。`--include-paths` 才保留路径，且要在 UI 上明说。

### 19.8 命名与商标

- 允许：`<你的名字> — a O824-compatible launcher`，以及"通过 O824 0.1 一致性测试"的陈述（自测通过即可，不发放徽标）。
- 不允许：使用 O824 / DeepSeek 的 logo、暗示官方身份、把 "O824" 单独当作产品名。
- 参考实现自身的品牌另行确定，规范不占用。

## 20. 面向 0 基础用户的体验约束

这一档的问题不是"少功能"，是**语言和信任**。以下十条是硬约束，违反即视为 bug。

1. **界面上不出现内部术语**：profile / bundle / junction / cordis / asar / apply / loader 一律禁止。对用户说"启动环境""插件""安装文件"。
2. **每个错误必须有"下一步"**：一个动词 + 一个对象，例如"重新安装 DSH""打开备份文件夹""复制诊断信息"。没有下一步的错误不算完成。
3. **安全网必须可见**：修复前显示"将先备份（约 2 MB，可一键还原）"；备份完成后，"还原"按钮必须可点，且写明还原点的时间。
4. **给出时间预期**：plan 里带 `estimatedSeconds` 区间，UI 显示"约 1–3 分钟"；超过预估 1.5 倍仍未完成，要显示"仍在进行"而不是沉默。
5. **空状态即入口**：未发现安装 → 主按钮是"安装 DSH"；发现多个 → "有 2 个安装，选一个修复"；有安装但健康 → 主按钮是"启动"。
6. **信任声明固定可见**：标题栏或关于页常驻"非官方工具，由 <作者> 维护"，附官网与源码地址。**本产品不购买代码签名证书**，所以必须主动说明"未签名"，并在关于页给出 sha256 核对入口——回避这件事比提示更伤信任。
7. **误操作防护**：删除、重置、卸载类动作要求二次确认，且默认焦点在"取消"；破坏性更强的动作要求输入安装名。
8. **不劝关安全软件**：任何界面都不出现"请关闭杀毒软件/添加白名单"作为解决方案；只提示"联系你的安全软件厂商"或给出申诉链接。
9. **一键求助常驻**：任何错误页都有"复制诊断信息"，脱敏规则见 §19.7。
10. **首版中英双语**：文案全部来自 i18n 词条，代码里禁止硬编码字符串；0 基础用户默认跟随系统语言。

### 20.1 首次使用的三条路径

| 情形 | 用户看到 | 主按钮 |
|---|---|---|
| 没有安装 | "还没装 DSH" | 安装 DSH |
| 有安装、能启动 | "DSH 0.2.1-rc.3，可以启动" | 启动 |
| 有安装、起不来 | "DSH 起不来。原因：插件配置重复（可自动修）" | 一键修复 |

第三条是产品的存在理由：**用户不需要知道 F03 是什么，只需要知道"能修"和"要多久"**。

### 20.2 我们明确不做的事

- 不做"专家模式"开关把术语放出来——那只是把责任推给用户。
- 不把 `detail` 里的原始堆栈直接上屏。
- 不在错误页放"重试"作为唯一动作（重试解决不了配置错误，只会让用户重复失败）。

### 20.3 托盘与常驻（2026-10-31 决定）

**默认驻留托盘，可在设置里关掉。**

- 关闭窗口 ≠ 退出：第一次关窗时明确告知"仍在后台运行，可从托盘退出"，并给"不再提示"。
- 托盘菜单固定四项：**启动 / 体检 / 打开备份 / 退出**。
- 常驻时在后台检查更新，但**仍然不静默安装**（§7.8）——只在托盘气泡与主界面提示。
- 设置里关掉常驻后：关窗即退出，自更新检查退化为"启动时检查"。
- 常驻不等于开机自启：自启是另一个独立开关，默认**关**。

## 21. 受管的三种 DSH 客户端形态（2026-10-31 补充）

> **修订说明**：本节最初被写成"启动器自己的三种 UI 形态"，那是对需求的理解错误。Object 824 要管理的是 **DSH 的三种客户端形态**。启动器自己的 UI 仍只有两种：桌面 GUI 与 CLI。

### 21.1 三种受管形态

| 形态 | 是什么 | 安装形态 | 启动方式 | 更新通道 |
|---|---|---|---|---|
| `desktop` | 官方桌面应用（Electron 壳 + 内置 dsh 运行时 + asar） | 官方安装包，per-user | 双击 exe | 官方 `electron-updater`（nightly 频道） |
| `web` | `dsh web`：npm 全局包 + 本地 HTTP 服务 + 浏览器 UI | `npm i -g @deepseek-ai/dsh`（或 pnpm） | `dsh web --port <p>` 后开浏览器 | npm 版本 |
| `tui` | 第三方终端应用 **dsh-TUI**（[ccch1mneyyy/dsh-TUI](https://github.com/ccch1mneyyy/dsh-TUI)）：自有入口与组合根，DSH 以 in-process 方式当后端 | ①Windows 安装包（per-user、离线、免 Node）②`npm i -g` + 首次运行初始化 `profiles/dsh-tui` | 在**真实终端**里跑 `dsh-tui` / `dst` | 自有 `dsh-tui update`（30 分钟超时、Windows 用 Job Object 监管）或 npm |

三者不互斥：同一台机器上可以同时存在，而且**共用 `~/.dsh`**。

### 21.2 形态适配器：一套操作，三种实现

内核里每个形态实现同一组操作，协议不暴露实现细节：

```
discover()             → 该形态的部署列表（版本、路径、profile、端口）
verify()               → 健康检查（复用 §18 的 F 编号，按形态筛选）
plan() / apply()       → 变更计划与执行
start() / stop() / status() → 进程、端口、URL、退出码
logs()                 → 日志位置与尾部
update() / rollback()  → 该形态的更新通道
```

**为什么必须抽象成适配器**：三种形态的"安装"根本不是一回事——桌面端是替换 asar，web 是 npm 包，tui 是另一个 npm 包且依赖前两者的运行时。塞进一个 `install()` 里会立刻长出三套 if。

**协议层没有 `o824.start` / `o824.stop` 这样的方法**：`start` / `stop` 是 `plan` 的 `action`，走同一套 plan/apply 与确认流程；查状态用 `o824.discover` 加 findings，不另开方法。这样"启动"也自动获得 plan 预览、幂等与审计。

### 21.3 各形态的特殊性（决定哪些检查有意义）

**desktop**：有 asar 与完整性块、有官方 updater、进程锁着原生模块（F05）、per-user 安装。修复手段是"替换/回滚 + profile 修复"。

**web**：没有 asar。健康判据是"包能解析 + `dsh web` 能起 + 端口能监听 + 浏览器能连"。要管理端口（冲突检测、`--port 0`、记住上次端口）、要能开/关浏览器、要能杀干净残留的 node 进程。**版本漂移（F11）在这里最常见**——全局包与 profile 里的 client 包不同步。

**tui（dsh-TUI）**：它**不是薄客户端，而是一个独立应用**——自有入口与组合根，DSH 以 in-process 方式当后端；有自己的 profile（`profiles/dsh-tui`）、自己的配置目录（`~/.dsh-tui/`：`settings.json`、`permission.json`、`extension-grants.json`、`restart.log`），而且**自带 `doctor` / `safe` / `safe --rescue` / `update`**。

管理它意味着：

- **两种安装形态要分别识别**：Windows 安装包（per-user、离线、免 Node、免 DSH）与 npm 全局包（首次运行自动初始化 profile，需要 pnpm）。
- **Node 版本是硬门槛**：要求 `^22.19 || >=24`，Node 23 与预发布在加载应用前就被拦下。
- **pnpm ≥11 默认拦构建脚本**：会报 `ERR_PNPM_IGNORED_BUILDS`，是 F17 的同类。
- **`~/.dsh-tui/restart.log`** 是"上次更新是否中断"的第一手证据。

**委托优先（delegate-first）**：它已经有 `doctor` / `safe` / `update`，就不要重写一套——先调它的命令、解析输出，我们只补它没有的（跨形态版本配平、共存检测、统一诊断包）。这条对任何"自带运维命令"的第三方客户端都成立。

### 21.4 共存风险（本节的真正价值）

三种形态共用 `~/.dsh`，于是产生三类只有"统一管理"才能发现的问题：

1. **并发实例写同一份 sessions**：两个形态同时跑且共享 `DSH_HOME` → 数据风险。`start()` 前必须检测已有实例并警告，而不是默默再起一个。
2. **版本漂移**：桌面端内置 rc.3、全局 npm 是 rc.5、tui 的 peer 要求 rc.2 —— 三者互相打架。`doctor` 要一次列出三个版本并给出"谁该升、谁该降"。
3. **端口与进程残留**：web 形态被杀后 node 可能还在，下次启动就端口冲突。要能识别"这个 node 是我们上次起的"（记录 pid + 命令行指纹），而不是见 node 就杀。

### 21.5 对协议的影响

- `installId` 带 `kind: desktop | web | tui` 与 `installMethod: installer | npm`（tui 与 web 都可能两种）；`o824.discover` 返回混合列表，UI 按 kind 分组。
- `o824.plan` 的 `action` 按 kind 分派：`install | update | rollback | repair | start | stop`。`start` 的返回因形态而异：web → `{pid, port, url}`，tui → `{pid, terminal:true}`（必须由调用方提供真实终端），desktop → `{pid}`。
- 新增错误码：`E_KIND_UNSUPPORTED`（该形态不支持此动作）、`E_PORT_IN_USE`、`E_CONCURRENT_INSTANCE`（检测到共享 `DSH_HOME` 的另一个实例）、`E_DELEGATED_FAILED`（委托第三方自带的 doctor/update 失败，附其原始输出摘要）。
- `capabilities` 按形态声明：`kind.desktop.start`、`kind.web.port`、`kind.tui.launch`、`kind.tui.delegate`（表示该目标自带运维命令，可委托）、`kind.tui.plugins: readonly`（插件清单只读，见 §21.6）。

### 21.6 我们不做的事

- **不做 TUI 的宿主**：不内嵌终端、不代理它的输入输出。GUI 只负责开窗口与传环境。
- **不做多实例调度**：一个形态起多个实例是 P5 之后的能力（§12）。
- **不替第三方客户端升级它的私有依赖**：只报告版本冲突，不擅自 `npm i -g` 别人的包。
- **不代管第三方客户端的插件体系**（2026-10-31 决定）：dsh-TUI 的 `/plugin install`、`extension-grants.json`、自有 feed 是它的私域，Object 824 **只读展示**清单，并给出"交给它自己管"的入口。理由：那套授权（grants）语义一旦被外部写入出错，是**权限问题**，不是显示问题。

## 22. 首次安装：没有 DSH 的机器（2026-10-31）

### 22.1 默认形态与边界

- 默认装 **`desktop`**：它是唯一"免 Node、免第三方、官方签名"的形态。
- `web` / `tui` **只引导、不代装**：它们要求机器上已有 Node，`tui` 还是第三方项目（§21.6）。UI 上以"我已有 Node"作为展开条件，默认收起，展开后给安装命令与文档链接，不代跑 `npm i -g`。

### 22.2 状态机

```
no-install → plan → confirmed → downloading → verifying
           → installing → rediscovering → health-check → ready
```

每个失败出口都有确定的下一步，不允许停在中间态：

| 出口 | 下一步 |
|---|---|
| `download-failed` | 换镜像重试，或让用户手动指定安装包文件 |
| `verify-failed` | 删缓存条目、重下；连续两次失败 → 提示来源可能被篡改 |
| `install-failed` | 读退出码与日志尾部，按 F20 判定是否半装 |
| `rediscover-empty` | 视为失败，**不得**报"安装成功" |
| `health-failed` | 进入修复流程（§18），并明确区分"安装问题"与"环境问题"（F21/F22） |

### 22.3 安装路径：用户可选，启动器不替用户决定

**默认值** = 官方安装器默认（`%LOCALAPPDATA%\Programs\DeepSeek Harness`）；**用户可以改**。这条替代了早期"一律用默认值"的说法（§15.2 已同步修订）。

- plan 里带 `installDir`；UI 提供目录选择器；plan 摘要必须显示最终路径，不能只在最后一步才出现。
- **校验全部在 plan 阶段做完**（别等 apply 才发现）：

| 校验 | 不通过时 |
|---|---|
| 绝对路径、已规范化（去尾随分隔符、展开 8.3 短名） | `E_INSTALL_DIR` |
| 不在另一个 DSH 安装目录内，也不是它的父目录 | `E_INSTALL_DIR` |
| 父目录可写（试写一个临时文件后删掉） | `E_PERMISSION` |
| 不是 UNC / 网络路径；可移动盘只警告不阻止 | 警告 |
| 剩余空间 ≥ 安装包大小 × 2.2（解压 + 临时） | `E_DISK_FULL` |
| 落在 `Program Files`、`C:\` 根等受保护目录 | **明确警告**"需要管理员权限，会破坏'日常免提权'的承诺" |
| 路径长度 < 200（给 asar 内的长路径留余量） | 警告 |

- **传参**：`Setup.exe /S /D=<路径>`。NSIS 的 `/D=` 必须是**最后一个参数且不加引号**；启动器直接构造 argv，不经 shell，避免空格与引号地狱。
- **装完必须重新 `discover`**（读注册表 `InstallLocation`），**绝不假定路径等于用户输入**——安装器可能规范化、可能被用户在中途改掉。

#### 22.3.1 目录已存在时：给提示，不硬拒

内核在 plan 阶段判定 `installDirState`，UI 据此给不同提示。**只有"嵌套/父目录"是硬拒**；受保护目录是强提示（需要 UAC），其余一律提示后由用户决定：

| 状态 | 判定 | 提示 | 默认焦点 |
|---|---|---|---|
| `absent` | 目录不存在 | 无 | 安装 |
| `empty` | 存在且为空 | 无（或一行"将使用已有空目录"） | 安装 |
| `existing-install` | 有 exe + asar | "这里已有一个 DSH（vX）。继续等于覆盖安装/升级"；目标版本更低时追加"会降级" | 取消 |
| `partial-install` | 只有 exe 或只有 asar | "这里有一次未完成的安装，建议先清理"，给"清理后安装" | 取消 |
| `foreign-files` | 非空且不含 DSH | "这个目录不是空的（N 个文件，不属于 DSH）。安装器不会清理它们" | **换个目录** |
| `nested` | 在另一个安装内，或它是别人的父目录 | 硬拒 `E_INSTALL_DIR` | — |

规则：

- 提示文案遵守 §20：一句人话 + 一个下一步 + 说清影响（覆盖什么、留下什么）。
- `foreign-files` 的默认动作是"换个目录"，**不是**"仍然安装"。
- 用户确认后，plan 把确认项写进 `acknowledgedWarnings[]`；**apply 校验这些确认项**，缺失即 `E_CONFIRMATION_REQUIRED`——防止 UI 漏弹窗就开装。
- 降级（目标版本 < 现有版本）永远二次确认，并提示"会话格式可能不可降级"（F15）。
- `partial-install` 的"清理"必须走官方 uninstaller 或明确的删除清单，不能 `rmdir` 整棵树。

### 22.4 空基线

全新安装没有旧状态可快照。规则：apply 前写一条**空基线**快照，内容为"此前无安装 + 目标路径 + 时间戳 + plan 指纹"。

这样"取消/失败"能回滚到一个**明确的**状态（确实没装过），而不是"不知道之前有什么"。它也满足 §16.1 的强承诺：apply 的第一个可回滚检查点永远是"快照已落盘且校验通过"，全新安装也不例外。

### 22.5 装完自动体检

`rediscovering` 成功后**立即**跑一次 `doctor`（只读、限定该安装），findings 直接进 UI：

- 全绿 → 主按钮变"启动"。
- 有可修项 → "已安装，但有 N 个问题，可一键修复"。
- 有环境故障（F21/F22）→ 明确说"不是安装失败"，避免用户重装一遍。

### 22.6 失败与回滚

- 下载/校验失败 → 删缓存条目，回到 plan。
- 安装器非 0 退出 → 读退出码与日志尾部；按 F20 判定半装，半装则调官方 uninstaller 清理，回到 `no-install`。
- 装完 `discover` 仍为空 → 失败，进 `install-failed`，不得宣称成功。

### 22.7 P0 期间必须实测的四件事

1. `oneClick: false` + `allowToChangeInstallationDirectory` 时，`/D=` 是否真的生效（若安装器忽略它，就退化为"交互安装 + 用户手选"）。
2. `/S` 静默安装是否需要交互式 UAC（per-user 预期不需要，要实测）。
3. 安装器对非 ASCII 路径（中文用户名/目录）的行为。
4. 安装器的退出码语义（0 是否唯一成功值、被取消时返回什么）。

## 23. 决策记录（2026-10-31）

| # | 决策 | 落地位置 |
|---|---|---|
| 1 | 对外产品名暂不定，正式版发布前更名 | 文档头 |
| 2 | 许可证 MIT（代码与规范同一许可） | 文档头 |
| 3 | 自更新源 GitHub Releases，支持镜像配置 | §7.8 |
| 4 | v1 只做 Windows x64；macOS / Linux / ARM64 后续加入 | §6、§23.1 |
| 5 | 托盘常驻，可在设置关闭；开机自启默认关 | §20.3 |
| 6 | DSH 版本窗口 = 当前 + 前 2 个 minor | §15.5 / §16.3 |
| 7 | 快照默认保留 5 个、可调；触发条件 = 可能影响启动的操作；用户可手动创建 | §16.1 |
| 8 | plan 有效期 15 分钟 | §7.3 / §19.2 |
| 9 | 机器级锁作用域 = per-machine（跨 Windows 用户互斥） | §15.3 |
| 10 | 卸载时询问是否删除数据目录，默认保留 | §7.8 |
| 11 | 启动器自身损坏 → 安装器"修复"模式（覆盖重装、保留数据） | §7.8 |
| 12 | 旧版本保留 2 个、缓存上限 2 GB | §7.8 / §13 |
| 13 | 第三方客户端（dsh-TUI）的插件体系只读，不代管 | §21.6 |
| 14 | 首次安装默认 desktop；空基线；装完自动体检；web/tui 只引导不代装 | §22 |
| 15 | **不购买代码签名证书**：始终未签名分发，防篡改靠自管 ed25519 签名的发布清单 + 逐文件 sha256 | §6、§7.8、§13、§20 |

### 23.1 平台路线

**v1：Windows 10/11 x64。** 之后按顺序：Windows ARM64 → macOS（Apple Silicon 优先）→ Linux。

每加一个平台要重做的部分：进程探测、路径规则、安装器格式、签名与更新链。**协议本身不变**——这正是内核/UI 分离与"布局描述符按（形态, 版本）索引"的收益：新平台是新增描述符与适配器，不是改协议。

## 24. 明确的产品判断（供评审挑战）

- 宁可少一个功能，也不要多一个"需要管理员权限的修复流程"。
- 宁可规范小一页，也不要多一个"只有参考实现知道的行为"。
- 界面的还原度服从于"故障时能用"：GUI 起不来时，CLI 必须完整。
