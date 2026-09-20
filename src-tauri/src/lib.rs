use serde::Deserialize;
use tauri::{WebviewUrl, WebviewWindowBuilder};

const START_URL: &str = "https://downloads.khinsider.com/";
// Create an application at https://discord.com/developers/applications and paste its Application ID here.
const DISCORD_APP_ID: &str = "1551290011939512440";

// On mobile there is no Discord IPC, so the presence fields are received but unused.
#[derive(Deserialize)]
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

#[cfg(not(desktop))]
mod rpc {
    use super::Presence;
    #[derive(Default)]
    pub struct State;
    pub fn set(_: &State, _: &str, _: &Presence) {}
    pub fn clear(_: &State) {}
}

#[tauri::command]
fn set_presence(state: tauri::State<rpc::State>, presence: Presence) {
    rpc::set(&state, DISCORD_APP_ID, &presence);
}

#[tauri::command]
fn clear_presence(state: tauri::State<rpc::State>) {
    rpc::clear(&state);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(rpc::State::default())
        .invoke_handler(tauri::generate_handler![set_presence, clear_presence])
        .setup(|app| {
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
