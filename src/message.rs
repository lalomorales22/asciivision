use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WsMessage {
    Join { username: String },
    Frame { user_id: String, username: String, frame: WsAsciiFrame },
    Chat { user_id: String, username: String, content: String },
    UserList(Vec<UserInfo>),
    UserLeft { user_id: String, username: String },
    UserJoined { user_id: String, username: String },
    Ack { success: bool, message: String },
    Ping,
    Pong,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsAsciiFrame {
    pub width: u16,
    pub height: u16,
    pub data: Vec<u8>,
}

#[allow(dead_code)]
impl WsAsciiFrame {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            data: vec![0; width as usize * height as usize * 4],
        }
    }

    pub fn set_cell(&mut self, x: u16, y: u16, ch: char, r: u8, g: u8, b: u8) {
        let idx = (y as usize * self.width as usize + x as usize) * 4;
        if idx + 3 < self.data.len() {
            self.data[idx] = ch as u8;
            self.data[idx + 1] = r;
            self.data[idx + 2] = g;
            self.data[idx + 3] = b;
        }
    }

    pub fn get_cell(&self, x: u16, y: u16) -> Option<(char, u8, u8, u8)> {
        let idx = (y as usize * self.width as usize + x as usize) * 4;
        if idx + 3 < self.data.len() {
            Some((
                self.data[idx] as char,
                self.data[idx + 1],
                self.data[idx + 2],
                self.data[idx + 3],
            ))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub user_id: String,
    pub username: String,
    pub connected_at: String,
}
