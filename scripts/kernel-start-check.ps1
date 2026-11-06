# Drives the kernel protocol directly, the way the window does, for the 2026-11-06 defect.
#
#     powershell -File scripts/kernel-start-check.ps1
#
# The window spawns `o824 kernel --stdio`, sends `o824.plan` for the desktop's "start" action,
# then `o824.apply`. When the desktop is already running this used to answer with
# "the app exited right after starting". This script asks the same question and prints the
# answer, so the fix can be checked on a machine where the desktop is open — which CI is not.
#
# It must run from a normal location: a process started from a low-integrity file (anything
# inside the build tree) cannot write where the kernel keeps its plans.

param(
    [string]$Kernel = "",
    [string]$Action = "start"
)

$ErrorActionPreference = "Stop"

if (-not $Kernel) {
    $Kernel = Join-Path $env:LOCALAPPDATA "o824\o824.exe"
}
if (-not (Test-Path $Kernel)) {
    Write-Error "no kernel at $Kernel - build and install first (o824 install)"
    exit 1
}

$startInfo = New-Object System.Diagnostics.ProcessStartInfo
$startInfo.FileName = $Kernel
$startInfo.Arguments = "kernel --stdio"
$startInfo.RedirectStandardInput = $true
$startInfo.RedirectStandardOutput = $true
$startInfo.RedirectStandardError = $true
$startInfo.UseShellExecute = $false
$startInfo.CreateNoWindow = $true

$kernelProcess = [System.Diagnostics.Process]::Start($startInfo)
$stdin = $kernelProcess.StandardInput
$stdout = $kernelProcess.StandardOutput
$reader = $stdout.BaseStream

function Send-Frame([string]$json) {
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
    $header = "Content-Length: $($bytes.Length)`r`n`r`n"
    $headerBytes = [System.Text.Encoding]::ASCII.GetBytes($header)
    $stdin.BaseStream.Write($headerBytes, 0, $headerBytes.Length)
    $stdin.BaseStream.Write($bytes, 0, $bytes.Length)
    $stdin.BaseStream.Flush()
}

function Read-Frame {
    # Header lines until a blank one, then exactly Content-Length bytes.
    $length = -1
    while ($true) {
        $line = ""
        while ($true) {
            $b = $reader.ReadByte()
            if ($b -lt 0) { return $null }
            if ($b -eq 13) { continue }
            if ($b -eq 10) { break }
            $line += [char]$b
        }
        if ($line -eq "") { break }
        if ($line -match "^Content-Length:\s*(\d+)$") { $length = [int]$matches[1] }
    }
    if ($length -lt 0) { throw "a frame arrived without Content-Length" }
    $buffer = New-Object byte[] $length
    $read = 0
    while ($read -lt $length) {
        $n = $reader.Read($buffer, $read, $length - $read)
        if ($n -le 0) { throw "the kernel closed mid-frame" }
        $read += $n
    }
    return [System.Text.Encoding]::UTF8.GetString($buffer)
}

function Invoke-Call([string]$method, [string]$paramsJson, [int]$id) {
    Send-Frame ("{`"jsonrpc`":`"2.0`",`"id`":$id,`"method`":`"$method`",`"params`":$paramsJson}")
    while ($true) {
        $frame = Read-Frame
        if (-not $frame) { throw "the kernel closed before answering $method" }
        $message = $frame | ConvertFrom-Json
        if ($message.PSObject.Properties.Name -contains "method") { continue }  # a job event
        if ($message.id -eq $id) { return $message }
    }
}

function Wait-JobResult([string]$jobId) {
    while ($true) {
        $frame = Read-Frame
        if (-not $frame) { throw "the kernel closed before finishing $jobId" }
        $message = $frame | ConvertFrom-Json
        if ($message.params -and $message.params.jobId -eq $jobId -and $message.method -eq "op.result") {
            return $message.params
        }
    }
}

try {
    "kernel: $Kernel"
    $info = Invoke-Call "info" "{}" 1
    "protocol: $($info.result.protocolVersion)  capabilities: $($info.result.capabilities -join ', ')"

    $discovered = Invoke-Call "o824.discover" "{}" 2
    $installs = $discovered.result.installs
    $desktop = $installs | Where-Object { $_.kind -eq "desktop" } | Select-Object -First 1
    if (-not $desktop) { "no desktop installation found; nothing to test"; exit 0 }
    "desktop install: $($desktop.installId)  version $($desktop.version)"

    $planReply = Invoke-Call "o824.plan" ("{`"installId`":`"$($desktop.installId)`",`"action`":`"$Action`"}") 3
    $result = Wait-JobResult $planReply.result.jobId
    # `op.result` carries the plan itself; a failure carries an `error` instead.
    if ($result.result.PSObject.Properties.Name -contains "error") {
        "plan failed: $($result.result | ConvertTo-Json -Compress -Depth 6)"
        exit 1
    }
    $plan = $result.result
    "plan: $($plan.id)  steps: $($plan.steps.kind -join ', ')"

    $applyReply = Invoke-Call "o824.apply" ("{`"planId`":`"$($plan.id)`"}") 4
    $applied = Wait-JobResult $applyReply.result.jobId
    $failed = $applied.result.PSObject.Properties.Name -contains "error"
    if ($failed) {
        "apply failed: $($applied.result | ConvertTo-Json -Compress -Depth 6)"
        exit 1
    }
    "the already-running desktop was accepted, not reported as a crash"
    exit 0
} finally {
    try { $stdin.Close() } catch { }
    try { if (-not $kernelProcess.HasExited) { $kernelProcess.Kill() } } catch { }
}
