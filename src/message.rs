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
    /// Sent right after `Ack` so a client knows its own id.
    Welcome { user_id: String },
    /// Multiplayer game traffic. The server stamps `user_id` with the
    /// sender's id and relays the envelope to everyone in the room.
    Game { user_id: String, payload: GameMsg },
    Ping,
    Pong,
}

/// Game-layer messages, relayed verbatim by the server. The host of a match
/// runs the simulation; other players send `Input` and render `State`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum GameMsg {
    /// Broadcast by a host every second so the lobby can list the match.
    Lobby { game: String, host_name: String, players: Vec<String> },
    /// A player asks the host to add them.
    Join { game: String, name: String },
    Leave { game: String },
    Input { seq: u32, forward: f32, strafe: f32, turn: f32, fire: bool },
    State { tick: u32, players: Vec<NetPlayer>, projectiles: Vec<NetProjectile>, events: Vec<String> },
    /// Frostbite (co-op campaign) input and state.
    DoomInput { seq: u32, forward: f32, strafe: f32, turn: f32, fire: bool, weapon: u8, interact: bool },
    DoomState {
        tick: u32,
        level: u8,
        phase: u8,
        phase_timer: f32,
        heroes: Vec<NetHero>,
        enemies: Vec<NetEnemy>,
        shots: Vec<NetShot>,
        pickups: Vec<NetPickup>,
        open_doors: Vec<(u8, u8)>,
        kills: u32,
        kills_needed: u32,
        events: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetHero {
    pub id: String,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub angle: f32,
    pub hp: i32,
    pub ammo: u32,
    pub keys: u8,
    pub weapon: u8,
    pub kills: u32,
    pub color: u8,
    pub dead_for: f32,
    pub hurt: f32,
    pub fired: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetEnemy {
    pub id: u32,
    pub kind: u8,
    pub x: f32,
    pub y: f32,
    pub hp: i32,
    pub max_hp: i32,
    pub hurt: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetShot {
    pub x: f32,
    pub y: f32,
    pub hostile: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetPickup {
    pub x: f32,
    pub y: f32,
    pub kind: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetPlayer {
    pub id: String,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub angle: f32,
    pub score: u32,
    pub color: u8,
    /// Seconds left frozen after being hit (0 = active).
    pub frozen: f32,
    pub bot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetProjectile {
    pub x: f32,
    pub y: f32,
    pub owner: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_envelope_round_trips_through_json() {
        let msg = WsMessage::Game {
            user_id: String::new(),
            payload: GameMsg::Lobby {
                game: "arena".to_string(),
                host_name: "alice".to_string(),
                players: vec!["alice".to_string()],
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: WsMessage = serde_json::from_str(&json).unwrap();
        match back {
            WsMessage::Game { payload: GameMsg::Lobby { host_name, .. }, .. } => assert_eq!(host_name, "alice"),
            other => panic!("unexpected: {:?}", other),
        }
        let state = WsMessage::Game {
            user_id: "x".into(),
            payload: GameMsg::State {
                tick: 3,
                players: vec![NetPlayer { id: "a".into(), name: "a".into(), x: 1.0, y: 2.0, angle: 0.5, score: 1, color: 0, frozen: 0.0, bot: false }],
                projectiles: vec![NetProjectile { x: 1.0, y: 1.0, owner: "a".into() }],
                events: vec!["hi".into()],
            },
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(serde_json::from_str::<WsMessage>(&json).is_ok(), "{}", json);
    }
}
