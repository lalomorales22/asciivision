use crate::message::{UserInfo, WsMessage, PROTOCOL_VERSION};
use anyhow::Result;
use futures::{SinkExt, StreamExt};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::Message as TungsteniteMsg;

/// A slow client's outbound queue keeps at most this many video frames in
/// flight; older frames are dropped for that connection (chat/control
/// messages are never dropped -- but see MAX_PENDING_CONTROL).
const MAX_PENDING_FRAMES: usize = 8;
/// A connection whose control queue (chat/roster/game) backs up past this
/// many messages has stopped draining; it is killed rather than letting the
/// queue grow without bound (finding #16).
const MAX_PENDING_CONTROL: usize = 1024;
/// WebSocket-level ping cadence per connection (finding #16). A live client
/// auto-pongs, producing the inbound traffic the silence check looks for.
const PING_INTERVAL: Duration = Duration::from_secs(20);
/// No inbound traffic for this long -> the peer is presumed dead (slept
/// laptop, dropped WiFi -- no TCP FIN ever arrives) and is cleaned up exactly
/// like a disconnect. Mirrors the client-side 60s silence detector.
const SILENCE_TIMEOUT: Duration = Duration::from_secs(60);

struct ConnHandle {
    tx: mpsc::UnboundedSender<WsMessage>,
    pending_frames: Arc<AtomicUsize>,
    pending_control: Arc<AtomicUsize>,
    /// wakes the owning session loop to tear the connection down (used by
    /// broadcast_except when the control queue overflows)
    kill: Arc<Notify>,
}

pub struct VideoChatServer {
    connections: Arc<RwLock<HashMap<String, ConnHandle>>>,
    users: Arc<RwLock<HashMap<String, UserInfo>>>,
    ping_interval: Duration,
    silence_timeout: Duration,
    max_pending_control: usize,
}

