# Activates the self-contained Rust toolchain that lives inside this repository.
#
#   . .\scripts\env.ps1
#
# Nothing is installed outside o824-rs: rustup, the cargo home, and the portable
# mingw-w64 linker all live under .toolchain\ (git-ignored). The linker is
# needed because this machine has no Visual Studio build tools, so the toolchain
# is x86_64-pc-windows-gnu rather than the MSVC host CI uses.

$env:O824_RS_ROOT = Split-Path -Parent $PSScriptRoot

$env:CARGO_HOME = Join-Path $env:O824_RS_ROOT '.toolchain\cargo'
$env:RUSTUP_HOME = Join-Path $env:O824_RS_ROOT '.toolchain\rustup'

$tmp = Join-Path $env:O824_RS_ROOT '.toolchain\tmp'
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
$env:TMPDIR = $tmp
$env:TMP = $tmp
$env:TEMP = $tmp

$cargoBin = Join-Path $env:CARGO_HOME 'bin'
$mingwBin = Join-Path $env:O824_RS_ROOT '.toolchain\mingw64\bin'
$env:PATH = "$cargoBin;$mingwBin;$env:PATH"

Write-Host "o824-rs: $(& cargo --version 2>$null)"
