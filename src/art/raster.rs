//! Stage 2.5 and 3 of the pipeline: a sub-cell `Raster` that every scene is
//! drawn into, and the per-`ThemeStyle` rasterizers that turn it into glyph
//! cells with color.

use super::render3d::{self, MeshRenderOpts};
use super::{ArtCell, Scene};
use crate::theme::{color_to_rgb, hsl_to_rgb, ramp_glyph, Theme, ThemeStyle};

/// Stroke direction tags for glyph selection in vector-first styles.
pub const DIR_NONE: u8 = 0;
pub const DIR_H: u8 = 1;
pub const DIR_V: u8 = 2;
pub const DIR_DIAG_DOWN: u8 = 3; // like '\'
pub const DIR_DIAG_UP: u8 = 4; // like '/'
pub const DIR_CROSS: u8 = 5;

/// Sub-samples per cell for a style: braille packs 2x4 dots, half-blocks 1x2.
pub fn subcell(style: ThemeStyle) -> (usize, usize) {
    match style {
        ThemeStyle::Braille => (2, 4),
        ThemeStyle::Blocks => (1, 2),
        _ => (1, 1),
    }
}

#[derive(Clone, Debug)]
pub struct Raster {
    pub w: usize,
    pub h: usize,
    pub intensity: Vec<f32>,
    /// Hue in degrees, or negative for "no hue, use the theme gradient".
    pub hue: Vec<f32>,
    /// Depth from the camera, `INFINITY` where nothing 3D was drawn.
    pub depth: Vec<f32>,
    pub dir: Vec<u8>,
}

impl Raster {
    pub fn new(w: usize, h: usize) -> Self {
        let n = w * h;
        Self {
            w,
            h,
            intensity: vec![0.0; n],
            hue: vec![-1.0; n],
            depth: vec![f32::INFINITY; n],
            dir: vec![DIR_NONE; n],
        }
    }

    #[inline]
    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.w + x
    }


    /// Additive plot with max-merge on intensity.
    pub fn plot(&mut self, x: usize, y: usize, intensity: f32, hue: Option<f32>, dir: u8) {
        if x >= self.w || y >= self.h {
            return;
        }
        let i = self.idx(x, y);
        if intensity >= self.intensity[i] {
            self.intensity[i] = intensity.clamp(0.0, 1.0);
            if let Some(h) = hue {
                self.hue[i] = h;
            }
        }
        self.dir[i] = match (self.dir[i], dir) {
            (DIR_NONE, d) => d,
            (d, DIR_NONE) => d,
            (a, b) if a == b => a,
            _ => DIR_CROSS,
        };
    }

    /// Z-tested plot for filled 3D surfaces: nearer wins.
    pub fn plot_depth(&mut self, x: usize, y: usize, intensity: f32, hue: Option<f32>, depth: f32) {
        if x >= self.w || y >= self.h {
            return;
        }
        let i = self.idx(x, y);
        if depth < self.depth[i] {
            self.depth[i] = depth;
            self.intensity[i] = intensity.clamp(0.0, 1.0);
            if let Some(h) = hue {
                self.hue[i] = h;
            }
        }
    }

    /// Draw a line in sample coordinates. `depths` tags each sample with an
    /// interpolated depth so 3D wireframes can be depth-shaded.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_segment(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        weight: f32,
        hue: Option<f32>,
        depths: Option<(f32, f32)>,
    ) {
        let dx = x1 - x0;
        let dy = y1 - y0;
        let steps = dx.abs().max(dy.abs()).ceil().max(1.0) as usize;
        let dir = if dx.abs() > dy.abs() * 2.0 {
            DIR_H
        } else if dy.abs() > dx.abs() * 2.0 {
            DIR_V
        } else if (dx > 0.0) == (dy > 0.0) {
            DIR_DIAG_DOWN
        } else {
            DIR_DIAG_UP
        };
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            let x = x0 + dx * t;
            let y = y0 + dy * t;
            if x < 0.0 || y < 0.0 {
                continue;
            }
            let (xi, yi) = (x as usize, y as usize);
            if xi >= self.w || yi >= self.h {
                continue;
            }
            self.plot(xi, yi, weight, hue, dir);
            if let Some((d0, d1)) = depths {
                let d = d0 + (d1 - d0) * t;
                let i = self.idx(xi, yi);
                if d < self.depth[i] {
                    self.depth[i] = d;
                }
            }
        }
    }

    /// Nearest-neighbour resample into a new size.
    pub fn resampled(&self, w: usize, h: usize) -> Raster {
        if w == self.w && h == self.h {
            return self.clone();
        }
        let mut out = Raster::new(w, h);
        for y in 0..h {
            let sy = (y * self.h / h.max(1)).min(self.h.saturating_sub(1));
            for x in 0..w {
                let sx = (x * self.w / w.max(1)).min(self.w.saturating_sub(1));
                let src = self.idx(sx, sy);
                let dst = out.idx(x, y);
                out.intensity[dst] = self.intensity[src];
                out.hue[dst] = self.hue[src];
                out.depth[dst] = self.depth[src];
                out.dir[dst] = self.dir[src];
            }
        }
        out
    }
}

