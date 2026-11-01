# 在副本上造一个"坏掉的 DSH"；真机安装与 ~/.dsh 只读不写。
# 用法: pwsh -File build.ps1 -Scenario clean|f03|f01|f02|f16|f17|f20
param([Parameter(Mandatory=$true)][string]$Scenario)

$ErrorActionPreference = 'Stop'
$Lab     = 'D:\dsh work\broken-dsh-lab'
$RealApp = 'D:\Users\X_Cyanogen\AppData\Local\Programs\DeepSeek Harness'
$RealHome= 'C:\Users\X_Cyanogen\.dsh'
$Dst     = Join-Path $Lab $Scenario

if (Test-Path $Dst) { Remove-Item -Recurse -Force $Dst }

$appRes  = Join-Path $Dst 'app\resources'
$profDir = Join-Path $Dst 'home\profiles\desktop'
$nmDir   = Join-Path $profDir 'node_modules'
$dataDir = Join-Path $Dst 'data'
foreach ($d in @($appRes, (Join-Path $nmDir '@deepseek-ai'), (Join-Path $Dst 'vendor\dsh-base'),
                 (Join-Path $Dst 'vendor\dsh-ops'), (Join-Path $Dst 'vendor\dsh-base-empty'), $dataDir)) {
  New-Item -ItemType Directory -Force -Path $d | Out-Null
}

# --- app 目录：三件套要的两件；asar 用硬链接，不复制 182MB ---
Set-Content -Path (Join-Path $Dst 'app\DeepSeek Harness.exe') -Value '' -NoNewline
Set-Content -Path (Join-Path $Dst 'app\version') -Value '44.7.0'
& fsutil hardlink create (Join-Path $appRes 'app.asar') (Join-Path $RealApp 'resources\app.asar') | Out-Null

# --- home：真实 profile 的五个配置文件原样复制 ---
foreach ($f in 'package.json','cordis.patch.yml','cordis.yml','pnpm-lock.yaml','pnpm-workspace.yaml') {
  Copy-Item (Join-Path $RealHome "profiles\desktop\$f") (Join-Path $profDir $f)
}

# --- vendor：两个插件，一个空壳（缺 package.json）---
Set-Content -Path (Join-Path $Dst 'vendor\dsh-base\package.json') -Value '{"name":"dsh-base","version":"0.2.1-rc.3"}'
Set-Content -Path (Join-Path $Dst 'vendor\dsh-ops\package.json')  -Value '{"name":"dsh-ops","version":"0.4.1"}'

function New-Junction([string]$Path, [string]$Target) {
  & cmd /c mklink /J $Path $Target | Out-Null
  if (-not (Test-Path $Path)) { throw "junction failed: $Path -> $Target" }
}
New-Junction (Join-Path $nmDir '@deepseek-ai\dsh-base') (Join-Path $Dst 'vendor\dsh-base')
New-Junction (Join-Path $nmDir 'dsh-ops')               (Join-Path $Dst 'vendor\dsh-ops')

$patch = Join-Path $profDir 'cordis.patch.yml'
switch ($Scenario) {
  'clean' { }
  'f03' {   # 旧 patch 行残留，与 bundles 撞 id
    Add-Content -Path $patch -Value "- id: dsh-ops`n  name: dsh-ops`n  config: {}"
  }
  'f01' {   # 链接目标存在，但穿透后没有 package.json
    Remove-Item (Join-Path $nmDir '@deepseek-ai\dsh-base') -Force -Recurse
    New-Junction (Join-Path $nmDir '@deepseek-ai\dsh-base') (Join-Path $Dst 'vendor\dsh-base-empty')
  }
  'f02' {   # 链接还在，目标目录被删
    Remove-Item (Join-Path $Dst 'vendor\dsh-base') -Force -Recurse
  }
  'f16' {   # cordis.yml 写坏
    Set-Content -Path (Join-Path $profDir 'cordis.yml') -Value "[`n  broken: [`n"
  }
  'f17' {   # git 直连插件没进 allowBuilds
    $p = Join-Path $profDir 'package.json'
    $j = Get-Content $p -Raw | ConvertFrom-Json
    $j.dependencies | Add-Member -NotePropertyName 'dshmarket-git-test' -NotePropertyValue 'git+https://github.com/example/plugin.git' -Force
    $j | ConvertTo-Json -Depth 20 | Set-Content -Path $p -Encoding utf8
  }
  'f20' {   # 上次升级被打断，journal 停在 applying
    New-Item -ItemType Directory -Force -Path (Join-Path $dataDir 'journal') | Out-Null
    Set-Content -Path (Join-Path $dataDir 'journal\install-9.json') -Value '{"id":"install-9","state":"applying"}'
  }
  default { throw "unknown scenario: $Scenario" }
}

Write-Output "lab ready: $Dst"