impl VideoChatServer {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            users: Arc::new(RwLock::new(HashMap::new())),
            ping_interval: PING_INTERVAL,
            silence_timeout: SILENCE_TIMEOUT,
            max_pending_control: MAX_PENDING_CONTROL,
        }
    }

    /// Test-only constructor with tunable liveness knobs so the timeout
    /// machinery can be exercised in milliseconds instead of minutes.
    #[cfg(test)]
    fn tuned(ping_interval: Duration, silence_timeout: Duration, max_pending_control: usize) -> Self {
        Self {
            ping_interval,
            silence_timeout,
            max_pending_control,
            ..Self::new()
        }
    }

    /// Serve on a pre-bound std listener. Binding synchronously lets callers
    /// report "port busy" immediately and guarantees the port is open before
    /// any self-connect attempt.
    pub async fn run_std(self: Arc<Self>, listener: std::net::TcpListener) -> Result<()> {
        listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(listener)?;
        self.serve(listener).await
    }

    async fn serve(self: Arc<Self>, listener: TcpListener) -> Result<()> {
        loop {
            let (stream, _) = listener.accept().await?;
            let server = Arc::clone(&self);
            tokio::spawn(async move {
                // per-connection errors are non-fatal; stderr is a black hole
                // in the TUI process, so they are intentionally not printed
                let _ = server.handle_connection(stream).await;
            });
        }
    }

    async fn handle_connection(&self, stream: TcpStream) -> Result<()> {
        let ws_stream = tokio_tungstenite::accept_async(stream).await?;
        let (mut ws_tx, mut ws_rx) = ws_stream.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<WsMessage>();
        let pending_frames = Arc::new(AtomicUsize::new(0));
        let pending_control = Arc::new(AtomicUsize::new(0));
        let kill = Arc::new(Notify::new());

        let mut user_id: Option<String> = None;
        let mut last_inbound = Instant::now();
        // Liveness timer (finding #16). A dedicated Interval, NOT a sleep
        // re-created per loop iteration: a ghost peer that still RECEIVES
        // broadcasts would reset such a sleep forever and dodge the check.
        let mut ping_timer = tokio::time::interval(self.ping_interval);
        ping_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        'session: loop {
            tokio::select! {
                msg = ws_rx.next() => {
                    let inbound = match msg {
                        Some(Ok(inbound)) => inbound,
                        Some(Err(_)) | None => break 'session,
                    };
                    last_inbound = Instant::now();
                    match inbound {
                        TungsteniteMsg::Text(text) => {
                            if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                                match ws_msg {
                                    WsMessage::Join { username, version } => {
                                        if version < PROTOCOL_VERSION {
                                            let ack = WsMessage::Ack {
                                                success: false,
                                                message: "version mismatch -- update asciivision to join this room".to_string(),
                                            };
                                            let _ = ws_tx
                                                .send(TungsteniteMsg::Text(serde_json::to_string(&ack)?))
                                                .await;
                                            break 'session;
                                        }
                                        if user_id.is_some() {
                                            // double Join on one socket: ignore
                                            continue;
                                        }
                                        let id = uuid::Uuid::new_v4().to_string();
                                        let info = UserInfo {
                                            user_id: id.clone(),
                                            username: username.clone(),
                                            connected_at: chrono::Utc::now().to_rfc3339(),
                                        };
                                        self.connections.write().insert(
                                            id.clone(),
                                            ConnHandle {
                                                tx: tx.clone(),
                                                pending_frames: Arc::clone(&pending_frames),
                                                pending_control: Arc::clone(&pending_control),
                                                kill: Arc::clone(&kill),
                                            },
                                        );
                                        self.users.write().insert(id.clone(), info);
                                        user_id = Some(id.clone());

                                        // Welcome first so the joiner learns its id
                                        // before any roster/frame traffic references it
                                        let welcome = WsMessage::Welcome { user_id: id.clone() };
                                        let _ = ws_tx
                                            .send(TungsteniteMsg::Text(serde_json::to_string(&welcome)?))
                                            .await;
                                        let ack = WsMessage::Ack {
                                            success: true,
                                            message: format!("welcome, {}!", username),
                                        };
                                        let _ = ws_tx
                                            .send(TungsteniteMsg::Text(serde_json::to_string(&ack)?))
                                            .await;
                                        self.broadcast_user_list();
                                        self.broadcast_except(
                                            &WsMessage::UserJoined { user_id: id.clone(), username },
                                            Some(&id),
                                        );
                                    }
                                    WsMessage::Frame { frame, .. } => {
                                        // finding #3: never relay a malformed frame
                                        // (hostile dimensions / mismatched buffer) --
                                        // a ~60-byte message must not become a giant
                                        // allocation on every peer in the room
                                        if let (Some(ref uid), true) =
                                            (&user_id, frame.is_well_formed())
                                        {
                                            let uname = self
                                                .users
                                                .read()
                                                .get(uid)
                                                .map(|u| u.username.clone())
                                                .unwrap_or_default();
                                            self.broadcast_except(
                                                &WsMessage::Frame {
                                                    user_id: uid.clone(),
                                                    username: uname,
                                                    frame,
                                                },
                                                Some(uid),
                                            );
                                        }
                                    }
                                    WsMessage::Chat { content, .. } => {
                                        if let Some(ref uid) = user_id {
                                            let uname = self
                                                .users
                                                .read()
                                                .get(uid)
                                                .map(|u| u.username.clone())
                                                .unwrap_or_default();
                                            self.broadcast_except(
                                                &WsMessage::Chat {
                                                    user_id: uid.clone(),
                                                    username: uname,
                                                    content,
                                                },
                                                Some(uid),
                                            );
                                        }
                                    }
                                    WsMessage::Game { game, payload, .. } => {
                                        // identity rewritten from the registry, like Chat,
                                        // so game messages cannot be spoofed
                                        if let Some(ref uid) = user_id {
                                            let uname = self
                                                .users
                                                .read()
                                                .get(uid)
                                                .map(|u| u.username.clone())
                                                .unwrap_or_default();
                                            self.broadcast_except(
                                                &WsMessage::Game {
                                                    user_id: uid.clone(),
                                                    username: uname,
                                                    game,
                                                    payload,
                                                },
                                                Some(uid),
                                            );
                                        }
                                    }
                                    WsMessage::Signal { to, payload, .. } => {
                                        // WebRTC signaling: authoritative sender,
                                        // delivered ONLY to the named target peer
                                        if let Some(ref uid) = user_id {
                                            self.send_to(
                                                &to,
                                                &WsMessage::Signal {
                                                    from: uid.clone(),
                                                    to: to.clone(),
                                                    payload,
                                                },
                                            );
                                        }
                                    }
                                    WsMessage::Ping => {
                                        let _ = ws_tx
                                            .send(TungsteniteMsg::Text(serde_json::to_string(&WsMessage::Pong)?))
                                            .await;
                                    }
                                    WsMessage::Pong => {}
                                    // server->client-only variants arriving inbound are protocol noise
                                    _ => {}
                                }
                            }
                        }
                        TungsteniteMsg::Close(_) => break 'session,
                        // ws-level ping/pong/binary count as traffic only
                        _ => {}
                    }
                }
                Some(msg) = rx.recv() => {
                    if matches!(msg, WsMessage::Frame { .. }) {
                        pending_frames.fetch_sub(1, Ordering::Relaxed);
                    } else {
                        pending_control.fetch_sub(1, Ordering::Relaxed);
                    }
                    if let Ok(json) = serde_json::to_string(&msg) {
                        // Race the write against the kill signal and a stall
                        // deadline: a peer with full TCP buffers must not be
                        // able to wedge this task in an unabortable send.
                        tokio::select! {
                            _ = kill.notified() => break 'session,
                            _ = tokio::time::sleep(self.silence_timeout) => break 'session,
                            sent = ws_tx.send(TungsteniteMsg::Text(json)) => {
                                if sent.is_err() {
                                    break 'session;
                                }
                            }
                        }
                    }
                }
                _ = kill.notified() => {
                    // broadcast_except killed this connection (control-queue
                    // overflow): clean up exactly like a disconnect
                    break 'session;
                }
                _ = ping_timer.tick() => {
                    if last_inbound.elapsed() > self.silence_timeout {
                        // half-open peer: no FIN will ever come, drop it now
                        break 'session;
                    }
                    tokio::select! {
                        _ = kill.notified() => break 'session,
                        _ = tokio::time::sleep(self.silence_timeout) => break 'session,
                        sent = ws_tx.send(TungsteniteMsg::Ping(Vec::new())) => {
                            if sent.is_err() {
                                break 'session;
                            }
                        }
                    }
                }
            }
        }

        if let Some(ref uid) = user_id {
            let uname = self
                .users
                .read()
                .get(uid)
                .map(|u| u.username.clone())
                .unwrap_or_default();
            self.connections.write().remove(uid);
            self.users.write().remove(uid);
            self.broadcast_all(&WsMessage::UserLeft {
                user_id: uid.clone(),
                username: uname,
            });
            self.broadcast_user_list();
        }

        Ok(())
    }

    fn broadcast_all(&self, msg: &WsMessage) {
        self.broadcast_except(msg, None);
    }

    /// Deliver a message to exactly ONE peer (WebRTC signaling). Treated like
    /// control traffic: never silently dropped, but a wedged consumer whose
    /// queue overflows is killed rather than growing it without bound.
    fn send_to(&self, target: &str, msg: &WsMessage) {
        let conns = self.connections.read();
        if let Some(handle) = conns.get(target) {
            if handle.pending_control.load(Ordering::Relaxed) >= self.max_pending_control {
                handle.kill.notify_one();
                return;
            }
            handle.pending_control.fetch_add(1, Ordering::Relaxed);
            if handle.tx.send(msg.clone()).is_err() {
                handle.pending_control.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    fn broadcast_except(&self, msg: &WsMessage, except: Option<&str>) {
        let is_frame = matches!(msg, WsMessage::Frame { .. });
        let conns = self.connections.read();
        for (id, handle) in conns.iter() {
            if except.map_or(false, |eid| id == eid) {
                continue;
            }
            if is_frame {
                // backpressure: drop stale frames for connections that can't
                // keep up instead of growing their queue without bound
                if handle.pending_frames.load(Ordering::Relaxed) >= MAX_PENDING_FRAMES {
                    continue;
                }
                // increment BEFORE enqueue so the consumer's decrement can
                // never observe the counter at zero and underflow
                handle.pending_frames.fetch_add(1, Ordering::Relaxed);
                if handle.tx.send(msg.clone()).is_err() {
                    handle.pending_frames.fetch_sub(1, Ordering::Relaxed);
                }
            } else {
                // control messages must not be silently dropped, but a queue
                // this deep means the consumer stopped draining entirely --
                // kill the connection, not the server (finding #16)
                if handle.pending_control.load(Ordering::Relaxed) >= self.max_pending_control {
                    handle.kill.notify_one();
                    continue;
                }
                handle.pending_control.fetch_add(1, Ordering::Relaxed);
                if handle.tx.send(msg.clone()).is_err() {
                    handle.pending_control.fetch_sub(1, Ordering::Relaxed);
                }
            }
        }
    }

    fn broadcast_user_list(&self) {
        let users: Vec<UserInfo> = self.users.read().values().cloned().collect();
        self.broadcast_all(&WsMessage::UserList(users));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::VideoChatClient;
    use crate::message::{rgb_frame_to_ws, WsVideoFrame, MAX_ENCODED_BYTES};
    use crate::render::RgbFrame;

    async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
        for _ in 0..250 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for: {}", what);
    }

    fn spawn(server: VideoChatServer) -> (Arc<VideoChatServer>, String) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Arc::new(server);
        tokio::spawn(Arc::clone(&server).run_std(listener));
        (server, format!("ws://127.0.0.1:{}", port))
    }

    fn join_text(name: &str) -> TungsteniteMsg {
        TungsteniteMsg::Text(
            serde_json::to_string(&WsMessage::Join {
                username: name.to_string(),
                version: PROTOCOL_VERSION,
            })
            .unwrap(),
        )
    }

    fn frame_msg(frame: WsVideoFrame) -> WsMessage {
        WsMessage::Frame {
            user_id: String::new(),
            username: String::new(),
            frame,
        }
    }

    /// An outdated (v1) client must be rejected with a friendly Ack and a
    /// closed connection, never silently ignored.
    #[tokio::test]
    async fn rejects_v1_clients_with_friendly_ack() {
        let (_server, url) = spawn(VideoChatServer::new());
        let (ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (mut tx, mut rx) = ws.split();

        // v1 Join: no version field -> serde default 0
        tx.send(TungsteniteMsg::Text(
            r#"{"type":"Join","data":{"username":"old-timer"}}"#.to_string(),
        ))
        .await
        .unwrap();

        let mut got_rejection = false;
        while let Ok(Some(msg)) =
            tokio::time::timeout(std::time::Duration::from_secs(5), rx.next()).await
        {
            match msg {
                Ok(TungsteniteMsg::Text(text)) => {
                    if let Ok(WsMessage::Ack { success, message }) =
                        serde_json::from_str::<WsMessage>(&text)
                    {
                        assert!(!success);
                        assert!(message.contains("version mismatch"));
                        got_rejection = true;
                    }
                }
                Ok(TungsteniteMsg::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
        assert!(got_rejection, "server never sent the version-mismatch Ack");
    }

    /// Finding #3, server side: malformed frames (hostile dimensions or a
    /// data buffer that does not match them) must be dropped at the relay,
    /// never fanned out to the room.
    #[tokio::test]
    async fn drops_malformed_frames_before_relay() {
        let (_server, url) = spawn(VideoChatServer::new());

        // victim: raw v2 client that records every relayed Frame
        let (victim_ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (mut victim_tx, mut victim_rx) = victim_ws.split();
        victim_tx.send(join_text("victim")).await.unwrap();
        // Welcome is sent after registration, so once it arrives the victim
        // is guaranteed to receive subsequent broadcasts
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(5), victim_rx.next())
                .await
                .expect("no Welcome for the victim")
                .expect("victim socket closed")
                .expect("victim read error");
            if let TungsteniteMsg::Text(text) = msg {
                if matches!(
                    serde_json::from_str::<WsMessage>(&text),
                    Ok(WsMessage::Welcome { .. })
                ) {
                    break;
                }
            }
        }

        let (attacker_ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (mut attacker_tx, _attacker_rx) = attacker_ws.split();
        attacker_tx.send(join_text("attacker")).await.unwrap();

        // the finding-#3 attack: tiny message claiming an enormous frame --
        // rejected by the pixel-dimension caps before any allocation
        let hostile = frame_msg(WsVideoFrame {
            width: 65535,
            height: 65535,
            data: "AAAA".to_string(),
        });
        // oversized encoded payload: past the byte cap, dropped before decode
        let oversized = frame_msg(WsVideoFrame {
            width: 2,
            height: 2,
            data: "A".repeat(MAX_ENCODED_BYTES + 1),
        });
        // a well-formed compressed frame, then a sentinel chat to bound the read
        let valid = frame_msg(rgb_frame_to_ws(&{
            let mut f = RgbFrame::new(1, 1);
            f.data = vec![9, 9, 9];
            f
        }));
        let sentinel = WsMessage::Chat {
            user_id: String::new(),
            username: String::new(),
            content: "sentinel".to_string(),
        };
        for msg in [&hostile, &oversized, &valid, &sentinel] {
            attacker_tx
                .send(TungsteniteMsg::Text(serde_json::to_string(msg).unwrap()))
                .await
                .unwrap();
        }

        // relay preserves per-connection order, so when the sentinel lands
        // every frame the server chose to relay has been observed
        let mut relayed = Vec::new();
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(5), victim_rx.next())
                .await
                .expect("sentinel never arrived")
                .expect("victim socket closed early")
                .expect("victim read error");
            if let TungsteniteMsg::Text(text) = msg {
                match serde_json::from_str::<WsMessage>(&text) {
                    Ok(WsMessage::Frame { frame, .. }) => relayed.push(frame),
                    Ok(WsMessage::Chat { content, .. }) if content == "sentinel" => break,
                    _ => {}
                }
            }
        }
        assert_eq!(relayed.len(), 1, "only the well-formed frame may be relayed");
        assert!(relayed[0].is_well_formed());
        assert_eq!((relayed[0].width, relayed[0].height), (1, 1));
    }

    /// Finding #16: a half-open peer (no FIN, no pongs -- a slept laptop)
    /// must be evicted by the ping/silence machinery, while a live peer that
    /// keeps ponging is untouched.
    #[tokio::test]
    async fn pings_and_drops_half_open_peers() {
        let (server, url) = spawn(VideoChatServer::tuned(
            Duration::from_millis(100),
            Duration::from_millis(400),
            MAX_PENDING_CONTROL,
        ));

        // watcher: a real client whose reader auto-pongs the server's pings
        let watcher = Arc::new(VideoChatClient::new("watcher".to_string(), url.clone()));
        watcher.set_webcam_enabled(false);
        Arc::clone(&watcher).connect().await.expect("watcher connects");
        wait_for("watcher welcome", || watcher.my_id().is_some()).await;

        // ghost: joins, then never reads or writes again; the socket stays
        // open so the server sees no FIN and no pongs
        let (ghost_ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (mut ghost_tx, _ghost_rx) = ghost_ws.split();
        ghost_tx.send(join_text("ghost")).await.unwrap();

        wait_for("both in roster", || {
            watcher.connected_users.read().len() == 2
        })
        .await;

        // silence from the ghost -> eviction, broadcast as a normal leave
        wait_for("ghost evicted from roster", || {
            watcher.connected_users.read().len() == 1
        })
        .await;
        wait_for("ghost connection cleaned up", || {
            server.connections.read().len() == 1 && server.users.read().len() == 1
        })
        .await;
        assert!(
            watcher.is_connected(),
            "the live, ponging watcher must not be collateral damage"
        );
        watcher.disconnect();
    }

    /// Finding #16: a consumer that stops draining entirely gets its control
    /// queue capped -- the connection is killed instead of growing the queue
    /// (and server memory) without bound.
    #[tokio::test]
    async fn kills_slow_consumers_when_control_queue_overflows() {
        // pings out of the picture; tiny control cap so the test is quick
        let (server, url) = spawn(VideoChatServer::tuned(
            Duration::from_secs(30),
            Duration::from_secs(60),
            8,
        ));

        let sender = Arc::new(VideoChatClient::new("sender".to_string(), url.clone()));
        sender.set_webcam_enabled(false);
        Arc::clone(&sender).connect().await.expect("sender connects");
        wait_for("sender welcome", || sender.my_id().is_some()).await;

        // slow consumer: joins, then never drains its socket
        let (slow_ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let (mut slow_tx, _slow_rx) = slow_ws.split();
        slow_tx.send(join_text("molasses")).await.unwrap();
        wait_for("both registered", || server.connections.read().len() == 2).await;

        // flood control traffic: large chats fill the slow consumer's TCP
        // buffers first, then its server-side queue, then the cap kills it
        let big = "x".repeat(128 * 1024);
        for _ in 0..60 {
            sender.send_chat(big.clone());
        }

        wait_for("slow consumer killed", || {
            server.connections.read().len() == 1
        })
        .await;
        wait_for("sender's roster shrinks", || {
            sender.connected_users.read().len() == 1
        })
        .await;
        assert!(
            sender.is_connected(),
            "killing the slow consumer must not touch the sender"
        );
        sender.disconnect();
    }
}
