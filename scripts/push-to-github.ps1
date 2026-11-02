# Pushes this repository to GitHub and prints the CI run URL.
#
# The first push needs two things that only the account owner can do:
#
#   1. an empty repository on GitHub (no README, no .gitignore, no licence),
#   2. a credential this machine can use. HTTPS to github.com is blocked on this
#      network (ssh and the API work, port 443 does not), so the push goes over
#      SSH: add the public key printed by
#
#         type C:\Users\X_Cyanogen\.ssh\id_rsa.pub
#
#      to https://github.com/settings/keys (or as a deploy key with write access
#      on the repository itself).
#
# Usage (from o824-rs):
#   pwsh -File scripts/push-to-github.ps1 -Repo git@github.com:<owner>/o824-rs.git
#
# Re-running is safe: the remote is replaced only when it differs, and the push is
# a normal non-forced push.

param(
  [Parameter(Mandatory = $true)][string]$Repo,
  [string]$Remote = "origin",
  [string]$Branch = "main"
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
  Write-Output "== checking the credential =="
  $who = (& ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 -T git@github.com 2>&1 | Out-String)
  if ($who -notmatch 'successfully authenticated') {
    Write-Output $who.Trim()
    throw "this machine's SSH key is not registered with GitHub; add $env:USERPROFILE\.ssh\id_rsa.pub at https://github.com/settings/keys"
  }
  Write-Output $who.Trim()

  Write-Output "== setting the remote =="
  $existing = (& git remote get-url $Remote 2>$null)
  if ($existing -ne $Repo) {
    if ($existing) { & git remote set-url $Remote $Repo } else { & git remote add $Remote $Repo }
  }
  & git remote -v

  Write-Output "== pushing $Branch and the tags =="
  & git push -u $Remote $Branch
  if ($LASTEXITCODE -ne 0) { throw "git push failed" }
  & git push $Remote --tags
  if ($LASTEXITCODE -ne 0) { throw "git push --tags failed" }

  $slug = ($Repo -replace '^git@github\.com:', '' -replace '\.git$', '')
  Write-Output ""
  Write-Output "CI: https://github.com/$slug/actions"
  Write-Output "Watch it with:"
  Write-Output "  start https://github.com/$slug/actions"
}
finally {
  Pop-Location
}
