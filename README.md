# KHInsider App

An unofficial desktop app for [khinsider](https://downloads.khinsider.com/) that shows what
you're listening to on Discord (track, publisher, game).
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

## macOS

No installer script. Install Xcode tools (`xcode-select --install`) and Rust (https://rustup.rs), then in this folder:

    cargo build --release --features tauri/custom-protocol --manifest-path src-tauri/Cargo.toml

and run `src-tauri/target/release/khinsider-app`.

---

Arch users who prefer a real pacman package can use `cd packaging/arch && makepkg -si` instead of `install.sh`.
