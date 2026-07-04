//! Breakout: single-player brick-buster. Paddle english, 3 lives,
//! per-level speed-up, theme-accent brick bands.

use crossterm::event::{KeyCode, KeyEvent};
use rand::Rng;
use ratatui::prelude::*;

use super::{project_axis, CellGrid, Game};
use crate::theme::t;

const BW: f32 = 60.0;
const BH: f32 = 40.0;
const ROWS: usize = 6;
const COLS: usize = 10;
const BRICK_W: f32 = BW / COLS as f32;
const BRICK_H: f32 = 2.0;
const BRICK_TOP: f32 = 4.0;
const PADDLE_Y: f32 = BH - 2.0;
const PAD_HALF_W: f32 = 4.5;
const PAD_SPEED: f32 = 36.0;
const BALL_SPEED0: f32 = 26.0;
const LEVEL_SPEEDUP: f32 = 1.12;
const MAX_ANGLE: f32 = 1.05; // radians from vertical on paddle bounce
const SPIN: f32 = 0.25;
const HOLD_SECS: f32 = 0.18;
const SERVE_SECS: f32 = 2.5;
const SUBSTEP: f32 = 0.8; // max ball travel per collision substep

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Axis {
    X,
    Y,
}

/// Which axis the ball should reflect on after entering a brick, based on
/// penetration depth relative to the brick center (deeper overlap = the axis
/// we did NOT cross).
pub(super) fn hit_axis(bx: f32, by: f32, cx: f32, cy: f32, half_w: f32, half_h: f32) -> Axis {
    let rx = ((bx - cx) / half_w).abs();
    let ry = ((by - cy) / half_h).abs();
    if rx > ry {
        Axis::X
    } else {
        Axis::Y
    }
}

/// Brick index for a point, if it lies inside the brick field.
pub(super) fn brick_at(x: f32, y: f32) -> Option<usize> {
    if !(0.0..BW).contains(&x) {
        return None;
    }
    if !(BRICK_TOP..BRICK_TOP + ROWS as f32 * BRICK_H).contains(&y) {
        return None;
    }
    let col = ((x / BRICK_W) as usize).min(COLS - 1);
    let row = (((y - BRICK_TOP) / BRICK_H) as usize).min(ROWS - 1);
    Some(row * COLS + col)
}

pub(super) fn brick_points(index: usize) -> u32 {
    let row = index / COLS;
    ((ROWS - row) as u32) * 10
}

fn brick_center(index: usize) -> (f32, f32) {
    let row = index / COLS;
    let col = index % COLS;
    (
        col as f32 * BRICK_W + BRICK_W / 2.0,
        BRICK_TOP + row as f32 * BRICK_H + BRICK_H / 2.0,
    )
}

/// Paddle bounce with english: contact offset bends the exit angle,
/// paddle motion adds sideways spin.
pub(super) fn paddle_english(speed: f32, offset: f32, paddle_vel: f32) -> (f32, f32) {
    let offset = offset.clamp(-1.15, 1.15);
    let angle = offset * MAX_ANGLE;
    let vx = speed * angle.sin() + paddle_vel * SPIN;
    let vy = -speed * angle.cos().max(0.35);
    (vx, vy)
}

pub(super) struct BreakoutGame {
    paddle_x: f32,
    pv: f32,
    hold_left: f32,
    hold_right: f32,
    bx: f32,
    by: f32,
    vx: f32,
    vy: f32,
    stuck: bool, // ball riding the paddle before serve
    serve_cd: f32,
    speed: f32,
    bricks: Vec<bool>,
    lives: u8,
    score: u32,
    level: u32,
    game_over: bool,
}