/// Draw a scene into a raster of `fw` x `fh` samples.
pub fn scene_to_raster(scene: &Scene, fw: usize, fh: usize, sx: usize, sy: usize, style: ThemeStyle) -> Raster {
    match scene {
        Scene::Field { w, h, intensity, hue } => {
            let mut r = Raster::new(*w, *h);
            r.intensity = intensity.iter().map(|v| v.clamp(0.0, 1.0)).collect();
            if let Some(hue) = hue {
                r.hue = hue.clone();
            }
            r.resampled(fw, fh)
        }
        Scene::Strokes { w, h, segments } => {
            let mut r = Raster::new(fw, fh);
            let kx = fw as f32 / w.max(1e-3);
            let ky = fh as f32 / h.max(1e-3);
            for s in segments {
                r.draw_segment(s.x0 * kx, s.y0 * ky, s.x1 * kx, s.y1 * ky, s.weight, s.hue, None);
            }
            r
        }
        Scene::Mesh { mesh, rotation, cull_backfaces, base_hue } => {
            let mut r = Raster::new(fw, fh);
            let filled = mesh.faces.is_some() && style_fills_meshes(style);
            let opts = MeshRenderOpts {
                rotation: *rotation,
                filled,
                cull_backfaces: *cull_backfaces,
                base_hue: *base_hue,
                ..Default::default()
            };
            render3d::render_mesh(mesh, &opts, &mut r, sx, sy);
            r
        }
    }
}

/// Vector-first styles show meshes as wireframes; the rest fill faces.
pub fn style_fills_meshes(style: ThemeStyle) -> bool {
    !matches!(style, ThemeStyle::Classic | ThemeStyle::Wireframe | ThemeStyle::Minimal)
}

// ---------------------------------------------------------------------------
// Colour helpers
// ---------------------------------------------------------------------------

fn lerp_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    (
        (a.0 as f32 + (b.0 as f32 - a.0 as f32) * t).round() as u8,
        (a.1 as f32 + (b.1 as f32 - a.1 as f32) * t).round() as u8,
        (a.2 as f32 + (b.2 as f32 - a.2 as f32) * t).round() as u8,
    )
}

fn scale_rgb(c: (u8, u8, u8), k: f32) -> (u8, u8, u8) {
    (
        (c.0 as f32 * k).clamp(0.0, 255.0) as u8,
        (c.1 as f32 * k).clamp(0.0, 255.0) as u8,
        (c.2 as f32 * k).clamp(0.0, 255.0) as u8,
    )
}

/// Theme gradient for samples without a hue: dark -> accent3 -> accent2.
fn gradient_color(theme: &Theme, t: f32) -> (u8, u8, u8) {
    let low = scale_rgb(color_to_rgb(theme.accent3), 0.45);
    let mid = color_to_rgb(theme.accent3);
    let high = color_to_rgb(theme.accent2);
    if t < 0.5 {
        lerp_rgb(low, mid, t * 2.0)
    } else {
        lerp_rgb(mid, high, (t - 0.5) * 2.0)
    }
}

/// Final colour for a sample. Hue-carrying samples get a saturated HSL colour
/// whose lightness follows intensity; Minimal ignores hue on purpose.
fn sample_color(theme: &Theme, intensity: f32, hue: f32) -> (u8, u8, u8) {
    let i = intensity.clamp(0.0, 1.0);
    match theme.style {
        ThemeStyle::Minimal => {
            let a = color_to_rgb(theme.muted);
            let b = color_to_rgb(theme.accent2);
            lerp_rgb(a, b, i)
        }
        _ if hue >= 0.0 => {
            let (sat, light) = match theme.style {
                ThemeStyle::Blocks => (0.95, 0.22 + 0.42 * i),
                ThemeStyle::Braille => (0.85, 0.30 + 0.40 * i),
                ThemeStyle::Wireframe => (0.70, 0.30 + 0.35 * i),
                ThemeStyle::Depth3D => (0.90, 0.20 + 0.45 * i),
                _ => (0.80, 0.25 + 0.45 * i),
            };
            hsl_to_rgb(hue, sat, light)
        }
        _ => gradient_color(theme, i),
    }
}

