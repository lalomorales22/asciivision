use rand::Rng;
use ratatui::prelude::*;

const MATRIX_CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789@#$%&*+=<>{}[]|/\\~";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectKind {
    MatrixRain,
    Plasma,
    Starfield,
    WireframeCube,
    Fire,
    Particles,
}

impl EffectKind {
    pub fn cycle(self) -> Self {
        match self {
            Self::MatrixRain => Self::Plasma,
            Self::Plasma => Self::Starfield,
            Self::Starfield => Self::WireframeCube,
            Self::WireframeCube => Self::Fire,
            Self::Fire => Self::Particles,
            Self::Particles => Self::MatrixRain,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::MatrixRain => "MATRIX RAIN",
            Self::Plasma => "PLASMA FIELD",
            Self::Starfield => "3D STARFIELD",
            Self::WireframeCube => "WIREFRAME 3D",
            Self::Fire => "FIRE SIM",
            Self::Particles => "PARTICLE STORM",
        }
    }
}

pub struct EffectsEngine {
    pub kind: EffectKind,
    pub active: bool,
    matrix_columns: Vec<MatrixColumn>,
    stars: Vec<Star>,
    particles: Vec<Particle>,
    fire_buf: Vec<Vec<f32>>,
    last_size: (u16, u16),
}

struct MatrixColumn {
    x: u16,
    y: f32,
    speed: f32,
    length: u16,
    chars: Vec<u8>,
    hue: f32,
}

struct Star {
    x: f32,
    y: f32,
    z: f32,
}

struct Particle {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    life: f32,
    color: (u8, u8, u8),
}

impl EffectsEngine {
    pub fn new() -> Self {
        Self {
            kind: EffectKind::MatrixRain,
            active: false,
            matrix_columns: Vec::new(),
            stars: Vec::new(),
            particles: Vec::new(),
            fire_buf: Vec::new(),
            last_size: (0, 0),
        }
    }

    pub fn cycle_with_off(&mut self) {
        if !self.active {
            self.active = true;
            self.reset_buffers();
            return;
        }

        if matches!(self.kind, EffectKind::Particles) {
            self.active = false;
            self.reset_buffers();
            return;
        }

        self.kind = self.kind.cycle();
        self.reset_buffers();
    }

    fn reset_buffers(&mut self) {
        self.matrix_columns.clear();
        self.stars.clear();
        self.particles.clear();
        self.fire_buf.clear();
        self.last_size = (0, 0);
    }

    fn ensure_init(&mut self, width: u16, height: u16) {
        if self.last_size == (width, height) && !self.needs_init() {
            return;
        }
        self.last_size = (width, height);
        let mut rng = rand::thread_rng();

        match self.kind {
            EffectKind::MatrixRain => {
                self.matrix_columns.clear();
                for x in 0..width {
                    if rng.gen_range(0..3) == 0 {
                        let length = rng.gen_range(4..height.max(5));
                        let mut chars = Vec::with_capacity(length as usize);
                        for _ in 0..length {
                            chars.push(MATRIX_CHARS[rng.gen_range(0..MATRIX_CHARS.len())]);
                        }
                        self.matrix_columns.push(MatrixColumn {
                            x,
                            y: -(rng.gen_range(0..height) as f32),
                            speed: rng.gen_range(0.3..1.8),
                            length,
                            chars,
                            hue: rng.gen_range(0.0..360.0),
                        });
                    }
                }
            }
            EffectKind::Starfield => {
                self.stars.clear();
                for _ in 0..200 {
                    self.stars.push(Star {
                        x: rng.gen_range(-1.0..1.0),
                        y: rng.gen_range(-1.0..1.0),
                        z: rng.gen_range(0.1..1.0),
                    });
                }
            }
            EffectKind::Particles => {
                self.particles.clear();
                for _ in 0..120 {
                    self.particles.push(new_particle(&mut rng, width, height));
                }
            }
            EffectKind::Fire => {
                self.fire_buf = vec![vec![0.0; width as usize]; height as usize];
            }
            _ => {}
        }
    }

    fn needs_init(&self) -> bool {
        match self.kind {
            EffectKind::MatrixRain => self.matrix_columns.is_empty(),
            EffectKind::Starfield => self.stars.is_empty(),
            EffectKind::Particles => self.particles.is_empty(),
            EffectKind::Fire => self.fire_buf.is_empty(),
            _ => false,
        }
    }

