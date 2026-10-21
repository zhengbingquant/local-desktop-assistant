@echo off
setlocal
set "POWERSHELL=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"
set "LAUNCHER=%~dp0scripts\start-agent.ps1"
if "%~1"=="" (
    start "Local Assistant" /D "%~dp0." "%POWERSHELL%" -NoExit -NoProfile -ExecutionPolicy Bypass -File "%LAUNCHER%"
) else (
    start "Local Assistant" /D "%~dp0." "%POWERSHELL%" -NoExit -NoProfile -ExecutionPolicy Bypass -File "%LAUNCHER%" -WorkspacePath "%~1\."
)
