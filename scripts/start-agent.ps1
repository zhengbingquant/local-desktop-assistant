param(
    [string]$WorkspacePath = (Get-Location).Path,
    [string]$ModelPath,
    [ValidateRange(4096, 32768)]
    [int]$ContextLength = 8192,
    [ValidateRange(0, 24)]
    [int]$CPUThreads = 0,
    [ValidateRange(0, 24)]
    [int]$BatchThreads = 0,
    [ValidateRange(128, 4096)]
    [int]$GPUReserveMiB = 1024,
    [ValidateSet('f16', 'q8_0')]
    [string]$KVCacheType = 'f16',
    [ValidateSet('auto', 'none', 'ngram-mod', 'ngram-simple', 'draft-mtp')]
    [string]$Speculative = 'none',
    [ValidateSet('none', 'low', 'medium', 'high')]
    [string]$ReasoningEffort = 'medium',
    [ValidateRange(0, 8192)]
    [int]$ThinkingBudget = 512,
    [switch]$EnableWeb,
    [switch]$DisableWeb
)

$ErrorActionPreference = 'Stop'
if ($EnableWeb -and $DisableWeb) {
    throw 'Choose either -EnableWeb or -DisableWeb, not both.'
}
$projectRoot = Split-Path -Parent $PSScriptRoot
$entryPoint = Join-Path $projectRoot 'local_assistant\__main__.py'
$defaultModel = Join-Path $projectRoot 'models\Qwen3.8-27B-HF\Qwen3.8-27B-UD-Q4_K_XL.gguf'
$modelFile = if ($ModelPath) { [IO.Path]::GetFullPath($ModelPath) } else { $defaultModel }
$draftModel = Join-Path $projectRoot 'models\Qwen3.8-27B-HF\MTP\mtp-Qwen3.8-27B-Q4_0.gguf'
$workspace = [IO.Path]::GetFullPath($WorkspacePath)

if (-not (Test-Path -LiteralPath $entryPoint)) {
    throw "Assistant entry point is missing: $entryPoint"
}
if (-not (Test-Path -LiteralPath $workspace -PathType Container)) {
    throw "Workspace folder does not exist: $workspace"
}
if (-not (Test-Path -LiteralPath $modelFile -PathType Leaf)) {
    throw "Model file not found: $modelFile"
}
if ($Speculative -eq 'auto') {
    $effectiveSpeculative = if (Test-Path -LiteralPath $draftModel -PathType Leaf) { 'draft-mtp' } else { 'none' }
} else {
    $effectiveSpeculative = $Speculative
}
if ($effectiveSpeculative -eq 'draft-mtp' -and -not (Test-Path -LiteralPath $draftModel -PathType Leaf)) {
    throw 'The MTP draft model is missing. Run .\scripts\download-model.ps1 -MTP first.'
}

$pythonCommand = Get-Command 'python.exe' -All -ErrorAction SilentlyContinue |
    Where-Object { $_.Source -and $_.Source -notmatch '\\Microsoft\\WindowsApps\\' } |
    Select-Object -First 1
$pythonPath = if ($pythonCommand) { $pythonCommand.Source } else { $null }
if (-not $pythonPath) {
    foreach ($candidate in @(
        (Join-Path $env:LOCALAPPDATA 'Programs\Python\Python311\python.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Python\Python314\python.exe')
    )) {
        if (Test-Path -LiteralPath $candidate) {
            $pythonPath = $candidate
            break
        }
    }
}
if (-not $pythonPath) {
    throw 'Python 3.11 or newer was not found. Install Python from python.org and retry.'
}
$versionOutput = & $pythonPath --version 2>&1
if ($versionOutput -notmatch 'Python\s+(\d+)\.(\d+)') {
    throw "Could not identify the Python version at $pythonPath"
}
if ([version]::new([int]$Matches[1], [int]$Matches[2]) -lt [version]::new(3, 11)) {
    throw "Python 3.11 or newer is required; found $versionOutput"
}

try {
    $Host.UI.RawUI.WindowTitle = 'Local Assistant | Qwen3.8-27B'
} catch {
    # A host without a console title can still run the CLI.
}

$arguments = @(
    '-m', 'local_assistant',
    '--workspace', $workspace,
    '--model', $modelFile,
    '--context-length', "$ContextLength",
    '--gpu-reserve-mib', "$GPUReserveMiB",
    '--kv-cache-type', $KVCacheType,
    '--speculative', $effectiveSpeculative,
    '--reasoning-effort', $ReasoningEffort,
    '--thinking-budget', "$ThinkingBudget",
    '--web-mode', $(if ($DisableWeb) { 'off' } else { 'auto' })
)
if ($effectiveSpeculative -eq 'draft-mtp') {
    $arguments += @('--draft-model', $draftModel)
}
if ($CPUThreads -gt 0) {
    $arguments += @('--cpu-threads', "$CPUThreads")
}
if ($BatchThreads -gt 0) {
    $arguments += @('--batch-threads', "$BatchThreads")
}
Write-Host "Python: $pythonPath" -ForegroundColor DarkGray
Write-Host "Workspace: $workspace" -ForegroundColor DarkGray
Write-Host "Web research: $(if ($DisableWeb) { 'off' } else { 'auto' })" -ForegroundColor DarkGray
Push-Location -LiteralPath $projectRoot
try {
    & $pythonPath @arguments
    $exitCode = $LASTEXITCODE
} finally {
    Pop-Location
}
exit $exitCode
