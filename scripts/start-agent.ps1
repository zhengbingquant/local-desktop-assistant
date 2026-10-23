param(
    [string]$WorkspacePath = (Get-Location).Path,
    [string]$ModelPath,
    [ValidateSet('auto', '27b', '7b', 'qwen27b', 'coder')]
    [string]$Model = 'auto',
    [ValidateRange(0, 262144)]
    [int]$ContextLength = 0,
    [ValidateRange(0, 256)]
    [int]$CPUThreads = 0,
    [ValidateRange(0, 256)]
    [int]$BatchThreads = 0,
    [ValidateRange(128, 4096)]
    [int]$GPUReserveMiB,
    [ValidateSet('q8_0', 'f16')]
    [string]$KVCacheType,
    [ValidateSet('auto', 'none', 'draft-mtp', 'ngram-mod', 'ngram-simple')]
    [string]$Speculative,
    [string]$DraftModelPath,
    [ValidateSet('none', 'low', 'medium', 'high')]
    [string]$ReasoningEffort,
    [ValidateRange(0, 8192)]
    [int]$ThinkingBudget,
    [ValidateSet('show', 'hide')]
    [string]$ThinkingDisplay,
    [switch]$EnableWeb,
    [switch]$DisableWeb
)

# Defaults live in the Python CLI (python -m local_assistant --help); this launcher only
# finds Python and forwards the options that were given. 0 means "choose automatically".

$ErrorActionPreference = 'Stop'
if ($EnableWeb -and $DisableWeb) {
    throw 'Choose either -EnableWeb or -DisableWeb, not both.'
}

function Resolve-FullPath([string]$Path) {
    # Resolve relative to the PowerShell location, not the .NET process directory.
    $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Path)
}

$projectRoot = Split-Path -Parent $PSScriptRoot
$entryPoint = Join-Path $projectRoot 'local_assistant\__main__.py'
if (-not (Test-Path -LiteralPath $entryPoint)) {
    throw "Assistant entry point is missing: $entryPoint"
}
$workspace = Resolve-FullPath $WorkspacePath
if (-not (Test-Path -LiteralPath $workspace -PathType Container)) {
    throw "Workspace folder does not exist: $workspace"
}

$pythonCommand = Get-Command 'python.exe' -All -ErrorAction SilentlyContinue |
    Where-Object { $_.Source -and $_.Source -notmatch '\\Microsoft\\WindowsApps\\' } |
    Select-Object -First 1
$pythonPath = if ($pythonCommand) { $pythonCommand.Source } else { $null }
if (-not $pythonPath) {
    $installs = Join-Path $env:LOCALAPPDATA 'Programs\Python'
    $pythonPath = Get-ChildItem -LiteralPath $installs -Directory -Filter 'Python3*' -ErrorAction SilentlyContinue |
        Sort-Object { [int]($_.Name -replace '\D', '') } -Descending |
        ForEach-Object { Join-Path $_.FullName 'python.exe' } |
        Where-Object { Test-Path -LiteralPath $_ } |
        Select-Object -First 1
}
if (-not $pythonPath) {
    $launcher = Get-Command 'py.exe' -ErrorAction SilentlyContinue
    if ($launcher) {
        $pythonPath = & $launcher.Source -3 -c 'import sys; print(sys.executable)' 2>$null
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
    $Host.UI.RawUI.WindowTitle = 'Local Assistant'
} catch {
    # A host without a console title can still run the CLI.
}

$modelArgument = if ($ModelPath) {
    Resolve-FullPath $ModelPath
} else {
    @{ 'auto' = 'auto'; '27b' = '27b'; 'qwen27b' = '27b'; '7b' = '7b'; 'coder' = '7b' }[$Model]
}
$webMode = if ($DisableWeb) { 'off' } else { 'auto' }
$arguments = @(
    '-m', 'local_assistant',
    '--workspace', $workspace,
    '--model', $modelArgument,
    '--web-mode', $webMode
)
if ($ContextLength -gt 0) { $arguments += @('--context-length', "$ContextLength") }
if ($CPUThreads -gt 0) { $arguments += @('--cpu-threads', "$CPUThreads") }
if ($BatchThreads -gt 0) { $arguments += @('--batch-threads', "$BatchThreads") }
if ($PSBoundParameters.ContainsKey('GPUReserveMiB')) { $arguments += @('--gpu-reserve-mib', "$GPUReserveMiB") }
if ($KVCacheType) { $arguments += @('--kv-cache-type', $KVCacheType) }
if ($Speculative) { $arguments += @('--speculative', $Speculative) }
if ($DraftModelPath) { $arguments += @('--draft-model', (Resolve-FullPath $DraftModelPath)) }
if ($ReasoningEffort) { $arguments += @('--reasoning-effort', $ReasoningEffort) }
if ($PSBoundParameters.ContainsKey('ThinkingBudget')) { $arguments += @('--thinking-budget', "$ThinkingBudget") }
if ($ThinkingDisplay) { $arguments += @('--thinking-display', $ThinkingDisplay) }

Write-Host "Python: $pythonPath" -ForegroundColor DarkGray
Write-Host "Workspace: $workspace" -ForegroundColor DarkGray
Write-Host "Web research: $webMode" -ForegroundColor DarkGray
Push-Location -LiteralPath $projectRoot
try {
    & $pythonPath @arguments
    $exitCode = $LASTEXITCODE
} finally {
    Pop-Location
}
exit $exitCode
