//! Effects registry: every visual effect implements [`Effect`] and lives in
//! an ordered `Vec<Box<dyn Effect>>` inside [`EffectsEngine`].
//!
//! Ring order: the five ray-marched / shader showpieces first (TORUS KNOT,
//! METABALLS, TUNNEL FLIGHT, SYNTHWAVE GRID, JULIA SET), then the six
//! dt-normalized legacy effects. F4 / `/fx` cycling and the off state are
//! pure index arithmetic -- off is reached after the last effect, no
//! hardcoded sentinel.
//!
//! All heavy pixel work goes through [`crate::shader`], which renders at
//! half-block ('▀') subpixel resolution with proper aspect correction.

use rand::Rng;
use ratatui::prelude::*;
use std::time::Instant;

use crate::shader::{
    self, apply_fog, clamp01, gamma_correct, hash01, hsv_to_rgb, march, mix_v3, normal,
    palette_nebula, palette_spectrum, rgb_to_v3, run_half_block, sd_sphere, smooth_min,
    smoothstep, v3, Canvas, Material, Vec3,
};
use crate::theme::{color_to_rgb, t};

const MATRIX_CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789@#$%&*+=<>{}[]|/\\~";

/// A single visual effect. `time` is wall-clock seconds (already wrapped to
/// a sane range), `dt` is the frame delta clamped to <= 0.05s.
pub trait Effect {
    fn name(&self) -> &'static str;
    fn render(&mut self, buf: &mut Buffer, area: Rect, time: f32, dt: f32);
    fn reset(&mut self) {}
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

pub struct EffectsEngine {
    effects: Vec<Box<dyn Effect>>,
    index: usize,
    pub active: bool,
    last_frame: Option<Instant>,
    last_size: (u16, u16),
}

impl EffectsEngine {
    pub fn new() -> Self {
        let effects: Vec<Box<dyn Effect>> = vec![
            Box::new(TorusKnot),
            Box::new(Metaballs),
            Box::new(Tunnel),
            Box::new(Synthwave),
            Box::new(Julia),
            Box::new(MatrixRain::new()),
            Box::new(Plasma),
            Box::new(Starfield::new()),
            Box::new(WireframeCube::new()),
            Box::new(Fire::new()),
            Box::new(Particles::new()),
        ];
        Self {
            effects,
            index: 0,
            active: false,
            last_frame: None,
            last_size: (0, 0),
        }
    }

    /// Name of the currently selected effect (valid even while inactive).
    pub fn current_name(&self) -> &'static str {
        self.effects[self.index].name()
    }

    /// All effect names in ring order (for the command palette / help).
    pub fn names(&self) -> Vec<&'static str> {
        self.effects.iter().map(|e| e.name()).collect()
    }

    /// F4 / `/fx` behavior: off -> first effect -> ... -> last effect -> off.
    pub fn cycle_with_off(&mut self) {
        if !self.active {
            self.active = true;
            self.effects[self.index].reset();
            self.last_frame = None;
            return;
        }
        if self.index + 1 >= self.effects.len() {
            // Past the last effect: switch off and rewind to the top.
            self.active = false;
            self.index = 0;
            return;
        }
        self.index += 1;
        self.effects[self.index].reset();
        self.last_frame = None;
    }

    /// Select an effect by (case-insensitive) name, prefix, or substring.
    /// Activates the engine on success.
    pub fn set_by_name(&mut self, name: &str) -> bool {
        let needle = name.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return false;
        }
        let pos = self
            .effects
            .iter()
            .position(|e| e.name().to_ascii_lowercase() == needle)
            .or_else(|| {
                self.effects
                    .iter()
                    .position(|e| e.name().to_ascii_lowercase().starts_with(&needle))
            })
            .or_else(|| {
                self.effects
                    .iter()
                    .position(|e| e.name().to_ascii_lowercase().contains(&needle))
            });
        match pos {
            Some(i) => {
                self.index = i;
                self.active = true;
                self.effects[i].reset();
                self.last_frame = None;
                true
            }
            None => false,
        }
    }

    /// Render the current effect. Computes dt internally (clamped to 0.05s)
    /// and resets the active effect's state when the panel is resized.
    pub fn render(&mut self, buffer: &mut Buffer, area: Rect, phase: f32) {
        if !self.active || area.width < 4 || area.height < 4 {
            return;
        }
        let now = Instant::now();
        let dt = self
            .last_frame
            .map(|prev| now.duration_since(prev).as_secs_f32())
            .unwrap_or(1.0 / 60.0)
            .clamp(0.0, 0.05);
        self.last_frame = Some(now);

        if self.last_size != (area.width, area.height) {
            self.last_size = (area.width, area.height);
            self.effects[self.index].reset();
        }

        let time = shader::wrap_time(phase);
        self.effects[self.index].render(buffer, area, time, dt);
    }
}

/// Snapshot the live theme ONCE per frame (never call `t()` per cell).
fn theme_snapshot() -> crate::theme::Theme {
    t().clone()
}

