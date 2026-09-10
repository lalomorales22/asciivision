//! Space Invaders: three animated squadron types, destructible bunkers, a
//! mystery UFO, smooth held-key movement, explosions, and waves that speed
//! up as the squadron thins out.

use super::{project_axis, rgb, CellGrid, Game, GameKind, Particles, Popups};
use crate::theme::t;
use crossterm::event::{KeyCode, KeyEvent};
use rand::Rng;
use std::collections::HashMap;

const FW: f32 = 64.0;
const FH: f32 = 30.0;
const ROWS: usize = 5;
const COLS: usize = 10;
const PLAYER_Y: f32 = FH - 2.0;

#[derive(Clone)]
struct Invader {
    x: f32,
    y: f32,
    kind: u8,
    alive: bool,
}

#[derive(Clone)]
struct Bullet {
    x: f32,
    y: f32,
    dy: f32,
    friendly: bool,
}

struct Star {
    x: f32,
    y: f32,
    speed: f32,
}

struct Ufo {
    x: f32,
    dir: f32,
    active: bool,
    next_in: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Playing,
    WaveClear(f32),
    GameOver,
}

pub struct InvadersGame {
    player_x: f32,
    player_alive: bool,
    respawn_timer: f32,
    invuln_timer: f32,
    hold_left: f32,
    hold_right: f32,
    bullets: Vec<Bullet>,
    invaders: Vec<Invader>,
    frame: bool,
    bunkers: HashMap<(i32, i32), u8>,
    stars: Vec<Star>,
    ufo: Ufo,
    enemy_dir: f32,
    enemy_timer: f32,
    enemy_fire_timer: f32,
    shot_cooldown: f32,
    shield_cooldown: f32,
    shield_timer: f32,
    score: u32,
    lives: u8,
    wave: u32,
    phase: Phase,
    particles: Particles,
    popups: Popups,
}

impl InvadersGame {
    pub fn new() -> Self {
        let mut rng = rand::thread_rng();
        let stars = (0..110)
            .map(|_| Star {
                x: rng.gen_range(0.0..FW),
                y: rng.gen_range(0.0..FH),
                speed: rng.gen_range(1.5..7.0),
            })
            .collect();
        let mut game = Self {
            player_x: FW / 2.0,
            player_alive: true,
            respawn_timer: 0.0,
            invuln_timer: 0.0,
            hold_left: 0.0,
            hold_right: 0.0,
            bullets: Vec::new(),
            invaders: Vec::new(),
            frame: false,
            bunkers: HashMap::new(),
            stars,
            ufo: Ufo { x: -5.0, dir: 1.0, active: false, next_in: rng.gen_range(12.0..20.0) },
            enemy_dir: 1.0,
            enemy_timer: 0.0,
            enemy_fire_timer: 1.0,
            shot_cooldown: 0.0,
            shield_cooldown: 0.0,
            shield_timer: 0.0,
            score: 0,
            lives: 3,
            wave: 1,
            phase: Phase::Playing,
            particles: Particles::default(),
            popups: Popups::default(),
        };
        game.spawn_wave();
        game.build_bunkers();
        game
    }

    fn spawn_wave(&mut self) {
        self.invaders.clear();
        let start_y = 3.0 + (self.wave as f32 - 1.0).min(4.0);
        for row in 0..ROWS {
            for col in 0..COLS {
                self.invaders.push(Invader {
                    x: 9.0 + col as f32 * 4.6,
                    y: start_y + row as f32 * 2.0,
                    kind: match row {
                        0 => 0,
                        1 | 2 => 1,
                        _ => 2,
                    },
                    alive: true,
                });
            }
        }
        self.enemy_dir = 1.0;
        self.enemy_timer = 0.0;
        self.enemy_fire_timer = 1.2;
        self.bullets.clear();
    }

    fn build_bunkers(&mut self) {
        self.bunkers.clear();
        for i in 0..4 {
            let cx = (FW * (i as f32 + 1.0) / 5.0).round() as i32;
            for dy in 0..3 {
                for dx in -3i32..=3 {
                    // Notch the bottom centre like the arcade bunkers.
                    if dy == 2 && dx.abs() <= 1 {
                        continue;
                    }
                    self.bunkers.insert((cx + dx, (FH as i32) - 6 + dy), 2);
                }
            }
        }
    }