fn depth_range(r: &Raster) -> Option<(f32, f32)> {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for &d in &r.depth {
        if d.is_finite() {
            lo = lo.min(d);
            hi = hi.max(d);
        }
    }
    if lo.is_finite() && hi > lo {
        Some((lo, hi))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Raster -> cells, per style
// ---------------------------------------------------------------------------

pub fn raster_to_cells(r: &Raster, theme: &Theme, w: usize, h: usize) -> Vec<ArtCell> {
    match theme.style {
        ThemeStyle::Braille => braille_cells(r, theme, w, h),
        ThemeStyle::Blocks => blocks_cells(r, theme, w, h),
        ThemeStyle::Classic | ThemeStyle::Minimal => glyph_cells(r, theme, w, h, false),
        ThemeStyle::Wireframe => glyph_cells(r, theme, w, h, true),
        ThemeStyle::Depth3D => depth_cells(r, theme, w, h),
    }
}

fn dir_glyph(dir: u8, box_drawing: bool) -> Option<char> {
    let g = match (dir, box_drawing) {
        (DIR_H, false) => '-',
        (DIR_V, false) => '|',
        (DIR_DIAG_DOWN, false) => '\\',
        (DIR_DIAG_UP, false) => '/',
        (DIR_CROSS, false) => '+',
        (DIR_H, true) => '─',
        (DIR_V, true) => '│',
        (DIR_DIAG_DOWN, true) => '╲',
        (DIR_DIAG_UP, true) => '╱',
        (DIR_CROSS, true) => '┼',
        _ => return None,
    };
    Some(g)
}

/// One sample per cell: intensity -> ramp glyph, stroke direction -> line glyph.
fn glyph_cells(r: &Raster, theme: &Theme, w: usize, h: usize, box_drawing: bool) -> Vec<ArtCell> {
    let r = r.resampled(w, h);
    let mut cells = Vec::with_capacity(w * h);
    let bg_dark = scale_rgb(color_to_rgb(theme.panel_bg), 1.0);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let intensity = r.intensity[i];
            if intensity <= 0.02 {
                cells.push(ArtCell::BLANK);
                continue;
            }
            let ch = match dir_glyph(r.dir[i], box_drawing) {
                Some(g) if theme.style != ThemeStyle::Minimal || intensity > 0.3 => g,
                _ => theme.ramp_glyph(intensity),
            };
            let fg = sample_color(theme, intensity, r.hue[i]);
            let bg = if theme.style == ThemeStyle::Wireframe && intensity > 0.6 {
                Some(lerp_rgb(bg_dark, fg, 0.12))
            } else {
                None
            };
            cells.push(ArtCell { ch, fg, bg });
        }
    }
    cells
}

/// Depth3D: glyph density follows intensity (lighting), colour follows depth
/// (near = accent2 warm, far = accent3 cool). For 2D fields with no depth,
/// a fake light from the left produces a relief look.
fn depth_cells(r: &Raster, theme: &Theme, w: usize, h: usize) -> Vec<ArtCell> {
    let r = r.resampled(w, h);
    let range = depth_range(&r);
    let near_color = color_to_rgb(theme.accent2);
    let far_color = color_to_rgb(theme.accent3);
    let rim = color_to_rgb(theme.accent4);
    let mut cells = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let intensity = r.intensity[i];
            let depth = r.depth[i];
            let (glyph_i, mut fg) = if let (Some((lo, hi)), true) = (range, depth.is_finite()) {
                let near = 1.0 - ((depth - lo) / (hi - lo)).clamp(0.0, 1.0);
                // Density = lighting * a little depth emphasis.
                let g = (intensity * (0.55 + 0.45 * near)).clamp(0.0, 1.0);
                let base = lerp_rgb(far_color, near_color, near);
                (g, scale_rgb(base, 0.45 + 0.55 * intensity))
            } else if intensity > 0.02 {
                // Relief shading for flat fields: slope to the left brightens.
                let left = if x > 0 { r.intensity[i - 1] } else { intensity };
                let slope = (intensity - left).clamp(-0.5, 0.5);
                let lit = (intensity + slope * 1.5).clamp(0.05, 1.0);
                let base = if r.hue[i] >= 0.0 {
                    sample_color(theme, lit, r.hue[i])
                } else {
                    lerp_rgb(far_color, near_color, lit)
                };
                (lit, base)
            } else {
                cells.push(ArtCell::BLANK);
                continue;
            };
            if r.dir[i] != DIR_NONE && glyph_i > 0.5 {
                fg = lerp_rgb(fg, rim, 0.3);
            }
            let ch = ramp_glyph(theme.ramp, glyph_i);
            if ch == ' ' {
                cells.push(ArtCell::BLANK);
            } else {
                cells.push(ArtCell {
                    ch,
                    fg,
                    bg: Some(scale_rgb(fg, 0.12)),
                });
            }
        }
    }
    cells
}

