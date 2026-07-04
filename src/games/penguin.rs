//! 3D Penguin: ported unchanged onto the `Game` trait.

use crossterm::event::{KeyCode, KeyEvent};
use rand::Rng;
use ratatui::prelude::*;
use std::f32::consts::PI;

use super::{CellGrid, Game};
use crate::theme::t;

const PENGUIN_WORLD_W: f32 = 120.0;
const PENGUIN_WORLD_H: f32 = 90.0;

#[derive(Clone)]
struct Fish {
    x: f32,
    y: f32,
    bob: f32,
    alive: bool,
}

struct Snow {
    x: f32,
    y: f32,
    speed: f32,
}

pub(super) struct PenguinGame {
    x: f32,
    y: f32,
    facing: f32,
    fishes: Vec<Fish>,
    snow: Vec<Snow>,
    score: u32,
    level: u32,
    combo: u32,
    combo_timer: f32,
    flash_timer: f32,
    elapsed: f32,
    hold_forward: f32,
    hold_back: f32,
    hold_left: f32,
    hold_right: f32,
}

impl PenguinGame {
    pub(super) fn new() -> Self {
        let mut game = Self {
            x: PENGUIN_WORLD_W / 2.0,
            y: PENGUIN_WORLD_H / 2.0,
            facing: 0.0,
            fishes: Vec::new(),
            snow: (0..48)
                .map(|_| {
                    let mut rng = rand::thread_rng();
                    Snow {
                        x: rng.gen_range(0.0..1.0),
                        y: rng.gen_range(0.0..1.0),
                        speed: rng.gen_range(0.12..0.45),
                    }
                })
                .collect(),
            score: 0,
            level: 1,
            combo: 0,
            combo_timer: 0.0,
            flash_timer: 0.0,
            elapsed: 0.0,
            hold_forward: 0.0,
            hold_back: 0.0,
            hold_left: 0.0,
            hold_right: 0.0,
        };
        game.spawn_fishes();
        game
    }

    fn restart(&mut self) {
        *self = Self::new();
    }

    fn spawn_fishes(&mut self) {
        self.fishes.clear();
        let count = 9 + self.level as usize * 2;
        let mut rng = rand::thread_rng();
        for _ in 0..count {
            self.fishes.push(Fish {
                x: rng.gen_range(8.0..PENGUIN_WORLD_W - 8.0),
                y: rng.gen_range(8.0..PENGUIN_WORLD_H - 8.0),
                bob: rng.gen_range(0.0..PI * 2.0),
                alive: true,
            });
        }
    }

    fn render_penguin_minimap(&self, grid: &mut CellGrid, sky: Color, ice: Color) {
        let map_w = 16i32;
        let map_h = 8i32;
        let x0 = grid.width as i32 - map_w - 1;
        let y0 = 2i32;
        if x0 < 0 {
            return;
        }

        for y in 0..map_h {
            for x in 0..map_w {
                let border = x == 0 || y == 0 || x == map_w - 1 || y == map_h - 1;
                grid.set(
                    x0 + x,
                    y0 + y,
                    if border { '#' } else { ' ' },
                    t().muted,
                    sky,
                );
            }
        }

        for fish in self.fishes.iter().filter(|fish| fish.alive) {
            let x = x0 + 1 + ((fish.x / PENGUIN_WORLD_W) * (map_w - 2) as f32) as i32;
            let y = y0 + 1 + ((fish.y / PENGUIN_WORLD_H) * (map_h - 2) as f32) as i32;
            grid.set(x, y, 'f', Color::Rgb(255, 148, 79), sky);
        }
        let px = x0 + 1 + ((self.x / PENGUIN_WORLD_W) * (map_w - 2) as f32) as i32;
        let py = y0 + 1 + ((self.y / PENGUIN_WORLD_H) * (map_h - 2) as f32) as i32;
        grid.set(px, py, 'P', t().accent4, ice);
    }
}

