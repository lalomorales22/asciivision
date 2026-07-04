//! The "ASCII GPU": a tiny software shading toolkit for terminal pixels.
//!
//! Provides Vec3 math, SDF primitives, a sphere-tracing ray marcher with
//! tetrahedron normals, a two-light + rim + fog + gamma shading model,
//! iq-style cosine palettes, and two rasterization drivers:
//!
//! * [`run_half_block`] -- writes U+2580 '▀' cells (fg = upper pixel,
//!   bg = lower pixel), doubling vertical resolution. Because terminal
//!   cells are ~2:1 (h:w), half-block pixels come out roughly square,
//!   so a sphere rendered through this driver is ROUND.
//! * [`run_ascii`] -- classic luminance-ramp glyph rendering at cell
//!   resolution (with the 2:1 cell aspect folded into the uv mapping).
//!
//! All coordinates handed to shader closures are centered uv:
//! `y` in [-1, 1] (up is +y), `x` in [-aspect, aspect].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

// ---------------------------------------------------------------------------
// Vec3
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Shorthand constructor.
pub const fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3 { x, y, z }
}

impl Vec3 {
    pub const ZERO: Vec3 = v3(0.0, 0.0, 0.0);
    #[allow(dead_code)] // part of the shader API surface (used in tests today)
    pub const ONE: Vec3 = v3(1.0, 1.0, 1.0);

    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    #[allow(dead_code)] // part of the shader API surface (used in tests today)
    pub fn cross(self, o: Vec3) -> Vec3 {
        v3(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length_sq(self) -> f32 {
        self.dot(self)
    }

    pub fn length(self) -> f32 {
        self.length_sq().sqrt()
    }

    pub fn normalize(self) -> Vec3 {
        let len = self.length();
        if len > 1e-8 {
            self * (1.0 / len)
        } else {
            v3(0.0, 0.0, 1.0)
        }
    }

    pub fn abs(self) -> Vec3 {
        v3(self.x.abs(), self.y.abs(), self.z.abs())
    }

    pub fn max_scalar(self, s: f32) -> Vec3 {
        v3(self.x.max(s), self.y.max(s), self.z.max(s))
    }

    /// Component-wise clamp into [0, 1].
    pub fn saturate(self) -> Vec3 {
        v3(clamp01(self.x), clamp01(self.y), clamp01(self.z))
    }

    /// Component-wise cosine (used by the palette helpers).
    pub fn cos(self) -> Vec3 {
        v3(self.x.cos(), self.y.cos(), self.z.cos())
    }

    /// Convert a linear [0,1] color vector into an RGB tuple.
    pub fn to_rgb8(self) -> (u8, u8, u8) {
        let c = self.saturate();
        (
            (c.x * 255.0) as u8,
            (c.y * 255.0) as u8,
            (c.z * 255.0) as u8,
        )
    }
}

impl std::ops::Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl std::ops::Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        v3(self.x * s, self.y * s, self.z * s)
    }
}

/// Component-wise multiply (color modulation).
impl std::ops::Mul<Vec3> for Vec3 {
    type Output = Vec3;
    fn mul(self, o: Vec3) -> Vec3 {
        v3(self.x * o.x, self.y * o.y, self.z * o.z)
    }
}

impl std::ops::Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        v3(-self.x, -self.y, -self.z)
    }
}

// ---------------------------------------------------------------------------
// Scalar helpers
// ---------------------------------------------------------------------------

pub fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

pub fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn mix_v3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a + (b - a) * t
}

/// Hermite smoothstep. Supports reversed edges (`e1 < e0`) for fade-outs.
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let d = e1 - e0;
    // Guard zero-width edges while PRESERVING the sign -- a plain .max()
    // here would invert every reversed-edge call.
    let d = if d.abs() < 1e-8 { 1e-8f32.copysign(d) } else { d };
    let t = clamp01((x - e0) / d);
    t * t * (3.0 - 2.0 * t)
}

/// Rotate the 2D point (x, y) by `angle` radians.
pub fn rot2(x: f32, y: f32, angle: f32) -> (f32, f32) {
    let (s, c) = angle.sin_cos();
    (c * x - s * y, s * x + c * y)
}

/// Wrap unbounded wall-clock time into a sane range before trig so
/// long-running sessions do not lose float precision.
pub fn wrap_time(t: f32) -> f32 {
    t.rem_euclid(3600.0)
}

