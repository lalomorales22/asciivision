//! Snake: single-player. Fixed-step accumulator, food growth, speed ramp,
//! walls kill, session hi-score survives R restarts.

use crossterm::event::{KeyCode, KeyEvent};
use rand::Rng;
use ratatui::prelude::*;
use std::collections::VecDeque;

use super::{CellGrid, Dir, Game};
use crate::theme::t;

const SNAKE_W: i32 = 36;
const SNAKE_H: i32 = 22;
const START_LEN: usize = 4;
const START_STEP_SECS: f32 = 0.135;
const MIN_STEP_SECS: f32 = 0.055;
const SPEED_RAMP: f32 = 0.955;
const GROW_PER_FOOD: u32 = 2;

pub(super) struct SnakeGame {
    body: VecDeque<(i32, i32)>, // front = head
    dir: Dir,
    pending: VecDeque<Dir>,
    food: (i32, i32),
    grow: u32,
    score: u32,
    hi_score: u32,
    step_secs: f32,
    acc: f32,
    game_over: bool,
}

impl SnakeGame {
    pub(super) fn new() -> Self {
        let cy = SNAKE_H / 2;
        let cx = SNAKE_W / 2;
        let mut body = VecDeque::new();
        for i in 0..START_LEN as i32 {
            body.push_back((cx - i, cy));
        }
        let mut game = Self {
            body,
            dir: Dir::Right,
            pending: VecDeque::new(),
            food: (0, 0),
            grow: 0,
            score: 0,
            hi_score: 0,
            step_secs: START_STEP_SECS,
            acc: 0.0,
            game_over: false,
        };
        game.spawn_food();
        game
    }

    fn restart(&mut self) {
        let hi = self.hi_score;
        *self = Self::new();
        self.hi_score = hi;
    }

    fn spawn_food(&mut self) {
        let mut rng = rand::thread_rng();
        for _ in 0..256 {
            let cell = (rng.gen_range(0..SNAKE_W), rng.gen_range(0..SNAKE_H));
            if !self.body.contains(&cell) {
                self.food = cell;
                return;
            }
        }
        // dense board fallback: first free cell
        for y in 0..SNAKE_H {
            for x in 0..SNAKE_W {
                if !self.body.contains(&(x, y)) {
                    self.food = (x, y);
                    return;
                }
            }
        }
    }

    fn queue_dir(&mut self, dir: Dir) {
        let last = self.pending.back().copied().unwrap_or(self.dir);
        if dir == last || dir == last.opposite() {
            return; // no-op or illegal reversal relative to what will apply
        }
        if self.pending.len() < 3 {
            self.pending.push_back(dir);
        }
    }

    /// Advance the snake one grid cell. Public within the module for tests.
    pub(super) fn step(&mut self) {
        if self.game_over {
            return;
        }
        if let Some(dir) = self.pending.pop_front() {
            if dir != self.dir.opposite() {
                self.dir = dir;
            }
        }
        let head = *self.body.front().expect("snake has a head");
        let (dx, dy) = self.dir.delta();
        let next = (head.0 + dx, head.1 + dy);

        // walls kill
        if next.0 < 0 || next.1 < 0 || next.0 >= SNAKE_W || next.1 >= SNAKE_H {
            self.die();
            return;
        }
        // self collision — moving into the cell the tail is vacating is legal
        let tail = *self.body.back().expect("snake has a tail");
        let tail_moves = self.grow == 0;
        if self.body.contains(&next) && !(tail_moves && next == tail) {
            self.die();
            return;
        }

        self.body.push_front(next);
        if next == self.food {
            self.score += 10;
            self.grow += GROW_PER_FOOD;
            self.step_secs = (self.step_secs * SPEED_RAMP).max(MIN_STEP_SECS);
            self.spawn_food();
        }
        if self.grow > 0 {
            self.grow -= 1;
        } else {
            self.body.pop_back();
        }
    }

    fn die(&mut self) {
        self.game_over = true;
        self.hi_score = self.hi_score.max(self.score);
    }
}

