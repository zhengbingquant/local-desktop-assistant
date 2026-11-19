$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$binary = Join-Path $projectRoot 'target\release\luma.exe'
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw "Build Luma from '$projectRoot' with: cargo build --release --locked"
}
& $binary --home $projectRoot @args
exit $LASTEXITCODE
