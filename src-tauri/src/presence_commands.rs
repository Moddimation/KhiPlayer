//! Add to src-tauri/src/lib.rs (or main.rs). Desktop's existing discord-rpc
//! setup is untouched -- everything here is behind
//! `#[cfg(any(target_os = "ios", target_os = "android"))]` and split further
//! by the `mobile-presence-token` / `mobile-presence-oauth` Cargo features
//! (see Cargo.toml.patch), so a non-mobile build never even compiles this
//! module.

#[cfg(feature = "mobile-presence-token")]
mod gateway_presence; // the --token build: self-bot Gateway path
#[cfg(feature = "mobile-presence-oauth")]
mod oauth_presence; // the default build: OAuth2 + Headless Sessions API

const SECURE_KEY_TOKEN: &str = "discord_mobile_token";
const SECURE_KEY_OAUTH: &str = "discord_mobile_oauth_tokens";

#[tauri::command]
fn mobile_presence_mode() -> &'static str {
    #[cfg(feature = "mobile-presence-token")]
    { "token" }
    #[cfg(feature = "mobile-presence-oauth")]
    { "oauth" }
}

// ---------------------------------------------------------------- token mode

#[cfg(feature = "mobile-presence-token")]
mod token_commands {
    use super::*;
    use gateway_presence::GatewayPresence;
    use tauri::State;

    #[tauri::command]
    pub async fn start_mobile_presence_token(
        token: String,
        presence: State<'_, GatewayPresence>,
    ) -> Result<(), String> {
        crate::secure_store::set(SECURE_KEY_TOKEN, &token)
            .await
            .map_err(|e| e.to_string())?;
        presence.start(token).await.map_err(|e| e.to_string())
    }

    #[tauri::command]
    pub async fn stop_mobile_presence(presence: State<'_, GatewayPresence>) -> Result<(), String> {
        presence.stop().await;
        crate::secure_store::delete(SECURE_KEY_TOKEN)
            .await
            .map_err(|e| e.to_string())
    }

    #[tauri::command]
    pub async fn mobile_presence_status() -> Result<bool, String> {
        Ok(crate::secure_store::get(SECURE_KEY_TOKEN)
            .await
            .map_err(|e| e.to_string())?
            .is_some())
    }

    pub async fn resume_on_startup(presence: &GatewayPresence) {
        if let Ok(Some(token)) = crate::secure_store::get(SECURE_KEY_TOKEN).await {
            let _ = presence.start(token).await;
        }
    }
}

// ---------------------------------------------------------------- oauth mode

#[cfg(feature = "mobile-presence-oauth")]
mod oauth_commands {
    use super::*;
    use oauth_presence::OauthPresence;
    use tauri::State;

    #[tauri::command]
    pub async fn start_discord_login(
        presence: State<'_, OauthPresence>,
        app: tauri::AppHandle,
    ) -> Result<(), String> {
        let url = presence.begin_login().await;
        // opener/shell plugin -- opens the system browser, not an in-app webview
        tauri_plugin_opener::open_url(&app, url, None::<String>).map_err(|e| e.to_string())
    }

    #[tauri::command]
    pub async fn finish_discord_login(
        code: String,
        presence: State<'_, OauthPresence>,
    ) -> Result<(), String> {
        presence.complete_login(code).await.map_err(|e| e.to_string())
        // presence.complete_login already stores tokens in-memory and starts
        // the keepalive loop; persist to secure storage here too so a
        // relaunch doesn't require re-login (mirror what resume_on_startup
        // below expects -- wire crate::secure_store::set(SECURE_KEY_OAUTH, ...)
        // with the refresh_token once you add a getter to OauthPresence).
    }

    #[tauri::command]
    pub async fn stop_mobile_presence(presence: State<'_, OauthPresence>) -> Result<(), String> {
        presence.logout().await;
        crate::secure_store::delete(SECURE_KEY_OAUTH)
            .await
            .map_err(|e| e.to_string())
    }

    #[tauri::command]
    pub async fn mobile_presence_status(presence: State<'_, OauthPresence>) -> Result<bool, String> {
        Ok(presence.is_logged_in().await)
    }
}

// register in your invoke_handler![...]:
//   mobile_presence_mode,
//   #[cfg(feature = "mobile-presence-token")]
//   token_commands::{start_mobile_presence_token, stop_mobile_presence, mobile_presence_status},
//   #[cfg(feature = "mobile-presence-oauth")]
//   oauth_commands::{start_discord_login, finish_discord_login, stop_mobile_presence, mobile_presence_status},
