//! Ice Arena: first-person multiplayer snowball deathmatch.
//!
//! Netcode is host-authoritative. Whoever presses H runs `Sim` at a fixed
//! rate and broadcasts `GameMsg::State` 20 times a second; everyone else
//! sends `GameMsg::Input` and renders the latest state, predicting their
//! own movement locally and easing toward the host's position when a
//! snapshot arrives. Solo mode runs the same `Sim` locally with bots, so
//! the game is playable without a room and the simulation has one code
//! path.

use super::raycast::{self, Camera, Map, Palette, Sprite, SpriteKind};
use super::{rgb, CellGrid, Game, GameKind, Particles};
use crate::client::NetHandle;
use crate::message::{GameMsg, NetPlayer, NetProjectile};
use crate::theme::t;
use crossterm::event::{KeyCode, KeyEvent};
use rand::Rng;
use std::collections::{HashMap, VecDeque};
use std::f32::consts::PI;

const GAME_ID: &str = "arena";
const FOV: f32 = 1.2;
const SIM_DT: f32 = 1.0 / 30.0;
const NET_DT: f32 = 1.0 / 20.0;
const MOVE_SPEED: f32 = 3.6;
const TURN_SPEED: f32 = 2.7;
const SNOWBALL_SPEED: f32 = 9.5;
const FREEZE_TIME: f32 = 1.6;
const FIRE_COOLDOWN: f32 = 0.45;
const PLAYER_TIMEOUT: f32 = 6.0;

const ARENA: [&str; 15] = [
    "#####################",
    "#........#.#........#",
    "#.##.###.....###.##.#",
    "#.#...............#.#",
    "#....#..#...#..#....#",
    "#.##.#.#.....#.#.##.#",
    "#....#...###...#....#",
    "#.......#...#.......#",
    "#....#...###...#....#",
    "#.##.#.#.....#.#.##.#",
    "#....#..#...#..#....#",
    "#.#...............#.#",
    "#.##.###.....###.##.#",
    "#........#.#........#",
    "#####################",
];
const SPAWNS: [(f32, f32, f32); 6] = [
    (1.5, 1.5, 0.0),
    (19.5, 1.5, PI),
    (1.5, 13.5, 0.0),
    (19.5, 13.5, PI),
    (10.5, 1.5, PI / 2.0),
    (10.5, 13.5, -PI / 2.0),
];
const COLORS: [(u8, u8, u8); 6] = [
    (255, 220, 90),
    (120, 200, 255),
    (255, 120, 200),
    (120, 255, 150),
    (255, 150, 80),
    (200, 160, 255),
];

#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Input {
    forward: f32,
    strafe: f32,
    turn: f32,
    fire: bool,
}

struct SimPlayer {
    name: String,
    x: f32,
    y: f32,
    angle: f32,
    score: u32,
    color: u8,
    frozen: f32,
    cooldown: f32,
    input: Input,
    last_seen: f32,
    bot: bool,
    bot_timer: f32,
    /// Heading a wandering bot is steering toward.
    bot_heading: f32,
}

struct Projectile {
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
    owner: String,
    life: f32,
}

/// The authoritative simulation. Runs on the host (and in solo mode).
struct Sim {
    map: Map,
    players: HashMap<String, SimPlayer>,
    order: Vec<String>,
    projectiles: Vec<Projectile>,
    tick: u32,
    clock: f32,
    events: Vec<String>,
}

impl Sim {
    fn new(map: Map) -> Self {
        Self {
            map,
            players: HashMap::new(),
            order: Vec::new(),
            projectiles: Vec::new(),
            tick: 0,
            clock: 0.0,
            events: Vec::new(),
        }
    }

    fn add_player(&mut self, id: &str, name: &str, bot: bool) {
        if self.players.contains_key(id) {
            return;
        }
        let idx = self.order.len();
        let (x, y, angle) = SPAWNS[idx % SPAWNS.len()];
        self.players.insert(
            id.to_string(),
            SimPlayer {
                name: name.to_string(),
                x,
                y,
                angle,
                score: 0,
                color: (idx % COLORS.len()) as u8,
                frozen: 0.0,
                cooldown: 0.0,
                input: Input::default(),
                last_seen: self.clock,
                bot,
                bot_timer: 0.0,
                bot_heading: angle,
            },
        );
        self.order.push(id.to_string());
        self.events.push(format!("{} entered the arena", name));
    }

