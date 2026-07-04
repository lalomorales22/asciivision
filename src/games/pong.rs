//! Pong: vs-AI, local 2P, and online host/join (host-authoritative).
//!
//! Wire protocol (payloads inside the app's Game envelope, game = "pong"):
//!   {"t":"invite"} {"t":"join"} {"t":"start","seed":u64} {"t":"full"}
//!   {"t":"ka"} {"t":"quit"}                                  handshake + liveness
//!   {"t":"input","y":f32}                                    guest -> host (on change, <=30Hz)
//!   {"t":"state","bx","by","vx","vy","p0","p1","s0","s1","ph","cd","mg"}  host -> guest (~17Hz)
//! ph: 0=lobby 1=serve 2=play 3=over. `mg` is the host's match generation:
//! it bumps on every match reset (rematch), telling the guest to re-center
//! its locally-predicted paddle. Guests dead-reckon the ball between
//! snapshots and keep their own paddle locally predicted.

use crossterm::event::{KeyCode, KeyEvent};
use rand::Rng;
use ratatui::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;

use super::net::{Handshake, HsEvent, Liveness, NetHandle, NetRole, Throttle};
use super::{draw_mode_menu, project_axis, CellGrid, Game};
use crate::theme::t;

const GAME: &str = "pong";
const PONG_W: f32 = 72.0;
const PONG_H: f32 = 36.0;
const PAD_HALF: f32 = 3.0;
const PAD_X0: f32 = 2.0;
const PAD_X1: f32 = PONG_W - 3.0;
const BALL_SPEED: f32 = 26.0;
const MAX_BALL_SPEED: f32 = 55.0;
const PAD_SPEED: f32 = 28.0;
const AI_PAD_SPEED: f32 = 20.0;
const WIN_SCORE: u32 = 7;
const SERVE_SECS: f32 = 2.2;
const HOLD_SECS: f32 = 0.18;
const SPEEDUP: f32 = 1.045;
const MAX_DEFLECT: f32 = 0.95;
const SPIN: f32 = 0.35;
const STATE_HZ: f32 = 17.0;
const INPUT_HZ: f32 = 30.0;
/// Max ball travel per collision substep; must stay below the 2.0-cell paddle
/// capture window so a fast frame can never tunnel the ball through a paddle.
const SUBSTEP: f32 = 0.8;

