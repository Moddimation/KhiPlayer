#!/usr/bin/env bash
# KHInsider App - iOS / iPadOS builder (macOS only).
# Sets up the Apple toolchain bits that are missing, generates the Xcode project
# and either builds a signed .ipa or hands you the project to run from Xcode.
#
#   ./install-ios.sh                 build for a connected iPhone/iPad (needs a team ID)
#   ./install-ios.sh --simulator     run in the iOS Simulator (no Apple account needed)
#   ./install-ios.sh --xcode         generate the project and open it in Xcode (free Apple ID route)
#   ./install-ios.sh --team ABCDE12345   set the Apple development team for this build
#   ./install-ios.sh --clean         delete the generated Xcode project (src-tauri/gen/apple)
#   ./install-ios.sh --token         build the Discord user-token self-bot presence path
#                                     instead of the default OAuth path (against Discord's
#                                     ToS -- see oauth_presence.rs / gateway_presence.rs)

set -euo pipefail

APP=khinsider-app
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

say() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

MODE=device; TEAM="${TAURI_APPLE_DEVELOPMENT_TEAM:-}"; CLEAN=0
PRESENCE_FEATURE=mobile-presence-oauth  # default: sanctioned OAuth + Headless Sessions
while [ $# -gt 0 ]; do
  case "$1" in
    --simulator|-s) MODE=simulator ;;
    --xcode|-x)     MODE=xcode ;;
    --team)         shift; TEAM="${1:-}"; [ -n "$TEAM" ] || die "--team needs a 10-character team ID." ;;
    --token)        PRESENCE_FEATURE=mobile-presence-token ;;
    --clean)        CLEAN=1 ;;
    -h|--help)      sed -n '2,12p' "$0"; exit 0 ;;
    *)              die "Unknown option: $1" ;;
  esac
  shift
done
if [ "$PRESENCE_FEATURE" = mobile-presence-token ]; then
  say "Discord status: --token build (self-bot Gateway path, against Discord's ToS -- your call)"
else
  say "Discord status: OAuth build (sanctioned Headless Sessions path, default)"
fi

[ "$(uname -s)" = Darwin ] || die "iOS apps can only be built on macOS. Apple's toolchain does not run on Linux or Windows."
[ -f "$ROOT/src-tauri/Cargo.toml" ] || die "Run this from the project folder (src-tauri/ not found next to install-ios.sh)."

# ---------------------------------------------------------------- clean
if [ "$CLEAN" -eq 1 ]; then
  rm -rf "$ROOT/src-tauri/gen/apple"
  say "Removed the generated Xcode project. Re-run this script to regenerate it."
  exit 0
fi

# ---------------------------------------------------------------- Xcode
# The Command Line Tools alone are not enough: iOS needs the full Xcode app.
command -v xcode-select >/dev/null 2>&1 || die "xcode-select not found. Install Xcode from the App Store."
DEVDIR="$(xcode-select -p 2>/dev/null || true)"
case "$DEVDIR" in
  *Xcode*.app/Contents/Developer) say "Xcode: $DEVDIR" ;;
  *) die "xcode-select points at '${DEVDIR:-nothing}', which is the Command Line Tools.
   Install Xcode from the App Store, launch it once to finish component setup, then run:
     sudo xcode-select -s /Applications/Xcode.app/Contents/Developer" ;;
esac
xcodebuild -checkFirstLaunchStatus >/dev/null 2>&1 || {
  say "Finishing Xcode's first-launch setup (asks for your password)"
  sudo xcodebuild -runFirstLaunch
}
xcodebuild -license check >/dev/null 2>&1 || {
  say "Accepting the Xcode license (asks for your password)"
  sudo xcodebuild -license accept
}

# ---------------------------------------------------------------- Rust + iOS targets
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v rustup >/dev/null 2>&1; then
  say "Installing Rust (rustup) into ~/.cargo"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | RUSTUP_INIT_SKIP_PATH_CHECK=yes sh -s -- -y --profile minimal --no-modify-path
fi
rustup show active-toolchain >/dev/null 2>&1 || rustup default stable