    pub fn render(&mut self, buffer: &mut Buffer, area: Rect, phase: f32) {
        if !self.active || area.width < 4 || area.height < 4 {
            return;
        }
        self.ensure_init(area.width, area.height);

        match self.kind {
            EffectKind::MatrixRain => self.render_matrix(buffer, area, phase),
            EffectKind::Plasma => render_plasma(buffer, area, phase),
            EffectKind::Starfield => self.render_starfield(buffer, area, phase),
            EffectKind::WireframeCube => render_wireframe_cube(buffer, area, phase),
            EffectKind::Fire => self.render_fire(buffer, area),
            EffectKind::Particles => self.render_particles(buffer, area),
        }
    }

    fn render_matrix(&mut self, buffer: &mut Buffer, area: Rect, phase: f32) {
        let mut rng = rand::thread_rng();

        for col in &mut self.matrix_columns {
            col.y += col.speed;
            // slowly drift the hue over time for a living rainbow
            col.hue = (col.hue + col.speed * 0.6) % 360.0;

            if col.y > (area.height + col.length) as f32 {
                col.y = -(col.length as f32);
                col.speed = rng.gen_range(0.3..1.8);
                col.hue = rng.gen_range(0.0..360.0);
                for c in col.chars.iter_mut() {
                    *c = MATRIX_CHARS[rng.gen_range(0..MATRIX_CHARS.len())];
                }
            }

            for i in 0..col.length {
                let row = col.y as i32 + i as i32;
                if row < 0 || row >= area.height as i32 {
                    continue;
                }
                let x = area.x + col.x;
                let y = area.y + row as u16;
                if x >= area.x + area.width {
                    continue;
                }

                let fade = i as f32 / col.length as f32;
                let ch = col.chars[i as usize % col.chars.len()] as char;

                // shift hue along the column for a gradient effect
                let cell_hue = (col.hue + i as f32 * 8.0 + phase * 20.0) % 360.0;
                let (r, g, b) = if i == 0 {
                    // bright white head
                    (240, 255, 240)
                } else {
                    let intensity = (1.0 - fade).clamp(0.0, 1.0);
                    let (hr, hg, hb) = hsv_to_rgb(cell_hue, 0.9, intensity);
                    (hr, hg, hb)
                };

                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.set_char(ch);
                    cell.set_fg(Color::Rgb(r, g, b));
                    cell.set_bg(Color::Rgb(r / 12, g / 12, b / 12));
                }
            }

            if rng.gen_range(0..10) == 0 {
                let idx = rng.gen_range(0..col.chars.len());
                col.chars[idx] = MATRIX_CHARS[rng.gen_range(0..MATRIX_CHARS.len())];
            }
        }
    }

    fn render_starfield(&mut self, buffer: &mut Buffer, area: Rect, _phase: f32) {
        let cx = area.width as f32 / 2.0;
        let cy = area.height as f32 / 2.0;

        for star in &mut self.stars {
            star.z -= 0.012;
            if star.z <= 0.01 {
                let mut rng = rand::thread_rng();
                star.x = rng.gen_range(-1.0..1.0);
                star.y = rng.gen_range(-1.0..1.0);
                star.z = 1.0;
            }

            let sx = (star.x / star.z) * cx + cx;
            let sy = (star.y / star.z) * cy + cy;

            let px = area.x + sx as u16;
            let py = area.y + sy as u16;

            if px >= area.x && px < area.x + area.width && py >= area.y && py < area.y + area.height
            {
                let brightness = ((1.0 - star.z) * 255.0).clamp(40.0, 255.0) as u8;
                let ch = if star.z < 0.3 {
                    '@'
                } else if star.z < 0.5 {
                    '*'
                } else if star.z < 0.7 {
                    '+'
                } else {
                    '.'
                };

                if let Some(cell) = buffer.cell_mut((px, py)) {
                    cell.set_char(ch);
                    cell.set_fg(Color::Rgb(brightness, brightness, (brightness as f32 * 0.9) as u8));
                    cell.set_bg(Color::Rgb(0, 0, (brightness / 20).min(15)));
                }
            }
        }
    }

    fn render_particles(&mut self, buffer: &mut Buffer, area: Rect) {
        let mut rng = rand::thread_rng();

        for p in &mut self.particles {
            p.x += p.vx;
            p.y += p.vy;
            p.vy += 0.03;
            p.life -= 0.015;

            if p.life <= 0.0
                || p.x < 0.0
                || p.x >= area.width as f32
                || p.y < 0.0
                || p.y >= area.height as f32
            {
                *p = new_particle(&mut rng, area.width, area.height);
            }
        }

        for p in &self.particles {
            let px = area.x + p.x as u16;
            let py = area.y + p.y as u16;
            if px < area.x + area.width && py < area.y + area.height {
                let fade = p.life.clamp(0.0, 1.0);
                let (r, g, b) = p.color;
                let r = (r as f32 * fade) as u8;
                let g = (g as f32 * fade) as u8;
                let b = (b as f32 * fade) as u8;

                let ch = if p.life > 0.7 {
                    '@'
                } else if p.life > 0.4 {
                    '*'
                } else if p.life > 0.2 {
                    '+'
                } else {
                    '.'
                };

                if let Some(cell) = buffer.cell_mut((px, py)) {
                    cell.set_char(ch);
                    cell.set_fg(Color::Rgb(r, g, b));
                }
            }
        }
    }

    fn render_fire(&mut self, buffer: &mut Buffer, area: Rect) {
        let w = area.width as usize;
        let h = area.height as usize;
        if self.fire_buf.is_empty() || self.fire_buf[0].len() != w || self.fire_buf.len() != h {
            self.fire_buf = vec![vec![0.0; w]; h];
        }

        let mut rng = rand::thread_rng();
        let bottom = h - 1;
        for x in 0..w {
            self.fire_buf[bottom][x] = rng.gen_range(0.6..1.0);
        }

        for y in 0..bottom {
            for x in 0..w {
                let left = if x > 0 { self.fire_buf[y + 1][x - 1] } else { 0.0 };
                let center = self.fire_buf[y + 1][x];
                let right = if x + 1 < w { self.fire_buf[y + 1][x + 1] } else { 0.0 };
                let below = if y + 2 < h { self.fire_buf[y + 2][x] } else { center };
                self.fire_buf[y][x] = ((left + center + right + below) / 4.04).max(0.0);
            }
        }

        for y in 0..h {
            for x in 0..w {
                let intensity = self.fire_buf[y][x].clamp(0.0, 1.0);
                if intensity < 0.01 {
                    continue;
                }

                let r = (intensity * 255.0).min(255.0) as u8;
                let g = (intensity * intensity * 180.0).min(255.0) as u8;
                let b = (intensity * intensity * intensity * 80.0).min(255.0) as u8;

                let ch = if intensity > 0.8 {
                    '#'
                } else if intensity > 0.6 {
                    '*'
                } else if intensity > 0.4 {
                    '+'
                } else if intensity > 0.2 {
                    '~'
                } else {
                    '.'
                };

                let px = area.x + x as u16;
                let py = area.y + y as u16;
                if px < area.x + area.width && py < area.y + area.height {
                    if let Some(cell) = buffer.cell_mut((px, py)) {
                        cell.set_char(ch);
                        cell.set_fg(Color::Rgb(r, g, b));
                        cell.set_bg(Color::Rgb(r / 6, g / 8, 0));
                    }
                }
            }
        }
    }
}

