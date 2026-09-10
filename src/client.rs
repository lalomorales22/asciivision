use crate::message::{GameMsg, WsMessage};
use crate::video::AsciiFrame;
use crate::webcam::{ascii_frame_to_ws, ws_frame_to_ascii};
use anyhow::Result;
use futures::{SinkExt, StreamExt};
use parking_lot::RwLock;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message as TungsteniteMsg};

/// Everything a game needs to talk to the room: send game messages and
/// drain the ones that arrived. Cheap to clone (all Arcs).
#[derive(Clone)]
pub struct NetHandle {
    pub username: String,
    my_id: Arc<RwLock<Option<String>>>,
    connected: Arc<RwLock<bool>>,
    tx: mpsc::UnboundedSender<GameMsg>,
    inbox: Arc<RwLock<VecDeque<(String, GameMsg)>>>,
}

impl NetHandle {
    pub fn my_id(&self) -> Option<String> {
        self.my_id.read().clone()
    }

    pub fn is_connected(&self) -> bool {
        *self.connected.read() && self.my_id.read().is_some()
    }

    pub fn send(&self, msg: GameMsg) {
        netlog(&format!("handle.send {:?}", std::mem::discriminant(&msg)));
        if self.tx.send(msg).is_err() {
            netlog("handle.send: receiver gone");
        }
    }

    /// Take every game message received since the last drain, oldest first.
    pub fn drain(&self) -> Vec<(String, GameMsg)> {
        self.inbox.write().drain(..).collect()
    }
}

pub struct VideoChatClient {
    pub username: String,
    pub server_url: String,
    pub connected_users: Arc<RwLock<Vec<String>>>,
    pub remote_frames: Arc<RwLock<HashMap<String, (String, AsciiFrame)>>>,
    pub chat_messages: Arc<RwLock<Vec<(String, String)>>>,
    pub connected: Arc<RwLock<bool>>,
    pub status: Arc<RwLock<String>>,
    chat_tx: mpsc::UnboundedSender<String>,
    chat_rx: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
    frame_tx: mpsc::UnboundedSender<AsciiFrame>,
    frame_rx: Arc<Mutex<mpsc::UnboundedReceiver<AsciiFrame>>>,
    my_id: Arc<RwLock<Option<String>>>,
    game_tx: mpsc::UnboundedSender<GameMsg>,
    game_rx: Arc<Mutex<mpsc::UnboundedReceiver<GameMsg>>>,
    game_inbox: Arc<RwLock<VecDeque<(String, GameMsg)>>>,
}

impl VideoChatClient {
    pub fn new(username: String, server_url: String) -> Self {
        let (chat_tx, chat_rx) = mpsc::unbounded_channel();
        let (frame_tx, frame_rx) = mpsc::unbounded_channel();
        let (game_tx, game_rx) = mpsc::unbounded_channel();
        Self {
            username,
            server_url,
            connected_users: Arc::new(RwLock::new(Vec::new())),
            remote_frames: Arc::new(RwLock::new(HashMap::new())),
            chat_messages: Arc::new(RwLock::new(Vec::new())),
            connected: Arc::new(RwLock::new(false)),
            status: Arc::new(RwLock::new("pending".to_string())),
            chat_tx,
            chat_rx: Arc::new(Mutex::new(chat_rx)),
            frame_tx,
            frame_rx: Arc::new(Mutex::new(frame_rx)),
            my_id: Arc::new(RwLock::new(None)),
            game_tx,
            game_rx: Arc::new(Mutex::new(game_rx)),
            game_inbox: Arc::new(RwLock::new(VecDeque::new())),
        }
    }

    pub fn game_handle(&self) -> NetHandle {
        NetHandle {
            username: self.username.clone(),
            my_id: Arc::clone(&self.my_id),
            connected: Arc::clone(&self.connected),
            tx: self.game_tx.clone(),
            inbox: Arc::clone(&self.game_inbox),
        }
    }

    pub fn send_chat(&self, content: String) {
        let _ = self.chat_tx.send(content);
    }

    /// Push a webcam frame for transmission to the server.
    /// Called by the app's tick loop with the existing F5 webcam frame.
    pub fn send_frame(&self, frame: &AsciiFrame) {
        let _ = self.frame_tx.send(frame.clone());
    }

    pub fn is_connected(&self) -> bool {
        *self.connected.read()
    }

    pub fn get_status(&self) -> String {
        self.status.read().clone()
    }

    /// Returns a `'static + Send` future that establishes the WebSocket
    /// connection and manages all communication.  Because it captures
    /// Arc-cloned handles from `self`, every update made by the returned
    /// future is visible through the `VideoChatClient` stored in the app.
    pub fn connect_future(
        &self,
    ) -> impl std::future::Future<Output = Result<()>> + Send + 'static {
        let server_url = self.server_url.clone();
        let username = self.username.clone();
        let connected_users = Arc::clone(&self.connected_users);
        let remote_frames = Arc::clone(&self.remote_frames);
        let chat_messages = Arc::clone(&self.chat_messages);
        let connected = Arc::clone(&self.connected);
        let status = Arc::clone(&self.status);
        let chat_rx = Arc::clone(&self.chat_rx);
        let frame_rx = Arc::clone(&self.frame_rx);
        let my_id = Arc::clone(&self.my_id);
        let game_rx = Arc::clone(&self.game_rx);
        let game_inbox = Arc::clone(&self.game_inbox);