    fn remove_player(&mut self, id: &str) {
        if let Some(p) = self.players.remove(id) {
            self.order.retain(|o| o != id);
            self.events.push(format!("{} left", p.name));
        }
    }

    fn set_input(&mut self, id: &str, input: Input) {
        let clock = self.clock;
        if let Some(p) = self.players.get_mut(id) {
            p.input = input;
            p.last_seen = clock;
        }
    }

    /// Spawn point farthest from `avoid`.
    fn respawn_point(&self, avoid: (f32, f32)) -> (f32, f32, f32) {
        *SPAWNS
            .iter()
            .max_by(|a, b| {
                let da = (a.0 - avoid.0).powi(2) + (a.1 - avoid.1).powi(2);
                let db = (b.0 - avoid.0).powi(2) + (b.1 - avoid.1).powi(2);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(&SPAWNS[0])
    }

    fn step(&mut self, dt: f32) {
        self.tick = self.tick.wrapping_add(1);
        self.clock += dt;
        let mut rng = rand::thread_rng();

        // Bot brains. With line of sight to the nearest player: face them,
        // close in, throw. Otherwise wander the corridors, preferring the
        // open cardinal direction that points toward the target, and turn
        // away from walls before bumping into them.
        let snapshot: Vec<(String, f32, f32, bool)> = self
            .players
            .iter()
            .map(|(id, p)| (id.clone(), p.x, p.y, p.frozen > 0.0))
            .collect();
        let map = &self.map;
        for id in self.order.clone() {
            let Some(p) = self.players.get_mut(&id) else { continue };
            if !p.bot {
                continue;
            }
            p.bot_timer -= dt;
            let target = snapshot
                .iter()
                .filter(|(tid, _, _, frozen)| *tid != id && !frozen)
                .map(|(_, tx, ty, _)| (*tx, *ty, (tx - p.x).powi(2) + (ty - p.y).powi(2)))
                .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
            let mut input = Input { forward: 1.0, ..Default::default() };
            let mut engaged = false;
            if let Some((tx, ty, d2)) = target {
                let want = (ty - p.y).atan2(tx - p.x);
                let dist = d2.sqrt();
                let (wall_dist, _) = map.cast(p.x, p.y, want);
                if wall_dist > dist {
                    engaged = true;
                    let diff = wrap_angle(want - p.angle);
                    input.turn = (diff * 3.0).clamp(-1.0, 1.0);
                    input.fire = diff.abs() < 0.2 && dist < 9.0 && rng.gen_bool(0.7);
                    if dist < 2.5 {
                        input.forward = 0.0;
                    }
                }
                if !engaged && p.bot_timer <= 0.0 {
                    // Pick the open cardinal heading closest to the target.
                    let cardinals = [0.0, PI / 2.0, PI, -PI / 2.0];
                    let mut best = p.bot_heading;
                    let mut best_score = f32::NEG_INFINITY;
                    for &h in &cardinals {
                        let (open, _) = map.cast(p.x, p.y, h);
                        if open < 1.5 {
                            continue;
                        }
                        let alignment = (h - want).cos();
                        let score = alignment + rng.gen_range(-0.5..0.5) + open.min(6.0) * 0.05;
                        if score > best_score {
                            best_score = score;
                            best = h;
                        }
                    }
                    p.bot_heading = best;
                    p.bot_timer = rng.gen_range(1.2..2.4);
                }
            }
            if !engaged {
                let (ahead, _) = map.cast(p.x, p.y, p.angle);
                if ahead < 0.7 {
                    p.bot_timer = 0.0;
                    input.forward = 0.0;
                }
                let diff = wrap_angle(p.bot_heading - p.angle);
                input.turn = (diff * 3.0).clamp(-1.0, 1.0);
                if diff.abs() > 0.6 {
                    input.forward = 0.0;
                }
            }
            p.input = input;
            p.last_seen = self.clock;
        }

        // Movement and firing.
        let mut new_projectiles = Vec::new();
        for id in self.order.clone() {
            let Some(p) = self.players.get_mut(&id) else { continue };
            p.cooldown = (p.cooldown - dt).max(0.0);
            if p.frozen > 0.0 {
                p.frozen -= dt;
                continue;
            }
            p.angle += p.input.turn * TURN_SPEED * dt;
            let (c, s) = (p.angle.cos(), p.angle.sin());
            let fwd = p.input.forward.clamp(-1.0, 1.0) * MOVE_SPEED * dt;
            let side = p.input.strafe.clamp(-1.0, 1.0) * MOVE_SPEED * 0.8 * dt;
            let (dx, dy) = (c * fwd - s * side, s * fwd + c * side);
            if dx != 0.0 || dy != 0.0 {
                let (mut x, mut y) = (p.x, p.y);
                self.map.try_move(&mut x, &mut y, dx, dy, 0.25);
                p.x = x;
                p.y = y;
            }
            if p.input.fire && p.cooldown <= 0.0 {
                p.cooldown = FIRE_COOLDOWN;
                new_projectiles.push(Projectile {
                    x: p.x + c * 0.5,
                    y: p.y + s * 0.5,
                    dx: c * SNOWBALL_SPEED,
                    dy: s * SNOWBALL_SPEED,
                    owner: id.clone(),
                    life: 2.5,
                });
                if !p.bot {
                    p.input.fire = false;
                }
            }
        }
        self.projectiles.extend(new_projectiles);

        // Projectiles fly, hit walls, hit players.
        let mut hits: Vec<(String, String)> = Vec::new();
        for pr in &mut self.projectiles {
            pr.x += pr.dx * dt;
            pr.y += pr.dy * dt;
            pr.life -= dt;
            if self.map.wall_at(pr.x, pr.y) {
                pr.life = 0.0;
                continue;
            }
            for (id, p) in &self.players {
                if *id == pr.owner || p.frozen > 0.0 {
                    continue;
                }
                if (p.x - pr.x).powi(2) + (p.y - pr.y).powi(2) < 0.36 {
                    hits.push((pr.owner.clone(), id.clone()));
                    pr.life = 0.0;
                    break;
                }
            }
        }
        self.projectiles.retain(|pr| pr.life > 0.0);
        for (shooter, victim) in hits {
            let shooter_pos = self.players.get(&shooter).map(|p| (p.x, p.y)).unwrap_or((10.5, 7.5));
            let shooter_name = self.players.get(&shooter).map(|p| p.name.clone()).unwrap_or_default();
            if let Some(p) = self.players.get_mut(&shooter) {
                p.score += 1;
            }
            let respawn = self.respawn_point(shooter_pos);
            if let Some(v) = self.players.get_mut(&victim) {
                v.frozen = FREEZE_TIME;
                v.x = respawn.0;
                v.y = respawn.1;
                v.angle = respawn.2;
                self.events.push(format!("{} froze {}", shooter_name, v.name));
            }
        }

        // Drop players who stopped sending input.
        let stale: Vec<String> = self
            .players
            .iter()
            .filter(|(_, p)| !p.bot && self.clock - p.last_seen > PLAYER_TIMEOUT)
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.remove_player(&id);
        }
    }

    fn snapshot(&mut self) -> GameMsg {
        let players = self
            .order
            .iter()
            .filter_map(|id| {
                self.players.get(id).map(|p| NetPlayer {
                    id: id.clone(),
                    name: p.name.clone(),
                    x: p.x,
                    y: p.y,
                    angle: p.angle,
                    score: p.score,
                    color: p.color,
                    frozen: p.frozen.max(0.0),
                    bot: p.bot,
                })
            })
            .collect();
        let projectiles = self
            .projectiles
            .iter()
            .map(|pr| NetProjectile { x: pr.x, y: pr.y, owner: pr.owner.clone() })
            .collect();
        GameMsg::State {
            tick: self.tick,
            players,
            projectiles,
            events: std::mem::take(&mut self.events),
        }
    }
}

fn wrap_angle(mut a: f32) -> f32 {
    while a > PI {
        a -= 2.0 * PI;
    }
    while a < -PI {
        a += 2.0 * PI;
    }
    a
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Lobby,
    Solo,
    Host,
    Client,
}

struct Flake {
    x: f32,
    y: f32,
    speed: f32,
}

pub struct ArenaGame {
    net: Option<NetHandle>,
    mode: Mode,
    map: Map,
    sim: Option<Sim>,
    my_id: String,
    my_name: String,
    /// Latest state everyone renders from (host fills it from `sim`).
    players: Vec<NetPlayer>,
    projectiles: Vec<NetProjectile>,
    /// Locally predicted camera.
    cam_x: f32,
    cam_y: f32,
    cam_angle: f32,
    bob: f32,
    hold_fwd: f32,
    hold_back: f32,
    hold_left: f32,
    hold_right: f32,
    hold_sl: f32,
    hold_sr: f32,
    fire_queued: bool,
    seq: u32,
    sim_accum: f32,
    net_timer: f32,
    lobby_timer: f32,
    /// Known match in the room: (host id, host name, players).
    known_host: Option<(String, String, Vec<String>)>,
    known_host_age: f32,
    feed: VecDeque<(String, f32)>,
    hurt_timer: f32,
    hit_flash: f32,
    last_frozen: bool,
    snow: Vec<Flake>,
    particles: Particles,
    status: String,
}

impl ArenaGame {
    pub fn new() -> Self {
        let mut rng = rand::thread_rng();
        let snow = (0..50)
            .map(|_| Flake { x: rng.gen_range(0.0..1.0), y: rng.gen_range(0.0..1.0), speed: rng.gen_range(0.06..0.25) })
            .collect();
        Self {
            net: None,
            mode: Mode::Lobby,
            map: Map::from_rows(&ARENA),
            sim: None,
            my_id: "solo".to_string(),
            my_name: "you".to_string(),
            players: Vec::new(),
            projectiles: Vec::new(),
            cam_x: SPAWNS[0].0,
            cam_y: SPAWNS[0].1,
            cam_angle: SPAWNS[0].2,
            bob: 0.0,
            hold_fwd: 0.0,
            hold_back: 0.0,
            hold_left: 0.0,
            hold_right: 0.0,
            hold_sl: 0.0,
            hold_sr: 0.0,
            fire_queued: false,
            seq: 0,
            sim_accum: 0.0,
            net_timer: 0.0,
            lobby_timer: 0.0,
            known_host: None,
            known_host_age: 0.0,
            feed: VecDeque::new(),
            hurt_timer: 0.0,
            hit_flash: 0.0,
            last_frozen: false,
            snow,
            particles: Particles::default(),
            status: String::new(),
        }
    }

