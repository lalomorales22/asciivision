//! Arcade bay: four games rendered into a tile.
//!
//! Shared pieces live here (the panel + menu, high scores, the `CellGrid`
//! back buffer, and a particle system); each game is its own module and
//! implements the `Game` trait.

mod arena;
mod caves;
mod doom;
mod invaders;
mod pacman;
mod raycast;
mod tetris;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders},
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};

use crate::client::NetHandle;
use crate::theme::{color_to_rgb, t};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameKind {
    PacMan,
    SpaceInvaders,
    IceCaves,
    Tetris,
    Arena,
    Frostbite,
}

impl GameKind {
    pub const ALL: [Self; 6] = [Self::PacMan, Self::SpaceInvaders, Self::IceCaves, Self::Tetris, Self::Arena, Self::Frostbite];

    pub fn index(self) -> usize {
        match self {
            Self::PacMan => 0,
            Self::SpaceInvaders => 1,
            Self::IceCaves => 2,
            Self::Tetris => 3,
            Self::Arena => 4,
            Self::Frostbite => 5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PacMan => "PAC-MAN",
            Self::SpaceInvaders => "SPACE INVADERS",
            Self::IceCaves => "PENGUIN ICE CAVES",
            Self::Tetris => "BLOCKDROP",
            Self::Arena => "ICE ARENA",
            Self::Frostbite => "FROSTBITE",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::PacMan => "pacman",
            Self::SpaceInvaders => "invaders",
            Self::IceCaves => "caves",
            Self::Tetris => "tetris",
            Self::Arena => "arena",
            Self::Frostbite => "frostbite",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            Self::PacMan => "true-scale maze, four ghost personalities, fruit, power mode",
            Self::SpaceInvaders => "animated squadrons, bunkers, mystery UFO, explosions",
            Self::IceCaves => "first-person raycast caves: collect fish, dodge seals",
            Self::Tetris => "hold, ghost piece, hard drop, line-clear fireworks",
            Self::Arena => "multiplayer first-person snowball deathmatch over the video chat room",
            Self::Frostbite => "co-op FPS campaign: doors, keys, weapons, seals, walruses and yetis",
        }
    }

    pub fn controls(self) -> &'static str {
        match self {
            Self::PacMan => "WASD/arrows move",
            Self::SpaceInvaders => "A/D move  W/Space fire  S shield",
            Self::IceCaves => "W/S move  A/D turn  Q/E strafe",
            Self::Tetris => "A/D move  W rotate  S soft  Space drop  C hold",
            Self::Arena => "W/S move  A/D turn  Q/E strafe  Space throw",
            Self::Frostbite => "W/S A/D Q/E move  Space throw  1/2 weapon",
        }
    }

    pub fn from_input(input: &str) -> Option<Self> {
        match input.trim().to_lowercase().as_str() {
            "1" | "pac" | "pacman" | "pac-man" => Some(Self::PacMan),
            "2" | "space" | "invaders" | "space-invaders" | "space invaders" => {
                Some(Self::SpaceInvaders)
            }
            "3" | "penguin" | "3d" | "caves" | "icecaves" | "ice-caves" | "peng" => Some(Self::IceCaves),
            "4" | "tetris" | "blocks" | "blockdrop" => Some(Self::Tetris),
            "5" | "arena" | "deathmatch" | "snowball" | "multiplayer" => Some(Self::Arena),
            "6" | "frostbite" | "doom" | "campaign" | "fps" | "coop" | "co-op" => Some(Self::Frostbite),
            _ => None,
        }
    }

    fn cycle_next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    fn cycle_prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// Everything a game needs to expose to the panel.
pub(crate) trait Game {
    fn kind(&self) -> GameKind;
    /// Return true when the key was consumed.
    fn handle_key(&mut self, key: KeyEvent) -> bool;
    fn tick(&mut self, dt: f32);
    fn render(&self, grid: &mut CellGrid, phase: f32);
    fn score(&self) -> u32;
    fn is_over(&self) -> bool;
    /// Called every tick with the room's network handle, if connected.
    fn set_net(&mut self, _net: Option<NetHandle>) {}
}

fn new_game(kind: GameKind) -> Box<dyn Game> {
    match kind {
        GameKind::PacMan => Box::new(pacman::PacManGame::new()),
        GameKind::SpaceInvaders => Box::new(invaders::InvadersGame::new()),
        GameKind::IceCaves => Box::new(caves::CavesGame::new()),
        GameKind::Tetris => Box::new(tetris::TetrisGame::new()),
        GameKind::Arena => Box::new(arena::ArenaGame::new()),
        GameKind::Frostbite => Box::new(doom::DoomGame::new()),
    }
}

