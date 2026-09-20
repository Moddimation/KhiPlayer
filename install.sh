#!/usr/bin/env bash
# KHInsider App - Linux installer.
# Detects your distro, installs only the packages that are missing, builds the
# optimized release and installs it for your user. Re-run any time to update.
#
#   ./install.sh                desktop app: build + install to ~/.local
#   ./install.sh --android      Android app: sets up JDK/SDK/NDK (user-local), builds a
#                               signed release APK, installs it if a phone is plugged in (adb)
#   ./install.sh --android --all-abis   also build 32-bit ARM and x86 (bigger APK, slower)
#   ./install.sh --uninstall    remove the desktop app
set -euo pipefail

APP=khinsider-app
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
TOOLS="$DATA_DIR/khinsider-build"          # user-local JDK + signing key for Android builds

say() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

MODE=desktop; ABIS="armv7"; UNINSTALL=0
for arg in "$@"; do
  case "$arg" in
    --android)   MODE=android ;;
    --all-abis)  ABIS="aarch64 armv7 i686 x86_64" ;;
    --uninstall) UNINSTALL=1 ;;
    -h|--help)   sed -n '2,12p' "$0"; exit 0 ;;
    *)           die "Unknown option: $arg" ;;
  esac
done

# ---------------------------------------------------------------- uninstall
if [ "$UNINSTALL" -eq 1 ]; then
  rm -f "$BIN_DIR/$APP" \
        "$DATA_DIR/applications/$APP.desktop" \
        "$DATA_DIR/icons/hicolor/32x32/apps/$APP.png" \
        "$DATA_DIR/icons/hicolor/128x128/apps/$APP.png"
  say "Removed $APP."
  exit 0
fi

[ -f "$ROOT/src-tauri/Cargo.toml" ] || die "Run this from the project folder (src-tauri/ not found next to install.sh)."
[ "$(id -u)" -ne 0 ] || say "Running as root: everything will be installed for root only."

# ---------------------------------------------------------------- helpers
as_root() {
  if [ "$(id -u)" -eq 0 ]; then "$@"
  elif command -v sudo >/dev/null 2>&1; then sudo "$@"
  elif command -v doas >/dev/null 2>&1; then doas "$@"
  else die "Root is needed to install packages (${MISSING[*]}). Install sudo, or run this script as root."
  fi
}
fetch() { curl -fL --retry 3 -# -o "$2" "$1"; }

# ---------------------------------------------------------------- distro
IDS=" $( . /etc/os-release 2>/dev/null; echo "${ID:-} ${ID_LIKE:-}" ) "
case "$IDS" in
  *" arch "*)                                   FAMILY=arch ;;
  *" debian "*|*" ubuntu "*)                    FAMILY=debian ;;
  *" fedora "*|*" rhel "*|*" centos "*)         FAMILY=fedora ;;
  *" suse "*|*" opensuse "*|*" sles "*)         FAMILY=suse ;;
  *)                                            FAMILY=unknown ;;
esac
say "Detected distro family: $FAMILY  (mode: $MODE)"

# ---------------------------------------------------------------- system packages (only what is missing)
MISSING=()
if [ "$MODE" = android ]; then
  # Android builds need no WebKit/GTK: just a C toolchain, OpenSSL headers (to compile tauri-cli), curl, unzip.
  case "$FAMILY" in
    arch)   PKGS=(gcc make pkgconf curl file openssl unzip tar) ;;
    debian) PKGS=(build-essential pkg-config curl file libssl-dev unzip tar) ;;
    fedora) PKGS=(gcc gcc-c++ make pkgconf-pkg-config curl file openssl-devel unzip tar) ;;
    suse)   PKGS=(gcc gcc-c++ make pkg-config curl file libopenssl-devel unzip tar) ;;
  esac
