//! Blockdrop: a Tetris with a 7-bag randomizer, hold, ghost piece, hard
//! drop, wall kicks, line-clear flash with particles, and level speed-up.

use super::{rgb, CellGrid, Game, GameKind, Particles, Popups};
use crate::theme::t;
use crossterm::event::{KeyCode, KeyEvent};
use rand::seq::SliceRandom;
use ratatui::prelude::Color;

const BW: i32 = 10;
const BH: i32 = 20;
const HIDDEN: i32 = 2;

/// Piece cells for rotation 0, centred on the spin origin.
const SHAPES: [[(i32, i32); 4]; 7] = [
    [(-1, 0), (0, 0), (1, 0), (2, 0)],  // I
    [(0, 0), (1, 0), (0, 1), (1, 1)],   // O
    [(-1, 0), (0, 0), (1, 0), (0, -1)], // T
    [(-1, 0), (0, 0), (0, -1), (1, -1)], // S
    [(-1, -1), (0, -1), (0, 0), (1, 0)], // Z
    [(-1, -1), (-1, 0), (0, 0), (1, 0)], // J
    [(-1, 0), (0, 0), (1, 0), (1, -1)],  // L
];

fn piece_color(kind: u8) -> (u8, u8, u8) {
    match kind {
        0 => (80, 220, 240),
        1 => (250, 220, 70),
        2 => (190, 110, 240),
        3 => (100, 230, 110),
        4 => (250, 90, 90),
        5 => (90, 130, 250),
        _ => (255, 160, 60),
    }
}

fn cells(kind: u8, rot: u8) -> [(i32, i32); 4] {
    let mut out = SHAPES[kind as usize];
    if kind == 1 {
        return out; // O never rotates
    }
    for _ in 0..(rot % 4) {
        for c in out.iter_mut() {
            *c = (-c.1, c.0);
        }
    }
    out
}

#[derive(Clone, Copy)]
struct Piece {
    kind: u8,
    rot: u8,
    x: i32,
    y: i32,
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Playing,
    Clearing(f32),
    GameOver,
}

pub struct TetrisGame {
    board: Vec<Option<u8>>, // (BH + HIDDEN) rows
    current: Piece,
    bag: Vec<u8>,
    queue: Vec<u8>,
    hold: Option<u8>,
    hold_used: bool,
    gravity_timer: f32,
    lock_timer: f32,
    score: u32,
    lines: u32,
    level: u32,
    clearing_rows: Vec<i32>,
    phase: Phase,
    particles: Particles,
    popups: Popups,
    last_clear_text: String,
}

impl TetrisGame {
    pub fn new() -> Self {
        let mut game = Self {
            board: vec![None; (BW * (BH + HIDDEN)) as usize],
            current: Piece { kind: 0, rot: 0, x: 0, y: 0 },
            bag: Vec::new(),
            queue: Vec::new(),
            hold: None,
            hold_used: false,
            gravity_timer: 0.0,
            lock_timer: 0.0,
            score: 0,
            lines: 0,
            level: 1,
            clearing_rows: Vec::new(),
            phase: Phase::Playing,
            particles: Particles::default(),
            popups: Popups::default(),
            last_clear_text: String::new(),
        };
        while game.queue.len() < 4 {
            let next = game.draw_from_bag();
            game.queue.push(next);
        }
        game.spawn();
        game
    }

    fn draw_from_bag(&mut self) -> u8 {
        if self.bag.is_empty() {
            self.bag = (0..7).collect();
            self.bag.shuffle(&mut rand::thread_rng());
        }
        self.bag.pop().unwrap_or(0)
    }

    fn spawn(&mut self) {
        let kind = self.queue.remove(0);
        let next = self.draw_from_bag();
        self.queue.push(next);
        self.current = Piece { kind, rot: 0, x: BW / 2 - 1, y: 1 };
        self.hold_used = false;
        self.gravity_timer = 0.0;
        self.lock_timer = 0.0;
        if self.collides(self.current) {
            self.phase = Phase::GameOver;
        }
    }

    fn at(&self, x: i32, y: i32) -> Option<u8> {
        if !(0..BW).contains(&x) || !(0..BH + HIDDEN).contains(&y) {
            return Some(255);
        }
        self.board[(y * BW + x) as usize]
    }

    fn collides(&self, p: Piece) -> bool {
        cells(p.kind, p.rot)
            .iter()
            .any(|(dx, dy)| self.at(p.x + dx, p.y + dy).is_some())
    }

    fn gravity_interval(&self) -> f32 {
        (0.8 * 0.82f32.powi(self.level as i32 - 1)).max(0.06)
    }

    fn try_shift(&mut self, dx: i32) -> bool {
        let mut p = self.current;
        p.x += dx;
        if !self.collides(p) {
            self.current = p;
            true
        } else {
            false
        }
    }

