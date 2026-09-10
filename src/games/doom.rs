//! Frostbite: a co-op first-person campaign. Three hand-built levels of
//! ice, rock and tech corridors with doors and locked doors, keys, health
//! and ammo pickups, three enemy types (seal grunts, walrus brutes, yeti
//! casters), two weapons, and an exit that unlocks once enough of the
//! level is cleared. Same host-authoritative netcode as Ice Arena; solo
//! runs the same simulation locally.

use super::raycast::{self, Camera, Map, Palette, Sprite, SpriteKind, DOOR, LOCKED_DOOR, OPEN};
use super::{rgb, CellGrid, Game, GameKind, Particles};
use crate::client::NetHandle;
use crate::message::{GameMsg, NetEnemy, NetHero, NetPickup, NetShot};
use crate::theme::t;
use crossterm::event::{KeyCode, KeyEvent};
use rand::Rng;
use std::collections::{HashMap, VecDeque};
use std::f32::consts::PI;

const GAME_ID: &str = "frostbite";
const FOV: f32 = 1.2;
const SIM_DT: f32 = 1.0 / 30.0;
const NET_DT: f32 = 1.0 / 20.0;
const MOVE_SPEED: f32 = 3.4;
const TURN_SPEED: f32 = 2.6;
const MAX_HP: i32 = 100;
const MAX_AMMO: u32 = 60;
const RESPAWN_TIME: f32 = 4.0;
const PLAYER_TIMEOUT: f32 = 6.0;

const KIND_GRUNT: u8 = 0;
const KIND_WALRUS: u8 = 1;
const KIND_YETI: u8 = 2;
const PICK_HEALTH: u8 = 0;
const PICK_AMMO: u8 = 1;
const PICK_KEY: u8 = 2;
const PHASE_PLAYING: u8 = 0;
const PHASE_LEVEL_CLEAR: u8 = 1;
const PHASE_VICTORY: u8 = 2;

const COLORS: [(u8, u8, u8); 6] = [
    (255, 220, 90),
    (120, 200, 255),
    (255, 120, 200),
    (120, 255, 150),
    (255, 150, 80),
    (200, 160, 255),
];

pub struct Level {
    pub name: &'static str,
    pub rows: &'static [&'static str],
}

pub const LEVELS: [Level; 3] = [
    Level {
        name: "ICE HALLS",
        rows: &[
            "#########################",
            "#P...........#....h.....#",
            "#....##..E...#..%%%%%%..#",
            "#....##......D..%....%..#",
            "#a...##..E...#..%.E..%..#",
            "######DD######..%....%..#",
            "#........#...#..%%%%D%..#",
            "#..E.....#.C.#.......k..#",
            "#........D...#..........#",
            "#..h.....#...####KK######",
            "####D#####...#.....a....#",
            "#........#...#..E....E..#",
            "#..a..E..D...D..........#",
            "#........#...#....C.....#",
            "#..E.....#...#......X...#",
            "#........#...#..........#",
            "#########################",
        ],
    },
    Level {
        name: "FROZEN DEPTHS",
        rows: &[
            "%%%%%%%%%%%%%%%%%%%%%%%%%",
            "%P...%.....E..%=======a=%",
            "%....%........%=.......=%",
            "%....D...C....D=..B....=%",
            "%....%........%=.......=%",
            "%%D%%%........%===D=====%",
            "%....%%%%DD%%%%......h..%",
            "%.h.........E......E....%",
            "%....%%%%%%%%%%%.....a..%",
            "%..E.%...k...%%..C......%",
            "%....%.......%%%%%%KK%%%%",
            "%....D...E...D..........%",
            "%....%.......%....E..a..%",
            "%.a..%%%%%%%%%....C.....%",
            "%....................X..%",
            "%%%%%%%%%%%%%%%%%%%%%%%%%",
        ],
    },
    Level {
        name: "WALRUS THRONE",
        rows: &[
            "=========================",
            "=P.....=...........=....=",
            "=......D....%%%....D..a.=",
            "=..h...=...%%B%%...=....=",
            "=......=....%.%....=..h.=",
            "====D=====.......====D===",
            "=.......=...E....=......=",
            "=..C....D.........D..C..=",
            "=.......=..B...B..=.....=",
            "=..a....=.........=..a..=",
            "====K=====..X..=====K====",
            "=.......=.......=.......=",
            "=..E....D.......D....E..=",
            "=...k...=..h.a..=...k...=",
            "=.......=.......=.......=",
            "=========================",
        ],
    },
];

#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Input {
    forward: f32,
    strafe: f32,
    turn: f32,
    fire: bool,
    weapon: u8,
}

struct Hero {
    name: String,
    x: f32,
    y: f32,
    angle: f32,
    hp: i32,
    ammo: u32,
    keys: u8,
    weapon: u8,
    kills: u32,
    color: u8,
    dead_for: f32,
    hurt: f32,
    fired: f32,
    cooldown: f32,
    input: Input,
    last_seen: f32,
    spawn: (f32, f32, f32),
}

struct Enemy {
    id: u32,
    kind: u8,
    x: f32,
    y: f32,
    angle: f32,
    hp: i32,
    max_hp: i32,
    hurt: f32,
    attack_cd: f32,
    windup: f32,
    alert: bool,
    heading: f32,
    retarget: f32,
}

struct Shot {
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
    hostile: bool,
    dmg: i32,
    owner: String,
    life: f32,
}

struct Pickup {
    x: f32,
    y: f32,
    kind: u8,
}

struct Door {
    x: usize,
    y: usize,
    locked: bool,
    open: bool,
    timer: f32,
}

/// Parsed level: walls plus every entity spawn.
struct LevelData {
    map: Map,
    spawns: Vec<(f32, f32, f32)>,
    enemies: Vec<(u8, f32, f32)>,
    pickups: Vec<Pickup>,
    doors: Vec<Door>,
    exit: (f32, f32),
}