# aarch64-apple-ios: real devices. -sim / x86_64: Simulator on Apple Silicon and Intel.
WANT_TARGETS=(aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios)
HAVE="$(rustup target list --installed)"
ADD=()
for t in "${WANT_TARGETS[@]}"; do grep -qx "$t" <<<"$HAVE" || ADD+=("$t"); done
if [ "${#ADD[@]}" -gt 0 ]; then
  say "Adding Rust targets: ${ADD[*]}"
  rustup target add "${ADD[@]}"
else
  say "Rust iOS targets already installed."
fi

# ---------------------------------------------------------------- CocoaPods
# Tauri wires the Rust staticlib into the Xcode project through CocoaPods.
if ! command -v pod >/dev/null 2>&1; then
  if command -v brew >/dev/null 2>&1; then
    say "Installing CocoaPods with Homebrew"
    brew install cocoapods
  else
    say "Homebrew not found, installing CocoaPods with the system Ruby (asks for your password)"
    sudo gem install cocoapods
  fi
else
  say "CocoaPods: $(pod --version)"
fi

# ---------------------------------------------------------------- Tauri CLI
if ! cargo tauri --version >/dev/null 2>&1; then
  say "Installing the Tauri CLI (compiles from source, a few minutes)"
  cargo install tauri-cli --version "^2" --locked
fi

cd "$ROOT"

# ---------------------------------------------------------------- generate the Xcode project
if [ ! -d src-tauri/gen/apple ]; then
  say "Generating the iOS project"
  if [ -n "$TEAM" ]; then
    TAURI_APPLE_DEVELOPMENT_TEAM="$TEAM" cargo tauri ios init --ci
  else
    cargo tauri ios init --ci
  fi
else
  say "iOS project already generated (src-tauri/gen/apple)."
fi

XCODEPROJ="$(find src-tauri/gen/apple -maxdepth 1 -name '*.xcodeproj' | head -1)"

case "$MODE" in
  simulator)
    say "Starting the iOS Simulator (Ctrl-C to stop)"
    exec cargo tauri ios dev --no-default-features --features "$PRESENCE_FEATURE"
    ;;

  xcode)
    [ -n "$XCODEPROJ" ] || die "No .xcodeproj found under src-tauri/gen/apple."
    say "Opening $XCODEPROJ"
    say "In Xcode: select the app target > Signing & Capabilities > tick 'Automatically manage signing'"
    say "and pick your Apple ID team. Then choose your iPhone/iPad at the top and press Run."
    say "With a free Apple ID the app stays installed for 7 days; re-run to renew it."
    say "Set the presence backend in the scheme's Run > Arguments, or just edit the default"
    say "feature in src-tauri/Cargo.toml before building from Xcode (this script only controls"
    say "the CLI build path below, not an Xcode Run)."
    exec open "$XCODEPROJ"
    ;;

  device)
    [ -n "$TEAM" ] || die "No Apple development team set.
   A signed build needs your 10-character team ID. Either:
     ./install-ios.sh --team ABCDE12345
     export TAURI_APPLE_DEVELOPMENT_TEAM=ABCDE12345
   or add it to src-tauri/tauri.conf.json under bundle > iOS > developmentTeam.
   Find it at https://developer.apple.com/account (Membership details), or run: cargo tauri info
   No paid developer account? Use: ./install-ios.sh --xcode"
    say "Building a signed release build (the first build takes several minutes)"
    TAURI_APPLE_DEVELOPMENT_TEAM="$TEAM" cargo tauri ios build --export-method debugging --ci \
      -- --no-default-features --features "$PRESENCE_FEATURE"

    mkdir -p dist
    IPA="$(find src-tauri/gen/apple/build -name '*.ipa' -print0 2>/dev/null \
           | xargs -0 ls -t 2>/dev/null | head -1)"
    if [ -n "$IPA" ]; then
      cp "$IPA" "dist/KHInsider-ios.ipa"
      say "IPA ready: $ROOT/dist/KHInsider-ios.ipa"
      say "Install it with Xcode (Window > Devices and Simulators > drag the .ipa in), or Apple Configurator."
    else
      say "Build finished but no .ipa was found under src-tauri/gen/apple/build."
      say "Open $XCODEPROJ and run it from Xcode instead."
    fi
    ;;
esac
