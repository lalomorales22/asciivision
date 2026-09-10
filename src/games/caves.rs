//! Penguin Ice Caves: a first-person raycast maze. Collect every fish
//! before the clock runs out while seals patrol the tunnels. Rendering is
//! shared with Ice Arena through `raycast`.

use super::raycast::{self, Camera, Map, Palette, Sprite, SpriteKind};
use super::{rgb, CellGrid, Game, GameKind, Particles, Popups};
use crate::theme::t;
use crossterm::event::{KeyCode, KeyEvent};
use rand::Rng;
use std::collections::HashSet;
use std::f32::consts::PI;

const CELLS_W: usize = 12;
const CELLS_H: usize = 9;
const MAP_W: usize = CELLS_W * 2 + 1;
const MAP_H: usize = CELLS_H * 2 + 1;
const FOV: f32 = 1.15;
const START_TIME: f32 = 75.0;

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Playing,
    LevelClear(f32),
    GameOver,
}

struct Seal {
    x: f32,
    y: f32,
    angle: f32,
    speed: f32,
    retarget: f32,
}

struct Flake {
    x: f32,
    y: f32,
    speed: f32,
}

pub struct CavesGame {
    map: Map,
    px: f32,
    py: f32,
    angle: f32,
    fish: Vec<(f32, f32)>,
    seals: Vec<Seal>,
    explored: HashSet<(i32, i32)>,
    hold_fwd: f32,
    hold_back: f32,
    hold_left: f32,
    hold_right: f32,
    hold_sl: f32,
    hold_sr: f32,
    time_left: f32,
    score: u32,
    level: u32,
    combo: u32,
    combo_timer: f32,
    hurt_timer: f32,
    phase: Phase,
    snow: Vec<Flake>,
    particles: Particles,
    popups: Popups,
    bob: f32,
}

impl CavesGame {
    pub fn new() -> Self {
        let mut rng = rand::thread_rng();
        let snow = (0..70)
            .map(|_| Flake { x: rng.gen_range(0.0..1.0), y: rng.gen_range(0.0..1.0), speed: rng.gen_range(0.08..0.3) })
            .collect();
        let mut game = Self {
            map: Map::filled(MAP_W, MAP_H),
            px: 1.5,
            py: 1.5,
            angle: 0.0,
            fish: Vec::new(),
            seals: Vec::new(),
            explored: HashSet::new(),
            hold_fwd: 0.0,
            hold_back: 0.0,
            hold_left: 0.0,
            hold_right: 0.0,
            hold_sl: 0.0,
            hold_sr: 0.0,
            time_left: START_TIME,
            score: 0,
            level: 1,
            combo: 0,
            combo_timer: 0.0,
            hurt_timer: 0.0,
            phase: Phase::Playing,
            snow,
            particles: Particles::default(),
            popups: Popups::default(),
            bob: 0.0,
        };
        game.build_level();
        game
    }