/// Reflect a ball off a paddle. `offset` is the contact point relative to the
/// paddle center in [-1, 1] (clamped a bit beyond for edge grazes),
/// `paddle_vel` is the paddle's vertical velocity (adds spin), and `dir` is
/// the horizontal direction the ball travels AFTER the bounce (+1 right).
pub(super) fn paddle_bounce(speed: f32, offset: f32, paddle_vel: f32, dir: f32) -> (f32, f32) {
    let offset = offset.clamp(-1.2, 1.2);
    let angle = offset * MAX_DEFLECT;
    let speed = (speed * SPEEDUP).clamp(BALL_SPEED * 0.8, MAX_BALL_SPEED);
    let vx = dir * speed * angle.cos();
    let vy = speed * angle.sin() + paddle_vel * SPIN;
    (vx, vy)
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "t", rename_all = "lowercase")]
pub(super) enum PongMsg {
    Invite,
    Join,
    Start { seed: u64 },
    Full,
    Ka,
    Quit,
    Input { y: f32 },
    State {
        bx: f32,
        by: f32,
        vx: f32,
        vy: f32,
        p0: f32,
        p1: f32,
        s0: u32,
        s1: u32,
        ph: u8,
        cd: f32,
        /// Match generation: bumped by the host on every match reset.
        #[serde(default)]
        mg: u32,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Menu,
    Lobby,
    Serve,
    Play,
    Over,
}

impl Phase {
    fn code(self) -> u8 {
        match self {
            Self::Menu | Self::Lobby => 0,
            Self::Serve => 1,
            Self::Play => 2,
            Self::Over => 3,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Serve,
            2 => Self::Play,
            3 => Self::Over,
            _ => Self::Lobby,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    VsAi,
    Local2P,
    Online,
}

pub(super) struct PongGame {
    net: NetHandle,
    mode: Mode,
    role: NetRole,
    hs: Option<Handshake>,
    phase: Phase,
    menu_error: Option<String>,
    // ball
    bx: f32,
    by: f32,
    vx: f32,
    vy: f32,
    trail: VecDeque<(f32, f32)>,
    // paddles: 0 = left, 1 = right
    py: [f32; 2],
    pv: [f32; 2],
    hold_up: [f32; 2],
    hold_down: [f32; 2],
    scores: [u32; 2],
    serve_cd: f32,
    // net
    my_side: usize,
    guest_target: f32,
    state_throttle: Throttle,
    input_throttle: Throttle,
    last_sent_y: f32,
    /// Bumped on every match reset; the host broadcasts it so the guest can
    /// detect rematches and re-center its locally-predicted paddle.
    match_gen: u32,
    liveness: Liveness,
    over_msg: String,
}

impl PongGame {
    pub(super) fn new(net: NetHandle) -> Self {
        Self {
            net,
            mode: Mode::VsAi,
            role: NetRole::Local,
            hs: None,
            phase: Phase::Menu,
            menu_error: None,
            bx: PONG_W / 2.0,
            by: PONG_H / 2.0,
            vx: 0.0,
            vy: 0.0,
            trail: VecDeque::new(),
            py: [PONG_H / 2.0; 2],
            pv: [0.0; 2],
            hold_up: [0.0; 2],
            hold_down: [0.0; 2],
            scores: [0; 2],
            serve_cd: 0.0,
            my_side: 0,
            guest_target: PONG_H / 2.0,
            state_throttle: Throttle::new(STATE_HZ),
            input_throttle: Throttle::new(INPUT_HZ),
            last_sent_y: PONG_H / 2.0,
            match_gen: 0,
            liveness: Liveness::new(),
            over_msg: String::new(),
        }
    }

    /// True when this instance simulates the match (local play or online host).
    fn authority(&self) -> bool {
        self.role != NetRole::Guest
    }

    fn reset_match(&mut self, serve_dir: f32) {
        self.scores = [0; 2];
        self.py = [PONG_H / 2.0; 2];
        self.pv = [0.0; 2];
        self.guest_target = PONG_H / 2.0;
        self.last_sent_y = PONG_H / 2.0;
        self.match_gen = self.match_gen.wrapping_add(1);
        self.over_msg.clear();
        self.trail.clear();
        self.serve(serve_dir);
    }

    fn serve(&mut self, dir: f32) {
        self.bx = PONG_W / 2.0;
        self.by = PONG_H / 2.0;
        let angle = rand::thread_rng().gen_range(-0.45..0.45f32);
        self.vx = dir.signum() * BALL_SPEED * angle.cos();
        self.vy = BALL_SPEED * angle.sin();
        self.serve_cd = SERVE_SECS;
        self.phase = Phase::Serve;
        self.trail.clear();
    }

    fn begin_online_match(&mut self, seed: u64) {
        let dir = if seed % 2 == 0 { 1.0 } else { -1.0 };
        self.liveness = Liveness::new();
        self.reset_match(dir);
    }

    fn abort_online(&mut self, message: &str) {
        self.hs = None;
        self.role = NetRole::Local;
        self.phase = Phase::Menu;
        self.menu_error = Some(message.to_string());
    }

    fn score_point(&mut self, side: usize) {
        self.scores[side] += 1;
        if self.scores[side] >= WIN_SCORE {
            self.phase = Phase::Over;
            self.over_msg = self.winner_message(side);
        } else {
            // serve toward the player who conceded
            let dir = if side == 0 { 1.0 } else { -1.0 };
            self.serve(dir);
        }
    }

    fn winner_message(&self, side: usize) -> String {
        match self.mode {
            Mode::VsAi => {
                if side == 0 {
                    "YOU WIN".to_string()
                } else {
                    "COMPUTER WINS".to_string()
                }
            }
            Mode::Local2P => format!("PLAYER {} WINS", side + 1),
            Mode::Online => {
                if side == self.my_side {
                    "YOU WIN".to_string()
                } else {
                    let name = self
                        .hs
                        .as_ref()
                        .map(|hs| hs.opponent_name.clone())
                        .unwrap_or_default();
                    if name.is_empty() {
                        "OPPONENT WINS".to_string()
                    } else {
                        format!("{} WINS", name.to_uppercase())
                    }
                }
            }
        }
    }

    fn press(&mut self, side: usize, up: bool) {
        if up {
            self.hold_up[side] = HOLD_SECS;
        } else {
            self.hold_down[side] = HOLD_SECS;
        }
    }

    fn human_dir(&self, side: usize) -> f32 {
        let mut dir = 0.0;
        if self.hold_up[side] > 0.0 {
            dir -= 1.0;
        }
        if self.hold_down[side] > 0.0 {
            dir += 1.0;
        }
        dir
    }

    fn decay_holds(&mut self, dt: f32) {
        for side in 0..2 {
            self.hold_up[side] = (self.hold_up[side] - dt).max(0.0);
            self.hold_down[side] = (self.hold_down[side] - dt).max(0.0);
        }
    }

    fn clamp_paddle(y: f32) -> f32 {
        y.clamp(PAD_HALF + 0.5, PONG_H - PAD_HALF - 0.5)
    }

    fn move_paddle(&mut self, side: usize, velocity: f32, dt: f32) {
        self.pv[side] = velocity;
        self.py[side] = Self::clamp_paddle(self.py[side] + velocity * dt);
    }

    /// Simulate paddles for one frame (authority only).
    fn sim_paddles(&mut self, dt: f32) {
        // left paddle: always human-driven here (vs-AI player, local P1, or host)
        let v0 = self.human_dir(0) * PAD_SPEED;
        self.move_paddle(0, v0, dt);

        // right paddle
        let v1 = match self.mode {
            Mode::VsAi => {
                let target = if self.vx > 0.0 { self.by } else { PONG_H / 2.0 };
                let delta = target - self.py[1];
                if delta.abs() < 0.7 {
                    0.0
                } else {
                    delta.signum() * AI_PAD_SPEED
                }
            }
            Mode::Local2P => self.human_dir(1) * PAD_SPEED,
            Mode::Online => {
                // move toward the guest's requested position
                let delta = Self::clamp_paddle(self.guest_target) - self.py[1];
                if delta.abs() < 0.2 {
                    0.0
                } else {
                    delta.signum() * PAD_SPEED.min(delta.abs() / dt.max(1e-6))
                }
            }
        };
        self.move_paddle(1, v1, dt);
    }

    /// Simulate the ball for one frame (authority only, Play phase).
    /// Substepped (like breakout) so one slow frame at high ball speed cannot
    /// move the ball across the 2.0-cell paddle window in a single jump.
    fn sim_ball(&mut self, dt: f32) {
        let dist = ((self.vx * dt).powi(2) + (self.vy * dt).powi(2)).sqrt();
        let steps = (dist / SUBSTEP).ceil().max(1.0) as u32;
        let sub = dt / steps as f32;
        for _ in 0..steps {
            self.ball_step(sub);
            if self.phase != Phase::Play {
                return; // point scored: serve()/match-over took over the ball
            }
        }

        self.trail.push_back((self.bx, self.by));
        while self.trail.len() > 7 {
            self.trail.pop_front();
        }
    }

    /// One collision substep of ball movement (authority only).
    fn ball_step(&mut self, dt: f32) {
        self.bx += self.vx * dt;
        self.by += self.vy * dt;

        // wall bounces
        if self.by < 1.0 {
            self.by = 2.0 - self.by;
            self.vy = self.vy.abs();
        } else if self.by > PONG_H - 1.0 {
            self.by = 2.0 * (PONG_H - 1.0) - self.by;
            self.vy = -self.vy.abs();
        }

        // paddle bounces
        if self.vx < 0.0 && self.bx <= PAD_X0 + 1.0 && self.bx > PAD_X0 - 1.0 {
            let offset = (self.by - self.py[0]) / PAD_HALF;
            if offset.abs() <= 1.25 {
                let speed = (self.vx * self.vx + self.vy * self.vy).sqrt();
                let (vx, vy) = paddle_bounce(speed, offset, self.pv[0], 1.0);
                self.vx = vx;
                self.vy = vy;
                self.bx = PAD_X0 + 1.0;
            }
        } else if self.vx > 0.0 && self.bx >= PAD_X1 - 1.0 && self.bx < PAD_X1 + 1.0 {
            let offset = (self.by - self.py[1]) / PAD_HALF;
            if offset.abs() <= 1.25 {
                let speed = (self.vx * self.vx + self.vy * self.vy).sqrt();
                let (vx, vy) = paddle_bounce(speed, offset, self.pv[1], -1.0);
                self.vx = vx;
                self.vy = vy;
                self.bx = PAD_X1 - 1.0;
            }
        }

        // scoring
        if self.bx < -1.5 {
            self.score_point(1);
        } else if self.bx > PONG_W + 1.5 {
            self.score_point(0);
        }
    }

    /// Guest-side dead reckoning between host snapshots.
    fn extrapolate_ball(&mut self, dt: f32) {
        self.bx = (self.bx + self.vx * dt).clamp(-2.0, PONG_W + 2.0);
        self.by += self.vy * dt;
        if self.by < 1.0 {
            self.by = 2.0 - self.by;
            self.vy = self.vy.abs();
        } else if self.by > PONG_H - 1.0 {
            self.by = 2.0 * (PONG_H - 1.0) - self.by;
            self.vy = -self.vy.abs();
        }
        self.trail.push_back((self.bx, self.by));
        while self.trail.len() > 7 {
            self.trail.pop_front();
        }
    }

    fn broadcast_state(&mut self, dt: f32) {
        if self.role != NetRole::Host || !self.hs.as_ref().is_some_and(Handshake::is_ready) {
            return;
        }
        if !self.state_throttle.ready(dt) {
            return;
        }
        let msg = PongMsg::State {
            bx: self.bx,
            by: self.by,
            vx: self.vx,
            vy: self.vy,
            p0: self.py[0],
            p1: self.py[1],
            s0: self.scores[0],
            s1: self.scores[1],
            ph: self.phase.code(),
            cd: self.serve_cd,
            mg: self.match_gen,
        };
        if let Ok(payload) = serde_json::to_value(&msg) {
            self.net.send(GAME, payload);
        }
    }

    fn send_guest_input(&mut self, dt: f32) {
        if self.role != NetRole::Guest {
            return;
        }
        let due = self.input_throttle.ready(dt);
        if due && (self.py[self.my_side] - self.last_sent_y).abs() > 0.3 {
            self.last_sent_y = self.py[self.my_side];
            if let Ok(payload) = serde_json::to_value(&PongMsg::Input {
                y: self.last_sent_y,
            }) {
                self.net.send(GAME, payload);
            }
        }
    }

    /// Apply a host snapshot on the guest. Own paddle stays locally predicted,
    /// except when the match generation bumps (host rematch): then the host
    /// has re-centered every paddle, so mirror that locally too.
    #[allow(clippy::too_many_arguments)]
    fn apply_state(
        &mut self,
        bx: f32,
        by: f32,
        vx: f32,
        vy: f32,
        p0: f32,
        p1: f32,
        s0: u32,
        s1: u32,
        ph: u8,
        cd: f32,
        mg: u32,
    ) {
        if mg != self.match_gen {
            self.match_gen = mg;
            self.py[self.my_side] = PONG_H / 2.0;
            self.pv[self.my_side] = 0.0;
            self.last_sent_y = PONG_H / 2.0;
            self.hold_up = [0.0; 2];
            self.hold_down = [0.0; 2];
            self.trail.clear();
        }
        self.bx = bx;
        self.by = by;
        self.vx = vx;
        self.vy = vy;
        if self.my_side != 0 {
            self.py[0] = p0;
        }
        if self.my_side != 1 {
            self.py[1] = p1;
        }
        let winner_side = if s0 > s1 { 0 } else { 1 };
        self.scores = [s0, s1];
        self.serve_cd = cd;
        let phase = Phase::from_code(ph);
        if phase != self.phase {
            self.trail.clear();
            if phase == Phase::Over {
                self.over_msg = self.winner_message(winner_side);
            }
        }
        self.phase = phase;
    }

    fn enter_online(&mut self, host: bool) {
        if !self.net.is_connected() {
            self.menu_error = Some("not connected — use /host or /join first".to_string());
            self.phase = Phase::Menu;
            return;
        }
        self.mode = Mode::Online;
        self.menu_error = None;
        if host {
            self.role = NetRole::Host;
            self.my_side = 0;
            let seed = rand::thread_rng().gen::<u64>();
            let (hs, payload) = Handshake::host(seed);
            self.hs = Some(hs);
            self.net.send(GAME, payload);
        } else {
            self.role = NetRole::Guest;
            self.my_side = 1;
            let (hs, payload) = Handshake::guest();
            self.hs = Some(hs);
            self.net.send(GAME, payload);
        }
        self.phase = Phase::Lobby;
    }

    fn mode_hint(&self) -> &'static str {
        match self.mode {
            Mode::VsAi => "W/S or Up/Down move  R restart",
            Mode::Local2P => "P1 W/S   P2 Up/Down  R restart",
            Mode::Online => "W/S or Up/Down move  first to 7",
        }
    }

    fn mode_label(&self) -> String {
        match self.mode {
            Mode::VsAi => "VS COMPUTER".to_string(),
            Mode::Local2P => "LOCAL 2P".to_string(),
            Mode::Online => {
                let name = self
                    .hs
                    .as_ref()
                    .map(|hs| hs.opponent_name.clone())
                    .unwrap_or_default();
                if name.is_empty() {
                    "ONLINE".to_string()
                } else {
                    format!("ONLINE vs {}", name)
                }
            }
        }
    }
}

impl Game for PongGame {
    fn tick(&mut self, dt: f32) {
        // handshake keepalive (invite/join re-sends while waiting)
        let hs_ready = self.hs.as_ref().is_some_and(Handshake::is_ready);
        if let Some(hs) = &mut self.hs {
            if let Some(payload) = hs.tick(dt) {
                self.net.send(GAME, payload);
            }
        }
        // Once locked, both sides prove presence ~1Hz and treat sustained
        // inbound silence as a dead peer instead of freezing forever.
        if hs_ready {
            let (ka_due, timed_out) = self.liveness.tick(dt);
            if timed_out {
                self.abort_online("connection lost — opponent gone");
                return;
            }
            if ka_due {
                self.net.send(GAME, json!({"t": "ka"}));
            }
        }
        self.decay_holds(dt);

        match self.phase {
            Phase::Menu | Phase::Lobby => {}
            Phase::Serve => {
                if self.authority() {
                    self.sim_paddles(dt);
                    self.serve_cd -= dt;
                    if self.serve_cd <= 0.0 {
                        self.serve_cd = 0.0;
                        self.phase = Phase::Play;
                    }
                } else {
                    let dir = self.human_dir(self.my_side);
                    self.move_paddle(self.my_side, dir * PAD_SPEED, dt);
                    self.send_guest_input(dt);
                }
            }
            Phase::Play => {
                if self.authority() {
                    self.sim_paddles(dt);
                    self.sim_ball(dt);
                } else {
                    let dir = self.human_dir(self.my_side);
                    self.move_paddle(self.my_side, dir * PAD_SPEED, dt);
                    self.extrapolate_ball(dt);
                    self.send_guest_input(dt);
                }
            }
            Phase::Over => {}
        }

        self.broadcast_state(dt);
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if self.phase == Phase::Menu {
            return match key.code {
                KeyCode::Char('1') => {
                    self.mode = Mode::VsAi;
                    self.role = NetRole::Local;
                    self.menu_error = None;
                    self.reset_match(1.0);
                    true
                }
                KeyCode::Char('2') => {
                    self.mode = Mode::Local2P;
                    self.role = NetRole::Local;
                    self.menu_error = None;
                    self.reset_match(1.0);
                    true
                }
                KeyCode::Char('3') => {
                    self.enter_online(true);
                    true
                }
                KeyCode::Char('4') => {
                    self.enter_online(false);
                    true
                }
                _ => false,
            };
        }

        match key.code {
            KeyCode::Char('w') | KeyCode::Char('W') => {
                let side = if self.mode == Mode::Online { self.my_side } else { 0 };
                self.press(side, true);
                true
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                let side = if self.mode == Mode::Online { self.my_side } else { 0 };
                self.press(side, false);
                true
            }
            KeyCode::Up => {
                let side = match self.mode {
                    Mode::VsAi => 0,
                    Mode::Local2P => 1,
                    Mode::Online => self.my_side,
                };
                self.press(side, true);
                true
            }
            KeyCode::Down => {
                let side = match self.mode {
                    Mode::VsAi => 0,
                    Mode::Local2P => 1,
                    Mode::Online => self.my_side,
                };
                self.press(side, false);
                true
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if self.phase == Phase::Lobby {
                    return false; // let the panel recreate the session
                }
                if self.authority() {
                    let dir = if rand::thread_rng().gen_bool(0.5) { 1.0 } else { -1.0 };
                    self.reset_match(dir);
                }
                // guest: consumed, host drives the rematch
                true
            }
            _ => false,
        }
    }