// ---------------------------------------------------------------------------
// High scores (persisted beside the conversation database)
// ---------------------------------------------------------------------------

#[derive(Default, Serialize, Deserialize)]
struct HighScores {
    best: HashMap<String, u32>,
}

impl HighScores {
    fn path() -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        Some(PathBuf::from(home).join(".config/asciivision/highscores.json"))
    }

    fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        if let Some(path) = Self::path() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(json) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(path, json);
            }
        }
    }

    fn get(&self, kind: GameKind) -> u32 {
        self.best.get(kind.key()).copied().unwrap_or(0)
    }

    /// Returns true when `score` is a new record.
    fn submit(&mut self, kind: GameKind, score: u32) -> bool {
        if score > self.get(kind) {
            self.best.insert(kind.key().to_string(), score);
            self.save();
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------

pub struct GamesPanel {
    selected: GameKind,
    session: Option<Box<dyn Game>>,
    paused: bool,
    status: String,
    high_scores: HighScores,
    /// Set once per session when the game ends, so the record is only
    /// submitted once.
    result_recorded: bool,
    new_record: bool,
    menu_particles: Particles,
    menu_time: f32,
    net: Option<NetHandle>,
}

impl GamesPanel {
    pub fn new() -> Self {
        Self {
            selected: GameKind::PacMan,
            session: None,
            paused: false,
            status: "games bay online".to_string(),
            high_scores: HighScores::load(),
            result_recorded: false,
            new_record: false,
            menu_particles: Particles::default(),
            menu_time: 0.0,
            net: None,
        }
    }

    /// Hand the room's network handle to the panel (and the running game).
    pub fn set_net(&mut self, net: Option<NetHandle>) {
        self.net = net;
    }

    pub fn status_note(&self) -> &str {
        &self.status
    }

    pub fn next_game(&mut self) {
        self.selected = self.selected.cycle_next();
        self.status = format!("games: selected {}", self.selected.label());
    }

    pub fn previous_game(&mut self) {
        self.selected = self.selected.cycle_prev();
        self.status = format!("games: selected {}", self.selected.label());
    }

    pub fn activate_selected(&mut self) {
        self.session = Some(new_game(self.selected));
        self.paused = false;
        self.result_recorded = false;
        self.new_record = false;
        self.status = format!("games: {} // {}", self.selected.label(), self.selected.controls());
    }

    pub fn launch(&mut self, game: GameKind) {
        self.selected = game;
        self.activate_selected();
    }

    pub fn stop(&mut self) {
        self.record_result();
        self.session = None;
        self.paused = false;
        self.status = "games: selector ready".to_string();
    }

    fn record_result(&mut self) {
        if self.result_recorded {
            return;
        }
        if let Some(session) = &self.session {
            let score = session.score();
            if score > 0 {
                self.new_record = self.high_scores.submit(session.kind(), score);
                self.result_recorded = true;
            }
        }
    }

    pub fn tick(&mut self, dt: f32) {
        let dt = dt.min(0.05);
        self.menu_time += dt;
        if self.session.is_none() {
            self.menu_particles.tick(dt, 0.0);
            return;
        }
        if self.paused {
            return;
        }
        if let Some(session) = &mut self.session {
            session.set_net(self.net.clone());
            session.tick(dt);
        }
        let over = self.session.as_ref().map(|s| s.is_over()).unwrap_or(false);
        if over && !self.result_recorded {
            self.record_result();
            let kind = self.selected;
            self.status = if self.new_record {
                format!("games: NEW RECORD on {}!", kind.label())
            } else {
                format!("games: {} over // R restart, Esc menu", kind.label())
            };
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if self.session.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.stop();
                    return true;
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    self.record_result();
                    self.activate_selected();
                    self.status = format!("games: restarted {}", self.selected.label());
                    return true;
                }
                KeyCode::Char('p') | KeyCode::Char('P') => {
                    self.paused = !self.paused;
                    self.status = if self.paused {
                        "games: paused (P resumes)".to_string()
                    } else {
                        format!("games: {}", self.selected.label())
                    };
                    return true;
                }
                _ => {}
            }
            if self.paused {
                return true;
            }
            if let Some(session) = &mut self.session {
                return session.handle_key(key);
            }
            return false;
        }

        if let Some(game) = key_to_selection(key.code) {
            self.launch(game);
            return true;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.activate_selected();
                true
            }
            KeyCode::Up | KeyCode::Left | KeyCode::Char('w') | KeyCode::Char('W') | KeyCode::Char('a')
            | KeyCode::Char('A') => {
                self.previous_game();
                true
            }
            KeyCode::Down | KeyCode::Right | KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Char('d')
            | KeyCode::Char('D') => {
                self.next_game();
                true
            }
            _ => false,
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, phase: f32, is_focused: bool) {
        let title = match &self.session {
            Some(session) if self.paused => format!(" GAMES // {} // PAUSED ", session.kind().label()),
            Some(session) => format!(
                " GAMES // {} // HI {:05} ",
                session.kind().label(),
                self.high_scores.get(session.kind())
            ),
            None => " GAMES // ARCADE BAY ".to_string(),
        };
        let border_color = if is_focused { t().accent4 } else { t().accent1 };
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(t().accent2).bold())
            .borders(Borders::ALL)
            .border_type(if is_focused { t().border } else { t().border_alt })
            .border_style(Style::default().fg(border_color));
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        let mut grid = CellGrid::new(inner.width, inner.height, t().panel_bg, t().text);
        if inner.width < 20 || inner.height < 8 {
            grid.center_text(inner.height as i32 / 2, "Grow this tile to play.", t().muted, t().panel_bg);
            grid.present(frame.buffer_mut(), inner);
            return;
        }

        match &self.session {
            Some(session) => {
                session.render(&mut grid, phase);
                if self.paused {
                    let mid = grid.height as i32 / 2;
                    grid.banner(mid, "PAUSED", t().accent2, t().bg_base);
                    grid.center_text(mid + 1, "P resume  R restart  Esc menu", t().text, t().bg_base);
                }
                if session.is_over() && self.new_record {
                    let y = (grid.height as i32 / 2 + 3).min(grid.height as i32 - 1);
                    grid.center_text(y, "*** NEW HIGH SCORE ***", t().accent2, t().bg_base);
                }
            }
            None => self.render_menu(&mut grid, phase, is_focused),
        }
        grid.present(frame.buffer_mut(), inner);
    }

    fn render_menu(&mut self, grid: &mut CellGrid, phase: f32, is_focused: bool) {
        let w = grid.width as i32;
        let h = grid.height as i32;

        // Drifting star particles behind the menu.
        if self.menu_particles.items.len() < (w as usize * h as usize / 40).max(12) {
            let mut rng = rand::thread_rng();
            use rand::Rng;
            self.menu_particles.items.push(Particle {
                x: rng.gen_range(0.0..w as f32),
                y: rng.gen_range(0.0..h as f32),
                vx: rng.gen_range(-0.4..0.4),
                vy: rng.gen_range(0.3..1.4),
                life: rng.gen_range(3.0..8.0),
                max_life: 8.0,
                ch: ['·', '.', '+', '*'][rng.gen_range(0..4)],
                color: color_to_rgb(t().muted),
            });
        }
        self.menu_particles.items.retain(|p| p.y < h as f32 + 1.0);
        self.menu_particles.draw(grid, |x, y| (x as i32, y as i32));

        let logo = [
            " ▄▀█ █▀█ █▀▀ ▄▀█ █▀▄ █▀▀   █▄▄ ▄▀█ █▄█",
            " █▀█ █▀▄ █▄▄ █▀█ █▄▀ ██▄   █▄█ █▀█  █ ",
        ];
        let logo_w = logo[0].chars().count() as i32;
        let compact = w < logo_w + 2 || h < 24;
        let mut y = 1;
        if !compact {
            let hue_shift = (phase * 40.0) % 360.0;
            for (i, line) in logo.iter().enumerate() {
                let x0 = (w - logo_w) / 2;
                for (j, ch) in line.chars().enumerate() {
                    let hue = (hue_shift + j as f32 * 6.0 + i as f32 * 20.0) % 360.0;
                    let (r, g, b) = crate::theme::hsl_to_rgb(hue, 0.85, 0.62);
                    grid.set(x0 + j as i32, y + i as i32, ch, Color::Rgb(r, g, b), t().panel_bg);
                }
            }
            y += 3;
        } else {
            grid.center_text(y, "ARCADE BAY", t().accent2, t().panel_bg);
            y += 2;
        }

        let hint = if is_focused {
            "Up/Down or 1-6 pick  //  Enter play  //  in game: P pause, R restart, Esc menu"
        } else {
            "focus this tile (Ctrl+h/j/k/l or F8) to play"
        };
        grid.center_text(y, hint, t().muted, t().panel_bg);
        y += 2;

        for (idx, game) in GameKind::ALL.iter().enumerate() {
            let selected = *game == self.selected;
            let blink = ((phase * 3.0) as i32) % 2 == 0;
            let accent = if selected { t().accent2 } else { t().text };
            let pointer = if selected && blink { "▶" } else { " " };
            let best = self.high_scores.get(*game);
            let x0 = 2;
            grid.text(x0, y, pointer, t().accent4, t().panel_bg);
            grid.text(x0 + 2, y, &format!("{}. {:<18}", idx + 1, game.label()), accent, t().panel_bg);
            let best_text = if best > 0 { format!("best {:>6}", best) } else { "unplayed".to_string() };
            grid.text(w - best_text.len() as i32 - 2, y, &best_text, if best > 0 { t().accent1 } else { t().muted }, t().panel_bg);
            if !compact {
                grid.text(x0 + 5, y + 1, game.subtitle(), if selected { t().text } else { t().muted }, t().panel_bg);
                y += 3;
            } else {
                y += 1;
            }
        }

        if !compact && y < h - 1 {
            let controls = format!("controls: {}", self.selected.controls());
            grid.center_text(h - 2, &controls, t().accent4, t().panel_bg);
        }
    }
}

