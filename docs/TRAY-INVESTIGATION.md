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

## 决定

**删除托盘功能。** 理由：

1. 无法验证的能力不该出现在界面里（§20 的原则：不承诺做不到的事）；
2. 它当前唯一的用途是"关窗后仍能找回窗口"，而这在没有可见图标时会让程序失去入口；
3. 关窗退出符合窗口关闭按钮的直觉。

后续如果要做，建议先用一个最小 C# 程序验证 `NotifyIcon` 与 P/Invoke 的差异到底在哪，再决定用哪种实现；不要在没搞清原因前重新接线。
