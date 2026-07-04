use crate::message::{UserInfo, WsMessage, PROTOCOL_VERSION};
use crate::video::AsciiFrame;
use crate::webcam::{ascii_frame_to_ws, ws_frame_to_ascii, WebcamCapture, WebcamConfig};
use anyhow::{anyhow, Result};
use futures::{SinkExt, StreamExt};
use parking_lot::{Mutex as PlMutex, RwLock};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message as TungsteniteMsg};

/// how often the client pings the server to keep the link warm
const PING_INTERVAL: Duration = Duration::from_secs(20);
/// no inbound traffic for this long -> the link is declared dead
const SILENCE_TIMEOUT: Duration = Duration::from_secs(60);
/// safety cap so an idle app never accumulates unbounded game messages
const GAME_INBOX_CAP: usize = 512;
/// a v3 server sends Welcome immediately after Join; if none arrives within
/// this window the server is a pre-v3 build that would otherwise leave us
/// half-connected forever (no my_id, games silently dead -- finding #15)
const WELCOME_TIMEOUT: Duration = Duration::from_secs(5);

/// A game-scoped message received from another participant
/// (identity fields are server-authoritative -- they cannot be spoofed).
/// Consumed by the games glue in App::tick -> games.handle_net.
#[derive(Debug, Clone)]
pub struct GameNetMsg {
    pub from_id: String,
    pub from_name: String,
    pub game: String,
    pub payload: Value,
}

/// Connection lifecycle events surfaced to the UI. main.rs installs a hook
/// that adapts these into AppEvents (never eprintln!).
#[derive(Debug, Clone)]
pub enum NetEvent {
    Connected { user_id: String },
    Disconnected { reason: String },
    UserJoined { username: String },
    /// `user_id` is the server-assigned id -- the games glue needs it to end
    /// an online match when its locked opponent leaves (finding #4).
    UserLeft { user_id: String, username: String },
}

enum Outgoing {
    Chat(String),
    Game(String, Value),
    Frame(AsciiFrame),
}

/// Resolve once the shutdown flag flips to true (Send-friendly wrapper:
/// `watch::Receiver::wait_for`'s output guard is not Send inside select!).
async fn wait_shutdown(shutdown: &mut watch::Receiver<bool>) {
    let _ = shutdown.wait_for(|stop| *stop).await;
}

pub struct VideoChatClient {
    pub username: String,
    pub server_url: String,
    /// roster as reported by the server (user_id + username + connected_at)
    pub connected_users: Arc<RwLock<Vec<UserInfo>>>,
    /// remote video feeds keyed by user_id -> (username, frame)
    pub remote_frames: Arc<RwLock<HashMap<String, (String, AsciiFrame)>>>,
    pub local_frame: Arc<RwLock<Option<AsciiFrame>>>,
    /// (username, content) pairs; own messages are appended locally on send,
    /// exactly once (the server does not echo back to the sender)
    pub chat_messages: Arc<RwLock<Vec<(String, String)>>>,
    pub connected: Arc<RwLock<bool>>,
    pub status: Arc<RwLock<String>>,
    /// inbound Game messages; the app drains these via drain_game_inbox()
    pub game_inbox: Arc<PlMutex<VecDeque<GameNetMsg>>>,
    my_id: Arc<RwLock<Option<String>>>,
    out_tx: mpsc::UnboundedSender<Outgoing>,
    /// taken by connect(); Some means connect() has not run yet
    out_rx: PlMutex<Option<mpsc::UnboundedReceiver<Outgoing>>>,
    shutdown_tx: watch::Sender<bool>,
    webcam_enabled: AtomicBool,
    event_hook: RwLock<Option<Arc<dyn Fn(NetEvent) + Send + Sync>>>,
    last_traffic: Arc<RwLock<Instant>>,
    disconnect_reason: Arc<RwLock<Option<String>>>,
    /// how long to wait for the server's Welcome (tunable only in tests)
    welcome_timeout: RwLock<Duration>,
}