// ===========================================================================
// SHOWPIECE 1: TORUS KNOT -- ray-marched interlocked strands around a torus
// ===========================================================================

struct TorusKnot;

impl Effect for TorusKnot {
    fn name(&self) -> &'static str {
        "TORUS KNOT"
    }

    fn render(&mut self, buf: &mut Buffer, area: Rect, time: f32, _dt: f32) {
        let th = theme_snapshot();
        let strand_a = rgb_to_v3(color_to_rgb(th.accent1));
        let strand_b = rgb_to_v3(color_to_rgb(th.accent4));
        let fog_col = v3(0.010, 0.012, 0.022);

        // Object rotation (precomputed once per frame, captured by copy).
        // Base tilt of ~55 deg keeps the ring from ever degenerating to an
        // edge-on line.
        let (sy, cy) = (time * 0.42).sin_cos();
        let (sx, cx) = (0.95 + time * 0.27).sin_cos();
        let twist = time * 0.9;

        let to_object = move |p: Vec3| -> Vec3 {
            let (px, pz) = (cy * p.x - sy * p.z, sy * p.x + cy * p.z);
            let (py, pz) = (cx * p.y - sx * pz, sx * p.y + cx * pz);
            v3(px, py, pz)
        };

        // Two tubes offset in a cross-section frame that rotates 1.5x per
        // revolution: a trefoil-style interlocked knot. The angular domain
        // distortion breaks the Lipschitz bound, so the distance is scaled.
        let knot = move |p: Vec3| -> (f32, f32) {
            let ang = p.z.atan2(p.x);
            let cr = (p.x * p.x + p.z * p.z).sqrt() - 1.0;
            let k = 1.5 * ang + twist;
            let (s, c) = k.sin_cos();
            let qx = c * cr - s * p.y;
            let qy = s * cr + c * p.y;
            let d1 = ((qx - 0.35) * (qx - 0.35) + qy * qy).sqrt() - 0.17;
            let d2 = ((qx + 0.35) * (qx + 0.35) + qy * qy).sqrt() - 0.17;
            (d1.min(d2) * 0.55, qx)
        };

        let sdf = move |p: Vec3| -> f32 {
            let p = to_object(p);
            // Bounding sphere: cheap early-out for empty space.
            let bound = p.length() - 1.55;
            if bound > 0.25 {
                return bound;
            }
            knot(p).0
        };

        let ro = v3(0.0, 0.0, -3.0);
        const MAX_STEPS: u32 = 64;
        const BOUND_R: f32 = 1.55;

        run_half_block(buf, area, |u, v| {
            let rd = v3(u, v, 1.8).normalize();
            // Analytic bounding-sphere clip: pixels that miss it never march.
            let span = shader::ray_sphere_span(ro, rd, BOUND_R + 0.02);
            let hit = span.and_then(|(t_in, t_out)| {
                march(ro + rd * t_in, rd, &sdf, MAX_STEPS, t_out - t_in + 0.05)
                    .map(|(t, steps)| (t_in + t, steps))
            });
            match hit {
                Some((dist, steps)) => {
                    let p = ro + rd * dist;
                    let n = normal(p, &sdf);
                    // Which strand did we hit? Recompute the frame once.
                    let qx = knot(to_object(p)).1;
                    let base = if qx >= 0.0 { strand_a } else { strand_b };
                    let mat = Material {
                        base,
                        spec_power: 26.0,
                        spec_strength: 0.85,
                        rim: 0.55,
                    };
                    // Step-count AO: crevices between strands darken free.
                    let ao = (1.0 - steps as f32 / MAX_STEPS as f32 * 0.85).clamp(0.25, 1.0);
                    let col = shader::shade(n, rd, &mat, ao);
                    let col = apply_fog(col, fog_col, (dist - 2.4).max(0.0), 0.05);
                    let c = gamma_correct(col);
                    (c.x, c.y, c.z)
                }
                None => {
                    // Void: subtle vignette, tinted faintly by the theme.
                    let r2 = u * u + v * v;
                    let bg = fog_col * (1.15 - r2 * 0.22).max(0.0)
                        + mix_v3(strand_a, strand_b, clamp01(v * 0.5 + 0.5)) * 0.012;
                    let c = gamma_correct(bg);
                    (c.x, c.y, c.z)
                }
            }
        });
    }
}

// ===========================================================================
// SHOWPIECE 2: METABALLS -- gooey smooth-min blobs with wet specular
// ===========================================================================

struct Metaballs;

