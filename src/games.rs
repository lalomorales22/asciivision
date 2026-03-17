use crossterm::event::{KeyCode, KeyEvent};
use rand::{prelude::SliceRandom, Rng};
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};
use std::{collections::HashSet, f32::consts::PI};

use crate::theme::t;

const PAC_MAZE: [&str; 21] = [
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

const PAC_W: i32 = 31;
const PAC_H: i32 = 21;
const SPACE_W: f32 = 40.0;
const SPACE_H: f32 = 24.0;
const PENGUIN_WORLD_W: f32 = 120.0;
const PENGUIN_WORLD_H: f32 = 90.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameKind {
    PacMan,
    SpaceInvaders,
    Penguin3D,
}

impl GameKind {
    pub const ALL: [Self; 3] = [Self::PacMan, Self::SpaceInvaders, Self::Penguin3D];

    pub fn index(self) -> usize {
        match self {
            Self::PacMan => 0,
            Self::SpaceInvaders => 1,
            Self::Penguin3D => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PacMan => "PAC-MAN",
            Self::SpaceInvaders => "SPACE INVADERS",
            Self::Penguin3D => "3D PENGUIN",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            Self::PacMan => "maze pellets, ghosts, power mode",
            Self::SpaceInvaders => "retro lane shooter with shield pulse",
            Self::Penguin3D => "snow-run fish collector with faux 3D view",
        }
    }

    pub fn from_input(input: &str) -> Option<Self> {
        match input.trim().to_lowercase().as_str() {
            "1" | "pac" | "pacman" | "pac-man" => Some(Self::PacMan),
            "2" | "space" | "invaders" | "space-invaders" | "space invaders" => {
                Some(Self::SpaceInvaders)
            }
            "3" | "penguin" | "3d" | "3d-penguin" | "peng" => Some(Self::Penguin3D),
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

pub struct GamesPanel {
    selected: GameKind,
    session: Option<GameSession>,
    status: String,
}

impl GamesPanel {
    pub fn new() -> Self {
        Self {
            selected: GameKind::PacMan,
            session: None,
            status: "games bay online".to_string(),
        }
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
        self.session = Some(GameSession::new(self.selected));
        self.status = format!("games: {}", self.selected.label());
    }

    pub fn launch(&mut self, game: GameKind) {
        self.selected = game;
        self.activate_selected();
    }

    pub fn stop(&mut self) {
        self.session = None;
        self.status = "games: selector ready".to_string();
    }

    pub fn tick(&mut self, dt: f32) {
        if let Some(session) = &mut self.session {
            session.tick(dt.min(0.05));
            if session.is_finished() {
                self.status = format!("games: {} complete", session.kind().label());
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if let Some(game) = self.key_to_selection(key.code) {
            self.launch(game);
            return true;
        }

        if let Some(session) = &mut self.session {
            match key.code {
                KeyCode::Esc => {
                    self.stop();
                    return true;
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    session.restart();
                    self.status = format!("games: restarted {}", session.kind().label());
                    return true;
                }
                _ => {
                    if session.handle_key(key) {
                        self.status = format!("games: {}", session.kind().label());
                        return true;
                    }
                }
            }
            return false;
        }

        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.activate_selected();
                true
            }
            KeyCode::Up | KeyCode::Left => {
                self.previous_game();
                true
            }
            KeyCode::Down | KeyCode::Right => {
                self.next_game();
                true
            }
            KeyCode::Char('w') | KeyCode::Char('W') | KeyCode::Char('a') | KeyCode::Char('A') => {
                self.previous_game();
                true
            }
            KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Char('d') | KeyCode::Char('D') => {
                self.next_game();
                true
            }
            _ => false,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, phase: f32, is_focused: bool) {
        let title = if let Some(session) = &self.session {
            format!(" GAMES // {} ", session.kind().label())
        } else {
            " GAMES // SELECT ".to_string()
        };
        let border_color = if is_focused { t().accent4 } else { t().accent1 };
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(t().accent2).bold())
            .borders(Borders::ALL)
            .border_type(if is_focused {
                BorderType::Double
            } else {
                BorderType::Plain
            })
            .border_style(Style::default().fg(border_color));
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        if inner.width < 18 || inner.height < 8 {
            frame.render_widget(
                Paragraph::new("Grow this tile to play.")
                    .style(Style::default().fg(t().muted).bg(t().panel_bg))
                    .alignment(Alignment::Center),
                inner,
            );
            return;
        }

        if let Some(session) = &self.session {
            session.render(frame.buffer_mut(), inner);
        } else {
            self.render_menu(frame, inner, phase);
        }
    }

    fn render_menu(&self, frame: &mut Frame, area: Rect, phase: f32) {
        let spinner = ["|", "/", "-", "\\"][((phase * 7.0) as usize) % 4];
        let mut lines = vec![
            Line::from(vec![
                Span::styled(
                    format!("{} ARCADE BAY READY ", spinner),
                    Style::default().fg(t().accent4).bold(),
                ),
                Span::styled(
                    "Focus this tile, then use 1-3 or WASD to choose.",
                    Style::default().fg(t().text),
                ),
            ]),
            Line::from(""),
        ];

        for (idx, game) in GameKind::ALL.iter().enumerate() {
            let selected = *game == self.selected;
            let accent = if selected { t().accent2 } else { t().muted };
            let pointer = if selected { ">" } else { " " };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{} {}. {:<15}", pointer, idx + 1, game.label()),
                    Style::default().fg(accent).bold(),
                ),
                Span::styled(game.subtitle(), Style::default().fg(t().text)),
            ]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Enter/Space", Style::default().fg(t().accent4).bold()),
            Span::styled(" launch selected game", Style::default().fg(t().text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Active Tile", Style::default().fg(t().accent4).bold()),
            Span::styled(
                " WASD routes into the game only when this panel is focused and the prompt is empty",
                Style::default().fg(t().text),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Esc", Style::default().fg(t().accent4).bold()),
            Span::styled(" back to selector", Style::default().fg(t().text)),
        ]));

        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(Style::default().bg(t().panel_bg))
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    fn key_to_selection(&self, code: KeyCode) -> Option<GameKind> {
        match code {
            KeyCode::Char('1') => Some(GameKind::PacMan),
            KeyCode::Char('2') => Some(GameKind::SpaceInvaders),
            KeyCode::Char('3') => Some(GameKind::Penguin3D),
            _ => None,
        }
    }
}

enum GameSession {
    PacMan(PacManGame),
    SpaceInvaders(SpaceInvadersGame),
    Penguin3D(PenguinGame),
}

impl GameSession {
    fn new(kind: GameKind) -> Self {
        match kind {
            GameKind::PacMan => Self::PacMan(PacManGame::new()),
            GameKind::SpaceInvaders => Self::SpaceInvaders(SpaceInvadersGame::new()),
            GameKind::Penguin3D => Self::Penguin3D(PenguinGame::new()),
        }
    }

    fn kind(&self) -> GameKind {
        match self {
            Self::PacMan(_) => GameKind::PacMan,
            Self::SpaceInvaders(_) => GameKind::SpaceInvaders,
            Self::Penguin3D(_) => GameKind::Penguin3D,
        }
    }

    fn restart(&mut self) {
        *self = Self::new(self.kind());
    }

    fn tick(&mut self, dt: f32) {
        match self {
            Self::PacMan(game) => game.tick(dt),
            Self::SpaceInvaders(game) => game.tick(dt),
            Self::Penguin3D(game) => game.tick(dt),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        match self {
            Self::PacMan(game) => game.handle_key(key),
            Self::SpaceInvaders(game) => game.handle_key(key),
            Self::Penguin3D(game) => game.handle_key(key),
        }
    }

    fn render(&self, buffer: &mut Buffer, area: Rect) {
        match self {
            Self::PacMan(game) => game.render(buffer, area),
            Self::SpaceInvaders(game) => game.render(buffer, area),
            Self::Penguin3D(game) => game.render(buffer, area),
        }
    }

    fn is_finished(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Up,
    Right,
    Down,
    Left,
}

impl Dir {
    const ALL: [Self; 4] = [Self::Up, Self::Right, Self::Down, Self::Left];

    fn delta(self) -> (i32, i32) {
        match self {
            Self::Up => (0, -1),
            Self::Right => (1, 0),
            Self::Down => (0, 1),
            Self::Left => (-1, 0),
        }
    }

    fn opposite(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Right => Self::Left,
            Self::Down => Self::Up,
            Self::Left => Self::Right,
        }
    }
}

#[derive(Clone, Copy)]
struct Ghost {
    x: i32,
    y: i32,
    dir: Dir,
    spawn: (i32, i32),
    home: (i32, i32),
    color: Color,
}

struct PacManGame {
    pac: (i32, i32),
    pac_dir: Dir,
    desired_dir: Dir,
    ghosts: Vec<Ghost>,
    pellets: HashSet<(i32, i32)>,
    power_pellets: HashSet<(i32, i32)>,
    score: u32,
    lives: u8,
    level: u32,
    pac_timer: f32,
    ghost_timer: f32,
    mode_timer: f32,
    scatter_mode: bool,
    frightened_timer: f32,
    game_over: bool,
}

impl PacManGame {
    fn new() -> Self {
        let mut game = Self {
            pac: (14, 13),
            pac_dir: Dir::Right,
            desired_dir: Dir::Right,
            ghosts: vec![
                Ghost {
                    x: 13,
                    y: 11,
                    dir: Dir::Left,
                    spawn: (13, 11),
                    home: (PAC_W - 2, 1),
                    color: Color::Rgb(255, 110, 130),
                },
                Ghost {
                    x: 14,
                    y: 11,
                    dir: Dir::Left,
                    spawn: (14, 11),
                    home: (1, 1),
                    color: Color::Rgb(252, 186, 255),
                },
                Ghost {
                    x: 15,
                    y: 11,
                    dir: Dir::Left,
                    spawn: (15, 11),
                    home: (PAC_W - 2, PAC_H - 2),
                    color: Color::Rgb(70, 222, 210),
                },
                Ghost {
                    x: 14,
                    y: 10,
                    dir: Dir::Left,
                    spawn: (14, 10),
                    home: (1, PAC_H - 2),
                    color: Color::Rgb(255, 158, 82),
                },
            ],
            pellets: HashSet::new(),
            power_pellets: HashSet::new(),
            score: 0,
            lives: 3,
            level: 1,
            pac_timer: 0.0,
            ghost_timer: 0.0,
            mode_timer: 0.0,
            scatter_mode: true,
            frightened_timer: 0.0,
            game_over: false,
        };
        game.reset_level();
        game
    }

    fn reset_level(&mut self) {
        self.pellets.clear();
        self.power_pellets.clear();
        for y in 0..PAC_H {
            for x in 0..PAC_W {
                match pac_tile(x, y) {
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
        self.reset_round();
    }

    fn reset_round(&mut self) {
        self.pac = (14, 13);
        self.pac_dir = Dir::Right;
        self.desired_dir = Dir::Right;
        for ghost in &mut self.ghosts {
            ghost.x = ghost.spawn.0;
            ghost.y = ghost.spawn.1;
            ghost.dir = Dir::Left;
        }
        self.frightened_timer = 0.0;
        self.pac_timer = 0.0;
        self.ghost_timer = 0.0;
        self.mode_timer = 0.0;
        self.scatter_mode = true;
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        self.desired_dir = match key.code {
            KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') => Dir::Up,
            KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => Dir::Right,
            KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => Dir::Down,
            KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('A') => Dir::Left,
            _ => return false,
        };
        true
    }

    fn tick(&mut self, dt: f32) {
        if self.game_over {
            return;
        }

        self.mode_timer += dt;
        if self.mode_timer >= if self.scatter_mode { 7.0 } else { 18.0 } {
            self.scatter_mode = !self.scatter_mode;
            self.mode_timer = 0.0;
        }
        self.frightened_timer = (self.frightened_timer - dt).max(0.0);

        self.pac_timer += dt;
        while self.pac_timer >= 0.12 {
            self.pac_timer -= 0.12;
            self.step_pac();
        }

        self.ghost_timer += dt;
        let ghost_step = if self.frightened_timer > 0.0 { 0.2 } else { 0.16 };
        while self.ghost_timer >= ghost_step {
            self.ghost_timer -= ghost_step;
            self.step_ghosts();
        }
    }

    fn step_pac(&mut self) {
        if self.can_move(self.pac, self.desired_dir) {
            self.pac_dir = self.desired_dir;
        }
        if self.can_move(self.pac, self.pac_dir) {
            let (dx, dy) = self.pac_dir.delta();
            let next = wrap_pos(self.pac.0 + dx, self.pac.1 + dy);
            self.pac = next;
        }

        if self.pellets.remove(&self.pac) {
            self.score += 10;
        }
        if self.power_pellets.remove(&self.pac) {
            self.score += 50;
            self.frightened_timer = 8.0;
        }

        self.resolve_collisions();

        if self.pellets.is_empty() && self.power_pellets.is_empty() {
            self.level += 1;
            self.score += 250;
            self.reset_level();
        }
    }

    fn step_ghosts(&mut self) {
        let pac = self.pac;
        let scatter = self.scatter_mode;
        let frightened = self.frightened_timer > 0.0;
        let mut rng = rand::thread_rng();

        for ghost in &mut self.ghosts {
            let mut choices: Vec<Dir> = Dir::ALL
                .iter()
                .copied()
                .filter(|dir| *dir != ghost.dir.opposite() && can_move_from((ghost.x, ghost.y), *dir))
                .collect();
            if choices.is_empty() {
                choices = Dir::ALL
                    .iter()
                    .copied()
                    .filter(|dir| can_move_from((ghost.x, ghost.y), *dir))
                    .collect();
            }
            if choices.is_empty() {
                continue;
            }

            let target = if frightened {
                pac
            } else if scatter {
                ghost.home
            } else {
                pac
            };

            let chosen = if frightened {
                choices
                    .iter()
                    .copied()
                    .max_by_key(|dir| {
                        let (dx, dy) = dir.delta();
                        let next = wrap_pos(ghost.x + dx, ghost.y + dy);
                        manhattan(next, target)
                    })
                    .unwrap_or(ghost.dir)
            } else if rng.gen_bool(0.22) {
                *choices.choose(&mut rng).unwrap_or(&ghost.dir)
            } else {
                choices
                    .iter()
                    .copied()
                    .min_by_key(|dir| {
                        let (dx, dy) = dir.delta();
                        let next = wrap_pos(ghost.x + dx, ghost.y + dy);
                        manhattan(next, target)
                    })
                    .unwrap_or(ghost.dir)
            };

            ghost.dir = chosen;
            let (dx, dy) = chosen.delta();
            let next = wrap_pos(ghost.x + dx, ghost.y + dy);
            ghost.x = next.0;
            ghost.y = next.1;
        }

        self.resolve_collisions();
    }

    fn resolve_collisions(&mut self) {
        for ghost in &mut self.ghosts {
            if (ghost.x, ghost.y) == self.pac {
                if self.frightened_timer > 0.0 {
                    self.score += 200;
                    ghost.x = ghost.spawn.0;
                    ghost.y = ghost.spawn.1;
                    ghost.dir = Dir::Left;
                } else if self.lives > 1 {
                    self.lives -= 1;
                    self.reset_round();
                    return;
                } else {
                    self.lives = 0;
                    self.game_over = true;
                    return;
                }
            }
        }
    }

    fn can_move(&self, pos: (i32, i32), dir: Dir) -> bool {
        can_move_from(pos, dir)
    }

    fn render(&self, buffer: &mut Buffer, area: Rect) {
        let mut grid = CellGrid::new(area.width, area.height, t().panel_bg, t().text);
        if area.height < 6 {
            grid.center_text(0, "PAC-MAN", t().accent2, t().panel_bg);
            grid.center_text(2, "Grow this tile to play.", t().muted, t().panel_bg);
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
            t().accent2,
            t().panel_bg,
        );
        let status = if self.game_over {
            "R restart  Esc menu"
        } else if self.frightened_timer > 0.0 {
            "WASD move  ghosts frightened"
        } else {
            "WASD move  1-3 switch games"
        };
        grid.text(0, 1, status, t().accent4, t().panel_bg);

        let game_top = 2u16;
        let game_h = area.height.saturating_sub(game_top);
        let game_w = area.width;

        for sy in 0..game_h {
            let src_y = (sy as i32 * PAC_H) / game_h.max(1) as i32;
            for sx in 0..game_w {
                let src_x = (sx as i32 * PAC_W) / game_w.max(1) as i32;
                let tile = pac_tile(src_x, src_y);
                let (ch, fg, bg) = if tile == '#' {
                    ('#', t().accent4, t().panel_alt)
                } else if self.power_pellets.contains(&(src_x, src_y)) {
                    ('o', t().accent2, t().panel_bg)
                } else if self.pellets.contains(&(src_x, src_y)) {
                    ('.', t().accent1, t().panel_bg)
                } else {
                    (' ', t().text, t().panel_bg)
                };
                grid.set(sx as i32, sy as i32 + game_top as i32, ch, fg, bg);
            }
        }

        let pac_x = project_axis(self.pac.0 as f32, PAC_W as f32, game_w);
        let pac_y = project_axis(self.pac.1 as f32, PAC_H as f32, game_h) + game_top as i32;
        grid.set(pac_x, pac_y, 'C', Color::Rgb(255, 232, 92), t().panel_bg);

        for ghost in &self.ghosts {
            let gx = project_axis(ghost.x as f32, PAC_W as f32, game_w);
            let gy = project_axis(ghost.y as f32, PAC_H as f32, game_h) + game_top as i32;
            let color = if self.frightened_timer > 0.0 {
                Color::Rgb(80, 180, 255)
            } else {
                ghost.color
            };
            grid.set(gx, gy, 'G', color, t().panel_bg);
        }

        if self.game_over {
            let y = area.height.saturating_sub(2) as i32;
            grid.center_text(y - 1, "GAME OVER", t().danger, t().panel_bg);
            grid.center_text(y, "Press R to restart or Esc to return", t().text, t().panel_bg);
        }

        grid.present(buffer, area);
    }
}

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

struct SpaceInvadersGame {
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
    fn new() -> Self {
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

    fn render(&self, buffer: &mut Buffer, area: Rect) {
        let mut grid = CellGrid::new(area.width, area.height, t().panel_bg, t().text);
        grid.text(
            0,
            0,
            &format!(
                "wave {}  score {:05}  lives {}",
                self.wave, self.score, self.lives
            ),
            t().accent2,
            t().panel_bg,
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
            t().accent4,
            t().panel_bg,
        );

        let game_top = 2u16;
        let game_h = area.height.saturating_sub(game_top);
        for star in &self.stars {
            let sx = project_axis(star.x, SPACE_W, area.width);
            let sy = project_axis(star.y, SPACE_H, game_h) + game_top as i32;
            grid.set(sx, sy, '.', t().muted, t().panel_bg);
        }

        for invader in self.invaders.iter().filter(|inv| inv.alive) {
            let x = project_axis(invader.x, SPACE_W, area.width);
            let y = project_axis(invader.y, SPACE_H, game_h) + game_top as i32;
            let ch = if ((invader.x + invader.y) as i32) % 2 == 0 {
                'W'
            } else {
                'M'
            };
            grid.set(x, y, ch, t().accent1, t().panel_bg);
        }

        for bullet in &self.bullets {
            let x = project_axis(bullet.x, SPACE_W, area.width);
            let y = project_axis(bullet.y, SPACE_H, game_h) + game_top as i32;
            let color = if bullet.friendly { t().accent4 } else { t().danger };
            grid.set(x, y, '|', color, t().panel_bg);
        }

        let player_x = project_axis(self.player_x, SPACE_W, area.width);
        let player_y = game_top as i32 + game_h.saturating_sub(1) as i32;
        grid.set(player_x, player_y, 'A', t().accent3, t().panel_bg);
        if self.shield_timer > 0.0 {
            grid.set(player_x - 1, player_y, '(', t().accent4, t().panel_bg);
            grid.set(player_x + 1, player_y, ')', t().accent4, t().panel_bg);
        }

        if self.game_over {
            let mid = area.height as i32 / 2;
            grid.center_text(mid - 1, "DEFENSE LINE COLLAPSED", t().danger, t().panel_bg);
            grid.center_text(mid, "Press R to restart or Esc for menu", t().text, t().panel_bg);
        }

        grid.present(buffer, area);
    }
}

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

struct PenguinGame {
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
    fn new() -> Self {
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

#[derive(Clone, Copy)]
struct GridCell {
    ch: char,
    fg: Color,
    bg: Color,
}

struct CellGrid {
    width: u16,
    height: u16,
    cells: Vec<GridCell>,
}

impl CellGrid {
    fn new(width: u16, height: u16, bg: Color, fg: Color) -> Self {
        Self {
            width,
            height,
            cells: vec![
                GridCell {
                    ch: ' ',
                    fg,
                    bg,
                };
                width as usize * height as usize
            ],
        }
    }

    fn set(&mut self, x: i32, y: i32, ch: char, fg: Color, bg: Color) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let idx = y as usize * self.width as usize + x as usize;
        if let Some(cell) = self.cells.get_mut(idx) {
            *cell = GridCell { ch, fg, bg };
        }
    }

    fn text(&mut self, x: i32, y: i32, text: &str, fg: Color, bg: Color) {
        for (i, ch) in text.chars().enumerate() {
            self.set(x + i as i32, y, ch, fg, bg);
        }
    }

    fn center_text(&mut self, y: i32, text: &str, fg: Color, bg: Color) {
        let width = text.chars().count() as i32;
        let x = ((self.width as i32 - width) / 2).max(0);
        self.text(x, y, text, fg, bg);
    }

    fn present(&self, buffer: &mut Buffer, area: Rect) {
        for y in 0..self.height {
            for x in 0..self.width {
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

fn pac_tile(x: i32, y: i32) -> char {
    PAC_MAZE
        .get(y as usize)
        .and_then(|row| row.as_bytes().get(x as usize))
        .copied()
        .unwrap_or(b'#') as char
}

fn can_move_from(pos: (i32, i32), dir: Dir) -> bool {
    let (dx, dy) = dir.delta();
    let next = wrap_pos(pos.0 + dx, pos.1 + dy);
    if next.0 < 0 || next.1 < 0 || next.0 >= PAC_W || next.1 >= PAC_H {
        return false;
    }
    pac_tile(next.0, next.1) != '#'
}

fn wrap_pos(x: i32, y: i32) -> (i32, i32) {
    if y >= 0 && y < PAC_H && pac_tile(0, y) == ' ' && pac_tile(PAC_W - 1, y) == ' ' {
        if x < 0 {
            return (PAC_W - 1, y);
        }
        if x >= PAC_W {
            return (0, y);
        }
    }
    (x, y)
}

fn manhattan(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.0 - b.0).abs() + (a.1 - b.1).abs()
}

fn project_axis(value: f32, src_max: f32, dst: u16) -> i32 {
    if dst == 0 {
        return 0;
    }
    let scaled = (value / src_max.max(1.0)) * dst.saturating_sub(1) as f32;
    scaled.round() as i32
}
