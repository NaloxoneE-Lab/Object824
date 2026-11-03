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