impl Effect for Metaballs {
    fn name(&self) -> &'static str {
        "METABALLS"
    }

    fn render(&mut self, buf: &mut Buffer, area: Rect, time: f32, _dt: f32) {
        let t5 = time;
        let centers: [Vec3; 5] = [
            v3(
                (t5 * 0.83).sin() * 0.85,
                (t5 * 0.61).cos() * 0.50,
                (t5 * 0.47).sin() * 0.45,
            ),
            v3(
                (t5 * 0.57 + 1.7).sin() * 0.80,
                (t5 * 0.90 + 0.4).sin() * 0.55,
                (t5 * 0.66).cos() * 0.40,
            ),
            v3(
                (t5 * 0.71 + 3.9).cos() * 0.75,
                (t5 * 0.44 + 2.1).sin() * 0.50,
                (t5 * 0.81 + 1.2).sin() * 0.50,
            ),
            v3(
                (t5 * 0.36 + 5.1).sin() * 0.90,
                (t5 * 0.73 + 3.3).cos() * 0.45,
                (t5 * 0.52 + 2.8).cos() * 0.42,
            ),
            v3(0.0, (t5 * 0.95).sin() * 0.25, 0.0),
        ];
        let ball_cols: [Vec3; 5] = [
            palette_spectrum(0.00 + t5 * 0.01),
            palette_spectrum(0.20 + t5 * 0.01),
            palette_spectrum(0.40 + t5 * 0.01),
            palette_spectrum(0.60 + t5 * 0.01),
            palette_spectrum(0.80 + t5 * 0.01),
        ];
        let fog_col = v3(0.012, 0.010, 0.020);

        let sdf = move |p: Vec3| -> f32 {
            let bound = p.length() - 1.9;
            if bound > 0.3 {
                return bound;
            }
            let mut d = 1e9f32;
            for c in centers {
                d = smooth_min(d, sd_sphere(p - c, 0.42), 0.35);
            }
            d
        };

        let ro = v3(0.0, 0.0, -3.1);
        const MAX_STEPS: u32 = 64;
        const BOUND_R: f32 = 1.9;

        run_half_block(buf, area, |u, v| {
            let rd = v3(u, v, 1.6).normalize();
            // Analytic bounding-sphere clip: pixels that miss it never march.
            let span = shader::ray_sphere_span(ro, rd, BOUND_R + 0.02);
            let hit = span.and_then(|(t_in, t_out)| {
                march(ro + rd * t_in, rd, &sdf, MAX_STEPS, t_out - t_in + 0.05)
                    .map(|(t, steps)| (t_in + t, steps))
            });
            match hit {
                Some((dist, steps)) => {
                    let p = ro + rd * dist;
                    let n = normal(p, &sdf);
                    // Blend albedo by inverse-square proximity to each ball.
                    let mut base = Vec3::ZERO;
                    let mut wsum = 0.0f32;
                    for (c, col) in centers.iter().zip(ball_cols.iter()) {
                        let w = 1.0 / (p - *c).length_sq().max(1e-3);
                        base = base + *col * w;
                        wsum += w;
                    }
                    let base = base * (1.0 / wsum.max(1e-6)) * 0.75;
                    let mat = Material {
                        base,
                        spec_power: 48.0,
                        spec_strength: 1.3,
                        rim: 0.7,
                    };
                    let ao = (1.0 - steps as f32 / MAX_STEPS as f32 * 0.8).clamp(0.3, 1.0);
                    let col = shader::shade(n, rd, &mat, ao);
                    let col = apply_fog(col, fog_col, (dist - 1.8).max(0.0), 0.05);
                    let c = gamma_correct(col);
                    (c.x, c.y, c.z)
                }
                None => {
                    let r2 = u * u + v * v;
                    let bg = fog_col * (1.15 - r2 * 0.25).max(0.0);
                    let c = gamma_correct(bg);
                    (c.x, c.y, c.z)
                }
            }
        });
    }
}

// ===========================================================================
// SHOWPIECE 3: TUNNEL FLIGHT -- closed-form polar tunnel fly-through
// ===========================================================================

struct Tunnel;

impl Effect for Tunnel {
    fn name(&self) -> &'static str {
        "TUNNEL FLIGHT"
    }

    fn render(&mut self, buf: &mut Buffer, area: Rect, time: f32, _dt: f32) {
        let fly = time * 1.9;
        let ox = (time * 0.50).sin() * 0.34;
        let oy = (time * 0.37).cos() * 0.22;
        let roll = time * 0.12;

        run_half_block(buf, area, |u, v| {
            let (x, y) = shader::rot2(u - ox, v - oy, roll);
            let r = (x * x + y * y).sqrt().max(1e-3);
            let a = y.atan2(x);
            let z = 0.55 / r + fly;

            // Rings racing toward the camera + slowly turning spokes.
            let rings = (0.5 + 0.5 * (z * 2.6).sin()).powf(7.0);
            let spokes = (0.5 + 0.5 * (a * 6.0 + z * 0.35).sin()).powf(3.0);
            let lattice = clamp01(rings * 0.95 + spokes * 0.30 + rings * spokes * 0.8);

            // Depth cues: the far center fades to black, walls glow nearby.
            let depth_fade = smoothstep(0.02, 0.42, r);
            let wall = clamp01(r * 1.15);

            let hue = z * 0.045 - time * 0.02;
            let col = palette_nebula(hue) * (0.10 + lattice * 1.05) * depth_fade * wall
                + palette_nebula(hue + 0.45) * (1.0 - depth_fade) * 0.05;
            let c = gamma_correct(col);
            (c.x, c.y, c.z)
        });
    }
}