    fn online(&self) -> bool {
        self.net.as_ref().map(|n| n.is_connected()).unwrap_or(false)
    }

    fn start_solo(&mut self) {
        let mut sim = Sim::new(self.map.clone());
        self.my_id = "solo".to_string();
        self.my_name = self.net.as_ref().map(|n| n.username.clone()).unwrap_or_else(|| "you".to_string());
        sim.add_player(&self.my_id, &self.my_name, false);
        sim.add_player("bot-1", "Frosty", true);
        sim.add_player("bot-2", "Slush", true);
        self.sim = Some(sim);
        self.mode = Mode::Solo;
        self.push_feed("solo match: two bots, first to freeze the most wins");
    }

    fn start_host(&mut self) {
        let Some(net) = self.net.clone() else { return };
        let Some(id) = net.my_id() else { return };
        let mut sim = Sim::new(self.map.clone());
        self.my_id = id.clone();
        self.my_name = net.username.clone();
        sim.add_player(&id, &net.username, false);
        self.sim = Some(sim);
        self.mode = Mode::Host;
        self.lobby_timer = 0.0;
        self.push_feed("hosting: others in the room can press J to join // B adds a bot");
    }

    fn start_join(&mut self) {
        let Some(net) = self.net.clone() else { return };
        let Some(id) = net.my_id() else { return };
        self.my_id = id;
        self.my_name = net.username.clone();
        net.send(GameMsg::Join { game: GAME_ID.to_string(), name: net.username.clone() });
        self.mode = Mode::Client;
        self.push_feed("joining match...");
    }

