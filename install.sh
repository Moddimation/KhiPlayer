#!/usr/bin/env bash
# KHInsider App - Linux installer.
# Detects your distro, installs only the packages that are missing, builds the
# optimized release binary and installs it for your user (nothing system-wide
# except the build dependencies). Re-run it any time to update.
#
#   ./install.sh              install / update
#   ./install.sh --uninstall  remove the app
set -euo pipefail

APP=khinsider-app
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"

say() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------- uninstall
if [ "${1:-}" = "--uninstall" ]; then
  rm -f "$BIN_DIR/$APP" \
        "$DATA_DIR/applications/$APP.desktop" \
        "$DATA_DIR/icons/hicolor/32x32/apps/$APP.png" \
        "$DATA_DIR/icons/hicolor/128x128/apps/$APP.png"
  say "Removed $APP."
  exit 0
fi

[ -f "$ROOT/src-tauri/Cargo.toml" ] || die "Run this from the project folder (src-tauri/ not found next to install.sh)."
[ "$(id -u)" -ne 0 ] || say "Running as root: the app will be installed for root only."

# ---------------------------------------------------------------- helpers
as_root() {
  if [ "$(id -u)" -eq 0 ]; then "$@"
  elif command -v sudo >/dev/null 2>&1; then sudo "$@"
  elif command -v doas >/dev/null 2>&1; then doas "$@"
  else die "Root is needed to install packages (${MISSING[*]}). Install sudo, or run this script as root."
  fi
}

# ---------------------------------------------------------------- distro
IDS=" $( . /etc/os-release 2>/dev/null; echo "${ID:-} ${ID_LIKE:-}" ) "
case "$IDS" in
  *" arch "*)                                   FAMILY=arch ;;
  *" debian "*|*" ubuntu "*)                    FAMILY=debian ;;
  *" fedora "*|*" rhel "*|*" centos "*)         FAMILY=fedora ;;
  *" suse "*|*" opensuse "*|*" sles "*)         FAMILY=suse ;;
  *)                                            FAMILY=unknown ;;
esac
say "Detected distro family: $FAMILY"

# ---------------------------------------------------------------- build dependencies
MISSING=()
case "$FAMILY" in
  arch)
    PKGS=(gcc make pkgconf curl wget file openssl webkit2gtk-4.1 gtk3 xdotool librsvg)
    mapfile -t MISSING < <(pacman -T "${PKGS[@]}" || true)   # prints only unsatisfied ones
    ;;
  debian)
    PKGS=(build-essential pkg-config curl wget file libwebkit2gtk-4.1-dev libgtk-3-dev libxdo-dev libssl-dev librsvg2-dev)
    for p in "${PKGS[@]}"; do
      dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'ok installed' || MISSING+=("$p")
    done
    ;;
  fedora)
    PKGS=(gcc gcc-c++ make pkgconf-pkg-config curl wget file webkit2gtk4.1-devel gtk3-devel libxdo-devel openssl-devel librsvg2-devel)
    for p in "${PKGS[@]}"; do rpm -q --whatprovides "$p" >/dev/null 2>&1 || MISSING+=("$p"); done
    ;;
  suse)
    PKGS=(gcc gcc-c++ make pkg-config curl wget file webkit2gtk3-devel gtk3-devel libxdo-devel libopenssl-devel librsvg-devel)
    for p in "${PKGS[@]}"; do rpm -q --whatprovides "$p" >/dev/null 2>&1 || MISSING+=("$p"); done
    ;;
  unknown)
    command -v pkg-config >/dev/null 2>&1 && pkg-config --exists webkit2gtk-4.1 gtk+-3.0 \
      || die "Unsupported distro. Install WebKitGTK 4.1 + GTK3 dev packages and a C toolchain (https://v2.tauri.app/start/prerequisites/#linux), then re-run."
    ;;
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
if ! command -v cargo >/dev/null 2>&1; then
  say "Installing Rust into ~/.cargo"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
elif command -v rustup >/dev/null 2>&1 && ! rustup show active-toolchain >/dev/null 2>&1; then
  say "rustup has no toolchain yet, installing stable"
  rustup default stable
fi
command -v cargo >/dev/null 2>&1 || die "cargo not found after Rust install."

# ---------------------------------------------------------------- build (optimized release)
say "Building optimized release (the first build takes several minutes)"
cargo build --release \
  --manifest-path "$ROOT/src-tauri/Cargo.toml" \
  --features tauri/custom-protocol

# ---------------------------------------------------------------- install for this user
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