// ===========================================================================
// SHOWPIECE 4: SYNTHWAVE GRID -- perspective grid, striped sun, glow horizon
// ===========================================================================

struct Synthwave;

impl Effect for Synthwave {
    fn name(&self) -> &'static str {
        "SYNTHWAVE GRID"
    }

    fn render(&mut self, buf: &mut Buffer, area: Rect, time: f32, _dt: f32) {
        const HORIZON: f32 = 0.02;
        const TAU: f32 = std::f32::consts::TAU;
        let scroll_t = time * 1.6;
        let pink = v3(1.0, 0.16, 0.62);
        let cyan = v3(0.18, 0.9, 1.0);
        let purple = v3(0.16, 0.03, 0.22);

        run_half_block(buf, area, |u, v| {
            let col = if v < HORIZON {
                // ---- ground: perspective-projected scrolling grid ----
                let depth = HORIZON - v; // > 0, grows downward
                let z = 0.28 / depth.max(1e-3);
                let x = u * z * 3.2;
                let scroll = z + scroll_t;
                // Cosine-glow grid lines (soft, anti-aliased for free); the
                // vertical fan fades out near the horizon where it would
                // alias into shimmer.
                let lx = (0.5 + 0.5 * (x * TAU).cos()).powf(16.0) * smoothstep(9.0, 3.0, z);
                let lz = (0.5 + 0.5 * (scroll * TAU).cos()).powf(16.0);
                let line = clamp01(lx + lz);
                let fade = (-z * 0.10).exp();
                let grid_col = mix_v3(pink, cyan, clamp01(x.abs() * 0.06));
                let horizon_glow = pink * (-depth * 9.0).exp() * 0.55;
                purple * 0.35 * fade + grid_col * line * fade + horizon_glow
            } else {
                // ---- sky: gradient, stars, striped sun ----
                let sky = mix_v3(
                    v3(0.10, 0.02, 0.16),
                    v3(0.01, 0.01, 0.05),
                    smoothstep(0.0, 0.95, v),
                );
                // Star field via hashed uv lattice (twinkling).
                let sx = ((u + 4.0) * 61.0) as u32;
                let sy = ((v + 2.0) * 61.0) as u32;
                let n = hash01(sx, sy, 77);
                let tw = 0.5 + 0.5 * (time * 2.1 + n * 31.0).sin();
                let star = if n > 0.985 {
                    (n - 0.985) / 0.015 * tw * 0.9
                } else {
                    0.0
                };
                // Sun disk with scanline cuts through its lower half.
                let sun_y = 0.42;
                let sd = (u * u + (v - sun_y) * (v - sun_y)).sqrt();
                let disk = smoothstep(0.30, 0.285, sd);
                let cuts = if v < sun_y {
                    let band = (v * 46.0 + time * 1.2).sin();
                    smoothstep(-0.15, 0.25, band + (sun_y - v) * 3.5)
                } else {
                    1.0
                };
                let sun_grad = mix_v3(
                    pink,
                    v3(1.0, 0.84, 0.25),
                    clamp01((v - sun_y + 0.30) / 0.60),
                );
                let sun_glow = pink * (-sd * 4.4).exp() * 0.30;
                let horizon_glow = pink * (-(v - HORIZON) * 7.0).exp() * 0.45;
                sky + v3(star, star, star) * (1.0 - disk)
                    + sun_grad * disk * cuts
                    + sun_glow
                    + horizon_glow
            };
            let c = gamma_correct(col);
            (c.x, c.y, c.z)
        });
    }
}

// ===========================================================================
// SHOWPIECE 5: JULIA SET -- animated escape-time fractal, smooth coloring
// ===========================================================================

struct Julia;

impl Effect for Julia {
    fn name(&self) -> &'static str {
        "JULIA SET"
    }

    fn render(&mut self, buf: &mut Buffer, area: Rect, time: f32, _dt: f32) {
        // Orbit c around the "seahorse valley" -- always intricate, never
        // collapses to a boring disk.
        let cr = -0.745 + 0.113 * (time * 0.31).cos();
        let ci = 0.186 + 0.095 * (time * 0.23).sin();
        let zoom = 1.30 + 0.45 * (time * 0.19).sin();
        let rot = time * 0.05;
        const MAX_ITER: u32 = 48;

        run_half_block(buf, area, |u, v| {
            let (x, y) = shader::rot2(u, v, rot);
            let mut zx = x * 1.4 / zoom;
            let mut zy = y * 1.4 / zoom;
            let mut i = 0u32;
            let mut m2 = zx * zx + zy * zy;
            while i < MAX_ITER && m2 < 16.0 {
                let nx = zx * zx - zy * zy + cr;
                zy = 2.0 * zx * zy + ci;
                zx = nx;
                m2 = zx * zx + zy * zy;
                i += 1;
            }
            if i >= MAX_ITER {
                // Interior: near-black with a faint violet breath.
                let c = gamma_correct(v3(0.010, 0.004, 0.020));
                (c.x, c.y, c.z)
            } else {
                // Smooth iteration count: kills the banding.
                let ln_z = m2.max(1.0001).ln() * 0.5;
                let mu = i as f32 + 1.0 - (ln_z / std::f32::consts::LN_2).ln() / std::f32::consts::LN_2;
                let glow = clamp01(mu / MAX_ITER as f32);
                let col = palette_spectrum(mu * 0.035 + time * 0.015) * (0.12 + glow * 1.15);
                let c = gamma_correct(col);
                (c.x, c.y, c.z)
            }
        });
    }
}