    fn push_feed(&mut self, text: impl Into<String>) {
        self.feed.push_back((text.into(), 6.0));
        while self.feed.len() > 5 {
            self.feed.pop_front();
        }
    }

    fn current_input(&mut self) -> Input {
        let mut input = Input::default();
        if self.hold_fwd > 0.0 {
            input.forward += 1.0;
        }
        if self.hold_back > 0.0 {
            input.forward -= 0.7;
        }
        if self.hold_left > 0.0 {
            input.turn -= 1.0;
        }
        if self.hold_right > 0.0 {
            input.turn += 1.0;
        }
        if self.hold_sl > 0.0 {
            input.strafe -= 1.0;
        }
        if self.hold_sr > 0.0 {
            input.strafe += 1.0;
        }
        input.fire = self.fire_queued;
        input
    }

    fn my_player(&self) -> Option<&NetPlayer> {
        self.players.iter().find(|p| p.id == self.my_id)
    }

    /// Local prediction: move the camera with our own input immediately.
    fn predict(&mut self, dt: f32, input: Input) {
        let frozen = self.my_player().map(|p| p.frozen > 0.0).unwrap_or(false);
        if frozen {
            return;
        }
        self.cam_angle += input.turn * TURN_SPEED * dt;
        let (c, s) = (self.cam_angle.cos(), self.cam_angle.sin());
        let fwd = input.forward * MOVE_SPEED * dt;
        let side = input.strafe * MOVE_SPEED * 0.8 * dt;
        let (dx, dy) = (c * fwd - s * side, s * fwd + c * side);
        if dx != 0.0 || dy != 0.0 {
            self.bob += dt * 9.0;
            let (mut x, mut y) = (self.cam_x, self.cam_y);
            self.map.try_move(&mut x, &mut y, dx, dy, 0.25);
            self.cam_x = x;
            self.cam_y = y;
        }
    }