        async move {
            *status.write() = format!("connecting to {}", server_url);

            let (ws_stream, _) = connect_async(&server_url).await?;
            let (ws_tx, mut ws_rx) = ws_stream.split();
            let ws_tx = Arc::new(Mutex::new(ws_tx));

            *connected.write() = true;
            *status.write() = "connected, joining...".to_string();

            let join_msg = WsMessage::Join {
                username: username.clone(),
            };
            {
                let mut tx = ws_tx.lock().await;
                tx.send(TungsteniteMsg::Text(serde_json::to_string(&join_msg)?))
                    .await?;
            }

            // -- receive task --
            let connected_users_rx = Arc::clone(&connected_users);
            let remote_frames_rx = Arc::clone(&remote_frames);
            let chat_messages_rx = Arc::clone(&chat_messages);
            let connected_rx = Arc::clone(&connected);
            let status_rx = Arc::clone(&status);

            tokio::spawn(async move {
                while let Some(msg_result) = ws_rx.next().await {
                    match msg_result {
                        Ok(TungsteniteMsg::Text(text)) => {
                            if let Ok(msg) = serde_json::from_str::<WsMessage>(&text) {
                                match msg {
                                    WsMessage::Ack { message, .. } => {
                                        *status_rx.write() = message;
                                    }
                                    WsMessage::Welcome { user_id } => {
                                        *my_id.write() = Some(user_id);
                                    }
                                    WsMessage::Game { user_id, payload } => {
                                        let mut inbox = game_inbox.write();
                                        inbox.push_back((user_id, payload));
                                        while inbox.len() > 512 {
                                            inbox.pop_front();
                                        }
                                    }
                                    WsMessage::UserList(users) => {
                                        let mut guard = connected_users_rx.write();
                                        *guard =
                                            users.iter().map(|u| u.username.clone()).collect();
                                        *status_rx.write() =
                                            format!("{} users online", guard.len());
                                    }
                                    WsMessage::Frame {
                                        user_id: frame_uid,
                                        username: frame_user,
                                        frame,
                                    } => {
                                        let ascii = ws_frame_to_ascii(&frame);
                                        remote_frames_rx
                                            .write()
                                            .insert(frame_uid, (frame_user, ascii));
                                    }
                                    WsMessage::Chat {
                                        username: chat_user,
                                        content,
                                        ..
                                    } => {
                                        let mut msgs = chat_messages_rx.write();
                                        msgs.push((chat_user, content));
                                        if msgs.len() > 200 {
                                            msgs.drain(0..50);
                                        }
                                    }
                                    WsMessage::UserJoined {
                                        username: joined, ..
                                    } => {
                                        chat_messages_rx.write().push((
                                            "SYSTEM".to_string(),
                                            format!("{} joined", joined),
                                        ));
                                    }
                                    WsMessage::UserLeft {
                                        user_id: left_uid,
                                        username: left,
                                    } => {
                                        chat_messages_rx.write().push((
                                            "SYSTEM".to_string(),
                                            format!("{} left", left),
                                        ));
                                        remote_frames_rx.write().remove(&left_uid);
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Ok(TungsteniteMsg::Close(_)) | Err(_) => break,
                        _ => {}
                    }
                }
                *connected_rx.write() = false;
                *status_rx.write() = "disconnected".to_string();
            });

            // -- frame send task (fed by app's webcam via send_frame) --
            let ws_tx_frame = Arc::clone(&ws_tx);
            let username_frame = username.clone();

            tokio::spawn(async move {
                let mut rx = frame_rx.lock().await;
                while let Some(frame) = rx.recv().await {
                    let ws_frame = ascii_frame_to_ws(&frame);
                    let msg = WsMessage::Frame {
                        user_id: String::new(),
                        username: username_frame.clone(),
                        frame: ws_frame,
                    };
                    if let Ok(json) = serde_json::to_string(&msg) {
                        let mut tx = ws_tx_frame.lock().await;
                        if tx.send(TungsteniteMsg::Text(json)).await.is_err() {
                            break;
                        }
                    }
                }
            });

            // -- game send task --
            let ws_tx_game = Arc::clone(&ws_tx);
            tokio::spawn(async move {
                netlog("game send task: waiting for receiver lock");
                let mut rx = game_rx.lock().await;
                netlog("game send task: running");
                while let Some(payload) = rx.recv().await {
                    let msg = WsMessage::Game { user_id: String::new(), payload };
                    if let Ok(json) = serde_json::to_string(&msg) {
                        let mut tx = ws_tx_game.lock().await;
                        if tx.send(TungsteniteMsg::Text(json)).await.is_err() {
                            netlog("game send task: socket write failed");
                            break;
                        }
                    }
                }
                netlog("game send task: ended");
            });

            // -- chat send task --
            let ws_tx_chat = Arc::clone(&ws_tx);
            let username_chat = username.clone();

            tokio::spawn(async move {
                let mut rx = chat_rx.lock().await;
                while let Some(content) = rx.recv().await {
                    let msg = WsMessage::Chat {
                        user_id: String::new(),
                        username: username_chat.clone(),
                        content,
                    };
                    if let Ok(json) = serde_json::to_string(&msg) {
                        let mut tx = ws_tx_chat.lock().await;
                        let _ = tx.send(TungsteniteMsg::Text(json)).await;
                    }
                }
            });

            Ok(())
        }
    }
}

/// Debug trace of the game transport, enabled by ASCIIVISION_NETLOG=<file>.
fn netlog(line: &str) {
    if let Ok(path) = std::env::var("ASCIIVISION_NETLOG") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{} {}", chrono::Utc::now().format("%H:%M:%S%.3f"), line);
        }
    }
}
