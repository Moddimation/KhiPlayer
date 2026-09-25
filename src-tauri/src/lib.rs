use serde::Deserialize;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

const START_URL: &str = "https://downloads.khinsider.com/";
// Create an application at https://discord.com/developers/applications and paste its Application ID here.
// Used both for the desktop IPC client and (mobile) as the OAuth2 client id + redirect scheme.
// The Discord app must have "Public Client" enabled under OAuth2, and
// discord-1551290011939512440:/authorize/callback must be an allowed OAuth2 redirect.
const DISCORD_APP_ID: &str = "1551290011939512440";

#[cfg(not(desktop))]
mod oauth_presence;

// ---------------------------------------------------------------------------------------------
// Keepalive: playback survives navigating to a different page (and, on desktop, the mini
// player). Design, so this doesn't turn into a second audio engine to maintain:
//
//   - The VISIBLE page's own <audio id="audio1"> stays the one and only thing making sound
//     while that page is open -- nothing about that changes, no muting, no double audio.
//   - A hidden, never-navigating "player" window holds a second <audio> that does nothing
//     *unless* handed a track. On `pagehide`, if the page was mid-playback, inject.js sends one
//     `report_playback` with the exact src/position/volume -- the hidden player picks up
//     exactly where the visible page left off, right as that page's own audio is destroyed by
//     the navigation. If the *new* page never plays anything, the handoff just keeps going
///    ("song keeps playing"). If the new page *does* start something, its own next `sync()`
//     overwrites the hidden player via the same command ("new song wins", automatically).
//   - `player_control` (from the mini player, or the mobile lockscreen via media-session) is
//     broadcast to *both* the main window and the hidden player. Only one of the two is ever
//     actually audible at a time, so applying it to both is harmless -- whichever one isn't
//     making sound just updates its paused/currentTime for free, and the one that matters
//     responds. next/prev only make sense with a visible page open (see inject.js).
//   - `report_now_playing_meta` is a separate, display-only event so the mini player can show
//     title/artist/art live while the visible page is the one actually playing, without that
//     touching the hidden player's audio at all.
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackHandoff {
    src: String,
    position: f64,
    volume: f64,
}

/// What the in-page overlay shows. Kept server-side (`NowPlayingState`) so a freshly loaded page
/// -- with nothing of its own playing yet -- can immediately show what's still playing in the
/// background instead of a blank overlay until something happens locally.
#[derive(Deserialize, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NowPlayingMeta {
    title: String,
    artist: Option<String>,
    artwork: Option<String>,
    paused: bool,
}

#[derive(Default)]
struct NowPlayingState(std::sync::Mutex<Option<NowPlayingMeta>>);

// ---------------------------------------------------------------------------------------------
// Background audio, desktop: plain native playback via rodio, on its own thread. Deliberately
// NOT a hidden webview with an <audio> tag (which is what this used to be) -- a webview nobody
// ever clicks in gets its audio autoplay blocked by browser security policy, differently broken
// on every OS (WebView2, WebKitGTK, WKWebView each need their own workaround, and macOS doesn't
// have a clean one at all through Tauri's API). Native playback has no such restriction: it was
// never a browser security boundary to begin with, so there's nothing to route around.
// ---------------------------------------------------------------------------------------------
#[cfg(desktop)]
mod background_audio {
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{channel, Sender};

    enum Cmd {
        Play { url: String, volume: f32, position: f64 },
        Pause,
        Resume,
        Volume(f32),
        Stop,
    }

    pub struct BackgroundAudio {
        tx: Sender<Cmd>,
        paused: AtomicBool,
    }

    impl BackgroundAudio {
        /// Spawns the audio thread and returns immediately. `rodio::OutputStream` isn't `Send`
        /// on every platform, so it lives entirely inside that one thread; everything else talks
        /// to it over a channel instead of touching it directly.
        pub fn spawn() -> Self {
            let (tx, rx) = channel::<Cmd>();
            std::thread::spawn(move || {
                let (_stream, handle) = match rodio::OutputStream::try_default() {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("[background-audio] no output device: {e}");
                        return;
                    }
                };
                let mut sink: Option<rodio::Sink> = None;
                for cmd in rx {
                    match cmd {
                        Cmd::Play { url, volume, position } => {
                            let fetched = reqwest::blocking::get(&url).and_then(|r| r.bytes());
                            match fetched {
                                Ok(bytes) => match rodio::Decoder::new(Cursor::new(bytes)) {
                                    Ok(source) => match rodio::Sink::try_new(&handle) {
                                        Ok(s) => {
                                            s.set_volume(volume);
                                            // Resumes at the same part the visible page was at
                                            // when it navigated away. This decodes-and-discards
                                            // up to `position`, so a very long skip briefly
                                            // delays playback starting -- fine for a normal
                                            // "picked up mid-song" handoff.
                                            if position > 0.0 {
                                                use rodio::Source;
                                                s.append(source.skip_duration(std::time::Duration::from_secs_f64(position)));
                                            } else {
                                                s.append(source);
                                            }
                                            sink = Some(s);
                                        }
                                        Err(e) => eprintln!("[background-audio] sink: {e}"),
                                    },
                                    Err(e) => eprintln!("[background-audio] decode: {e}"),
                                },
                                Err(e) => eprintln!("[background-audio] fetch: {e}"),
                            }
                        }
                        Cmd::Pause => {
                            if let Some(s) = &sink {
                                s.pause();
                            }
                        }
                        Cmd::Resume => {
                            if let Some(s) = &sink {
                                s.play();
                            }
                        }
                        Cmd::Volume(v) => {
                            if let Some(s) = &sink {
                                s.set_volume(v);
                            }
                        }
                        Cmd::Stop => {
                            if let Some(s) = sink.take() {
                                s.stop();
                            }
                        }
                    }
                }
            });
            Self { tx, paused: AtomicBool::new(false) }
        }