    /// Ease the camera toward the authoritative position.
    fn reconcile(&mut self) {
        let Some(me) = self.my_player().cloned() else { return };
        let d2 = (me.x - self.cam_x).powi(2) + (me.y - self.cam_y).powi(2);
        if d2 > 2.25 || me.frozen > 0.0 {
            self.cam_x = me.x;
            self.cam_y = me.y;
            self.cam_angle = me.angle;
        } else {
            self.cam_x += (me.x - self.cam_x) * 0.35;
            self.cam_y += (me.y - self.cam_y) * 0.35;
            let mut da = me.angle - self.cam_angle;
            while da > PI {
                da -= 2.0 * PI;
            }
            while da < -PI {
                da += 2.0 * PI;
            }
            self.cam_angle += da * 0.2;
        }
        let frozen_now = me.frozen > 0.0;
        if frozen_now && !self.last_frozen {
            self.hurt_timer = FREEZE_TIME;
            self.particles.burst(0.5, 0.5, 30, (200, 230, 255), 1.2, &['*', '❄', '·', '+']);
        }
        self.last_frozen = frozen_now;
    }

    fn apply_state(&mut self, players: Vec<NetPlayer>, projectiles: Vec<NetProjectile>, events: Vec<String>) {
        // A snowball that vanished near us was probably ours landing; a hit
        // on someone shows up in the events feed.
        let before = self.projectiles.len();
        self.players = players;
        self.projectiles = projectiles;
        if self.projectiles.len() < before {
            self.hit_flash = 0.15;
        }
        for e in events {
            self.push_feed(e);
        }
        self.reconcile();
    }

    fn handle_net(&mut self, dt: f32) {
        let Some(net) = self.net.clone() else { return };
        self.known_host_age += dt;
        if self.known_host_age > 4.0 {
            self.known_host = None;
        }
        let my_id = net.my_id().unwrap_or_default();
        for (from, msg) in net.drain() {
            match msg {
                GameMsg::Lobby { game, host_name, players } if game == GAME_ID => {
                    if from != my_id {
                        self.known_host = Some((from, host_name, players));
                        self.known_host_age = 0.0;
                    }
                }
                GameMsg::Join { game, name } if game == GAME_ID && self.mode == Mode::Host => {
                    if let Some(sim) = &mut self.sim {
                        sim.add_player(&from, &name, false);
                    }
                }
                GameMsg::Leave { game } if game == GAME_ID && self.mode == Mode::Host => {
                    if let Some(sim) = &mut self.sim {
                        sim.remove_player(&from);
                    }
                }
                GameMsg::Input { forward, strafe, turn, fire, .. } if self.mode == Mode::Host && from != my_id => {
                    if let Some(sim) = &mut self.sim {
                        sim.set_input(&from, Input { forward, strafe, turn, fire });
                    }
                }
                GameMsg::State { players, projectiles, events, .. }
                    if self.mode == Mode::Client
                        && self.known_host.as_ref().map(|h| h.0 == from).unwrap_or(true) =>
                {
                    self.apply_state(players, projectiles, events);
                }
                _ => {}
            }
        }
    }
}

impl Game for ArenaGame {
    fn kind(&self) -> GameKind {
        GameKind::Arena
    }

    fn score(&self) -> u32 {
        self.my_player().map(|p| p.score).unwrap_or(0)
    }

    fn is_over(&self) -> bool {
        false
    }