// ===========================================================================
// LEGACY 1: MATRIX RAIN (dt-normalized)
// ===========================================================================

struct MatrixColumn {
    x: u16,
    y: f32,
    speed: f32, // rows per frame at 60fps (legacy units)
    length: u16,
    chars: Vec<u8>,
    hue: f32,
}

struct MatrixRain {
    columns: Vec<MatrixColumn>,
}

impl MatrixRain {
    fn new() -> Self {
        Self {
            columns: Vec::new(),
        }
    }

    fn seed(&mut self, width: u16, height: u16) {
        let mut rng = rand::thread_rng();
        self.columns.clear();
        for x in 0..width {
            if rng.gen_range(0..3) == 0 {
                self.columns.push(Self::column(&mut rng, x, height));
            }
        }
        if self.columns.is_empty() {
            self.columns.push(Self::column(&mut rng, 0, height));
        }
    }

    fn column(rng: &mut impl Rng, x: u16, height: u16) -> MatrixColumn {
        let length = rng.gen_range(4..height.max(5));
        let mut chars = Vec::with_capacity(length as usize);
        for _ in 0..length {
            chars.push(MATRIX_CHARS[rng.gen_range(0..MATRIX_CHARS.len())]);
        }
        MatrixColumn {
            x,
            y: -(rng.gen_range(0..height) as f32),
            speed: rng.gen_range(0.3..1.8),
            length,
            chars,
            hue: rng.gen_range(0.0..360.0),
        }
    }
}

impl Effect for MatrixRain {
    fn name(&self) -> &'static str {
        "MATRIX RAIN"
    }

    fn reset(&mut self) {
        self.columns.clear();
    }

    fn render(&mut self, buf: &mut Buffer, area: Rect, time: f32, dt: f32) {
        if self.columns.is_empty() {
            self.seed(area.width, area.height);
        }
        let mut rng = rand::thread_rng();
        let frames = dt * 60.0; // legacy speeds were tuned per-frame at 60fps

        for col in &mut self.columns {
            col.y += col.speed * frames;
            col.hue = (col.hue + col.speed * 0.6 * frames) % 360.0;

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
                if col.x >= area.width {
                    continue;
                }
                let x = area.x + col.x;
                let y = area.y + row as u16;

                let fade = i as f32 / col.length as f32;
                let ch = col.chars[i as usize % col.chars.len()] as char;
                let cell_hue = (col.hue + i as f32 * 8.0 + time * 20.0) % 360.0;
                let (r, g, b) = if i == 0 {
                    (240, 255, 240)
                } else {
                    hsv_to_rgb(cell_hue, 0.9, (1.0 - fade).clamp(0.0, 1.0))
                };

                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(ch);
                    cell.set_fg(Color::Rgb(r, g, b));
                    cell.set_bg(Color::Rgb(r / 12, g / 12, b / 12));
                }
            }

            // ~6 glyph mutations per second per column (was 1-in-10 per frame).
            if rng.gen::<f32>() < (dt * 6.0).min(1.0) {
                let idx = rng.gen_range(0..col.chars.len());
                col.chars[idx] = MATRIX_CHARS[rng.gen_range(0..MATRIX_CHARS.len())];
            }
        }
    }
}

// ===========================================================================
// LEGACY 2: PLASMA FIELD (half-block upgrade)
// ===========================================================================

struct Plasma;

impl Effect for Plasma {
    fn name(&self) -> &'static str {
        "PLASMA FIELD"
    }

    fn render(&mut self, buf: &mut Buffer, area: Rect, time: f32, _dt: f32) {
        let wx = (time * 0.33).sin() * 1.4;
        let wy = (time * 0.41).cos() * 1.4;

        run_half_block(buf, area, |u, v| {
            let x = u * 2.3;
            let y = v * 2.3;
            let v1 = (x * 1.7 + time * 1.3).sin();
            let v2 = ((x * 1.1).cos() + (y * 1.5 + time * 0.8).sin()) * 0.5;
            let dx = x + wx;
            let dy = y + wy;
            let v3n = ((dx * dx + dy * dy).sqrt() * 1.8 - time * 1.6).sin();
            let v4 = (x * 0.9 + time).sin() * (y * 1.2 - time * 0.7).cos();
            let val = (v1 + v2 + v3n + v4) / 4.0;

            let col = palette_spectrum(val * 0.55 + time * 0.04) * (0.55 + 0.45 * (val * 0.5 + 0.5));
            let c = gamma_correct(col);
            (c.x, c.y, c.z)
        });
    }
}

// ===========================================================================
// LEGACY 3: 3D STARFIELD (dt-normalized, negative-cull fixed)
// ===========================================================================

