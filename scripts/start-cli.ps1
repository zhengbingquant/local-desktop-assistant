param(
    [int]$ContextLength = 8192
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$cli = Join-Path $repo 'runtime\llama.cpp\llama-completion.exe'
$model = Join-Path $repo 'models\Qwen3.8-27B-HF\Qwen3.8-27B-UD-Q4_K_XL.gguf'

if (-not (Test-Path -LiteralPath $cli)) {
    Write-Host 'The local inference runtime is missing.' -ForegroundColor Red
    Write-Host "Expected: $cli"
    exit 1
}
if (-not (Test-Path -LiteralPath $model)) {
    Write-Host 'The Qwen3.8 model is not downloaded yet.' -ForegroundColor Yellow
    Write-Host 'From this project folder, run: .\scripts\download-model.ps1'
    exit 1
}

$systemPrompt = 'You are a helpful local desktop assistant. Be clear, practical, and concise by default. Use steps or examples when they make the answer easier to act on. Be honest when you are uncertain. You can only access files, applications, or the internet when tools are explicitly provided; never claim to have taken an action you could not perform.'
$cliArgs = @(
    '--model', $model,
    '--conversation',
    '--interactive-first',
    '--system-prompt', $systemPrompt,
    '--ctx-size', "$ContextLength",
    '--fit', 'on',
    '--fit-target', '1024',
    '--flash-attn', 'on',
    '--batch-size', '256',
    '--ubatch-size', '128',
    '--reasoning', 'on',
    '--reasoning-effort', 'xhigh',
    '--temp', '1.0',
    '--top-p', '0.95',
    '--top-k', '20',
    '--verbosity', '1',
    '--no-log-prefix',
    '--no-log-timestamps',
    '--log-colors', 'off',
    '--no-display-prompt',
    '--simple-io'
)

Set-Location -LiteralPath $repo
$Host.UI.RawUI.WindowTitle = 'Local Assistant | Qwen3.8-27B'
Clear-Host
Write-Host 'LOCAL DESKTOP ASSISTANT' -ForegroundColor Cyan
Write-Host 'Qwen3.8-27B  |  Offline text chat  |  Runs on this laptop' -ForegroundColor Gray
Write-Host ('-' * 68) -ForegroundColor DarkGray
Write-Host 'Type a message and press Enter.' -ForegroundColor White
Write-Host 'Ctrl+C interrupts generation. Close this window to exit.' -ForegroundColor DarkGray
Write-Host "Context: $([math]::Round($ContextLength / 1024, 1))K  |  Loading the model may take a little while." -ForegroundColor DarkGray
Write-Host ''
& $cli @cliArgs
$exitCode = $LASTEXITCODE
if ($exitCode -ne 0) {
    Write-Host "The model process ended with exit code $exitCode." -ForegroundColor Red
}
exit $exitCode
