param([switch]$Vision, [switch]$MTP)
$options = @('download', '27b')
if ($Vision) { $options += '--vision' }
if ($MTP) { $options += '--mtp' }
& (Join-Path $PSScriptRoot 'invoke-luma.ps1') @options
exit $LASTEXITCODE
