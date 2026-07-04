//! Tron light cycles: vs-AI, local 2P, and online host/join (host-authoritative).
//!
//! Wire protocol (payloads inside the app's Game envelope, game = "tron"):
//!   {"t":"invite"} {"t":"join"} {"t":"start","seed":u64} {"t":"quit"}   handshake
//!   {"t":"input","d":u8}                       guest -> host (on direction change)
//!   {"t":"state", x0,y0,d0,a0, x1,y1,d1,a1, w0,w1, rnd, ph, cd}
//!       host -> guest, once per simulation step (grid step ~12Hz) plus a slow
//!       idle tick outside Play. Guests extend trails locally from the head
//!       positions; the host owns all collisions. `rnd` bumps reset trails.
//! d/dir codes: 0=up 1=right 2=down 3=left. ph: 0=lobby 1=countdown 2=play
//! 3=round-over 4=match-over.

use crossterm::event::{KeyCode, KeyEvent};
use rand::{prelude::SliceRandom, Rng};
use ratatui::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

use super::net::{Handshake, HsEvent, NetHandle, NetRole, Throttle};
use super::{draw_mode_menu, CellGrid, Dir, Game};
use crate::theme::t;

const GAME: &str = "tron";
const TRON_W: i32 = 56;
const TRON_H: i32 = 30;
const STEP_SECS: f32 = 0.085;
const ROUND_WINS: u32 = 3;
const ROUND_OVER_SECS: f32 = 1.8;
const COUNTDOWN_SECS: f32 = 1.5;
const IDLE_STATE_HZ: f32 = 12.0;

/// A cell is fatal when it is outside the arena or already occupied by a trail.
pub(super) fn tron_collides(cell: (i32, i32), occupied: &HashSet<(i32, i32)>) -> bool {
    cell.0 < 0 || cell.1 < 0 || cell.0 >= TRON_W || cell.1 >= TRON_H || occupied.contains(&cell)
}

