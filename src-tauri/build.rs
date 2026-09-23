fn main() {
    // Registers our commands so a capability can allow them for the remote (khinsider) origin.
    // connect_discord / discord_connected only exist under #[cfg(not(desktop))], but listing them
    // here unconditionally is harmless -- this is just ACL metadata, not a link-time reference.
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new().commands(&[
                "set_presence",
                "clear_presence",
                "connect_discord",
                "discord_connected",
                "report_playback",
                "report_now_playing_meta",
                "get_now_playing",
                "player_control",
            ]),
        ),
    )
    .expect("failed to run tauri-build");
}
