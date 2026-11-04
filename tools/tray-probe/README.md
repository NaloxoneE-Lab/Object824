# tray-probe

A 60-line diagnostic that asks Windows for a notification-area icon six ways and writes down
what the shell answered. It exists because the launcher's icon never appeared for an entire
evening, and every explanation tried before this tool was wrong.

## What it established (2026-11-04)

| Where the executable ran | Result |
|---|---|
| `D:\dsh work\Object 824\` (the DSH workspace) | `added=False error=5` |
| `D:\o824-probe-test\` | `added=True`, registered |
| `%LOCALAPPDATA%\o824-probe\` | `added=True`, registered |
| the same code inside Microsoft-signed `powershell.exe` | `added=True` |

The difference is a mandatory integrity label on the file:

```
icacls "D:\dsh work\Object 824\tray-probe.exe"
  ... Mandatory Label\Low Mandatory Level:(I)(NW)
```

A process started from a low-integrity image runs at low integrity, and the notification area
— which is medium — refuses it with `ERROR_ACCESS_DENIED`. Nothing about the call was wrong:
six parameter combinations, two struct sizes, before and after the message loop started, on
Tauri, raw Win32 and WinForms windows, all behaved identically. A self-signed certificate
changed nothing either, which ruled out "signature trust" as the cause.

The practical rule that came out of it: **run the launcher from an install location**
(`o824 install` puts it in `%LOCALAPPDATA%\o824`), not from the build tree.

## Why it is kept

The next time an icon does not appear, this answers "is it the call, the location, or the
machine" in one run instead of a night. It also proves registration rather than assuming it:
after `NIM_ADD` it asks the shell to `NIM_MODIFY` the same icon, which only succeeds if the
icon is really there. `System.Windows.Forms.NotifyIcon` is tried too, and it is instructive
that it never throws — WinForms ignores `Shell_NotifyIcon`'s return value, so "no exception"
is not evidence of success.

## Building and running it

```
csc /nologo /target:winexe /out:tray-probe.exe /r:System.Windows.Forms.dll /r:System.Drawing.dll Program.cs
```

Double-click the result. It writes `tray-probe.log` next to itself and keeps an icon on screen
for thirty seconds. `sample-output.log` is a real run from inside the workspace.
