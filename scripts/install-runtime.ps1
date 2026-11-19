param(
    [string]$Tag = 'b11748',
    [ValidateSet('cuda-13','cuda-12','vulkan','cpu')][string]$Backend = 'cuda-13',
    [switch]$Force, [switch]$DryRun
)
$options = @('install-runtime', '--tag', $Tag, '--backend', $Backend)
if ($Force) { $options += '--force' }
if ($DryRun) { $options += '--dry-run' }
& (Join-Path $PSScriptRoot 'invoke-luma.ps1') @options
exit $LASTEXITCODE