fn new_particle(rng: &mut impl Rng, w: u16, h: u16) -> Particle {
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    let angle = rng.gen_range(0.0..std::f32::consts::TAU);
    let speed = rng.gen_range(0.3..2.0);
    let palette = [
        (255, 120, 50),
        (50, 200, 255),
        (255, 50, 180),
        (100, 255, 100),
        (255, 220, 50),
        (180, 80, 255),
    ];
    Particle {
        x: cx + rng.gen_range(-3.0..3.0),
        y: cy + rng.gen_range(-2.0..2.0),
        vx: angle.cos() * speed,
        vy: angle.sin() * speed * 0.5 - 0.5,
        life: rng.gen_range(0.5..1.0),
        color: palette[rng.gen_range(0..palette.len())],
    }
}

fn render_plasma(buffer: &mut Buffer, area: Rect, phase: f32) {
    for y in 0..area.height {
        for x in 0..area.width {
            let fx = x as f32 / area.width.max(1) as f32;
            let fy = y as f32 / area.height.max(1) as f32;

            let v1 = (fx * 10.0 + phase * 1.5).sin();
            let v2 = ((fy * 8.0 + phase * 1.1).sin() + (fx * 6.0).cos()) / 2.0;
            let v3 = ((fx * fx + fy * fy).sqrt() * 8.0 - phase * 2.0).sin();
            let v4 = (fx * 5.0 + phase).sin() * (fy * 5.0 - phase * 0.7).cos();

            let val = (v1 + v2 + v3 + v4) / 4.0;
            let norm = (val + 1.0) / 2.0;

            let r = ((norm * 3.14159 * 2.0).sin() * 127.0 + 128.0) as u8;
            let g = ((norm * 3.14159 * 2.0 + 2.094).sin() * 127.0 + 128.0) as u8;
            let b = ((norm * 3.14159 * 2.0 + 4.189).sin() * 127.0 + 128.0) as u8;

            let chars = b" .:-=+*#%@";
            let ci = (norm * (chars.len() - 1) as f32) as usize;
            let ch = chars[ci.min(chars.len() - 1)] as char;

            let px = area.x + x;
            let py = area.y + y;
            if let Some(cell) = buffer.cell_mut((px, py)) {
                cell.set_char(ch);
                cell.set_fg(Color::Rgb(r, g, b));
                cell.set_bg(Color::Rgb(r / 8, g / 8, b / 8));
            }
        }
    }
}