    fn fire(&mut self) {
        if self.shot_cooldown > 0.0 || !self.player_alive || self.phase != Phase::Playing {
            return;
        }
        if self.bullets.iter().filter(|b| b.friendly).count() >= 2 {
            return;
        }
        self.bullets.push(Bullet { x: self.player_x, y: PLAYER_Y - 1.0, dy: -34.0, friendly: true });
        self.shot_cooldown = 0.22;
    }

    fn kill_player(&mut self) {
        if self.shield_timer > 0.0 || self.invuln_timer > 0.0 || !self.player_alive {
            return;
        }
        self.particles.burst(self.player_x, PLAYER_Y, 26, (255, 200, 90), 14.0, &['*', '+', '·', '\'']);
        self.player_alive = false;
        if self.lives > 1 {
            self.lives -= 1;
            self.respawn_timer = 1.4;
        } else {
            self.lives = 0;
            self.phase = Phase::GameOver;
        }
    }

    fn invader_color(kind: u8) -> (u8, u8, u8) {
        match kind {
            0 => (255, 120, 210),
            1 => (120, 235, 130),
            _ => (120, 200, 255),
        }
    }
}

impl Game for InvadersGame {
    fn kind(&self) -> GameKind {
        GameKind::SpaceInvaders
    }

    fn score(&self) -> u32 {
        self.score
    }

