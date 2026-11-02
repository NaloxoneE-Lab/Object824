# Acceptance check for `repair --with-install` (the staged pnpm reinstall).
#
# This is the one repair that cannot be exercised from a fixture: it needs the
# host application's own executable and the pnpm it ships, and it really installs a
# dependency tree. The check therefore builds a throwaway DSH home beside the real
# installation — the app root is the real one, the home is a copy — breaks one
# link the way F01 does, and verifies that the repair:
#
#   1. plans a single reinstall-profile step under --with-install,
#   2. runs the host's pnpm and swaps a real node_modules tree in,
#   3. leaves the profile diagnosing clean,
#   4. records a "done" journal with the step's note,
#   5. leaves no staging directory behind.
#
# The real profile is never written to: only the throwaway copy is.
#
# Usage (from o824-rs, after a release build):
#   pwsh -File scripts/with-install-check.ps1

param(
  [string]$Exe = "",
  [string]$RealApp = "D:\Users\X_Cyanogen\AppData\Local\Programs\DeepSeek Harness",
  [string]$RealHome = "C:\Users\X_Cyanogen\.dsh"
)

$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Exe) { $Exe = Join-Path $root 'target\release\o824.exe' }
if (-not (Test-Path $Exe)) { throw "missing $Exe; run scripts/release.ps1 first" }

$work = Join-Path $root '.toolchain\with-install'
$home2 = Join-Path $work 'home'
$profile = Join-Path $home2 'profiles\desktop'
$vendorEmpty = Join-Path $work 'vendor\empty'
$data = Join-Path $work 'data'

Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $profile, $vendorEmpty, $data | Out-Null

# The five settings files, copied from the real profile; node_modules is not
# copied because the whole point is to rebuild it.
foreach ($file in 'package.json', 'cordis.patch.yml', 'cordis.yml', 'pnpm-lock.yaml', 'pnpm-workspace.yaml') {
  Copy-Item (Join-Path $RealHome "profiles\desktop\$file") (Join-Path $profile $file)
}

# The F01 shape: a link whose target exists but carries no package.json.
New-Item -ItemType Directory -Force -Path (Join-Path $profile 'node_modules\@deepseek-ai') | Out-Null
& cmd /c mklink /J (Join-Path $profile 'node_modules\@deepseek-ai\dsh-base') $vendorEmpty | Out-Null
if (-not (Test-Path (Join-Path $profile 'node_modules\@deepseek-ai\dsh-base'))) {
  throw 'cannot create the broken link fixture'
}

$env:O824_DATA_ROOT = $data
$failures = 0
function Check([string]$name, [bool]$ok, [string]$detail) {
  if ($ok) { Write-Output ("{0,-34} PASS" -f $name) }
  else { Write-Output ("{0,-34} FAIL  {0}: $detail" -f $name); $script:failures++ }
}

$before = (& $Exe doctor --root $RealApp --home $home2 2>&1 | Out-String)
Check 'doctor reports F01' ($before -match 'F01') $before.Trim()

$plan = (& $Exe repair --root $RealApp --home $home2 --with-install --dry-run 2>&1 | Out-String)
Check 'the plan is one reinstall step' (($plan -match 'reinstall-profile') -and ($plan -match 'steps: 1')) $plan.Trim()

$sw = [Diagnostics.Stopwatch]::StartNew()
$repair = (& $Exe repair --root $RealApp --home $home2 --with-install --yes 2>&1 | Out-String)
$repairExit = $LASTEXITCODE
$elapsed = [int]$sw.Elapsed.TotalSeconds
Check 'the repair applies' ($repairExit -eq 0) "$repairExit`: $($repair.Trim())"

$after = (& $Exe doctor --root $RealApp --home $home2 2>&1 | Out-String)
Check 'the profile diagnoses clean' ($after -match 'no findings') $after.Trim()

$nodeModules = Join-Path $profile 'node_modules'
$entries = if (Test-Path $nodeModules) { (Get-ChildItem $nodeModules -ErrorAction SilentlyContinue).Count } else { 0 }
Check 'a dependency tree was swapped in' ($entries -gt 1) "entries=$entries"
Check 'no staging copy is left behind' (-not (Test-Path "$profile.o824-stage")) 'staging directory still exists'
Check 'no old tree is left behind' (-not (Test-Path "$nodeModules.o824-old")) 'old tree still exists'

$journal = Get-ChildItem (Join-Path $data 'journal') -ErrorAction SilentlyContinue |
  Sort-Object LastWriteTime | Select-Object -Last 1
$journalText = if ($journal) { Get-Content $journal.FullName -Raw } else { '' }
Check 'the journal records done' ($journalText -match '"state": "done"') $journalText
Check 'the journal records the step note' ($journalText -match 'reinstalled \d+ top-level') $journalText

$env:O824_DATA_ROOT = $null
Write-Output ''
Write-Output ("elapsed={0}s  top-level packages={1}" -f $elapsed, $entries)
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
if ($failures -gt 0) { exit 1 }