    fn try_rotate(&mut self, dir: i32) {
        let mut p = self.current;
        p.rot = ((p.rot as i32 + dir).rem_euclid(4)) as u8;
        for (kx, ky) in [(0, 0), (-1, 0), (1, 0), (-2, 0), (2, 0), (0, -1), (-1, -1), (1, -1)] {
            let mut q = p;
            q.x += kx;
            q.y += ky;
            if !self.collides(q) {
                self.current = q;
                return;
            }
        }
    }

    fn drop_one(&mut self) -> bool {
        let mut p = self.current;
        p.y += 1;
        if self.collides(p) {
            false
        } else {
            self.current = p;
            true
        }
    }

    fn ghost_y(&self) -> i32 {
        let mut p = self.current;
        while !self.collides(Piece { y: p.y + 1, ..p }) {
            p.y += 1;
        }
        p.y
    }

    fn lock(&mut self) {
        let p = self.current;
        for (dx, dy) in cells(p.kind, p.rot) {
            let (x, y) = (p.x + dx, p.y + dy);
            if (0..BW).contains(&x) && (0..BH + HIDDEN).contains(&y) {
                self.board[(y * BW + x) as usize] = Some(p.kind);
            }
        }
        let full: Vec<i32> = (0..BH + HIDDEN)
            .filter(|&y| (0..BW).all(|x| self.board[(y * BW + x) as usize].is_some()))
            .collect();
        if full.is_empty() {
            self.spawn();
            return;
        }
        let n = full.len() as u32;
        let base = match n {
            1 => 100,
            2 => 300,
            3 => 500,
            _ => 800,
        };
        self.score += base * self.level;
        self.last_clear_text = match n {
            4 => "TETRIS!".to_string(),
            3 => "TRIPLE".to_string(),
            2 => "DOUBLE".to_string(),
            _ => "SINGLE".to_string(),
        };
        for &y in &full {
            for x in 0..BW {
                let kind = self.board[(y * BW + x) as usize].unwrap_or(0);
                self.particles.burst(x as f32 + 0.5, y as f32, 2, piece_color(kind), 6.0, &['*', '+', '·']);
            }
        }
        self.clearing_rows = full;
        self.phase = Phase::Clearing(0.35);
    }

    fn finish_clear(&mut self) {
        let rows = std::mem::take(&mut self.clearing_rows);
        let n = rows.len() as u32;
        for &y in &rows {
            for yy in (1..=y).rev() {
                for x in 0..BW {
                    self.board[(yy * BW + x) as usize] = self.board[((yy - 1) * BW + x) as usize];
                }
            }
            for x in 0..BW {
                self.board[x as usize] = None;
            }
        }
        self.lines += n;
        self.level = self.lines / 10 + 1;
        self.phase = Phase::Playing;
        self.spawn();
    }
}

impl Game for TetrisGame {
    fn kind(&self) -> GameKind {
        GameKind::Tetris
    }

    fn score(&self) -> u32 {
        self.score
    }

    fn is_over(&self) -> bool {
        self.phase == Phase::GameOver
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if self.phase != Phase::Playing {
            return matches!(key.code, KeyCode::Char('a' | 'd' | 'w' | 's' | 'c' | ' ') | KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down);
        }
        match key.code {
            KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('A') => {
                self.try_shift(-1);
            }
            KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => {
                self.try_shift(1);
            }
            KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') => self.try_rotate(1),
            KeyCode::Char('q') | KeyCode::Char('Q') => self.try_rotate(-1),
            KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => {
                if self.drop_one() {
                    self.score += 1;
                    self.gravity_timer = 0.0;
                }
            }
            KeyCode::Char(' ') => {
                let mut dropped = 0;
                while self.drop_one() {
                    dropped += 1;
                }
                self.score += dropped * 2;
                let p = self.current;
                self.particles.burst(p.x as f32 + 0.5, p.y as f32 + 1.0, 6, piece_color(p.kind), 5.0, &['·', '\'', '.']);
                self.lock();
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                if !self.hold_used {
                    let kind = self.current.kind;
                    match self.hold.take() {
                        Some(held) => {
                            self.current = Piece { kind: held, rot: 0, x: BW / 2 - 1, y: 1 };
                        }
                        None => self.spawn(),
                    }
                    self.hold = Some(kind);
                    self.hold_used = true;
                    if self.collides(self.current) {
                        self.phase = Phase::GameOver;
                    }
                }
            }
            _ => return false,
        }
        true
    }

    fn tick(&mut self, dt: f32) {
        self.particles.tick(dt, 8.0);
        self.popups.tick(dt);
        match self.phase {
            Phase::GameOver => return,
            Phase::Clearing(tl) => {
                if tl - dt <= 0.0 {
                    self.finish_clear();
                } else {
                    self.phase = Phase::Clearing(tl - dt);
                }
                return;
            }
            Phase::Playing => {}
        }
        self.gravity_timer += dt;
        if self.gravity_timer >= self.gravity_interval() {
            self.gravity_timer = 0.0;
            if !self.drop_one() {
                self.lock_timer += self.gravity_interval();
            }
        }
        if !self.drop_one_peek() {
            self.lock_timer += dt;
            if self.lock_timer >= 0.45 {
                self.lock();
            }
        } else {
            self.lock_timer = 0.0;
        }
    }