/// Deterministic Wang-style integer hash for coordinate noise.
pub fn hash32(x: u32, y: u32, seed: u32) -> u32 {
    let mut value = x;
    value = value.wrapping_mul(0x045d_9f3b);
    value ^= y.wrapping_mul(0x119d_e1f3);
    value ^= seed.wrapping_mul(0x3449_5cbd);
    value ^= value >> 16;
    value = value.wrapping_mul(0x045d_9f3b);
    value ^ (value >> 16)
}

/// Hash noise mapped into [0, 1].
pub fn hash01(x: u32, y: u32, seed: u32) -> f32 {
    (hash32(x, y, seed) & 0xffff) as f32 / 65535.0
}

// ---------------------------------------------------------------------------
// Color helpers
// ---------------------------------------------------------------------------

/// Standard HSV -> RGB. `h` in degrees, `s`/`v` in [0, 1].
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = h.rem_euclid(360.0);
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

/// Linear blend between two RGB tuples.
#[allow(dead_code)] // contracted shader API; consumers land in stage 2
pub fn mix_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = clamp01(t);
    (
        (a.0 as f32 + (b.0 as f32 - a.0 as f32) * t) as u8,
        (a.1 as f32 + (b.1 as f32 - a.1 as f32) * t) as u8,
        (a.2 as f32 + (b.2 as f32 - a.2 as f32) * t) as u8,
    )
}

/// Convert an RGB tuple to a linear [0,1] color vector.
pub fn rgb_to_v3(rgb: (u8, u8, u8)) -> Vec3 {
    v3(
        rgb.0 as f32 / 255.0,
        rgb.1 as f32 / 255.0,
        rgb.2 as f32 / 255.0,
    )
}

