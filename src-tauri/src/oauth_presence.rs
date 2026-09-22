//! Mobile Discord Rich Presence via OAuth2 (PKCE) + Discord's Headless Sessions API.
//!
//! Flow (see inject.js / android-overlay for the other half):
//!   1. `begin_login()` -> authorize URL (opened in the *system browser* by the Android bridge)
//!   2. Discord redirects to `discord-<app id>:/authorize/callback?code=..&state=..`;
//!      Android routes that intent to MainActivity, which hands the URL to the page,
//!      which calls `finish_login(url)`.
//!   3. Tokens are stored in the app's private data dir, so the login survives restarts.
//!   4. A background worker keeps one headless session alive and updates it whenever
//!      `update()` receives new "now playing" info.
//!
//! Scope: `openid sdk.social_layer_presence`. Discord does not hand out the narrower
//! `activities.write` scope, and this umbrella scope is the only self-serve way to write
//! activities. It also grants friends-list access, which this module never uses.
//!
//! The Discord application must have "Public Client" enabled (Developer Portal -> OAuth2),
//! so the token exchange works with PKCE alone and no client secret ships in the APK.

use crate::Presence;
use base64::Engine as _;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;

const AUTHORIZE_URL: &str = "https://discord.com/oauth2/authorize";
const API_BASE: &str = "https://discord.com/api/v10";
const SCOPE: &str = "openid sdk.social_layer_presence";
// Headless sessions expire after ~20 minutes without an update.
const KEEPALIVE: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Serialize, Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    expires_at: i64, // unix seconds
}

struct Pending {
    verifier: String,
    state: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

struct ApiErr {
    status: u16,
    msg: String,
}

struct Inner {
    client_id: String,
    redirect_uri: String,
    store_path: Option<PathBuf>,
    http: reqwest::Client,
    tokens: Mutex<Option<Tokens>>,
    pending: Mutex<Option<Pending>>,
    now_playing: watch::Sender<Option<Presence>>,
    worker_stop: Mutex<Option<watch::Sender<bool>>>,
}

#[derive(Clone)]
pub struct OauthPresence(Arc<Inner>);

impl OauthPresence {
    pub fn new(client_id: String, store_path: Option<PathBuf>) -> Self {
        let (tx, _rx) = watch::channel(None);
        let redirect_uri = format!("discord-{client_id}:/authorize/callback");
        Self(Arc::new(Inner {
            client_id,
            redirect_uri,
            store_path,
            http: reqwest::Client::new(),
            tokens: Mutex::new(None),
            pending: Mutex::new(None),
            now_playing: tx,
            worker_stop: Mutex::new(None),
        }))
    }

    /// Restore a previous login from disk (call once at startup).
    pub fn load_saved(&self) {
        let Some(path) = &self.0.store_path else { return };
        let Ok(text) = std::fs::read_to_string(path) else { return };
        let Ok(tokens) = serde_json::from_str::<Tokens>(&text) else { return };
        *self.0.tokens.lock().unwrap() = Some(tokens);
        self.start_worker();
    }

    pub fn is_logged_in(&self) -> bool {
        self.0.tokens.lock().unwrap().is_some()
    }

    /// Latest "now playing" (None = nothing playing). Kept even while logged out, so the
    /// worker can publish it right after a login.
    pub fn update(&self, p: Option<Presence>) {
        self.0.now_playing.send_replace(p);
    }

    /// Step 1: build the authorize URL.
    pub fn begin_login(&self) -> String {
        let verifier = random_b64(32);
        let state = random_b64(16);
        let challenge = b64(&Sha256::digest(verifier.as_bytes()));
        *self.0.pending.lock().unwrap() = Some(Pending {
            verifier,
            state: state.clone(),
        });
        format!(
            "{AUTHORIZE_URL}?client_id={cid}&response_type=code&redirect_uri={redir}&scope={scope}\
             &state={state}&code_challenge={challenge}&code_challenge_method=S256",
            cid = self.0.client_id,
            redir = urlencoding::encode(&self.0.redirect_uri),
            scope = urlencoding::encode(SCOPE),
        )
    }

