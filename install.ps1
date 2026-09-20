# KHInsider App - Windows installer.
# Checks for what's needed (C++ Build Tools, WebView2, Rust), downloads and installs
# only what is missing, builds the optimized release exe and installs it for your user.
# Re-run it any time to update.
#
#   install.bat                 (double-click)  or
#   powershell -ExecutionPolicy Bypass -File install.ps1
#   ... -Uninstall              remove the app
param([switch]$Uninstall)

$ErrorActionPreference = 'Stop'
$ProgressPreference    = 'SilentlyContinue'   # much faster downloads
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Root       = $PSScriptRoot
$AppName    = 'khinsider-app'
$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\KHInsider'
$Exe        = Join-Path $InstallDir "$AppName.exe"
$Shortcut   = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\KHInsider.lnk'
$Arch       = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }

function Say($m) { Write-Host "==> $m" -ForegroundColor Cyan }

# Run a native command; PowerShell 5.1 treats stderr output as errors, so relax that and check the exit code instead.
function Invoke-Native([scriptblock]$Block) {
  $old = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
  try { & $Block } finally { $ErrorActionPreference = $old }
  if ($LASTEXITCODE -ne 0) { throw "Command failed with exit code $LASTEXITCODE" }
}
function Test-Native([scriptblock]$Block) {
  $old = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
  try { & $Block *> $null } catch {} finally { $ErrorActionPreference = $old }
  return ($LASTEXITCODE -eq 0)
}
function Get-Download($Url, $Name) {
  $path = Join-Path $env:TEMP $Name
  Invoke-WebRequest -Uri $Url -OutFile $path -UseBasicParsing
  return $path
}
# Only the machine-wide installs need admin; Windows shows one UAC prompt for them.
function Invoke-Elevated($File, $Arguments) {
  return (Start-Process -FilePath $File -ArgumentList $Arguments -Verb RunAs -Wait -PassThru).ExitCode
}

# ------------------------------------------------------------------ uninstall
if ($Uninstall) {
  Get-Process $AppName -ErrorAction SilentlyContinue | Stop-Process -Force
  Remove-Item -Recurse -Force $InstallDir -ErrorAction SilentlyContinue
  Remove-Item -Force $Shortcut -ErrorAction SilentlyContinue
  Say 'Removed KHInsider.'
  exit 0
}

if (-not (Test-Path (Join-Path $Root 'src-tauri\Cargo.toml'))) {
  throw 'Run this from the project folder (src-tauri\ not found next to install.ps1).'
}

# ------------------------------------------------------------------ 1. C++ Build Tools
function Test-Msvc {
  $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
  if (-not (Test-Path $vswhere)) { return $false }
  $found = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
  return [bool]$found
}
if (Test-Msvc) {
  Say 'C++ Build Tools: found.'
} else {
  Say 'C++ Build Tools: missing. Downloading and installing (several GB, this takes a while)...'
  $installer = Get-Download 'https://aka.ms/vs/17/release/vs_BuildTools.exe' 'vs_BuildTools.exe'
  $code = Invoke-Elevated $installer '--quiet --wait --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
  if ($code -notin 0, 1641, 3010) { throw "C++ Build Tools installer failed (exit code $code)." }
  if (-not (Test-Msvc)) { throw 'C++ Build Tools still not detected after install. Reboot and run this script again.' }
}

# ------------------------------------------------------------------ 2. WebView2 runtime
function Test-WebView2 {
  $id = '{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
  foreach ($root in 'HKLM:\SOFTWARE\WOW6432Node', 'HKLM:\SOFTWARE', 'HKCU:\SOFTWARE') {
    $v = (Get-ItemProperty -LiteralPath "$root\Microsoft\EdgeUpdate\Clients\$id" -Name pv -ErrorAction SilentlyContinue).pv
    if ($v -and $v -ne '0.0.0.0') { return $true }
  }
  return $false
}
if (Test-WebView2) {
  Say 'WebView2 runtime: found.'
} else {
  Say 'WebView2 runtime: missing. Downloading and installing...'
  $installer = Get-Download 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' 'MicrosoftEdgeWebview2Setup.exe'
  [void](Invoke-Elevated $installer '/silent /install')
  if (-not (Test-WebView2)) { throw 'WebView2 runtime still not detected after install.' }
}

# ------------------------------------------------------------------ 3. Rust (per-user, ~\.cargo)
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path $cargoBin) { $env:Path = "$cargoBin;$env:Path" }
if (Get-Command cargo -ErrorAction SilentlyContinue) {
  Say 'Rust: found.'
} else {
  Say 'Rust: missing. Downloading and installing...'
  $rustup = Get-Download "https://win.rustup.rs/$Arch" 'rustup-init.exe'
  Invoke-Native { & $rustup -y --profile minimal --default-toolchain stable --no-modify-path }
  $env:Path = "$cargoBin;$env:Path"
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'cargo not found after Rust install.' }

# Make sure a toolchain is active and it is the MSVC one
if (-not (Test-Native { rustup show active-toolchain })) { Invoke-Native { rustup default stable } }
$hostLine = (& rustc -vV | Select-String '^host:').ToString()
if ($hostLine -notmatch 'msvc') { Invoke-Native { rustup default "stable-$Arch-pc-windows-msvc" } }

# ------------------------------------------------------------------ 4. Build (optimized release)
Say 'Building optimized release (the first build takes several minutes)...'
$manifest = Join-Path $Root 'src-tauri\Cargo.toml'
Invoke-Native { cargo build --release --manifest-path $manifest --features tauri/custom-protocol }

$built = Join-Path $Root "src-tauri\target\release\$AppName.exe"
if (-not (Test-Path $built)) { throw "Build finished but $built was not found." }

# ------------------------------------------------------------------ 5. Install for this user
Say "Installing to $InstallDir"
Get-Process $AppName -ErrorAction SilentlyContinue | Stop-Process -Force
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item $built $Exe -Force

$lnk = (New-Object -ComObject WScript.Shell).CreateShortcut($Shortcut)
$lnk.TargetPath       = $Exe
$lnk.WorkingDirectory = $InstallDir
$lnk.IconLocation     = "$Exe,0"
$lnk.Description      = 'khinsider with Discord Rich Presence'
$lnk.Save()

Say 'Done. Start "KHInsider" from the Start menu.'
Say "Discord's desktop app must be running for the status to show."
