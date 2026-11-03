#!/usr/bin/env bash
# Build, launch, and capture the O824 window — with the checks that make the capture
# trustworthy.
#
# Two things silently invalidated earlier attempts: the frontend is embedded at compile
# time (a changed file that is not rebuilt is not in the binary), and a running instance
# holds the executable (cargo keeps the old binary). So this script refuses to capture
# unless the process started *after* the binary was written.
#
#   scripts/gui-shot.sh <output.png> [seconds-to-wait]
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="$ROOT/apps/o824-gui"
OUT="${1:-$APP/shot.png}"
WAIT="${2:-6}"
EXE="$APP/target/release/o824-gui.exe"

if [ ! -f "$EXE" ]; then
  echo "gui-shot: build first (source scripts/env-msvc.sh && cd apps/o824-gui && cargo build --release)" >&2
  exit 1
fi

# A running copy would keep the old binary and the old window.
if pwsh -NoProfile -Command 'Get-Process o824-gui -ErrorAction SilentlyContinue | Stop-Process -Force' 2>/dev/null; then
  sleep 1
fi

EXE_MTIME=$(stat -c %Y "$EXE")

"$EXE" >/dev/null 2>&1 &
sleep "$WAIT"

pwsh -NoProfile -Command "
\$p = Get-Process o824-gui -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not \$p) { 'gui-shot: the window is not running' ; exit 1 }
\$started = [DateTimeOffset]::new(\$p.StartTime).ToUnixTimeSeconds()
\$exe = [DateTimeOffset]::new((Get-Item '$EXE').LastWriteTime).ToUnixTimeSeconds()
'started=' + \$started + ' exe=' + \$exe
if (\$started -lt \$exe) { 'gui-shot: the window predates the binary; refusing to capture' ; exit 2 }
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices;
public class Shot {
  [DllImport(\"user32.dll\")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport(\"user32.dll\")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int ht, bool repaint);
}
'@
[Shot]::MoveWindow(\$p.MainWindowHandle, 40, 40, 940, 700, \$true) | Out-Null
[Shot]::SetForegroundWindow(\$p.MainWindowHandle) | Out-Null
Start-Sleep -Milliseconds 900
\$bmp = New-Object System.Drawing.Bitmap 940, 700
\$g = [System.Drawing.Graphics]::FromImage(\$bmp)
\$g.CopyFromScreen(40, 40, 0, 0, \$bmp.Size)
\$bmp.Save('$OUT', [System.Drawing.Imaging.ImageFormat]::Png)
'saved: $OUT'
"
