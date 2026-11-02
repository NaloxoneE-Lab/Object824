# Builds the release assets for the `o824-mock` npm package.
#
# Two products come out of one Rust binary, so both are built here and both are
# shipped:
#
#   dist/o824-mock-<version>.tgz   the package (wasm + launcher + recordings)
#   dist/o824-mock.exe             the same replayer as a native binary, for people on
#                                  Windows who would rather not go through Node
#   dist/SHA256SUMS                checksums for both
#
# The wasm module is a build product and is never committed: a binary in the repository
# drifts from the source that should have rebuilt it. The prepack guard in
# packages/o824-mock/scripts refuses to pack without it, so a half-built package cannot
# reach a release.
#
# Publishing is deliberately not automated: creating a release and uploading assets
# needs GitHub API credentials, and this repository is pushed with a deploy key that
# cannot call the API. The script prints what to drag where.
#
# Usage (from o824-rs):
#   pwsh -File scripts/build-mock-package.ps1 -Version 0.1.2

param([string]$Version = "")

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
  $packageDir = Join-Path (Join-Path $root 'packages') 'o824-mock'
  $manifestPath = Join-Path $packageDir 'package.json'
  $manifest = Get-Content $manifestPath -Raw | ConvertFrom-Json
  if (-not $Version) { $Version = $manifest.version }
  if ($Version -ne $manifest.version) {
    throw ("packages/o824-mock/package.json says {0} but {1} was requested. The manifest is " +
      "the single source of the version: bump it, commit it, then build.") -f $manifest.version, $Version
  }
  Write-Output "version $Version"

  # The toolchain is self-contained; a plain `pwsh` has no cargo on PATH.
  $cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
  if (-not $cargo) {
    $candidate = Join-Path (Join-Path (Join-Path (Join-Path $root '.toolchain') 'cargo') 'bin') 'cargo.exe'
    if (Test-Path $candidate) { $cargo = $candidate } else { throw 'cargo not found (run scripts/env.ps1 first)' }
  }

  $dist = Join-Path $root 'dist'
  New-Item -ItemType Directory -Force -Path $dist | Out-Null

  Write-Output '== building the host binary =='
  & $cargo build --release -p o824-mock --bin o824-mock
  if ($LASTEXITCODE -ne 0) { throw 'cargo build (host) failed' }
  Copy-Item (Join-Path (Join-Path (Join-Path $root 'target') 'release') 'o824-mock.exe') (Join-Path $dist 'o824-mock.exe') -Force

  Write-Output '== building the wasm module =='
  & $cargo build --release -p o824-mock --bin o824-mock --target wasm32-wasip1
  if ($LASTEXITCODE -ne 0) { throw 'cargo build (wasm32-wasip1) failed' }
  $builtWasm = Join-Path (Join-Path (Join-Path (Join-Path $root 'target') 'wasm32-wasip1') 'release') 'o824-mock.wasm'
  if (-not (Test-Path $builtWasm)) { throw "the wasm build produced no $builtWasm" }
  Copy-Item $builtWasm (Join-Path $packageDir 'o824-mock.wasm') -Force

  Write-Output '== copying the recordings =='
  $transcripts = Join-Path $packageDir 'transcripts'
  New-Item -ItemType Directory -Force -Path $transcripts | Out-Null
  Copy-Item (Join-Path (Join-Path (Join-Path $root 'spec') 'transcripts') '*.jsonl') $transcripts -Force

  Write-Output '== packing =='
  Push-Location $packageDir
  try {
    & npm pack --pack-destination $dist
    if ($LASTEXITCODE -ne 0) { throw 'npm pack failed' }
  }
  finally { Pop-Location }

  $tgz = Join-Path $dist "o824-mock-$Version.tgz"
  if (-not (Test-Path $tgz)) { throw "npm pack produced no $tgz" }

  Write-Output '== checksums =='
  $sums = @()
  $assets = @($tgz)
  if ($IsWindows) { $assets += $hostExe }
  foreach ($file in $assets) {
    $hash = (Get-FileHash -Algorithm SHA256 -Path $file).Hash.ToLowerInvariant()
    $sums += "$hash  $(Split-Path -Leaf $file)"
  }
  $sums | Set-Content -Path (Join-Path $dist 'SHA256SUMS') -Encoding ascii
  $sums | ForEach-Object { Write-Output "  $_" }

  Write-Output ''
  Write-Output 'assets ready in dist/. To publish:'
  Write-Output "  1. git tag -a v$Version -m 'o824-mock $Version' && git push origin v$Version"
  Write-Output "  2. open https://github.com/NaloxoneE-Lab/Object824/releases/new?tag=v$Version"
  Write-Output "  3. attach dist/o824-mock-$Version.tgz, dist/o824-mock.exe and dist/SHA256SUMS"
  Write-Output '  4. publish, then verify the install line from the release page:'
  Write-Output "     npm i -D https://github.com/NaloxoneE-Lab/Object824/releases/download/v$Version/o824-mock-$Version.tgz"
  Write-Output '     npx o824-mock --transcript doctor-clean'
}
finally {
  Pop-Location
}
