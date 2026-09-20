fn main() {
    // Registers our commands so a capability can allow them for the remote (khinsider) origin.
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(
            tauri_build::AppManifest::new().commands(&["set_presence", "clear_presence"]),
        ),
    )
    .expect("failed to run tauri-build");
}
