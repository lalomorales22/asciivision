//! Penguin Ice Caves: a first-person raycast maze. Collect every fish
//! before the clock runs out while seals patrol the tunnels. Uses DDA
//! raycasting with a per-column depth buffer, distance fog, side shading,
//! depth-sorted billboard sprites, a fog-of-war minimap, and falling snow.

use super::{rgb, CellGrid, Game, GameKind, Particles, Popups};
use crate::theme::t;
use crossterm::event::{KeyCode, KeyEvent};
use rand::Rng;
use ratatui::prelude::Color;
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
    map: Vec<bool>, // true = wall
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
    flash_timer: f32,
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
            map: Vec::new(),
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
            flash_timer: 0.0,
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
        let mut map = vec![true; MAP_W * MAP_H];
        let mut visited = [false; CELLS_W * CELLS_H];
        let mut stack = vec![(0usize, 0usize)];
        visited[0] = true;
        map[MAP_W + 1] = false;
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
            map[wy * MAP_W + wx] = false;
            map[(ny * 2 + 1) * MAP_W + nx * 2 + 1] = false;
            stack.push((nx, ny));
        }
        // Knock out a few extra walls so there are loops to escape seals.
        for _ in 0..(6 + self.level as usize) {
            let x = rng.gen_range(1..MAP_W - 1);
            let y = rng.gen_range(1..MAP_H - 1);
            if (x % 2 == 1) != (y % 2 == 1) {
                map[y * MAP_W + x] = false;
            }
        }
        self.map = map;
        self.px = 1.5;
        self.py = 1.5;
        // Face down the open corridor rather than the wall in front of us.
        self.angle = if !self.map[MAP_W + 2] { 0.0 } else { PI / 2.0 };
        self.explored.clear();

        let open: Vec<(usize, usize)> = (0..MAP_H)
            .flat_map(|y| (0..MAP_W).map(move |x| (x, y)))
            .filter(|&(x, y)| !self.map[y * MAP_W + x] && (x > 3 || y > 3))
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

    fn wall_at(&self, x: f32, y: f32) -> bool {
        if x < 0.0 || y < 0.0 {
            return true;
        }
        let (xi, yi) = (x as usize, y as usize);
        if xi >= MAP_W || yi >= MAP_H {
            return true;
        }
        self.map[yi * MAP_W + xi]
    }

    fn try_move(&mut self, dx: f32, dy: f32) {
        let r = 0.22;
        let nx = self.px + dx;
        if !self.wall_at(nx + r * dx.signum(), self.py - r) && !self.wall_at(nx + r * dx.signum(), self.py + r) {
            self.px = nx;
        }
        let ny = self.py + dy;
        if !self.wall_at(self.px - r, ny + r * dy.signum()) && !self.wall_at(self.px + r, ny + r * dy.signum()) {
            self.py = ny;
        }
    }

    /// DDA raycast. Returns (perpendicular distance, hit a vertical wall side).
    fn cast(&self, angle: f32) -> (f32, bool) {
        let (dir_x, dir_y) = (angle.cos(), angle.sin());
        let mut map_x = self.px as i32;
        let mut map_y = self.py as i32;
        let delta_x = if dir_x.abs() < 1e-6 { 1e6 } else { (1.0 / dir_x).abs() };
        let delta_y = if dir_y.abs() < 1e-6 { 1e6 } else { (1.0 / dir_y).abs() };
        let (step_x, mut side_x) = if dir_x < 0.0 {
            (-1, (self.px - map_x as f32) * delta_x)
        } else {
            (1, (map_x as f32 + 1.0 - self.px) * delta_x)
        };
        let (step_y, mut side_y) = if dir_y < 0.0 {
            (-1, (self.py - map_y as f32) * delta_y)
        } else {
            (1, (map_y as f32 + 1.0 - self.py) * delta_y)
        };
        for _ in 0..80 {
            let vertical = if side_x < side_y {
                side_x += delta_x;
                map_x += step_x;
                true
            } else {
                side_y += delta_y;
                map_y += step_y;
                false
            };
            if map_x < 0 || map_y < 0 || map_x as usize >= MAP_W || map_y as usize >= MAP_H {
                break;
            }
            if self.map[map_y as usize * MAP_W + map_x as usize] {
                let dist = if vertical { side_x - delta_x } else { side_y - delta_y };
                return (dist.max(0.05), vertical);
            }
        }
        (40.0, false)
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
        self.flash_timer = (self.flash_timer - dt).max(0.0);
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
            self.try_move(c * fwd - s * side, s * fwd + c * side);
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
                self.flash_timer = 0.4;
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
            let blocked = {
                let xi = nx as usize;
                let yi = ny as usize;
                xi >= MAP_W || yi >= MAP_H || self.map[yi * MAP_W + xi]
            };
            if blocked || seal.retarget <= 0.0 {
                seal.angle = [0.0, PI / 2.0, PI, -PI / 2.0][rng.gen_range(0..4)] + rng.gen_range(-0.2..0.2);
                seal.retarget = rng.gen_range(1.0..3.0);
            } else {
                seal.x = nx;
                seal.y = ny;
            }
        }
        if self.hurt_timer <= 0.0 {
            let mut bumped = false;
            for seal in &self.seals {
                if (seal.x - self.px).powi(2) + (seal.y - self.py).powi(2) < 0.5 {
                    bumped = true;
                }
            }
            if bumped {
                self.hurt_timer = 1.5;
                self.time_left = (self.time_left - 6.0).max(0.5);
                self.score = self.score.saturating_sub(150);
                self.combo = 0;
                self.popups.add(0.5, 0.42, "SEAL!  -6s", (255, 90, 90));
                // Shove the player back a step.
                let (c, s) = (self.angle.cos(), self.angle.sin());
                self.try_move(-c * 0.8, -s * 0.8);
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
        let horizon = hud + view_h / 2;
        let bob = (self.bob.sin() * 0.6).round() as i32;

        // Sky and floor.
        let sky_top = (10u8, 18u8, 40u8);
        let sky_bot = (30u8, 50u8, 90u8);
        let floor_far = (60u8, 90u8, 120u8);
        let floor_near = (170u8, 215u8, 235u8);
        for y in hud..grid.height as i32 {
            let bg = if y < horizon {
                let k = (y - hud) as f32 / (horizon - hud).max(1) as f32;
                lerp(sky_top, sky_bot, k)
            } else {
                let k = (y - horizon) as f32 / (grid.height as i32 - horizon).max(1) as f32;
                lerp(floor_far, floor_near, k)
            };
            for x in 0..w {
                grid.set(x, y, ' ', bg, bg);
            }
        }
        for flake in &self.snow {
            let x = (flake.x * w as f32) as i32;
            let y = hud + (flake.y * (horizon - hud).max(1) as f32) as i32;
            grid.put(x, y, if flake.speed > 0.2 { '*' } else { '·' }, rgb(230, 240, 250));
        }

        // Walls.
        let mut depth = vec![f32::INFINITY; w as usize];
        let hurt = self.hurt_timer > 0.0 && ((phase * 10.0) as i32) % 2 == 0;
        for x in 0..w {
            let cam = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
            let ray = self.angle + cam * FOV / 2.0;
            let (dist, vertical) = self.cast(ray);
            let perp = dist * (ray - self.angle).cos();
            depth[x as usize] = perp;
            let height = ((view_h as f32 * 1.1) / perp.max(0.3)).min(view_h as f32 * 3.0) as i32;
            let top = horizon - height / 2 + bob;
            let bottom = horizon + height / 2 + bob;
            let fog = (1.0 - perp / 14.0).clamp(0.12, 1.0);
            let base = if hurt { (255, 120, 120) } else if vertical { (120, 175, 225) } else { (165, 210, 240) };
            let color = lerp((8, 14, 30), base, fog);
            let ch = if perp < 1.6 {
                '█'
            } else if perp < 3.5 {
                '▓'
            } else if perp < 7.0 {
                '▒'
            } else {
                '░'
            };
            for y in top.max(hud)..=bottom.min(grid.height as i32 - 1) {
                // Faint horizontal striations give the ice some texture.
                let stripe = (y + x / 3) % 7 == 0 && perp < 5.0;
                let c = if stripe { lerp(color_tuple(color), (255, 255, 255), 0.18) } else { color };
                grid.set(x, y, ch, c, c);
            }
        }

        // Sprites, far to near.
        struct Sprite {
            dist: f32,
            screen_x: f32,
            seal: bool,
        }
        let mut sprites: Vec<Sprite> = Vec::new();
        let (ca, sa) = (self.angle.cos(), self.angle.sin());
        let push = |x: f32, y: f32, seal: bool, sprites: &mut Vec<Sprite>| {
            let (dx, dy) = (x - self.px, y - self.py);
            let forward = dx * ca + dy * sa;
            let right = -dx * sa + dy * ca;
            if forward <= 0.2 {
                return;
            }
            let screen = (0.5 + (right / forward) / FOV) * w as f32;
            sprites.push(Sprite { dist: forward, screen_x: screen, seal });
        };
        for &(fx, fy) in &self.fish {
            push(fx, fy, false, &mut sprites);
        }
        for seal in &self.seals {
            push(seal.x, seal.y, true, &mut sprites);
        }
        sprites.sort_by(|a, b| b.dist.partial_cmp(&a.dist).unwrap_or(std::cmp::Ordering::Equal));
        for sp in &sprites {
            let size = (view_h as f32 * 0.9 / sp.dist).clamp(1.0, view_h as f32) as i32;
            let text = if sp.seal {
                match size {
                    s if s >= 8 => "(ó.ò)",
                    s if s >= 4 => "(oo)",
                    s if s >= 2 => "oo",
                    _ => "o",
                }
            } else {
                match size {
                    s if s >= 9 => "><(((°>",
                    s if s >= 5 => "><((°>",
                    s if s >= 3 => "><°>",
                    s if s >= 2 => "><>",
                    _ => "<",
                }
            };
            let fog = (1.0 - sp.dist / 14.0).clamp(0.2, 1.0);
            let base = if sp.seal { (200, 200, 210) } else { (255, 165, 70) };
            let color = lerp((10, 20, 40), base, fog);
            let y = horizon + (size / 4).max(0) + bob;
            let half = text.chars().count() as i32 / 2;
            let x0 = sp.screen_x.round() as i32 - half;
            for (i, ch) in text.chars().enumerate() {
                let x = x0 + i as i32;
                if x < 0 || x >= w {
                    continue;
                }
                if depth[x as usize] > sp.dist {
                    grid.put(x, y, ch, color);
                    if size >= 6 && !sp.seal {
                        grid.put(x, y - 1, if i % 2 == 0 { '~' } else { ' ' }, lerp((10, 20, 40), (120, 200, 255), fog));
                    }
                }
            }
        }

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
            for y in 0..mm_h {
                for x in 0..mm_w {
                    let known = self.explored.contains(&(x, y));
                    let wall = self.map[y as usize * MAP_W + x as usize];
                    let (ch, c) = if !known {
                        (' ', (20, 26, 40))
                    } else if wall {
                        ('▪', (110, 150, 190))
                    } else {
                        (' ', (40, 60, 90))
                    };
                    grid.set(x0 + x, y0 + y, ch, rgb(c.0, c.1, c.2), rgb(20, 26, 40));
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

fn lerp(a: (u8, u8, u8), b: (u8, u8, u8), k: f32) -> Color {
    let k = k.clamp(0.0, 1.0);
    rgb(
        (a.0 as f32 + (b.0 as f32 - a.0 as f32) * k) as u8,
        (a.1 as f32 + (b.1 as f32 - a.1 as f32) * k) as u8,
        (a.2 as f32 + (b.2 as f32 - a.2 as f32) * k) as u8,
    )
}

fn color_tuple(c: Color) -> (u8, u8, u8) {
    crate::theme::color_to_rgb(c)
}