    fn is_over(&self) -> bool {
        self.phase == Phase::GameOver
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('A') => {
                self.hold_left = 0.14;
                true
            }
            KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => {
                self.hold_right = 0.14;
                true
            }
            KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') | KeyCode::Char(' ') => {
                self.fire();
                true
            }
            KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => {
                if self.shield_cooldown <= 0.0 && self.player_alive && self.phase == Phase::Playing {
                    self.shield_timer = 1.0;
                    self.shield_cooldown = 6.0;
                }
                true
            }
            _ => false,
        }
    }

    fn tick(&mut self, dt: f32) {
        let mut rng = rand::thread_rng();
        for star in &mut self.stars {
            star.y += star.speed * dt;
            if star.y >= FH {
                star.y = 0.0;
                star.x = rng.gen_range(0.0..FW);
            }
        }
        self.particles.tick(dt, 6.0);
        self.popups.tick(dt);
        self.shot_cooldown = (self.shot_cooldown - dt).max(0.0);
        self.shield_cooldown = (self.shield_cooldown - dt).max(0.0);
        self.shield_timer = (self.shield_timer - dt).max(0.0);
        self.invuln_timer = (self.invuln_timer - dt).max(0.0);
        self.hold_left = (self.hold_left - dt).max(0.0);
        self.hold_right = (self.hold_right - dt).max(0.0);

        match self.phase {
            Phase::GameOver => return,
            Phase::WaveClear(tl) => {
                if tl - dt <= 0.0 {
                    self.wave += 1;
                    self.score += 200;
                    self.spawn_wave();
                    self.build_bunkers();
                    self.phase = Phase::Playing;
                } else {
                    self.phase = Phase::WaveClear(tl - dt);
                }
                return;
            }
            Phase::Playing => {}
        }

        // Player movement and respawn.
        if !self.player_alive {
            self.respawn_timer -= dt;
            if self.respawn_timer <= 0.0 {
                self.player_alive = true;
                self.player_x = FW / 2.0;
                self.invuln_timer = 2.0;
            }
        } else {
            let speed = 30.0;
            if self.hold_left > 0.0 {
                self.player_x -= speed * dt;
            }
            if self.hold_right > 0.0 {
                self.player_x += speed * dt;
            }
            self.player_x = self.player_x.clamp(2.0, FW - 3.0);
        }

        // Squadron march: faster as it thins out.
        let alive = self.invaders.iter().filter(|i| i.alive).count();
        let total = ROWS * COLS;
        let ratio = alive as f32 / total as f32;
        let step_time = (0.62 * ratio.sqrt() - self.wave as f32 * 0.03).clamp(0.07, 0.62);
        self.enemy_timer += dt;
        if self.enemy_timer >= step_time {
            self.enemy_timer = 0.0;
            self.frame = !self.frame;
            let dx = 1.4 * self.enemy_dir;
            let hit_edge = self
                .invaders
                .iter()
                .filter(|i| i.alive)
                .any(|i| i.x + dx >= FW - 2.5 || i.x + dx <= 2.5);
            if hit_edge {
                self.enemy_dir *= -1.0;
                for inv in self.invaders.iter_mut().filter(|i| i.alive) {
                    inv.y += 1.0;
                }
            } else {
                for inv in self.invaders.iter_mut().filter(|i| i.alive) {
                    inv.x += dx;
                }
            }
        }

        // Bombs: the lowest invader in a random column fires.
        self.enemy_fire_timer -= dt;
        if self.enemy_fire_timer <= 0.0 {
            let mut lowest: HashMap<i32, (f32, f32)> = HashMap::new();
            for inv in self.invaders.iter().filter(|i| i.alive) {
                let key = (inv.x / 4.6).round() as i32;
                let entry = lowest.entry(key).or_insert((inv.x, inv.y));
                if inv.y > entry.1 {
                    *entry = (inv.x, inv.y);
                }
            }
            let shooters: Vec<(f32, f32)> = lowest.values().copied().collect();
            if !shooters.is_empty() {
                let (x, y) = shooters[rng.gen_range(0..shooters.len())];
                self.bullets.push(Bullet { x, y: y + 1.0, dy: 13.0 + self.wave as f32 * 1.2, friendly: false });
            }
            self.enemy_fire_timer = (1.3 - self.wave as f32 * 0.08).clamp(0.4, 1.3) * rng.gen_range(0.6..1.3);
        }

        // UFO.
        if self.ufo.active {
            self.ufo.x += self.ufo.dir * 9.0 * dt;
            if self.ufo.x < -6.0 || self.ufo.x > FW + 6.0 {
                self.ufo.active = false;
                self.ufo.next_in = rng.gen_range(14.0..26.0);
            }
        } else {
            self.ufo.next_in -= dt;
            if self.ufo.next_in <= 0.0 {
                self.ufo.active = true;
                self.ufo.dir = if rng.gen_bool(0.5) { 1.0 } else { -1.0 };
                self.ufo.x = if self.ufo.dir > 0.0 { -4.0 } else { FW + 4.0 };
            }
        }

        // Bullets.
        for b in &mut self.bullets {
            b.y += b.dy * dt;
        }
        let mut kills: Vec<(f32, f32, u8)> = Vec::new();
        let mut player_hit = false;
        let mut ufo_hit = false;
        for b in &mut self.bullets {
            if b.y < -1.0 || b.y > FH + 1.0 {
                continue;
            }
            // Bunkers absorb anything.
            let cell = (b.x.round() as i32, b.y.round() as i32);
            if let Some(hp) = self.bunkers.get_mut(&cell) {
                *hp -= 1;
                if *hp == 0 {
                    self.bunkers.remove(&cell);
                }
                b.y = -100.0;
                continue;
            }
            if b.friendly {
                if self.ufo.active && (self.ufo.x - b.x).abs() < 2.0 && b.y < 2.5 {
                    ufo_hit = true;
                    b.y = -100.0;
                    continue;
                }
                for inv in self.invaders.iter_mut().filter(|i| i.alive) {
                    if (inv.x - b.x).abs() < 1.6 && (inv.y - b.y).abs() < 1.0 {
                        inv.alive = false;
                        kills.push((inv.x, inv.y, inv.kind));
                        b.y = -100.0;
                        break;
                    }
                }
            } else if self.player_alive
                && (b.x - self.player_x).abs() < 1.6
                && (b.y - PLAYER_Y).abs() < 1.0
            {
                if self.shield_timer > 0.0 {
                    b.y = -100.0;
                    self.particles.burst(b.x, PLAYER_Y - 1.0, 4, (120, 220, 255), 6.0, &['·', '*']);
                } else {
                    player_hit = true;
                    b.y = -100.0;
                }
            }
        }
        self.bullets.retain(|b| b.y >= -0.5 && b.y <= FH + 0.5);
        for (x, y, kind) in kills {
            let points = match kind {
                0 => 30,
                1 => 20,
                _ => 10,
            } * self.wave.min(5);
            self.score += points;
            self.particles.burst(x, y, 12, Self::invader_color(kind), 9.0, &['*', '+', '·', '\'']);
        }
        if ufo_hit {
            let bonus = 50 * rng.gen_range(1..=6);
            self.score += bonus;
            self.popups.add(self.ufo.x, 1.0, format!("{}", bonus), (255, 90, 90));
            self.particles.burst(self.ufo.x, 1.0, 20, (255, 90, 90), 12.0, &['*', '+', '·']);
            self.ufo.active = false;
            self.ufo.next_in = rng.gen_range(14.0..26.0);
        }
        if player_hit {
            self.kill_player();
        }

        // Squadron reaching the bunkers or the player ends the game.
        if self.invaders.iter().filter(|i| i.alive).any(|i| i.y >= FH - 5.0) {
            self.phase = Phase::GameOver;
        }
        if alive == 0 && self.phase == Phase::Playing {
            self.phase = Phase::WaveClear(2.0);
        }
    }

    fn render(&self, grid: &mut CellGrid, phase: f32) {
        let bg = t().panel_bg;
        let hud = 2i32;
        let field_h = grid.height.saturating_sub(hud as u16);
        let gw = grid.width;
        let sx = gw as f32 / FW;
        let wide = sx >= 0.85;
        let project = move |x: f32, y: f32| -> (i32, i32) {
            (project_axis(x, FW, gw), project_axis(y, FH, field_h) + hud)
        };

        grid.text(0, 0, &format!("WAVE {:>2}   SCORE {:06}   {}", self.wave, self.score, "♥".repeat(self.lives as usize)), t().accent2, bg);
        let shield = if self.shield_cooldown <= 0.0 { "S shield READY" } else { "S shield charging" };
        grid.text(0, 1, &format!("A/D move  W/Space fire  {}", shield), t().accent4, bg);

        for star in &self.stars {
            let (x, y) = project(star.x, star.y);
            let ch = if star.speed > 5.0 { '·' } else { '.' };
            grid.set(x, y, ch, t().muted, bg);
        }

        // Bunkers.
        for (&(bx, by), &hp) in &self.bunkers {
            let (x, y) = project(bx as f32, by as f32);
            let (ch, color) = if hp >= 2 { ('█', rgb(120, 230, 130)) } else { ('▒', rgb(90, 160, 100)) };
            grid.set(x, y, ch, color, bg);
        }

        // Squadron.
        for inv in self.invaders.iter().filter(|i| i.alive) {
            let (x, y) = project(inv.x, inv.y);
            let (r, g, b) = Self::invader_color(inv.kind);
            let color = rgb(r, g, b);
            if wide {
                let sprite = match (inv.kind, self.frame) {
                    (0, false) => "╭o╮",
                    (0, true) => "╰o╯",
                    (1, false) => "{@}",
                    (1, true) => "[@]",
                    (_, false) => "<=>",
                    (_, true) => ">=<",
                };
                grid.text(x - 1, y, sprite, color, bg);
            } else {
                let ch = match inv.kind {
                    0 => 'o',
                    1 => '@',
                    _ => '=',
                };
                grid.set(x, y, ch, color, bg);
            }
        }

        // UFO.
        if self.ufo.active {
            let (x, y) = project(self.ufo.x, 1.0);
            let blink = ((phase * 10.0) as i32) % 2 == 0;
            grid.text(x - 2, y, if blink { "<▓O▓>" } else { "<▒O▒>" }, rgb(255, 90, 90), bg);
        }

        // Bullets.
        for b in &self.bullets {
            let (x, y) = project(b.x, b.y);
            if b.friendly {
                grid.set(x, y, '|', t().accent4, bg);
            } else {
                let ch = if ((phase * 12.0) as i32 + x) % 2 == 0 { '¡' } else { '¦' };
                grid.set(x, y, ch, t().danger, bg);
            }
        }

        // Player.
        if self.player_alive {
            let (x, y) = project(self.player_x, PLAYER_Y);
            let visible = self.invuln_timer <= 0.0 || ((phase * 10.0) as i32) % 2 == 0;
            if visible {
                if wide {
                    grid.text(x - 1, y, "▟▲▙", t().accent3, bg);
                } else {
                    grid.set(x, y, 'A', t().accent3, bg);
                }
            }
            if self.shield_timer > 0.0 {
                let c = rgb(120, 220, 255);
                grid.set(x - 2, y, '(', c, bg);
                grid.set(x + 2, y, ')', c, bg);
                grid.text(x - 1, y - 1, "╌╌╌", c, bg);
            }
        }

        self.particles.draw(grid, project);
        self.popups.draw(grid, project);

        match self.phase {
            Phase::GameOver => {
                let mid = grid.height as i32 / 2;
                grid.banner(mid, "DEFENSE LINE COLLAPSED", t().danger, t().bg_base);
                grid.center_text(mid + 2, "R restart   Esc menu", t().text, t().bg_base);
            }
            Phase::WaveClear(_) => {
                let mid = grid.height as i32 / 2;
                grid.banner(mid, &format!("WAVE {} CLEARED", self.wave), t().accent2, t().bg_base);
            }
            Phase::Playing => {}
        }
    }
}