struct Star {
    x: f32,
    y: f32,
    z: f32,
}

struct Starfield {
    stars: Vec<Star>,
}

impl Starfield {
    fn new() -> Self {
        Self { stars: Vec::new() }
    }
}

impl Effect for Starfield {
    fn name(&self) -> &'static str {
        "3D STARFIELD"
    }

    fn reset(&mut self) {
        self.stars.clear();
    }

    fn render(&mut self, buf: &mut Buffer, area: Rect, _time: f32, dt: f32) {
        if self.stars.is_empty() {
            let mut rng = rand::thread_rng();
            for _ in 0..200 {
                self.stars.push(Star {
                    x: rng.gen_range(-1.0..1.0),
                    y: rng.gen_range(-1.0..1.0),
                    z: rng.gen_range(0.1..1.0),
                });
            }
        }

        let cx = area.width as f32 / 2.0;
        let cy = area.height as f32 / 2.0;

        for star in &mut self.stars {
            star.z -= 0.72 * dt; // was 0.012/frame at 60fps
            if star.z <= 0.01 {
                let mut rng = rand::thread_rng();
                star.x = rng.gen_range(-1.0..1.0);
                star.y = rng.gen_range(-1.0..1.0);
                star.z = 1.0;
            }

            let sx = (star.x / star.z) * cx + cx;
            let sy = (star.y / star.z) * cy + cy;

            // Cull in f32 BEFORE casting: negatives must never smear to 0.
            if sx < 0.0 || sy < 0.0 || sx >= area.width as f32 || sy >= area.height as f32 {
                continue;
            }
            let px = area.x + sx as u16;
            let py = area.y + sy as u16;

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

            if let Some(cell) = buf.cell_mut((px, py)) {
                cell.set_char(ch);
                cell.set_fg(Color::Rgb(
                    brightness,
                    brightness,
                    (brightness as f32 * 0.9) as u8,
                ));
                cell.set_bg(Color::Rgb(0, 0, (brightness / 20).min(15)));
            }
        }
    }
}

// ===========================================================================
// LEGACY 4: WIREFRAME 3D (half-block glow-line upgrade)
// ===========================================================================

struct WireframeCube {
    canvas: Canvas,
}

impl WireframeCube {
    fn new() -> Self {
        Self {
            canvas: Canvas::new(),
        }
    }
}

impl Effect for WireframeCube {
    fn name(&self) -> &'static str {
        "WIREFRAME 3D"
    }

    fn render(&mut self, buf: &mut Buffer, area: Rect, time: f32, _dt: f32) {
        let th = theme_snapshot();
        let accents = [
            rgb_to_v3(color_to_rgb(th.accent3)),
            rgb_to_v3(color_to_rgb(th.accent4)),
            rgb_to_v3(color_to_rgb(th.accent1)),
            rgb_to_v3(color_to_rgb(th.accent2)),
        ];
        let vertex_col = rgb_to_v3(color_to_rgb(th.text));

        self.canvas.begin(area, v3(0.004, 0.006, 0.012));
        let w = self.canvas.width() as f32;
        let h = self.canvas.height() as f32;
        let cx = w / 2.0;
        let cy = h / 2.0;
        // Canvas pixels are square: no cell-aspect fudge needed here.
        let scale = (w.min(h) * 0.30).max(4.0);

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
            (0, 1),
            (1, 2),
            (2, 3),
            (3, 0),
            (4, 5),
            (5, 6),
            (6, 7),
            (7, 4),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ];

        let ax = time * 0.7;
        let ay = time * 1.1;
        let az = time * 0.4;
        let projected: Vec<(f32, f32, f32)> = vertices
            .iter()
            .map(|&(x, y, z)| {
                let (x1, y1) = (x * ay.cos() - z * ay.sin(), x * ay.sin() + z * ay.cos());
                let z1 = y1;
                let (y2, z2) = (y * ax.cos() - z1 * ax.sin(), y * ax.sin() + z1 * ax.cos());
                let (x2, y3) = (x1 * az.cos() - y2 * az.sin(), x1 * az.sin() + y2 * az.cos());
                let depth = z2 + 3.0;
                let perspective = 2.0 / depth.max(0.5);
                (
                    x2 * perspective * scale + cx,
                    y3 * perspective * scale + cy,
                    depth,
                )
            })
            .collect();

        for (i, &(a, b)) in edges.iter().enumerate() {
            let (x0, y0, d0) = projected[a];
            let (x1, y1, d1) = projected[b];
            let base = accents[i % 4];
            let b0 = (2.4 - d0 * 0.5).clamp(0.35, 1.0);
            let b1 = (2.4 - d1 * 0.5).clamp(0.35, 1.0);
            self.canvas.line(x0, y0, x1, y1, base * b0, base * b1);
        }
        for &(sx, sy, depth) in &projected {
            let glow = (2.6 - depth * 0.55).clamp(0.4, 1.2);
            self.canvas.splat(sx, sy, vertex_col * glow);
        }

        self.canvas.blit(buf, area);
    }
}

