# 验证 F17 的 --with-install 全链路（在普通 PowerShell 里跑）。
#
# 造一个带 postinstall 的本地 git 包 + 一个只依赖它的临时 DSH profile，
# 然后 doctor -> repair --with-install -> 检查 postinstall 是否真的执行。
# 全程只在 $Lab 下操作，DSH_HOME 指向临时目录，绝不碰你的 ~/.dsh。
#
# 用法：powershell -ExecutionPolicy Bypass -File verify-f17.ps1

$ErrorActionPreference = 'Stop'

$Lab  = 'D:\dsh work\broken-dsh-lab\f17verify'
$Exe  = 'D:\dsh work\Object 824\o824.exe'
$App  = 'D:\Users\X_Cyanogen\AppData\Local\Programs\DeepSeek Harness'
$Pkg  = Join-Path $Lab 'pkg'
$HomeDir = Join-Path $Lab 'home'
$Prof = Join-Path $HomeDir 'profiles\f17verify'
$Store = Join-Path $Lab 'store'

if (-not (Test-Path $Exe)) { throw "找不到 o824.exe: $Exe" }
if (-not (Test-Path $App)) { throw "找不到 DSH 安装目录: $App" }

if (Test-Path $Lab) { Remove-Item $Lab -Recurse -Force }
New-Item -ItemType Directory -Force -Path $Pkg, $Prof | Out-Null

# 1) 本地 git 包：postinstall 写一个标记文件
$pkgJson = @'
{
  "name": "f17-verify-probe",
  "version": "1.0.0",
  "scripts": { "postinstall": "node -e \"require('fs').writeFileSync('built.txt','built')\"" }
}
'@
[IO.File]::WriteAllText((Join-Path $Pkg 'package.json'), $pkgJson, (New-Object System.Text.UTF8Encoding($false)))
Push-Location $Pkg
git init -q .
git add -A
git -c user.email=o824@local -c user.name=o824 commit -qm init
Pop-Location

# 2) 临时 profile：只依赖这个 git 包，故意不带 pnpm-lock.yaml（新 profile 的真实形态）
$url = 'git+file:///' + $Pkg.Replace([char]92, [char]47)
$profJson = @"
{
  "name": "dsh-profile-f17verify",
  "private": true,
  "dependencies": { "f17-verify-probe": "$url" },
  "dsh": { "profile": { "bundles": [] } }
}
"@
[IO.File]::WriteAllText((Join-Path $Prof 'package.json'), $profJson, (New-Object System.Text.UTF8Encoding($false)))
Set-Content -Path (Join-Path $Prof 'cordis.yml') -Value '[]'
Set-Content -Path (Join-Path $Prof 'cordis.patch.yml') -Value '[]'
$storeYaml = $Store.Replace([char]92, [char]47)
Set-Content -Path (Join-Path $Prof 'pnpm-workspace.yaml') -Value ("packages:`n  - .`n`nnodeLinker: hoisted`nautoInstallPeers: false`nstoreDir: " + $storeYaml)

$env:DSH_HOME = $HomeDir
$env:O824_DATA_ROOT = Join-Path $Lab 'data'

Write-Host ''
Write-Host '=== 1) doctor（应当报 F17）==='
& $Exe doctor --root $App --home $HomeDir --profile f17verify
$doctor1 = $LASTEXITCODE
Write-Host "doctor exit = $doctor1"

Write-Host ''
Write-Host '=== 2) repair --with-install（副本内 pnpm install + 交换）==='
& $Exe repair --root $App --home $HomeDir --profile f17verify --yes --with-install
$repair = $LASTEXITCODE
Write-Host "repair exit = $repair"

Write-Host ''
Write-Host '=== 3) 结果 ==='
$built = Join-Path $Prof 'node_modules\f17-verify-probe\built.txt'
$ws = Get-Content (Join-Path $Prof 'pnpm-workspace.yaml') -Raw
$keyOk = $ws -match 'allowBuilds' -and $ws -match '%20'
Write-Host ("allowBuilds 已写入（含 %20 编码）: " + $keyOk)
Write-Host ("postinstall 已执行            : " + (Test-Path $built))

$env:DSH_HOME = $HomeDir
& $Exe doctor --root $App --home $HomeDir --profile f17verify | Out-Null
$doctor2 = $LASTEXITCODE
Write-Host ("doctor exit（应为 0）         : " + $doctor2)

$ok = ($doctor1 -eq 1) -and ($repair -eq 0) -and (Test-Path $built) -and $keyOk -and ($doctor2 -eq 0)
Write-Host ''
if ($ok) {
  Write-Host 'PASS - F17 全链路成立：诊断 -> 写仓库键 -> 副本内装包 -> postinstall 执行 -> 复检干净' -ForegroundColor Green
} else {
  Write-Host 'FAIL - 把上面的完整输出发给我' -ForegroundColor Red
}
Write-Host ''
Write-Host "临时目录：$Lab（可整个删掉）"
