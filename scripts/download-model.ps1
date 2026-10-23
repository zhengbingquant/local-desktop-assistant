param(
    [switch]$Vision,
    # Separate MTP draft file. Not needed with current llama.cpp builds: the main GGUF
    # already contains the MTP head, which the assistant uses automatically.
    [switch]$MTP
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$modelDir = Join-Path $repo 'models\Qwen3.8-27B-HF'
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

$files = @(
    @{
        Name = 'Qwen3.8-27B-UD-Q4_K_XL.gguf'
        Url = 'https://huggingface.co/unsloth/Qwen3.8-27B-GGUF/resolve/main/Qwen3.8-27B-UD-Q4_K_XL.gguf?download=true'
        Bytes = 17559178144
        Sha256 = '3f227079003add2511437e5b1e94812e363385225bf6a9b47b0054a72bc8b01e'
    }
)
if ($Vision) {
    $files += @{
        Name = 'mmproj-F16.gguf'
        Url = 'https://huggingface.co/unsloth/Qwen3.8-27B-GGUF/resolve/main/mmproj-F16.gguf?download=true'
        Bytes = 927607488
        Sha256 = 'cbb841a9ee0636b2ec172f5bb8df2ea8dfeb01e90fe7c6126581d662a0b4e43e'
    }
}
if ($MTP) {
    $files += @{
        Name = 'MTP\mtp-Qwen3.8-27B-Q4_0.gguf'
        Url = 'https://huggingface.co/unsloth/Qwen3.8-27B-GGUF/resolve/main/MTP/mtp-Qwen3.8-27B-Q4_0.gguf?download=true'
        Bytes = 1369590656
        Sha256 = '50d9ce5a6da381bbcfb31061cf73df94a90e6faf8efeddee379a9cb8f1501c6e'
    }
}

foreach ($file in $files) {
    $destination = Join-Path $modelDir $file.Name
    $destinationDirectory = Split-Path -Parent $destination
    New-Item -ItemType Directory -Path $destinationDirectory -Force | Out-Null
    if (Test-Path -LiteralPath $destination) {
        $existing = Get-Item -LiteralPath $destination
        if ($existing.Length -eq $file.Bytes) {
            $existingHash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($existingHash -eq $file.Sha256) {
                Write-Host "$($file.Name) is already present and verified."
                continue
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
        '--summary-interval=15',
        '--console-log-level=warn',
        '--dir', $modelDir,
        '--out', $file.Name,
        $file.Url
    )
    & $aria @ariaArgs
    if ($LASTEXITCODE -ne 0) {
        throw "aria2c failed to download $($file.Name) with exit code $LASTEXITCODE"
    }
}

Write-Host "Model files are in $modelDir"
