//! Procedural art pipeline.
//!
//! ```text
//! Generator ──► Scene ──► Raster (sub-cell samples) ──► theme rasterizer ──► ArtBlock cells
//! ```
//!
//! Generators are pure functions of `(seed, size)`. The active `Theme` decides
//! what glyphs and colors a `Scene` becomes, so the same generator + seed looks
//! different under every `ThemeStyle`.

pub mod generators;
pub mod raster;
pub mod render3d;
pub mod rng;

use crate::theme::{Theme, ThemeStyle};
use ratatui::prelude::*;

pub use render3d::Mesh;

/// Hard caps so a stray `--size 999x999` cannot stall the UI.
pub const MAX_WIDTH: u16 = 200;
pub const MAX_HEIGHT: u16 = 80;
pub const MIN_WIDTH: u16 = 8;
pub const MIN_HEIGHT: u16 = 4;

/// A line segment in *physical* units (1 unit = one cell width; a cell is
/// 2 units tall). Intensity weight in `0..=1`, optional hue in degrees.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment2 {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub weight: f32,
    pub hue: Option<f32>,
}

/// The intermediate representation between generators and theme rasterizers.
#[derive(Clone, Debug)]
pub enum Scene {
    /// 2D scalar field at the raster's sample resolution.
    Field {
        w: usize,
        h: usize,
        intensity: Vec<f32>,
        hue: Option<Vec<f32>>,
    },
    /// Vector art in physical units spanning `0..w` x `0..h`.
    Strokes {
        w: f32,
        h: f32,
        segments: Vec<Segment2>,
    },
    /// 3D geometry. Rotated, projected and depth-shaded by the rasterizer.
    Mesh {
        mesh: Mesh,
        rotation: [f32; 3],
        cull_backfaces: bool,
        base_hue: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Generator {
    Automaton,
    Life,
    Noise,
    Mandelbrot,
    Julia,
    Maze,
    Attractor,
    Voronoi,
    Flow,
    Plant,
    Truchet,
    Lissajous,
    Cube,
    Torus,
    Sphere,
    Icosahedron,
    Terrain,
}

impl Generator {
    pub const ALL: [Generator; 17] = [
        Generator::Automaton,
        Generator::Life,
        Generator::Noise,
        Generator::Mandelbrot,
        Generator::Julia,
        Generator::Maze,
        Generator::Attractor,
        Generator::Voronoi,
        Generator::Flow,
        Generator::Plant,
        Generator::Truchet,
        Generator::Lissajous,
        Generator::Cube,
        Generator::Torus,
        Generator::Sphere,
        Generator::Icosahedron,
        Generator::Terrain,
    ];

    pub fn name(&self) -> &'static str {
        match self {
            Self::Automaton => "automaton",
            Self::Life => "life",
            Self::Noise => "noise",
            Self::Mandelbrot => "mandelbrot",
            Self::Julia => "julia",
            Self::Maze => "maze",
            Self::Attractor => "attractor",
            Self::Voronoi => "voronoi",
            Self::Flow => "flow",
            Self::Plant => "plant",
            Self::Truchet => "truchet",
            Self::Lissajous => "lissajous",
            Self::Cube => "cube",
            Self::Torus => "torus",
            Self::Sphere => "sphere",
            Self::Icosahedron => "icosahedron",
            Self::Terrain => "terrain",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Automaton => "1D cellular automaton (rule 30/90/110 family)",
            Self::Life => "Game of Life snapshot with fading trails",
            Self::Noise => "domain-warped fractal value noise",
            Self::Mandelbrot => "escape-time Mandelbrot zoom",
            Self::Julia => "escape-time Julia set",
            Self::Maze => "recursive-backtracker maze, rainbow by distance",
            Self::Attractor => "Clifford strange attractor density",
            Self::Voronoi => "Voronoi cells with dark boundaries",
            Self::Flow => "particles traced through a noise flow field",
            Self::Plant => "L-system plant",
            Self::Truchet => "Truchet arc tiles",
            Self::Lissajous => "Lissajous / harmonograph curves",
            Self::Cube => "3D cube",
            Self::Torus => "3D torus",
            Self::Sphere => "3D sphere",
            Self::Icosahedron => "3D icosahedron",
            Self::Terrain => "3D noise heightmap terrain",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        match name.as_str() {
            "automaton" | "ca" | "rule" | "rule30" | "rule110" => Some(Self::Automaton),
            "life" | "gol" | "conway" => Some(Self::Life),
            "noise" | "fbm" | "clouds" => Some(Self::Noise),
            "mandelbrot" | "mandel" | "fractal" => Some(Self::Mandelbrot),
            "julia" => Some(Self::Julia),
            "maze" => Some(Self::Maze),
            "attractor" | "clifford" | "strange" => Some(Self::Attractor),
            "voronoi" | "cells" => Some(Self::Voronoi),
            "flow" | "flowfield" | "field" => Some(Self::Flow),
            "plant" | "tree" | "lsystem" | "l-system" => Some(Self::Plant),
            "truchet" | "tiles" | "arcs" => Some(Self::Truchet),
            "lissajous" | "harmonograph" | "curve" => Some(Self::Lissajous),
            "cube" | "box" => Some(Self::Cube),
            "torus" | "donut" | "ring" => Some(Self::Torus),
            "sphere" | "ball" | "globe" => Some(Self::Sphere),
            "icosahedron" | "icosa" | "ico" | "d20" => Some(Self::Icosahedron),
            "terrain" | "heightmap" | "landscape" => Some(Self::Terrain),
            _ => None,
        }
    }


    /// Deterministic pick from a seed (used by `/art` with no generator).
    pub fn from_seed(seed: u64) -> Self {
        Self::ALL[(seed % Self::ALL.len() as u64) as usize]
    }

    pub fn list_text() -> String {
        Self::ALL
            .iter()
            .map(|g| format!("  {:<12} {}", g.name(), g.description()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtSpec {
    pub generator: Generator,
    pub seed: u64,
    pub width: u16,
    pub height: u16,
}

impl ArtSpec {
    pub fn new(generator: Generator, seed: u64, width: u16, height: u16) -> Self {
        Self {
            generator,
            seed,
            width: width.clamp(MIN_WIDTH, MAX_WIDTH),
            height: height.clamp(MIN_HEIGHT, MAX_HEIGHT),
        }
    }

    /// A pleasant default size for a panel of the given inner width.
    pub fn default_size(panel_width: u16) -> (u16, u16) {
        let width = panel_width.saturating_sub(2).clamp(MIN_WIDTH, 72);
        let height = (width as f32 / 2.6).round() as u16;
        (width, height.clamp(MIN_HEIGHT, 28))
    }

    /// Parse `[generator] [seed] [--seed N] [--size WxH] [WxH]` in any order.
    /// Missing pieces fall back to `fallback_seed` and `default_size`.
    pub fn parse(args: &str, fallback_seed: u64, panel_width: u16) -> Result<Self, String> {
        let (mut width, mut height) = Self::default_size(panel_width);
        let mut generator: Option<Generator> = None;
        let mut seed: Option<u64> = None;
        let mut tokens = args.split_whitespace().peekable();

        while let Some(tok) = tokens.next() {
            let lower = tok.to_ascii_lowercase();
            if lower == "--seed" || lower == "-s" {
                let v = tokens.next().ok_or("--seed needs a number")?;
                seed = Some(v.parse::<u64>().map_err(|_| format!("bad seed: {}", v))?);
            } else if let Some(v) = lower.strip_prefix("--seed=") {
                seed = Some(v.parse::<u64>().map_err(|_| format!("bad seed: {}", v))?);
            } else if lower == "--size" {
                let v = tokens.next().ok_or("--size needs WxH")?;
                let (w, h) = parse_size(v)?;
                width = w;
                height = h;
            } else if let Some(v) = lower.strip_prefix("--size=") {
                let (w, h) = parse_size(v)?;
                width = w;
                height = h;
            } else if let Ok((w, h)) = parse_size(&lower) {
                width = w;
                height = h;
            } else if let Ok(n) = lower.parse::<u64>() {
                seed = Some(n);
            } else if let Some(g) = Generator::from_name(&lower) {
                generator = Some(g);
            } else {
                return Err(format!("unknown generator or option: {}", tok));
            }
        }

        let seed = seed.unwrap_or(fallback_seed);
        let generator = generator.unwrap_or_else(|| Generator::from_seed(seed));
        Ok(Self::new(generator, seed, width, height))
    }
}

fn parse_size(v: &str) -> Result<(u16, u16), String> {
    let (w, h) = v
        .split_once('x')
        .ok_or_else(|| format!("size must look like 60x24, got {}", v))?;
    let w = w.parse::<u16>().map_err(|_| format!("bad width: {}", w))?;
    let h = h.parse::<u16>().map_err(|_| format!("bad height: {}", h))?;
    if w == 0 || h == 0 {
        return Err("size must be non-zero".to_string());
    }
    Ok((w.clamp(MIN_WIDTH, MAX_WIDTH), h.clamp(MIN_HEIGHT, MAX_HEIGHT)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArtCell {
    pub ch: char,
    pub fg: (u8, u8, u8),
    /// `None` = let the panel background show through.
    pub bg: Option<(u8, u8, u8)>,
}

impl ArtCell {
    pub const BLANK: ArtCell = ArtCell {
        ch: ' ',
        fg: (0, 0, 0),
        bg: None,
    };
}

/// One rendered, static frame of art: a rectangle of glyph cells.
#[derive(Clone, Debug)]
pub struct ArtBlock {
    pub width: u16,
    pub height: u16,
    pub cells: Vec<ArtCell>,
    pub caption: String,
    pub spec: ArtSpec,
    pub style: ThemeStyle,
}

impl ArtBlock {
    pub fn cell(&self, x: u16, y: u16) -> ArtCell {
        if x >= self.width || y >= self.height {
            return ArtCell::BLANK;
        }
        self.cells[y as usize * self.width as usize + x as usize]
    }

    /// Plain glyphs, one row per line, no color. Used by tests and by the
    /// textual summary handed back to the model.
    pub fn to_plain_text(&self) -> String {
        let mut out = String::with_capacity((self.width as usize + 1) * self.height as usize);
        for y in 0..self.height {
            for x in 0..self.width {
                out.push(self.cell(x, y).ch);
            }
            out.push('\n');
        }
        out
    }

    /// Write one row of the block into a buffer row, clipped to `max_width`.
    pub fn blit_row(&self, buffer: &mut Buffer, x: u16, y: u16, row: u16, max_width: u16, fallback_bg: Color) {
        let width = self.width.min(max_width);
        for col in 0..width {
            let cell = self.cell(col, row);
            if let Some(target) = buffer.cell_mut((x + col, y)) {
                target.set_char(cell.ch);
                target.set_fg(Color::Rgb(cell.fg.0, cell.fg.1, cell.fg.2));
                target.set_bg(match cell.bg {
                    Some((r, g, b)) => Color::Rgb(r, g, b),
                    None => fallback_bg,
                });
            }
        }
    }

    /// Write the whole block into `area` (top-left aligned, clipped).
    pub fn blit(&self, buffer: &mut Buffer, area: Rect, fallback_bg: Color) {
        let rows = self.height.min(area.height);
        for row in 0..rows {
            self.blit_row(buffer, area.x, area.y + row, row, area.width, fallback_bg);
        }
    }
}

/// Render a spec under a theme, using the rotation derived from the seed.
pub fn render(spec: &ArtSpec, theme: &Theme) -> ArtBlock {
    render_with_rotation(spec, theme, None)
}

/// Render a spec under a theme. `rotation` overrides the seed-derived
/// rotation for 3D generators, which is how the Effects3D panel animates.
pub fn render_with_rotation(spec: &ArtSpec, theme: &Theme, rotation: Option<[f32; 3]>) -> ArtBlock {
    let (sx, sy) = raster::subcell(theme.style);
    let fw = spec.width as usize * sx;
    let fh = spec.height as usize * sy;
    let phys_w = spec.width as f32;
    let phys_h = spec.height as f32 * 2.0;

    let mut scene = generators::generate(spec.generator, spec.seed, fw, fh, phys_w, phys_h);
    if let (Some(rot), Scene::Mesh { rotation: r, .. }) = (rotation, &mut scene) {
        *r = rot;
    }

    let raster = raster::scene_to_raster(&scene, fw, fh, sx, sy, theme.style);
    let cells = raster::raster_to_cells(&raster, theme, spec.width as usize, spec.height as usize);

    ArtBlock {
        width: spec.width,
        height: spec.height,
        cells,
        caption: format!(
            "{} · seed {} · {}x{} · {}",
            spec.generator.name(),
            spec.seed,
            spec.width,
            spec.height,
            theme.style.name()
        ),
        spec: spec.clone(),
        style: theme.style,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn all_specs() -> Vec<ArtSpec> {
        Generator::ALL
            .iter()
            .map(|g| ArtSpec::new(*g, 1234, 40, 16))
            .collect()
    }

    #[test]
    fn every_generator_is_deterministic_under_every_theme() {
        for style in ThemeStyle::ALL {
            let theme = Theme::for_style(style);
            for spec in all_specs() {
                let a = render(&spec, &theme);
                let b = render(&spec, &theme);
                assert_eq!(a.cells, b.cells, "{} under {} not deterministic", spec.generator.name(), style.name());
                assert_eq!(a.cells.len(), 40 * 16);
            }
        }
    }

    #[test]
    fn different_seeds_produce_different_art() {
        let theme = Theme::for_style(ThemeStyle::Classic);
        for g in Generator::ALL {
            let a = render(&ArtSpec::new(g, 1, 48, 20), &theme);
            let b = render(&ArtSpec::new(g, 2, 48, 20), &theme);
            assert_ne!(a.to_plain_text(), b.to_plain_text(), "{} ignores its seed", g.name());
        }
    }

    #[test]
    fn every_generator_draws_something() {
        let theme = Theme::for_style(ThemeStyle::Classic);
        for spec in all_specs() {
            let block = render(&spec, &theme);
            let inked = block.cells.iter().filter(|c| c.ch != ' ').count();
            assert!(inked > 10, "{} rendered almost nothing ({} cells)", spec.generator.name(), inked);
        }
    }

    #[test]
    fn same_seed_differs_across_themes() {
        // The proof that themes are render targets, not palettes.
        let spec = ArtSpec::new(Generator::Mandelbrot, 99, 48, 20);
        let mut seen: Vec<String> = Vec::new();
        for style in ThemeStyle::ALL {
            let text = render(&spec, &Theme::for_style(style)).to_plain_text();
            assert!(!seen.contains(&text), "{} renders identically to another style", style.name());
            seen.push(text);
        }
        assert_eq!(seen.len(), ThemeStyle::ALL.len());
    }

    #[test]
    fn braille_uses_braille_glyphs_and_blocks_uses_blocks() {
        let spec = ArtSpec::new(Generator::Noise, 5, 40, 16);
        let braille = render(&spec, &Theme::for_style(ThemeStyle::Braille));
        assert!(braille
            .cells
            .iter()
            .all(|c| c.ch == ' ' || ('\u{2800}'..='\u{28FF}').contains(&c.ch)));
        let blocks = render(&spec, &Theme::for_style(ThemeStyle::Blocks));
        assert!(blocks
            .cells
            .iter()
            .all(|c| matches!(c.ch, ' ' | '░' | '▒' | '▓' | '█' | '▀' | '▄')));
    }

    #[test]
    fn spec_parser_accepts_common_shapes() {
        let s = ArtSpec::parse("mandelbrot --seed 7 --size 50x20", 1, 80).unwrap();
        assert_eq!(s.generator, Generator::Mandelbrot);
        assert_eq!((s.seed, s.width, s.height), (7, 50, 20));

        let s = ArtSpec::parse("torus 12 30x12", 1, 80).unwrap();
        assert_eq!(s.generator, Generator::Torus);
        assert_eq!((s.seed, s.width, s.height), (12, 30, 12));

        let s = ArtSpec::parse("", 77, 80).unwrap();
        assert_eq!(s.seed, 77);
        assert!(ArtSpec::parse("bogus", 1, 80).is_err());

        // Sizes clamp instead of exploding.
        let s = ArtSpec::parse("noise 9999x9999", 1, 80).unwrap();
        assert_eq!((s.width, s.height), (MAX_WIDTH, MAX_HEIGHT));
    }

    #[test]
    fn rotation_override_changes_3d_output() {
        let theme = Theme::for_style(ThemeStyle::Depth3D);
        let spec = ArtSpec::new(Generator::Torus, 3, 48, 20);
        let a = render_with_rotation(&spec, &theme, Some([0.0, 0.0, 0.0]));
        let b = render_with_rotation(&spec, &theme, Some([1.0, 0.7, 0.2]));
        assert_ne!(a.to_plain_text(), b.to_plain_text());
    }
}
