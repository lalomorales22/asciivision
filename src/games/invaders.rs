//! Space Invaders: ported unchanged onto the `Game` trait.

use crossterm::event::{KeyCode, KeyEvent};
use rand::{prelude::SliceRandom, Rng};
use ratatui::prelude::*;

use super::{CellGrid, Game, project_axis};
use crate::theme::t;

const SPACE_W: f32 = 40.0;
const SPACE_H: f32 = 24.0;

#[derive(Clone)]
struct SpaceBullet {
    x: f32,
    y: f32,
    dy: f32,
    friendly: bool,
}

#[derive(Clone)]
struct Invader {
    x: f32,
    y: f32,
    alive: bool,
}

#[derive(Clone)]
struct Star {
    x: f32,
    y: f32,
    speed: f32,
}

pub(super) struct SpaceInvadersGame {
    player_x: f32,
    bullets: Vec<SpaceBullet>,
    invaders: Vec<Invader>,
    stars: Vec<Star>,
    enemy_dir: f32,
    enemy_timer: f32,
    enemy_fire_timer: f32,
    shot_cooldown: f32,
    shield_cooldown: f32,
    shield_timer: f32,
    score: u32,
    lives: u8,
    wave: u32,
    game_over: bool,
}

impl SpaceInvadersGame {
    pub(super) fn new() -> Self {
        let mut game = Self {
            player_x: SPACE_W / 2.0,
            bullets: Vec::new(),
            invaders: Vec::new(),
            stars: Vec::new(),
            enemy_dir: 1.0,
            enemy_timer: 0.0,
            enemy_fire_timer: 0.0,
            shot_cooldown: 0.0,
            shield_cooldown: 0.0,
            shield_timer: 0.0,
            score: 0,
            lives: 3,
            wave: 1,
            game_over: false,
        };
        game.stars = (0..90)
            .map(|_| {
                let mut rng = rand::thread_rng();
                Star {
                    x: rng.gen_range(0.0..SPACE_W),
                    y: rng.gen_range(0.0..SPACE_H),
                    speed: rng.gen_range(2.0..8.0),
                }
            })
            .collect();
        game.spawn_wave();
        game
    }

    fn spawn_wave(&mut self) {
        self.invaders.clear();
        for row in 0..5 {
            for col in 0..8 {
                self.invaders.push(Invader {
                    x: 6.0 + col as f32 * 3.6,
                    y: 2.0 + row as f32 * 2.1,
                    alive: true,
                });
            }
        }
        self.enemy_dir = 1.0;
        self.enemy_timer = 0.0;
        self.enemy_fire_timer = 0.7;
        self.bullets.clear();
        self.player_x = SPACE_W / 2.0;
    }

    fn restart(&mut self) {
        *self = Self::new();
    }
}