else
  case "$FAMILY" in
    arch)   PKGS=(gcc make pkgconf curl wget file openssl webkit2gtk-4.1 gtk3 xdotool librsvg) ;;
    debian) PKGS=(build-essential pkg-config curl wget file libwebkit2gtk-4.1-dev libgtk-3-dev libxdo-dev libssl-dev librsvg2-dev) ;;
    fedora) PKGS=(gcc gcc-c++ make pkgconf-pkg-config curl wget file webkit2gtk4.1-devel gtk3-devel libxdo-devel openssl-devel librsvg2-devel) ;;
    suse)   PKGS=(gcc gcc-c++ make pkg-config curl wget file webkit2gtk3-devel gtk3-devel libxdo-devel libopenssl-devel librsvg-devel) ;;
  esac
fi

case "$FAMILY" in
  arch)   mapfile -t MISSING < <(pacman -T "${PKGS[@]}" || true) ;;   # prints only unsatisfied ones
  debian) for p in "${PKGS[@]}"; do
            dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'ok installed' || MISSING+=("$p")
          done ;;
  fedora|suse) for p in "${PKGS[@]}"; do rpm -q --whatprovides "$p" >/dev/null 2>&1 || MISSING+=("$p"); done ;;
  unknown)
    if [ "$MODE" = android ]; then
      for c in cc pkg-config curl unzip tar; do command -v "$c" >/dev/null 2>&1 || die "Unsupported distro and '$c' is missing. Install a C toolchain, pkg-config, OpenSSL headers, curl, unzip, tar and re-run."; done
    else
      command -v pkg-config >/dev/null 2>&1 && pkg-config --exists webkit2gtk-4.1 gtk+-3.0 \
        || die "Unsupported distro. Install WebKitGTK 4.1 + GTK3 dev packages and a C toolchain (https://v2.tauri.app/start/prerequisites/#linux), then re-run."
    fi ;;
esac

if [ "${#MISSING[@]}" -gt 0 ]; then
  say "Installing missing packages: ${MISSING[*]}"
  case "$FAMILY" in
    arch)   as_root pacman -S --needed --noconfirm "${MISSING[@]}" \
              || die "pacman failed. If it says a package was not found, your package database is stale: refresh it yourself (sudo pacman -Sy, or a full update) and re-run." ;;
    debian) as_root env DEBIAN_FRONTEND=noninteractive apt-get update
            as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${MISSING[@]}" ;;
    fedora) as_root dnf install -y "${MISSING[@]}" ;;
    suse)   as_root zypper --non-interactive install --no-recommends "${MISSING[@]}" ;;
  esac
else
  say "All system packages already present."
fi

# ---------------------------------------------------------------- Rust (user-local, ~/.cargo)
export PATH="$HOME/.cargo/bin:$PATH"
if command -v rustup >/dev/null 2>&1; then
  rustup show active-toolchain >/dev/null 2>&1 || { say "rustup has no toolchain yet, installing stable"; rustup default stable; }
elif [ "$MODE" = android ] || ! command -v cargo >/dev/null 2>&1; then
  # Android needs rustup (to add the Android targets), even if a distro Rust is installed.
  say "Installing Rust (rustup) into ~/.cargo"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | RUSTUP_INIT_SKIP_PATH_CHECK=yes sh -s -- -y --profile minimal --no-modify-path
fi
command -v cargo >/dev/null 2>&1 || die "cargo not found after Rust install."