impl BreakoutGame {
    pub(super) fn new() -> Self {
        let mut game = Self {
            paddle_x: BW / 2.0,
            pv: 0.0,
            hold_left: 0.0,
            hold_right: 0.0,
            bx: BW / 2.0,
            by: PADDLE_Y - 1.0,
            vx: 0.0,
            vy: 0.0,
            stuck: true,
            serve_cd: SERVE_SECS,
            speed: BALL_SPEED0,
            bricks: vec![true; ROWS * COLS],
            lives: 3,
            score: 0,
            level: 1,
            game_over: false,
        };
        game.hold_ball();
        game
    }

    fn restart(&mut self) {
        *self = Self::new();
    }

    fn hold_ball(&mut self) {
        self.stuck = true;
        self.serve_cd = SERVE_SECS;
        self.bx = self.paddle_x;
        self.by = PADDLE_Y - 1.0;
        self.vx = 0.0;
        self.vy = 0.0;
    }

    fn launch(&mut self) {
        if !self.stuck || self.game_over {
            return;
        }
        self.stuck = false;
        let angle = rand::thread_rng().gen_range(-0.5..0.5f32);
        self.vx = self.speed * angle.sin() + self.pv * SPIN;
        self.vy = -self.speed * angle.cos();
    }

    fn bricks_left(&self) -> usize {
        self.bricks.iter().filter(|alive| **alive).count()
    }

    fn next_level(&mut self) {
        self.level += 1;
        self.speed = (self.speed * LEVEL_SPEEDUP).min(60.0);
        self.bricks = vec![true; ROWS * COLS];
        self.hold_ball();
    }

    fn lose_life(&mut self) {
        if self.lives > 1 {
            self.lives -= 1;
            self.hold_ball();
        } else {
            self.lives = 0;
            self.game_over = true;
        }
    }

    /// Move the ball by (dx, dy) resolving brick/wall/paddle collisions.
    /// Exposed within the module for tests.
    pub(super) fn move_ball(&mut self, dx: f32, dy: f32) {
        self.bx += dx;
        self.by += dy;

        // side + top walls
        if self.bx < 0.5 {
            self.bx = 1.0 - self.bx;
            self.vx = self.vx.abs();
        } else if self.bx > BW - 0.5 {
            self.bx = 2.0 * (BW - 0.5) - self.bx;
            self.vx = -self.vx.abs();
        }
        if self.by < 0.5 {
            self.by = 1.0 - self.by;
            self.vy = self.vy.abs();
        }

        // bricks
        if let Some(index) = brick_at(self.bx, self.by) {
            if self.bricks[index] {
                self.bricks[index] = false;
                self.score += brick_points(index);
                let (cx, cy) = brick_center(index);
                match hit_axis(self.bx, self.by, cx, cy, BRICK_W / 2.0, BRICK_H / 2.0) {
                    Axis::X => self.vx = -self.vx,
                    Axis::Y => self.vy = -self.vy,
                }
            }
        }

        // paddle
        if self.vy > 0.0 && self.by >= PADDLE_Y - 0.6 && self.by <= PADDLE_Y + 0.8 {
            let offset = (self.bx - self.paddle_x) / PAD_HALF_W;
            if offset.abs() <= 1.15 {
                let (vx, vy) = paddle_english(self.speed, offset, self.pv);
                self.vx = vx;
                self.vy = vy;
                self.by = PADDLE_Y - 0.7;
            }
        }

        // bottom: life lost
        if self.by > BH + 0.5 {
            self.lose_life();
        }
    }
}