impl Game for SpaceInvadersGame {
    fn tick(&mut self, dt: f32) {
        for star in &mut self.stars {
            star.y += star.speed * dt;
            if star.y >= SPACE_H {
                star.y = 0.0;
                star.x = rand::thread_rng().gen_range(0.0..SPACE_W);
            }
        }

        self.shot_cooldown = (self.shot_cooldown - dt).max(0.0);
        self.shield_cooldown = (self.shield_cooldown - dt).max(0.0);
        self.shield_timer = (self.shield_timer - dt).max(0.0);

        if self.game_over {
            return;
        }

        self.enemy_timer += dt;
        let step_time = (0.42 - self.wave as f32 * 0.025).clamp(0.12, 0.42);
        if self.enemy_timer >= step_time {
            self.enemy_timer = 0.0;
            let mut hit_edge = false;
            for invader in self.invaders.iter().filter(|inv| inv.alive) {
                if invader.x + self.enemy_dir >= SPACE_W - 1.5 || invader.x + self.enemy_dir <= 1.5 {
                    hit_edge = true;
                    break;
                }
            }
            if hit_edge {
                self.enemy_dir *= -1.0;
                for invader in self.invaders.iter_mut().filter(|inv| inv.alive) {
                    invader.y += 1.1;
                }
            } else {
                for invader in self.invaders.iter_mut().filter(|inv| inv.alive) {
                    invader.x += self.enemy_dir;
                }
            }
        }

        self.enemy_fire_timer -= dt;
        if self.enemy_fire_timer <= 0.0 {
            let alive: Vec<_> = self
                .invaders
                .iter()
                .filter(|inv| inv.alive)
                .cloned()
                .collect();
            if let Some(shooter) = alive.choose(&mut rand::thread_rng()) {
                self.bullets.push(SpaceBullet {
                    x: shooter.x,
                    y: shooter.y + 1.0,
                    dy: 17.0 + self.wave as f32,
                    friendly: false,
                });
            }
            self.enemy_fire_timer = (1.25 - self.wave as f32 * 0.05).clamp(0.45, 1.25);
        }

        for bullet in &mut self.bullets {
            bullet.y += bullet.dy * dt;
        }
        self.bullets
            .retain(|b| b.y >= 0.0 && b.y <= SPACE_H + 1.0);

        for bullet in &mut self.bullets {
            if bullet.friendly {
                for invader in self.invaders.iter_mut().filter(|inv| inv.alive) {
                    if (invader.x - bullet.x).abs() < 1.2 && (invader.y - bullet.y).abs() < 0.9 {
                        invader.alive = false;
                        bullet.y = -100.0;
                        self.score += 50;
                        break;
                    }
                }
            } else if (bullet.x - self.player_x).abs() < 1.4
                && (bullet.y - (SPACE_H - 1.5)).abs() < 1.0
            {
                if self.shield_timer > 0.0 {
                    bullet.y = SPACE_H + 10.0;
                } else if self.lives > 1 {
                    self.lives -= 1;
                    bullet.y = SPACE_H + 10.0;
                    self.player_x = SPACE_W / 2.0;
                } else {
                    self.lives = 0;
                    self.game_over = true;
                }
            }
        }
        self.bullets
            .retain(|b| b.y >= 0.0 && b.y <= SPACE_H + 1.0);

        if self.shield_timer > 0.0 {
            self.bullets
                .retain(|b| b.friendly || (b.x - self.player_x).abs() > 3.0);
        }

        if self.invaders.iter().filter(|inv| inv.alive).count() == 0 {
            self.wave += 1;
            self.score += 200;
            self.spawn_wave();
        }

        if self
            .invaders
            .iter()
            .filter(|inv| inv.alive)
            .any(|inv| inv.y >= SPACE_H - 4.0)
        {
            self.game_over = true;
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if self.game_over {
            if matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R')) {
                self.restart();
                return true;
            }
        }

        match key.code {
            KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('A') => {
                self.player_x = (self.player_x - 1.8).clamp(2.0, SPACE_W - 2.0);
                true
            }
            KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => {
                self.player_x = (self.player_x + 1.8).clamp(2.0, SPACE_W - 2.0);
                true
            }
            KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') | KeyCode::Char(' ') => {
                if self.shot_cooldown <= 0.0 && !self.game_over {
                    self.bullets.push(SpaceBullet {
                        x: self.player_x,
                        y: SPACE_H - 3.0,
                        dy: -30.0,
                        friendly: true,
                    });
                    self.shot_cooldown = 0.24;
                }
                true
            }
            KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => {
                if self.shield_cooldown <= 0.0 && !self.game_over {
                    self.shield_timer = 0.9;
                    self.shield_cooldown = 5.5;
                }
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
        // Snapshot the theme once: no RwLock churn inside the render loops.
        let theme = t().clone();
        let mut grid = CellGrid::new(area.width, area.height, theme.panel_bg, theme.text);
        grid.text(
            0,
            0,
            &format!(
                "wave {}  score {:05}  lives {}",
                self.wave, self.score, self.lives
            ),
            theme.accent2,
            theme.panel_bg,
        );
        let shield = if self.shield_cooldown <= 0.0 {
            "S shield ready"
        } else {
            "S shield cooling"
        };
        grid.text(
            0,
            1,
            &format!("A/D move  W fire  {}  Esc menu", shield),
            theme.accent4,
            theme.panel_bg,
        );

        let game_top = 2u16;
        let game_h = area.height.saturating_sub(game_top);
        for star in &self.stars {
            let sx = project_axis(star.x, SPACE_W, area.width);
            let sy = project_axis(star.y, SPACE_H, game_h) + game_top as i32;
            grid.set(sx, sy, '.', theme.muted, theme.panel_bg);
        }

        for invader in self.invaders.iter().filter(|inv| inv.alive) {
            let x = project_axis(invader.x, SPACE_W, area.width);
            let y = project_axis(invader.y, SPACE_H, game_h) + game_top as i32;
            let ch = if ((invader.x + invader.y) as i32) % 2 == 0 {
                'W'
            } else {
                'M'
            };
            grid.set(x, y, ch, theme.accent1, theme.panel_bg);
        }

        for bullet in &self.bullets {
            let x = project_axis(bullet.x, SPACE_W, area.width);
            let y = project_axis(bullet.y, SPACE_H, game_h) + game_top as i32;
            let color = if bullet.friendly { theme.accent4 } else { theme.danger };
            grid.set(x, y, '|', color, theme.panel_bg);
        }

        let player_x = project_axis(self.player_x, SPACE_W, area.width);
        let player_y = game_top as i32 + game_h.saturating_sub(1) as i32;
        grid.set(player_x, player_y, 'A', theme.accent3, theme.panel_bg);
        if self.shield_timer > 0.0 {
            grid.set(player_x - 1, player_y, '(', theme.accent4, theme.panel_bg);
            grid.set(player_x + 1, player_y, ')', theme.accent4, theme.panel_bg);
        }

        if self.game_over {
            let mid = area.height as i32 / 2;
            grid.center_text(mid - 1, "DEFENSE LINE COLLAPSED", theme.danger, theme.panel_bg);
            grid.center_text(mid, "Press R to restart or Esc for menu", theme.text, theme.panel_bg);
        }

        grid.present(buffer, area);
    }
}
