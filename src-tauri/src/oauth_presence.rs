//! Mobile Rich Presence via OAuth2 + Discord's Headless Sessions API.
//!
//! This is the sanctioned replacement for gateway_presence.rs. It never
//! touches a user token or the Gateway directly -- it's a normal "Login
//! with Discord" OAuth2 flow, same shape as any bot dashboard's login
//! button, just requesting the `sdk.social_layer_presence` scope instead
//! of `identify`.
//!
//! Why that scope and not the narrower `activities.write`: `activities.write`
//! is currently not grantable to third-party apps at all. The only working,
//! self-serve route to activity writes today is through the Social SDK's
//! umbrella scope, `sdk.social_layer_presence`. It grants more than presence
//! writes (gateway-connect-on-your-behalf, friends-list read/write) -- this
//! module only ever calls the headless-session endpoints below and nothing
//! else, but disclose the full scope grant to the user before they approve
//! it, since that's what they're actually authorizing.
//!
//! Endpoints used (POST https://discord.com/api/v10/...):
//!   /users/@me/headless-sessions         create/update (send `token` to update)
//!   /users/@me/headless-sessions/delete  end the session
//! Sessions expire after 20 minutes untouched, hence the keepalive loop.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{watch, Mutex};

const AUTHORIZE_URL: &str = "https://discord.com/oauth2/authorize";
const TOKEN_URL: &str = "https://discord.com/api/v10/oauth2/token";
const API_BASE: &str = "https://discord.com/api/v10";
const SCOPE: &str = "sdk.social_layer_presence";

