# 对每个场景跑一遍：doctor -> repair --yes -> doctor
param([string[]]$Scenarios = @('f03','f01','f02','f16','f17','f20'))

$ErrorActionPreference = 'Continue'
$Lab  = 'D:\dsh work\broken-dsh-lab'
$Exe  = 'D:\dsh work\Object 824\o824.exe'

foreach ($s in $Scenarios) {
  & (Join-Path $Lab 'build.ps1') -Scenario $s | Out-Null
  $app  = Join-Path $Lab "$s\app"
  $homeDir = Join-Path $Lab "$s\home"
  $env:O824_DATA_ROOT = Join-Path $Lab "$s\data"

  Write-Output ""
  Write-Output "############ $s ############"

  $out = & $Exe doctor --root $app --home $homeDir 2>&1 | Out-String
  $d1  = $LASTEXITCODE
  Write-Output "--- doctor #1 (exit $d1) ---"
  Write-Output $out.Trim()

  $out2 = & $Exe repair --root $app --home $homeDir --yes 2>&1 | Out-String
  $r = $LASTEXITCODE
  Write-Output "--- repair --yes (exit $r) ---"
  Write-Output $out2.Trim()

  $out3 = & $Exe doctor --root $app --home $homeDir 2>&1 | Out-String
  $d2  = $LASTEXITCODE
  Write-Output "--- doctor #2 (exit $d2) ---"
  Write-Output $out3.Trim()
}
$env:O824_DATA_ROOT = $null