/// Blocks: two samples per cell (top/bottom). Solid poster look using half
/// blocks with independent fg/bg colours, plus shade glyphs when both halves
/// carry the same intensity.
fn blocks_cells(r: &Raster, theme: &Theme, w: usize, h: usize) -> Vec<ArtCell> {
    let r = r.resampled(w, h * 2);
    let range = depth_range(&r);
    let mut cells = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let top = y * 2 * w + x;
            let bot = (y * 2 + 1) * w + x;
            let (it, ib) = (r.intensity[top], r.intensity[bot]);
            let color_of = |idx: usize, intensity: f32| {
                let mut c = sample_color(theme, intensity, r.hue[idx]);
                if let Some((lo, hi)) = range {
                    if r.depth[idx].is_finite() {
                        let near = 1.0 - ((r.depth[idx] - lo) / (hi - lo)).clamp(0.0, 1.0);
                        c = scale_rgb(c, 0.6 + 0.4 * near);
                    }
                }
                c
            };
            let on_t = it > 0.04;
            let on_b = ib > 0.04;
            let cell = match (on_t, on_b) {
                (false, false) => ArtCell::BLANK,
                (true, false) => ArtCell {
                    ch: '▀',
                    fg: color_of(top, it),
                    bg: None,
                },
                (false, true) => ArtCell {
                    ch: '▄',
                    fg: color_of(bot, ib),
                    bg: None,
                },
                (true, true) => {
                    let same_hue = (r.hue[top] - r.hue[bot]).abs() < 12.0;
                    if (it - ib).abs() < 0.12 && same_hue {
                        // Shade glyph by intensity, on a dimmed bg of the same colour.
                        let avg = (it + ib) / 2.0;
                        let fg = color_of(top, avg);
                        let ch = ramp_glyph(theme.ramp, 0.35 + 0.65 * avg);
                        ArtCell {
                            ch: if ch == ' ' { '░' } else { ch },
                            fg,
                            bg: Some(scale_rgb(fg, 0.25)),
                        }
                    } else {
                        ArtCell {
                            ch: '▀',
                            fg: color_of(top, it),
                            bg: Some(color_of(bot, ib)),
                        }
                    }
                }
            };
            cells.push(cell);
        }
    }
    cells
}

/// Ordered-dither thresholds for a 2x4 braille cell, column-major.
const BAYER_2X4: [[f32; 2]; 4] = [
    [0.0625, 0.5625],
    [0.8125, 0.3125],
    [0.1875, 0.6875],
    [0.9375, 0.4375],
];