    fn render(&self, buffer: &mut Buffer, area: Rect) {
        let theme = t().clone();
        let mut grid = CellGrid::new(area.width, area.height, theme.panel_bg, theme.text);

        if area.width < 26 || area.height < 10 {
            grid.center_text(1, "PONG", theme.accent2, theme.panel_bg);
            grid.center_text(3, "Grow this tile to play.", theme.muted, theme.panel_bg);
            grid.present(buffer, area);
            return;
        }

        if self.phase == Phase::Menu {
            draw_mode_menu(&mut grid, "P O N G", self.menu_error.as_deref(), &theme);
            grid.present(buffer, area);
            return;
        }

        if self.phase == Phase::Lobby {
            let mid = area.height as i32 / 2;
            match self.role {
                NetRole::Host => {
                    grid.center_text(mid - 1, "ROOM OPEN", theme.accent2, theme.panel_bg);
                    grid.center_text(
                        mid + 1,
                        "waiting for a challenger...",
                        theme.text,
                        theme.panel_bg,
                    );
                    grid.center_text(
                        mid + 3,
                        "they run /games pong and press 4",
                        theme.muted,
                        theme.panel_bg,
                    );
                }
                _ => {
                    grid.center_text(mid - 1, "SEARCHING FOR HOST", theme.accent2, theme.panel_bg);
                    grid.center_text(
                        mid + 1,
                        "waiting for a start signal...",
                        theme.text,
                        theme.panel_bg,
                    );
                    grid.center_text(
                        mid + 3,
                        "a host must open /games pong and press 3",
                        theme.muted,
                        theme.panel_bg,
                    );
                }
            }
            grid.center_text(
                area.height as i32 - 1,
                "Esc back to arcade",
                theme.muted,
                theme.panel_bg,
            );
            grid.present(buffer, area);
            return;
        }

        // HUD
        let score_line = format!("{:>2}  :  {:<2}", self.scores[0], self.scores[1]);
        grid.center_text(0, &score_line, theme.accent2, theme.panel_bg);
        grid.text(0, 0, &self.mode_label(), theme.accent4, theme.panel_bg);
        grid.text(0, 1, self.mode_hint(), theme.muted, theme.panel_bg);

        let field_top = 2i32;
        let field_h = area.height.saturating_sub(field_top as u16);
        let px = |x: f32| project_axis(x, PONG_W, area.width);
        let py = |y: f32| project_axis(y, PONG_H, field_h) + field_top;

        // dashed center line
        let cx = px(PONG_W / 2.0);
        let mut row = field_top;
        while row < area.height as i32 {
            grid.set(cx, row, ':', theme.muted, theme.panel_bg);
            row += 2;
        }

        // paddles
        for side in 0..2 {
            let x = if side == 0 { px(PAD_X0) } else { px(PAD_X1) };
            let top = py(self.py[side] - PAD_HALF);
            let bottom = py(self.py[side] + PAD_HALF);
            let color = if side == 0 { theme.accent2 } else { theme.accent3 };
            for y in top..=bottom {
                grid.set(x, y, '#', color, theme.panel_bg);
            }
        }

        // ball trail + ball
        if self.phase == Phase::Play {
            for (tx, ty) in self.trail.iter().take(self.trail.len().saturating_sub(1)) {
                grid.set(px(*tx), py(*ty), '.', theme.muted, theme.panel_bg);
            }
        }
        if self.phase != Phase::Over {
            grid.set(px(self.bx), py(self.by), 'O', theme.accent4, theme.panel_bg);
        }

        let mid = area.height as i32 / 2;
        match self.phase {
            Phase::Serve => {
                grid.center_text(mid - 1, &format!("FIRST TO {}", WIN_SCORE), theme.muted, theme.panel_bg);
                grid.center_text(
                    mid + 1,
                    &format!("SERVE IN {}", self.serve_cd.ceil().max(1.0) as u32),
                    theme.accent2,
                    theme.panel_bg,
                );
            }
            Phase::Over => {
                grid.center_text(mid - 1, &self.over_msg, theme.accent2, theme.panel_bg);
                let hint = if self.role == NetRole::Guest {
                    "host presses R for rematch  Esc menu"
                } else {
                    "R rematch  Esc menu"
                };
                grid.center_text(mid + 1, hint, theme.text, theme.panel_bg);
            }
            _ => {}
        }

        grid.present(buffer, area);
    }

