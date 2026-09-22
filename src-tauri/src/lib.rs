use serde::Deserialize;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const START_URL: &str = "https://downloads.khinsider.com/";
// Create an application at https://discord.com/developers/applications and paste its Application ID here.
// Used both for the desktop IPC client and (mobile) as the OAuth2 client id + redirect scheme.
// The Discord app must have "Public Client" enabled under OAuth2, and
// discord-1551290011939512440:/authorize/callback must be an allowed OAuth2 redirect.
const DISCORD_APP_ID: &str = "1551290011939512440";

mod oauth_presence;

// On mobile there is no Discord IPC, so these fields are read by oauth_presence instead.
#[derive(Deserialize, Clone)]
#[cfg_attr(not(desktop), allow(dead_code))]
#[serde(rename_all = "camelCase")]
pub struct Presence {
    details: String,
    state: String,
    image: Option<String>,
    image_text: Option<String>,
    paused: bool,
    url: Option<String>,
    /// Unix time in milliseconds (Discord's unit); start+end together draw the Spotify-style time bar
    start: Option<i64>,
    end: Option<i64>,
}

#[cfg(desktop)]
mod rpc {
    use super::Presence;
    use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
    use std::sync::Mutex;

    /// Shown as "Listening to <APP_NAME>" (Name) - or use Details / State to show the track / album in the member list.
    const APP_NAME: &str = "KHInsider";
    const STATUS_DISPLAY: activity::StatusDisplayType = activity::StatusDisplayType::Name;

    #[derive(Default)]
    pub struct State(Mutex<Option<DiscordIpcClient>>);

    fn connect(app_id: &str) -> Option<DiscordIpcClient> {
        let mut c = DiscordIpcClient::new(app_id);
        c.connect().ok()?; // Discord not running => None, retried on the next update
        Some(c)
    }

    pub fn set(state: &State, app_id: &str, p: &Presence) {
        let mut guard = state.0.lock().unwrap();
        if guard.is_none() {
            *guard = connect(app_id);
        }
        let Some(client) = guard.as_mut() else { return };

        let mut assets = activity::Assets::new();
        if let Some(i) = &p.image {
            assets = assets.large_image(i.as_str());
        }
        if let Some(t) = &p.image_text {
            assets = assets.large_text(t.as_str());
        }
        if let Some(u) = &p.url {
            assets = assets.large_url(u.as_str());
        }

        let state_text = if p.paused { format!("Paused - {}", p.state) } else { p.state.clone() };
        let mut act = activity::Activity::new()
            .name(APP_NAME)
            .activity_type(activity::ActivityType::Listening)
            .status_display_type(STATUS_DISPLAY)
            .details(p.details.as_str())
            .state(state_text.as_str())
            .assets(assets);
        if let Some(u) = &p.url {
            act = act.details_url(u.as_str());
        }
        // Spotify-style bar: only while playing, and only with both start and end
        if !p.paused {
            if let Some(s) = p.start {
                let mut ts = activity::Timestamps::new().start(s);
                if let Some(e) = p.end {
                    ts = ts.end(e);
                }
                act = act.timestamps(ts);
            }
        }
        if client.set_activity(act).is_err() {
            *guard = None; // connection died; reconnect on the next update
        }
    }

    pub fn clear(state: &State) {
        if let Some(mut c) = state.0.lock().unwrap().take() {
            let _ = c.clear_activity();
            let _ = c.close();
        }
    }
}

// On mobile, "presence" goes out over OAuth2 + Discord's Headless Sessions API instead of IPC.
// `rpc::State` is a plain alias for `OauthPresence`, so `set`/`clear`/`connect_discord` all forward
// straight into it; see oauth_presence.rs for the actual login + HTTP logic.
#[cfg(not(desktop))]
mod rpc {
    use super::Presence;
    pub use crate::oauth_presence::OauthPresence as State;

    pub fn set(state: &State, _app_id: &str, p: &Presence) {
        state.update(Some(p.clone()));
    }

    pub fn clear(state: &State) {
        state.update(None);
    }
}

#[tauri::command]
fn set_presence(state: tauri::State<rpc::State>, presence: Presence) {
    rpc::set(&state, DISCORD_APP_ID, &presence);
}

#[tauri::command]
fn clear_presence(state: tauri::State<rpc::State>) {
    rpc::clear(&state);
}

// ---------------------------------------------------------------------------------------------
// Mobile-only commands. Login happens in the system browser (Tauri's webview can't complete an
// OAuth2 flow against a site that blocks embedded webviews), and Discord bounces back to the app
// via the `discord-<app id>:/authorize/callback` deep link registered in tauri.conf.json. Because
// login happens in a *separate* browser, our own webview never navigates away in the first place,
// so there's nothing to "return to" -- Android brings the app back to the foreground on its own
// once the redirect fires.
// ---------------------------------------------------------------------------------------------

#[cfg(not(desktop))]
#[tauri::command]
fn connect_discord(state: tauri::State<rpc::State>, app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let url = state.begin_login();
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[cfg(not(desktop))]
#[tauri::command]
fn discord_connected(state: tauri::State<rpc::State>) -> bool {
    state.is_logged_in()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
    {
        builder = builder.manage(rpc::State::default());
    }
    #[cfg(not(desktop))]
    {
        builder = builder
            .plugin(tauri_plugin_opener::init())
            .plugin(tauri_plugin_deep_link::init())
            // Keeps audio alive with the app backgrounded (foreground service on Android) and
            // gives the site's now-playing info a lockscreen/notification surface. inject.js
            // pushes updates into it via `plugin:media-session|update_state`.
            .plugin(tauri_plugin_media_session::init());
    }

    builder
        .invoke_handler(tauri::generate_handler![
            set_presence,
            clear_presence,
            #[cfg(not(desktop))]
            connect_discord,
            #[cfg(not(desktop))]
            discord_connected,
        ])
        .setup(|app| {
            #[cfg(not(desktop))]
            {
                // `data_dir/discord_oauth.json` -- private to the app, survives restarts and updates
                // (but not uninstalls). Delete it to force a fresh login.
                let store = app.path().app_data_dir().ok().map(|d| d.join("discord_oauth.json"));
                let oauth = rpc::State::new(DISCORD_APP_ID.to_string(), store);
                oauth.load_saved();
                app.manage(oauth);

                // Discord redirected back here: hand the callback URL to the pending login.
                use tauri_plugin_deep_link::DeepLinkExt;
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        let handle = handle.clone();
                        let url = url.to_string();
                        tauri::async_runtime::spawn(async move {
                            if let Some(oauth) = handle.try_state::<rpc::State>() {
                                if let Err(e) = oauth.finish_login(url).await {
                                    eprintln!("[oauth] login failed: {e}");
                                }
                            }
                        });
                    }
                });
            }

            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(START_URL.parse().unwrap()))
                .title("KHInsider")
                .inner_size(1100.0, 800.0)
                .initialization_script(include_str!("../inject.js"))
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running app");
}
