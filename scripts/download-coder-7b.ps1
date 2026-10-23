param()

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$modelDir = Join-Path $repo 'models\Qwen2.5-Coder-7B'
$aria = (Get-Command 'aria2c' -ErrorAction SilentlyContinue).Source

if (-not $aria) {
    $wingetPackages = Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Packages'
    $ariaPackage = Get-ChildItem -LiteralPath $wingetPackages -Directory -Filter 'aria2.aria2_*' -Force -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($ariaPackage) {
        $aria = Get-ChildItem -LiteralPath $ariaPackage.FullName -Filter 'aria2c.exe' -File -Force -Recurse -ErrorAction SilentlyContinue |
            Select-Object -First 1 -ExpandProperty FullName
    }
}
if (-not $aria -or -not (Test-Path -LiteralPath $aria)) {
    throw 'aria2c is required. Install it with: winget install --id aria2.aria2 --exact'
}

New-Item -ItemType Directory -Path $modelDir -Force | Out-Null

$file = @{
    Name = 'qwen2.5-coder-7b-instruct-q4_k_m.gguf'
    Url = 'https://huggingface.co/Qwen/Qwen2.5-Coder-7B-Instruct-GGUF/resolve/main/qwen2.5-coder-7b-instruct-q4_k_m.gguf?download=true'
    Bytes = 4683073536
    Sha256 = '509287f78cb4d4cf6b3843734733b914b2c158e43e22a7f4bf5e963800894d3c'
}

$destination = Join-Path $modelDir $file.Name
if (Test-Path -LiteralPath $destination) {
    $existing = Get-Item -LiteralPath $destination
    if ($existing.Length -eq $file.Bytes) {
        $existingHash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($existingHash -eq $file.Sha256) {
            Write-Host "$($file.Name) is already present and verified."
            exit 0
        }
    }
}

Write-Host "Downloading $($file.Name) with resumable parallel HTTP ranges..."
$ariaArgs = @(
    '--continue=true',
    "--checksum=sha-256=$($file.Sha256)",
    '--max-connection-per-server=16',
    '--split=16',
    '--min-split-size=1M',
    '--file-allocation=none',
    '--connect-timeout=30',
    '--timeout=60',
    '--max-tries=10',
    '--retry-wait=5',
    '--summary-interval=10',
    '--console-log-level=warn',
    '--dir', $modelDir,
    '--out', $file.Name,
    $file.Url
)
& $aria @ariaArgs
if ($LASTEXITCODE -ne 0) {
    throw "aria2c failed to download $($file.Name) with exit code $LASTEXITCODE"
}

Write-Host "Qwen 2.5 Coder 7B is ready in $modelDir"
