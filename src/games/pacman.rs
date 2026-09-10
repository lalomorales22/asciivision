//! Pac-Man: true-scale maze with box-drawing walls, four ghost
//! personalities (chase / ambush / flank / shy), scatter-chase cycles,
//! frightened mode with a catch chain, eyes returning home, fruit, a READY
//! countdown, death and level-clear animations.

use super::{dim, rgb, CellGrid, Game, GameKind, Particles, Popups};
use crate::theme::{color_to_rgb, t};
use crossterm::event::{KeyCode, KeyEvent};
use rand::Rng;
use std::collections::HashSet;

const MAZE: [&str; 21] = [
    "###############################",
    "#............##............o..#",
    "#.####.#####.##.#####.####.#..#",
    "#.#  #.#   #.##.#   #.#  #.#..#",
    "#.####.#####.##.#####.####.#..#",
    "#............................#.#",
    "#.####.##.########.##.####.#..#",
    "#......##....##....##......#..#",
    "######.##### ## #####.######..#",
    "     #.##### ## #####.#       #",
    "######.##          ##.######  #",
    "#......## ###GG### ##......#  #",
    "#.####.## #      # ##.####.#  #",
    "#....#.... # P  # ....#....#  #",
    "####.#.#######  #######.#.### #",
    "#............##............o..#",
    "#.####.#####.##.#####.####.#..#",
    "#...##................##...#..#",
    "###.##.##.########.##.##.###..#",
    "#......##....##....##......#..#",
    "###############################",
];
const W: i32 = 31;
const H: i32 = 21;
const PAC_START: (i32, i32) = (14, 13);
const FRUIT_TILE: (i32, i32) = (14, 13);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Up,
    Right,
    Down,
    Left,
}

