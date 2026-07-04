use serde::{Deserialize, Serialize};

/// Wire protocol version. v2 adds: Join.version, Welcome, Game, u32-codepoint
/// frame cells (full Unicode glyphs survive the wire).
pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WsMessage {
    Join {
        username: String,
        #[serde(default)]
        version: u32,
    },
    /// server -> joining client: your server-assigned id
    Welcome { user_id: String },
    Frame { user_id: String, username: String, frame: WsAsciiFrame },
    Chat { user_id: String, username: String, content: String },
    /// game-scoped payload relayed to all OTHER clients; identity is
    /// rewritten server-side so it cannot be spoofed
    Game {
        user_id: String,
        username: String,
        game: String,
        payload: serde_json::Value,
    },
    UserList(Vec<UserInfo>),
    UserLeft { user_id: String, username: String },
    UserJoined { user_id: String, username: String },
    Ack { success: bool, message: String },
    Ping,
    Pong,
}

/// Wire video frame: 4 u32 words per cell -- [glyph codepoint, r, g, b].
/// Full char codepoints so Unicode shading glyphs (e.g. '▀') survive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsAsciiFrame {
    pub width: u16,
    pub height: u16,
    pub data: Vec<u32>,
}

/// Hard caps on relayed video frames. Anything larger is hostile or corrupt
/// -- a ~60-byte JSON message claiming 65535x65535 would otherwise make every
/// receiver allocate gigabytes (see review finding #3).
pub const MAX_FRAME_WIDTH: u16 = 400;
pub const MAX_FRAME_HEIGHT: u16 = 200;

#[allow(dead_code)]
impl WsAsciiFrame {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            data: vec![0; width as usize * height as usize * 4],
        }
    }

    /// True when the claimed dimensions are within the hard caps AND the data
    /// buffer length matches them exactly (4 words per cell). Both the server
    /// (before relaying) and the client (before decoding) must check this so
    /// a hostile participant can never force a huge allocation on peers.
    /// The dimension caps are checked first so the multiplication below can
    /// never overflow.
    pub fn is_well_formed(&self) -> bool {
        self.width <= MAX_FRAME_WIDTH
            && self.height <= MAX_FRAME_HEIGHT
            && self.data.len() == self.width as usize * self.height as usize * 4
    }

    pub fn set_cell(&mut self, x: u16, y: u16, ch: char, r: u8, g: u8, b: u8) {
        let idx = (y as usize * self.width as usize + x as usize) * 4;
        if idx + 3 < self.data.len() {
            self.data[idx] = ch as u32;
            self.data[idx + 1] = r as u32;
            self.data[idx + 2] = g as u32;
            self.data[idx + 3] = b as u32;
        }
    }

    pub fn get_cell(&self, x: u16, y: u16) -> Option<(char, u8, u8, u8)> {
        let idx = (y as usize * self.width as usize + x as usize) * 4;
        if idx + 3 < self.data.len() {
            Some((
                decode_glyph(self.data[idx]),
                (self.data[idx + 1] & 0xff) as u8,
                (self.data[idx + 2] & 0xff) as u8,
                (self.data[idx + 3] & 0xff) as u8,
            ))
        } else {
            None
        }
    }
}

/// Decode a wire codepoint into a renderable char; control chars and invalid
/// codepoints become spaces so they never corrupt the terminal.
pub fn decode_glyph(codepoint: u32) -> char {
    char::from_u32(codepoint)
        .filter(|c| !c.is_control())
        .unwrap_or(' ')
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub user_id: String,
    pub username: String,
    pub connected_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_without_version_defaults_to_zero() {
        let msg: WsMessage =
            serde_json::from_str(r#"{"type":"Join","data":{"username":"old"}}"#).unwrap();
        match msg {
            WsMessage::Join { username, version } => {
                assert_eq!(username, "old");
                assert_eq!(version, 0);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn join_roundtrip_carries_version() {
        let msg = WsMessage::Join {
            username: "nu".to_string(),
            version: PROTOCOL_VERSION,
        };
        let json = serde_json::to_string(&msg).unwrap();
        match serde_json::from_str::<WsMessage>(&json).unwrap() {
            WsMessage::Join { version, .. } => assert_eq!(version, PROTOCOL_VERSION),
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn game_message_roundtrip() {
        let msg = WsMessage::Game {
            user_id: "u1".to_string(),
            username: "alice".to_string(),
            game: "pong".to_string(),
            payload: serde_json::json!({"t": "input", "dir": -1}),
        };
        let json = serde_json::to_string(&msg).unwrap();
        match serde_json::from_str::<WsMessage>(&json).unwrap() {
            WsMessage::Game { user_id, game, payload, .. } => {
                assert_eq!(user_id, "u1");
                assert_eq!(game, "pong");
                assert_eq!(payload["dir"], -1);
            }
            other => panic!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn frame_cells_preserve_multibyte_glyphs() {
        let mut frame = WsAsciiFrame::new(2, 1);
        frame.set_cell(0, 0, '▀', 255, 10, 20);
        frame.set_cell(1, 0, 'A', 1, 2, 3);
        assert_eq!(frame.get_cell(0, 0), Some(('▀', 255, 10, 20)));
        assert_eq!(frame.get_cell(1, 0), Some(('A', 1, 2, 3)));

        // survives a serde roundtrip too
        let json = serde_json::to_string(&frame).unwrap();
        let back: WsAsciiFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(back.get_cell(0, 0), Some(('▀', 255, 10, 20)));
    }

    #[test]
    fn frame_validation_rejects_hostile_and_mismatched_frames() {
        // a normal constructed frame is well-formed
        assert!(WsAsciiFrame::new(2, 2).is_well_formed());
        assert!(WsAsciiFrame::new(0, 0).is_well_formed());
        // the largest allowed frame is well-formed
        assert!(WsAsciiFrame::new(MAX_FRAME_WIDTH, MAX_FRAME_HEIGHT).is_well_formed());

        // hostile dims with a tiny buffer (the finding-#3 attack message)
        let hostile = WsAsciiFrame {
            width: 65535,
            height: 65535,
            data: vec![],
        };
        assert!(!hostile.is_well_formed());

        // one past either cap is rejected even with a consistent buffer
        let too_wide = WsAsciiFrame::new(MAX_FRAME_WIDTH + 1, 1);
        assert!(!too_wide.is_well_formed());
        let too_tall = WsAsciiFrame::new(1, MAX_FRAME_HEIGHT + 1);
        assert!(!too_tall.is_well_formed());

        // data length must match exactly: short and long both rejected
        let short = WsAsciiFrame {
            width: 2,
            height: 1,
            data: vec![0; 4], // one cell of data for two cells
        };
        assert!(!short.is_well_formed());
        let long = WsAsciiFrame {
            width: 2,
            height: 1,
            data: vec![0; 12],
        };
        assert!(!long.is_well_formed());
    }

    #[test]
    fn decode_glyph_sanitizes_garbage() {
        assert_eq!(decode_glyph(0), ' ');
        assert_eq!(decode_glyph(0x07), ' ');
        assert_eq!(decode_glyph(0xD800), ' '); // unpaired surrogate
        assert_eq!(decode_glyph('▀' as u32), '▀');
    }
}
