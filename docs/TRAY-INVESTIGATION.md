# 托盘图标为什么没出现（2026-11-04 排查记录）

结论先说：**没有找到原因，所以这个功能被删除了。**

## 现象

窗口里建了托盘图标，Tauri 报"已创建"（`tray: created`，32×32 图标），但：

- 任务栏可见区域没有；
- 折叠区（`^`）里没有；
- 应用运行期间用 UI Automation 枚举 `Shell_TrayWnd` 与 `NotifyIconOverflowWindow` 都没有。

## 排查过程与结论

| 检查 | 结果 | 是否排除 |
|---|---|---|
| 图标像素是否透明 | 32×32，1024 像素中 956 个不透明 | 排除 |
| 进程是否受限令牌 | `TokenIsRestricted = false` | 排除 |
| 完整性级别 | 本进程与 Explorer 都是 Medium，且都 `not elevated` | 排除 |
| 残留 explorer 进程 | 曾有 4 个，清掉 3 个残骸后仍失败 | 排除 |
| 窗口类型 | Tauri 窗口、自建顶层窗口、仅消息窗口，三种都失败 | 排除 |
| 回调消息范围 | `0x401`、`0x800`、`0x8001`、`0x801` 都失败 | 排除 |
| 注册时机 | `setup` 里与事件循环 `Ready` 之后各试一次，都失败 | 排除 |
| 直接调 `Shell_NotifyIconW` | `FALSE`，`GetLastError() = 5 (ACCESS_DENIED)`，`cbSize=976` | — |
| **对照实验：.NET `System.Windows.Forms.NotifyIcon`** | **成功**，图标出现在任务栏（tooltip "O824 control test"） | **未排除** |

也就是说：**同一台机器、同一个用户、同一时刻，.NET 能加图标，我们直接调 API 不能。**

## 仍未解释的差别

同一个 PowerShell 进程里，托管 `NotifyIcon` 成功而手工 P/Invoke 失败，说明差别在**调用本身**而不是进程环境。已试过 .NET 用到的参数组合（`uID`、`uFlags` 含 `NIF_SHOWTIP`、`uCallbackMessage = 0x800`、系统图标句柄）仍然失败。

要继续查需要：
- Process Monitor 抓 `Shell_NotifyIcon` 前后的注册表/文件访问；
- 或在 shell 侧挂调试器看它为什么拒绝。


## 2026-11-04 晚：找到一个真正的判别条件

用户下载了 Procmon64.exe，但**Process Monitor 需要管理员**（加载内核过滤驱动），本机账户不是管理员，
所以那条路走不通。改用自建探针（`D:\dsh work\Object 824	ray-probe.exe`，配套源码 `tray-probe.cs`）。

### 探针做了什么

一次运行里试六种组合，并且**自判是否真的注册成功**（`NIM_ADD` 之后再发 `NIM_MODIFY`——图标若真在，
shell 会接受修改）：

1. flags `0x7` / cb 976
2. flags `0x87`（含 `NIF_SHOWTIP`）/ cb 976
3. flags `0x7` / cb 952（老结构布局）
4. flags `0x7` / 回调消息 `0x800`
5. 消息循环启动后再试（前两种）
6. `System.Windows.Forms.NotifyIcon`

### 结果

| 场景 | 结果 |
|---|---|
| 探针在**我的会话**里跑 | 六种全部 `added=False error=5`，`proves-registered=False` |
| 探针由**用户双击**（不在我的沙箱里） | **完全一样** |
| 同一段代码在 **`powershell.exe`（微软签名）里跑** | **`added=True error=0 proves-registered=True`** |
| 探针**自签名**后（自签证书，`UnknownError`） | 仍然 `error=5` |

### 由此可以确定

- **不是参数**：六种组合、两种结构大小、消息循环前后，全部同一个错误码；
- **不是我的沙箱**：用户双击也是同样结果；
- **不是窗口类型**：Tauri 窗口、自建 Win32 窗口、WinForms 窗口都试过；
- **不是"有没有签名文件"**：自签名无效；
- **与进程镜像本身的可信度相关**：同一台机器、同一会话、同一用户、同一段代码，微软签名的宿主成功，
  我们自己编译的 exe 被拒绝。

另外：这台机器的 SmartScreen **会拦截这两个未签名 exe**（"Windows 已保护你的电脑 / 无法识别的应用"），
说明本机对未知二进制的判定是活跃的。两者是否同一机制，我没有证据；但**它们同时出现**。

### 顺带纠正一个我犯过的错

早前我写过"同一台机器上 .NET 的 `NotifyIcon` 能加图标"——那是**错的**。WinForms 的 `NotifyIcon`
根本不检查 `Shell_NotifyIcon` 的返回值，`Visible = true` 失败也不会抛异常；我当时的"证据"是一次
UI Automation 枚举（那个枚举现在连 OneDrive 等已有图标都枚举不到，本身就不可靠）。


## 最终根因（2026-11-04 深夜，已验证）

**不是签名，不是参数，不是沙箱会话——是运行位置。**

同一个未签名 exe，三个位置：

| 位置 | 结果 |
|---|---|
| `D:\dsh work\Object 824\`（DSH 工作区） | `added=False error=5` |
| `D:\o824-probe-test\`（D 盘别处） | **`added=True`**，注册成功 |
| `C:\Users\X_Cyanogen\AppData\Local\o824-probe\` | **`added=True`**，注册成功 |

差别在文件的**强制完整性标签**：

```
icacls "D:\dsh work\Object 824\tray-probe.exe"
  ... Mandatory Label\Low Mandatory Level:(I)(NW)     <- 关键

icacls "D:\o824-probe-test\tray-probe.exe"
  ...（没有这一行）
```

从 **Low 完整性文件**启动的进程以 Low IL 运行；通知区域是 Medium，**UIPI 不允许低完整性进程向它注册**，
于是 Shell_NotifyIcon 返回 FALSE + ERROR_ACCESS_DENIED。

这解释了为什么试过的一切都没用：参数、结构大小、消息循环时机、窗口类型、签名、自签名、微软签名的宿主
（powershell.exe 在 C:\Windows 下，文件没有低完整性标签）——变量根本不是那些。

**结论**：托盘代码本身没问题；它失败是因为 exe 放在 DSH 工作区里。真实安装位置
（%LOCALAPPDATA% 或 Program Files）不会有这个标签。

### 之前的结论（已被上面推翻，保留作记录）


未签名启动器在**这类机器上**无法获得通知区域图标。按 §13（不购买代码签名证书、未签名分发），
以及 §20（不承诺做不到的事、不劝关闭安全软件），**产品里不做托盘**：关窗即退出。写在这里，是为了
下次有人再想加托盘时，从这份证据开始，而不是从零开始。

## 决定

**删除托盘功能。** 理由：

1. 无法验证的能力不该出现在界面里（§20 的原则：不承诺做不到的事）；
2. 它当前唯一的用途是"关窗后仍能找回窗口"，而这在没有可见图标时会让程序失去入口；
3. 关窗退出符合窗口关闭按钮的直觉。

后续如果要做，建议先用一个最小 C# 程序验证 `NotifyIcon` 与 P/Invoke 的差异到底在哪，再决定用哪种实现；不要在没搞清原因前重新接线。