    fn status(&self) -> Option<String> {
        Some(match self.phase {
            Phase::Menu => "pong: choose a mode (1-4)".to_string(),
            Phase::Lobby => match self.role {
                NetRole::Host => "pong: room open — waiting for a challenger".to_string(),
                _ => "pong: searching for a host".to_string(),
            },
            _ => format!("pong: {} - {} ({})", self.scores[0], self.scores[1], self.mode_label()),
        })
    }

    fn handle_net(&mut self, from_id: &str, from_name: &str, payload: &Value) {
        let mut event = None;
        let mut reply = None;
        if let Some(hs) = &mut self.hs {
            let (ev, rep) = hs.on_net(from_id, from_name, payload);
            event = ev;
            reply = rep;
        }
        if let Some(reply) = reply {
            self.net.send(GAME, reply);
        }
        match event {
            Some(HsEvent::Started { seed }) => {
                self.begin_online_match(seed);
                return;
            }
            Some(HsEvent::OpponentLeft) => {
                self.abort_online("opponent left the match");
                return;
            }
            Some(HsEvent::Full) => {
                self.abort_online("match is full — that host already has a challenger");
                return;
            }
            None => {}
        }

        let is_opponent = self
            .hs
            .as_ref()
            .is_some_and(|hs| hs.is_ready() && hs.is_opponent(from_id));
        if !is_opponent {
            return;
        }
        // Any payload from the locked opponent (state, input, ka) proves life.
        self.liveness.on_inbound();
        match serde_json::from_value::<PongMsg>(payload.clone()) {
            Ok(PongMsg::Input { y }) if self.role == NetRole::Host => {
                self.guest_target = Self::clamp_paddle(y);
            }
            Ok(PongMsg::State {
                bx,
                by,
                vx,
                vy,
                p0,
                p1,
                s0,
                s1,
                ph,
                cd,
                mg,
            }) if self.role == NetRole::Guest => {
                self.apply_state(bx, by, vx, vy, p0, p1, s0, s1, ph, cd, mg);
            }
            _ => {}
        }
    }