    fn build_level(&mut self) {
        let mut rng = rand::thread_rng();
        // Recursive backtracker on a cell grid, carved into a wall map.
        let mut map = Map::filled(MAP_W, MAP_H);
        let mut visited = [false; CELLS_W * CELLS_H];
        let mut stack = vec![(0usize, 0usize)];
        visited[0] = true;
        map.set(1, 1, false);
        while let Some(&(cx, cy)) = stack.last() {
            let mut options = Vec::new();
            if cx > 0 && !visited[cy * CELLS_W + cx - 1] {
                options.push((cx - 1, cy));
            }
            if cx + 1 < CELLS_W && !visited[cy * CELLS_W + cx + 1] {
                options.push((cx + 1, cy));
            }
            if cy > 0 && !visited[(cy - 1) * CELLS_W + cx] {
                options.push((cx, cy - 1));
            }
            if cy + 1 < CELLS_H && !visited[(cy + 1) * CELLS_W + cx] {
                options.push((cx, cy + 1));
            }
            if options.is_empty() {
                stack.pop();
                continue;
            }
            let (nx, ny) = options[rng.gen_range(0..options.len())];
            visited[ny * CELLS_W + nx] = true;
            let wx = cx * 2 + 1 + (nx as isize - cx as isize) as usize;
            let wy = cy * 2 + 1 + (ny as isize - cy as isize) as usize;
            map.set(wx, wy, false);
            map.set(nx * 2 + 1, ny * 2 + 1, false);
            stack.push((nx, ny));
        }
        // Knock out a few extra walls so there are loops to escape seals.
        for _ in 0..(6 + self.level as usize) {
            let x = rng.gen_range(1..MAP_W - 1);
            let y = rng.gen_range(1..MAP_H - 1);
            if (x % 2 == 1) != (y % 2 == 1) {
                map.set(x, y, false);
            }
        }
        self.map = map;
        self.px = 1.5;
        self.py = 1.5;
        // Face down the open corridor rather than the wall in front of us.
        self.angle = if !self.map.is_wall(2, 1) { 0.0 } else { PI / 2.0 };
        self.explored.clear();

        let open: Vec<(usize, usize)> = self
            .map
            .open_cells()
            .into_iter()
            .filter(|&(x, y)| x > 3 || y > 3)
            .collect();
        let fish_count = (7 + self.level * 2).min(24) as usize;
        self.fish.clear();
        let mut used = HashSet::new();
        while self.fish.len() < fish_count && used.len() < open.len() {
            let (x, y) = open[rng.gen_range(0..open.len())];
            if used.insert((x, y)) {
                self.fish.push((x as f32 + 0.5, y as f32 + 0.5));
            }
        }
        let seal_count = (1 + self.level / 2).min(5) as usize;
        self.seals.clear();
        for _ in 0..seal_count {
            let (x, y) = open[rng.gen_range(0..open.len())];
            if x + y < 12 {
                continue;
            }
            self.seals.push(Seal {
                x: x as f32 + 0.5,
                y: y as f32 + 0.5,
                angle: rng.gen_range(0.0..PI * 2.0),
                speed: 1.2 + self.level as f32 * 0.15,
                retarget: 0.0,
            });
        }
    }
}

impl Game for CavesGame {
    fn kind(&self) -> GameKind {
        GameKind::IceCaves
    }

    fn score(&self) -> u32 {
        self.score
    }

