//! Shared first-person raycaster used by Ice Caves and Ice Arena.
//!
//! DDA rays against a boolean wall grid, a per-column depth buffer, distance
//! fog, side shading, and depth-sorted billboard sprites with multi-row
//! glyph art chosen by apparent size.

use super::{rgb, CellGrid};

#[derive(Clone, Debug)]
pub struct Map {
    pub w: usize,
    pub h: usize,
    cells: Vec<bool>,
}

impl Map {
    pub fn filled(w: usize, h: usize) -> Self {
        Self { w, h, cells: vec![true; w * h] }
    }

    /// `#` is a wall, anything else is open.
    pub fn from_rows(rows: &[&str]) -> Self {
        let h = rows.len();
        let w = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
        let mut map = Self::filled(w, h);
        for (y, row) in rows.iter().enumerate() {
            for (x, ch) in row.chars().enumerate() {
                map.cells[y * w + x] = ch == '#';
            }
        }
        map
    }

    pub fn is_wall(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return true;
        }
        self.cells[y as usize * self.w + x as usize]
    }

    pub fn wall_at(&self, x: f32, y: f32) -> bool {
        if x < 0.0 || y < 0.0 {
            return true;
        }
        self.is_wall(x as i32, y as i32)
    }

    pub fn set(&mut self, x: usize, y: usize, wall: bool) {
        if x < self.w && y < self.h {
            self.cells[y * self.w + x] = wall;
        }
    }

    pub fn open_cells(&self) -> Vec<(usize, usize)> {
        (0..self.h)
            .flat_map(|y| (0..self.w).map(move |x| (x, y)))
            .filter(|&(x, y)| !self.cells[y * self.w + x])
            .collect()
    }

    /// Slide-along-walls movement for a circle of radius `r`.
    pub fn try_move(&self, px: &mut f32, py: &mut f32, dx: f32, dy: f32, r: f32) {
        let nx = *px + dx;
        if !self.wall_at(nx + r * dx.signum(), *py - r) && !self.wall_at(nx + r * dx.signum(), *py + r) {
            *px = nx;
        }
        let ny = *py + dy;
        if !self.wall_at(*px - r, ny + r * dy.signum()) && !self.wall_at(*px + r, ny + r * dy.signum()) {
            *py = ny;
        }
    }

    /// DDA raycast. Returns (distance along the ray, hit a vertical face).
    pub fn cast(&self, px: f32, py: f32, angle: f32) -> (f32, bool) {
        let (dir_x, dir_y) = (angle.cos(), angle.sin());
        let mut map_x = px as i32;
        let mut map_y = py as i32;
        let delta_x = if dir_x.abs() < 1e-6 { 1e6 } else { (1.0 / dir_x).abs() };
        let delta_y = if dir_y.abs() < 1e-6 { 1e6 } else { (1.0 / dir_y).abs() };
        let (step_x, mut side_x) = if dir_x < 0.0 {
            (-1, (px - map_x as f32) * delta_x)
        } else {
            (1, (map_x as f32 + 1.0 - px) * delta_x)
        };
        let (step_y, mut side_y) = if dir_y < 0.0 {
            (-1, (py - map_y as f32) * delta_y)
        } else {
            (1, (map_y as f32 + 1.0 - py) * delta_y)
        };
        for _ in 0..96 {
            let vertical = if side_x < side_y {
                side_x += delta_x;
                map_x += step_x;
                true
            } else {
                side_y += delta_y;
                map_y += step_y;
                false
            };
            if self.is_wall(map_x, map_y) {
                let dist = if vertical { side_x - delta_x } else { side_y - delta_y };
                return (dist.max(0.05), vertical);
            }
        }
        (48.0, false)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub sky_top: (u8, u8, u8),
    pub sky_bot: (u8, u8, u8),
    pub floor_far: (u8, u8, u8),
    pub floor_near: (u8, u8, u8),
    pub wall_v: (u8, u8, u8),
    pub wall_h: (u8, u8, u8),
    pub fog_range: f32,
}

impl Palette {
    pub fn ice() -> Self {
        Self {
            sky_top: (10, 18, 40),
            sky_bot: (30, 50, 90),
            floor_far: (60, 90, 120),
            floor_near: (170, 215, 235),
            wall_v: (120, 175, 225),
            wall_h: (165, 210, 240),
            fog_range: 14.0,
        }
    }

