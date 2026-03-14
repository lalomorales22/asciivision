use crate::message::{UserInfo, WsMessage};
use anyhow::Result;
use futures::{SinkExt, StreamExt};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as TungsteniteMsg;

pub struct VideoChatServer {
    connections: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>,
    users: Arc<RwLock<HashMap<String, UserInfo>>>,
}

impl VideoChatServer {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            users: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn run(self: Arc<Self>, addr: &str) -> Result<()> {
        let listener = TcpListener::bind(addr).await?;
        loop {
            let (stream, _) = listener.accept().await?;
            let server = Arc::clone(&self);
            tokio::spawn(async move {
                if let Err(e) = server.handle_connection(stream).await {
                    eprintln!("connection error: {}", e);
                }
            });
        }
    }

    async fn handle_connection(&self, stream: TcpStream) -> Result<()> {
        let ws_stream = tokio_tungstenite::accept_async(stream).await?;
        let (mut ws_tx, mut ws_rx) = ws_stream.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<WsMessage>();

        let mut user_id: Option<String> = None;

        loop {
            tokio::select! {
                msg = ws_rx.next() => {
                    match msg {
                        Some(Ok(TungsteniteMsg::Text(text))) => {
                            if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                                match ws_msg {
                                    WsMessage::Join { username } => {
                                        let id = uuid::Uuid::new_v4().to_string();
                                        let info = UserInfo {
                                            user_id: id.clone(),
                                            username: username.clone(),
                                            connected_at: chrono::Utc::now().to_rfc3339(),
                                        };
                                        self.connections.write().insert(id.clone(), tx.clone());
                                        self.users.write().insert(id.clone(), info);
                                        user_id = Some(id.clone());

                                        let ack = WsMessage::Ack {
                                            success: true,
                                            message: format!("welcome, {}!", username),
                                        };
                                        let _ = ws_tx.send(TungsteniteMsg::Text(serde_json::to_string(&ack)?)).await;
                                        self.broadcast_user_list()?;
                                        self.broadcast_except(&WsMessage::UserJoined {
                                            user_id: id,
                                            username,
                                        }, user_id.as_deref())?;
                                    }
                                    WsMessage::Frame { frame, .. } => {
                                        if let Some(ref uid) = user_id {
                                            let uname = self.users.read().get(uid).map(|u| u.username.clone()).unwrap_or_default();
                                            self.broadcast_all(&WsMessage::Frame {
                                                user_id: uid.clone(),
                                                username: uname,
                                                frame,
                                            })?;
                                        }
                                    }
                                    WsMessage::Chat { content, .. } => {
                                        if let Some(ref uid) = user_id {
                                            let uname = self.users.read().get(uid).map(|u| u.username.clone()).unwrap_or_default();
                                            self.broadcast_all(&WsMessage::Chat {
                                                user_id: uid.clone(),
                                                username: uname,
                                                content,
                                            })?;
                                        }
                                    }
                                    WsMessage::Ping => {
                                        let _ = ws_tx.send(TungsteniteMsg::Text(serde_json::to_string(&WsMessage::Pong)?)).await;
                                    }
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
                    if let Ok(json) = serde_json::to_string(&msg) {
                        let _ = ws_tx.send(TungsteniteMsg::Text(json)).await;
                    }
                }
            }
        }

        if let Some(ref uid) = user_id {
            let uname = self.users.read().get(uid).map(|u| u.username.clone()).unwrap_or_default();
            self.connections.write().remove(uid);
            self.users.write().remove(uid);
            let _ = self.broadcast_all(&WsMessage::UserLeft {
                user_id: uid.clone(),
                username: uname,
            });
            let _ = self.broadcast_user_list();
        }

        Ok(())
    }

    fn broadcast_all(&self, msg: &WsMessage) -> Result<()> {
        let conns = self.connections.read();
        for tx in conns.values() {
            let _ = tx.send(msg.clone());
        }
        Ok(())
    }

    fn broadcast_except(&self, msg: &WsMessage, except: Option<&str>) -> Result<()> {
        let conns = self.connections.read();
        for (id, tx) in conns.iter() {
            if except.map_or(true, |eid| id != eid) {
                let _ = tx.send(msg.clone());
            }
        }
        Ok(())
    }

    fn broadcast_user_list(&self) -> Result<()> {
        let users: Vec<UserInfo> = self.users.read().values().cloned().collect();
        self.broadcast_all(&WsMessage::UserList(users))
    }
}