# ================================================================ desktop
build_desktop() {
  say "Building optimized release (the first build takes several minutes)"
  cargo build --release \
    --manifest-path "$ROOT/src-tauri/Cargo.toml" \
    --features tauri/custom-protocol

  say "Installing to $BIN_DIR"
  install -Dm755 "$ROOT/src-tauri/target/release/$APP" "$BIN_DIR/$APP"
  install -Dm644 "$ROOT/src-tauri/icons/32x32.png"   "$DATA_DIR/icons/hicolor/32x32/apps/$APP.png"
  install -Dm644 "$ROOT/src-tauri/icons/128x128.png" "$DATA_DIR/icons/hicolor/128x128/apps/$APP.png"

  mkdir -p "$DATA_DIR/applications"
  cat > "$DATA_DIR/applications/$APP.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=KHInsider
Comment=khinsider with Discord Rich Presence
Exec=$BIN_DIR/$APP
Icon=$APP
Categories=AudioVideo;Audio;Player;
Terminal=false
DESKTOP

  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DATA_DIR/applications" >/dev/null 2>&1 || true
  command -v gtk-update-icon-cache   >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$DATA_DIR/icons/hicolor" >/dev/null 2>&1 || true

  say "Done. Start \"KHInsider\" from your app menu, or run: $BIN_DIR/$APP"
  case ":$PATH:" in *":$BIN_DIR:"*) ;; *) say "(Add $BIN_DIR to your PATH if you want to launch it as '$APP' from a terminal.)" ;; esac
  say "Discord's desktop app must be running for the status to show."
}

