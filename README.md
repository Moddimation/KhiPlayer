# khinsider-app (unofficial wrapper, not affiliated with khinsider)

Tauri v2 shell around https://downloads.khinsider.com/ + Discord Rich Presence
(desktop). Presence is read from the site's own player (#audio1).

## Setup (Linux)
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    cargo install tauri-cli --version "^2" --locked
    # system libs: https://v2.tauri.app/start/prerequisites/

## Configure
    # 1. src-tauri/src/lib.rs -> set DISCORD_APP_ID
    # 2. icons (any square PNG, 1024x1024 ideal):
    cargo tauri icon path/to/icon.png

## Discord presence
Shown as "Listening to KHInsider" with a Spotify-style time bar (start+end in Unix ms).
Line 1 = track, line 2 = album, cover art as large image, click the title to open the page.
Change APP_NAME / STATUS_DISPLAY (Name | Details | State) in src-tauri/src/lib.rs.

## Run / build (desktop)
    cargo tauri dev
    cargo tauri build

## Android (no Discord RPC yet)
    rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
    cargo tauri android init
    cargo tauri android dev