// ===========================================================================
// LEGACY 5: FIRE SIM (fixed-rate accumulator stepping)
// ===========================================================================

struct Fire {
    grid: Vec<Vec<f32>>,
    acc: f32,
}

impl Fire {
    fn new() -> Self {
        Self {
            grid: Vec::new(),
            acc: 0.0,
        }
    }

    fn step(&mut self, rng: &mut impl Rng) {
        let h = self.grid.len();
        if h < 2 {
            return;
        }
        let w = self.grid[0].len();
        let bottom = h - 1;
        for x in 0..w {
            self.grid[bottom][x] = rng.gen_range(0.6..1.0);
        }
        for y in 0..bottom {
            for x in 0..w {
                let left = if x > 0 { self.grid[y + 1][x - 1] } else { 0.0 };
                let center = self.grid[y + 1][x];
                let right = if x + 1 < w { self.grid[y + 1][x + 1] } else { 0.0 };
                let below = if y + 2 < h { self.grid[y + 2][x] } else { center };
                self.grid[y][x] = ((left + center + right + below) / 4.04).max(0.0);
            }
        }
    }
}

impl Effect for Fire {
    fn name(&self) -> &'static str {
        "FIRE SIM"
    }

    fn reset(&mut self) {
        self.grid.clear();
        self.acc = 0.0;
    }

    fn render(&mut self, buf: &mut Buffer, area: Rect, _time: f32, dt: f32) {
        let w = area.width as usize;
        let h = area.height as usize;
        let mut rng = rand::thread_rng();
        if self.grid.len() != h || self.grid.first().map_or(true, |row| row.len() != w) {
            self.grid = vec![vec![0.0; w]; h];
            self.acc = 0.0;
            // Prime the bottom row so the very first frame already burns.
            self.step(&mut rng);
        }

        // Advance the sim at a fixed 45Hz regardless of frame rate.
        self.acc += dt;
        const STEP: f32 = 1.0 / 45.0;
        let mut steps = 0;
        while self.acc >= STEP && steps < 4 {
            self.step(&mut rng);
            self.acc -= STEP;
            steps += 1;
        }
        if steps == 4 {
            self.acc = 0.0;
        }

        for y in 0..h {
            for x in 0..w {
                let intensity = self.grid[y][x].clamp(0.0, 1.0);
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
                if let Some(cell) = buf.cell_mut((area.x + x as u16, area.y + y as u16)) {
                    cell.set_char(ch);
                    cell.set_fg(Color::Rgb(r, g, b));
                    cell.set_bg(Color::Rgb(r / 6, g / 8, 0));
                }
            }
        }
    }
}

// ===========================================================================
// LEGACY 6: PARTICLE STORM (dt-normalized)
// ===========================================================================

struct Particle {
    x: f32,
    y: f32,
    vx: f32, // cells per frame at 60fps (legacy units)
    vy: f32,
    life: f32,
    color: (u8, u8, u8),
}

struct Particles {
    particles: Vec<Particle>,
}