    fn set_net(&mut self, net: Option<NetHandle>) {
        self.net = net;
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if self.mode == Mode::Lobby {
            match key.code {
                KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Enter | KeyCode::Char(' ') => self.start_solo(),
                KeyCode::Char('h') | KeyCode::Char('H') if self.online() => self.start_host(),
                KeyCode::Char('j') | KeyCode::Char('J') if self.online() && self.known_host.is_some() => self.start_join(),
                _ => return false,
            }
            return true;
        }
        let hold = 0.16;
        match key.code {
            KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') => self.hold_fwd = hold,
            KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => self.hold_back = hold,
            KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('A') => self.hold_left = hold,
            KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => self.hold_right = hold,
            KeyCode::Char('q') | KeyCode::Char('Q') => self.hold_sl = hold,
            KeyCode::Char('e') | KeyCode::Char('E') => self.hold_sr = hold,
            KeyCode::Char(' ') | KeyCode::Char('f') | KeyCode::Char('F') => self.fire_queued = true,
            KeyCode::Char('b') | KeyCode::Char('B') if matches!(self.mode, Mode::Host | Mode::Solo) => {
                if let Some(sim) = &mut self.sim {
                    let n = sim.order.iter().filter(|id| id.starts_with("bot-")).count() + 1;
                    let names = ["Frosty", "Slush", "Flurry", "Drift", "Sleet", "Hail"];
                    sim.add_player(&format!("bot-{}", n), names[(n - 1) % names.len()], true);
                }
            }
            _ => return false,
        }
        true
    }

    fn tick(&mut self, dt: f32) {
        let mut rng = rand::thread_rng();
        for flake in &mut self.snow {
            flake.y += flake.speed * dt;
            if flake.y > 1.0 {
                flake.y = 0.0;
                flake.x = rng.gen_range(0.0..1.0);
            }
        }
        self.particles.tick(dt, 0.0);
        self.hurt_timer = (self.hurt_timer - dt).max(0.0);
        self.hit_flash = (self.hit_flash - dt).max(0.0);
        for h in [&mut self.hold_fwd, &mut self.hold_back, &mut self.hold_left, &mut self.hold_right, &mut self.hold_sl, &mut self.hold_sr] {
            *h = (*h - dt).max(0.0);
        }
        for (_, life) in &mut self.feed {
            *life -= dt;
        }
        self.feed.retain(|(_, life)| *life > 0.0);

        self.handle_net(dt);
        if self.mode == Mode::Lobby {
            return;
        }

        let input = self.current_input();
        self.predict(dt, input);

        match self.mode {
            Mode::Solo | Mode::Host => {
                if let Some(sim) = &mut self.sim {
                    sim.set_input(&self.my_id, input);
                }
                self.fire_queued = false;
                self.sim_accum += dt;
                let mut stepped = false;
                while self.sim_accum >= SIM_DT {
                    self.sim_accum -= SIM_DT;
                    if let Some(sim) = &mut self.sim {
                        sim.step(SIM_DT);
                    }
                    stepped = true;
                }
                if stepped {
                    let snapshot = self.sim.as_mut().map(|s| s.snapshot());
                    if let Some(GameMsg::State { players, projectiles, events, tick }) = snapshot {
                        self.apply_state(players.clone(), projectiles.clone(), events.clone());
                        if self.mode == Mode::Host {
                            self.net_timer += dt;
                            if let Some(net) = &self.net {
                                if self.net_timer >= NET_DT {
                                    self.net_timer = 0.0;
                                    net.send(GameMsg::State { tick, players, projectiles, events });
                                }
                            }
                        }
                    }
                }
                if self.mode == Mode::Host {
                    self.lobby_timer += dt;
                    if self.lobby_timer >= 1.0 {
                        self.lobby_timer = 0.0;
                        let names = self.players.iter().map(|p| p.name.clone()).collect();
                        if let Some(net) = &self.net {
                            net.send(GameMsg::Lobby {
                                game: GAME_ID.to_string(),
                                host_name: self.my_name.clone(),
                                players: names,
                            });
                        }
                    }
                }
            }
            Mode::Client => {
                self.net_timer += dt;
                if self.net_timer >= NET_DT || self.fire_queued {
                    self.net_timer = 0.0;
                    self.seq = self.seq.wrapping_add(1);
                    if let Some(net) = &self.net {
                        net.send(GameMsg::Input {
                            seq: self.seq,
                            forward: input.forward,
                            strafe: input.strafe,
                            turn: input.turn,
                            fire: input.fire,
                        });
                    }
                    self.fire_queued = false;
                }
            }
            Mode::Lobby => {}
        }
    }