impl Game for SnakeGame {
    fn tick(&mut self, dt: f32) {
        if self.game_over {
            return;
        }
        self.acc += dt;
        while self.acc >= self.step_secs {
            self.acc -= self.step_secs;
            self.step();
            if self.game_over {
                break;
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        let dir = match key.code {
            KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') => Some(Dir::Up),
            KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => Some(Dir::Down),
            KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('A') => Some(Dir::Left),
            KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => Some(Dir::Right),
            _ => None,
        };
        if let Some(dir) = dir {
            self.queue_dir(dir);
            return true;
        }
        if matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R')) {
            self.restart();
            return true;
        }
        false
    }

    fn render(&self, buffer: &mut Buffer, area: Rect) {
        let theme = t().clone();
        let mut grid = CellGrid::new(area.width, area.height, theme.panel_bg, theme.text);

        if area.width < 24 || area.height < 10 {
            grid.center_text(1, "SNAKE", theme.accent2, theme.panel_bg);
            grid.center_text(3, "Grow this tile to play.", theme.muted, theme.panel_bg);
            grid.present(buffer, area);
            return;
        }

        grid.text(
            0,
            0,
            &format!(
                "score {:04}  best {:04}  len {}",
                self.score,
                self.hi_score.max(self.score),
                self.body.len()
            ),
            theme.accent2,
            theme.panel_bg,
        );
        grid.text(
            0,
            1,
            "WASD or arrows steer  walls kill  R restart",
            theme.muted,
            theme.panel_bg,
        );

        // arena frame, playfield inset by 1
        let field_top = 2i32;
        let frame_w = area.width as i32;
        let bottom = area.height as i32 - 1;
        for x in 0..frame_w {
            grid.set(x, field_top, '-', theme.accent1, theme.panel_bg);
            grid.set(x, bottom, '-', theme.accent1, theme.panel_bg);
        }
        for y in field_top..=bottom {
            grid.set(0, y, '|', theme.accent1, theme.panel_bg);
            grid.set(frame_w - 1, y, '|', theme.accent1, theme.panel_bg);
        }
        for &(x, y) in &[(0, field_top), (frame_w - 1, field_top), (0, bottom), (frame_w - 1, bottom)] {
            grid.set(x, y, '+', theme.accent1, theme.panel_bg);
        }

        let inner_w = (frame_w - 2).max(1) as i64;
        let inner_h = (bottom - field_top - 1).max(1) as i64;
        let px = |x: i32| 1 + ((x as i64 * (inner_w - 1).max(0)) / (SNAKE_W as i64 - 1)) as i32;
        let py = |y: i32| {
            field_top + 1 + ((y as i64 * (inner_h - 1).max(0)) / (SNAKE_H as i64 - 1)) as i32
        };

        grid.set(px(self.food.0), py(self.food.1), '*', theme.accent2, theme.panel_bg);
        for (idx, cell) in self.body.iter().enumerate() {
            let (ch, color) = if idx == 0 {
                ('@', theme.accent4)
            } else {
                ('o', theme.accent3)
            };
            grid.set(px(cell.0), py(cell.1), ch, color, theme.panel_bg);
        }

        if self.game_over {
            let mid = area.height as i32 / 2;
            grid.center_text(mid - 1, "SNAKE DOWN", theme.danger, theme.panel_bg);
            let line = if self.score >= self.hi_score && self.score > 0 {
                format!("NEW BEST {:04} — R restart  Esc menu", self.score)
            } else {
                "R restart  Esc menu".to_string()
            };
            grid.center_text(mid + 1, &line, theme.text, theme.panel_bg);
        }

        grid.present(buffer, area);
    }

    fn status(&self) -> Option<String> {
        Some(format!(
            "snake: score {} (best {})",
            self.score,
            self.hi_score.max(self.score)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_grows_when_eating() {
        let mut game = SnakeGame::new();
        let head = *game.body.front().unwrap();
        game.food = (head.0 + 1, head.1); // directly ahead
        let len = game.body.len();
        let speed = game.step_secs;
        game.step(); // eats
        assert_eq!(game.score, 10);
        assert!(game.step_secs < speed, "speed ramps up per food");
        // growth is applied over the next GROW_PER_FOOD steps
        game.food = (0, 0); // move food out of the path
        game.step();
        game.step();
        assert_eq!(game.body.len(), len + GROW_PER_FOOD as usize);
        // afterwards length is stable
        game.step();
        assert_eq!(game.body.len(), len + GROW_PER_FOOD as usize);
    }

    #[test]
    fn wall_kills_and_records_hi_score() {
        let mut game = SnakeGame::new();
        game.score = 70;
        // drive the head to the right wall
        for _ in 0..SNAKE_W {
            game.food = (0, 0);
            game.step();
            if game.game_over {
                break;
            }
        }
        assert!(game.game_over, "snake must die on the wall");
        assert_eq!(game.hi_score, 70);
        // restart preserves the session hi-score
        game.restart();
        assert!(!game.game_over);
        assert_eq!(game.score, 0);
        assert_eq!(game.hi_score, 70);
    }

    #[test]
    fn self_collision_kills() {
        let mut game = SnakeGame::new();
        game.grow = 10; // keep the tail in place so a tight turn self-collides
        game.food = (0, 0);
        // head loops back into its own body: up, left, down
        game.queue_dir(Dir::Up);
        game.step();
        game.queue_dir(Dir::Left);
        game.step();
        game.queue_dir(Dir::Down);
        game.step();
        assert!(game.game_over, "snake ran into its own body");
    }

    #[test]
    fn following_own_tail_is_legal() {
        let mut game = SnakeGame::new();
        game.food = (0, 0);
        // a 2x2 loop with a 4-long snake: the head always enters the cell the
        // tail just vacated — this must NOT die
        for dir in [Dir::Up, Dir::Left, Dir::Down, Dir::Right, Dir::Up, Dir::Left] {
            game.queue_dir(dir);
            game.step();
            assert!(!game.game_over, "tail-following must be survivable");
        }
    }

    #[test]
    fn reversal_is_ignored() {
        let mut game = SnakeGame::new();
        game.food = (0, 0);
        game.queue_dir(Dir::Left); // reversal of Right
        game.step();
        assert_eq!(game.dir, Dir::Right, "180 reversal must be ignored");
        assert!(!game.game_over);
    }
}