impl Game for BreakoutGame {
    fn tick(&mut self, dt: f32) {
        if self.game_over {
            return;
        }

        self.hold_left = (self.hold_left - dt).max(0.0);
        self.hold_right = (self.hold_right - dt).max(0.0);
        let mut dir = 0.0;
        if self.hold_left > 0.0 {
            dir -= 1.0;
        }
        if self.hold_right > 0.0 {
            dir += 1.0;
        }
        self.pv = dir * PAD_SPEED;
        self.paddle_x =
            (self.paddle_x + self.pv * dt).clamp(PAD_HALF_W + 0.5, BW - PAD_HALF_W - 0.5);

        if self.stuck {
            self.bx = self.paddle_x;
            self.by = PADDLE_Y - 1.0;
            self.serve_cd -= dt;
            if self.serve_cd <= 0.0 {
                self.launch();
            }
            return;
        }

        // substep the ball so fast frames cannot tunnel through bricks
        let dist = ((self.vx * dt).powi(2) + (self.vy * dt).powi(2)).sqrt();
        let steps = (dist / SUBSTEP).ceil().max(1.0) as u32;
        let sub = dt / steps as f32;
        for _ in 0..steps {
            self.move_ball(self.vx * sub, self.vy * sub);
            if self.stuck || self.game_over {
                break;
            }
        }

        if self.bricks_left() == 0 {
            self.score += 100;
            self.next_level();
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('A') => {
                self.hold_left = HOLD_SECS;
                true
            }
            KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => {
                self.hold_right = HOLD_SECS;
                true
            }
            KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') | KeyCode::Char(' ') => {
                self.launch();
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
        let theme = t().clone();
        let mut grid = CellGrid::new(area.width, area.height, theme.panel_bg, theme.text);

        if area.width < 24 || area.height < 10 {
            grid.center_text(1, "BREAKOUT", theme.accent2, theme.panel_bg);
            grid.center_text(3, "Grow this tile to play.", theme.muted, theme.panel_bg);
            grid.present(buffer, area);
            return;
        }

        grid.text(
            0,
            0,
            &format!(
                "score {:05}  level {}  lives {}",
                self.score, self.level, self.lives
            ),
            theme.accent2,
            theme.panel_bg,
        );
        grid.text(
            0,
            1,
            "A/D move  W launch  R restart  Esc menu",
            theme.muted,
            theme.panel_bg,
        );

        let field_top = 2i32;
        let field_h = area.height.saturating_sub(field_top as u16);
        let px = |x: f32| project_axis(x, BW, area.width);
        let py = |y: f32| project_axis(y, BH, field_h) + field_top;

        // bricks in theme accent bands per row
        let band = [theme.accent2, theme.accent3, theme.accent1, theme.accent4];
        for (index, alive) in self.bricks.iter().enumerate() {
            if !alive {
                continue;
            }
            let row = index / COLS;
            let col = index % COLS;
            let color = band[row % band.len()];
            let (_, cy) = brick_center(index);
            let y = py(cy);
            let x0 = px(col as f32 * BRICK_W) + 1;
            let x1 = px((col + 1) as f32 * BRICK_W) - 1;
            for x in x0..=x1 {
                grid.set(x, y, '=', color, theme.panel_bg);
            }
        }

        // paddle
        let pad_y = py(PADDLE_Y);
        let pad_x0 = px(self.paddle_x - PAD_HALF_W);
        let pad_x1 = px(self.paddle_x + PAD_HALF_W);
        for x in pad_x0..=pad_x1 {
            grid.set(x, pad_y, '#', theme.accent4, theme.panel_bg);
        }

        // ball
        if !self.game_over {
            grid.set(px(self.bx), py(self.by), 'O', theme.accent2, theme.panel_bg);
        }

        let mid = area.height as i32 / 2;
        if self.game_over {
            grid.center_text(mid - 1, "OUT OF BALLS", theme.danger, theme.panel_bg);
            grid.center_text(
                mid + 1,
                &format!("final score {:05} — R restart  Esc menu", self.score),
                theme.text,
                theme.panel_bg,
            );
        } else if self.stuck {
            grid.center_text(
                mid,
                &format!("W to launch — auto in {}", self.serve_cd.ceil().max(1.0) as u32),
                theme.accent2,
                theme.panel_bg,
            );
        }

        grid.present(buffer, area);
    }

    fn status(&self) -> Option<String> {
        Some(format!(
            "breakout: score {} level {} lives {}",
            self.score, self.level, self.lives
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_axis_picks_the_crossed_face() {
        // ball near the left face of a brick: reflect on X
        assert_eq!(hit_axis(0.1, 1.0, 1.0, 1.0, 1.0, 1.0), Axis::X);
        // ball near the bottom face: reflect on Y
        assert_eq!(hit_axis(1.0, 1.9, 1.0, 1.0, 1.0, 1.0), Axis::Y);
    }

    #[test]
    fn brick_lookup_and_points() {
        // dead center of the first brick
        let index = brick_at(BRICK_W / 2.0, BRICK_TOP + BRICK_H / 2.0).unwrap();
        assert_eq!(index, 0);
        assert_eq!(brick_points(index), ROWS as u32 * 10, "top row is worth the most");
        // last brick
        let index = brick_at(
            BW - BRICK_W / 2.0,
            BRICK_TOP + (ROWS as f32 - 0.5) * BRICK_H,
        )
        .unwrap();
        assert_eq!(index, ROWS * COLS - 1);
        assert_eq!(brick_points(index), 10, "bottom row is worth the least");
        // outside the field
        assert!(brick_at(5.0, BRICK_TOP - 1.0).is_none());
        assert!(brick_at(5.0, BRICK_TOP + ROWS as f32 * BRICK_H + 1.0).is_none());
        assert!(brick_at(-1.0, BRICK_TOP + 1.0).is_none());
    }

    #[test]
    fn brick_hit_scores_and_reflects() {
        let mut game = BreakoutGame::new();
        game.stuck = false;
        // approach the bottom row of bricks from below, moving up
        let target = (ROWS - 1) * COLS + 4; // row 5, col 4
        let (cx, cy) = brick_center(target);
        game.bx = cx;
        game.by = cy + BRICK_H; // just below the brick
        game.vx = 0.0;
        game.vy = -20.0;
        game.move_ball(0.0, -BRICK_H); // enter the brick from the bottom edge
        assert!(!game.bricks[target], "brick destroyed");
        assert_eq!(game.score, brick_points(target));
        assert!(game.vy > 0.0, "ball reflected downward off the brick");
    }

    #[test]
    fn paddle_english_bends_the_ball() {
        let (vx_left, vy_left) = paddle_english(BALL_SPEED0, -1.0, 0.0);
        let (vx_right, _) = paddle_english(BALL_SPEED0, 1.0, 0.0);
        let (vx_center, vy_center) = paddle_english(BALL_SPEED0, 0.0, 0.0);
        assert!(vx_left < 0.0, "left-edge contact sends the ball left");
        assert!(vx_right > 0.0, "right-edge contact sends the ball right");
        assert!(vx_center.abs() < 1e-4);
        assert!(vy_left < 0.0 && vy_center < 0.0, "ball always exits upward");
        // paddle motion adds spin
        let (vx_spun, _) = paddle_english(BALL_SPEED0, 0.0, PAD_SPEED);
        assert!(vx_spun > 0.0);
    }

    #[test]
    fn ball_below_paddle_costs_a_life_then_game_over() {
        let mut game = BreakoutGame::new();
        game.stuck = false;
        game.by = BH;
        game.vy = 10.0;
        game.move_ball(0.0, 1.0);
        assert_eq!(game.lives, 2);
        assert!(game.stuck, "ball returns to the paddle after a lost life");

        game.lives = 1;
        game.stuck = false;
        game.by = BH;
        game.vy = 10.0;
        game.move_ball(0.0, 1.0);
        assert!(game.game_over);
        assert_eq!(game.lives, 0);
    }

    #[test]
    fn clearing_bricks_advances_level_and_speeds_up() {
        let mut game = BreakoutGame::new();
        let speed = game.speed;
        game.bricks = vec![false; ROWS * COLS];
        game.stuck = false;
        game.vy = -10.0;
        game.tick(0.016);
        assert_eq!(game.level, 2);
        assert!(game.speed > speed);
        assert!(game.stuck, "new level starts with a held ball");
        assert!(game.bricks.iter().all(|alive| *alive), "bricks rebuilt");
    }
}