impl Dir {
    const ALL: [Dir; 4] = [Dir::Up, Dir::Right, Dir::Down, Dir::Left];
    fn delta(self) -> (i32, i32) {
        match self {
            Dir::Up => (0, -1),
            Dir::Right => (1, 0),
            Dir::Down => (0, 1),
            Dir::Left => (-1, 0),
        }
    }
    fn opposite(self) -> Dir {
        match self {
            Dir::Up => Dir::Down,
            Dir::Right => Dir::Left,
            Dir::Down => Dir::Up,
            Dir::Left => Dir::Right,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GhostState {
    Normal,
    Frightened,
    Eyes,
}

#[derive(Clone, Copy)]
struct Ghost {
    x: i32,
    y: i32,
    dir: Dir,
    spawn: (i32, i32),
    home: (i32, i32),
    color: (u8, u8, u8),
    state: GhostState,
    /// Seconds before this ghost leaves the house.
    dormant: f32,
    personality: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Ready(f32),
    Playing,
    Dying(f32),
    LevelClear(f32),
    GameOver,
}

pub struct PacManGame {
    pac: (i32, i32),
    pac_dir: Dir,
    desired_dir: Dir,
    mouth_open: bool,
    ghosts: Vec<Ghost>,
    pellets: HashSet<(i32, i32)>,
    power_pellets: HashSet<(i32, i32)>,
    score: u32,
    lives: u8,
    level: u32,
    eaten_this_level: u32,
    pac_timer: f32,
    ghost_timer: f32,
    mode_timer: f32,
    scatter_mode: bool,
    frightened_timer: f32,
    catch_chain: u32,
    fruit_timer: f32,
    fruit_spawned: [bool; 2],
    phase: Phase,
    elapsed: f32,
    particles: Particles,
    popups: Popups,
}

impl PacManGame {
    pub fn new() -> Self {
        let ghosts = vec![
            Ghost { x: 14, y: 10, dir: Dir::Left, spawn: (14, 10), home: (W - 2, 1), color: (255, 80, 80), state: GhostState::Normal, dormant: 0.0, personality: 0 },
            Ghost { x: 13, y: 11, dir: Dir::Left, spawn: (13, 11), home: (1, 1), color: (255, 160, 220), state: GhostState::Normal, dormant: 2.5, personality: 1 },
            Ghost { x: 15, y: 11, dir: Dir::Left, spawn: (15, 11), home: (W - 2, H - 2), color: (70, 230, 230), state: GhostState::Normal, dormant: 5.0, personality: 2 },
            Ghost { x: 14, y: 11, dir: Dir::Left, spawn: (14, 11), home: (1, H - 2), color: (255, 170, 70), state: GhostState::Normal, dormant: 7.5, personality: 3 },
        ];
        let mut game = Self {
            pac: PAC_START,
            pac_dir: Dir::Left,
            desired_dir: Dir::Left,
            mouth_open: true,
            ghosts,
            pellets: HashSet::new(),
            power_pellets: HashSet::new(),
            score: 0,
            lives: 3,
            level: 1,
            eaten_this_level: 0,
            pac_timer: 0.0,
            ghost_timer: 0.0,
            mode_timer: 0.0,
            scatter_mode: true,
            frightened_timer: 0.0,
            catch_chain: 0,
            fruit_timer: 0.0,
            fruit_spawned: [false, false],
            phase: Phase::Ready(2.0),
            elapsed: 0.0,
            particles: Particles::default(),
            popups: Popups::default(),
        };
        game.reset_level();
        game
    }

    fn reset_level(&mut self) {
        self.pellets.clear();
        self.power_pellets.clear();
        for y in 0..H {
            for x in 0..W {
                match tile(x, y) {
                    '.' => {
                        self.pellets.insert((x, y));
                    }
                    'o' => {
                        self.power_pellets.insert((x, y));
                    }
                    _ => {}
                }
            }
        }
        self.eaten_this_level = 0;
        self.fruit_spawned = [false, false];
        self.fruit_timer = 0.0;
        self.reset_round();
    }

    fn reset_round(&mut self) {
        self.pac = PAC_START;
        self.pac_dir = Dir::Left;
        self.desired_dir = Dir::Left;
        for (i, ghost) in self.ghosts.iter_mut().enumerate() {
            ghost.x = ghost.spawn.0;
            ghost.y = ghost.spawn.1;
            ghost.dir = Dir::Left;
            ghost.state = GhostState::Normal;
            ghost.dormant = i as f32 * 2.5;
        }
        self.frightened_timer = 0.0;
        self.catch_chain = 0;
        self.pac_timer = 0.0;
        self.ghost_timer = 0.0;
        self.mode_timer = 0.0;
        self.scatter_mode = true;
        self.phase = Phase::Ready(2.0);
    }

    fn pac_step_time(&self) -> f32 {
        (0.125 - self.level as f32 * 0.005).max(0.085)
    }

    fn ghost_step_time(&self) -> f32 {
        (0.165 - self.level as f32 * 0.008).max(0.095)
    }

    fn frightened_duration(&self) -> f32 {
        (8.5 - self.level as f32 * 0.8).max(2.0)
    }

    fn step_pac(&mut self) {
        if can_move(self.pac, self.desired_dir) {
            self.pac_dir = self.desired_dir;
        }
        if can_move(self.pac, self.pac_dir) {
            let (dx, dy) = self.pac_dir.delta();
            self.pac = wrap(self.pac.0 + dx, self.pac.1 + dy);
            self.mouth_open = !self.mouth_open;
        }

        if self.pellets.remove(&self.pac) {
            self.score += 10;
            self.eaten_this_level += 1;
        }
        if self.power_pellets.remove(&self.pac) {
            self.score += 50;
            self.eaten_this_level += 1;
            self.frightened_timer = self.frightened_duration();
            self.catch_chain = 0;
            for ghost in &mut self.ghosts {
                if ghost.state == GhostState::Normal {
                    ghost.state = GhostState::Frightened;
                    ghost.dir = ghost.dir.opposite();
                }
            }
            self.particles.burst(self.pac.0 as f32, self.pac.1 as f32, 10, (120, 200, 255), 6.0, &['*', '+', '·']);
        }

        // Fruit appears twice per level.
        for (i, threshold) in [60u32, 150].iter().enumerate() {
            if !self.fruit_spawned[i] && self.eaten_this_level >= *threshold {
                self.fruit_spawned[i] = true;
                self.fruit_timer = 9.5;
            }
        }
        if self.fruit_timer > 0.0 && self.pac == FRUIT_TILE {
            self.fruit_timer = 0.0;
            let bonus = 100 * self.level.min(10);
            self.score += bonus;
            self.popups.add(FRUIT_TILE.0 as f32, FRUIT_TILE.1 as f32, format!("{}", bonus), (255, 120, 220));
            self.particles.burst(FRUIT_TILE.0 as f32, FRUIT_TILE.1 as f32, 12, (255, 120, 220), 7.0, &['*', '♦', '·']);
        }

        self.resolve_collisions();

        if self.pellets.is_empty() && self.power_pellets.is_empty() && self.phase == Phase::Playing {
            self.phase = Phase::LevelClear(2.0);
        }
    }

    fn ghost_target(&self, ghost: &Ghost) -> (i32, i32) {
        if ghost.state == GhostState::Eyes {
            return ghost.spawn;
        }
        if self.scatter_mode {
            return ghost.home;
        }
        let (dx, dy) = self.pac_dir.delta();
        match ghost.personality {
            0 => self.pac,
            1 => (self.pac.0 + dx * 4, self.pac.1 + dy * 4),
            2 => {
                let ahead = (self.pac.0 + dx * 2, self.pac.1 + dy * 2);
                let blinky = &self.ghosts[0];
                (ahead.0 + (ahead.0 - blinky.x), ahead.1 + (ahead.1 - blinky.y))
            }
            _ => {
                if manhattan((ghost.x, ghost.y), self.pac) > 8 {
                    self.pac
                } else {
                    ghost.home
                }
            }
        }
    }

    fn step_ghosts(&mut self, eyes_only: bool) {
        let mut rng = rand::thread_rng();
        let count = self.ghosts.len();
        for i in 0..count {
            let ghost = self.ghosts[i];
            if eyes_only && ghost.state != GhostState::Eyes {
                continue;
            }
            if !eyes_only && ghost.state == GhostState::Eyes {
                continue;
            }
            if ghost.dormant > 0.0 {
                continue;
            }
            let mut choices: Vec<Dir> = Dir::ALL
                .iter()
                .copied()
                .filter(|d| *d != ghost.dir.opposite() && can_move((ghost.x, ghost.y), *d))
                .collect();
            if choices.is_empty() {
                choices = Dir::ALL.iter().copied().filter(|d| can_move((ghost.x, ghost.y), *d)).collect();
            }
            if choices.is_empty() {
                continue;
            }
            let chosen = if ghost.state == GhostState::Frightened {
                choices[rng.gen_range(0..choices.len())]
            } else {
                let target = self.ghost_target(&ghost);
                choices
                    .iter()
                    .copied()
                    .min_by_key(|d| {
                        let (dx, dy) = d.delta();
                        let next = wrap(ghost.x + dx, ghost.y + dy);
                        manhattan(next, target)
                    })
                    .unwrap_or(ghost.dir)
            };
            let (dx, dy) = chosen.delta();
            let next = wrap(ghost.x + dx, ghost.y + dy);
            let g = &mut self.ghosts[i];
            g.dir = chosen;
            g.x = next.0;
            g.y = next.1;
            if g.state == GhostState::Eyes && (g.x, g.y) == g.spawn {
                g.state = GhostState::Normal;
                g.dormant = 1.0;
            }
        }
        self.resolve_collisions();
    }

    fn resolve_collisions(&mut self) {
        if self.phase != Phase::Playing {
            return;
        }
        for i in 0..self.ghosts.len() {
            let ghost = self.ghosts[i];
            if (ghost.x, ghost.y) != self.pac {
                continue;
            }
            match ghost.state {
                GhostState::Frightened => {
                    self.catch_chain += 1;
                    let points = 200 * (1 << (self.catch_chain - 1).min(3));
                    self.score += points;
                    self.popups.add(ghost.x as f32, ghost.y as f32, format!("{}", points), (120, 220, 255));
                    self.particles.burst(ghost.x as f32, ghost.y as f32, 14, ghost.color, 8.0, &['*', '+', '·']);
                    self.ghosts[i].state = GhostState::Eyes;
                }
                GhostState::Eyes => {}
                GhostState::Normal => {
                    self.particles.burst(self.pac.0 as f32, self.pac.1 as f32, 16, (255, 232, 92), 6.0, &['*', '·', '+']);
                    self.phase = Phase::Dying(1.4);
                    return;
                }
            }
        }
    }
}

impl Game for PacManGame {
    fn kind(&self) -> GameKind {
        GameKind::PacMan
    }

    fn score(&self) -> u32 {
        self.score
    }

    fn is_over(&self) -> bool {
        self.phase == Phase::GameOver
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        self.desired_dir = match key.code {
            KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') => Dir::Up,
            KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => Dir::Right,
            KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => Dir::Down,
            KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('A') => Dir::Left,
            _ => return false,
        };
        if let Phase::Ready(_) = self.phase {
            self.phase = Phase::Playing;
        }
        true
    }

    fn tick(&mut self, dt: f32) {
        self.elapsed += dt;
        self.particles.tick(dt, 0.0);
        self.popups.tick(dt);

        match self.phase {
            Phase::Ready(t) => {
                if t - dt <= 0.0 {
                    self.phase = Phase::Playing;
                } else {
                    self.phase = Phase::Ready(t - dt);
                }
                return;
            }
            Phase::Dying(t) => {
                if t - dt <= 0.0 {
                    if self.lives > 1 {
                        self.lives -= 1;
                        self.reset_round();
                    } else {
                        self.lives = 0;
                        self.phase = Phase::GameOver;
                    }
                } else {
                    self.phase = Phase::Dying(t - dt);
                }
                return;
            }
            Phase::LevelClear(t) => {
                if t - dt <= 0.0 {
                    self.level += 1;
                    self.score += 250;
                    self.reset_level();
                } else {
                    self.phase = Phase::LevelClear(t - dt);
                }
                return;
            }
            Phase::GameOver => return,
            Phase::Playing => {}
        }

        for ghost in &mut self.ghosts {
            ghost.dormant = (ghost.dormant - dt).max(0.0);
        }

        self.mode_timer += dt;
        let mode_len = if self.scatter_mode { 7.0 } else { 20.0 };
        if self.mode_timer >= mode_len {
            self.scatter_mode = !self.scatter_mode;
            self.mode_timer = 0.0;
        }

        if self.frightened_timer > 0.0 {
            self.frightened_timer -= dt;
            if self.frightened_timer <= 0.0 {
                self.frightened_timer = 0.0;
                for ghost in &mut self.ghosts {
                    if ghost.state == GhostState::Frightened {
                        ghost.state = GhostState::Normal;
                    }
                }
            }
        }
        self.fruit_timer = (self.fruit_timer - dt).max(0.0);

        self.pac_timer += dt;
        let pac_step = self.pac_step_time();
        while self.pac_timer >= pac_step && self.phase == Phase::Playing {
            self.pac_timer -= pac_step;
            self.step_pac();
        }

        self.ghost_timer += dt;
        let ghost_step = if self.frightened_timer > 0.0 {
            self.ghost_step_time() * 1.35
        } else {
            self.ghost_step_time()
        };
        while self.ghost_timer >= ghost_step && self.phase == Phase::Playing {
            self.ghost_timer -= ghost_step;
            self.step_ghosts(false);
            // Eyes move twice as fast.
            self.step_ghosts(true);
            self.step_ghosts(true);
        }
    }

    fn render(&self, grid: &mut CellGrid, phase: f32) {
        let bg = t().panel_bg;
        let hud_rows = 2;
        let field_h = grid.height as i32 - hud_rows;
        let exact = grid.width as i32 >= W && field_h >= H;
        let cw = if grid.width as i32 >= W * 2 { 2 } else { 1 };

        // Cell mapping for a tile.
        let (off_x, off_y) = if exact {
            ((grid.width as i32 - W * cw) / 2, hud_rows + (field_h - H) / 2)
        } else {
            (0, hud_rows)
        };
        let sx = if exact { cw as f32 } else { grid.width as f32 / W as f32 };
        let sy = if exact { 1.0 } else { field_h as f32 / H as f32 };
        let map = |x: f32, y: f32| -> (i32, i32) { (off_x + (x * sx).round() as i32, off_y + (y * sy).round() as i32) };

        // HUD
        let hearts: String = "♥".repeat(self.lives as usize);
        grid.text(0, 0, &format!("SCORE {:06}   LVL {:>2}   {}", self.score, self.level, hearts), t().accent2, bg);
        let status = match self.phase {
            Phase::GameOver => "GAME OVER  //  R restart  Esc menu".to_string(),
            Phase::LevelClear(_) => "LEVEL CLEAR!".to_string(),
            Phase::Ready(_) => "READY!  WASD / arrows to move".to_string(),
            _ if self.frightened_timer > 0.0 => format!("POWER MODE {:.0}s  eat the ghosts!", self.frightened_timer.ceil()),
            _ if self.scatter_mode => "ghosts scattering".to_string(),
            _ => "ghosts hunting".to_string(),
        };
        grid.text(0, 1, &status, t().accent4, bg);

        // Walls and pellets.
        let flash = matches!(self.phase, Phase::LevelClear(_)) && ((phase * 6.0) as i32) % 2 == 0;
        let wall_color = if flash { rgb(240, 240, 255) } else { t().accent4 };
        let wall_dim = if flash { rgb(120, 120, 140) } else { dim(color_to_rgb(t().accent4), 0.55) };
        let power_on = ((phase * 4.0) as i32) % 2 == 0;
        let (pellet_color, power_color) = (t().accent1, t().accent2);

        if exact {
            for y in 0..H {
                for x in 0..W {
                    let (cx, cy) = map(x as f32, y as f32);
                    match tile(x, y) {
                        '#' => {
                            grid.set(cx, cy, wall_glyph(x, y), wall_color, bg);
                            if cw == 2 {
                                let ext = if is_wall(x + 1, y) { '─' } else { ' ' };
                                grid.set(cx + 1, cy, ext, wall_color, bg);
                            }
                        }
                        'G' => {
                            grid.set(cx, cy, '═', wall_dim, bg);
                            if cw == 2 {
                                grid.set(cx + 1, cy, '═', wall_dim, bg);
                            }
                        }
                        _ => {
                            if self.power_pellets.contains(&(x, y)) {
                                grid.set(cx, cy, if power_on { '●' } else { '○' }, power_color, bg);
                            } else if self.pellets.contains(&(x, y)) {
                                grid.set(cx, cy, '·', pellet_color, bg);
                            }
                        }
                    }
                }
            }
        } else {
            // Compressed fallback for small tiles: sample the nearest tile.
            for cy in 0..field_h {
                let ty = ((cy as f32 / sy) as i32).min(H - 1);
                for cx in 0..grid.width as i32 {
                    let tx = ((cx as f32 / sx) as i32).min(W - 1);
                    let (ch, fg) = match tile(tx, ty) {
                        '#' => (wall_glyph(tx, ty), wall_color),
                        'G' => ('═', wall_dim),
                        _ if self.power_pellets.contains(&(tx, ty)) => ('●', power_color),
                        _ if self.pellets.contains(&(tx, ty)) => ('·', pellet_color),
                        _ => (' ', bg),
                    };
                    grid.set(cx, cy + hud_rows, ch, fg, bg);
                }
            }
        }

        // Fruit
        if self.fruit_timer > 0.0 && self.phase == Phase::Playing {
            let (fx, fy) = map(FRUIT_TILE.0 as f32, FRUIT_TILE.1 as f32);
            grid.set(fx, fy, '♦', rgb(255, 120, 220), bg);
        }

        // Ghosts
        let blink_end = self.frightened_timer > 0.0 && self.frightened_timer < 2.0 && ((phase * 8.0) as i32) % 2 == 0;
        for ghost in &self.ghosts {
            let (gx, gy) = map(ghost.x as f32, ghost.y as f32);
            let (ch, color) = match ghost.state {
                GhostState::Eyes => ('"', rgb(200, 220, 255)),
                GhostState::Frightened if blink_end => ('ᗣ', rgb(240, 240, 240)),
                GhostState::Frightened => ('ᗣ', rgb(70, 110, 255)),
                GhostState::Normal => ('ᗣ', rgb(ghost.color.0, ghost.color.1, ghost.color.2)),
            };
            grid.set(gx, gy, ch, color, bg);
        }

        // Pac-Man
        let (px, py) = map(self.pac.0 as f32, self.pac.1 as f32);
        let pac_color = rgb(255, 232, 92);
        let pac_ch = match self.phase {
            Phase::Dying(tl) => {
                let frames = ['ᗧ', 'O', 'o', '·', '.', ' '];
                let idx = (((1.4 - tl) / 1.4) * frames.len() as f32) as usize;
                frames[idx.min(frames.len() - 1)]
            }
            _ if !self.mouth_open => 'O',
            _ => match self.pac_dir {
                Dir::Right => 'ᗧ',
                Dir::Left => 'ᗤ',
                Dir::Up => 'ᗢ',
                Dir::Down => 'ᗥ',
            },
        };
        grid.set(px, py, pac_ch, pac_color, bg);

        self.particles.draw(grid, map);
        self.popups.draw(grid, map);

        match self.phase {
            Phase::Ready(_) => {
                let (rx, ry) = map(14.0, 15.0);
                grid.text(rx - 2, ry, "READY!", t().accent2, bg);
            }
            Phase::GameOver => {
                let mid = grid.height as i32 / 2;
                grid.banner(mid, "GAME OVER", t().danger, t().bg_base);
            }
            Phase::LevelClear(_) => {
                let mid = grid.height as i32 / 2;
                grid.banner(mid, &format!("LEVEL {} CLEAR", self.level), t().accent2, t().bg_base);
            }
            _ => {}
        }
    }
}

fn tile(x: i32, y: i32) -> char {
    MAZE.get(y as usize)
        .and_then(|row| row.as_bytes().get(x as usize))
        .copied()
        .unwrap_or(b' ') as char
}

fn is_wall(x: i32, y: i32) -> bool {
    x >= 0 && y >= 0 && x < W && y < H && tile(x, y) == '#'
}

/// Box-drawing glyph for a wall tile based on which neighbours are walls.
fn wall_glyph(x: i32, y: i32) -> char {
    let n = is_wall(x, y - 1);
    let e = is_wall(x + 1, y);
    let s = is_wall(x, y + 1);
    let w = is_wall(x - 1, y);
    match (n, e, s, w) {
        (false, false, false, false) => '□',
        (true, false, false, false) => '╵',
        (false, true, false, false) => '╶',
        (false, false, true, false) => '╷',
        (false, false, false, true) => '╴',
        (true, false, true, false) => '│',
        (false, true, false, true) => '─',
        (true, true, false, false) => '╰',
        (true, false, false, true) => '╯',
        (false, true, true, false) => '╭',
        (false, false, true, true) => '╮',
        (true, true, true, false) => '├',
        (true, false, true, true) => '┤',
        (true, true, false, true) => '┴',
        (false, true, true, true) => '┬',
        (true, true, true, true) => '┼',
    }
}

fn can_move(pos: (i32, i32), dir: Dir) -> bool {
    let (dx, dy) = dir.delta();
    let next = wrap(pos.0 + dx, pos.1 + dy);
    if !(0..W).contains(&next.0) || !(0..H).contains(&next.1) {
        return false;
    }
    tile(next.0, next.1) != '#'
}

fn wrap(x: i32, y: i32) -> (i32, i32) {
    if (0..H).contains(&y) && tile(0, y) == ' ' && tile(W - 1, y) == ' ' {
        if x < 0 {
            return (W - 1, y);
        }
        if x >= W {
            return (0, y);
        }
    }
    (x, y)
}

fn manhattan(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.0 - b.0).abs() + (a.1 - b.1).abs()
}