    /// Step 2: `url` is the full redirect URL Discord sent us back to.
    pub async fn finish_login(&self, url: String) -> Result<(), String> {
        let parsed = url::Url::parse(&url).map_err(|e| format!("bad callback URL: {e}"))?;
        let mut code = None;
        let mut state = None;
        let mut error = None;
        for (k, v) in parsed.query_pairs() {
            match k.as_ref() {
                "code" => code = Some(v.into_owned()),
                "state" => state = Some(v.into_owned()),
                "error_description" | "error" => {
                    if error.is_none() {
                        error = Some(v.into_owned());
                    }
                }
                _ => {}
            }
        }
        if let Some(e) = error {
            return Err(format!("Discord: {e}"));
        }
        let code = code.ok_or("callback has no code")?;
        let pending = self
            .0
            .pending
            .lock()
            .unwrap()
            .take()
            .ok_or("no login in progress (start it from the app again)")?;
        if state.as_deref() != Some(pending.state.as_str()) {
            return Err("login state mismatch, please try again".into());
        }

        let resp = self
            .token_request(&[
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("redirect_uri", self.0.redirect_uri.as_str()),
                ("code_verifier", pending.verifier.as_str()),
            ])
            .await?;
        self.store_tokens(resp);
        self.start_worker();
        Ok(())
    }

    // ------------------------------------------------------------------ tokens

    async fn token_request(&self, extra: &[(&str, &str)]) -> Result<TokenResponse, String> {
        let mut form: Vec<(&str, &str)> = vec![("client_id", self.0.client_id.as_str())];
        form.extend_from_slice(extra);
        let resp = self
            .0
            .http
            .post(format!("{API_BASE}/oauth2/token"))
            .form(&form)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Discord token error {status}: {text}"));
        }
        resp.json::<TokenResponse>().await.map_err(|e| e.to_string())
    }

    fn store_tokens(&self, resp: TokenResponse) {
        let tokens = Tokens {
            access_token: resp.access_token,
            refresh_token: resp.refresh_token,
            expires_at: now_secs() + resp.expires_in,
        };
        if let Some(path) = &self.0.store_path {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(text) = serde_json::to_string(&tokens) {
                let _ = std::fs::write(path, text);
            }
        }
        *self.0.tokens.lock().unwrap() = Some(tokens);
    }

    /// Refresh token rejected / revoked: forget the login so the app asks again.
    fn invalidate(&self) {
        *self.0.tokens.lock().unwrap() = None;
        if let Some(path) = &self.0.store_path {
            let _ = std::fs::remove_file(path);
        }
        if let Some(stop) = self.0.worker_stop.lock().unwrap().take() {
            let _ = stop.send(true);
        }
    }