    fn render(&self, grid: &mut CellGrid, phase: f32) {
        let bg = t().panel_bg;
        let cell_w = if grid.width as i32 >= BW * 2 + 26 { 2 } else { 1 };
        let board_w = BW * cell_w;
        let visible_rows = (grid.height as i32 - 2).min(BH);
        let x0 = (grid.width as i32 - board_w - 14).max(0) / 2 + 1;
        let y0 = 1;
        let top_row = BH + HIDDEN - visible_rows; // first board row drawn

        // Frame.
        let frame_color = t().accent4;
        for y in 0..visible_rows {
            grid.set(x0 - 1, y0 + y, '│', frame_color, bg);
            grid.set(x0 + board_w, y0 + y, '│', frame_color, bg);
        }
        grid.text(x0 - 1, y0 + visible_rows, &format!("╰{}╯", "─".repeat(board_w as usize)), frame_color, bg);
        grid.text(x0 - 1, y0 - 1, &format!("╭{}╮", "─".repeat(board_w as usize)), frame_color, bg);

        let draw_cell = |grid: &mut CellGrid, bx: i32, by: i32, ch: char, color: Color| {
            let row = by - top_row;
            if row < 0 || row >= visible_rows {
                return;
            }
            for i in 0..cell_w {
                grid.set(x0 + bx * cell_w + i, y0 + row, ch, color, bg);
            }
        };

        // Settled blocks.
        let flash = ((phase * 16.0) as i32) % 2 == 0;
        for y in 0..BH + HIDDEN {
            for x in 0..BW {
                if let Some(kind) = self.board[(y * BW + x) as usize] {
                    let clearing = self.clearing_rows.contains(&y);
                    let (r, g, b) = piece_color(kind);
                    let (ch, color) = if clearing {
                        if flash { ('█', rgb(255, 255, 255)) } else { ('▒', rgb(r, g, b)) }
                    } else {
                        ('█', rgb(r, g, b))
                    };
                    draw_cell(grid, x, y, ch, color);
                }
            }
        }

        if self.phase == Phase::Playing {
            // Ghost, then the live piece.
            let p = self.current;
            let gy = self.ghost_y();
            let (r, g, b) = piece_color(p.kind);
            for (dx, dy) in cells(p.kind, p.rot) {
                draw_cell(grid, p.x + dx, gy + dy, '░', rgb(r / 2, g / 2, b / 2));
            }
            for (dx, dy) in cells(p.kind, p.rot) {
                draw_cell(grid, p.x + dx, p.y + dy, '█', rgb(r, g, b));
            }
        }

        self.particles.draw(grid, |x, y| (x0 + (x * cell_w as f32) as i32, y0 + y as i32 - top_row));

        // Side panel.
        let sx = x0 + board_w + 3;
        if sx + 10 <= grid.width as i32 {
            grid.text(sx, y0, "NEXT", t().accent2, bg);
            for (i, &kind) in self.queue.iter().take(3).enumerate() {
                let (r, g, b) = piece_color(kind);
                for (dx, dy) in cells(kind, 0) {
                    let px = sx + 2 + (dx + 1) * cell_w;
                    let py = y0 + 2 + i as i32 * 3 + dy + 1;
                    for k in 0..cell_w {
                        grid.set(px + k, py, '█', rgb(r, g, b), bg);
                    }
                }
            }
            let hy = y0 + 12;
            grid.text(sx, hy, "HOLD", if self.hold_used { t().muted } else { t().accent2 }, bg);
            if let Some(kind) = self.hold {
                let (r, g, b) = piece_color(kind);
                for (dx, dy) in cells(kind, 0) {
                    let px = sx + 2 + (dx + 1) * cell_w;
                    let py = hy + 2 + dy + 1;
                    for k in 0..cell_w {
                        grid.set(px + k, py, '█', rgb(r, g, b), bg);
                    }
                }
            }
            let iy = (hy + 5).min(grid.height as i32 - 4);
            grid.text(sx, iy, &format!("SCORE {}", self.score), t().text, bg);
            grid.text(sx, iy + 1, &format!("LINES {}", self.lines), t().text, bg);
            grid.text(sx, iy + 2, &format!("LEVEL {}", self.level), t().text, bg);
            if !self.last_clear_text.is_empty() {
                grid.text(sx, iy + 3, &self.last_clear_text, t().accent2, bg);
            }
        }

        let hint = "A/D move  W rotate  S soft  Space drop  C hold";
        if (hint.len() as i32) < grid.width as i32 {
            grid.text(0, grid.height as i32 - 1, hint, t().muted, bg);
        }

        if self.phase == Phase::GameOver {
            let mid = grid.height as i32 / 2;
            grid.banner(mid, "STACK OVERFLOW", t().danger, t().bg_base);
            grid.center_text(mid + 2, "R restart   Esc menu", t().text, t().bg_base);
        }
    }
}

impl TetrisGame {
    fn drop_one_peek(&self) -> bool {
        let p = Piece { y: self.current.y + 1, ..self.current };
        !self.collides(p)
    }
}
