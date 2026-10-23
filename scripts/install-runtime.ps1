param(
    # Release tag to install. b11748 is the build this project was tuned and tested with;
    # 'latest' picks the newest release that has a matching Windows build.
    [string]$Tag = 'b11748',
    [ValidateSet('cuda-13', 'cuda-12', 'vulkan', 'cpu')]
    [string]$Backend = 'cuda-13',
    # Replace an existing runtime. The old one is kept in runtime\llama.cpp.previous.
    [switch]$Force,
    # Show which files would be downloaded without downloading anything.
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'  # Invoke-WebRequest is much slower with a progress bar.
$repo = Split-Path -Parent $PSScriptRoot
$runtimeRoot = Join-Path $repo 'runtime'
$runtimeDir = Join-Path $runtimeRoot 'llama.cpp'
$server = Join-Path $runtimeDir 'llama-server.exe'
$headers = @{ 'User-Agent' = 'local-assistant-installer'; 'Accept' = 'application/vnd.github+json' }

function Get-RuntimeVersion {
    # llama-server prints its version on stderr, which 'Stop' would turn into an error.
    $ErrorActionPreference = 'Continue'
    ((& $server --version 2>&1) | ForEach-Object { "$_" } | Select-String 'version') -join ' '
}

if ((Test-Path -LiteralPath $server) -and -not $Force -and -not $DryRun) {
    Write-Host "llama.cpp is already installed: $(Get-RuntimeVersion)"
    Write-Host 'Use -Force to replace it, for example: .\scripts\install-runtime.ps1 -Tag latest -Force'
    exit 0
}

$patterns = @{
    'cuda-13' = @('llama-*-bin-win-cuda-13*-x64.zip', 'cudart-llama-bin-win-cuda-13*-x64.zip')
    'cuda-12' = @('llama-*-bin-win-cuda-12*-x64.zip', 'cudart-llama-bin-win-cuda-12*-x64.zip')
    'vulkan' = @('llama-*-bin-win-vulkan-x64.zip')
    'cpu' = @('llama-*-bin-win-cpu-x64.zip')
}[$Backend]

Write-Host "Looking up llama.cpp release '$Tag' on GitHub..."
if ($Tag -eq 'latest') {
    $releases = Invoke-RestMethod -Uri 'https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=20' -Headers $headers
    $release = $releases | Where-Object { $_.assets | Where-Object { $_.name -like $patterns[0] } } | Select-Object -First 1
    if (-not $release) {
        throw "No recent llama.cpp release has a $Backend Windows build."
    }
} else {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/ggml-org/llama.cpp/releases/tags/$Tag" -Headers $headers
}

$assets = foreach ($pattern in $patterns) {
    $asset = $release.assets | Where-Object { $_.name -like $pattern } | Select-Object -First 1
    if (-not $asset) {
        throw "Release $($release.tag_name) has no asset matching $pattern."
    }
    $asset
}
$totalMiB = [math]::Round((($assets | Measure-Object -Property size -Sum).Sum) / 1MB)
Write-Host "Release $($release.tag_name) ($Backend), $totalMiB MiB:"
foreach ($asset in $assets) {
    Write-Host "  $($asset.name)  $([math]::Round($asset.size / 1MB)) MiB  $($asset.digest)" -ForegroundColor DarkGray
}
if ($DryRun) {
    Write-Host 'Dry run: nothing was downloaded.'
    exit 0
}

$downloadDir = Join-Path ([IO.Path]::GetTempPath()) ("llama-cpp-" + [guid]::NewGuid().ToString('N'))
$staging = Join-Path $runtimeRoot 'llama.cpp.staging'
New-Item -ItemType Directory -Path $downloadDir -Force | Out-Null
try {
    if (Test-Path -LiteralPath $staging) {
        Remove-Item -LiteralPath $staging -Recurse -Force
    }
    New-Item -ItemType Directory -Path $staging -Force | Out-Null
    foreach ($asset in $assets) {
        $file = Join-Path $downloadDir $asset.name
        Write-Host "Downloading $($asset.name)..."
        Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $file -Headers @{ 'User-Agent' = 'local-assistant-installer' } -UseBasicParsing
        if ($asset.digest -match '^sha256:([0-9a-fA-F]{64})$') {
            $expected = $Matches[1].ToLowerInvariant()
            $actual = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($actual -ne $expected) {
                throw "Checksum mismatch for $($asset.name): expected $expected, got $actual"
            }
            Write-Host '  SHA-256 verified.' -ForegroundColor DarkGray
        } else {
            Write-Warning "GitHub published no checksum for $($asset.name); it was not verified."
        }
        Expand-Archive -LiteralPath $file -DestinationPath $staging -Force
    }

    # Some archives nest their files in a folder; use the folder that holds llama-server.exe.
    $found = Get-ChildItem -LiteralPath $staging -Filter 'llama-server.exe' -File -Recurse | Select-Object -First 1
    if (-not $found) {
        throw 'The downloaded archive does not contain llama-server.exe.'
    }
    if ($found.DirectoryName -ne (Resolve-Path -LiteralPath $staging).Path) {
        Get-ChildItem -LiteralPath $staging -File -Recurse | Move-Item -Destination $found.DirectoryName -Force -ErrorAction SilentlyContinue
        $staging = $found.DirectoryName
    }

    if (Test-Path -LiteralPath $runtimeDir) {
        $previous = Join-Path $runtimeRoot 'llama.cpp.previous'
        if (Test-Path -LiteralPath $previous) {
            Remove-Item -LiteralPath $previous -Recurse -Force
        }
        Move-Item -LiteralPath $runtimeDir -Destination $previous
        Write-Host "Previous runtime kept in $previous" -ForegroundColor DarkGray
    }
    Move-Item -LiteralPath $staging -Destination $runtimeDir
} finally {
    Remove-Item -LiteralPath $downloadDir -Recurse -Force -ErrorAction SilentlyContinue
    $leftover = Join-Path $runtimeRoot 'llama.cpp.staging'
    if (Test-Path -LiteralPath $leftover) {
        Remove-Item -LiteralPath $leftover -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "Installed llama.cpp to $runtimeDir" -ForegroundColor Green
Write-Host (Get-RuntimeVersion) -ForegroundColor DarkGray
