# KHInsider App - Windows installer.
# Checks for what's needed (C++ Build Tools, WebView2, Rust), downloads and installs
# only what is missing, builds the optimized release exe and installs it for your user.
# Re-run it any time to update.
#
#   install.bat                 (double-click)  or
#   powershell -ExecutionPolicy Bypass -File install.ps1
#   ... -Uninstall              remove the desktop app
#   ... -Android                build a signed Android APK instead (sets up JDK/SDK/NDK per-user),
#                               installs it if a phone is plugged in (adb)
#   ... -Android -AllAbis       also build 32-bit ARM and x86 (bigger APK, slower)
param([switch]$Uninstall, [switch]$Android, [switch]$AllAbis)

$ErrorActionPreference = 'Stop'
$ProgressPreference    = 'SilentlyContinue'   # much faster downloads
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Root       = $PSScriptRoot
$AppName    = 'khinsider-app'
$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\KHInsider'
$Exe        = Join-Path $InstallDir "$AppName.exe"
$Shortcut   = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\KHInsider.lnk'
$Tools      = Join-Path $env:LOCALAPPDATA 'KHInsider\build'   # user-local JDK + signing key (Android)
$Abis       = if ($AllAbis) { @('aarch64','armv7','i686','x86_64') } else { @('aarch64') }
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
if ($Android) {
  # not needed for Android builds
} elseif (Test-WebView2) {
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

# ------------------------------------------------------------------ Android
if ($Android) {
  if ($Arch -ne 'x86_64') { throw 'Android builds need an x86_64 Windows machine.' }
  New-Item -ItemType Directory -Force -Path $Tools | Out-Null

  # Tauri links native libraries into the Android project with symlinks: Windows needs Developer Mode for that.
  $dev = (Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock' -Name AllowDevelopmentWithoutDevLicense -ErrorAction SilentlyContinue).AllowDevelopmentWithoutDevLicense
  if ($dev -ne 1) {
    Say 'Enabling Windows Developer Mode (needed for symlinks, one admin prompt)...'
    [void](Invoke-Elevated 'reg.exe' 'add HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock /t REG_DWORD /f /v AllowDevelopmentWithoutDevLicense /d 1')
  }

  # --- JDK 17 (user-local Temurin unless a JDK >= 17 is already set up)
  function Test-Jdk($dir) {
    $ErrorActionPreference = 'Continue'
    $javac = Join-Path $dir 'bin\javac.exe'
    if (-not $dir -or -not (Test-Path $javac)) { return $false }
    $out = (& $javac -version 2>&1 | Out-String)
    return ($out -match 'javac (\d+)') -and ([int]$Matches[1] -ge 17)
  }
  $jdk = Join-Path $Tools 'jdk'
  if ($env:JAVA_HOME -and (Test-Jdk $env:JAVA_HOME)) {
    Say "JDK: using $env:JAVA_HOME"
  } elseif (Test-Jdk $jdk) {
    $env:JAVA_HOME = $jdk; Say "JDK: found $jdk"
  } else {
    Say "JDK: downloading Temurin 17 to $jdk"
    $zip = Get-Download 'https://api.adoptium.net/v3/binary/latest/17/ga/windows/x64/jdk/hotspot/normal/eclipse' 'jdk17.zip'
    $tmp = Join-Path $Tools 'jdk_tmp'
    Remove-Item -Recurse -Force $tmp, $jdk -ErrorAction SilentlyContinue
    Expand-Archive -Path $zip -DestinationPath $tmp -Force
    Move-Item (Get-ChildItem $tmp -Directory | Select-Object -First 1).FullName $jdk
    Remove-Item -Recurse -Force $tmp
    $env:JAVA_HOME = $jdk
  }
  $env:Path = "$env:JAVA_HOME\bin;$env:Path"

  # --- Android SDK (command-line tools, platform-tools, build-tools, NDK), user-local
  if (-not $env:ANDROID_HOME) { $env:ANDROID_HOME = Join-Path $env:LOCALAPPDATA 'Android\Sdk' }
  $env:ANDROID_SDK_ROOT = $env:ANDROID_HOME
  $sdkm = Join-Path $env:ANDROID_HOME 'cmdline-tools\latest\bin\sdkmanager.bat'
  if (-not (Test-Path $sdkm)) {
    Say "Android SDK: downloading command-line tools to $env:ANDROID_HOME"
    $zip = Get-Download 'https://dl.google.com/android/repository/commandlinetools-win-11076708_latest.zip' 'cmdtools.zip'
    $ct  = Join-Path $env:ANDROID_HOME 'cmdline-tools'
    New-Item -ItemType Directory -Force -Path $ct | Out-Null
    Remove-Item -Recurse -Force (Join-Path $ct '_tmp'), (Join-Path $ct 'latest') -ErrorAction SilentlyContinue
    Expand-Archive -Path $zip -DestinationPath (Join-Path $ct '_tmp') -Force
    Move-Item (Join-Path $ct '_tmp\cmdline-tools') (Join-Path $ct 'latest')
    Remove-Item -Recurse -Force (Join-Path $ct '_tmp')
  }
  $ndkVer = '27.0.12077973'; $btVer = '34.0.0'
  $want = @()
  if (-not (Test-Path (Join-Path $env:ANDROID_HOME 'platform-tools')))            { $want += 'platform-tools' }
  if (-not (Test-Path (Join-Path $env:ANDROID_HOME "build-tools\$btVer")))        { $want += "build-tools;$btVer" }
  if (-not (Test-Path (Join-Path $env:ANDROID_HOME 'platforms\android-34')))      { $want += 'platforms;android-34' }
  if (-not (Test-Path (Join-Path $env:ANDROID_HOME "ndk\$ndkVer")))               { $want += "ndk;$ndkVer" }
  if ($want.Count -gt 0) {
    Say "Android SDK: installing $($want -join ', ') (accepting licenses)..."
    [void](Test-Native { ("y`n" * 100) | & $sdkm --licenses })
    Invoke-Native { & $sdkm --install @want }
  } else { Say 'Android SDK: all components present.' }
  if (-not $env:NDK_HOME) { $env:NDK_HOME = Join-Path $env:ANDROID_HOME "ndk\$ndkVer" }

  # --- Rust Android targets + Tauri CLI
  $triples = @{ aarch64 = 'aarch64-linux-android'; armv7 = 'armv7-linux-androideabi'; i686 = 'i686-linux-android'; x86_64 = 'x86_64-linux-android' }
  $targets = $Abis | ForEach-Object { $triples[$_] }
  Say "Rust targets: $($targets -join ', ')"
  Invoke-Native { rustup target add @targets }
  if (-not (Test-Native { cargo tauri --version })) {
    Say 'Installing the Tauri CLI (compiles from source, a few minutes)...'
    Invoke-Native { cargo install tauri-cli --version "^2" --locked }
  }

  Push-Location $Root
  try {
    if (-not (Test-Path 'src-tauri\gen\android')) {
      Say 'Generating the Android project...'
      Invoke-Native { cargo tauri android init --ci }
    }
    Say "Building optimized release APK ($($Abis -join ', ')); the first build takes several minutes..."
    $targetArgs = foreach ($a in $Abis) { '--target'; $a }
    Invoke-Native { cargo tauri android build --apk @targetArgs --ci }

    $apk = Get-ChildItem 'src-tauri\gen\android\app\build\outputs' -Recurse -Filter *.apk |
           Where-Object { $_.FullName -match 'release' } | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $apk) { throw 'Build finished but no release APK was found.' }

    # Sign: an unsigned release APK will not install. Personal key, created once.
    New-Item -ItemType Directory -Force -Path 'dist' | Out-Null
    $out = Join-Path $Root 'dist\KHInsider-android.apk'
    if ($apk.Name -match 'unsigned') {
      $ks = Join-Path $Tools 'release.keystore'
      if (-not (Test-Path $ks)) {
        Say "Creating a personal signing key at $ks (keep it: updates must use the same key)"
        Invoke-Native { keytool -genkeypair -keystore $ks -alias khinsider -keyalg RSA -keysize 2048 -validity 10000 -storepass khinsider -keypass khinsider -dname 'CN=KHInsider' }
      }
      $bt = Join-Path $env:ANDROID_HOME "build-tools\$btVer"
      $aligned = Join-Path $Tools 'aligned.apk'
      Remove-Item $aligned, $out -Force -ErrorAction SilentlyContinue
      Invoke-Native { & (Join-Path $bt 'zipalign.exe') -f -p 4 $apk.FullName $aligned }
      Invoke-Native { & (Join-Path $bt 'apksigner.bat') sign --ks $ks --ks-pass pass:khinsider --key-pass pass:khinsider --out $out $aligned }
      Remove-Item $aligned, "$out.idsig" -Force -ErrorAction SilentlyContinue
    } else { Copy-Item $apk.FullName $out -Force }
    Say "APK ready: $out"

    $adb = Join-Path $env:ANDROID_HOME 'platform-tools\adb.exe'
    $devices = $null
    if (Test-Path $adb) {
      $old = $ErrorActionPreference; $ErrorActionPreference = 'Continue'   # adb prints "daemon started" on stderr
      $devices = (& $adb devices 2>&1 | Out-String) -split "`n" | Select-String "`tdevice"
      $ErrorActionPreference = $old
    }
    if ($devices) {
      Say 'Phone detected, installing over adb...'
      Invoke-Native { & $adb install -r $out }
      Say 'Installed. Open KHInsider on your phone.'
    } else {
      Say "No phone connected over adb. Copy the APK to your phone and open it (allow 'install unknown apps' when asked),"
      Say 'or enable USB debugging, plug it in, and re-run this script to install automatically.'
    }
    Say 'Note: Discord Rich Presence is desktop-only; the Android app plays music but shows no Discord status.'
  } finally { Pop-Location }
  exit 0
}

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