    fn render(&self, grid: &mut CellGrid, phase: f32) {
        let bg = t().panel_bg;
        let w = grid.width as i32;
        let h = grid.height as i32;

        if self.mode == Mode::Lobby {
            self.render_lobby(grid, phase);
            return;
        }

        let hud = 2;
        let view_h = h - hud;
        let hurt = self.hurt_timer > 0.0 && ((phase * 10.0) as i32) % 2 == 0;
        let cam = Camera {
            x: self.cam_x,
            y: self.cam_y,
            angle: self.cam_angle,
            bob: (self.bob.sin() * 0.6).round() as i32,
            fov: FOV,
        };
        let mut sprites: Vec<Sprite> = self
            .players
            .iter()
            .filter(|p| p.id != self.my_id)
            .map(|p| {
                let base = COLORS[p.color as usize % COLORS.len()];
                let color = if p.frozen > 0.0 { (170, 220, 255) } else { base };
                Sprite {
                    x: p.x,
                    y: p.y,
                    kind: SpriteKind::Penguin,
                    color,
                    label: Some(if p.frozen > 0.0 { format!("{} ❄", p.name) } else { p.name.clone() }),
                }
            })
            .collect();
        sprites.extend(self.projectiles.iter().map(|pr| Sprite {
            x: pr.x,
            y: pr.y,
            kind: SpriteKind::Snowball,
            color: (240, 250, 255),
            label: None,
        }));
        let snow: Vec<(f32, f32, bool)> = self.snow.iter().map(|f| (f.x, f.y, f.speed > 0.18)).collect();
        raycast::render_view(
            grid,
            &self.map,
            &cam,
            &Palette::arena(),
            if hurt { Some((150, 200, 255)) } else { None },
            &sprites,
            &snow,
            hud,
        );
        self.particles.draw(grid, |x, y| ((x * w as f32) as i32, hud + (y * view_h as f32) as i32));

        // Crosshair.
        let (cx, cy) = (w / 2, hud + view_h / 2);
        let cross = if self.hit_flash > 0.0 { rgb(255, 240, 120) } else { rgb(240, 240, 250) };
        grid.put(cx, cy, '+', cross);
        grid.put(cx - 2, cy, '‹', cross);
        grid.put(cx + 2, cy, '›', cross);

        // Frozen overlay.
        if let Some(me) = self.my_player() {
            if me.frozen > 0.0 {
                grid.banner(cy, "FROZEN!", rgb(180, 220, 255), rgb(20, 40, 80));
            }
        }

        // HUD line 1: mode + score, line 2: controls.
        let mode = match self.mode {
            Mode::Solo => "SOLO vs BOTS".to_string(),
            Mode::Host => format!("HOSTING // {} players", self.players.len()),
            Mode::Client => "ONLINE".to_string(),
            Mode::Lobby => String::new(),
        };
        let my_score = self.score();
        grid.text(0, 0, &format!("{}   YOUR FREEZES {}", mode, my_score), t().accent2, bg);
        let extra = if matches!(self.mode, Mode::Host | Mode::Solo) { "  B add bot" } else { "" };
        grid.text(0, 1, &format!("W/S move  A/D turn  Q/E strafe  Space throw{}  Esc leave", extra), t().accent4, bg);

        // Scoreboard, top right.
        let mut board: Vec<&NetPlayer> = self.players.iter().collect();
        board.sort_by_key(|p| std::cmp::Reverse(p.score));
        let bw = 22;
        if w > bw + 40 {
            let x0 = w - bw - 1;
            let panel = rgb(16, 20, 36);
            grid.fill(x0, hud + 1, bw, board.len() as i32 + 2, ' ', t().text, panel);
            grid.text(x0 + 1, hud + 1, "FREEZES", t().accent2, panel);
            for (i, p) in board.iter().enumerate() {
                let c = COLORS[p.color as usize % COLORS.len()];
                let name = if p.id == self.my_id { format!("{} (you)", p.name) } else { p.name.clone() };
                let name: String = name.chars().take(14).collect();
                let line = format!("{:<14} {:>4}", name, p.score);
                grid.text(x0 + 1, hud + 2 + i as i32, &line, rgb(c.0, c.1, c.2), panel);
            }
        }

        // Event feed, bottom left.
        let feed_y0 = h - self.feed.len() as i32 - 1;
        for (i, (text, _)) in self.feed.iter().enumerate() {
            grid.text_over(1, feed_y0 + i as i32, text, rgb(230, 235, 245));
        }
        if !self.status.is_empty() {
            grid.text_over(1, h - 1, &self.status, t().muted);
        }
    }
}

impl ArenaGame {
    fn render_lobby(&self, grid: &mut CellGrid, phase: f32) {
        let bg = t().panel_bg;
        let w = grid.width as i32;
        let h = grid.height as i32;
        // Slowly orbiting camera over the arena as a backdrop.
        let cam = Camera {
            x: 10.5 + (phase * 0.25).cos() * 3.0,
            y: 7.5 + (phase * 0.25).sin() * 2.0,
            angle: phase * 0.25 + PI,
            bob: 0,
            fov: FOV,
        };
        let snow: Vec<(f32, f32, bool)> = self.snow.iter().map(|f| (f.x, f.y, f.speed > 0.18)).collect();
        raycast::render_view(grid, &self.map, &cam, &Palette::arena(), None, &[], &snow, 0);

        let box_w = 58.min(w - 2);
        let box_h = 12.min(h - 2);
        let x0 = (w - box_w) / 2;
        let y0 = (h - box_h) / 2;
        let panel = rgb(14, 18, 34);
        grid.fill(x0, y0, box_w, box_h, ' ', t().text, panel);
        grid.center_text(y0 + 1, "ICE ARENA // SNOWBALL DEATHMATCH", t().accent2, panel);
        let mut y = y0 + 3;
        if self.online() {
            let name = self.net.as_ref().map(|n| n.username.clone()).unwrap_or_default();
            grid.text(x0 + 2, y, &format!("room: connected as {}", name), t().accent4, panel);
            y += 1;
            match &self.known_host {
                Some((_, host, players)) => {
                    grid.text(x0 + 2, y, &format!("match hosted by {}: {}", host, players.join(", ")), t().text, panel);
                    y += 1;
                    grid.text(x0 + 2, y, "J  join that match", t().accent2, panel);
                    y += 1;
                }
                None => {
                    grid.text(x0 + 2, y, "no match hosted yet", t().muted, panel);
                    y += 1;
                }
            }
            grid.text(x0 + 2, y, "H  host a match (others press J)", t().accent2, panel);
            y += 1;
        } else {
            grid.text(x0 + 2, y, "not in a room: /server <port> or /connect <ip>:<port>", t().muted, panel);
            y += 1;
            grid.text(x0 + 2, y, "then come back here to host or join", t().muted, panel);
            y += 1;
        }
        grid.text(x0 + 2, y, "S  solo match against two bots", t().accent2, panel);
        y += 2;
        grid.text(x0 + 2, y, "W/S move  A/D turn  Q/E strafe  Space throw", t().muted, panel);
        let _ = bg;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bots_move_and_engage() {
        let mut sim = Sim::new(Map::from_rows(&ARENA));
        sim.add_player("me", "me", false);
        sim.add_player("bot-1", "Frosty", true);
        sim.add_player("bot-2", "Slush", true);
        let start: Vec<(f32, f32)> = ["bot-1", "bot-2"].iter().map(|id| (sim.players[*id].x, sim.players[*id].y)).collect();
        for _ in 0..150 {
            sim.step(SIM_DT);
        }
        for (i, id) in ["bot-1", "bot-2"].iter().enumerate() {
            let p = &sim.players[*id];
            let moved = ((p.x - start[i].0).powi(2) + (p.y - start[i].1).powi(2)).sqrt();
            assert!(moved > 1.0, "{} barely moved: {:.2}", id, moved);
            assert!(!sim.map.wall_at(p.x, p.y), "{} is inside a wall", id);
        }
        let snap = sim.snapshot();
        if let GameMsg::State { players, .. } = snap {
            assert_eq!(players.len(), 3);
        } else {
            panic!("snapshot is not a State");
        }
    }

    #[test]
    fn snowball_freezes_and_scores() {
        let mut sim = Sim::new(Map::from_rows(&ARENA));
        sim.add_player("a", "a", false);
        sim.add_player("b", "b", false);
        // Put b two cells east of a, in a's line of fire.
        {
            let b = sim.players.get_mut("b").unwrap();
            b.x = 3.5;
            b.y = 1.5;
        }
        sim.set_input("a", Input { fire: true, ..Default::default() });
        for _ in 0..30 {
            sim.step(SIM_DT);
        }
        assert_eq!(sim.players["a"].score, 1);
        assert!(sim.players["b"].frozen > 0.0 || sim.players["b"].x != 3.5);
    }
}
