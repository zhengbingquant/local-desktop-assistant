param(
    [ValidateSet('auto', '27b', '7b')]
    [string]$Model = 'auto',
    [ValidateRange(0, 262144)]
    [int]$ContextLength = 0
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$cli = Join-Path $repo 'runtime\llama.cpp\llama-completion.exe'
$models = @{
    '27b' = Join-Path $repo 'models\Qwen3.8-27B-HF\Qwen3.8-27B-UD-Q4_K_XL.gguf'
    '7b' = Join-Path $repo 'models\Qwen2.5-Coder-7B\qwen2.5-coder-7b-instruct-q4_k_m.gguf'
}
if ($Model -eq 'auto') {
    $Model = if (Test-Path -LiteralPath $models['27b']) { '27b' } else { '7b' }
}
$modelFile = $models[$Model]

if (-not (Test-Path -LiteralPath $cli)) {
    Write-Host 'The local inference runtime is missing.' -ForegroundColor Red
    Write-Host 'From this project folder, run: .\scripts\install-runtime.ps1'
    exit 1
}
if (-not (Test-Path -LiteralPath $modelFile)) {
    Write-Host "The $Model model is not downloaded yet." -ForegroundColor Yellow
    Write-Host $(if ($Model -eq '27b') { 'From this project folder, run: .\scripts\download-model.ps1' } else { 'From this project folder, run: .\scripts\download-coder-7b.ps1' })
    exit 1
}

# Same measured settings as the agent (see docs/model-setup.md).
if ($Model -eq '27b') {
    $title = 'Qwen3.8-27B'
    if ($ContextLength -eq 0) { $ContextLength = 16384 }
    $threads = [math]::Max(1, [math]::Floor([Environment]::ProcessorCount * 2 / 3))
    $modelArgs = @(
        '--threads', "$threads",
        '--batch-size', '512',
        '--ubatch-size', '512',
        '--reasoning', 'on',
        '--reasoning-effort', 'xhigh',
        '--temp', '1.0',
        '--top-p', '0.95',
        '--top-k', '20'
    )
} else {
    $title = 'Qwen 2.5 Coder 7B'
    if ($ContextLength -eq 0) { $ContextLength = 8192 }
    $modelArgs = @(
        '--batch-size', '256',
        '--ubatch-size', '128',
        '--reasoning', 'off',
        '--temp', '0.2',
        '--top-p', '0.9',
        '--top-k', '40'
    )
}

$systemPrompt = 'You are a helpful local desktop assistant. Be clear, practical, and concise by default. Use steps or examples when they make the answer easier to act on. Be honest when you are uncertain. You can only access files, applications, or the internet when tools are explicitly provided; never claim to have taken an action you could not perform.'
$cliArgs = @(
    '--model', $modelFile,
    '--conversation',
    '--interactive-first',
    '--system-prompt', $systemPrompt,
    '--ctx-size', "$ContextLength",
    '--fit', 'on',
    '--fit-target', '512',
    '--flash-attn', 'on',
    '--cache-type-k', 'q8_0',
    '--cache-type-v', 'q8_0',
    '--verbosity', '1',
    '--no-log-prefix',
    '--no-log-timestamps',
    '--log-colors', 'off',
    '--no-display-prompt',
    '--simple-io'
) + $modelArgs

Set-Location -LiteralPath $repo
$Host.UI.RawUI.WindowTitle = "Local Assistant | $title"
Clear-Host
Write-Host 'LOCAL DESKTOP ASSISTANT' -ForegroundColor Cyan
Write-Host "$title  |  Offline text chat  |  Runs on this computer" -ForegroundColor Gray
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
