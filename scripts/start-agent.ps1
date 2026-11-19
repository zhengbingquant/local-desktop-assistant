# The native CLI accepts both --model and the original -Model launcher options.
& (Join-Path $PSScriptRoot 'invoke-luma.ps1') @args
exit $LASTEXITCODE