    fn is_over(&self) -> bool {
        self.phase == Phase::GameOver
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        let hold = 0.16;
        match key.code {
            KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') => self.hold_fwd = hold,
            KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => self.hold_back = hold,
            KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('A') => self.hold_left = hold,
            KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => self.hold_right = hold,
            KeyCode::Char('q') | KeyCode::Char('Q') => self.hold_sl = hold,
            KeyCode::Char('e') | KeyCode::Char('E') => self.hold_sr = hold,
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
        self.popups.tick(dt);
        self.combo_timer = (self.combo_timer - dt).max(0.0);
        self.hurt_timer = (self.hurt_timer - dt).max(0.0);
        for h in [&mut self.hold_fwd, &mut self.hold_back, &mut self.hold_left, &mut self.hold_right, &mut self.hold_sl, &mut self.hold_sr] {
            *h = (*h - dt).max(0.0);
        }
        if self.combo_timer <= 0.0 {
            self.combo = 0;
        }

        match self.phase {
            Phase::GameOver => return,
            Phase::LevelClear(tl) => {
                if tl - dt <= 0.0 {
                    self.level += 1;
                    self.score += 500;
                    self.time_left += 30.0;
                    self.build_level();
                    self.phase = Phase::Playing;
                } else {
                    self.phase = Phase::LevelClear(tl - dt);
                }
                return;
            }
            Phase::Playing => {}
        }

        self.time_left -= dt;
        if self.time_left <= 0.0 {
            self.time_left = 0.0;
            self.phase = Phase::GameOver;
            return;
        }

        let turn = 2.6;
        if self.hold_left > 0.0 {
            self.angle -= turn * dt;
        }
        if self.hold_right > 0.0 {
            self.angle += turn * dt;
        }
        let speed = 3.4;
        let mut fwd = 0.0;
        if self.hold_fwd > 0.0 {
            fwd += speed * dt;
        }
        if self.hold_back > 0.0 {
            fwd -= speed * 0.7 * dt;
        }
        let mut side = 0.0;
        if self.hold_sl > 0.0 {
            side -= speed * 0.8 * dt;
        }
        if self.hold_sr > 0.0 {
            side += speed * 0.8 * dt;
        }
        if fwd != 0.0 || side != 0.0 {
            self.bob += dt * 9.0;
            let (c, s) = (self.angle.cos(), self.angle.sin());
            let (dx, dy) = (c * fwd - s * side, s * fwd + c * side);
            self.map.try_move(&mut self.px, &mut self.py, dx, dy, 0.22);
        }
        let (cx, cy) = (self.px as i32, self.py as i32);
        for dy in -2..=2 {
            for dx in -2..=2 {
                self.explored.insert((cx + dx, cy + dy));
            }
        }

        // Fish pickup.
        let mut i = 0;
        while i < self.fish.len() {
            let (fx, fy) = self.fish[i];
            if (fx - self.px).powi(2) + (fy - self.py).powi(2) < 0.45 {
                self.fish.swap_remove(i);
                self.combo += 1;
                self.combo_timer = 4.0;
                let points = 100 * self.combo.min(8);
                self.score += points;
                self.time_left += 5.0;
                self.popups.add(0.5, 0.42, format!("+{}  +5s", points), (255, 170, 80));
                self.particles.burst(0.5, 0.5, 14, (255, 170, 80), 0.9, &['*', '+', '·', '°']);
            } else {
                i += 1;
            }
        }
        if self.fish.is_empty() {
            self.phase = Phase::LevelClear(2.2);
            return;
        }

        // Seals wander and bump.
        for seal in &mut self.seals {
            seal.retarget -= dt;
            let (dx, dy) = (seal.angle.cos() * seal.speed * dt, seal.angle.sin() * seal.speed * dt);
            let (nx, ny) = (seal.x + dx, seal.y + dy);
            if self.map.wall_at(nx, ny) || seal.retarget <= 0.0 {
                seal.angle = [0.0, PI / 2.0, PI, -PI / 2.0][rng.gen_range(0..4)] + rng.gen_range(-0.2..0.2);
                seal.retarget = rng.gen_range(1.0..3.0);
            } else {
                seal.x = nx;
                seal.y = ny;
            }
        }
        if self.hurt_timer <= 0.0 {
            let bumped = self
                .seals
                .iter()
                .any(|seal| (seal.x - self.px).powi(2) + (seal.y - self.py).powi(2) < 0.5);
            if bumped {
                self.hurt_timer = 1.5;
                self.time_left = (self.time_left - 6.0).max(0.5);
                self.score = self.score.saturating_sub(150);
                self.combo = 0;
                self.popups.add(0.5, 0.42, "SEAL!  -6s", (255, 90, 90));
                let (c, s) = (self.angle.cos(), self.angle.sin());
                self.map.try_move(&mut self.px, &mut self.py, -c * 0.8, -s * 0.8, 0.22);
            }
        }
    }

    fn render(&self, grid: &mut CellGrid, phase: f32) {
        let w = grid.width as i32;
        let hud = 2i32;
        let view_h = grid.height as i32 - hud;
        if view_h < 4 {
            return;
        }

        let hurt = self.hurt_timer > 0.0 && ((phase * 10.0) as i32) % 2 == 0;
        let cam = Camera {
            x: self.px,
            y: self.py,
            angle: self.angle,
            bob: (self.bob.sin() * 0.6).round() as i32,
            fov: FOV,
        };
        let mut sprites: Vec<Sprite> = self
            .fish
            .iter()
            .map(|&(x, y)| Sprite { x, y, kind: SpriteKind::Fish, color: (255, 165, 70), label: None })
            .collect();
        sprites.extend(self.seals.iter().map(|s| Sprite {
            x: s.x,
            y: s.y,
            kind: SpriteKind::Seal,
            color: (200, 200, 210),
            label: None,
        }));
        let snow: Vec<(f32, f32, bool)> = self.snow.iter().map(|f| (f.x, f.y, f.speed > 0.2)).collect();
        raycast::render_view(
            grid,
            &self.map,
            &cam,
            &Palette::ice(),
            if hurt { Some((255, 120, 120)) } else { None },
            &sprites,
            &snow,
            hud,
        );

        self.particles.draw(grid, |x, y| ((x * w as f32) as i32, hud + (y * view_h as f32) as i32));
        self.popups.draw(grid, |x, y| ((x * w as f32) as i32, hud + (y * view_h as f32) as i32));

        // HUD
        let bg = t().panel_bg;
        let timer_color = if self.time_left < 12.0 && ((phase * 4.0) as i32) % 2 == 0 { t().danger } else { t().accent2 };
        grid.text(0, 0, &format!("LVL {:>2}   FISH LEFT {:>2}   SCORE {:06}", self.level, self.fish.len(), self.score), t().accent2, bg);
        let combo = if self.combo > 1 { format!("   combo x{}", self.combo) } else { String::new() };
        grid.text(0, 1, &format!("TIME {:>4.1}s{}   W/S move  A/D turn  Q/E strafe", self.time_left, combo), timer_color, bg);

        // Minimap (fog of war) in the top-right of the view.
        let mm_w = MAP_W as i32;
        let mm_h = MAP_H as i32;
        if w > mm_w + 30 && view_h > mm_h + 2 {
            let x0 = w - mm_w - 1;
            let y0 = hud + 1;
            let panel = rgb(20, 26, 40);
            for y in 0..mm_h {
                for x in 0..mm_w {
                    let known = self.explored.contains(&(x, y));
                    let (ch, c) = if !known {
                        (' ', (20, 26, 40))
                    } else if self.map.is_wall(x, y) {
                        ('▪', (110, 150, 190))
                    } else {
                        (' ', (40, 60, 90))
                    };
                    grid.set(x0 + x, y0 + y, ch, rgb(c.0, c.1, c.2), panel);
                }
            }
            for &(fx, fy) in &self.fish {
                if self.explored.contains(&(fx as i32, fy as i32)) {
                    grid.put(x0 + fx as i32, y0 + fy as i32, 'f', rgb(255, 165, 70));
                }
            }
            for seal in &self.seals {
                if self.explored.contains(&(seal.x as i32, seal.y as i32)) {
                    grid.put(x0 + seal.x as i32, y0 + seal.y as i32, 's', rgb(230, 230, 240));
                }
            }
            let arrow = match ((self.angle / (PI / 4.0)).round() as i32).rem_euclid(8) {
                0 => '►',
                1 | 2 => '▼',
                3 | 4 => '◄',
                _ => '▲',
            };
            grid.put(x0 + self.px as i32, y0 + self.py as i32, arrow, t().accent2);
        }

        match self.phase {
            Phase::GameOver => {
                let mid = grid.height as i32 / 2;
                grid.banner(mid, "OUT OF TIME", t().danger, t().bg_base);
                grid.center_text(mid + 2, "R restart   Esc menu", t().text, t().bg_base);
            }
            Phase::LevelClear(_) => {
                let mid = grid.height as i32 / 2;
                grid.banner(mid, &format!("CAVE {} CLEARED  +500  +30s", self.level), t().accent2, t().bg_base);
            }
            Phase::Playing => {}
        }
    }
}
