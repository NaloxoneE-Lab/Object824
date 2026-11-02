# Pushes this repository to GitHub and prints the CI run URL.
#
# The first push needs two things only the account owner can do:
#
#   1. an empty repository on GitHub (no README, no .gitignore, no licence),
#   2. a credential this machine can use. HTTPS to github.com is blocked on this
#      network (ssh and the API work, port 443 does not), so the push goes over
#      SSH: add the public key printed by
#
#         type "$Key"
#
#      to https://github.com/settings/keys, or as a write-enabled deploy key on
#      the repository itself.
#
# This machine has one trap worth knowing about: HOME is redirected to
# %APPDATA%\SPB_Data, so ssh looks for identities under
# %APPDATA%\SPB_Data\.ssh and never finds the real key in %USERPROFILE%\.ssh. The
# script therefore pins core.sshCommand to the key by absolute path; without it
# GitHub answers "Permission denied (publickey)" for a key that is registered.
#
# Usage (from o824-rs):
#   pwsh -File scripts/push-to-github.ps1 -Repo git@github.com:<owner>/<repo>.git
#
# Re-running is safe: the remote is replaced only when it differs, and the push is
# a normal non-forced push.

param(
  [Parameter(Mandatory = $true)][string]$Repo,
  [string]$Remote = "origin",
  [string]$Branch = "main",
  [string]$Key = "$env:USERPROFILE\.ssh\id_rsa"
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
  if (-not (Test-Path $Key)) { throw "missing private key $Key" }

  Write-Output "== pinning the identity (HOME is redirected on this machine) =="
  $sshCommand = "ssh -i `"$Key`" -o IdentitiesOnly=yes"
  & git config core.sshCommand $sshCommand
  Write-Output $sshCommand

  Write-Output "== checking the credential =="
  $who = (& ssh -i $Key -o BatchMode=yes -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 -T git@github.com 2>&1 | Out-String)
  if ($who -notmatch 'successfully authenticated') {
    Write-Output $who.Trim()
    throw "this machine's SSH key is not accepted by GitHub; add $Key.pub at https://github.com/settings/keys"
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
}
finally {
  Pop-Location
}
