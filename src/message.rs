use crate::render::RgbFrame;
use base64::prelude::{Engine, BASE64_STANDARD};
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// Wire protocol version.
/// * v2 added: Join.version, Welcome, Game, u32-codepoint frame cells.
/// * v3 replaces the per-cell glyph frame with a compressed raw-RGB
///   [`WsVideoFrame`] so feeds carry true pixels at a fraction of the
///   bandwidth, and the renderer -- not the wire -- decides glyph vs half-block
///   vs true-pixel. v3 peers are mutually incompatible with v2 (the frame shape
///   changed), which the version gate already enforces.
pub const PROTOCOL_VERSION: u32 = 3;

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
    Frame {
        user_id: String,
        username: String,
        frame: WsVideoFrame,
    },
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

/// Wire video frame: raw RGB24 pixels, DEFLATE-compressed then base64-encoded so
/// it rides inside the JSON control channel. `width`/`height` are pixel
/// dimensions; the decoded buffer is exactly `width*height*3` bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsVideoFrame {
    pub width: u16,
    pub height: u16,
    /// base64(deflate(RGB24))
    pub data: String,
}

/// Hard caps on relayed video frames (in PIXELS). Anything larger is hostile or
/// corrupt. Two independent bounds -- the pixel dimensions AND the encoded
/// payload length -- so neither a giant claimed frame nor a giant blob can
/// force a huge allocation on a peer before validation.
pub const MAX_FRAME_WIDTH: u16 = 640;
pub const MAX_FRAME_HEIGHT: u16 = 480;
/// Upper bound on the base64 payload we will even attempt to decode.
pub const MAX_ENCODED_BYTES: usize = 512 * 1024;

impl WsVideoFrame {
    /// True when the claimed dimensions are within the pixel caps and the
    /// encoded payload is non-empty and within its own cap. Cheap structural
    /// gate the server runs BEFORE relaying and the client runs BEFORE decoding
    /// -- a hostile participant can never force a huge allocation on peers.
    pub fn is_well_formed(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.width <= MAX_FRAME_WIDTH
            && self.height <= MAX_FRAME_HEIGHT
            && !self.data.is_empty()
            && self.data.len() <= MAX_ENCODED_BYTES
    }
}

/// Encode an [`RgbFrame`] for the wire: deflate the raw RGB then base64 it.
pub fn rgb_frame_to_ws(frame: &RgbFrame) -> WsVideoFrame {
    let mut enc = DeflateEncoder::new(Vec::new(), Compression::fast());
    let _ = enc.write_all(&frame.data);
    let compressed = enc.finish().unwrap_or_default();
    WsVideoFrame {
        width: frame.width,
        height: frame.height,
        data: BASE64_STANDARD.encode(&compressed),
    }
}

/// Decode a wire frame into a renderable [`RgbFrame`], or `None` for anything
/// malformed. Safe against BOTH hostile dimensions (rejected by the caps in
/// [`WsVideoFrame::is_well_formed`]) and decompression bombs: the inflate output
/// is bounded to `expected + 1` bytes and an exact-length match is required, so
/// a tiny payload claiming to expand to gigabytes can never over-allocate.
pub fn ws_frame_to_rgb(ws: &WsVideoFrame) -> Option<RgbFrame> {
    if !ws.is_well_formed() {
        return None;
    }
    let expected = ws.width as usize * ws.height as usize * 3;
    let compressed = BASE64_STANDARD.decode(ws.data.as_bytes()).ok()?;
    let mut out = Vec::with_capacity(expected.min(1 << 20));
    DeflateDecoder::new(&compressed[..])
        .take(expected as u64 + 1)
        .read_to_end(&mut out)
        .ok()?;
    if out.len() != expected {
        return None;
    }
    Some(RgbFrame {
        width: ws.width,
        height: ws.height,
        data: out,
    })
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
    fn video_frame_roundtrips_through_the_wire() {
        let mut frame = RgbFrame::new(4, 3);
        for (i, b) in frame.data.iter_mut().enumerate() {
            *b = (i * 7 % 256) as u8;
        }
        let ws = rgb_frame_to_ws(&frame);
        assert!(ws.is_well_formed());
        let back = ws_frame_to_rgb(&ws).expect("well-formed frame decodes");
        assert_eq!((back.width, back.height), (4, 3));
        assert_eq!(back.data, frame.data, "pixels must survive the wire");
    }

    #[test]
    fn rejects_hostile_dimensions_without_allocating() {
        // tiny payload claiming an enormous frame
        let hostile = WsVideoFrame {
            width: 65535,
            height: 65535,
            data: "AAAA".to_string(),
        };
        assert!(!hostile.is_well_formed());
        assert!(ws_frame_to_rgb(&hostile).is_none());

        // one past a single cap is rejected
        let too_wide = WsVideoFrame {
            width: MAX_FRAME_WIDTH + 1,
            height: 1,
            data: "AAAA".to_string(),
        };
        assert!(!too_wide.is_well_formed());

        // empty payload rejected
        let empty = WsVideoFrame {
            width: 2,
            height: 2,
            data: String::new(),
        };
        assert!(!empty.is_well_formed());
    }

    #[test]
    fn rejects_decompression_bomb() {
        // A well-formed 1x1 frame (expects 3 bytes) whose payload actually
        // decompresses to far more must be rejected, never over-allocated.
        let big = RgbFrame::new(64, 64); // 12288 bytes of zeros -> tiny deflate
        let mut ws = rgb_frame_to_ws(&big);
        // lie about the dimensions: claim 1x1 (expects 3 bytes)
        ws.width = 1;
        ws.height = 1;
        assert!(ws.is_well_formed(), "structurally it still looks fine");
        assert!(
            ws_frame_to_rgb(&ws).is_none(),
            "output longer than expected must be rejected (bomb guard)"
        );
    }

    #[test]
    fn accepts_maximum_allowed_dimensions() {
        let frame = RgbFrame::new(MAX_FRAME_WIDTH, MAX_FRAME_HEIGHT);
        let ws = rgb_frame_to_ws(&frame);
        assert!(ws.is_well_formed());
        let back = ws_frame_to_rgb(&ws).expect("cap-sized frame is valid");
        assert_eq!(
            back.data.len(),
            MAX_FRAME_WIDTH as usize * MAX_FRAME_HEIGHT as usize * 3
        );
    }
}
