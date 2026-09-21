//! Mobile Rich Presence via the Discord Gateway.
//!
//! ============================================================================
//! READ THIS FIRST
//! ============================================================================
//! This talks to Discord as a full client session using a *user* token, because
//! Discord's mobile apps expose no local IPC socket for third-party presence
//! (unlike desktop, where discord-rpc talks to the socket the desktop client
//! opens). There is no Discord-sanctioned way to set a user's presence from a
//! third-party mobile app. This is the same technique used by community tools
//! like Kizzy and MRPC.
//!
//! Consequences the user must understand before enabling this:
//!   - Automating a user account this way is self-botting, which is against
//!     Discord's Terms of Service. Enforcement is inconsistent, but accounts
//!     have been actioned for it.
//!   - A user token is equivalent to a password: anyone who gets it controls
//!     the account entirely (DMs, servers, payment methods, everything) --
//!     not just presence. Getting the token means pulling it from Discord's
//!     own web client in browser devtools; that is on the user, this module
//!     never asks for or transmits credentials, only the token string once
//!     given to it.
//!   - This module must never be enabled by default, never bundled with a
//!     token, and the token must only ever live in OS-level secure storage
//!     (Keychain on iOS, EncryptedSharedPreferences/Keystore on Android),
//!     never in a config file, log, or plaintext preference.
//!
//! This module does the minimum needed for presence: IDENTIFY, heartbeat,
//! PRESENCE_UPDATE. It does not read messages, join servers, or do anything
//! else a full client can do, but the token it holds *can* do all of that,
//! so treat it accordingly.
//! ============================================================================

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Mutex};
use tokio::time::interval;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::{SinkExt, StreamExt};

const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NowPlaying {
    pub track: String,
    pub game: String,      // e.g. album / soundtrack name -> shown as "details"
    pub platform: String,  // e.g. "SNES" -> shown as "state"
    pub started_at: i64,   // unix ms, for the elapsed timer
}

#[derive(Clone)]
pub struct GatewayPresence {
    token: Arc<Mutex<Option<String>>>,
    now_playing: watch::Sender<Option<NowPlaying>>,
    shutdown: Arc<Mutex<Option<watch::Sender<bool>>>>,
}

impl GatewayPresence {
    pub fn new() -> (Self, watch::Receiver<Option<NowPlaying>>) {
        let (tx, rx) = watch::channel(None);
        (
            Self {
                token: Arc::new(Mutex::new(None)),
                now_playing: tx,
                shutdown: Arc::new(Mutex::new(None)),
            },
            rx,
        )
    }

    /// Called once the user has pasted their token into the (clearly-labeled,
    /// clearly-warned) settings screen and confirmed the ToS risk dialog.
    /// `token` should come straight from secure storage, never from a plain
    /// preference file.
    pub async fn start(&self, token: String) -> anyhow::Result<()> {
        *self.token.lock().await = Some(token.clone());

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        *self.shutdown.lock().await = Some(shutdown_tx);

        let now_playing_rx = self.now_playing.subscribe();
        tokio::spawn(async move {
            loop {
                if *shutdown_rx.borrow() {
                    return;
                }
                if let Err(e) =
                    run_session(&token, now_playing_rx.clone(), &mut shutdown_rx).await
                {
                    eprintln!("[gateway_presence] session ended: {e:?}, reconnecting in 5s");
                }
                if *shutdown_rx.borrow() {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
        Ok(())
    }

    pub async fn stop(&self) {
        if let Some(tx) = self.shutdown.lock().await.take() {
            let _ = tx.send(true);
        }
        *self.token.lock().await = None;
    }

    /// Call this whenever the WebView reports a new track. Feeds both the
    /// desktop discord-rpc path and this one from the same event.
    pub fn update(&self, np: Option<NowPlaying>) {
        let _ = self.now_playing.send(np);
    }
}

async fn run_session(
    token: &str,
    mut now_playing_rx: watch::Receiver<Option<NowPlaying>>,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let (ws_stream, _) = connect_async(GATEWAY_URL).await?;
    let (mut write, mut read) = ws_stream.split();

    // First frame is always OP 10 Hello, carrying heartbeat_interval.
    let hello = next_json(&mut read).await?;
    let heartbeat_ms = hello["d"]["heartbeat_interval"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("no heartbeat_interval in Hello"))?;

    // IDENTIFY. This is the step that requires a *user* token -- a bot token
    // cannot set a user's own presence, only the bot's own.
    let identify = json!({
        "op": 2,
        "d": {
            "token": token,
            "capabilities": 30717,
            "properties": {
                "os": "iOS",
                "browser": "Discord iOS",
                "device": "iOS"
            },
            "presence": { "status": "online", "afk": false }
        }
    });
    write.send(Message::text(identify.to_string())).await?;

    // Wait for READY (or an auth failure) before doing anything else.
    loop {
        let msg = next_json(&mut read).await?;
        match msg["t"].as_str() {
            Some("READY") => break,
            _ if msg["op"] == 9 => {
                anyhow::bail!("gateway rejected the session (invalid or revoked token)")
            }
            _ => continue,
        }
    }

    let mut heartbeat = interval(Duration::from_millis(heartbeat_ms));
    heartbeat.tick().await; // first tick fires immediately, discard it

    let mut last_sent: Option<NowPlaying> = None;

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                write.send(Message::text(json!({"op": 1, "d": Value::Null}).to_string())).await?;
            }
            changed = now_playing_rx.changed() => {
                changed?;
                let np = now_playing_rx.borrow().clone();
                if np.as_ref().map(|n| &n.track) != last_sent.as_ref().map(|n| &n.track) {
                    send_presence(&mut write, np.as_ref()).await?;
                    last_sent = np;
                }
            }
            changed = shutdown_rx.changed() => {
                changed?;
                if *shutdown_rx.borrow() {
                    // Clear presence on the way out so it doesn't stick as "still playing".
                    send_presence(&mut write, None).await?;
                    return Ok(());
                }
            }
            frame = read.next() => {
                match frame {
                    Some(Ok(Message::Text(_))) => { /* ignore other gateway events */ }
                    Some(Ok(Message::Close(_))) | None => anyhow::bail!("gateway closed the connection"),
                    Some(Err(e)) => return Err(e.into()),
                    _ => {}
                }
            }
        }
    }
}

async fn send_presence<S>(
    write: &mut futures_util::stream::SplitSink<S, Message>,
    np: Option<&NowPlaying>,
) -> anyhow::Result<()>
where
    S: futures_util::Sink<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::error::Error + Send + Sync + 'static,
{
    let activities = match np {
        Some(n) => vec![json!({
            "name": "KHInsider",
            "type": 2, // Listening
            "details": n.game,
            "state": n.platform,
            "timestamps": { "start": n.started_at }
        })],
        None => vec![],
    };
    let payload = json!({
        "op": 3,
        "d": {
            "since": Value::Null,
            "activities": activities,
            "status": "online",
            "afk": false
        }
    });
    write.send(Message::text(payload.to_string())).await?;
    Ok(())
}

async fn next_json<S>(
    read: &mut futures_util::stream::SplitStream<S>,
) -> anyhow::Result<Value>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    loop {
        match read.next().await {
            Some(Ok(Message::Text(t))) => return Ok(serde_json::from_str(&t)?),
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.into()),
            None => anyhow::bail!("gateway closed before expected frame"),
        }
    }
}
