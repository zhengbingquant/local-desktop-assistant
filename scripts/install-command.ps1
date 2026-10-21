$ErrorActionPreference = 'Stop'

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$launcher = Join-Path $projectRoot 'scripts\start-agent.ps1'
if (-not (Test-Path -LiteralPath $launcher -PathType Leaf)) {
    throw "Assistant launcher not found: $launcher"
}

$installDirectory = Join-Path $env:LOCALAPPDATA 'Programs\LocalAssistant\bin'
New-Item -ItemType Directory -Path $installDirectory -Force | Out-Null

$shimTemplate = @'
@echo off
setlocal
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "__LAUNCHER__" %*
exit /b %ERRORLEVEL%
'@
$shim = $shimTemplate.Replace('__LAUNCHER__', $launcher)

foreach ($commandName in @('luma', 'local-assistant')) {
    $commandPath = Join-Path $installDirectory "$commandName.cmd"
    Set-Content -LiteralPath $commandPath -Value $shim -Encoding ASCII
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$pathEntries = @($userPath -split ';' | Where-Object { $_.Trim() })
$alreadyInstalled = $pathEntries | Where-Object {
    [string]::Equals($_.TrimEnd('\'), $installDirectory.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)
}
if (-not $alreadyInstalled) {
    $newUserPath = (@($pathEntries) + $installDirectory) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
}

Write-Host "Installed terminal commands: luma, local-assistant" -ForegroundColor Green
Write-Host "Launcher: $launcher" -ForegroundColor DarkGray
Write-Host 'Workspace defaults to the folder where you run the command.' -ForegroundColor DarkGray
Write-Host 'Close and reopen your terminal, then enter: luma' -ForegroundColor Cyan
Write-Host 'Use: luma -DisableWeb   to force an offline session.' -ForegroundColor DarkGray