# ================================================================ android
build_android() {
  [ "$(uname -m)" = x86_64 ] || die "Android builds need an x86_64 Linux machine (Google ships no ARM Linux SDK tools)."
  mkdir -p "$TOOLS"

  # --- JDK 17 (user-local Temurin, unless a JDK >= 17 is already set up)
  java_ok() { [ -x "$1/bin/javac" ] && [ "$("$1/bin/javac" -version 2>&1 | awk '{print $2}' | cut -d. -f1)" -ge 17 ] 2>/dev/null; }
  if [ -n "${JAVA_HOME:-}" ] && java_ok "$JAVA_HOME"; then
    say "JDK: using $JAVA_HOME"
  elif java_ok "$TOOLS/jdk"; then
    export JAVA_HOME="$TOOLS/jdk"; say "JDK: found $JAVA_HOME"
  else
    say "JDK: downloading Temurin 17 to $TOOLS/jdk"
    fetch "https://api.adoptium.net/v3/binary/latest/17/ga/linux/x64/jdk/hotspot/normal/eclipse" "$TOOLS/jdk.tar.gz"
    rm -rf "$TOOLS/jdk"; mkdir -p "$TOOLS/jdk"
    tar -xzf "$TOOLS/jdk.tar.gz" -C "$TOOLS/jdk" --strip-components=1
    rm -f "$TOOLS/jdk.tar.gz"
    export JAVA_HOME="$TOOLS/jdk"
  fi
  export PATH="$JAVA_HOME/bin:$PATH"

  # --- Android SDK command-line tools, platform-tools, build-tools, NDK (user-local)
  export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
  export ANDROID_SDK_ROOT="$ANDROID_HOME"
  SDKM="$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager"
  if [ ! -x "$SDKM" ]; then
    say "Android SDK: downloading command-line tools to $ANDROID_HOME"
    mkdir -p "$ANDROID_HOME/cmdline-tools"
    fetch "https://dl.google.com/android/repository/commandlinetools-linux-11076708_latest.zip" "$TOOLS/cmdtools.zip"
    rm -rf "$ANDROID_HOME/cmdline-tools/_tmp" "$ANDROID_HOME/cmdline-tools/latest"
    unzip -q -o "$TOOLS/cmdtools.zip" -d "$ANDROID_HOME/cmdline-tools/_tmp"
    mv "$ANDROID_HOME/cmdline-tools/_tmp/cmdline-tools" "$ANDROID_HOME/cmdline-tools/latest"
    rm -rf "$ANDROID_HOME/cmdline-tools/_tmp" "$TOOLS/cmdtools.zip"
  fi

  NDK_VER=27.0.12077973
  BT_VER=34.0.0
  WANT=()
  [ -d "$ANDROID_HOME/platform-tools" ]          || WANT+=("platform-tools")
  [ -d "$ANDROID_HOME/build-tools/$BT_VER" ]     || WANT+=("build-tools;$BT_VER")
  [ -d "$ANDROID_HOME/platforms/android-34" ]    || WANT+=("platforms;android-34")
  [ -d "$ANDROID_HOME/ndk/$NDK_VER" ]            || WANT+=("ndk;$NDK_VER")
  if [ "${#WANT[@]}" -gt 0 ]; then
    say "Android SDK: installing ${WANT[*]} (accepting licenses)"
    { yes || true; } 2>/dev/null | "$SDKM" --licenses >/dev/null 2>&1 || true
    "$SDKM" --install "${WANT[@]}" >/dev/null
  else
    say "Android SDK: all components present."
  fi
  export NDK_HOME="${NDK_HOME:-$ANDROID_HOME/ndk/$NDK_VER}"

  # --- Rust Android targets + Tauri CLI (needed for the Android glue)
  TARGETS=()
  for a in $ABIS; do
    case "$a" in
      aarch64) TARGETS+=(aarch64-linux-android) ;;
      armv7)   TARGETS+=(armv7-linux-androideabi) ;;
      i686)    TARGETS+=(i686-linux-android) ;;
      x86_64)  TARGETS+=(x86_64-linux-android) ;;
    esac
  done
  say "Rust targets: ${TARGETS[*]}"
  rustup target add "${TARGETS[@]}"
  if ! cargo tauri --version >/dev/null 2>&1; then
    say "Installing the Tauri CLI (compiles from source, a few minutes)"
    cargo install tauri-cli --version "^2" --locked
  fi

  cd "$ROOT"
  if [ ! -d src-tauri/gen/android ]; then
    say "Generating the Android project"
    cargo tauri android init --ci
  fi

  say "Building optimized release APK (${ABIS// /, }); the first build takes several minutes"
  TARGET_ARGS=()
  for a in $ABIS; do TARGET_ARGS+=(--target "$a"); done
  cargo tauri android build --apk "${TARGET_ARGS[@]}" --ci

  APK="$(find src-tauri/gen/android/app/build/outputs -name '*.apk' -path '*release*' -printf '%T@ %p\n' | sort -n | tail -1 | cut -d' ' -f2-)"
  [ -n "$APK" ] || die "Build finished but no release APK was found under src-tauri/gen/android/app/build/outputs."

  # --- Sign (an unsigned release APK will not install). Personal key, created once.
  mkdir -p dist
  OUT="dist/KHInsider-android.apk"
  case "$APK" in
    *unsigned*)
      KS="$TOOLS/release.keystore"
      if [ ! -f "$KS" ]; then
        say "Creating a personal signing key at $KS (keep it: updates must use the same key)"
        keytool -genkeypair -keystore "$KS" -alias khinsider -keyalg RSA -keysize 2048 -validity 10000 \
          -storepass khinsider -keypass khinsider -dname "CN=KHInsider" >/dev/null 2>&1
      fi
      BT="$ANDROID_HOME/build-tools/$BT_VER"
      "$BT/zipalign" -f -p 4 "$APK" "$TOOLS/aligned.apk"
      "$BT/apksigner" sign --ks "$KS" --ks-pass pass:khinsider --key-pass pass:khinsider --out "$OUT" "$TOOLS/aligned.apk"
      rm -f "$TOOLS/aligned.apk" "$OUT.idsig"
      ;;
    *) cp "$APK" "$OUT" ;;
  esac
  say "APK ready: $ROOT/$OUT"

  ADB="$ANDROID_HOME/platform-tools/adb"
  if [ -x "$ADB" ] && "$ADB" devices 2>/dev/null | awk 'NR>1 && $2=="device"{f=1} END{exit !f}'; then
    say "Phone detected, installing over adb"
    "$ADB" install -r "$OUT"
    say "Installed. Open KHInsider on your phone."
  else
    say "No phone connected over adb. Copy the APK to your phone and open it (allow 'install unknown apps' when asked),"
    say "or enable USB debugging, plug it in, and re-run this script to install automatically."
  fi
  say "Note: Discord Rich Presence is desktop-only; the Android app plays music but shows no Discord status."
}

case "$MODE" in
  desktop) build_desktop ;;
  android) build_android ;;
esac