    pub fn arena() -> Self {
        Self {
            sky_top: (14, 10, 36),
            sky_bot: (60, 30, 90),
            floor_far: (50, 70, 110),
            floor_near: (150, 200, 230),
            wall_v: (110, 160, 230),
            wall_h: (150, 200, 250),
            fog_range: 18.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpriteKind {
    Fish,
    Seal,
    Penguin,
    Snowball,
}

#[derive(Clone, Debug)]
pub struct Sprite {
    pub x: f32,
    pub y: f32,
    pub kind: SpriteKind,
    pub color: (u8, u8, u8),
    pub label: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub x: f32,
    pub y: f32,
    pub angle: f32,
    /// Vertical bob offset in rows.
    pub bob: i32,
    pub fov: f32,
}

/// Glyph rows for a sprite at an apparent height of `size` rows, bottom row
/// last.
fn sprite_rows(kind: SpriteKind, size: i32) -> Vec<&'static str> {
    match kind {
        SpriteKind::Fish => match size {
            s if s >= 9 => vec!["~  ~  ~", "><(((°>"],
            s if s >= 5 => vec!["><((°>"],
            s if s >= 3 => vec!["><°>"],
            s if s >= 2 => vec!["><>"],
            _ => vec!["<"],
        },
        SpriteKind::Seal => match size {
            s if s >= 8 => vec!["(ó.ò)", "/‾‾‾\\"],
            s if s >= 4 => vec!["(oo)"],
            s if s >= 2 => vec!["oo"],
            _ => vec!["o"],
        },
        SpriteKind::Penguin => match size {
            s if s >= 12 => vec![" (●▾●) ", "/▓▓▓▓▓\\", "|▓▓▓▓▓|", " ▀   ▀ "],
            s if s >= 8 => vec!["(●▾●)", "/▓▓▓\\", " ▀ ▀ "],
            s if s >= 5 => vec!["(o o)", "/███\\"],
            s if s >= 3 => vec!["(oo)", "/██\\"],
            s if s >= 2 => vec!["oo", "██"],
            _ => vec!["o"],
        },
        SpriteKind::Snowball => match size {
            s if s >= 7 => vec!["●"],
            s if s >= 3 => vec!["o"],
            _ => vec!["·"],
        },
    }
}

/// Draw the first-person view into `grid` starting at row `top`. Returns
/// the per-column depth buffer.
#[allow(clippy::too_many_arguments)]
pub fn render_view(
    grid: &mut CellGrid,
    map: &Map,
    cam: &Camera,
    palette: &Palette,
    tint: Option<(u8, u8, u8)>,
    sprites: &[Sprite],
    snow: &[(f32, f32, bool)],
    top: i32,
) -> Vec<f32> {
    let w = grid.width as i32;
    let bottom = grid.height as i32;
    let view_h = bottom - top;
    if view_h < 3 || w < 4 {
        return Vec::new();
    }
    let horizon = top + view_h / 2;

    // Sky and floor gradients.
    for y in top..bottom {
        let color = if y < horizon {
            let k = (y - top) as f32 / (horizon - top).max(1) as f32;
            lerp(palette.sky_top, palette.sky_bot, k)
        } else {
            let k = (y - horizon) as f32 / (bottom - horizon).max(1) as f32;
            lerp(palette.floor_far, palette.floor_near, k)
        };
        let c = rgb(color.0, color.1, color.2);
        for x in 0..w {
            grid.set(x, y, ' ', c, c);
        }
    }
    for &(fx, fy, bright) in snow {
        let x = (fx * w as f32) as i32;
        let y = top + (fy * (horizon - top).max(1) as f32) as i32;
        grid.put(x, y, if bright { '*' } else { '·' }, rgb(230, 240, 250));
    }

    // Walls.
    let mut depth = vec![f32::INFINITY; w as usize];
    let dark = (8u8, 14u8, 30u8);
    for x in 0..w {
        let screen = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
        let ray = cam.angle + screen * cam.fov / 2.0;
        let (dist, vertical) = map.cast(cam.x, cam.y, ray);
        let perp = dist * (ray - cam.angle).cos();
        depth[x as usize] = perp;
        let height = ((view_h as f32 * 1.1) / perp.max(0.3)).min(view_h as f32 * 3.0) as i32;
        let col_top = horizon - height / 2 + cam.bob;
        let col_bottom = horizon + height / 2 + cam.bob;
        let fog = (1.0 - perp / palette.fog_range).clamp(0.12, 1.0);
        let base = match tint {
            Some(t) => t,
            None if vertical => palette.wall_v,
            None => palette.wall_h,
        };
        let color = lerp(dark, base, fog);
        let ch = if perp < 1.6 {
            '█'
        } else if perp < 3.5 {
            '▓'
        } else if perp < 7.0 {
            '▒'
        } else {
            '░'
        };
        for y in col_top.max(top)..=col_bottom.min(bottom - 1) {
            let stripe = (y + x / 3) % 7 == 0 && perp < 5.0;
            let c = if stripe { lerp(color, (255, 255, 255), 0.18) } else { color };
            let cc = rgb(c.0, c.1, c.2);
            grid.set(x, y, ch, cc, cc);
        }
    }

    // Sprites, far to near, z-tested per column.
    struct Placed<'a> {
        dist: f32,
        screen_x: f32,
        sprite: &'a Sprite,
    }
    let (ca, sa) = (cam.angle.cos(), cam.angle.sin());
    let mut placed: Vec<Placed> = sprites
        .iter()
        .filter_map(|sp| {
            let (dx, dy) = (sp.x - cam.x, sp.y - cam.y);
            let forward = dx * ca + dy * sa;
            let right = -dx * sa + dy * ca;
            if forward <= 0.2 {
                return None;
            }
            let screen_x = (0.5 + (right / forward) / cam.fov) * w as f32;
            Some(Placed { dist: forward, screen_x, sprite: sp })
        })
        .collect();
    placed.sort_by(|a, b| b.dist.partial_cmp(&a.dist).unwrap_or(std::cmp::Ordering::Equal));

    for p in &placed {
        let size = (view_h as f32 * 0.9 / p.dist).clamp(1.0, view_h as f32) as i32;
        let rows = sprite_rows(p.sprite.kind, size);
        let fog = (1.0 - p.dist / palette.fog_range).clamp(0.2, 1.0);
        let color = lerp(dark, p.sprite.color, fog);
        let cc = rgb(color.0, color.1, color.2);
        let base_y = horizon + (size / 3).max(0) + cam.bob;
        let n = rows.len() as i32;
        for (i, row) in rows.iter().enumerate() {
            let y = base_y - (n - 1 - i as i32);
            let chars: Vec<char> = row.chars().collect();
            let half = chars.len() as i32 / 2;
            let x0 = p.screen_x.round() as i32 - half;
            // Edge padding is transparent; gaps inside the body are painted so
            // walls do not show through a sprite.
            let first = chars.iter().position(|c| *c != ' ').unwrap_or(0);
            let last = chars.iter().rposition(|c| *c != ' ').unwrap_or(0);
            for (j, ch) in chars.iter().enumerate() {
                let x = x0 + j as i32;
                if x < 0 || x >= w || j < first || j > last {
                    continue;
                }
                if depth[x as usize] > p.dist && y >= top && y < bottom {
                    grid.put(x, y, *ch, cc);
                }
            }
        }
        if let (Some(label), true) = (&p.sprite.label, size >= 3) {
            let y = base_y - n;
            let half = label.chars().count() as i32 / 2;
            let x0 = p.screen_x.round() as i32 - half;
            for (j, ch) in label.chars().enumerate() {
                let x = x0 + j as i32;
                if x >= 0 && x < w && y >= top && depth[x as usize] > p.dist {
                    grid.put(x, y, ch, rgb(240, 240, 250));
                }
            }
        }
    }

    depth
}

pub fn lerp(a: (u8, u8, u8), b: (u8, u8, u8), k: f32) -> (u8, u8, u8) {
    let k = k.clamp(0.0, 1.0);
    (
        (a.0 as f32 + (b.0 as f32 - a.0 as f32) * k) as u8,
        (a.1 as f32 + (b.1 as f32 - a.1 as f32) * k) as u8,
        (a.2 as f32 + (b.2 as f32 - a.2 as f32) * k) as u8,
    )
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cast_finds_the_wall_ahead() {
        let map = Map::from_rows(&["#####", "#...#", "#####"]);
        let (d, vertical) = map.cast(1.5, 1.5, 0.0);
        assert!((d - 2.5).abs() < 1e-4, "d = {}", d);
        assert!(vertical);
        let (d, vertical) = map.cast(1.5, 1.5, std::f32::consts::FRAC_PI_2);
        assert!((d - 0.5).abs() < 1e-4);
        assert!(!vertical);
    }

    #[test]
    fn penguin_sprite_lands_in_front_of_the_camera() {
        use crate::games::CellGrid;
        use ratatui::prelude::Color;
        let map = Map::from_rows(&[
            "########",
            "#......#",
            "#......#",
            "#......#",
            "########",
        ]);
        let mut grid = CellGrid::new(60, 24, Color::Black, Color::White);
        let cam = Camera { x: 1.5, y: 2.5, angle: 0.0, bob: 0, fov: 1.2 };
        let sprites = [Sprite { x: 4.5, y: 2.5, kind: SpriteKind::Penguin, color: (255, 220, 90), label: Some("bot".into()) }];
        render_view(&mut grid, &map, &cam, &Palette::arena(), None, &sprites, &[], 2);
        let text: String = (0..24)
            .map(|y| (0..60).map(|x| grid.get_char(x, y)).collect::<String>() + "\n")
            .collect();
        assert!(text.contains("(o o)") || text.contains("(●▾●)") || text.contains("(oo)"), "no penguin:\n{}", text);
        assert!(text.contains("bot"), "no label:\n{}", text);
    }

    #[test]
    fn try_move_slides_along_walls() {
        let map = Map::from_rows(&["#####", "#...#", "#####"]);
        let (mut x, mut y) = (1.5, 1.5);
        map.try_move(&mut x, &mut y, 0.0, 5.0, 0.2);
        assert!((y - 1.5).abs() < 1e-6, "blocked vertically");
        map.try_move(&mut x, &mut y, 1.0, 0.0, 0.2);
        assert!(x > 2.4);
    }
}