fn render_wireframe_cube(buffer: &mut Buffer, area: Rect, phase: f32) {
    let cx = area.width as f32 / 2.0;
    let cy = area.height as f32 / 2.0;
    let scale = (cx.min(cy * 2.0) * 0.6).max(4.0);

    let vertices: [(f32, f32, f32); 8] = [
        (-1.0, -1.0, -1.0),
        (1.0, -1.0, -1.0),
        (1.0, 1.0, -1.0),
        (-1.0, 1.0, -1.0),
        (-1.0, -1.0, 1.0),
        (1.0, -1.0, 1.0),
        (1.0, 1.0, 1.0),
        (-1.0, 1.0, 1.0),
    ];

    let edges: [(usize, usize); 12] = [
        (0, 1), (1, 2), (2, 3), (3, 0),
        (4, 5), (5, 6), (6, 7), (7, 4),
        (0, 4), (1, 5), (2, 6), (3, 7),
    ];

    let ax = phase * 0.7;
    let ay = phase * 1.1;
    let az = phase * 0.4;

    let projected: Vec<(f32, f32, f32)> = vertices
        .iter()
        .map(|&(x, y, z)| {
            let (x1, y1) = (x * ay.cos() - z * ay.sin(), x * ay.sin() + z * ay.cos());
            let z1 = y1;
            let (y2, z2) = (y * ax.cos() - z1 * ax.sin(), y * ax.sin() + z1 * ax.cos());
            let (x2, y3) = (x1 * az.cos() - y2 * az.sin(), x1 * az.sin() + y2 * az.cos());
            let depth = z2 + 3.0;
            let perspective = 2.0 / depth.max(0.5);
            (x2 * perspective * scale + cx, y3 * perspective * scale * 0.5 + cy, depth)
        })
        .collect();

    use crate::theme::{t, color_to_rgb};
    let th = t();
    let c1 = color_to_rgb(th.accent3);
    let c2 = color_to_rgb(th.accent4);
    let c3 = color_to_rgb(th.accent1);
    let c4 = color_to_rgb(th.accent2);
    let c5 = color_to_rgb(th.text);
    let edge_colors = [
        c1, c2, c3, c4,
        c1, c2, c3, c4,
        c5, c5, c5, c5,
    ];

    for (i, &(a, b)) in edges.iter().enumerate() {
        let (x0, y0, _) = projected[a];
        let (x1, y1, _) = projected[b];
        let (cr, cg, cb) = edge_colors[i % edge_colors.len()];
        draw_line(buffer, area, x0, y0, x1, y1, cr, cg, cb);
    }

    for (i, &(sx, sy, depth)) in projected.iter().enumerate() {
        let px = area.x + sx as u16;
        let py = area.y + sy as u16;
        if px >= area.x && px < area.x + area.width && py >= area.y && py < area.y + area.height {
            let brightness = ((1.0 - (depth - 2.0).abs() / 2.0) * 255.0).clamp(120.0, 255.0) as u8;
            if let Some(cell) = buffer.cell_mut((px, py)) {
                cell.set_char(['A', 'S', 'C', 'I', 'I', 'V', 'I', 'S'][i % 8]);
                cell.set_fg(Color::Rgb(brightness, brightness, (brightness as f32 * 0.8) as u8));
            }
        }
    }
}

fn draw_line(buffer: &mut Buffer, area: Rect, x0: f32, y0: f32, x1: f32, y1: f32, r: u8, g: u8, b: u8) {
    let dx = (x1 - x0).abs();
    let dy = (y1 - y0).abs();
    let steps = dx.max(dy).max(1.0) as usize;

    for i in 0..=steps {
        let t = i as f32 / steps.max(1) as f32;
        let x = x0 + (x1 - x0) * t;
        let y = y0 + (y1 - y0) * t;

        let px = area.x + x as u16;
        let py = area.y + y as u16;

        if px >= area.x && px < area.x + area.width && py >= area.y && py < area.y + area.height {
            let ch = if dx > dy * 2.0 {
                '-'
            } else if dy > dx * 2.0 {
                '|'
            } else if (x1 > x0) == (y1 > y0) {
                '\\'
            } else {
                '/'
            };

            if let Some(cell) = buffer.cell_mut((px, py)) {
                cell.set_char(ch);
                cell.set_fg(Color::Rgb(r, g, b));
            }
        }
    }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = h % 360.0;
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    (
        ((r1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).clamp(0.0, 255.0) as u8,
    )
}