fn free_dist(from: (i32, i32), dir: Dir, occupied: &HashSet<(i32, i32)>, max: i32) -> i32 {
    let (dx, dy) = dir.delta();
    let mut cell = from;
    let mut dist = 0;
    while dist < max {
        cell = (cell.0 + dx, cell.1 + dy);
        if tron_collides(cell, occupied) {
            break;
        }
        dist += 1;
    }
    dist
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "t", rename_all = "lowercase")]
pub(super) enum TronMsg {
    Invite,
    Join,
    Start { seed: u64 },
    Quit,
    Input { d: u8 },
    State {
        x0: i32,
        y0: i32,
        d0: u8,
        a0: bool,
        x1: i32,
        y1: i32,
        d1: u8,
        a1: bool,
        w0: u32,
        w1: u32,
        rnd: u32,
        ph: u8,
        cd: f32,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Menu,
    Lobby,
    Countdown,
    Play,
    RoundOver,
    MatchOver,
}

impl Phase {
    fn code(self) -> u8 {
        match self {
            Self::Menu | Self::Lobby => 0,
            Self::Countdown => 1,
            Self::Play => 2,
            Self::RoundOver => 3,
            Self::MatchOver => 4,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Countdown,
            2 => Self::Play,
            3 => Self::RoundOver,
            4 => Self::MatchOver,
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

struct Cycle {
    x: i32,
    y: i32,
    dir: Dir,
    alive: bool,
    trail: Vec<(i32, i32)>,
}

impl Cycle {
    fn spawn(x: i32, y: i32, dir: Dir) -> Self {
        Self {
            x,
            y,
            dir,
            alive: true,
            trail: vec![(x, y)],
        }
    }
}

pub(super) struct TronGame {
    net: NetHandle,
    mode: Mode,
    role: NetRole,
    hs: Option<Handshake>,
    phase: Phase,
    menu_error: Option<String>,
    cycles: [Cycle; 2],
    occupied: HashSet<(i32, i32)>,
    pending: [Option<Dir>; 2],
    wins: [u32; 2],
    round: u32,
    acc: f32,
    cd: f32,
    banner: String,
    my_side: usize,
    last_sent_dir: Option<Dir>,
    idle_throttle: Throttle,
}

impl TronGame {
    pub(super) fn new(net: NetHandle) -> Self {
        Self {
            net,
            mode: Mode::VsAi,
            role: NetRole::Local,
            hs: None,
            phase: Phase::Menu,
            menu_error: None,
            cycles: [
                Cycle::spawn(TRON_W / 4, TRON_H / 2, Dir::Right),
                Cycle::spawn(3 * TRON_W / 4, TRON_H / 2, Dir::Left),
            ],
            occupied: HashSet::new(),
            pending: [None, None],
            wins: [0; 2],
            round: 1,
            acc: 0.0,
            cd: 0.0,
            banner: String::new(),
            my_side: 0,
            last_sent_dir: None,
            idle_throttle: Throttle::new(IDLE_STATE_HZ),
        }
    }

    fn authority(&self) -> bool {
        self.role != NetRole::Guest
    }

    fn reset_round(&mut self) {
        self.cycles = [
            Cycle::spawn(TRON_W / 4, TRON_H / 2, Dir::Right),
            Cycle::spawn(3 * TRON_W / 4, TRON_H / 2, Dir::Left),
        ];
        self.occupied.clear();
        for cycle in &self.cycles {
            self.occupied.insert((cycle.x, cycle.y));
        }
        self.pending = [None, None];
        self.acc = 0.0;
        self.cd = COUNTDOWN_SECS;
        self.phase = Phase::Countdown;
    }

    fn reset_match(&mut self) {
        self.wins = [0; 2];
        self.round = 1;
        self.banner.clear();
        self.reset_round();
    }

    fn begin_online_match(&mut self, _seed: u64) {
        if self.authority() {
            self.reset_match();
        } else {
            // guest is fully state-driven; round 0 forces a trail reset on the
            // first snapshot
            self.round = 0;
            self.wins = [0; 2];
            self.phase = Phase::Countdown;
            self.cd = COUNTDOWN_SECS;
        }
    }

    fn abort_online(&mut self, message: &str) {
        self.hs = None;
        self.role = NetRole::Local;
        self.phase = Phase::Menu;
        self.menu_error = Some(message.to_string());
    }

    fn side_name(&self, side: usize) -> String {
        match self.mode {
            Mode::VsAi => {
                if side == 0 {
                    "YOU".to_string()
                } else {
                    "CPU".to_string()
                }
            }
            Mode::Local2P => format!("P{}", side + 1),
            Mode::Online => {
                if side == self.my_side {
                    "YOU".to_string()
                } else {
                    let name = self
                        .hs
                        .as_ref()
                        .map(|hs| hs.opponent_name.clone())
                        .unwrap_or_default();
                    if name.is_empty() {
                        "RIVAL".to_string()
                    } else {
                        name.to_uppercase()
                    }
                }
            }
        }
    }

    fn queue_dir(&mut self, side: usize, dir: Dir) {
        if matches!(self.phase, Phase::Countdown | Phase::Play) {
            self.pending[side] = Some(dir);
        }
    }

    fn ai_dir(&self) -> Option<Dir> {
        let cycle = &self.cycles[1];
        if !cycle.alive {
            return None;
        }
        let from = (cycle.x, cycle.y);
        let ahead = free_dist(from, cycle.dir, &self.occupied, 6);
        let mut rng = rand::thread_rng();
        let turns: Vec<Dir> = Dir::ALL
            .iter()
            .copied()
            .filter(|dir| *dir != cycle.dir && *dir != cycle.dir.opposite())
            .collect();
        // occasionally weave when it is safe, otherwise ride straight
        if ahead >= 3 {
            if rng.gen_bool(0.05) {
                let safe: Vec<Dir> = turns
                    .iter()
                    .copied()
                    .filter(|dir| free_dist(from, *dir, &self.occupied, 6) >= 3)
                    .collect();
                return safe.choose(&mut rng).copied();
            }
            return None;
        }
        // danger ahead: take the most open turn
        let mut best: Option<(Dir, i32)> = None;
        for dir in turns {
            let dist = free_dist(from, dir, &self.occupied, 8);
            if best.map(|(_, d)| dist > d).unwrap_or(true) {
                best = Some((dir, dist));
            }
        }
        match best {
            Some((dir, dist)) if dist > ahead => Some(dir),
            _ => None,
        }
    }

    fn step(&mut self) {
        if self.mode == Mode::VsAi {
            if let Some(dir) = self.ai_dir() {
                self.pending[1] = Some(dir);
            }
        }
        for side in 0..2 {
            if let Some(dir) = self.pending[side].take() {
                if dir != self.cycles[side].dir.opposite() {
                    self.cycles[side].dir = dir;
                }
            }
        }

        let mut next = [(0, 0); 2];
        for side in 0..2 {
            let cycle = &self.cycles[side];
            let (dx, dy) = cycle.dir.delta();
            next[side] = (cycle.x + dx, cycle.y + dy);
        }

        let mut died = [false, false];
        for side in 0..2 {
            if self.cycles[side].alive && tron_collides(next[side], &self.occupied) {
                died[side] = true;
            }
        }
        // head-on into the same cell: both crash
        if self.cycles[0].alive && self.cycles[1].alive && next[0] == next[1] {
            died = [true, true];
        }

        for side in 0..2 {
            if !self.cycles[side].alive {
                continue;
            }
            if died[side] {
                self.cycles[side].alive = false;
                continue;
            }
            let cycle = &mut self.cycles[side];
            cycle.x = next[side].0;
            cycle.y = next[side].1;
            cycle.trail.push(next[side]);
            self.occupied.insert(next[side]);
        }

        if died[0] || died[1] {
            self.end_round(died);
        }
    }

    fn end_round(&mut self, died: [bool; 2]) {
        let winner = match died {
            [true, true] => None,
            [true, false] => Some(1),
            [false, true] => Some(0),
            [false, false] => None,
        };
        match winner {
            Some(side) => {
                self.wins[side] += 1;
                self.banner = format!(
                    "{} CRASHED — {} TAKES THE ROUND",
                    self.side_name(1 - side),
                    self.side_name(side)
                );
                if self.wins[side] >= ROUND_WINS {
                    self.banner = format!("{} WINS THE MATCH", self.side_name(side));
                    self.phase = Phase::MatchOver;
                    return;
                }
            }
            None => {
                self.banner = "HEAD-ON — DRAW ROUND".to_string();
            }
        }
        self.phase = Phase::RoundOver;
        self.cd = ROUND_OVER_SECS;
    }

    fn make_state(&self) -> TronMsg {
        TronMsg::State {
            x0: self.cycles[0].x,
            y0: self.cycles[0].y,
            d0: self.cycles[0].dir.code(),
            a0: self.cycles[0].alive,
            x1: self.cycles[1].x,
            y1: self.cycles[1].y,
            d1: self.cycles[1].dir.code(),
            a1: self.cycles[1].alive,
            w0: self.wins[0],
            w1: self.wins[1],
            rnd: self.round,
            ph: self.phase.code(),
            cd: self.cd,
        }
    }

    fn broadcast_state(&mut self) {
        if self.role != NetRole::Host || !self.hs.as_ref().is_some_and(Handshake::is_ready) {
            return;
        }
        if let Ok(payload) = serde_json::to_value(self.make_state()) {
            self.net.send(GAME, payload);
        }
    }

    /// Guest-side snapshot application: extend trails from head movement,
    /// reset them when the round index bumps. Host owns all collisions.
    #[allow(clippy::too_many_arguments)]
    fn apply_state(
        &mut self,
        heads: [(i32, i32); 2],
        dirs: [u8; 2],
        alive: [bool; 2],
        wins: [u32; 2],
        rnd: u32,
        ph: u8,
        cd: f32,
    ) {
        if rnd != self.round {
            // new round: rebuild trails from the received heads
            self.round = rnd;
            self.occupied.clear();
            for side in 0..2 {
                self.cycles[side] = Cycle::spawn(
                    heads[side].0,
                    heads[side].1,
                    Dir::from_code(dirs[side]).unwrap_or(Dir::Right),
                );
                self.occupied.insert(heads[side]);
            }
        } else {
            for side in 0..2 {
                let cycle = &mut self.cycles[side];
                if let Some(dir) = Dir::from_code(dirs[side]) {
                    cycle.dir = dir;
                }
                let old = (cycle.x, cycle.y);
                let new = heads[side];
                if new != old {
                    // walk any gap in a straight line so trails stay contiguous
                    let steps = (new.0 - old.0).abs().max((new.1 - old.1).abs());
                    let sx = (new.0 - old.0).signum();
                    let sy = (new.1 - old.1).signum();
                    for i in 1..=steps {
                        let cell = (old.0 + sx * i, old.1 + sy * i);
                        cycle.trail.push(cell);
                        self.occupied.insert(cell);
                    }
                    cycle.x = new.0;
                    cycle.y = new.1;
                }
            }
        }
        for side in 0..2 {
            self.cycles[side].alive = alive[side];
        }
        let old_phase = self.phase;
        self.wins = wins;
        self.cd = cd;
        self.phase = Phase::from_code(ph);
        if self.phase != old_phase
            && matches!(self.phase, Phase::RoundOver | Phase::MatchOver)
        {
            let died = [!self.cycles[0].alive, !self.cycles[1].alive];
            let winner = match died {
                [true, true] => None,
                [true, false] => Some(1),
                [false, true] => Some(0),
                [false, false] => None,
            };
            self.banner = match (self.phase, winner) {
                (Phase::MatchOver, Some(side)) => {
                    format!("{} WINS THE MATCH", self.side_name(side))
                }
                (_, Some(side)) => format!(
                    "{} CRASHED — {} TAKES THE ROUND",
                    self.side_name(1 - side),
                    self.side_name(side)
                ),
                _ => "HEAD-ON — DRAW ROUND".to_string(),
            };
        }
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

    fn send_guest_dir(&mut self, dir: Dir) {
        if self.role != NetRole::Guest || self.last_sent_dir == Some(dir) {
            return;
        }
        self.last_sent_dir = Some(dir);
        if let Ok(payload) = serde_json::to_value(&TronMsg::Input { d: dir.code() }) {
            self.net.send(GAME, payload);
        }
    }
}

impl Game for TronGame {
    fn tick(&mut self, dt: f32) {
        if let Some(hs) = &mut self.hs {
            if let Some(payload) = hs.tick(dt) {
                self.net.send(GAME, payload);
            }
        }

        match self.phase {
            Phase::Menu | Phase::Lobby => {}
            Phase::Countdown => {
                self.cd -= dt;
                if self.authority() && self.cd <= 0.0 {
                    self.cd = 0.0;
                    self.phase = Phase::Play;
                }
            }
            Phase::Play => {
                if self.authority() {
                    self.acc += dt;
                    while self.acc >= STEP_SECS {
                        self.acc -= STEP_SECS;
                        self.step();
                        self.broadcast_state(); // per-tick head broadcast
                        if self.phase != Phase::Play {
                            break;
                        }
                    }
                }
            }
            Phase::RoundOver => {
                self.cd -= dt;
                if self.authority() && self.cd <= 0.0 {
                    self.round += 1;
                    self.reset_round();
                }
            }
            Phase::MatchOver => {}
        }

        // slow keepalive broadcast outside the per-step path
        if self.phase != Phase::Play && self.idle_throttle.ready(dt) {
            self.broadcast_state();
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if self.phase == Phase::Menu {
            return match key.code {
                KeyCode::Char('1') => {
                    self.mode = Mode::VsAi;
                    self.role = NetRole::Local;
                    self.menu_error = None;
                    self.reset_match();
                    true
                }
                KeyCode::Char('2') => {
                    self.mode = Mode::Local2P;
                    self.role = NetRole::Local;
                    self.menu_error = None;
                    self.reset_match();
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

        // direction inputs
        let wasd = match key.code {
            KeyCode::Char('w') | KeyCode::Char('W') => Some(Dir::Up),
            KeyCode::Char('s') | KeyCode::Char('S') => Some(Dir::Down),
            KeyCode::Char('a') | KeyCode::Char('A') => Some(Dir::Left),
            KeyCode::Char('d') | KeyCode::Char('D') => Some(Dir::Right),
            _ => None,
        };
        let arrows = match key.code {
            KeyCode::Up => Some(Dir::Up),
            KeyCode::Down => Some(Dir::Down),
            KeyCode::Left => Some(Dir::Left),
            KeyCode::Right => Some(Dir::Right),
            _ => None,
        };

        if let Some(dir) = wasd {
            match self.mode {
                Mode::Online => {
                    if self.role == NetRole::Guest {
                        self.send_guest_dir(dir);
                    } else {
                        self.queue_dir(self.my_side, dir);
                    }
                }
                _ => self.queue_dir(0, dir),
            }
            return true;
        }
        if let Some(dir) = arrows {
            match self.mode {
                Mode::VsAi => self.queue_dir(0, dir),
                Mode::Local2P => self.queue_dir(1, dir),
                Mode::Online => {
                    if self.role == NetRole::Guest {
                        self.send_guest_dir(dir);
                    } else {
                        self.queue_dir(self.my_side, dir);
                    }
                }
            }
            return true;
        }

        if matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R')) {
            if self.phase == Phase::Lobby {
                return false; // let the panel recreate the session
            }
            if self.authority() {
                self.reset_match();
            }
            return true;
        }

        false
    }

    fn render(&self, buffer: &mut Buffer, area: Rect) {
        let theme = t().clone();
        let mut grid = CellGrid::new(area.width, area.height, theme.panel_bg, theme.text);

        if area.width < 26 || area.height < 10 {
            grid.center_text(1, "TRON", theme.accent2, theme.panel_bg);
            grid.center_text(3, "Grow this tile to play.", theme.muted, theme.panel_bg);
            grid.present(buffer, area);
            return;
        }

        if self.phase == Phase::Menu {
            draw_mode_menu(&mut grid, "T R O N", self.menu_error.as_deref(), &theme);
            grid.present(buffer, area);
            return;
        }

        if self.phase == Phase::Lobby {
            let mid = area.height as i32 / 2;
            let (title, hint) = match self.role {
                NetRole::Host => ("GRID OPEN", "they run /games tron and press 4"),
                _ => ("SEARCHING FOR HOST", "a host must open /games tron and press 3"),
            };
            grid.center_text(mid - 1, title, theme.accent2, theme.panel_bg);
            grid.center_text(mid + 1, "waiting...", theme.text, theme.panel_bg);
            grid.center_text(mid + 3, hint, theme.muted, theme.panel_bg);
            grid.present(buffer, area);
            return;
        }

        // HUD
        let hud = format!(
            "{} {}  -  {} {}   round {}   first to {}",
            self.side_name(0),
            self.wins[0],
            self.wins[1],
            self.side_name(1),
            self.round,
            ROUND_WINS
        );
        grid.text(0, 0, &hud, theme.accent2, theme.panel_bg);
        let hint = match self.mode {
            Mode::VsAi => "WASD or arrows steer",
            Mode::Local2P => "P1 WASD   P2 arrows",
            Mode::Online => "WASD or arrows steer  host owns physics",
        };
        grid.text(0, 1, hint, theme.muted, theme.panel_bg);

        // arena frame + play field (inset by 1 so the frame is not playable)
        let field_top = 2i32;
        let frame_h = area.height as i32 - field_top;
        let frame_w = area.width as i32;
        for x in 0..frame_w {
            grid.set(x, field_top, '-', theme.accent1, theme.panel_bg);
            grid.set(x, area.height as i32 - 1, '-', theme.accent1, theme.panel_bg);
        }
        for y in field_top..area.height as i32 {
            grid.set(0, y, '|', theme.accent1, theme.panel_bg);
            grid.set(frame_w - 1, y, '|', theme.accent1, theme.panel_bg);
        }
        grid.set(0, field_top, '+', theme.accent1, theme.panel_bg);
        grid.set(frame_w - 1, field_top, '+', theme.accent1, theme.panel_bg);
        grid.set(0, area.height as i32 - 1, '+', theme.accent1, theme.panel_bg);
        grid.set(frame_w - 1, area.height as i32 - 1, '+', theme.accent1, theme.panel_bg);

        let inner_w = (frame_w - 2).max(1) as u16;
        let inner_h = (frame_h - 2).max(1) as u16;
        let px = |x: i32| {
            1 + ((x as i64 * inner_w.saturating_sub(1) as i64)
                / (TRON_W as i64 - 1).max(1)) as i32
        };
        let py = |y: i32| {
            field_top
                + 1
                + ((y as i64 * inner_h.saturating_sub(1) as i64)
                    / (TRON_H as i64 - 1).max(1)) as i32
        };

        let colors = [theme.accent2, theme.accent3];
        let trail_glyphs = ['#', '%'];
        for side in 0..2 {
            let cycle = &self.cycles[side];
            for cell in &cycle.trail {
                grid.set(
                    px(cell.0),
                    py(cell.1),
                    trail_glyphs[side],
                    colors[side],
                    theme.panel_bg,
                );
            }
            let head_color = if cycle.alive { theme.accent4 } else { theme.danger };
            grid.set(px(cycle.x), py(cycle.y), '@', head_color, theme.panel_bg);
        }

        let mid = area.height as i32 / 2;
        match self.phase {
            Phase::Countdown => {
                grid.center_text(
                    mid,
                    &format!("ROUND {} — GO IN {}", self.round, self.cd.ceil().max(1.0) as u32),
                    theme.accent2,
                    theme.panel_bg,
                );
            }
            Phase::RoundOver => {
                grid.center_text(mid, &self.banner, theme.accent2, theme.panel_bg);
            }
            Phase::MatchOver => {
                grid.center_text(mid - 1, &self.banner, theme.accent2, theme.panel_bg);
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
            Phase::Menu => "tron: choose a mode (1-4)".to_string(),
            Phase::Lobby => match self.role {
                NetRole::Host => "tron: grid open — waiting for a challenger".to_string(),
                _ => "tron: searching for a host".to_string(),
            },
            _ => format!(
                "tron: {} {} - {} {} (round {})",
                self.side_name(0),
                self.wins[0],
                self.wins[1],
                self.side_name(1),
                self.round
            ),
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
                self.abort_online("opponent left the grid");
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
        match serde_json::from_value::<TronMsg>(payload.clone()) {
            Ok(TronMsg::Input { d }) if self.role == NetRole::Host => {
                if let Some(dir) = Dir::from_code(d) {
                    self.queue_dir(1, dir); // guest is always side 1
                }
            }
            Ok(TronMsg::State {
                x0,
                y0,
                d0,
                a0,
                x1,
                y1,
                d1,
                a1,
                w0,
                w1,
                rnd,
                ph,
                cd,
            }) if self.role == NetRole::Guest => {
                self.apply_state(
                    [(x0, y0), (x1, y1)],
                    [d0, d1],
                    [a0, a1],
                    [w0, w1],
                    rnd,
                    ph,
                    cd,
                );
            }
            _ => {}
        }
    }
}

impl Drop for TronGame {
    fn drop(&mut self) {
        if self.hs.is_some() {
            self.net.send(GAME, json!({"t": "quit"}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_game() -> TronGame {
        let mut game = TronGame::new(NetHandle::new());
        game.mode = Mode::Local2P;
        game.reset_match();
        game.phase = Phase::Play;
        game
    }

    #[test]
    fn collision_detection() {
        let mut occupied = HashSet::new();
        occupied.insert((5, 5));
        assert!(tron_collides((-1, 3), &occupied), "left wall");
        assert!(tron_collides((TRON_W, 3), &occupied), "right wall");
        assert!(tron_collides((3, -1), &occupied), "top wall");
        assert!(tron_collides((3, TRON_H), &occupied), "bottom wall");
        assert!(tron_collides((5, 5), &occupied), "trail cell");
        assert!(!tron_collides((6, 5), &occupied), "open cell");
    }

    #[test]
    fn cycle_dies_on_wall() {
        let mut game = local_game();
        // aim P1 straight at the top wall from one cell away
        game.cycles[0].x = 10;
        game.cycles[0].y = 0;
        game.cycles[0].dir = Dir::Up;
        game.step();
        assert!(!game.cycles[0].alive);
        assert!(game.cycles[1].alive);
        assert_eq!(game.wins, [0, 1]);
        assert_eq!(game.phase, Phase::RoundOver);
    }

    #[test]
    fn cycle_dies_on_trail() {
        let mut game = local_game();
        let block = (game.cycles[0].x + 1, game.cycles[0].y);
        game.occupied.insert(block);
        game.step(); // P1 rides right into the trail cell
        assert!(!game.cycles[0].alive);
        assert_eq!(game.phase, Phase::RoundOver);
    }

    #[test]
    fn head_on_collision_kills_both_and_draws() {
        let mut game = local_game();
        game.cycles[0].x = 10;
        game.cycles[0].y = 5;
        game.cycles[0].dir = Dir::Right;
        game.cycles[1].x = 12;
        game.cycles[1].y = 5;
        game.cycles[1].dir = Dir::Left;
        game.occupied.clear();
        game.occupied.insert((10, 5));
        game.occupied.insert((12, 5));
        game.step(); // both target (11,5)
        assert!(!game.cycles[0].alive);
        assert!(!game.cycles[1].alive);
        assert_eq!(game.wins, [0, 0], "draw rounds score nothing");
        assert_eq!(game.phase, Phase::RoundOver);
    }

    #[test]
    fn trails_grow_and_reversals_are_ignored() {
        let mut game = local_game();
        let before = game.cycles[0].trail.len();
        game.pending[0] = Some(Dir::Left); // reversal of Right: must be ignored
        game.step();
        assert_eq!(game.cycles[0].dir, Dir::Right);
        assert_eq!(game.cycles[0].trail.len(), before + 1);
        assert!(game.occupied.contains(&(game.cycles[0].x, game.cycles[0].y)));
    }

    #[test]
    fn match_ends_at_three_wins() {
        let mut game = local_game();
        game.wins = [ROUND_WINS - 1, 0];
        game.end_round([false, true]); // P1 survives
        assert_eq!(game.phase, Phase::MatchOver);
        assert!(game.banner.contains("WINS THE MATCH"));
    }

    #[test]
    fn msg_serde_roundtrip_every_variant() {
        let msgs = vec![
            TronMsg::Invite,
            TronMsg::Join,
            TronMsg::Start { seed: 3 },
            TronMsg::Quit,
            TronMsg::Input { d: 2 },
            TronMsg::State {
                x0: 1,
                y0: 2,
                d0: 1,
                a0: true,
                x1: 3,
                y1: 4,
                d1: 3,
                a1: false,
                w0: 2,
                w1: 1,
                rnd: 5,
                ph: 2,
                cd: 0.4,
            },
        ];
        for msg in msgs {
            let value = serde_json::to_value(&msg).unwrap();
            let back: TronMsg = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(back, msg);
            assert!(value.get("t").is_some());
        }
        assert_eq!(
            serde_json::to_value(&TronMsg::Invite).unwrap(),
            json!({"t": "invite"})
        );
    }

    #[test]
    fn guest_extends_trails_and_resets_on_round_bump() {
        let mut game = TronGame::new(NetHandle::new());
        game.mode = Mode::Online;
        game.role = NetRole::Guest;
        game.my_side = 1;
        game.begin_online_match(0);

        // first snapshot of round 1 resets trails to the heads
        game.apply_state([(14, 15), (42, 15)], [1, 3], [true, true], [0, 0], 1, 2, 0.0);
        assert_eq!(game.round, 1);
        assert_eq!(game.cycles[0].trail, vec![(14, 15)]);

        // next snapshot: heads moved one cell, trails extend locally
        game.apply_state([(15, 15), (41, 15)], [1, 3], [true, true], [0, 0], 1, 2, 0.0);
        assert_eq!(game.cycles[0].trail, vec![(14, 15), (15, 15)]);
        assert_eq!(game.cycles[1].trail, vec![(42, 15), (41, 15)]);
        assert!(game.occupied.contains(&(15, 15)));

        // round bump: trails reset to the new heads
        game.apply_state([(14, 15), (42, 15)], [1, 3], [true, true], [1, 0], 2, 1, 1.5);
        assert_eq!(game.round, 2);
        assert_eq!(game.cycles[0].trail, vec![(14, 15)]);
        assert_eq!(game.wins, [1, 0]);
        assert_eq!(game.phase, Phase::Countdown);
    }

    #[test]
    fn guest_round_over_snapshot_builds_banner() {
        let mut game = TronGame::new(NetHandle::new());
        game.mode = Mode::Online;
        game.role = NetRole::Guest;
        game.my_side = 1;
        game.begin_online_match(0);
        game.apply_state([(14, 15), (42, 15)], [1, 3], [true, true], [0, 0], 1, 2, 0.0);
        // opponent (side 0 = host) crashes; we take the round
        game.apply_state([(14, 15), (41, 15)], [1, 3], [false, true], [0, 1], 1, 3, 1.8);
        assert_eq!(game.phase, Phase::RoundOver);
        assert!(game.banner.contains("YOU TAKES THE ROUND") || game.banner.contains("YOU"));
    }
}