fn parse_level(level: &Level) -> LevelData {
    let map = Map::from_rows(level.rows);
    let mut spawns = Vec::new();
    let mut enemies = Vec::new();
    let mut pickups = Vec::new();
    let mut doors = Vec::new();
    let mut exit = (1.5, 1.5);
    for (y, row) in level.rows.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            match ch {
                'P' => spawns.push((fx, fy, 0.0)),
                'E' => enemies.push((KIND_GRUNT, fx, fy)),
                'B' => enemies.push((KIND_WALRUS, fx, fy)),
                'C' => enemies.push((KIND_YETI, fx, fy)),
                'h' => pickups.push(Pickup { x: fx, y: fy, kind: PICK_HEALTH }),
                'a' => pickups.push(Pickup { x: fx, y: fy, kind: PICK_AMMO }),
                'k' => pickups.push(Pickup { x: fx, y: fy, kind: PICK_KEY }),
                'X' => exit = (fx, fy),
                'D' => doors.push(Door { x, y, locked: false, open: false, timer: 0.0 }),
                'K' => doors.push(Door { x, y, locked: true, open: false, timer: 0.0 }),
                _ => {}
            }
        }
    }
    // Face spawns toward the most open direction.
    for spawn in &mut spawns {
        let best = [0.0, PI / 2.0, PI, -PI / 2.0]
            .iter()
            .copied()
            .max_by(|a, b| {
                let da = map.cast(spawn.0, spawn.1, *a).0;
                let db = map.cast(spawn.0, spawn.1, *b).0;
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(0.0);
        spawn.2 = best;
    }
    if spawns.is_empty() {
        spawns.push((1.5, 1.5, 0.0));
    }
    LevelData { map, spawns, enemies, pickups, doors, exit }
}

fn enemy_stats(kind: u8) -> (i32, f32, i32, &'static str, u32) {
    // (hp, speed, melee damage, name, score)
    match kind {
        KIND_WALRUS => (240, 1.4, 28, "walrus", 300),
        KIND_YETI => (90, 1.6, 0, "yeti", 150),
        _ => (60, 2.2, 12, "seal", 100),
    }
}

struct Sim {
    map: Map,
    level: u8,
    heroes: HashMap<String, Hero>,
    order: Vec<String>,
    enemies: Vec<Enemy>,
    next_enemy_id: u32,
    shots: Vec<Shot>,
    pickups: Vec<Pickup>,
    doors: Vec<Door>,
    exit: (f32, f32),
    spawns: Vec<(f32, f32, f32)>,
    phase: u8,
    phase_timer: f32,
    kills: u32,
    kills_needed: u32,
    tick: u32,
    clock: f32,
    events: Vec<String>,
}

impl Sim {
    fn new() -> Self {
        let mut sim = Self {
            map: Map::filled(3, 3),
            level: 0,
            heroes: HashMap::new(),
            order: Vec::new(),
            enemies: Vec::new(),
            next_enemy_id: 1,
            shots: Vec::new(),
            pickups: Vec::new(),
            doors: Vec::new(),
            exit: (1.5, 1.5),
            spawns: Vec::new(),
            phase: PHASE_PLAYING,
            phase_timer: 0.0,
            kills: 0,
            kills_needed: 0,
            tick: 0,
            clock: 0.0,
            events: Vec::new(),
        };
        sim.load_level(0);
        sim
    }

    fn load_level(&mut self, index: u8) {
        let data = parse_level(&LEVELS[index as usize % LEVELS.len()]);
        self.level = index;
        self.map = data.map;
        self.spawns = data.spawns;
        self.pickups = data.pickups;
        self.doors = data.doors;
        self.exit = data.exit;
        self.enemies = data
            .enemies
            .iter()
            .map(|&(kind, x, y)| {
                let (hp, _, _, _, _) = enemy_stats(kind);
                let id = self.next_enemy_id;
                self.next_enemy_id += 1;
                Enemy {
                    id,
                    kind,
                    x,
                    y,
                    angle: 0.0,
                    hp,
                    max_hp: hp,
                    hurt: 0.0,
                    attack_cd: 0.0,
                    windup: 0.0,
                    alert: false,
                    heading: 0.0,
                    retarget: 0.0,
                }
            })
            .collect();
        self.shots.clear();
        self.kills = 0;
        self.kills_needed = ((self.enemies.len() as f32) * 0.6).ceil() as u32;
        self.phase = PHASE_PLAYING;
        self.phase_timer = 0.0;
        for (i, id) in self.order.clone().iter().enumerate() {
            let spawn = self.spawns[i % self.spawns.len()];
            if let Some(h) = self.heroes.get_mut(id) {
                h.spawn = spawn;
                h.x = spawn.0;
                h.y = spawn.1;
                h.angle = spawn.2;
                h.hp = h.hp.max(60);
                h.dead_for = 0.0;
                h.keys = 0;
            }
        }
        self.events.push(format!("LEVEL {}: {}", index + 1, LEVELS[index as usize % LEVELS.len()].name));
    }

    fn add_hero(&mut self, id: &str, name: &str) {
        if self.heroes.contains_key(id) {
            return;
        }
        let idx = self.order.len();
        let spawn = self.spawns[idx % self.spawns.len()];
        self.heroes.insert(
            id.to_string(),
            Hero {
                name: name.to_string(),
                x: spawn.0,
                y: spawn.1,
                angle: spawn.2,
                hp: MAX_HP,
                ammo: 24,
                keys: 0,
                weapon: 0,
                kills: 0,
                color: (idx % COLORS.len()) as u8,
                dead_for: 0.0,
                hurt: 0.0,
                fired: 0.0,
                cooldown: 0.0,
                input: Input::default(),
                last_seen: self.clock,
                spawn,
            },
        );
        self.order.push(id.to_string());
        self.events.push(format!("{} joined the expedition", name));
    }

    fn remove_hero(&mut self, id: &str) {
        if let Some(h) = self.heroes.remove(id) {
            self.order.retain(|o| o != id);
            self.events.push(format!("{} left", h.name));
        }
    }

    fn set_input(&mut self, id: &str, input: Input) {
        let clock = self.clock;
        if let Some(h) = self.heroes.get_mut(id) {
            h.input = input;
            h.last_seen = clock;
        }
    }

    fn any_entity_near(&self, x: f32, y: f32, r: f32) -> bool {
        let r2 = r * r;
        self.heroes.values().any(|h| h.dead_for == 0.0 && (h.x - x).powi(2) + (h.y - y).powi(2) < r2)
            || self.enemies.iter().any(|e| (e.x - x).powi(2) + (e.y - y).powi(2) < r2)
    }

    fn step(&mut self, dt: f32) {
        self.tick = self.tick.wrapping_add(1);
        self.clock += dt;
        let mut rng = rand::thread_rng();

        if self.phase != PHASE_PLAYING {
            self.phase_timer -= dt;
            if self.phase_timer <= 0.0 && self.phase == PHASE_LEVEL_CLEAR {
                let next = self.level + 1;
                if (next as usize) < LEVELS.len() {
                    self.load_level(next);
                } else {
                    self.phase = PHASE_VICTORY;
                    self.phase_timer = 1e9;
                    self.events.push("CAMPAIGN COMPLETE".to_string());
                }
            }
            return;
        }

        // Doors: open for nearby heroes (locked ones consume a key), close
        // after a few seconds once nothing stands in them.
        for i in 0..self.doors.len() {
            let (dx, dy) = (self.doors[i].x as f32 + 0.5, self.doors[i].y as f32 + 0.5);
            let mut open_by: Option<String> = None;
            for (id, h) in &self.heroes {
                if h.dead_for > 0.0 {
                    continue;
                }
                let near = (h.x - dx).powi(2) + (h.y - dy).powi(2) < 1.7;
                if near && (!self.doors[i].locked || h.keys > 0) {
                    open_by = Some(id.clone());
                    break;
                }
            }
            let occupied = self.any_entity_near(dx, dy, 0.9);
            let door = &mut self.doors[i];
            if let Some(id) = open_by {
                if door.locked {
                    door.locked = false;
                    if let Some(h) = self.heroes.get_mut(&id) {
                        h.keys = h.keys.saturating_sub(1);
                        self.events.push(format!("{} unlocked a door", h.name));
                    }
                }
                door.open = true;
                door.timer = 4.0;
            } else if door.open {
                if occupied {
                    door.timer = 4.0;
                } else {
                    door.timer -= dt;
                    if door.timer <= 0.0 {
                        door.open = false;
                    }
                }
            }
            let kind = if door.open {
                OPEN
            } else if door.locked {
                LOCKED_DOOR
            } else {
                DOOR
            };
            let (x, y) = (door.x, door.y);
            self.map.set_cell(x, y, kind);
        }

        // Heroes: respawn, move, fire.
        let mut new_shots = Vec::new();
        let mut pickup_events = Vec::new();
        for id in self.order.clone() {
            let Some(h) = self.heroes.get_mut(&id) else { continue };
            h.hurt = (h.hurt - dt).max(0.0);
            h.fired = (h.fired - dt).max(0.0);
            h.cooldown = (h.cooldown - dt).max(0.0);
            if h.dead_for > 0.0 {
                h.dead_for += dt;
                if h.dead_for >= RESPAWN_TIME {
                    h.dead_for = 0.0;
                    h.hp = MAX_HP;
                    h.ammo = h.ammo.max(12);
                    h.x = h.spawn.0;
                    h.y = h.spawn.1;
                    h.angle = h.spawn.2;
                }
                continue;
            }
            if h.input.weapon < 2 {
                h.weapon = h.input.weapon;
            }
            h.angle += h.input.turn * TURN_SPEED * dt;
            let (c, s) = (h.angle.cos(), h.angle.sin());
            let fwd = h.input.forward.clamp(-1.0, 1.0) * MOVE_SPEED * dt;
            let side = h.input.strafe.clamp(-1.0, 1.0) * MOVE_SPEED * 0.8 * dt;
            let (dx, dy) = (c * fwd - s * side, s * fwd + c * side);
            if dx != 0.0 || dy != 0.0 {
                let (mut x, mut y) = (h.x, h.y);
                self.map.try_move(&mut x, &mut y, dx, dy, 0.25);
                h.x = x;
                h.y = y;
            }
            if h.input.fire && h.cooldown <= 0.0 {
                h.input.fire = false;
                let (cost, cd) = if h.weapon == 1 { (3, 0.9) } else { (1, 0.4) };
                if h.ammo >= cost {
                    h.ammo -= cost;
                    h.cooldown = cd;
                    h.fired = 0.18;
                    let spread: &[f32] = if h.weapon == 1 { &[-0.18, 0.0, 0.18] } else { &[0.0] };
                    for off in spread {
                        let a = h.angle + off;
                        new_shots.push(Shot {
                            x: h.x + a.cos() * 0.5,
                            y: h.y + a.sin() * 0.5,
                            dx: a.cos() * 11.0,
                            dy: a.sin() * 11.0,
                            hostile: false,
                            dmg: if h.weapon == 1 { 30 } else { 34 },
                            owner: id.clone(),
                            life: 2.0,
                        });
                    }
                } else {
                    h.cooldown = 0.3;
                }
            }
            // Pickups.
            let mut i = 0;
            while i < self.pickups.len() {
                let p = &self.pickups[i];
                if (p.x - h.x).powi(2) + (p.y - h.y).powi(2) < 0.4 {
                    match p.kind {
                        PICK_HEALTH => {
                            if h.hp >= MAX_HP {
                                i += 1;
                                continue;
                            }
                            h.hp = (h.hp + 35).min(MAX_HP);
                            pickup_events.push(format!("{} ate a fish (+35 hp)", h.name));
                        }
                        PICK_AMMO => {
                            if h.ammo >= MAX_AMMO {
                                i += 1;
                                continue;
                            }
                            h.ammo = (h.ammo + 12).min(MAX_AMMO);
                            pickup_events.push(format!("{} found snowballs (+12)", h.name));
                        }
                        _ => {
                            h.keys += 1;
                            pickup_events.push(format!("{} picked up a key", h.name));
                        }
                    }
                    self.pickups.swap_remove(i);
                } else {
                    i += 1;
                }
            }
            // Exit.
            if self.kills >= self.kills_needed
                && (self.exit.0 - h.x).powi(2) + (self.exit.1 - h.y).powi(2) < 0.6
            {
                self.phase = PHASE_LEVEL_CLEAR;
                self.phase_timer = 3.0;
                pickup_events.push(format!("{} reached the exit!", h.name));
            }
        }
        self.shots.extend(new_shots);
        self.events.extend(pickup_events);

        // Enemies.
        let heroes_snapshot: Vec<(String, f32, f32)> = self
            .heroes
            .iter()
            .filter(|(_, h)| h.dead_for == 0.0)
            .map(|(id, h)| (id.clone(), h.x, h.y))
            .collect();
        let mut melee_hits: Vec<(String, i32, u8)> = Vec::new();
        let mut enemy_shots = Vec::new();
        let map = &self.map;
        for e in &mut self.enemies {
            e.hurt = (e.hurt - dt).max(0.0);
            e.attack_cd = (e.attack_cd - dt).max(0.0);
            e.retarget -= dt;
            let (_, speed, melee, _, _) = enemy_stats(e.kind);
            let target = heroes_snapshot
                .iter()
                .map(|(id, hx, hy)| (id, *hx, *hy, (hx - e.x).powi(2) + (hy - e.y).powi(2)))
                .min_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));
            let mut wants_move = 0.0;
            let mut see = false;
            if let Some((tid, tx, ty, d2)) = target {
                let dist = d2.sqrt();
                let want = (ty - e.y).atan2(tx - e.x);
                if dist < 16.0 && map.cast(e.x, e.y, want).0 > dist {
                    see = true;
                    e.alert = true;
                    e.heading = want;
                    let diff = wrap_angle(want - e.angle);
                    e.angle += diff.clamp(-3.5 * dt, 3.5 * dt);
                    if e.kind == KIND_YETI {
                        wants_move = if dist > 9.0 { 1.0 } else if dist < 4.0 { -0.7 } else { 0.0 };
                        if dist < 12.0 && e.attack_cd <= 0.0 {
                            e.attack_cd = 1.8;
                            enemy_shots.push(Shot {
                                x: e.x + want.cos() * 0.5,
                                y: e.y + want.sin() * 0.5,
                                dx: want.cos() * 6.5,
                                dy: want.sin() * 6.5,
                                hostile: true,
                                dmg: 15,
                                owner: String::new(),
                                life: 3.0,
                            });
                        }
                    } else if dist < 1.3 {
                        wants_move = 0.0;
                        if e.attack_cd <= 0.0 {
                            if e.windup <= 0.0 {
                                e.windup = 0.5;
                            } else {
                                e.windup -= dt;
                                if e.windup <= 0.0 {
                                    melee_hits.push((tid.clone(), melee, e.kind));
                                    e.attack_cd = 1.1;
                                }
                            }
                        }
                    } else {
                        wants_move = 1.0;
                        e.windup = 0.0;
                    }
                }
            }
            if !see && e.alert {
                // Lost sight: keep hunting along corridors toward the last heading.
                if e.retarget <= 0.0 || map.cast(e.x, e.y, e.angle).0 < 0.7 {
                    let cardinals = [0.0, PI / 2.0, PI, -PI / 2.0];
                    let mut best = e.angle;
                    let mut best_score = f32::NEG_INFINITY;
                    for &h in &cardinals {
                        let open = map.cast(e.x, e.y, h).0;
                        if open < 1.5 {
                            continue;
                        }
                        let score = (h - e.heading).cos() + rng.gen_range(-0.4..0.4) + open.min(6.0) * 0.05;
                        if score > best_score {
                            best_score = score;
                            best = h;
                        }
                    }
                    e.angle = best;
                    e.retarget = rng.gen_range(0.8..1.8);
                }
                wants_move = 0.8;
            }
            if wants_move != 0.0 {
                let (c, s) = (e.angle.cos(), e.angle.sin());
                let step = speed * wants_move * dt;
                let (mut x, mut y) = (e.x, e.y);
                map.try_move(&mut x, &mut y, c * step, s * step, 0.3);
                e.x = x;
                e.y = y;
            }
        }
        // Keep enemies from stacking on one spot.
        for i in 0..self.enemies.len() {
            for j in (i + 1)..self.enemies.len() {
                let (dx, dy) = (self.enemies[j].x - self.enemies[i].x, self.enemies[j].y - self.enemies[i].y);
                let d2 = dx * dx + dy * dy;
                if d2 < 0.5 && d2 > 1e-4 {
                    let d = d2.sqrt();
                    let push = (0.7 - d) * 0.5;
                    let (ux, uy) = (dx / d * push, dy / d * push);
                    let (mut ax, mut ay) = (self.enemies[i].x, self.enemies[i].y);
                    self.map.try_move(&mut ax, &mut ay, -ux, -uy, 0.3);
                    self.enemies[i].x = ax;
                    self.enemies[i].y = ay;
                    let (mut bx, mut by) = (self.enemies[j].x, self.enemies[j].y);
                    self.map.try_move(&mut bx, &mut by, ux, uy, 0.3);
                    self.enemies[j].x = bx;
                    self.enemies[j].y = by;
                }
            }
        }
        self.shots.extend(enemy_shots);
        for (hid, dmg, kind) in melee_hits {
            let (_, _, _, name, _) = enemy_stats(kind);
            self.damage_hero(&hid, dmg, &format!("a {}", name));
        }

        // Shots.
        let mut hero_hits: Vec<(String, i32)> = Vec::new();
        let mut enemy_hits: Vec<(u32, i32, String)> = Vec::new();
        for shot in &mut self.shots {
            shot.x += shot.dx * dt;
            shot.y += shot.dy * dt;
            shot.life -= dt;
            if self.map.wall_at(shot.x, shot.y) {
                shot.life = 0.0;
                continue;
            }
            if shot.hostile {
                for (id, h) in &self.heroes {
                    if h.dead_for == 0.0 && (h.x - shot.x).powi(2) + (h.y - shot.y).powi(2) < 0.3 {
                        hero_hits.push((id.clone(), shot.dmg));
                        shot.life = 0.0;
                        break;
                    }
                }
            } else {
                for e in &self.enemies {
                    let r = if e.kind == KIND_WALRUS { 0.55 } else { 0.36 };
                    if (e.x - shot.x).powi(2) + (e.y - shot.y).powi(2) < r {
                        enemy_hits.push((e.id, shot.dmg, shot.owner.clone()));
                        shot.life = 0.0;
                        break;
                    }
                }
            }
        }
        self.shots.retain(|s| s.life > 0.0);
        for (hid, dmg) in hero_hits {
            self.damage_hero(&hid, dmg, "an ice bolt");
        }
        let mut drops = Vec::new();
        for (eid, dmg, owner) in enemy_hits {
            if let Some(e) = self.enemies.iter_mut().find(|e| e.id == eid) {
                e.hp -= dmg;
                e.hurt = 0.25;
                e.alert = true;
                if e.hp <= 0 {
                    let (_, _, _, name, _) = enemy_stats(e.kind);
                    let killer = self.heroes.get(&owner).map(|h| h.name.clone()).unwrap_or_default();
                    self.events.push(format!("{} froze the {}", killer, name));
                    if let Some(h) = self.heroes.get_mut(&owner) {
                        h.kills += 1;
                    }
                    self.kills += 1;
                    if rng.gen_bool(0.3) {
                        drops.push(Pickup { x: e.x, y: e.y, kind: PICK_AMMO });
                    }
                }
            }
        }
        self.enemies.retain(|e| e.hp > 0);
        self.pickups.extend(drops);
        if self.kills == self.kills_needed && self.kills_needed > 0 {
            // Announce once: the counter passes this value exactly once.
            if self
                .events
                .iter()
                .all(|e| !e.contains("EXIT OPEN"))
            {
                self.events.push("EXIT OPEN: head for the beacon".to_string());
            }
        }

        // Drop heroes who stopped sending input.
        let stale: Vec<String> = self
            .heroes
            .iter()
            .filter(|(_, h)| self.clock - h.last_seen > PLAYER_TIMEOUT)
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.remove_hero(&id);
        }
    }

    fn damage_hero(&mut self, id: &str, dmg: i32, by: &str) {
        if let Some(h) = self.heroes.get_mut(id) {
            if h.dead_for > 0.0 {
                return;
            }
            h.hp -= dmg;
            h.hurt = 0.4;
            if h.hp <= 0 {
                h.hp = 0;
                h.dead_for = 0.01;
                self.events.push(format!("{} was frozen solid by {}", h.name, by));
            }
        }
    }

    fn snapshot(&mut self) -> GameMsg {
        GameMsg::DoomState {
            tick: self.tick,
            level: self.level,
            phase: self.phase,
            phase_timer: self.phase_timer.min(60.0),
            heroes: self
                .order
                .iter()
                .filter_map(|id| {
                    self.heroes.get(id).map(|h| NetHero {
                        id: id.clone(),
                        name: h.name.clone(),
                        x: h.x,
                        y: h.y,
                        angle: h.angle,
                        hp: h.hp,
                        ammo: h.ammo,
                        keys: h.keys,
                        weapon: h.weapon,
                        kills: h.kills,
                        color: h.color,
                        dead_for: h.dead_for,
                        hurt: h.hurt,
                        fired: h.fired,
                    })
                })
                .collect(),
            enemies: self
                .enemies
                .iter()
                .map(|e| NetEnemy { id: e.id, kind: e.kind, x: e.x, y: e.y, hp: e.hp, max_hp: e.max_hp, hurt: e.hurt })
                .collect(),
            shots: self.shots.iter().map(|s| NetShot { x: s.x, y: s.y, hostile: s.hostile }).collect(),
            pickups: self.pickups.iter().map(|p| NetPickup { x: p.x, y: p.y, kind: p.kind }).collect(),
            open_doors: self.doors.iter().filter(|d| d.open).map(|d| (d.x as u8, d.y as u8)).collect(),
            kills: self.kills,
            kills_needed: self.kills_needed,
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

/// What everyone renders from.
#[derive(Default)]
struct View {
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
}

struct Flake {
    x: f32,
    y: f32,
    speed: f32,
}

pub struct DoomGame {
    net: Option<NetHandle>,
    mode: Mode,
    map: Map,
    doors: Vec<(usize, usize, bool)>, // x, y, locked (client-side mirror)
    exit: (f32, f32),
    sim: Option<Sim>,
    view: View,
    my_id: String,
    my_name: String,
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
    weapon: u8,
    seq: u32,
    sim_accum: f32,
    net_timer: f32,
    lobby_timer: f32,
    known_host: Option<(String, String, Vec<String>)>,
    known_host_age: f32,
    feed: VecDeque<(String, f32)>,
    last_hp: i32,
    snow: Vec<Flake>,
    particles: Particles,
    total_kills: u32,
}

impl DoomGame {
    pub fn new() -> Self {
        let mut rng = rand::thread_rng();
        let snow = (0..40)
            .map(|_| Flake { x: rng.gen_range(0.0..1.0), y: rng.gen_range(0.0..1.0), speed: rng.gen_range(0.05..0.2) })
            .collect();
        let data = parse_level(&LEVELS[0]);
        Self {
            net: None,
            mode: Mode::Lobby,
            map: data.map,
            doors: data.doors.iter().map(|d| (d.x, d.y, d.locked)).collect(),
            exit: data.exit,
            sim: None,
            view: View::default(),
            my_id: "solo".to_string(),
            my_name: "you".to_string(),
            cam_x: data.spawns[0].0,
            cam_y: data.spawns[0].1,
            cam_angle: data.spawns[0].2,
            bob: 0.0,
            hold_fwd: 0.0,
            hold_back: 0.0,
            hold_left: 0.0,
            hold_right: 0.0,
            hold_sl: 0.0,
            hold_sr: 0.0,
            fire_queued: false,
            weapon: 0,
            seq: 0,
            sim_accum: 0.0,
            net_timer: 0.0,
            lobby_timer: 0.0,
            known_host: None,
            known_host_age: 0.0,
            feed: VecDeque::new(),
            last_hp: MAX_HP,
            snow,
            particles: Particles::default(),
            total_kills: 0,
        }
    }

    fn online(&self) -> bool {
        self.net.as_ref().map(|n| n.is_connected()).unwrap_or(false)
    }

    fn push_feed(&mut self, text: impl Into<String>) {
        self.feed.push_back((text.into(), 7.0));
        while self.feed.len() > 5 {
            self.feed.pop_front();
        }
    }

    fn start_solo(&mut self) {
        let mut sim = Sim::new();
        self.my_id = "solo".to_string();
        self.my_name = self.net.as_ref().map(|n| n.username.clone()).unwrap_or_else(|| "you".to_string());
        sim.add_hero(&self.my_id, &self.my_name);
        self.sim = Some(sim);
        self.mode = Mode::Solo;
        self.push_feed("solo expedition: clear 60% of each level to open the exit");
    }

    fn start_host(&mut self) {
        let Some(net) = self.net.clone() else { return };
        let Some(id) = net.my_id() else { return };
        let mut sim = Sim::new();
        self.my_id = id.clone();
        self.my_name = net.username.clone();
        sim.add_hero(&id, &net.username);
        self.sim = Some(sim);
        self.mode = Mode::Host;
        self.push_feed("hosting the expedition: others press J to join");
    }

    fn start_join(&mut self) {
        let Some(net) = self.net.clone() else { return };
        let Some(id) = net.my_id() else { return };
        self.my_id = id;
        self.my_name = net.username.clone();
        net.send(GameMsg::Join { game: GAME_ID.to_string(), name: net.username.clone() });
        self.mode = Mode::Client;
        self.push_feed("joining expedition...");
    }

    fn current_input(&self) -> Input {
        let mut input = Input { weapon: self.weapon, ..Default::default() };
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

    fn me(&self) -> Option<&NetHero> {
        self.view.heroes.iter().find(|h| h.id == self.my_id)
    }

    fn predict(&mut self, dt: f32, input: Input) {
        if self.me().map(|h| h.dead_for > 0.0).unwrap_or(false) || self.view.phase != PHASE_PLAYING {
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

    fn reconcile(&mut self) {
        let Some(me) = self.me().cloned() else { return };
        let d2 = (me.x - self.cam_x).powi(2) + (me.y - self.cam_y).powi(2);
        if d2 > 2.25 || me.dead_for > 0.0 {
            self.cam_x = me.x;
            self.cam_y = me.y;
            self.cam_angle = me.angle;
        } else {
            self.cam_x += (me.x - self.cam_x) * 0.35;
            self.cam_y += (me.y - self.cam_y) * 0.35;
            self.cam_angle += wrap_angle(me.angle - self.cam_angle) * 0.2;
        }
        if me.hp < self.last_hp {
            self.particles.burst(0.5, 0.5, 12, (255, 90, 90), 1.0, &['*', '·', '+']);
        }
        self.last_hp = me.hp;
    }

    /// Mirror door state into the local map so prediction and rendering match.
    fn sync_doors(&mut self) {
        for &(x, y, locked) in &self.doors.clone() {
            let open = self.view.open_doors.iter().any(|&(dx, dy)| dx as usize == x && dy as usize == y);
            let kind = if open {
                OPEN
            } else if locked {
                LOCKED_DOOR
            } else {
                DOOR
            };
            self.map.set_cell(x, y, kind);
        }
    }

    fn apply_state(&mut self, state: GameMsg) {
        if let GameMsg::DoomState { level, phase, phase_timer, heroes, enemies, shots, pickups, open_doors, kills, kills_needed, events, .. } = state {
            if level != self.view.level || self.view.heroes.is_empty() {
                let data = parse_level(&LEVELS[level as usize % LEVELS.len()]);
                self.map = data.map;
                self.doors = data.doors.iter().map(|d| (d.x, d.y, d.locked)).collect();
                self.exit = data.exit;
            }
            // Locked doors that were opened stay unlocked; approximate by
            // treating any door we have seen open as unlocked from then on.
            for (x, y, locked) in &mut self.doors {
                if *locked && open_doors.iter().any(|&(dx, dy)| dx as usize == *x && dy as usize == *y) {
                    *locked = false;
                }
            }
            self.view = View { level, phase, phase_timer, heroes, enemies, shots, pickups, open_doors, kills, kills_needed };
            self.sync_doors();
            for e in events {
                self.push_feed(e);
            }
            self.reconcile();
            self.total_kills = self.me().map(|h| h.kills).unwrap_or(self.total_kills);
        }
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
                GameMsg::Lobby { game, host_name, players } if game == GAME_ID && from != my_id => {
                    self.known_host = Some((from, host_name, players));
                    self.known_host_age = 0.0;
                }
                GameMsg::Join { game, name } if game == GAME_ID && self.mode == Mode::Host => {
                    if let Some(sim) = &mut self.sim {
                        sim.add_hero(&from, &name);
                    }
                }
                GameMsg::Leave { game } if game == GAME_ID && self.mode == Mode::Host => {
                    if let Some(sim) = &mut self.sim {
                        sim.remove_hero(&from);
                    }
                }
                GameMsg::DoomInput { forward, strafe, turn, fire, weapon, .. } if self.mode == Mode::Host && from != my_id => {
                    if let Some(sim) = &mut self.sim {
                        sim.set_input(&from, Input { forward, strafe, turn, fire, weapon });
                    }
                }
                state @ GameMsg::DoomState { .. }
                    if self.mode == Mode::Client
                        && self.known_host.as_ref().map(|h| h.0 == from).unwrap_or(true) =>
                {
                    self.apply_state(state);
                }
                _ => {}
            }
        }
    }
}

impl Game for DoomGame {
    fn kind(&self) -> GameKind {
        GameKind::Frostbite
    }

    fn score(&self) -> u32 {
        self.total_kills * 100 + self.view.level as u32 * 500
    }

    fn is_over(&self) -> bool {
        self.view.phase == PHASE_VICTORY
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
            KeyCode::Char('1') => self.weapon = 0,
            KeyCode::Char('2') => self.weapon = 1,
            KeyCode::Tab => self.weapon = 1 - self.weapon,
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
                    if let Some(state) = self.sim.as_mut().map(|s| s.snapshot()) {
                        self.apply_state(state.clone());
                        if self.mode == Mode::Host {
                            self.net_timer += dt;
                            if self.net_timer >= NET_DT {
                                self.net_timer = 0.0;
                                if let Some(net) = &self.net {
                                    net.send(state);
                                }
                            }
                        }
                    }
                }
                if self.mode == Mode::Host {
                    self.lobby_timer += dt;
                    if self.lobby_timer >= 1.0 {
                        self.lobby_timer = 0.0;
                        let names = self.view.heroes.iter().map(|h| h.name.clone()).collect();
                        if let Some(net) = &self.net {
                            net.send(GameMsg::Lobby { game: GAME_ID.to_string(), host_name: self.my_name.clone(), players: names });
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
                        net.send(GameMsg::DoomInput {
                            seq: self.seq,
                            forward: input.forward,
                            strafe: input.strafe,
                            turn: input.turn,
                            fire: input.fire,
                            weapon: input.weapon,
                            interact: false,
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
        let me = self.me().cloned();
        let dead = me.as_ref().map(|m| m.dead_for > 0.0).unwrap_or(false);
        let hurt = me.as_ref().map(|m| m.hurt > 0.0).unwrap_or(false) && ((phase * 12.0) as i32) % 2 == 0;
        let tint = if dead {
            Some((120, 130, 160))
        } else if hurt {
            Some((255, 100, 100))
        } else {
            None
        };
        let cam = Camera {
            x: self.cam_x,
            y: self.cam_y,
            angle: self.cam_angle,
            bob: (self.bob.sin() * 0.6).round() as i32,
            fov: FOV,
        };

        let mut sprites: Vec<Sprite> = Vec::new();
        for e in &self.view.enemies {
            let kind = match e.kind {
                KIND_WALRUS => SpriteKind::Walrus,
                KIND_YETI => SpriteKind::Yeti,
                _ => SpriteKind::Seal,
            };
            let base = match e.kind {
                KIND_WALRUS => (180, 140, 120),
                KIND_YETI => (200, 220, 240),
                _ => (190, 190, 205),
            };
            let color = if e.hurt > 0.0 { (255, 255, 255) } else { base };
            let hp_bar = if e.hp < e.max_hp {
                let filled = ((e.hp as f32 / e.max_hp as f32) * 6.0).ceil() as usize;
                Some(format!("{}{}", "▰".repeat(filled), "▱".repeat(6 - filled.min(6))))
            } else {
                None
            };
            sprites.push(Sprite { x: e.x, y: e.y, kind, color, label: hp_bar });
        }
        for hero in self.view.heroes.iter().filter(|hh| hh.id != self.my_id && hh.dead_for == 0.0) {
            let c = COLORS[hero.color as usize % COLORS.len()];
            sprites.push(Sprite { x: hero.x, y: hero.y, kind: SpriteKind::Penguin, color: c, label: Some(hero.name.clone()) });
        }
        for s in &self.view.shots {
            sprites.push(Sprite {
                x: s.x,
                y: s.y,
                kind: if s.hostile { SpriteKind::IceBolt } else { SpriteKind::Snowball },
                color: if s.hostile { (150, 220, 255) } else { (245, 250, 255) },
                label: None,
            });
        }
        for p in &self.view.pickups {
            let (kind, color) = match p.kind {
                PICK_HEALTH => (SpriteKind::Fish, (255, 165, 70)),
                PICK_AMMO => (SpriteKind::AmmoCrate, (200, 230, 255)),
                _ => (SpriteKind::Key, (255, 220, 90)),
            };
            sprites.push(Sprite { x: p.x, y: p.y, kind, color, label: None });
        }
        let exit_open = self.view.kills >= self.view.kills_needed;
        let beacon = if exit_open {
            if ((phase * 4.0) as i32) % 2 == 0 { (120, 255, 150) } else { (60, 200, 100) }
        } else {
            (90, 90, 110)
        };
        sprites.push(Sprite { x: self.exit.0, y: self.exit.1, kind: SpriteKind::ExitBeacon, color: beacon, label: Some(if exit_open { "EXIT".into() } else { "LOCKED".into() }) });

        let snow: Vec<(f32, f32, bool)> = self.snow.iter().map(|f| (f.x, f.y, f.speed > 0.15)).collect();
        let palette = match self.view.level {
            1 => Palette { sky_top: (8, 8, 20), sky_bot: (40, 30, 60), floor_far: (50, 45, 60), floor_near: (120, 110, 120), ..Palette::ice() },
            2 => Palette { sky_top: (6, 10, 24), sky_bot: (30, 40, 90), floor_far: (40, 50, 80), floor_near: (110, 130, 170), ..Palette::ice() },
            _ => Palette::ice(),
        };
        raycast::render_view(grid, &self.map, &cam, &palette, tint, &sprites, &snow, hud);
        self.particles.draw(grid, |x, y| ((x * w as f32) as i32, hud + (y * view_h as f32) as i32));

        // Crosshair and weapon view.
        let (cx, cy) = (w / 2, hud + view_h / 2);
        grid.put(cx, cy, '+', rgb(240, 240, 250));
        if !dead {
            let fired = me.as_ref().map(|m| m.fired > 0.0).unwrap_or(false);
            let weapon = me.as_ref().map(|m| m.weapon).unwrap_or(self.weapon);
            let rows: [&str; 3] = if weapon == 1 {
                if fired {
                    ["  ●●●  ", " ╱███╲ ", "▐█████▌"]
                } else {
                    ["       ", " ╱▓●▓╲ ", "▐█████▌"]
                }
            } else if fired {
                ["   ●   ", "  ╱█╲  ", " ▐███▌ "]
            } else {
                ["       ", "  ╱●╲  ", " ▐███▌ "]
            };
            let lift = if fired { 1 } else { 0 };
            for (i, row) in rows.iter().enumerate() {
                let y = h - 3 + i as i32 - lift;
                grid.text_over(cx + 8 - (row.chars().count() as i32 / 2), y, row, rgb(230, 235, 245));
            }
        }

        // HUD.
        let level_name = LEVELS[self.view.level as usize % LEVELS.len()].name;
        let (hp, ammo, keys, weapon, kills) = me
            .as_ref()
            .map(|m| (m.hp, m.ammo, m.keys, m.weapon, m.kills))
            .unwrap_or((0, 0, 0, 0, 0));
        let hp_cells = ((hp.max(0) as f32 / MAX_HP as f32) * 12.0).round() as usize;
        let hp_bar = format!("{}{}", "█".repeat(hp_cells), "░".repeat(12 - hp_cells.min(12)));
        let hp_color = if hp > 50 { rgb(120, 230, 130) } else if hp > 25 { rgb(250, 200, 80) } else { t().danger };
        grid.text(0, 0, &format!("LVL {} {:<14}", self.view.level + 1, level_name), t().accent2, bg);
        grid.text(22, 0, "HP ", t().text, bg);
        grid.text(25, 0, &hp_bar, hp_color, bg);
        grid.text(38, 0, &format!("{:>3}", hp), hp_color, bg);
        grid.text(44, 0, &format!("SNOWBALLS {:>2}   KEYS {}   KILLS {}", ammo, keys, kills), t().accent4, bg);
        let w1 = if weapon == 0 { "[1] SNOWBALL" } else { " 1  snowball" };
        let w2 = if weapon == 1 { "[2] SPREAD" } else { " 2  spread" };
        let exit_txt = if exit_open {
            "EXIT OPEN: find the beacon".to_string()
        } else {
            format!("EXIT LOCKED: freeze {} more", self.view.kills_needed.saturating_sub(self.view.kills))
        };
        grid.text(0, 1, &format!("{}  {}   {}   W/S A/D Q/E move  Space throw  Tab swap", w1, w2, exit_txt), t().muted, bg);

        // Party list, top right.
        if w > 70 {
            let x0 = w - 24;
            let panel = rgb(16, 20, 36);
            grid.fill(x0, hud + 1, 23, self.view.heroes.len() as i32 + 2, ' ', t().text, panel);
            grid.text(x0 + 1, hud + 1, "EXPEDITION", t().accent2, panel);
            for (i, hh) in self.view.heroes.iter().enumerate() {
                let c = COLORS[hh.color as usize % COLORS.len()];
                let name: String = hh.name.chars().take(10).collect();
                let status = if hh.dead_for > 0.0 { "  ❄".to_string() } else { format!("{:>3}", hh.hp) };
                grid.text(x0 + 1, hud + 2 + i as i32, &format!("{:<10} {} k{:<3}", name, status, hh.kills), rgb(c.0, c.1, c.2), panel);
            }
        }

        // Feed.
        let feed_y0 = h - self.feed.len() as i32 - 1;
        for (i, (text, _)) in self.feed.iter().enumerate() {
            grid.text_over(1, feed_y0 + i as i32, text, rgb(230, 235, 245));
        }

        if dead {
            let left = (RESPAWN_TIME - me.as_ref().map(|m| m.dead_for).unwrap_or(0.0)).max(0.0);
            grid.banner(cy, &format!("FROZEN SOLID  //  back in {:.0}s", left.ceil()), rgb(200, 220, 255), rgb(20, 30, 60));
        }
        match self.view.phase {
            PHASE_LEVEL_CLEAR => {
                grid.banner(
                    cy - 2,
                    &format!("LEVEL {} CLEARED  //  next in {:.0}s", self.view.level + 1, self.view.phase_timer.max(0.0).ceil()),
                    t().accent2,
                    t().bg_base,
                );
            }
            PHASE_VICTORY => {
                grid.banner(cy - 2, "EXPEDITION COMPLETE", t().accent2, t().bg_base);
                grid.center_text(cy + 1, &format!("{} kills   //   R restart   Esc menu", kills), t().text, t().bg_base);
            }
            _ => {}
        }
    }
}

impl DoomGame {
    fn render_lobby(&self, grid: &mut CellGrid, phase: f32) {
        let w = grid.width as i32;
        let h = grid.height as i32;
        let cam = Camera {
            x: 12.5 + (phase * 0.2).cos() * 2.0,
            y: 7.5 + (phase * 0.2).sin() * 1.5,
            angle: phase * 0.2,
            bob: 0,
            fov: FOV,
        };
        let snow: Vec<(f32, f32, bool)> = self.snow.iter().map(|f| (f.x, f.y, f.speed > 0.15)).collect();
        raycast::render_view(grid, &self.map, &cam, &Palette::ice(), None, &[], &snow, 0);

        let box_w = 62.min(w - 2);
        let box_h = 13.min(h - 2);
        let x0 = (w - box_w) / 2;
        let y0 = (h - box_h) / 2;
        let panel = rgb(14, 18, 34);
        grid.fill(x0, y0, box_w, box_h, ' ', t().text, panel);
        grid.center_text(y0 + 1, "FROSTBITE // CO-OP CAMPAIGN", t().accent2, panel);
        grid.center_text(y0 + 2, "three levels of ice, rock and steel. seals, walruses, yetis.", t().muted, panel);
        let mut y = y0 + 4;
        if self.online() {
            let name = self.net.as_ref().map(|n| n.username.clone()).unwrap_or_default();
            grid.text(x0 + 2, y, &format!("room: connected as {}", name), t().accent4, panel);
            y += 1;
            match &self.known_host {
                Some((_, host, players)) => {
                    grid.text(x0 + 2, y, &format!("expedition led by {}: {}", host, players.join(", ")), t().text, panel);
                    y += 1;
                    grid.text(x0 + 2, y, "J  join the expedition", t().accent2, panel);
                    y += 1;
                }
                None => {
                    grid.text(x0 + 2, y, "no expedition yet", t().muted, panel);
                    y += 1;
                }
            }
            grid.text(x0 + 2, y, "H  host an expedition (others press J)", t().accent2, panel);
            y += 1;
        } else {
            grid.text(x0 + 2, y, "not in a room: /server <port> or /connect <ip>:<port> for co-op", t().muted, panel);
            y += 1;
        }
        grid.text(x0 + 2, y, "S  solo expedition", t().accent2, panel);
        y += 2;
        grid.text(x0 + 2, y, "W/S A/D Q/E move   Space throw   1/2 or Tab weapon", t().muted, panel);
        grid.text(x0 + 2, y + 1, "doors open as you approach; red doors need a key", t().muted, panel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashSet, VecDeque};

    /// Cells reachable from the first spawn, treating doors as passable and
    /// locked doors as walls unless `with_keys`.
    fn reachable(data: &LevelData, with_keys: bool) -> HashSet<(usize, usize)> {
        let start = (data.spawns[0].0 as usize, data.spawns[0].1 as usize);
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([start]);
        seen.insert(start);
        while let Some((x, y)) = queue.pop_front() {
            for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                let kind = data.map.cell(nx, ny);
                let passable = kind == OPEN || kind == DOOR || (with_keys && kind == LOCKED_DOOR);
                if passable && seen.insert((nx as usize, ny as usize)) {
                    queue.push_back((nx as usize, ny as usize));
                }
            }
        }
        seen
    }

    #[test]
    fn every_level_is_fully_reachable_without_keys() {
        for level in &LEVELS {
            let data = parse_level(level);
            let seen = reachable(&data, false);
            let cell = |x: f32, y: f32| (x as usize, y as usize);
            assert!(seen.contains(&cell(data.exit.0, data.exit.1)), "{}: exit unreachable", level.name);
            for (kind, x, y) in &data.enemies {
                assert!(seen.contains(&cell(*x, *y)), "{}: enemy kind {} at ({},{}) unreachable", level.name, kind, x, y);
            }
            for p in &data.pickups {
                assert!(seen.contains(&cell(p.x, p.y)), "{}: pickup at ({},{}) unreachable", level.name, p.x, p.y);
            }
            assert!(!data.enemies.is_empty() && data.pickups.iter().any(|p| p.kind == PICK_KEY));
            for row in level.rows {
                assert_eq!(row.chars().count(), level.rows[0].chars().count(), "{}: ragged row", level.name);
            }
        }
    }

    #[test]
    fn campaign_progresses_when_exit_reached() {
        let mut sim = Sim::new();
        sim.add_hero("me", "me");
        assert_eq!(sim.level, 0);
        // Cheat: count enough kills, stand on the exit.
        sim.kills = sim.kills_needed;
        let (ex, ey) = sim.exit;
        {
            let h = sim.heroes.get_mut("me").unwrap();
            h.x = ex;
            h.y = ey;
        }
        sim.step(SIM_DT);
        assert_eq!(sim.phase, PHASE_LEVEL_CLEAR);
        for _ in 0..120 {
            sim.step(SIM_DT);
        }
        assert_eq!(sim.level, 1, "should have advanced to level 2");
        assert_eq!(sim.phase, PHASE_PLAYING);
    }

    #[test]
    fn snowballs_hurt_enemies_and_doors_open_for_heroes() {
        let mut sim = Sim::new();
        sim.add_hero("me", "me");
        // Put a grunt right in front of the hero and shoot it.
        let (hx, hy, ha) = (sim.heroes["me"].x, sim.heroes["me"].y, sim.heroes["me"].angle);
        sim.enemies.clear();
        sim.enemies.push(Enemy {
            id: 99, kind: KIND_GRUNT, x: hx + ha.cos() * 2.0, y: hy + ha.sin() * 2.0, angle: 0.0,
            hp: 60, max_hp: 60, hurt: 0.0, attack_cd: 0.0, windup: 0.0, alert: false, heading: 0.0, retarget: 0.0,
        });
        sim.set_input("me", Input { fire: true, ..Default::default() });
        for _ in 0..20 {
            sim.step(SIM_DT);
        }
        assert!(sim.enemies.is_empty() || sim.enemies[0].hp < 60, "shot did not connect");

        // Walk a hero next to the first door and see it open.
        let door = (sim.doors[0].x, sim.doors[0].y);
        {
            let h = sim.heroes.get_mut("me").unwrap();
            h.x = door.0 as f32 + 0.5;
            h.y = door.1 as f32 + 0.5 + 0.9;
        }
        sim.step(SIM_DT);
        assert!(sim.doors[0].open);
        assert_eq!(sim.map.cell(door.0 as i32, door.1 as i32), OPEN);
    }
}
