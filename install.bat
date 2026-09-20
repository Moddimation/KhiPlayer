@echo off
rem Double-click to install / update KHInsider. Pass /uninstall-style args through: install.bat -Uninstall
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
echo.
pause
