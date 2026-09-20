# KHInsider App

An unofficial desktop app for [khinsider](https://downloads.khinsider.com/) that shows what
you're listening to on Discord (track, "by publisher", "in Game (Platform)").
Not affiliated with khinsider.

The installer builds the optimized release version on your own computer and installs it
for your user. It checks what's missing and installs only that. Re-run it any time to update.
Discord's desktop app must be open for the Discord status to show.

---

## Windows

Double-click **`install.bat`**.

It downloads and installs whatever is missing (C++ Build Tools, WebView2, Rust), builds the app,
and adds **KHInsider** to your Start menu. Windows shows one admin prompt if the C++ Build Tools
need installing; click Yes. That first install is several GB and takes a while.

Uninstall: `install.bat -Uninstall`

## Linux

    ./install.sh

It detects your distro (Arch, Debian/Ubuntu, Fedora, openSUSE families), installs only the missing
build packages, gets Rust into `~/.cargo` if you don't have it, builds the app, and installs it to
`~/.local/bin` with an app menu entry. It asks for your sudo password only if system packages
are missing.

Uninstall: `./install.sh --uninstall`

## Android

Same scripts, one extra flag. It sets up a JDK, the Android SDK and NDK inside your home folder
(nothing system-wide), builds an optimized signed release APK, and installs it on your phone if
one is plugged in.

    ./install.sh --android              # Linux (x86_64)
    install.bat -Android                # Windows (x86_64)

The APK ends up in `dist/KHInsider-android.apk`. To install it automatically, turn on
**USB debugging** on the phone and plug it in before running the script. Otherwise copy the APK
to the phone and open it (allow "install unknown apps").

Add `--all-abis` / `-AllAbis` only if you need 32-bit ARM or x86 (bigger APK, much slower build).
Windows will ask for admin once to switch on Developer Mode (Android builds need symlinks).
The script creates a personal signing key once; keep it, later updates must be signed with the same one.

**Discord status on phones:** Discord's status works through a local connection to the desktop Discord
app, which Android and iOS don't have. The phone app plays music but shows no Discord status.

## macOS / iOS

No installer script. macOS desktop: install Xcode tools (`xcode-select --install`) and Rust (https://rustup.rs), then

    cargo build --release --features tauri/custom-protocol --manifest-path src-tauri/Cargo.toml

and run `src-tauri/target/release/khinsider-app`. iOS additionally needs Xcode and an Apple developer
account (`cargo install tauri-cli --version "^2" --locked`, then `cargo tauri ios init` and `cargo tauri ios build`).

---

Arch users who prefer a real pacman package can use `cd packaging/arch && makepkg -si` instead of `install.sh`.