impl VideoChatClient {
    pub fn new(username: String, server_url: String) -> Self {
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, _) = watch::channel(false);
        Self {
            username,
            server_url,
            connected_users: Arc::new(RwLock::new(Vec::new())),
            remote_frames: Arc::new(RwLock::new(HashMap::new())),
            local_frame: Arc::new(RwLock::new(None)),
            chat_messages: Arc::new(RwLock::new(Vec::new())),
            connected: Arc::new(RwLock::new(false)),
            status: Arc::new(RwLock::new("disconnected".to_string())),
            game_inbox: Arc::new(PlMutex::new(VecDeque::new())),
            my_id: Arc::new(RwLock::new(None)),
            out_tx,
            out_rx: PlMutex::new(Some(out_rx)),
            shutdown_tx,
            webcam_enabled: AtomicBool::new(true),
            event_hook: RwLock::new(None),
            last_traffic: Arc::new(RwLock::new(Instant::now())),
            disconnect_reason: Arc::new(RwLock::new(None)),
            welcome_timeout: RwLock::new(WELCOME_TIMEOUT),
        }
    }

    /// Shrink the Welcome watchdog window so tests don't wait 5 real seconds.
    #[cfg(test)]
    fn set_welcome_timeout(&self, timeout: Duration) {
        *self.welcome_timeout.write() = timeout;
    }

    /// Install the UI event hook (call before connect). The hook must be
    /// cheap and non-blocking -- main.rs sends an AppEvent over an
    /// unbounded channel.
    pub fn set_event_hook(&self, hook: impl Fn(NetEvent) + Send + Sync + 'static) {
        *self.event_hook.write() = Some(Arc::new(hook));
    }

    /// Disable the client-owned webcam capture (tests, headless use).
    #[allow(dead_code)]
    pub fn set_webcam_enabled(&self, enabled: bool) {
        self.webcam_enabled.store(enabled, Ordering::Relaxed);
    }

    /// Queue a chat line and append it to the local transcript exactly once.
    pub fn send_chat(&self, content: String) {
        {
            let mut msgs = self.chat_messages.write();
            msgs.push((self.username.clone(), content.clone()));
            if msgs.len() > 200 {
                msgs.drain(0..50);
            }
        }
        let _ = self.out_tx.send(Outgoing::Chat(content));
    }

    /// Queue a game payload for relay to all other participants.
    pub fn send_game(&self, game: &str, payload: Value) {
        let _ = self.out_tx.send(Outgoing::Game(game.to_string(), payload));
    }

    /// Queue an ASCII frame as this client's video feed (also updates the
    /// local preview). The webcam pump uses this same path.
    pub fn send_frame(&self, frame: AsciiFrame) {
        *self.local_frame.write() = Some(frame.clone());
        let _ = self.out_tx.send(Outgoing::Frame(frame));
    }

    /// Drain all pending inbound game messages (called from App::tick).
    pub fn drain_game_inbox(&self) -> Vec<GameNetMsg> {
        self.game_inbox.lock().drain(..).collect()
    }

    /// The server-assigned id from Welcome; None until joined.
    pub fn my_id(&self) -> Option<String> {
        self.my_id.read().clone()
    }

    pub fn is_connected(&self) -> bool {
        *self.connected.read()
    }

    pub fn get_status(&self) -> String {
        self.status.read().clone()
    }

    /// Signal every task to stop and mark the client offline. Idempotent.
    pub fn disconnect(&self) {
        {
            let mut reason = self.disconnect_reason.write();
            if reason.is_none() {
                *reason = Some("disconnected".to_string());
            }
        }
        *self.connected.write() = false;
        *self.status.write() = "disconnected".to_string();
        let _ = self.shutdown_tx.send(true);
    }

    fn fire(&self, event: NetEvent) {
        let hook = self.event_hook.read().clone();
        if let Some(hook) = hook {
            hook(event);
        }
    }

    /// Dial the server and spawn the reader / outbox / webcam / keepalive
    /// tasks -- all operating on THIS instance's shared state, so the App's
    /// stored `Arc<VideoChatClient>` is the one that is actually live.
    pub async fn connect(self: Arc<Self>) -> Result<()> {
        let out_rx = self
            .out_rx
            .lock()
            .take()
            .ok_or_else(|| anyhow!("client already connected; create a new client"))?;

        *self.status.write() = format!("connecting to {}", self.server_url);

        let (ws_stream, _) = match connect_async(&self.server_url).await {
            Ok(ok) => ok,
            Err(e) => {
                *self.out_rx.lock() = Some(out_rx); // allow a retry on this instance
                *self.status.write() = format!("connection failed: {}", e);
                return Err(e.into());
            }
        };
        let (ws_tx, mut ws_rx) = ws_stream.split();
        let ws_tx = Arc::new(Mutex::new(ws_tx));

        *self.connected.write() = true;
        *self.status.write() = "connected, joining...".to_string();
        *self.last_traffic.write() = Instant::now();

        let join_msg = WsMessage::Join {
            username: self.username.clone(),
            version: PROTOCOL_VERSION,
        };
        {
            let mut tx = ws_tx.lock().await;
            if let Err(e) = tx
                .send(TungsteniteMsg::Text(serde_json::to_string(&join_msg)?))
                .await
            {
                drop(tx);
                *self.out_rx.lock() = Some(out_rx);
                *self.connected.write() = false;
                *self.status.write() = format!("connection failed: {}", e);
                return Err(e.into());
            }
        }

        let webcam = if self.webcam_enabled.load(Ordering::Relaxed) {
            WebcamCapture::start(WebcamConfig::default()).ok()
        } else {
            None
        };

        // reader: demux inbound messages into shared state, then tear down
        {
            let me = Arc::clone(&self);
            let ws_tx_reader = Arc::clone(&ws_tx);
            let mut shutdown = self.shutdown_tx.subscribe();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = wait_shutdown(&mut shutdown) => break,
                        msg = ws_rx.next() => {
                            match msg {
                                Some(Ok(TungsteniteMsg::Text(text))) => {
                                    *me.last_traffic.write() = Instant::now();
                                    if let Ok(parsed) = serde_json::from_str::<WsMessage>(&text) {
                                        me.handle_inbound(parsed);
                                    }
                                }
                                Some(Ok(TungsteniteMsg::Ping(_)))
                                | Some(Ok(TungsteniteMsg::Pong(_))) => {
                                    *me.last_traffic.write() = Instant::now();
                                }
                                Some(Ok(TungsteniteMsg::Close(_))) | Some(Err(_)) | None => break,
                                Some(Ok(_)) => {}
                            }
                        }
                    }
                }
                // tear down: stop siblings, flip state, close the socket
                let _ = me.shutdown_tx.send(true);
                let was_connected = {
                    let mut connected = me.connected.write();
                    std::mem::replace(&mut *connected, false)
                };
                let reason = me
                    .disconnect_reason
                    .write()
                    .take()
                    .unwrap_or_else(|| "connection closed".to_string());
                *me.status.write() = reason.clone();
                {
                    let mut tx = ws_tx_reader.lock().await;
                    let _ = tx.send(TungsteniteMsg::Close(None)).await;
                    let _ = tx.close().await;
                }
                if was_connected {
                    me.fire(NetEvent::Disconnected { reason });
                }
            });
        }

        // outbox pump: single write path for chat / game / frame traffic
        {
            let me = Arc::clone(&self);
            let ws_tx_out = Arc::clone(&ws_tx);
            let mut shutdown = self.shutdown_tx.subscribe();
            let mut out_rx = out_rx;
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = wait_shutdown(&mut shutdown) => break,
                        item = out_rx.recv() => {
                            let Some(item) = item else { break };
                            let my_id = me.my_id().unwrap_or_default();
                            let msg = match item {
                                Outgoing::Chat(content) => WsMessage::Chat {
                                    user_id: my_id,
                                    username: me.username.clone(),
                                    content,
                                },
                                Outgoing::Game(game, payload) => WsMessage::Game {
                                    user_id: my_id,
                                    username: me.username.clone(),
                                    game,
                                    payload,
                                },
                                Outgoing::Frame(frame) => WsMessage::Frame {
                                    user_id: my_id,
                                    username: me.username.clone(),
                                    frame: ascii_frame_to_ws(&frame),
                                },
                            };
                            if let Ok(json) = serde_json::to_string(&msg) {
                                let mut tx = ws_tx_out.lock().await;
                                if tx.send(TungsteniteMsg::Text(json)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            });
        }

        // webcam pump: feed captured frames through the outbox path
        if let Some(cam) = webcam {
            let me = Arc::clone(&self);
            let mut shutdown = self.shutdown_tx.subscribe();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = wait_shutdown(&mut shutdown) => break,
                        _ = tokio::time::sleep(Duration::from_millis(33)) => {
                            if let Some(frame) = cam.try_recv() {
                                me.send_frame(frame);
                            }
                        }
                    }
                }
                // cam drops here, stopping its capture thread
            });
        }

        // keepalive: ping every 20s; declare the link dead after 60s silence
        {
            let me = Arc::clone(&self);
            let ws_tx_ping = Arc::clone(&ws_tx);
            let mut shutdown = self.shutdown_tx.subscribe();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = wait_shutdown(&mut shutdown) => break,
                        _ = tokio::time::sleep(PING_INTERVAL) => {
                            let silent = me.last_traffic.read().elapsed();
                            if silent > SILENCE_TIMEOUT {
                                {
                                    let mut reason = me.disconnect_reason.write();
                                    if reason.is_none() {
                                        *reason = Some(
                                            "connection lost (no traffic for 60s)".to_string(),
                                        );
                                    }
                                }
                                let _ = me.shutdown_tx.send(true);
                                break;
                            }
                            if let Ok(json) = serde_json::to_string(&WsMessage::Ping) {
                                let mut tx = ws_tx_ping.lock().await;
                                if tx.send(TungsteniteMsg::Text(json)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            });
        }

        // welcome watchdog (finding #15): a pre-v3 server accepts the socket
        // and the Join (serde ignores the extra `version` field) but never
        // sends Welcome, leaving this client half-connected forever -- my_id
        // stays None and online games silently never start. Detect the
        // silence and tear the link down with an explanation instead.
        {
            let me = Arc::clone(&self);
            let mut shutdown = self.shutdown_tx.subscribe();
            let timeout = *self.welcome_timeout.read();
            tokio::spawn(async move {
                tokio::select! {
                    _ = wait_shutdown(&mut shutdown) => {}
                    _ = tokio::time::sleep(timeout) => {
                        if me.my_id().is_none() {
                            {
                                let mut reason = me.disconnect_reason.write();
                                if reason.is_none() {
                                    *reason = Some(
                                        "server incompatible (no welcome) -- both sides need asciivision v3"
                                            .to_string(),
                                    );
                                }
                            }
                            let _ = me.shutdown_tx.send(true);
                        }
                    }
                }
            });
        }

        Ok(())
    }

    /// Apply one inbound message to shared state. Fully synchronous: no lock
    /// is ever held across an await, and no lock is held while firing the
    /// UI hook.
    fn handle_inbound(&self, msg: WsMessage) {
        match msg {
            WsMessage::Welcome { user_id } => {
                *self.my_id.write() = Some(user_id.clone());
                *self.status.write() = format!("connected as {}", self.username);
                self.fire(NetEvent::Connected { user_id });
            }
            WsMessage::Ack { success, message } => {
                *self.status.write() = message.clone();
                if !success {
                    // e.g. version mismatch; the server closes right after,
                    // so remember why for the Disconnected event
                    let mut reason = self.disconnect_reason.write();
                    if reason.is_none() {
                        *reason = Some(message);
                    }
                }
            }
            WsMessage::UserList(users) => {
                let count = users.len();
                *self.connected_users.write() = users;
                if self.my_id.read().is_some() {
                    *self.status.write() =
                        format!("{} user{} online", count, if count == 1 { "" } else { "s" });
                }
            }
            WsMessage::Frame {
                user_id,
                username,
                frame,
            } => {
                // malformed frames (hostile dims / mismatched buffer) decode
                // to None and are dropped -- never trusted with an allocation
                if let Some(ascii) = ws_frame_to_ascii(&frame) {
                    self.remote_frames.write().insert(user_id, (username, ascii));
                }
            }
            WsMessage::Chat {
                username, content, ..
            } => {
                let mut msgs = self.chat_messages.write();
                msgs.push((username, content));
                if msgs.len() > 200 {
                    msgs.drain(0..50);
                }
            }
            WsMessage::Game {
                user_id,
                username,
                game,
                payload,
            } => {
                let mut inbox = self.game_inbox.lock();
                inbox.push_back(GameNetMsg {
                    from_id: user_id,
                    from_name: username,
                    game,
                    payload,
                });
                if inbox.len() > GAME_INBOX_CAP {
                    let overflow = inbox.len() - GAME_INBOX_CAP;
                    inbox.drain(0..overflow);
                }
            }
            WsMessage::UserJoined { username, .. } => {
                self.chat_messages
                    .write()
                    .push(("SYSTEM".to_string(), format!("{} joined", username)));
                self.fire(NetEvent::UserJoined { username });
            }
            WsMessage::UserLeft { user_id, username } => {
                self.remote_frames.write().remove(&user_id);
                self.chat_messages
                    .write()
                    .push(("SYSTEM".to_string(), format!("{} left", username)));
                self.fire(NetEvent::UserLeft { user_id, username });
            }
            WsMessage::Ping | WsMessage::Pong | WsMessage::Join { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::VideoChatServer;

    async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
        for _ in 0..250 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for: {}", what);
    }

    fn spawn_server() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Arc::new(VideoChatServer::new());
        tokio::spawn(server.run_std(listener));
        format!("ws://127.0.0.1:{}", port)
    }

    fn test_client(name: &str, url: &str) -> Arc<VideoChatClient> {
        let client = Arc::new(VideoChatClient::new(name.to_string(), url.to_string()));
        client.set_webcam_enabled(false); // no hardware in tests
        client
    }

    /// The core end-to-end contract: two real clients against a real server.
    #[tokio::test]
    async fn two_client_full_flow() {
        let url = spawn_server();

        let a = test_client("alice", &url);
        let b = test_client("bob", &url);

        let a_events: Arc<PlMutex<Vec<NetEvent>>> = Arc::new(PlMutex::new(Vec::new()));
        let b_events: Arc<PlMutex<Vec<NetEvent>>> = Arc::new(PlMutex::new(Vec::new()));
        {
            let sink = Arc::clone(&a_events);
            a.set_event_hook(move |ev| sink.lock().push(ev));
            let sink = Arc::clone(&b_events);
            b.set_event_hook(move |ev| sink.lock().push(ev));
        }

        Arc::clone(&a).connect().await.expect("alice connects");
        Arc::clone(&b).connect().await.expect("bob connects");

        // --- both get Welcome with distinct user_ids ---
        wait_for("both welcomes", || a.my_id().is_some() && b.my_id().is_some()).await;
        let a_id = a.my_id().unwrap();
        let b_id = b.my_id().unwrap();
        assert_ne!(a_id, b_id, "server must mint distinct ids");
        assert!(a.is_connected() && b.is_connected());
        assert!(a_events
            .lock()
            .iter()
            .any(|e| matches!(e, NetEvent::Connected { user_id } if *user_id == a_id)));

        // roster reaches both, keyed by user_id
        wait_for("rosters", || {
            a.connected_users.read().len() == 2 && b.connected_users.read().len() == 2
        })
        .await;
        assert!(a_events
            .lock()
            .iter()
            .any(|e| matches!(e, NetEvent::UserJoined { username } if username == "bob")));

        // --- chat from A appears at B, and exactly once at A (no echo) ---
        a.send_chat("hello bob".to_string());
        wait_for("chat delivery", || {
            b.chat_messages
                .read()
                .iter()
                .any(|(user, content)| user == "alice" && content == "hello bob")
        })
        .await;
        tokio::time::sleep(Duration::from_millis(250)).await; // window for a wrong echo
        let a_copies = a
            .chat_messages
            .read()
            .iter()
            .filter(|(_, content)| content == "hello bob")
            .count();
        assert_eq!(a_copies, 1, "own chat must appear exactly once (local append, no echo)");

        // --- game payload from A arrives at B with rewritten identity, not echoed to A ---
        a.send_game("pong", serde_json::json!({"t": "input", "dir": -1, "spoof": true}));
        wait_for("game delivery", || !b.game_inbox.lock().is_empty()).await;
        let inbox = b.drain_game_inbox();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].from_id, a_id, "server must rewrite sender identity");
        assert_eq!(inbox[0].from_name, "alice");
        assert_eq!(inbox[0].game, "pong");
        assert_eq!(inbox[0].payload["dir"], -1);
        assert!(b.game_inbox.lock().is_empty(), "drain must empty the inbox");
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            a.drain_game_inbox().is_empty(),
            "game messages must not be echoed to the sender"
        );

        // --- frames from A reach B, keyed by user_id, multi-byte glyph intact ---
        a.send_frame(AsciiFrame {
            width: 2,
            height: 1,
            cells: vec![('▀', 255, 0, 0), ('X', 0, 255, 0)],
        });
        wait_for("frame delivery", || {
            b.remote_frames.read().contains_key(&a_id)
        })
        .await;
        {
            let frames = b.remote_frames.read();
            let (uname, frame) = frames.get(&a_id).unwrap();
            assert_eq!(uname, "alice");
            assert_eq!(frame.cells[0], ('▀', 255, 0, 0), "glyph must survive the wire");
            assert_eq!(frame.cells[1], ('X', 0, 255, 0));
        }
        assert!(
            !a.remote_frames.read().contains_key(&a_id),
            "own frames must not be echoed back"
        );

        // --- disconnect() ends A's tasks and B observes UserLeft ---
        a.disconnect();
        assert!(!a.is_connected());
        wait_for("bob sees alice leave", || {
            b.chat_messages
                .read()
                .iter()
                .any(|(user, content)| user == "SYSTEM" && content == "alice left")
        })
        .await;
        wait_for("frame cleanup by user_id", || {
            !b.remote_frames.read().contains_key(&a_id)
        })
        .await;
        wait_for("roster shrinks", || b.connected_users.read().len() == 1).await;
        assert!(b_events
            .lock()
            .iter()
            .any(|e| matches!(e, NetEvent::UserLeft { user_id, username }
                if username == "alice" && *user_id == a_id)));

        // A must not be able to silently reuse the dead pipeline
        let reuse = Arc::clone(&a).connect().await;
        assert!(reuse.is_err(), "a used client cannot connect() again");

        b.disconnect();
        wait_for("bob offline", || !b.is_connected()).await;
    }

    /// Finding #15: a v1 server accepts the socket and the Join but never
    /// sends Welcome. The client must not sit half-connected forever -- the
    /// welcome watchdog disconnects with an actionable status.
    #[tokio::test]
    async fn v1_server_silence_triggers_incompatibility_disconnect() {
        // v1 server sim: complete the websocket handshake, swallow every
        // inbound message, never reply with anything.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    if let Ok(ws) = tokio_tungstenite::accept_async(stream).await {
                        let (_tx, mut rx) = ws.split();
                        while let Some(Ok(_)) = rx.next().await {} // accept + silence
                    }
                });
            }
        });

        let client = test_client("hopeful", &format!("ws://127.0.0.1:{}", port));
        client.set_welcome_timeout(Duration::from_millis(300));
        let events: Arc<PlMutex<Vec<NetEvent>>> = Arc::new(PlMutex::new(Vec::new()));
        {
            let sink = Arc::clone(&events);
            client.set_event_hook(move |ev| sink.lock().push(ev));
        }

        Arc::clone(&client).connect().await.expect("socket opens fine");
        assert!(client.is_connected(), "pre-timeout the link looks up");

        wait_for("incompatibility disconnect", || !client.is_connected()).await;
        assert!(client.my_id().is_none(), "no Welcome must mean no id");
        assert!(
            client.get_status().contains("server incompatible"),
            "status was: {}",
            client.get_status()
        );
        wait_for("disconnected event fires", || {
            events.lock().iter().any(|e| {
                matches!(e, NetEvent::Disconnected { reason } if reason.contains("server incompatible"))
            })
        })
        .await;
    }

    /// The watchdog must NOT fire against a real v3 server: Welcome arrives
    /// well inside the window and the session keeps running.
    #[tokio::test]
    async fn welcome_watchdog_is_inert_on_a_v3_server() {
        let url = spawn_server();
        let client = test_client("patient", &url);
        client.set_welcome_timeout(Duration::from_millis(200));
        Arc::clone(&client).connect().await.expect("connects");
        wait_for("welcome", || client.my_id().is_some()).await;
        // outlive the (shortened) watchdog window, then verify liveness
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(client.is_connected(), "watchdog must not kill a good link");
        client.disconnect();
    }

    /// connect() failures leave the client reusable (needed by /host's retry).
    #[tokio::test]
    async fn failed_dial_can_be_retried_on_same_instance() {
        // nothing is listening here
        let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = dead.local_addr().unwrap().port();
        drop(dead);

        let client = test_client("solo", &format!("ws://127.0.0.1:{}", port));
        assert!(Arc::clone(&client).connect().await.is_err());
        assert!(!client.is_connected());
        assert!(client.get_status().contains("connection failed"));

        // second attempt against a live server on that very port succeeds
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        let server = Arc::new(VideoChatServer::new());
        tokio::spawn(server.run_std(listener));
        Arc::clone(&client).connect().await.expect("retry connects");
        wait_for("welcome", || client.my_id().is_some()).await;
        client.disconnect();
    }
}