// Headless sessions expire at 20 min; refresh well before that.
const SESSION_REFRESH_INTERVAL: Duration = Duration::from_secs(12 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NowPlaying {
    pub track: String,
    pub game: String,     // -> "details"
    pub platform: String, // -> "state"
    pub started_at: i64,  // unix ms
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredTokens {
    access_token: String,
    refresh_token: String,
    expires_at: i64, // unix seconds
}

/// You must register this as a URL scheme in the mobile app (Info.plist /
/// AndroidManifest) and as a redirect URI on the Discord application, e.g.
/// via tauri-plugin-deep-link. Any unused custom scheme works.
const REDIRECT_URI: &str = "khiplayer://oauth-callback";

#[derive(Clone)]
pub struct OauthPresence {
    client_id: String,
    client_secret: String, // see note on public clients + PKCE below
    tokens: Arc<Mutex<Option<StoredTokens>>>,
    now_playing: watch::Sender<Option<NowPlaying>>,
    pending_verifier: Arc<Mutex<Option<String>>>,
    shutdown: Arc<Mutex<Option<watch::Sender<bool>>>>,
    application_id: String, // your Discord application's own ID, for the activity object
}

impl OauthPresence {
    pub fn new(client_id: String, client_secret: String, application_id: String) -> (Self, watch::Receiver<Option<NowPlaying>>) {
        let (tx, rx) = watch::channel(None);
        (
            Self {
                client_id,
                client_secret,
                tokens: Arc::new(Mutex::new(None)),
                now_playing: tx,
                pending_verifier: Arc::new(Mutex::new(None)),
                shutdown: Arc::new(Mutex::new(None)),
                application_id,
            },
            rx,
        )
    }

    /// Step 1: build the authorize URL and hand it to the system browser
    /// (ASWebAuthenticationSession on iOS, Chrome Custom Tabs on Android --
    /// never an in-app webview, so the user is entering credentials on
    /// Discord's own page, not one you control).
    pub async fn begin_login(&self) -> String {
        let verifier = generate_code_verifier();
        let challenge = pkce_challenge(&verifier);
        *self.pending_verifier.lock().await = Some(verifier);

        format!(
            "{AUTHORIZE_URL}?client_id={cid}&response_type=code&redirect_uri={redir}&scope={scope}\
             &code_challenge={chal}&code_challenge_method=S256",
            cid = self.client_id,
            redir = urlencoding::encode(REDIRECT_URI),
            scope = urlencoding::encode(SCOPE),
            chal = challenge,
        )
    }

    /// Step 2: called from the deep-link handler once Discord redirects back
    /// with `?code=...`.
    pub async fn complete_login(&self, code: String) -> anyhow::Result<()> {
        let verifier = self
            .pending_verifier
            .lock()
            .await
            .take()
            .ok_or_else(|| anyhow::anyhow!("no login in progress"))?;

        let client = reqwest::Client::new();
        let resp: TokenResponse = client
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()), // see note below
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", REDIRECT_URI),
                ("code_verifier", &verifier),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        self.store_tokens(resp).await;
        self.start_keepalive();
        Ok(())
    }

    pub async fn logout(&self) {
        if let Some(tx) = self.shutdown.lock().await.take() {
            let _ = tx.send(true);
        }
        if let Some(tokens) = self.tokens.lock().await.take() {
            let client = reqwest::Client::new();
            // best-effort: end whatever session is live
            let _ = client
                .post(format!("{API_BASE}/users/@me/headless-sessions/delete"))
                .bearer_auth(&tokens.access_token)
                .json(&json!({}))
                .send()
                .await;
        }
    }

    pub fn update(&self, np: Option<NowPlaying>) {
        let _ = self.now_playing.send(np);
    }

    pub async fn is_logged_in(&self) -> bool {
        self.tokens.lock().await.is_some()
    }

    async fn store_tokens(&self, resp: TokenResponse) {
        let expires_at = now_secs() + resp.expires_in;
        *self.tokens.lock().await = Some(StoredTokens {
            access_token: resp.access_token,
            refresh_token: resp.refresh_token,
            expires_at,
        });
        // Persist to secure storage from the Tauri command layer, not here --
        // this module stays storage-agnostic.
    }

    async fn access_token(&self) -> anyhow::Result<String> {
        let mut guard = self.tokens.lock().await;
        let tokens = guard.as_mut().ok_or_else(|| anyhow::anyhow!("not logged in"))?;

        if now_secs() < tokens.expires_at - 60 {
            return Ok(tokens.access_token.clone());
        }

        // Refresh.
        let client = reqwest::Client::new();
        let resp: TokenResponse = client
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", tokens.refresh_token.as_str()),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        tokens.access_token = resp.access_token.clone();
        tokens.refresh_token = resp.refresh_token.clone();
        tokens.expires_at = now_secs() + resp.expires_in;
        Ok(resp.access_token)
    }

    fn start_keepalive(&self) {
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        {
            let this = self.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    *this.shutdown.lock().await = Some(shutdown_tx);
                });
            });
        }

        let this = self.clone();
        let mut now_playing_rx = self.now_playing.subscribe();
        tokio::spawn(async move {
            let mut session_token: Option<String> = None;
            let mut ticker = tokio::time::interval(SESSION_REFRESH_INTERVAL);

            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let np = now_playing_rx.borrow().clone();
                        if let Err(e) = this.push_session(&mut session_token, np.as_ref()).await {
                            eprintln!("[oauth_presence] keepalive failed: {e:?}");
                        }
                    }
                    changed = now_playing_rx.changed() => {
                        if changed.is_err() { return; }
                        let np = now_playing_rx.borrow().clone();
                        if let Err(e) = this.push_session(&mut session_token, np.as_ref()).await {
                            eprintln!("[oauth_presence] update failed: {e:?}");
                        }
                    }
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() { return; }
                        if *shutdown_rx.borrow() { return; }
                    }
                }
            }
        });
    }

    async fn push_session(
        &self,
        session_token: &mut Option<String>,
        np: Option<&NowPlaying>,
    ) -> anyhow::Result<()> {
        let Some(np) = np else { return Ok(()) };
        let access_token = self.access_token().await?;

        let mut body = json!({
            "activities": [{
                "type": 2, // LISTENING
                "name": "KHInsider",
                "application_id": self.application_id,
                "platform": "ios", // or "android" -- set per build target
                "details": np.game,
                "state": np.platform,
                "timestamps": { "start": np.started_at }
            }]
        });
        if let Some(tok) = session_token.as_ref() {
            body["token"] = json!(tok);
        }

        let client = reqwest::Client::new();
        let resp: HeadlessSessionResponse = client
            .post(format!("{API_BASE}/users/@me/headless-sessions"))
            .bearer_auth(&access_token)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        *session_token = Some(resp.token);
        Ok(())
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

#[derive(Deserialize)]
struct HeadlessSessionResponse {
    token: String,
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn generate_code_verifier() -> String {
    use rand::Rng;
    let bytes: [u8; 32] = rand::thread_rng().gen();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash)
}