    fn peer_disconnected(&mut self, from_id: Option<&str>) {
        let Some(hs) = &self.hs else { return };
        let ends = match from_id {
            None => true, // we lost our own connection: no online play possible
            Some(id) => hs.opponent_id.as_deref() == Some(id),
        };
        if ends {
            self.abort_online(match from_id {
                None => "disconnected — online match ended",
                Some(_) => "opponent left the match",
            });
        }
    }
}

impl Drop for PongGame {
    fn drop(&mut self) {
        if self.hs.is_some() {
            self.net.send(GAME, json!({"t": "quit"}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounce_reflects_and_speeds_up() {
        let (vx, vy) = paddle_bounce(BALL_SPEED, 0.0, 0.0, 1.0);
        assert!(vx > 0.0, "ball must travel in the requested direction");
        assert!(vy.abs() < 1e-4, "center hit with still paddle has no deflection");
        let speed = (vx * vx + vy * vy).sqrt();
        assert!(speed > BALL_SPEED, "bounce speeds the ball up");

        let (vx, _) = paddle_bounce(BALL_SPEED, 0.0, 0.0, -1.0);
        assert!(vx < 0.0);
    }

    #[test]
    fn bounce_deflects_by_contact_point() {
        let (_, vy_top) = paddle_bounce(BALL_SPEED, -1.0, 0.0, 1.0);
        let (_, vy_bottom) = paddle_bounce(BALL_SPEED, 1.0, 0.0, 1.0);
        assert!(vy_top < 0.0, "top-edge hit deflects upward");
        assert!(vy_bottom > 0.0, "bottom-edge hit deflects downward");
        assert!((vy_top + vy_bottom).abs() < 1e-4, "deflection is symmetric");
    }

    #[test]
    fn bounce_adds_spin_from_paddle_motion() {
        let (_, still) = paddle_bounce(BALL_SPEED, 0.3, 0.0, 1.0);
        let (_, moving) = paddle_bounce(BALL_SPEED, 0.3, PAD_SPEED, 1.0);
        assert!(moving > still, "a downward-moving paddle adds downward spin");
        assert!((moving - still - PAD_SPEED * SPIN).abs() < 1e-4);
    }

    #[test]
    fn bounce_speed_is_capped() {
        let (vx, vy) = paddle_bounce(MAX_BALL_SPEED * 2.0, 0.5, 0.0, 1.0);
        let speed = (vx * vx + vy * vy).sqrt();
        assert!(speed <= MAX_BALL_SPEED + PAD_SPEED * SPIN + 1e-3);
    }

    #[test]
    fn msg_serde_roundtrip_every_variant() {
        let msgs = vec![
            PongMsg::Invite,
            PongMsg::Join,
            PongMsg::Start { seed: 99 },
            PongMsg::Full,
            PongMsg::Ka,
            PongMsg::Quit,
            PongMsg::Input { y: 12.5 },
            PongMsg::State {
                bx: 1.0,
                by: 2.0,
                vx: -3.0,
                vy: 4.0,
                p0: 5.0,
                p1: 6.0,
                s0: 3,
                s1: 4,
                ph: 2,
                cd: 0.5,
                mg: 2,
            },
        ];
        for msg in msgs {
            let value = serde_json::to_value(&msg).unwrap();
            let back: PongMsg = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(back, msg);
            assert!(value.get("t").is_some(), "every message carries a t tag");
        }
        // tags match the handshake payloads used on the wire
        assert_eq!(
            serde_json::to_value(&PongMsg::Invite).unwrap(),
            json!({"t": "invite"})
        );
        assert_eq!(
            serde_json::to_value(&PongMsg::Start { seed: 7 }).unwrap(),
            json!({"t": "start", "seed": 7})
        );
    }

    #[test]
    fn guest_applies_host_state_but_keeps_own_paddle() {
        let mut game = PongGame::new(NetHandle::new());
        game.role = NetRole::Guest;
        game.mode = Mode::Online;
        game.my_side = 1;
        game.py[1] = 30.0;
        game.phase = Phase::Play;
        let mg = game.match_gen;

        game.apply_state(10.0, 11.0, 5.0, -4.0, 8.0, 20.0, 2, 3, 2, 0.0, mg);
        assert_eq!(game.bx, 10.0);
        assert_eq!(game.by, 11.0);
        assert_eq!(game.py[0], 8.0, "opponent paddle comes from the host");
        assert_eq!(game.py[1], 30.0, "own paddle stays locally predicted");
        assert_eq!(game.scores, [2, 3]);
        assert_eq!(game.phase, Phase::Play);

        game.apply_state(10.0, 11.0, 5.0, -4.0, 8.0, 20.0, 7, 3, 3, 0.0, mg);
        assert_eq!(game.phase, Phase::Over);
        assert!(!game.over_msg.is_empty());
    }

    #[test]
    fn rematch_generation_recenters_guest_paddle() {
        let mut game = PongGame::new(NetHandle::new());
        game.role = NetRole::Guest;
        game.mode = Mode::Online;
        game.my_side = 1;
        game.begin_online_match(0);
        let mg = game.match_gen;
        game.phase = Phase::Over;
        game.py[1] = 30.0;
        game.last_sent_y = 30.0;

        // Host pressed R: snapshot arrives with a bumped generation.
        game.apply_state(36.0, 18.0, 20.0, 0.0, 18.0, 18.0, 0, 0, 1, 2.2, mg + 1);
        assert_eq!(game.match_gen, mg + 1);
        assert_eq!(
            game.py[1],
            PONG_H / 2.0,
            "guest's locally-predicted paddle re-centers on rematch"
        );
        assert_eq!(game.last_sent_y, PONG_H / 2.0);
        assert_eq!(game.phase, Phase::Serve);

        // Same generation again: own paddle stays locally predicted.
        game.py[1] = 12.0;
        game.apply_state(36.0, 18.0, 20.0, 0.0, 18.0, 18.0, 0, 0, 2, 0.0, mg + 1);
        assert_eq!(game.py[1], 12.0);
    }

    #[test]
    fn fast_ball_cannot_tunnel_through_paddles() {
        // Max ball speed at the max clamped frame dt used to step 2.75 cells,
        // clean through the 2.0-cell paddle window. Substepping must catch it.
        let mut game = PongGame::new(NetHandle::new());
        game.mode = Mode::Local2P;
        game.reset_match(1.0);
        game.phase = Phase::Play;

        // toward the left paddle
        game.bx = PAD_X0 + 1.1;
        game.by = game.py[0];
        game.vx = -MAX_BALL_SPEED;
        game.vy = 0.0;
        game.sim_ball(0.05);
        assert!(game.vx > 0.0, "left paddle must bounce a max-speed ball");
        assert_eq!(game.scores, [0, 0]);

        // toward the right paddle
        game.phase = Phase::Play;
        game.bx = PAD_X1 - 1.1;
        game.by = game.py[1];
        game.vx = MAX_BALL_SPEED;
        game.vy = 0.0;
        game.sim_ball(0.05);
        assert!(game.vx < 0.0, "right paddle must bounce a max-speed ball");
        assert_eq!(game.scores, [0, 0]);
    }

    #[test]
    fn ready_guest_times_out_after_peer_silence() {
        let mut game = PongGame::new(NetHandle::new());
        game.mode = Mode::Online;
        game.role = NetRole::Guest;
        game.my_side = 1;
        let (mut hs, _) = Handshake::guest();
        hs.on_net("host-id", "HOSTY", &json!({"t": "start", "seed": 2}));
        game.hs = Some(hs);
        game.begin_online_match(2);

        // 4s of silence, then a keepalive from the host: no timeout.
        for _ in 0..80 {
            game.tick(0.05);
        }
        assert_ne!(game.phase, Phase::Menu, "alive before the timeout");
        game.handle_net("host-id", "HOSTY", &json!({"t": "ka"}));
        for _ in 0..80 {
            game.tick(0.05);
        }
        assert_ne!(game.phase, Phase::Menu, "inbound traffic resets the clock");

        // Silence past the threshold ends the session gracefully.
        for _ in 0..30 {
            game.tick(0.05);
        }
        assert_eq!(game.phase, Phase::Menu);
        assert!(game.hs.is_none());
        assert!(game
            .menu_error
            .as_deref()
            .unwrap_or_default()
            .contains("connection lost"));
    }

    #[test]
    fn ready_peer_sends_keepalives() {
        let handle = NetHandle::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        handle.configure(Some(tx), Some("me".to_string()));
        let mut game = PongGame::new(handle);
        game.mode = Mode::Online;
        game.role = NetRole::Guest;
        game.my_side = 1;
        let (mut hs, _) = Handshake::guest();
        hs.on_net("host-id", "HOSTY", &json!({"t": "start", "seed": 2}));
        game.hs = Some(hs);
        game.begin_online_match(2);

        for _ in 0..24 {
            game.tick(0.05); // 1.2s
        }
        let mut kas = 0;
        while let Ok((game_name, payload)) = rx.try_recv() {
            assert_eq!(game_name, "pong");
            if payload == json!({"t": "ka"}) {
                kas += 1;
            }
        }
        assert!(kas >= 1, "a ready peer keepalives about once per second");
    }

    #[test]
    fn full_rejection_returns_guest_to_menu() {
        let mut game = PongGame::new(NetHandle::new());
        game.mode = Mode::Online;
        game.role = NetRole::Guest;
        game.my_side = 1;
        game.phase = Phase::Lobby;
        let (hs, _) = Handshake::guest();
        game.hs = Some(hs);

        game.handle_net("host-id", "HOSTY", &json!({"t": "full"}));
        assert_eq!(game.phase, Phase::Menu);
        assert!(game.hs.is_none());
        assert!(game
            .menu_error
            .as_deref()
            .unwrap_or_default()
            .contains("full"));
    }

    #[test]
    fn peer_disconnected_ends_only_matching_sessions() {
        // Opponent's id ends the match.
        let mut game = PongGame::new(NetHandle::new());
        game.mode = Mode::Online;
        game.role = NetRole::Guest;
        game.my_side = 1;
        let (mut hs, _) = Handshake::guest();
        hs.on_net("host-id", "HOSTY", &json!({"t": "start", "seed": 2}));
        game.hs = Some(hs);
        game.begin_online_match(2);

        game.peer_disconnected(Some("someone-else"));
        assert_ne!(game.phase, Phase::Menu, "unrelated peers leaving is ignored");
        game.peer_disconnected(Some("host-id"));
        assert_eq!(game.phase, Phase::Menu);
        assert!(game.hs.is_none());

        // None (local disconnect) ends any online session, even a waiting lobby.
        let mut game = PongGame::new(NetHandle::new());
        game.mode = Mode::Online;
        game.role = NetRole::Host;
        let (hs, _) = Handshake::host(1);
        game.hs = Some(hs);
        game.phase = Phase::Lobby;
        game.peer_disconnected(None);
        assert_eq!(game.phase, Phase::Menu);
        assert!(game.hs.is_none());

        // Local games are unaffected.
        let mut game = PongGame::new(NetHandle::new());
        game.mode = Mode::VsAi;
        game.reset_match(1.0);
        game.peer_disconnected(None);
        assert_eq!(game.phase, Phase::Serve);
    }

    #[test]
    fn scoring_ends_match_at_win_score() {
        let mut game = PongGame::new(NetHandle::new());
        game.mode = Mode::Local2P;
        game.reset_match(1.0);
        for _ in 0..WIN_SCORE - 1 {
            game.score_point(0);
        }
        assert_eq!(game.phase, Phase::Serve, "match continues before 7");
        game.score_point(0);
        assert_eq!(game.phase, Phase::Over);
        assert_eq!(game.scores[0], WIN_SCORE);
        assert!(game.over_msg.contains("PLAYER 1"));
    }

    #[test]
    fn ball_scores_when_leaving_field() {
        let mut game = PongGame::new(NetHandle::new());
        game.mode = Mode::Local2P;
        game.reset_match(1.0);
        game.phase = Phase::Play;
        game.bx = PONG_W + 1.0;
        game.by = PONG_H / 2.0;
        game.vx = 40.0;
        game.vy = 0.0;
        game.py[1] = 1000.0; // paddle far away (clamped, but ball is past it anyway)
        game.sim_ball(0.05);
        assert_eq!(game.scores[0], 1, "left player scores when ball exits right");
    }

    #[test]
    fn ball_bounces_off_paddle_in_sim() {
        let mut game = PongGame::new(NetHandle::new());
        game.mode = Mode::VsAi;
        game.reset_match(1.0);
        game.phase = Phase::Play;
        game.trail.clear();
        game.bx = PAD_X0 + 1.2;
        game.by = game.py[0]; // dead center of left paddle
        game.vx = -20.0;
        game.vy = 0.0;
        game.sim_ball(0.05);
        assert!(game.vx > 0.0, "ball reflected off the left paddle");
        assert_eq!(game.scores, [0, 0]);
    }
}
