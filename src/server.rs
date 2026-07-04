use crate::message::{UserInfo, WsMessage, PROTOCOL_VERSION};
use anyhow::Result;
use futures::{SinkExt, StreamExt};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as TungsteniteMsg;

/// A slow client's outbound queue keeps at most this many video frames in
/// flight; older frames are dropped for that connection (chat/control
/// messages are never dropped).
const MAX_PENDING_FRAMES: usize = 8;

struct ConnHandle {
    tx: mpsc::UnboundedSender<WsMessage>,
    pending_frames: Arc<AtomicUsize>,
}

pub struct VideoChatServer {
    connections: Arc<RwLock<HashMap<String, ConnHandle>>>,
    users: Arc<RwLock<HashMap<String, UserInfo>>>,
}

impl VideoChatServer {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            users: Arc::new(RwLock::new(HashMap::new())),
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

        let mut user_id: Option<String> = None;

        'session: loop {
            tokio::select! {
                msg = ws_rx.next() => {
                    match msg {
                        Some(Ok(TungsteniteMsg::Text(text))) => {
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
                                        if let Some(ref uid) = user_id {
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
                        Some(Ok(TungsteniteMsg::Close(_))) | None => break,
                        Some(Err(_)) => break,
                        _ => {}
                    }
                }
                Some(msg) = rx.recv() => {
                    if matches!(msg, WsMessage::Frame { .. }) {
                        pending_frames.fetch_sub(1, Ordering::Relaxed);
                    }
                    if let Ok(json) = serde_json::to_string(&msg) {
                        if ws_tx.send(TungsteniteMsg::Text(json)).await.is_err() {
                            break;
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
                let _ = handle.tx.send(msg.clone());
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

    /// An outdated (v1) client must be rejected with a friendly Ack and a
    /// closed connection, never silently ignored.
    #[tokio::test]
    async fn rejects_v1_clients_with_friendly_ack() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Arc::new(VideoChatServer::new());
        tokio::spawn(server.run_std(listener));

        let url = format!("ws://127.0.0.1:{}", port);
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
}