impl Game for PenguinGame {
    fn tick(&mut self, dt: f32) {
        self.elapsed += dt;
        self.combo_timer = (self.combo_timer - dt).max(0.0);
        self.flash_timer = (self.flash_timer - dt).max(0.0);
        self.hold_forward = (self.hold_forward - dt).max(0.0);
        self.hold_back = (self.hold_back - dt).max(0.0);
        self.hold_left = (self.hold_left - dt).max(0.0);
        self.hold_right = (self.hold_right - dt).max(0.0);

        let turn_speed = 2.5;
        if self.hold_left > 0.0 {
            self.facing -= turn_speed * dt;
        }
        if self.hold_right > 0.0 {
            self.facing += turn_speed * dt;
        }

        let move_speed = 20.0 + self.level as f32 * 2.0;
        let mut distance = 0.0;
        if self.hold_forward > 0.0 {
            distance += move_speed * dt;
        }
        if self.hold_back > 0.0 {
            distance -= move_speed * dt * 0.75;
        }
        self.x = (self.x + self.facing.cos() * distance).clamp(2.0, PENGUIN_WORLD_W - 2.0);
        self.y = (self.y + self.facing.sin() * distance).clamp(2.0, PENGUIN_WORLD_H - 2.0);

        for flake in &mut self.snow {
            flake.y += flake.speed * dt;
            if flake.y > 1.0 {
                flake.y = 0.0;
                flake.x = rand::thread_rng().gen_range(0.0..1.0);
            }
        }

        for fish in &mut self.fishes {
            fish.bob += dt * 2.1;
            if fish.alive {
                let dist = ((fish.x - self.x).powi(2) + (fish.y - self.y).powi(2)).sqrt();
                if dist < 5.0 {
                    fish.alive = false;
                    self.combo = if self.combo_timer > 0.0 {
                        self.combo + 1
                    } else {
                        1
                    };
                    self.combo_timer = 2.5;
                    self.score += 100 * self.combo;
                    self.flash_timer = 0.6;
                }
            }
        }

        if self.fishes.iter().all(|fish| !fish.alive) {
            self.level += 1;
            self.flash_timer = 1.0;
            self.spawn_fishes();
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') => {
                self.hold_forward = 0.18;
                true
            }
            KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => {
                self.hold_back = 0.18;
                true
            }
            KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('A') => {
                self.hold_left = 0.18;
                true
            }
            KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => {
                self.hold_right = 0.18;
                true
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.restart();
                true
            }
            _ => false,
        }
    }

    fn render(&self, buffer: &mut Buffer, area: Rect) {
        let mut grid = CellGrid::new(area.width, area.height, t().panel_bg, t().text);
        let sky = Color::Rgb(16, 34, 52);
        let ice = Color::Rgb(178, 212, 226);
        let horizon = area.height.saturating_sub(6).max(6) / 2;

        for y in 0..area.height {
            let bg = if y <= horizon { sky } else { ice };
            for x in 0..area.width {
                grid.set(x as i32, y as i32, ' ', t().text, bg);
            }
        }

        grid.text(
            0,
            0,
            &format!(
                "level {}  fish {}  combo x{}",
                self.level,
                self.score / 100,
                self.combo.max(1)
            ),
            t().accent2,
            sky,
        );
        grid.text(
            0,
            1,
            "W/S move  A/D turn  collect fish  Esc menu",
            t().accent4,
            sky,
        );

        for x in 0..area.width {
            grid.set(x as i32, horizon as i32, '-', t().muted, sky);
        }

        for flake in &self.snow {
            let x = (flake.x * area.width.max(1) as f32) as i32;
            let y = (flake.y * horizon.max(1) as f32) as i32;
            grid.set(x, y + 1, '.', Color::Rgb(230, 241, 247), sky);
        }

        for fish in self.fishes.iter().filter(|fish| fish.alive) {
            let dx = fish.x - self.x;
            let dy = fish.y - self.y;
            let forward = dx * self.facing.cos() + dy * self.facing.sin();
            let side = -dx * self.facing.sin() + dy * self.facing.cos();
            if forward <= 1.5 {
                continue;
            }

            let sx = area.width as f32 / 2.0 + side / forward * area.width as f32 * 0.75;
            let sy = horizon as f32 + (14.0 / forward) * area.height as f32 * 0.22;
            let y = sy.round() as i32;
            let x = sx.round() as i32;
            let sprite = if forward < 10.0 { "><>" } else { "><" };
            let color = if self.flash_timer > 0.0 {
                t().accent2
            } else {
                Color::Rgb(255, 148, 79)
            };
            grid.text(x - (sprite.len() as i32 / 2), y, sprite, color, ice);
        }

        let penguin = [" _n_", "(o )", "/_|"];
        let base_y = area.height.saturating_sub(3) as i32;
        for (idx, line) in penguin.iter().enumerate() {
            grid.center_text(base_y + idx as i32, line, t().accent3, ice);
        }

        self.render_penguin_minimap(&mut grid, sky, ice);
        grid.present(buffer, area);
    }
}
