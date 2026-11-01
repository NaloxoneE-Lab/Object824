# Builds the release binary with the build identity injected.
#
# The reference implementation injects `Version`, `Commit` and `BuildTime` with
# `-ldflags -X`; the Rust build reads the same three values from the environment
# at compile time (see crates/o824-version). Nothing here reads the clock at run
# time, so the reported identity is exactly what was compiled in.
#
# Usage (from o824-rs):
#   pwsh -File scripts/release.ps1 [-Version 0.1.0]

param([string]$Version = "0.1.0")

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
  $commit = (& git rev-parse --short HEAD 2>$null)
  if (-not $commit) { $commit = "" }
  $built = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")

  # The toolchain identity is part of the bundle's version.json.
  $cargoHome = Join-Path $root '.toolchain\cargo'
  $rustc = Join-Path $cargoHome 'bin\rustc.exe'
  $rustcVersion = if (Test-Path $rustc) { (& $rustc --version) } else { "unknown" }

  $env:O824_VERSION = $Version
  $env:O824_COMMIT = $commit
  $env:O824_BUILD_TIME = $built
  $env:O824_RUSTC_VERSION = $rustcVersion

  & cargo build --release --bin o824
  if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

  $exe = Join-Path $root 'target\release\o824.exe'
  Write-Output "built $exe"
  & $exe --version
}
finally {
  Remove-Item Env:O824_VERSION, Env:O824_COMMIT, Env:O824_BUILD_TIME, Env:O824_RUSTC_VERSION -ErrorAction SilentlyContinue
  Pop-Location
}
