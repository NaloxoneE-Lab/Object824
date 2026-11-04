# Build, launch, and capture the O824 window — with the two checks that make a capture
# trustworthy.
#
#    scripts/gui-shot.ps1 -Out shot.png [-Wait 9] [-KeepOpen]
#
# The frontend is embedded at compile time, and a running copy keeps the old binary, so this
# refuses to capture a window that started before the executable was written. The screen
# capture happens in a DPI-aware process: this machine scales its display, and a
# non-DPI-aware capture silently returns only the top-left corner of the window.
param(
    [string]$Out = "shot.png",
    [int]$Wait = 9,
    [switch]$KeepOpen
)

$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public struct ShotRect { public int Left, Top, Right, Bottom; }
public static class ShotNative {
    [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr value);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out ShotRect rect);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr window, int command);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr window, IntPtr insertAfter, int x, int y, int width, int height, uint flags);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr window, int attribute, out ShotRect rect, int size);
}
"@

# Per-monitor v2, before anything reads a window rectangle.
[void][ShotNative]::SetProcessDpiAwarenessContext([IntPtr](-4))

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root "apps/o824-gui/target/release/o824-gui.exe"
if (-not (Test-Path $exe)) {
    throw "gui-shot: build first (source scripts/env.sh; source scripts/env-msvc.sh; cd apps/o824-gui; cargo +stable-x86_64-pc-windows-msvc build --release)"
}

Get-Process o824-gui -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

$written = (Get-Item $exe).LastWriteTimeUtc
$process = Start-Process -FilePath $exe -PassThru
Start-Sleep -Seconds $Wait
$process.Refresh()
if ($process.HasExited) {
    throw "gui-shot: the window exited with code $($process.ExitCode)"
}
if ($process.StartTime.ToUniversalTime() -lt $written) {
    throw "gui-shot: the window predates the binary; refusing to capture"
}

$handle = $process.MainWindowHandle
if ($handle -eq [IntPtr]::Zero) {
    throw "gui-shot: the process has no main window"
}

# A screen capture reads whatever is on top. `SetForegroundWindow` is a request Windows may
# refuse, and when it refuses, the capture is of somebody else's window. Topmost for the two
# seconds the capture takes is the one thing the shell cannot ignore.
[void][ShotNative]::ShowWindow($handle, 5)
[void][ShotNative]::SetWindowPos($handle, [IntPtr](-1), 0, 0, 0, 0, 0x0001 -bor 0x0002 -bor 0x0040)
[void][ShotNative]::SetForegroundWindow($handle)
Start-Sleep -Milliseconds 1200

$rect = New-Object ShotRect
# The visible frame, not the window rectangle: on Windows the latter includes the invisible
# resize border and the drop shadow, which would surround every capture with the desktop.
if ([ShotNative]::DwmGetWindowAttribute($handle, 9, [ref]$rect, 16) -ne 0) {
    if (-not [ShotNative]::GetWindowRect($handle, [ref]$rect)) {
        throw "gui-shot: could not read the window rectangle"
    }
}
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
if ($width -lt 200 -or $height -lt 200) {
    throw "gui-shot: the window rectangle is $width x $height"
}

$bitmap = New-Object System.Drawing.Bitmap $width, $height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
$bitmap.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()

[void][ShotNative]::SetWindowPos($handle, [IntPtr](-2), 0, 0, 0, 0, 0x0001 -bor 0x0002)

if (-not $KeepOpen) {
    Stop-Process -Name o824-gui -Force -ErrorAction SilentlyContinue
}

"gui-shot: $Out ($width x $height, process $($process.Id))"