/// iq-style cosine palette: `a + b * cos(TAU * (c*t + d))`.
pub fn cos_palette(t: f32, a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> Vec3 {
    const TAU: f32 = std::f32::consts::TAU;
    (a + b * ((c * t + d) * TAU).cos()).saturate()
}

/// Classic rainbow-ish palette (iq's default).
pub fn palette_spectrum(t: f32) -> Vec3 {
    cos_palette(
        t,
        v3(0.5, 0.5, 0.5),
        v3(0.5, 0.5, 0.5),
        v3(1.0, 1.0, 1.0),
        v3(0.0, 0.33, 0.67),
    )
}

/// Deep-space blues into hot magenta/orange.
pub fn palette_nebula(t: f32) -> Vec3 {
    cos_palette(
        t,
        v3(0.5, 0.5, 0.5),
        v3(0.5, 0.5, 0.5),
        v3(1.0, 1.0, 1.0),
        v3(0.30, 0.20, 0.20),
    )
}

/// Synthwave sunset: purple / pink / cyan.
#[allow(dead_code)] // contracted shader API; consumers land in stage 2
pub fn palette_synthwave(t: f32) -> Vec3 {
    cos_palette(
        t,
        v3(0.55, 0.25, 0.55),
        v3(0.45, 0.35, 0.45),
        v3(1.0, 1.0, 1.0),
        v3(0.85, 0.55, 0.20),
    )
}

/// Apply gamma 2.2 to a linear color so terminal output matches a
/// WebGL-style pipeline (light math in linear space, display in sRGB).
pub fn gamma_correct(c: Vec3) -> Vec3 {
    let c = c.max_scalar(0.0);
    v3(
        c.x.powf(1.0 / 2.2),
        c.y.powf(1.0 / 2.2),
        c.z.powf(1.0 / 2.2),
    )
}

// ---------------------------------------------------------------------------
// SDF primitives
// ---------------------------------------------------------------------------

pub fn sd_sphere(p: Vec3, r: f32) -> f32 {
    p.length() - r
}

#[allow(dead_code)] // contracted SDF primitive; consumers land in stage 2
pub fn sd_box(p: Vec3, b: Vec3) -> f32 {
    let q = p.abs() - b;
    q.max_scalar(0.0).length() + q.x.max(q.y.max(q.z)).min(0.0)
}

/// Torus in the xz plane: `major` ring radius, `minor` tube radius.
#[allow(dead_code)] // contracted SDF primitive; consumers land in stage 2
pub fn sd_torus(p: Vec3, major: f32, minor: f32) -> f32 {
    let qx = (p.x * p.x + p.z * p.z).sqrt() - major;
    (qx * qx + p.y * p.y).sqrt() - minor
}

/// Horizontal plane at height `h` (normal +y).
#[allow(dead_code)] // contracted SDF primitive; consumers land in stage 2
pub fn sd_plane_y(p: Vec3, h: f32) -> f32 {
    p.y - h
}

/// Polynomial smooth minimum (iq). `k` controls blend radius.
pub fn smooth_min(a: f32, b: f32, k: f32) -> f32 {
    let h = clamp01(0.5 + 0.5 * (b - a) / k.max(1e-6));
    mix(b, a, h) - k * h * (1.0 - h)
}

// ---------------------------------------------------------------------------
// Ray marching
// ---------------------------------------------------------------------------

/// Sphere-trace `sdf` from `ro` along `rd`. Returns `(distance, steps)` on a
/// hit, `None` if the ray escapes past `max_dist` or runs out of steps.
/// Distorted (non-Lipschitz) SDFs should pre-scale their return value.
pub fn march(
    ro: Vec3,
    rd: Vec3,
    sdf: impl Fn(Vec3) -> f32,
    max_steps: u32,
    max_dist: f32,
) -> Option<(f32, u32)> {
    let mut t = 0.0f32;
    for i in 0..max_steps {
        let d = sdf(ro + rd * t);
        if d < 0.0018 * t.max(0.6) {
            return Some((t, i));
        }
        t += d;
        if t > max_dist {
            return None;
        }
    }
    None
}

/// Analytic ray / sphere-at-origin intersection for bounding volumes.
/// Returns the (t_enter, t_exit) span (clamped to t >= 0) or `None` if the
/// ray misses -- lets marchers skip empty pixels entirely.
pub fn ray_sphere_span(ro: Vec3, rd: Vec3, radius: f32) -> Option<(f32, f32)> {
    let b = ro.dot(rd);
    let c = ro.length_sq() - radius * radius;
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let sq = disc.sqrt();
    let t1 = -b + sq;
    if t1 < 0.0 {
        return None;
    }
    Some(((-b - sq).max(0.0), t1))
}

/// Surface normal via the tetrahedron technique (4 SDF taps).
pub fn normal(p: Vec3, sdf: impl Fn(Vec3) -> f32) -> Vec3 {
    const E: f32 = 0.0015;
    let k0 = v3(1.0, -1.0, -1.0);
    let k1 = v3(-1.0, -1.0, 1.0);
    let k2 = v3(-1.0, 1.0, -1.0);
    let k3 = v3(1.0, 1.0, 1.0);
    (k0 * sdf(p + k0 * E)
        + k1 * sdf(p + k1 * E)
        + k2 * sdf(p + k2 * E)
        + k3 * sdf(p + k3 * E))
    .normalize()
}

/// Cheap soft shadow factor in [0, 1] toward `light_dir` (iq's trick).
#[allow(dead_code)] // contracted shader API; consumers land in stage 2
pub fn soft_shadow(
    p: Vec3,
    light_dir: Vec3,
    k: f32,
    sdf: impl Fn(Vec3) -> f32,
    max_dist: f32,
) -> f32 {
    let mut res: f32 = 1.0;
    let mut t = 0.04f32;
    for _ in 0..24 {
        let d = sdf(p + light_dir * t);
        if d < 0.001 {
            return 0.0;
        }
        res = res.min(k * d / t);
        t += d.clamp(0.02, 0.35);
        if t > max_dist {
            break;
        }
    }
    clamp01(res)
}

// ---------------------------------------------------------------------------
// Lighting
// ---------------------------------------------------------------------------

/// Surface material for [`shade`].
pub struct Material {
    /// Albedo in linear [0,1] space.
    pub base: Vec3,
    /// Blinn-Phong specular exponent.
    pub spec_power: f32,
    /// Specular strength multiplier.
    pub spec_strength: f32,
    /// Rim/fresnel strength multiplier.
    pub rim: f32,
}

/// Warm key light direction (roughly upper-right-front).
pub const KEY_DIR: Vec3 = v3(0.5411, 0.6303, -0.5570);
/// Cool fill light direction (lower-left-back).
pub const FILL_DIR: Vec3 = v3(-0.6155, 0.1231, 0.7784);

/// Standard 2-light shading: warm key (lambert + blinn specular), cool fill,
/// fresnel rim, ambient floor, and step-count ambient occlusion. Returns a
/// LINEAR color -- callers apply fog then [`gamma_correct`].
pub fn shade(n: Vec3, rd: Vec3, mat: &Material, ao: f32) -> Vec3 {
    let key_col = v3(1.05, 0.97, 0.88);
    let fill_col = v3(0.22, 0.28, 0.42);

    let diff_key = n.dot(KEY_DIR).max(0.0);
    let diff_fill = n.dot(FILL_DIR).max(0.0);

    // Blinn-Phong specular from the key light.
    let h = (KEY_DIR - rd).normalize();
    let spec = n.dot(h).max(0.0).powf(mat.spec_power) * mat.spec_strength;

    // Fresnel rim: bright silhouette edges, the classic "WebGL demo" look.
    let facing = n.dot(-rd).max(0.0);
    let rim = (1.0 - facing).powf(3.0) * mat.rim;

    let ambient = 0.07;
    let lit = key_col * diff_key + fill_col * diff_fill + v3(ambient, ambient, ambient);
    (mat.base * lit * ao + key_col * spec + v3(0.65, 0.8, 1.0) * rim).max_scalar(0.0)
}

/// Exponential-squared distance fog toward `fog_color`.
pub fn apply_fog(color: Vec3, fog_color: Vec3, dist: f32, density: f32) -> Vec3 {
    let f = 1.0 - (-dist * dist * density).exp();
    mix_v3(color, fog_color, clamp01(f))
}

// ---------------------------------------------------------------------------
// Rasterization drivers
// ---------------------------------------------------------------------------

const HALF_BLOCK: char = '\u{2580}'; // '▀' upper half block (single-width)

/// Luminance ramp shared by the ascii driver (dark -> bright).
pub const LUMA_RAMP: &[u8] = b" .:-=+*#%@";

/// uv for a virtual pixel center. `w`/`h_px` are the virtual framebuffer
/// dimensions in (square-ish) pixels. +y is up, x spans [-aspect, aspect].
#[inline]
fn pixel_uv(px: f32, py: f32, w: f32, h_px: f32) -> (f32, f32) {
    let half_h = h_px * 0.5;
    let u = (px - w * 0.5) / half_h;
    let v = -(py - half_h) / half_h;
    (u, v)
}

/// Half-block subpixel driver: samples a virtual framebuffer of
/// `area.width x area.height*2` pixels and writes '▀' cells with
/// fg = upper pixel and bg = lower pixel. The shader `f` receives centered
/// uv coordinates (y in [-1,1] pointing up, x in [-aspect, aspect]) and
/// returns a display-ready (r, g, b) color in [0,1] per channel.
pub fn run_half_block(
    buffer: &mut Buffer,
    area: Rect,
    mut f: impl FnMut(f32, f32) -> (f32, f32, f32),
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let w = area.width as f32;
    let h_px = (area.height as f32) * 2.0;
    for cy in 0..area.height {
        let py_top = (cy as f32) * 2.0 + 0.5;
        let py_bot = py_top + 1.0;
        for cx in 0..area.width {
            let px = cx as f32 + 0.5;
            let (u_t, v_t) = pixel_uv(px, py_top, w, h_px);
            let (u_b, v_b) = pixel_uv(px, py_bot, w, h_px);
            let top = f(u_t, v_t);
            let bot = f(u_b, v_b);
            if let Some(cell) = buffer.cell_mut((area.x + cx, area.y + cy)) {
                cell.set_char(HALF_BLOCK);
                cell.set_fg(Color::Rgb(
                    (clamp01(top.0) * 255.0) as u8,
                    (clamp01(top.1) * 255.0) as u8,
                    (clamp01(top.2) * 255.0) as u8,
                ));
                cell.set_bg(Color::Rgb(
                    (clamp01(bot.0) * 255.0) as u8,
                    (clamp01(bot.1) * 255.0) as u8,
                    (clamp01(bot.2) * 255.0) as u8,
                ));
            }
        }
    }
}

/// Glyph-ramp driver at cell resolution. The 2:1 cell aspect is folded into
/// the uv mapping (same [-aspect, aspect] x [-1, 1] contract as
/// [`run_half_block`]). Luminance selects a ramp glyph; fg is the shader
/// color, bg is a dimmed glow of it.
#[allow(dead_code)] // contracted driver (glyph-ramp looks); consumers land in stage 2
pub fn run_ascii(
    buffer: &mut Buffer,
    area: Rect,
    mut f: impl FnMut(f32, f32) -> (f32, f32, f32),
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let w = area.width as f32;
    // Cells are ~2 units tall, 1 wide: virtual height = 2*rows units.
    let h_units = (area.height as f32) * 2.0;
    for cy in 0..area.height {
        let py = (cy as f32) * 2.0 + 1.0; // cell center in units
        for cx in 0..area.width {
            let px = cx as f32 + 0.5;
            let (u, v) = pixel_uv(px, py, w, h_units);
            let (r, g, b) = f(u, v);
            let (r, g, b) = (clamp01(r), clamp01(g), clamp01(b));
            let luma = 0.299 * r + 0.587 * g + 0.114 * b;
            let idx = ((luma * (LUMA_RAMP.len() - 1) as f32) as usize).min(LUMA_RAMP.len() - 1);
            let ch = LUMA_RAMP[idx] as char;
            if let Some(cell) = buffer.cell_mut((area.x + cx, area.y + cy)) {
                cell.set_char(ch);
                cell.set_fg(Color::Rgb(
                    (r * 255.0) as u8,
                    (g * 255.0) as u8,
                    (b * 255.0) as u8,
                ));
                cell.set_bg(Color::Rgb(
                    (r * 38.0) as u8,
                    (g * 38.0) as u8,
                    (b * 38.0) as u8,
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pixel canvas (for line/point renderers that want half-block resolution)
// ---------------------------------------------------------------------------

/// A reusable RGB framebuffer at half-block resolution (width x height*2).
/// Vector renderers (wireframes, particles) rasterize into it with additive
/// glow, then [`Canvas::blit`] writes it to the terminal as '▀' cells.
pub struct Canvas {
    w: i32,
    h: i32,
    px: Vec<Vec3>,
}

impl Canvas {
    pub fn new() -> Self {
        Canvas {
            w: 0,
            h: 0,
            px: Vec::new(),
        }
    }

    pub fn width(&self) -> i32 {
        self.w
    }

    pub fn height(&self) -> i32 {
        self.h
    }

    /// Resize to match `area` (pixels = width x height*2) and clear to `bg`.
    pub fn begin(&mut self, area: Rect, bg: Vec3) {
        let w = area.width as i32;
        let h = area.height as i32 * 2;
        if w != self.w || h != self.h {
            self.w = w;
            self.h = h;
            self.px = vec![bg; (w * h).max(0) as usize];
        } else {
            for p in &mut self.px {
                *p = bg;
            }
        }
    }

    /// Additive splat with i32 culling (negative coords are dropped, never
    /// smeared onto the edge).
    pub fn add(&mut self, x: i32, y: i32, c: Vec3) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let idx = (y * self.w + x) as usize;
        self.px[idx] = self.px[idx] + c;
    }

    /// Additive anti-glow point: full color at the pixel plus a soft halo.
    pub fn splat(&mut self, x: f32, y: f32, c: Vec3) {
        let xi = x.floor() as i32;
        let yi = y.floor() as i32;
        self.add(xi, yi, c);
        let halo = c * 0.28;
        self.add(xi + 1, yi, halo);
        self.add(xi - 1, yi, halo);
        self.add(xi, yi + 1, halo);
        self.add(xi, yi - 1, halo);
    }

    /// DDA line in pixel space with per-endpoint colors and additive glow.
    /// Coordinates may be negative / out of range; culling is per-pixel.
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c0: Vec3, c1: Vec3) {
        let dx = x1 - x0;
        let dy = y1 - y0;
        let steps = dx.abs().max(dy.abs()).ceil().max(1.0);
        let n = steps as i32;
        for i in 0..=n {
            let t = i as f32 / steps;
            self.splat(x0 + dx * t, y0 + dy * t, mix_v3(c0, c1, t) * 0.9);
        }
    }

    /// Write the framebuffer into `buffer` as half-block cells.
    pub fn blit(&self, buffer: &mut Buffer, area: Rect) {
        if self.w != area.width as i32 || self.h != area.height as i32 * 2 {
            return;
        }
        for cy in 0..area.height {
            for cx in 0..area.width {
                let top = self.px[(cy as i32 * 2 * self.w + cx as i32) as usize];
                let bot = self.px[((cy as i32 * 2 + 1) * self.w + cx as i32) as usize];
                if let Some(cell) = buffer.cell_mut((area.x + cx, area.y + cy)) {
                    let t = top.to_rgb8();
                    let b = bot.to_rgb8();
                    cell.set_char(HALF_BLOCK);
                    cell.set_fg(Color::Rgb(t.0, t.1, t.2));
                    cell.set_bg(Color::Rgb(b.0, b.1, b.2));
                }
            }
        }
    }
}

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn vec3_basic_ops() {
        let a = v3(1.0, 2.0, 3.0);
        let b = v3(4.0, 5.0, 6.0);
        assert_eq!(a + b, v3(5.0, 7.0, 9.0));
        assert_eq!(b - a, v3(3.0, 3.0, 3.0));
        assert_eq!(a * 2.0, v3(2.0, 4.0, 6.0));
        assert_eq!(a * b, v3(4.0, 10.0, 18.0));
        assert!(approx(a.dot(b), 32.0, 1e-6));
        assert_eq!(-a, v3(-1.0, -2.0, -3.0));
    }

    #[test]
    fn vec3_cross_is_orthogonal() {
        let a = v3(1.0, 0.0, 0.0);
        let b = v3(0.0, 1.0, 0.0);
        assert_eq!(a.cross(b), v3(0.0, 0.0, 1.0));
        let c = v3(2.0, -3.0, 1.5).cross(v3(0.4, 2.0, -7.0));
        assert!(approx(c.dot(v3(2.0, -3.0, 1.5)), 0.0, 1e-4));
    }

    #[test]
    fn vec3_normalize_unit_length() {
        let n = v3(3.0, -4.0, 12.0).normalize();
        assert!(approx(n.length(), 1.0, 1e-5));
        // Degenerate input must not produce NaN.
        let z = Vec3::ZERO.normalize();
        assert!(z.length().is_finite());
    }

    #[test]
    fn scalar_helpers() {
        assert_eq!(clamp01(-2.0), 0.0);
        assert_eq!(clamp01(2.0), 1.0);
        assert!(approx(smoothstep(0.0, 1.0, 0.0), 0.0, 1e-6));
        assert!(approx(smoothstep(0.0, 1.0, 1.0), 1.0, 1e-6));
        assert!(approx(smoothstep(0.0, 1.0, 0.5), 0.5, 1e-6));
        // Reversed edges must fade the other way, not invert.
        assert!(approx(smoothstep(1.0, 0.0, 0.0), 1.0, 1e-6));
        assert!(approx(smoothstep(1.0, 0.0, 1.0), 0.0, 1e-6));
        assert!(approx(smoothstep(0.30, 0.285, 0.58), 0.0, 1e-6));
        assert!(approx(smoothstep(0.30, 0.285, 0.1), 1.0, 1e-6));
        assert!(approx(mix(2.0, 4.0, 0.5), 3.0, 1e-6));
        let (x, y) = rot2(1.0, 0.0, std::f32::consts::FRAC_PI_2);
        assert!(approx(x, 0.0, 1e-5) && approx(y, 1.0, 1e-5));
        assert!(wrap_time(3600.5) < 3600.0);
        assert!(wrap_time(-1.0) >= 0.0);
    }

    #[test]
    fn hsv_known_values() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), (255, 0, 0));
        assert_eq!(hsv_to_rgb(120.0, 1.0, 1.0), (0, 255, 0));
        assert_eq!(hsv_to_rgb(240.0, 1.0, 1.0), (0, 0, 255));
        assert_eq!(hsv_to_rgb(0.0, 0.0, 1.0), (255, 255, 255));
        assert_eq!(hsv_to_rgb(0.0, 0.0, 0.0), (0, 0, 0));
    }

    #[test]
    fn palette_stays_in_range() {
        for i in 0..100 {
            let t = i as f32 * 0.13 - 5.0;
            for c in [palette_spectrum(t), palette_nebula(t), palette_synthwave(t)] {
                assert!((0.0..=1.0).contains(&c.x));
                assert!((0.0..=1.0).contains(&c.y));
                assert!((0.0..=1.0).contains(&c.z));
            }
        }
    }

    #[test]
    fn sdf_distances() {
        assert!(approx(sd_sphere(v3(2.0, 0.0, 0.0), 1.0), 1.0, 1e-5));
        assert!(sd_sphere(Vec3::ZERO, 1.0) < 0.0);
        assert!(approx(sd_box(v3(2.0, 0.0, 0.0), Vec3::ONE), 1.0, 1e-5));
        assert!(sd_box(Vec3::ZERO, Vec3::ONE) < 0.0);
        // Point on the torus ring center circle is exactly -minor away.
        assert!(approx(sd_torus(v3(2.0, 0.0, 0.0), 2.0, 0.5), -0.5, 1e-5));
        assert!(approx(sd_plane_y(v3(0.0, 3.0, 0.0), 1.0), 2.0, 1e-6));
        // Smooth min never exceeds the plain min, and equals it far apart.
        assert!(smooth_min(1.0, 5.0, 0.3) <= 1.0 + 1e-6);
        assert!(approx(smooth_min(1.0, 50.0, 0.3), 1.0, 1e-4));
    }

    #[test]
    fn march_hits_sphere_dead_center() {
        let sdf = |p: Vec3| sd_sphere(p, 1.0);
        let hit = march(v3(0.0, 0.0, -3.0), v3(0.0, 0.0, 1.0), sdf, 64, 10.0);
        let (t, steps) = hit.expect("ray straight at a sphere must hit");
        assert!(approx(t, 2.0, 0.02), "hit distance {t} should be ~2.0");
        assert!(steps < 64);
        // A ray pointed away must miss.
        assert!(march(v3(0.0, 0.0, -3.0), v3(0.0, 0.0, -1.0), sdf, 64, 10.0).is_none());
    }

    #[test]
    fn ray_sphere_span_enter_exit() {
        let ro = v3(0.0, 0.0, -3.0);
        let rd = v3(0.0, 0.0, 1.0);
        let (t_in, t_out) = ray_sphere_span(ro, rd, 1.0).expect("must hit");
        assert!(approx(t_in, 2.0, 1e-4));
        assert!(approx(t_out, 4.0, 1e-4));
        // Grazing miss.
        assert!(ray_sphere_span(ro, v3(0.9, 0.0, 0.1).normalize(), 1.0).is_none());
        // Sphere fully behind the origin.
        assert!(ray_sphere_span(v3(0.0, 0.0, 3.0), rd, 1.0).is_none());
        // Ray starting inside clamps t_enter to 0.
        let (t_in, t_out) = ray_sphere_span(Vec3::ZERO, rd, 1.0).expect("inside");
        assert_eq!(t_in, 0.0);
        assert!(approx(t_out, 1.0, 1e-5));
    }

    #[test]
    fn normal_of_sphere_points_outward() {
        let sdf = |p: Vec3| sd_sphere(p, 1.0);
        let n = normal(v3(0.0, 0.0, -1.0), sdf);
        assert!(approx(n.z, -1.0, 1e-2));
        assert!(approx(n.length(), 1.0, 1e-4));
    }

    #[test]
    fn shade_and_fog_behave() {
        let mat = Material {
            base: v3(0.8, 0.3, 0.2),
            spec_power: 16.0,
            spec_strength: 0.5,
            rim: 0.4,
        };
        let lit = shade(KEY_DIR, v3(0.0, 0.0, 1.0), &mat, 1.0);
        let unlit = shade(-KEY_DIR, v3(0.0, 0.0, 1.0), &mat, 1.0);
        assert!(lit.x > unlit.x, "facing the key light must be brighter");
        let fogged = apply_fog(v3(1.0, 1.0, 1.0), Vec3::ZERO, 100.0, 0.1);
        assert!(fogged.length() < 0.01, "far surfaces vanish into fog");
        let near = apply_fog(v3(1.0, 1.0, 1.0), Vec3::ZERO, 0.0, 0.1);
        assert!(approx(near.x, 1.0, 1e-5));
    }

    /// The headline invariant: a sphere rendered through run_half_block must
    /// be ROUND -- lit width (cells) ~= lit height (half-block rows) since
    /// half-block pixels are square under the 2:1 cell aspect model.
    #[test]
    fn half_block_sphere_is_round() {
        let area = Rect::new(0, 0, 80, 24); // 80x48 virtual pixels
        let mut buf = Buffer::empty(area);
        let sdf = |p: Vec3| sd_sphere(p, 1.0);
        run_half_block(&mut buf, area, |u, v| {
            let ro = v3(0.0, 0.0, -3.0);
            let rd = v3(u, v, 1.8).normalize();
            if march(ro, rd, sdf, 64, 10.0).is_some() {
                (1.0, 1.0, 1.0)
            } else {
                (0.0, 0.0, 0.0)
            }
        });

        // Reconstruct the 80x48 pixel mask from fg (top) / bg (bottom).
        let lit = |c: Color| matches!(c, Color::Rgb(r, _, _) if r > 128);
        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        for cy in 0..area.height {
            for cx in 0..area.width {
                let cell = &buf[(cx, cy)];
                for (row, on) in [(cy as i32 * 2, lit(cell.fg)), (cy as i32 * 2 + 1, lit(cell.bg))]
                {
                    if on {
                        min_x = min_x.min(cx as i32);
                        max_x = max_x.max(cx as i32);
                        min_y = min_y.min(row);
                        max_y = max_y.max(row);
                    }
                }
            }
        }
        assert!(min_x < max_x && min_y < max_y, "sphere must be visible");
        let w = (max_x - min_x + 1) as f32;
        let h = (max_y - min_y + 1) as f32;
        let ratio = w / h;
        assert!(
            (0.85..=1.18).contains(&ratio),
            "sphere aspect ratio {ratio} (w={w}, h={h}) is not round"
        );
        // And it must be centered.
        let cx_mid = (min_x + max_x) as f32 / 2.0;
        let cy_mid = (min_y + max_y) as f32 / 2.0;
        assert!((cx_mid - 39.5).abs() < 2.0, "sphere x-center {cx_mid} off");
        assert!((cy_mid - 23.5).abs() < 2.0, "sphere y-center {cy_mid} off");
    }

    #[test]
    fn ascii_driver_writes_ramp_glyphs() {
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        run_ascii(&mut buf, area, |u, _v| {
            let x = clamp01(u * 0.5 + 0.5);
            (x, x, x)
        });
        let left = buf[(0, 5)].symbol().as_bytes()[0];
        let right = buf[(19, 5)].symbol().as_bytes()[0];
        assert!(LUMA_RAMP.contains(&left));
        assert!(LUMA_RAMP.contains(&right));
        assert!(
            LUMA_RAMP.iter().position(|&c| c == right)
                > LUMA_RAMP.iter().position(|&c| c == left),
            "brighter side must use a denser glyph"
        );
    }

    #[test]
    fn canvas_culls_negative_coordinates() {
        let area = Rect::new(0, 0, 10, 5);
        let mut canvas = Canvas::new();
        canvas.begin(area, Vec3::ZERO);
        // A line running far off the top-left must not smear onto the edge.
        canvas.line(-30.0, -20.0, -1.0, -1.0, Vec3::ONE, Vec3::ONE);
        let mut buf = Buffer::empty(area);
        canvas.blit(&mut buf, area);
        for cy in 0..area.height {
            for cx in 0..area.width {
                let cell = buf.cell((cx, cy)).unwrap();
                assert_eq!(cell.fg, Color::Rgb(0, 0, 0), "cell ({cx},{cy}) smeared");
                assert_eq!(cell.bg, Color::Rgb(0, 0, 0), "cell ({cx},{cy}) smeared");
            }
        }
        // An in-bounds splat lands where asked (pixel 4,6 -> cell 4, row 3 bg).
        canvas.splat(4.0, 7.0, Vec3::ONE);
        canvas.blit(&mut buf, area);
        let cell = buf.cell((4u16, 3u16)).unwrap();
        assert_ne!(cell.bg, Color::Rgb(0, 0, 0));
    }

    #[test]
    fn hash_is_deterministic_and_spread() {
        assert_eq!(hash32(3, 7, 11), hash32(3, 7, 11));
        assert_ne!(hash32(3, 7, 11), hash32(4, 7, 11));
        let n = hash01(10, 20, 30);
        assert!((0.0..=1.0).contains(&n));
    }
}