    async fn access_token(&self, force_refresh: bool) -> Result<String, String> {
        let cur = self
            .0
            .tokens
            .lock()
            .unwrap()
            .clone()
            .ok_or("not logged in")?;
        if !force_refresh && now_secs() < cur.expires_at - 60 {
            return Ok(cur.access_token);
        }
        match self
            .token_request(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", cur.refresh_token.as_str()),
            ])
            .await
        {
            Ok(resp) => {
                let access = resp.access_token.clone();
                self.store_tokens(resp);
                Ok(access)
            }
            Err(e) => {
                if e.contains("invalid_grant") {
                    self.invalidate();
                }
                Err(e)
            }
        }
    }

    // ------------------------------------------------------------------ worker

    fn start_worker(&self) {
        let (stop_tx, mut stop_rx) = watch::channel(false);
        if let Some(old) = self.0.worker_stop.lock().unwrap().replace(stop_tx) {
            let _ = old.send(true);
        }
        let this = self.clone();
        let mut rx = self.0.now_playing.subscribe();

        tauri::async_runtime::spawn(async move {
            let mut session: Option<String> = None;
            let mut tick = tokio::time::interval(KEEPALIVE);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            // Publish whatever is playing right now (e.g. right after the login finished).
            let first = rx.borrow().clone();
            this.sync(&mut session, &first).await;

            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        let np = rx.borrow().clone();
                        this.sync(&mut session, &np).await;
                    }
                    changed = rx.changed() => {
                        if changed.is_err() { break; }
                        let np = rx.borrow_and_update().clone();
                        this.sync(&mut session, &np).await;
                    }
                    _ = stop_rx.changed() => { break; }
                }
            }
            if let Some(tok) = session.take() {
                this.delete_session(&tok).await;
            }
        });
    }

    async fn sync(&self, session: &mut Option<String>, np: &Option<Presence>) {
        match np {
            None => {
                if let Some(tok) = session.take() {
                    self.delete_session(&tok).await;
                }
            }
            Some(p) => match self.push_session(session.as_deref(), p).await {
                Ok(tok) => *session = Some(tok),
                Err(e) => eprintln!("[oauth_presence] update failed: {e}"),
            },
        }
    }

    async fn push_session(&self, token: Option<&str>, p: &Presence) -> Result<String, String> {
        let mut result = self.post_session(token, p, true, false).await;
        // Image URLs are not accepted everywhere: retry once without assets.
        if matches!(&result, Err(e) if e.status == 400) && p.image.is_some() {
            result = self.post_session(token, p, false, false).await;
        }
        // Expired/revoked access token: refresh once and retry.
        if matches!(&result, Err(e) if e.status == 401) {
            result = self.post_session(token, p, p.image.is_some(), true).await;
        }
        result.map_err(|e| format!("{} {}", e.status, e.msg))
    }

    async fn post_session(
        &self,
        token: Option<&str>,
        p: &Presence,
        with_assets: bool,
        force_refresh: bool,
    ) -> Result<String, ApiErr> {
        let access = self
            .access_token(force_refresh)
            .await
            .map_err(|msg| ApiErr { status: 0, msg })?;

        let state_text = if p.paused {
            format!("Paused - {}", p.state)
        } else {
            p.state.clone()
        };
        let mut activity = json!({
            "type": 2, // LISTENING
            "name": "KHInsider",
            "application_id": self.0.client_id,
            "platform": if cfg!(target_os = "ios") { "ios" } else { "android" },
            "details": p.details,
            "state": state_text,
        });
        if !p.paused {
            if let Some(start) = p.start {
                let mut ts = json!({ "start": start });
                if let Some(end) = p.end {
                    ts["end"] = json!(end);
                }
                activity["timestamps"] = ts;
            }
        }
        if with_assets {
            let mut assets = json!({});
            if let Some(i) = &p.image {
                assets["large_image"] = json!(i);
            }
            if let Some(t) = &p.image_text {
                assets["large_text"] = json!(t);
            }
            if assets.as_object().map_or(false, |o| !o.is_empty()) {
                activity["assets"] = assets;
            }
        }
        let mut body = json!({ "activities": [activity] });
        if let Some(t) = token {
            body["token"] = json!(t);
        }

        let resp = self
            .0
            .http
            .post(format!("{API_BASE}/users/@me/headless-sessions"))
            .bearer_auth(access)
            .json(&body)
            .send()
            .await
            .map_err(|e| ApiErr { status: 0, msg: e.to_string() })?;
        let status = resp.status();
        if !status.is_success() {
            let msg = resp.text().await.unwrap_or_default();
            return Err(ApiErr { status: status.as_u16(), msg });
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ApiErr { status: 0, msg: e.to_string() })?;
        v["token"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or(ApiErr { status: 0, msg: "response has no session token".into() })
    }

    async fn delete_session(&self, token: &str) {
        let Ok(access) = self.access_token(false).await else { return };
        let _ = self
            .0
            .http
            .post(format!("{API_BASE}/users/@me/headless-sessions/delete"))
            .bearer_auth(access)
            .json(&json!({ "token": token }))
            .send()
            .await;
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random_b64(n: usize) -> String {
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    b64(&buf)
}