fn key_to_selection(code: KeyCode) -> Option<GameKind> {
    match code {
        KeyCode::Char('1') => Some(GameKind::PacMan),
        KeyCode::Char('2') => Some(GameKind::SpaceInvaders),
        KeyCode::Char('3') => Some(GameKind::IceCaves),
        KeyCode::Char('4') => Some(GameKind::Tetris),
        KeyCode::Char('5') => Some(GameKind::Arena),
        KeyCode::Char('6') => Some(GameKind::Frostbite),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// CellGrid: a back buffer the games draw into, presented once per frame.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub(crate) struct GridCell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
}

pub(crate) struct CellGrid {
    pub width: u16,
    pub height: u16,
    cells: Vec<GridCell>,
}

impl CellGrid {
    pub fn new(width: u16, height: u16, bg: Color, fg: Color) -> Self {
        Self {
            width,
            height,
            cells: vec![GridCell { ch: ' ', fg, bg }; width as usize * height as usize],
        }
    }

    #[inline]
    fn index(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            None
        } else {
            Some(y as usize * self.width as usize + x as usize)
        }
    }

    pub fn set(&mut self, x: i32, y: i32, ch: char, fg: Color, bg: Color) {
        if let Some(idx) = self.index(x, y) {
            self.cells[idx] = GridCell { ch, fg, bg };
        }
    }

    /// Set the glyph and foreground, keeping whatever background is there.
    pub fn put(&mut self, x: i32, y: i32, ch: char, fg: Color) {
        if let Some(idx) = self.index(x, y) {
            let bg = self.cells[idx].bg;
            self.cells[idx] = GridCell { ch, fg, bg };
        }
    }


    #[cfg(test)]
    pub fn get_char(&self, x: i32, y: i32) -> char {
        self.index(x, y).map(|i| self.cells[i].ch).unwrap_or(' ')
    }

    #[allow(clippy::too_many_arguments)]
    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, ch: char, fg: Color, bg: Color) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.set(xx, yy, ch, fg, bg);
            }
        }
    }

    pub fn text(&mut self, x: i32, y: i32, text: &str, fg: Color, bg: Color) {
        for (i, ch) in text.chars().enumerate() {
            self.set(x + i as i32, y, ch, fg, bg);
        }
    }

    /// Text over whatever background is already drawn.
    pub fn text_over(&mut self, x: i32, y: i32, text: &str, fg: Color) {
        for (i, ch) in text.chars().enumerate() {
            self.put(x + i as i32, y, ch, fg);
        }
    }

    pub fn center_text(&mut self, y: i32, text: &str, fg: Color, bg: Color) {
        let width = text.chars().count() as i32;
        let x = ((self.width as i32 - width) / 2).max(0);
        self.text(x, y, text, fg, bg);
    }

    /// A centred, boxed headline (game over, level banners).
    pub fn banner(&mut self, y: i32, text: &str, fg: Color, bg: Color) {
        let width = text.chars().count() as i32 + 4;
        let x = ((self.width as i32 - width) / 2).max(0);
        self.fill(x, y - 1, width, 3, ' ', fg, bg);
        self.text(x, y - 1, &"▄".repeat(width as usize), fg, bg);
        self.text(x + 2, y, text, fg, bg);
        self.text(x, y + 1, &"▀".repeat(width as usize), fg, bg);
    }

    pub fn present(&self, buffer: &mut Buffer, area: Rect) {
        for y in 0..self.height.min(area.height) {
            for x in 0..self.width.min(area.width) {
                let idx = y as usize * self.width as usize + x as usize;
                if let Some(cell) = self.cells.get(idx) {
                    if let Some(buf_cell) = buffer.cell_mut((area.x + x, area.y + y)) {
                        buf_cell.set_char(cell.ch);
                        buf_cell.set_fg(cell.fg);
                        buf_cell.set_bg(cell.bg);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Particles
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub(crate) struct Particle {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub life: f32,
    pub max_life: f32,
    pub ch: char,
    pub color: (u8, u8, u8),
}

#[derive(Default)]
pub(crate) struct Particles {
    pub items: Vec<Particle>,
}

impl Particles {
    /// Spawn `count` particles flying out of `(x, y)` in logical units.
    #[allow(clippy::too_many_arguments)]
    pub fn burst(&mut self, x: f32, y: f32, count: usize, color: (u8, u8, u8), speed: f32, chars: &[char]) {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        for _ in 0..count {
            let angle = rng.gen_range(0.0..std::f32::consts::TAU);
            let v = rng.gen_range(speed * 0.3..speed);
            let life = rng.gen_range(0.35..0.9);
            self.items.push(Particle {
                x,
                y,
                vx: angle.cos() * v,
                vy: angle.sin() * v * 0.6,
                life,
                max_life: life,
                ch: chars[rng.gen_range(0..chars.len())],
                color,
            });
        }
    }

    pub fn tick(&mut self, dt: f32, gravity: f32) {
        for p in &mut self.items {
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.vy += gravity * dt;
            p.life -= dt;
        }
        self.items.retain(|p| p.life > 0.0);
    }

    /// Draw with a projection from logical coordinates to grid cells.
    pub fn draw(&self, grid: &mut CellGrid, project: impl Fn(f32, f32) -> (i32, i32)) {
        for p in &self.items {
            let fade = (p.life / p.max_life).clamp(0.0, 1.0);
            let (r, g, b) = p.color;
            let color = Color::Rgb(
                (r as f32 * (0.35 + 0.65 * fade)) as u8,
                (g as f32 * (0.35 + 0.65 * fade)) as u8,
                (b as f32 * (0.35 + 0.65 * fade)) as u8,
            );
            let (x, y) = project(p.x, p.y);
            let ch = if fade > 0.5 { p.ch } else { '.' };
            grid.put(x, y, ch, color);
        }
    }
}

// ---------------------------------------------------------------------------
// Floating score popups
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub(crate) struct Popup {
    pub x: f32,
    pub y: f32,
    pub text: String,
    pub life: f32,
    pub color: (u8, u8, u8),
}

#[derive(Default)]
pub(crate) struct Popups {
    pub items: Vec<Popup>,
}

impl Popups {
    pub fn add(&mut self, x: f32, y: f32, text: impl Into<String>, color: (u8, u8, u8)) {
        self.items.push(Popup {
            x,
            y,
            text: text.into(),
            life: 1.1,
            color,
        });
    }

    pub fn tick(&mut self, dt: f32) {
        for p in &mut self.items {
            p.life -= dt;
            p.y -= dt * 1.5;
        }
        self.items.retain(|p| p.life > 0.0);
    }

    pub fn draw(&self, grid: &mut CellGrid, project: impl Fn(f32, f32) -> (i32, i32)) {
        for p in &self.items {
            let (x, y) = project(p.x, p.y);
            let (r, g, b) = p.color;
            let half = p.text.chars().count() as i32 / 2;
            grid.text_over(x - half, y, &p.text, Color::Rgb(r, g, b));
        }
    }
}

/// Map a logical coordinate in `0..src_max` onto `0..dst` cells.
pub(crate) fn project_axis(value: f32, src_max: f32, dst: u16) -> i32 {
    if dst == 0 {
        return 0;
    }
    let scaled = (value / src_max.max(1.0)) * dst.saturating_sub(1) as f32;
    scaled.round() as i32
}

pub(crate) fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

pub(crate) fn dim(c: (u8, u8, u8), k: f32) -> Color {
    Color::Rgb(
        (c.0 as f32 * k).clamp(0.0, 255.0) as u8,
        (c.1 as f32 * k).clamp(0.0, 255.0) as u8,
        (c.2 as f32 * k).clamp(0.0, 255.0) as u8,
    )
}
