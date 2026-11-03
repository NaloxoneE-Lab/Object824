# 装 MSVC 工具链（Tauri 的前置）

## 为什么需要你动手

- **当前进程没有管理员权限**，而 VS Build Tools 安装器必须提权；本会话的 UAC 请求被禁用，我无法请求。
- **C: 只剩 2.8 GB**。VS Build Tools 的 C++ 工作负载装完通常占 4–7 GB，装 C: 会失败或把系统盘塞满。

所以安装包已经下到 D 盘，命令也备好了，只需要你在**管理员** PowerShell 里跑一次。

## 一条命令

安装包：`.toolchain\vs\vs_buildtools.exe`（4,473,832 字节，SHA256 见同目录 `SHA256`）

以管理员身份打开 PowerShell，然后：

```powershell
& "D:\dsh work\Object 824\o824-rs\.toolchain\vs\vs_buildtools.exe" `
  --quiet --wait --norestart `
  --installPath "D:\VS\BuildTools" `
  --add Microsoft.VisualStudio.Workload.VCTools `
  --add Microsoft.VisualStudio.Component.Windows11SDK.22621 `
  --includeRecommended
```

- `--installPath "D:\VS\BuildTools"`：装到 D 盘，避开 C: 的 2.8 GB。
- `VCTools`：C++ 生成工具（`link.exe`、`lib.exe`）。
- `Windows11SDK.22621`：Windows SDK（`rc.exe` 与系统库）。若你的机器 SDK 版本不同，可以去掉这一项，让安装器选默认。
- `--quiet --wait`：无人值守，装完才返回。全程约 10–20 分钟，取决于网络。

装完自查：

```powershell
& "C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
```

有输出就是成功。

## 装完之后这边要做的

1. `rustup toolchain install stable-x86_64-pc-windows-msvc`（装进 `.toolchain`，不外溢）。
2. `rustup target add x86_64-pc-windows-msvc`。
3. 在 `.cargo/config.toml` 里给 msvc target 指向 `link.exe`，或让环境变量 `LIB`/`PATH` 带上 VS 的 `VC\Tools\MSVC\*\bin\Hostx64\x64`。

## 如果不想装

设计文档 §7.4 已经写明回退方案：**WPF（.NET Framework 4.8，零依赖、单 exe）**，代价是界面动效要自己造。本机 `csc.exe` 已存在，`dotnet` 只有运行时没有 SDK，所以 WPF 是"今天就能开工"的那条路。

无论走哪条，**下面这层与 UI 工具无关**，我现在就可以开工：

- `o824-bridge`：GUI 的后端——spawn `o824 kernel --stdio`、按帧通信、暴露 discover/doctor/plan/apply/start/stop，可以用真内核和 fixture 做无头测试。
- 展示模型：把 doctor 的 findings 变成 §20.1 那三句话（"还没装 DSH" / "可以启动" / "起不来，原因…可自动修"），以及 §20 十条约束里可测的部分（i18n 词条完整、每个错误带下一步、破坏性动作要二次确认）。

Tauri 与 WPF 都吃同一份展示模型，所以这部分不会白做。

---

## 2026-11-03：MSVC 装好之后的结果

`D:\VS\BuildTools`（MSVC 14.44.35207）+ `D:\Windows Kits\10`（SDK 10.0.26100.0）就位后：

| 步骤 | 结果 |
|---|---|
| `rustup toolchain install stable-x86_64-pc-windows-msvc`（装进 `.toolchain`） | ✅ |
| `rustc` 编 hello world（MSVC 目标） | ✅ 运行输出 `msvc ok` |
| **Tauri 2.12.5 全依赖树编译**（约 400 个 crate） | ✅ |
| `tauri-build` 生成 Windows 资源 | ✅（需要 `icons/icon.ico`，已生成） |
| `cargo build` 产出 exe | ✅ 12 MB |
| **运行 exe 开窗口** | ❌ `Failed to setup app: 拒绝访问 (os error 5)`（`tauri::app` setup 阶段） |

排查：`APPDATA` / `LOCALAPPDATA` / `TEMP` 都可写（实测写入成功），`WEBVIEW2_USER_DATA_FOLDER` 也指到了可写目录，仍然拒绝。**最可能的原因是这个 agent 会话不是交互式桌面会话**，WebView2 无法创建窗口——不是代码问题。

**待你验证**：双击运行

```
D:\dsh work\Object 824\o824-rs\.toolchain\tauri-spike\target\debug\o824-gui-spike.exe
```

看到标题为 `O824` 的窗口就说明壳没问题；如果也报同样的错，那就是 WebView2 环境的问题，需要单独查。

### 本机启用 MSVC 的两条命令

```bash
source scripts/env.sh          # 自包含的 gnu 工具链（默认）
source scripts/env-msvc.sh     # MSVC + Windows SDK（路径自动探测）
export RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc
cargo build                    # 现在编出来的是 MSVC 目标
```

`scripts/env-msvc.sh` 的路径是从 vswhere 与 SDK 目录**探测**出来的，不写死；换机器只要装在同一类位置就能用。

## 双击闪退的真相（2026-11-03）

现象：**以管理员运行正常，直接双击闪退。**

原因不在 Tauri，而在 WebView2 的数据目录：

- WebView2 默认把数据放在 `%LOCALAPPDATA%\<identifier>`。
- 管理员运行时创建的那个目录带**高完整性级别**（High IL），普通权限的进程即使 DACL 里写着
  FullControl 也写不进去——Windows 的强制完整性检查先于 DACL 生效。
- 于是双击（Medium IL）在创建窗口前就 Access Denied，进程直接退出，看起来就是"闪退"。

那条被污染的目录是 `%LOCALAPPDATA%\dev.o824.gui.spike`，已删除。

**真 app 的修法**（已实现，`apps/o824-gui/src/main.rs::prepare_webview_data`）：

1. 自己在**当前用户**身份下创建 `%LOCALAPPDATA%\O824\WebView2`；
2. 把它设成 `WEBVIEW2_USER_DATA_FOLDER`；
3. 如果连窗口都起不来，弹原生 MessageBox 指向命令行（§7.4），而不是静默退出。

这样即使以后有人用管理员跑过一次，也不会毁掉普通用户的启动路径——因为路径是 app 自己建的、
属于当前用户。

## 另一个环境的坑（只影响我这个会话）

我的进程跑在受限令牌 `CodexSandboxUsers` 下（ACL 里可见：`ReadAndExecute`），所以**我这边**启动
GUI 一定失败，与你看到的闪退是两回事。判断 GUI 是否可用，只能由你在桌面上双击验证。

## 第二次闪退：数据目录建不出来（2026-11-03 深夜）

加了日志之后拿到了确切原因：

```
webview data dir FAILED: ...\AppData\Local\O824-webview2 (拒绝访问 os error 5)
webview data dir FAILED: ...\TEMP\O824-webview2 (拒绝访问 os error 5)
PANIC: tauri ... Failed to setup app: 拒绝访问
```

**我的会话建不出这个目录**（沙箱令牌只有受限权限），于是回退到 WebView2 的默认目录，同样被拒，
Tauri 在 setup 阶段 panic。而 Tauri 是**用 panic 报告 setup 失败**的——所以我原先写在 `run()`
之后的对话框永远轮不到执行，这就是"闪退且无声"的机制。

现在 app 的行为：

1. 浏览器数据目录有**回退链**：`%LOCALAPPDATA%\O824-webview2` → `%TEMP%\O824-webview2` → WebView2 默认；
2. 全都失败时**不再静默退出**：`catch_unwind` 包住 `run()`，弹原生对话框，**写明哪个目录、什么错误码**（§7.4）；
3. 日志写 `%LOCALAPPDATA%\O824-gui.log`，写不进去就写 `%TEMP%\O824-gui.log`——**建文件而不是建目录**，
   因为在"建目录被拒"的地方建文件常常仍然允许。

## 判断 GUI 是否可用的唯一办法

我的会话沙箱受限，**不能**作为判据。只能由你在桌面上双击验证；如果仍然失败，对话框现在会写明
是哪个目录、什么错误码，把那段文字发我即可定位。