impl Particles {
    fn new() -> Self {
        Self {
            particles: Vec::new(),
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

impl Effect for Particles {
    fn name(&self) -> &'static str {
        "PARTICLE STORM"
    }

    fn reset(&mut self) {
        self.particles.clear();
    }

    fn render(&mut self, buf: &mut Buffer, area: Rect, _time: f32, dt: f32) {
        let mut rng = rand::thread_rng();
        if self.particles.is_empty() {
            for _ in 0..120 {
                self.particles
                    .push(new_particle(&mut rng, area.width, area.height));
            }
        }

        let frames = dt * 60.0;
        for p in &mut self.particles {
            p.x += p.vx * frames;
            p.y += p.vy * frames;
            p.vy += 0.03 * frames; // gravity
            p.life -= 0.015 * frames;

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
            // Positions are guaranteed in-bounds (respawn above), but keep
            // the i32 cull as a safety net against float drift.
            let ix = p.x as i32;
            let iy = p.y as i32;
            if ix < 0 || iy < 0 || ix >= area.width as i32 || iy >= area.height as i32 {
                continue;
            }
            let px = area.x + ix as u16;
            let py = area.y + iy as u16;
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
            if let Some(cell) = buf.cell_mut((px, py)) {
                cell.set_char(ch);
                cell.set_fg(Color::Rgb(r, g, b));
            }
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn render_frame(engine: &mut EffectsEngine, area: Rect, phase: f32) -> Buffer {
        let mut buf = Buffer::empty(area);
        engine.render(&mut buf, area, phase);
        buf
    }

    fn buffer_has_content(buf: &Buffer, area: Rect) -> bool {
        for y in 0..area.height {
            for x in 0..area.width {
                let cell = buf.cell((x, y)).unwrap();
                if cell.symbol() != " " || cell.fg != Color::Reset || cell.bg != Color::Reset {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn registry_order_and_names() {
        let engine = EffectsEngine::new();
        let names = engine.names();
        assert_eq!(names.len(), 11);
        assert_eq!(
            names,
            vec![
                "TORUS KNOT",
                "METABALLS",
                "TUNNEL FLIGHT",
                "SYNTHWAVE GRID",
                "JULIA SET",
                "MATRIX RAIN",
                "PLASMA FIELD",
                "3D STARFIELD",
                "WIREFRAME 3D",
                "FIRE SIM",
                "PARTICLE STORM",
            ]
        );
        assert!(!engine.active);
        assert_eq!(engine.current_name(), "TORUS KNOT");
    }

    #[test]
    fn cycle_ring_reaches_off_after_last_effect() {
        let mut engine = EffectsEngine::new();
        let count = engine.names().len();
        engine.cycle_with_off();
        assert!(engine.active);
        assert_eq!(engine.current_name(), "TORUS KNOT");
        for _ in 1..count {
            engine.cycle_with_off();
            assert!(engine.active);
        }
        assert_eq!(engine.current_name(), "PARTICLE STORM");
        engine.cycle_with_off();
        assert!(!engine.active, "cycling past the last effect turns off");
        engine.cycle_with_off();
        assert!(engine.active);
        assert_eq!(
            engine.current_name(),
            "TORUS KNOT",
            "ring restarts at the top"
        );
    }

    #[test]
    fn set_by_name_matches_loosely() {
        let mut engine = EffectsEngine::new();
        assert!(engine.set_by_name("PLASMA FIELD"));
        assert_eq!(engine.current_name(), "PLASMA FIELD");
        assert!(engine.active);
        assert!(engine.set_by_name("julia"));
        assert_eq!(engine.current_name(), "JULIA SET");
        assert!(engine.set_by_name("knot")); // substring
        assert_eq!(engine.current_name(), "TORUS KNOT");
        assert!(!engine.set_by_name("does-not-exist"));
        assert!(!engine.set_by_name("  "));
        assert_eq!(
            engine.current_name(),
            "TORUS KNOT",
            "failed lookup keeps selection"
        );
    }

    #[test]
    fn inactive_engine_renders_nothing() {
        let mut engine = EffectsEngine::new();
        let area = Rect::new(0, 0, 40, 20);
        let buf = render_frame(&mut engine, area, 0.5);
        assert!(!buffer_has_content(&buf, area));
        // Tiny areas are skipped even when active.
        engine.active = true;
        let tiny = Rect::new(0, 0, 3, 3);
        let mut buf = Buffer::empty(tiny);
        engine.render(&mut buf, tiny, 0.5);
        assert!(!buffer_has_content(&buf, tiny));
    }

    #[test]
    fn every_effect_renders_and_animates() {
        let mut engine = EffectsEngine::new();
        let area = Rect::new(0, 0, 60, 24);
        let count = engine.names().len();
        for i in 0..count {
            let name = engine.names()[i];
            assert!(engine.set_by_name(name), "set_by_name({name})");
            let a = render_frame(&mut engine, area, 0.0);
            assert!(buffer_has_content(&a, area), "{name} wrote nothing");
            // dt-driven effects need real elapsed time between frames.
            let mut b = render_frame(&mut engine, area, 0.25);
            for step in 0..4 {
                std::thread::sleep(std::time::Duration::from_millis(15));
                b = render_frame(&mut engine, area, 0.5 + step as f32 * 0.25);
            }
            assert!(buffer_has_content(&b, area), "{name} wrote nothing at t>0");
            assert_ne!(
                a.content(),
                b.content(),
                "{name} must animate over time"
            );
        }
    }

    #[test]
    fn resize_does_not_panic_or_leak_state() {
        let mut engine = EffectsEngine::new();
        for name in ["MATRIX RAIN", "FIRE SIM", "PARTICLE STORM", "3D STARFIELD"] {
            assert!(engine.set_by_name(name));
            let big = Rect::new(0, 0, 80, 30);
            let small = Rect::new(0, 0, 12, 6);
            let _ = render_frame(&mut engine, big, 0.1);
            let _ = render_frame(&mut engine, small, 0.2);
            let _ = render_frame(&mut engine, big, 0.3);
        }
    }

    /// Perf sanity: one frame of every effect on a 100x40 panel. Generous
    /// debug budget -- this only catches pathological regressions. Release
    /// must stay well under the ~8ms frame budget.
    #[test]
    fn timing_sanity_all_effects() {
        let budget_ms: f64 = if cfg!(debug_assertions) { 30.0 } else { 8.0 };
        let mut engine = EffectsEngine::new();
        let area = Rect::new(0, 0, 100, 40);
        let count = engine.names().len();
        for i in 0..count {
            let name = engine.names()[i];
            assert!(engine.set_by_name(name));
            // Warm-up frame (allocations, lazy seeding).
            let _ = render_frame(&mut engine, area, 0.0);
            let start = Instant::now();
            let _ = render_frame(&mut engine, area, 0.5);
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            println!("{name}: {elapsed:.2}ms"); // visible with -- --nocapture
            assert!(
                elapsed < budget_ms,
                "{name} took {elapsed:.2}ms (budget {budget_ms}ms)"
            );
        }
    }
}
