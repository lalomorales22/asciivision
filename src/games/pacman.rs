//! Pac-Man: ported unchanged onto the `Game` trait.

use crossterm::event::{KeyCode, KeyEvent};
use rand::{prelude::SliceRandom, Rng};
use ratatui::prelude::*;
use std::collections::HashSet;

use super::{CellGrid, Dir, Game, project_axis};
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

#[derive(Clone, Copy)]
struct Ghost {
    x: i32,
    y: i32,
    dir: Dir,
    spawn: (i32, i32),
    home: (i32, i32),
    color: Color,
}

pub(super) struct PacManGame {
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
    pub(super) fn new() -> Self {
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
}

impl Game for PacManGame {
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
            "WASD move  1-7 switch games"
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