        /// Starts a new track at `position` seconds in, replacing whatever was playing.
        pub fn play(&self, url: String, volume: f32, position: f64) {
            self.paused.store(false, Ordering::SeqCst);
            let _ = self.tx.send(Cmd::Play { url, volume, position });
        }

        pub fn toggle_play_pause(&self) {
            let now_paused = !self.paused.load(Ordering::SeqCst);
            self.paused.store(now_paused, Ordering::SeqCst);
            let _ = self.tx.send(if now_paused { Cmd::Pause } else { Cmd::Resume });
        }

        pub fn set_volume(&self, v: f32) {
            let _ = self.tx.send(Cmd::Volume(v));
        }

        pub fn stop(&self) {
            self.paused.store(false, Ordering::SeqCst);
            let _ = self.tx.send(Cmd::Stop);
        }
    }
}

#[tauri::command]
fn report_playback(app: tauri::AppHandle, info: PlaybackHandoff) {
    #[cfg(desktop)]
    {
        if let Some(audio) = app.try_state::<background_audio::BackgroundAudio>() {
            audio.play(info.src, info.volume as f32, info.position);
        }
    }
    #[cfg(not(desktop))]
    {
        if let Some(player) = app.get_webview_window("player") {
            let _ = player.emit("mirror-playback", info);
        }
    }
}

/// Called the moment a page's own audio genuinely starts playing for real (see inject.js) --
/// stops anything left over from a previous page's handoff so the two are never briefly audible
/// at once.
#[tauri::command]
fn stop_background_audio(app: tauri::AppHandle) {
    #[cfg(desktop)]
    {
        if let Some(audio) = app.try_state::<background_audio::BackgroundAudio>() {
            audio.stop();
        }
    }
    #[cfg(not(desktop))]
    {
        if let Some(player) = app.get_webview_window("player") {
            #[derive(Clone, serde::Serialize)]
            struct Ctl {
                action: String,
            }
            let _ = player.emit("player-control", Ctl { action: "stop".into() });
        }
    }
}

#[tauri::command]
fn report_now_playing_meta(app: tauri::AppHandle, state: tauri::State<NowPlayingState>, meta: NowPlayingMeta) {
    *state.0.lock().unwrap() = Some(meta.clone());
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.emit("now-playing-meta", meta);
    }
}

/// Lets a just-loaded page's overlay backfill immediately, instead of waiting for the next
/// `report_now_playing_meta` push (which won't come at all if nothing plays on the new page).
#[tauri::command]
fn get_now_playing(state: tauri::State<NowPlayingState>) -> Option<NowPlayingMeta> {
    state.0.lock().unwrap().clone()
}

/// Only used for the mobile lockscreen (media-session onAction) reaching into whichever webview
/// is actually playing right now. The in-page overlay itself talks to its own page's <audio>
/// directly and doesn't need this -- see inject.js.
///
/// action: "playPause" | "seek" (value = seconds) | "volume" (value = 0..1) | "next" | "prev"
#[tauri::command]
fn player_control(app: tauri::AppHandle, action: String, value: Option<f64>) {
    #[cfg(desktop)]
    {
        if let Some(audio) = app.try_state::<background_audio::BackgroundAudio>() {
            match action.as_str() {
                "playPause" => audio.toggle_play_pause(),
                "volume" => {
                    if let Some(v) = value {
                        audio.set_volume(v as f32);
                    }
                }
                _ => {} // "seek": not supported by rodio's Sink; "next"/"prev": handled in JS
            }
        }
    }

    #[derive(Clone, serde::Serialize)]
    struct Ctl {
        action: String,
        value: Option<f64>,
    }
    let payload = Ctl { action, value };
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.emit("player-control", payload.clone());
    }
    #[cfg(not(desktop))]
    if let Some(player) = app.get_webview_window("player") {
        let _ = player.emit("player-control", payload);
    }
}


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
    let mut builder = tauri::Builder::default().manage(NowPlayingState::default());

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
            report_playback,
            report_now_playing_meta,
            get_now_playing,
            player_control,
            stop_background_audio,
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

            #[cfg(desktop)]
            app.manage(background_audio::BackgroundAudio::spawn());

            // Mobile only: hidden, never-navigating window hosting <audio id="a">
            // (src/player.html), so a track can keep playing after the visible page above
            // navigates away. Desktop does this with native audio instead -- see
            // background_audio above -- specifically to avoid needing this at all.
            #[cfg(not(desktop))]
            WebviewWindowBuilder::new(app, "player", WebviewUrl::App("player.html".into()))
                .visible(false)
                .inner_size(1.0, 1.0)
                .build()?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running app");
}