/// Braille: eight dots per cell via ordered dithering, so intensity gradients
/// become dot density. Bit layout for U+2800+n: dots 1-3 are bits 0-2 (left
/// column, top to bottom), dots 4-6 are bits 3-5 (right column), dot 7 is bit
/// 6 (bottom-left), dot 8 is bit 7 (bottom-right).
fn braille_cells(r: &Raster, theme: &Theme, w: usize, h: usize) -> Vec<ArtCell> {
    let r = r.resampled(w * 2, h * 4);
    let range = depth_range(&r);
    let rw = r.w;
    let mut cells = Vec::with_capacity(w * h);
    for cy in 0..h {
        for cx in 0..w {
            let mut bits: u8 = 0;
            let mut sum_i = 0.0;
            let mut sum_hue = 0.0;
            let mut hue_n = 0;
            let mut sum_near = 0.0;
            let mut on = 0;
            for (col, row, threshold) in BAYER_2X4
                .iter()
                .enumerate()
                .flat_map(|(row, cols)| cols.iter().enumerate().map(move |(col, t)| (col, row, *t)))
            {
                {
                    let x = cx * 2 + col;
                    let y = cy * 4 + row;
                    let idx = y * rw + x;
                    let v = r.intensity[idx];
                    if v > threshold * 0.98 {
                        let bit = match (col, row) {
                            (0, 3) => 6,
                            (1, 3) => 7,
                            (c, rr) => c * 3 + rr,
                        };
                        bits |= 1 << bit;
                        on += 1;
                        sum_i += v;
                        if r.hue[idx] >= 0.0 {
                            sum_hue += r.hue[idx];
                            hue_n += 1;
                        }
                        if let Some((lo, hi)) = range {
                            if r.depth[idx].is_finite() {
                                sum_near += 1.0 - ((r.depth[idx] - lo) / (hi - lo)).clamp(0.0, 1.0);
                            }
                        }
                    }
                }
            }
            if bits == 0 {
                cells.push(ArtCell::BLANK);
                continue;
            }
            let mean_i = (sum_i / on as f32).max(0.3);
            let hue = if hue_n > 0 { sum_hue / hue_n as f32 } else { -1.0 };
            let mut fg = sample_color(theme, mean_i, hue);
            if range.is_some() {
                let near = sum_near / on as f32;
                fg = scale_rgb(fg, 0.55 + 0.45 * near);
            }
            cells.push(ArtCell {
                ch: char::from_u32(0x2800 + bits as u32).unwrap_or('⠿'),
                fg,
                bg: None,
            });
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::art::Segment2;

    #[test]
    fn braille_bit_layout_matches_unicode() {
        // A raster with only the top-left dot lit must produce U+2801.
        let mut r = Raster::new(2, 4);
        r.intensity[0] = 1.0;
        let theme = Theme::for_style(ThemeStyle::Braille);
        let cells = raster_to_cells(&r, &theme, 1, 1);
        assert_eq!(cells[0].ch, '\u{2801}');

        // Bottom-right dot only -> dot 8 -> bit 7 -> U+2880.
        let mut r = Raster::new(2, 4);
        r.intensity[3 * 2 + 1] = 1.0;
        let cells = raster_to_cells(&r, &theme, 1, 1);
        assert_eq!(cells[0].ch, '\u{2880}');

        // Everything lit -> U+28FF.
        let mut r = Raster::new(2, 4);
        r.intensity.iter_mut().for_each(|v| *v = 1.0);
        let cells = raster_to_cells(&r, &theme, 1, 1);
        assert_eq!(cells[0].ch, '\u{28FF}');
    }

    #[test]
    fn blocks_pick_half_block_glyphs() {
        let theme = Theme::for_style(ThemeStyle::Blocks);
        let mut r = Raster::new(1, 2);
        r.intensity[0] = 1.0;
        assert_eq!(raster_to_cells(&r, &theme, 1, 1)[0].ch, '▀');
        let mut r = Raster::new(1, 2);
        r.intensity[1] = 1.0;
        assert_eq!(raster_to_cells(&r, &theme, 1, 1)[0].ch, '▄');
        let mut r = Raster::new(1, 2);
        r.intensity[0] = 1.0;
        r.intensity[1] = 0.2;
        let c = raster_to_cells(&r, &theme, 1, 1)[0];
        assert_eq!(c.ch, '▀');
        assert!(c.bg.is_some());
    }

    #[test]
    fn strokes_get_direction_glyphs() {
        let scene = Scene::Strokes {
            w: 10.0,
            h: 10.0,
            segments: vec![
                Segment2 { x0: 0.0, y0: 5.0, x1: 10.0, y1: 5.0, weight: 1.0, hue: None },
                Segment2 { x0: 5.0, y0: 0.0, x1: 5.0, y1: 10.0, weight: 1.0, hue: None },
            ],
        };
        let r = scene_to_raster(&scene, 10, 10, 1, 1, ThemeStyle::Wireframe);
        let cells = raster_to_cells(&r, &Theme::for_style(ThemeStyle::Wireframe), 10, 10);
        assert_eq!(cells[5 * 10 + 1].ch, '─');
        assert_eq!(cells[1 * 10 + 5].ch, '│');
        assert_eq!(cells[5 * 10 + 5].ch, '┼');
        let classic = raster_to_cells(&r, &Theme::for_style(ThemeStyle::Classic), 10, 10);
        assert_eq!(classic[5 * 10 + 1].ch, '-');
        assert_eq!(classic[5 * 10 + 5].ch, '+');
    }

    #[test]
    fn subcell_counts_match_styles() {
        assert_eq!(subcell(ThemeStyle::Braille), (2, 4));
        assert_eq!(subcell(ThemeStyle::Blocks), (1, 2));
        assert_eq!(subcell(ThemeStyle::Classic), (1, 1));
    }
}
