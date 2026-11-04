# Installs the launcher where it belongs and starts it.
#
# Why this exists: running the executable straight out of the build tree does not, on this
# machine, produce a notification-area icon — files inside the DSH workspace carry a
# low-integrity mandatory label, and a process started from one runs at low integrity, which
# the notification area refuses. The same bytes in %LOCALAPPDATA% work. That is also where a
# real installer would put them, so this is less a workaround than the honest layout.
#
#   pwsh -File scripts/install-local.ps1
param(
    [string]$Source = "",
    [string]$Target = "",
    [switch]$NoLaunch
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
if (-not $Source) { $Source = Join-Path $root "apps\o824-gui\target\release" }
if (-not $Target) { $Target = Join-Path $env:LOCALAPPDATA "o824" }

foreach ($name in @("o824-gui.exe", "o824.exe")) {
    $from = Join-Path $Source $name
    if (-not (Test-Path $from)) {
        Write-Error "cannot find $from - build first: cargo build --release --manifest-path apps/o824-gui/Cargo.toml"
        exit 1
    }
}

# A running copy holds the executable; stop it before replacing.
Get-Process o824-gui -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 800

New-Item -ItemType Directory -Force -Path $Target | Out-Null
foreach ($name in @("o824-gui.exe", "o824.exe")) {
    Copy-Item (Join-Path $Source $name) $Target -Force
    Write-Output "installed $name -> $Target"
}

# The label is the whole reason for this script; say it out loud rather than assume it.
$acl = (icacls (Join-Path $Target "o824-gui.exe")) -join " "
if ($acl -match "Mandatory Label\\Low") {
    Write-Warning "the installed copy still carries a low-integrity label; the icon will not appear"
} else {
    Write-Output "no low-integrity label: the notification-area icon can register"
}

if (-not $NoLaunch) {
    Start-Process -FilePath (Join-Path $Target "o824-gui.exe")
    Write-Output "started; the log is at $(Join-Path $Target 'o824-gui.log')"
}
