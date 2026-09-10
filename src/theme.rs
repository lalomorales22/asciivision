//! Theme engine.
//!
//! A theme is a *render target*, not just a palette: it carries the eleven
//! flat colors that the rest of the app reads through `t().accent1` and
//! friends, plus structural fields (a `ThemeStyle`, a glyph ramp, and border
//! treatments) that decide how procedural art, video, and panels are drawn.

use parking_lot::RwLock;
use rand::Rng;
use ratatui::prelude::Color;
use ratatui::widgets::BorderType;
use std::sync::OnceLock;

static THEME: OnceLock<RwLock<Theme>> = OnceLock::new();

fn theme_lock() -> &'static RwLock<Theme> {
    THEME.get_or_init(|| RwLock::new(Theme::default_theme()))
}

pub fn t() -> parking_lot::RwLockReadGuard<'static, Theme> {
    theme_lock().read()
}

/// Randomize the palette *within* the active style. The glyph ramp and border
/// treatment stay put; only the hues move.
pub fn set_random_theme() {
    let style = theme_lock().read().style;
    *theme_lock().write() = Theme::randomize_within(style);
}

pub fn reset_theme() {
    *theme_lock().write() = Theme::default_theme();
}

pub fn set_style(style: ThemeStyle) {
    *theme_lock().write() = Theme::for_style(style);
}

pub fn cycle_style() -> ThemeStyle {
    let next = theme_lock().read().style.cycle();
    set_style(next);
    next
}

pub fn current_style() -> ThemeStyle {
    theme_lock().read().style
}

// ---------------------------------------------------------------------------
// Glyph ramps (sparse -> dense). Every ramp starts with a space.
// ---------------------------------------------------------------------------

/// The 69-step luminance ramp originally hardcoded in `video.rs`.
pub const CLASSIC_RAMP: &[char] = &[
    ' ', '.', '\'', '`', '^', '"', ',', ':', ';', 'I', 'l', '!', 'i', '>', '<', '~', '+', '_',
    '-', '?', ']', '[', '}', '{', '1', ')', '(', '|', '\\', 't', 'f', 'j', 'r', 'x', 'n', 'u',
    'v', 'c', 'z', 'X', 'Y', 'U', 'J', 'C', 'L', 'Q', '0', 'O', 'Z', 'm', 'w', 'q', 'p', 'd',
    'b', 'k', 'h', 'a', 'o', '*', '#', 'M', 'W', '&', '8', '%', 'B', '@', '$',
];
pub const BLOCKS_RAMP: &[char] = &[' ', '░', '▒', '▓', '█'];
pub const BRAILLE_RAMP: &[char] = &[' ', '⠁', '⠃', '⠇', '⡇', '⡏', '⡟', '⡿', '⣿'];
pub const MINIMAL_RAMP: &[char] = &[' ', '·', ':', '-', '=', '+', '*', '#'];
pub const WIREFRAME_RAMP: &[char] = &[' ', '·', '╌', '─', '═', '╪', '╬', '█'];
pub const DEPTH3D_RAMP: &[char] = &[' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];

/// A structural look. Each style owns a glyph vocabulary, a border treatment,
/// a default palette, and (in `art::raster`) its own rasterizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeStyle {
    Classic,
    Blocks,
    Braille,
    Minimal,
    Wireframe,
    Depth3D,
}

impl ThemeStyle {
    pub const ALL: [ThemeStyle; 6] = [
        ThemeStyle::Classic,
        ThemeStyle::Blocks,
        ThemeStyle::Braille,
        ThemeStyle::Minimal,
        ThemeStyle::Wireframe,
        ThemeStyle::Depth3D,
    ];

    pub fn cycle(self) -> Self {
        match self {
            Self::Classic => Self::Blocks,
            Self::Blocks => Self::Braille,
            Self::Braille => Self::Minimal,
            Self::Minimal => Self::Wireframe,
            Self::Wireframe => Self::Depth3D,
            Self::Depth3D => Self::Classic,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Blocks => "blocks",
            Self::Braille => "braille",
            Self::Minimal => "minimal",
            Self::Wireframe => "wireframe",
            Self::Depth3D => "depth3d",
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            Self::Classic => "CLASSIC",
            Self::Blocks => "BLOCKS",
            Self::Braille => "BRAILLE",
            Self::Minimal => "MINIMAL",
            Self::Wireframe => "WIREFRAME",
            Self::Depth3D => "DEPTH 3D",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Classic => "69-step ASCII ramp, teal/amber, double borders",
            Self::Blocks => "half-block pixels + shade glyphs, neon poster look, thick borders",
            Self::Braille => "2x4 braille dot matrix, 8x resolution, rounded borders",
            Self::Minimal => "mono glyphs, restrained two-tone palette, plain borders",
            Self::Wireframe => "box-drawing vectors, phosphor CAD palette",
            Self::Depth3D => "depth-indexed ramp with lighting, sunset palette",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "classic" | "ascii" => Some(Self::Classic),
            "blocks" | "block" | "pixel" => Some(Self::Blocks),
            "braille" | "dots" => Some(Self::Braille),
            "minimal" | "mono" | "min" => Some(Self::Minimal),
            "wireframe" | "wire" | "cad" => Some(Self::Wireframe),
            "depth3d" | "3d" | "depth" => Some(Self::Depth3D),
            _ => None,
        }
    }

    pub fn ramp(&self) -> &'static [char] {
        match self {
            Self::Classic => CLASSIC_RAMP,
            Self::Blocks => BLOCKS_RAMP,
            Self::Braille => BRAILLE_RAMP,
            Self::Minimal => MINIMAL_RAMP,
            Self::Wireframe => WIREFRAME_RAMP,
            Self::Depth3D => DEPTH3D_RAMP,
        }
    }

    /// Border used for focused / primary panels.
    pub fn border(&self) -> BorderType {
        match self {
            Self::Classic => BorderType::Double,
            Self::Blocks => BorderType::Thick,
            Self::Braille => BorderType::Rounded,
            Self::Minimal => BorderType::Plain,
            Self::Wireframe => BorderType::Double,
            Self::Depth3D => BorderType::QuadrantOutside,
        }
    }

    /// Border used for unfocused / secondary panels.
    pub fn border_alt(&self) -> BorderType {
        match self {
            Self::Classic => BorderType::Plain,
            Self::Blocks => BorderType::Thick,
            Self::Braille => BorderType::Rounded,
            Self::Minimal => BorderType::Plain,
            Self::Wireframe => BorderType::Plain,
            Self::Depth3D => BorderType::QuadrantInside,
        }
    }

}

#[derive(Clone)]
pub struct Theme {
    // Palette. These stay flat so every `t().accent1` call site keeps working.
    pub bg_base: Color,
    pub bg_alt: Color,
    pub panel_bg: Color,
    pub panel_alt: Color,
    pub accent1: Color,
    pub accent2: Color,
    pub accent3: Color,
    pub accent4: Color,
    pub text: Color,
    pub danger: Color,
    pub muted: Color,

    // Structure.
    pub style: ThemeStyle,
    pub ramp: &'static [char],
    pub border: BorderType,
    pub border_alt: BorderType,
}

impl Theme {
    pub fn default_theme() -> Self {
        Self {
            bg_base: Color::Rgb(3, 8, 12),
            bg_alt: Color::Rgb(10, 17, 24),
            panel_bg: Color::Rgb(6, 13, 19),
            panel_alt: Color::Rgb(10, 19, 29),
            accent1: Color::Rgb(214, 153, 104),
            accent2: Color::Rgb(241, 189, 105),
            accent3: Color::Rgb(54, 154, 158),
            accent4: Color::Rgb(118, 214, 226),
            text: Color::Rgb(207, 230, 232),
            danger: Color::Rgb(225, 92, 84),
            muted: Color::Rgb(101, 121, 134),
            style: ThemeStyle::Classic,
            ramp: CLASSIC_RAMP,
            border: ThemeStyle::Classic.border(),
            border_alt: ThemeStyle::Classic.border_alt(),
        }
    }

    /// The canonical look for a style: a hand-tuned palette plus structure.
    pub fn for_style(style: ThemeStyle) -> Self {
        let mut theme = match style {
            ThemeStyle::Classic => Self::default_theme(),
            ThemeStyle::Blocks => Self {
                bg_base: Color::Rgb(8, 2, 14),
                bg_alt: Color::Rgb(16, 6, 26),
                panel_bg: Color::Rgb(12, 4, 20),
                panel_alt: Color::Rgb(20, 8, 32),
                accent1: Color::Rgb(255, 64, 160),
                accent2: Color::Rgb(255, 210, 64),
                accent3: Color::Rgb(0, 200, 220),
                accent4: Color::Rgb(120, 255, 200),
                text: Color::Rgb(240, 230, 250),
                danger: Color::Rgb(255, 70, 90),
                muted: Color::Rgb(120, 90, 140),
                style,
                ramp: style.ramp(),
                border: style.border(),
                border_alt: style.border_alt(),
            },
            ThemeStyle::Braille => Self {
                bg_base: Color::Rgb(4, 6, 14),
                bg_alt: Color::Rgb(9, 12, 24),
                panel_bg: Color::Rgb(6, 9, 18),
                panel_alt: Color::Rgb(11, 15, 28),
                accent1: Color::Rgb(150, 170, 255),
                accent2: Color::Rgb(200, 150, 255),
                accent3: Color::Rgb(90, 200, 255),
                accent4: Color::Rgb(180, 240, 255),
                text: Color::Rgb(220, 228, 250),
                danger: Color::Rgb(255, 110, 130),
                muted: Color::Rgb(95, 105, 150),
                style,
                ramp: style.ramp(),
                border: style.border(),
                border_alt: style.border_alt(),
            },
            ThemeStyle::Minimal => Self {
                bg_base: Color::Rgb(10, 10, 10),
                bg_alt: Color::Rgb(16, 16, 16),
                panel_bg: Color::Rgb(12, 12, 12),
                panel_alt: Color::Rgb(18, 18, 18),
                accent1: Color::Rgb(200, 200, 200),
                accent2: Color::Rgb(150, 210, 160),
                accent3: Color::Rgb(120, 120, 120),
                accent4: Color::Rgb(230, 230, 230),
                text: Color::Rgb(215, 215, 215),
                danger: Color::Rgb(220, 110, 110),
                muted: Color::Rgb(85, 85, 85),
                style,
                ramp: style.ramp(),
                border: style.border(),
                border_alt: style.border_alt(),
            },
            ThemeStyle::Wireframe => Self {
                bg_base: Color::Rgb(2, 10, 4),
                bg_alt: Color::Rgb(5, 18, 8),
                panel_bg: Color::Rgb(3, 14, 6),
                panel_alt: Color::Rgb(6, 22, 10),
                accent1: Color::Rgb(90, 255, 120),
                accent2: Color::Rgb(220, 255, 140),
                accent3: Color::Rgb(40, 190, 90),
                accent4: Color::Rgb(160, 255, 200),
                text: Color::Rgb(200, 245, 210),
                danger: Color::Rgb(255, 120, 80),
                muted: Color::Rgb(70, 130, 90),
                style,
                ramp: style.ramp(),
                border: style.border(),
                border_alt: style.border_alt(),
            },
            ThemeStyle::Depth3D => Self {
                bg_base: Color::Rgb(10, 4, 18),
                bg_alt: Color::Rgb(18, 8, 30),
                panel_bg: Color::Rgb(14, 6, 24),
                panel_alt: Color::Rgb(22, 10, 36),
                accent1: Color::Rgb(255, 140, 80),
                accent2: Color::Rgb(255, 200, 90),
                accent3: Color::Rgb(170, 80, 255),
                accent4: Color::Rgb(255, 120, 200),
                text: Color::Rgb(245, 225, 240),
                danger: Color::Rgb(255, 80, 100),
                muted: Color::Rgb(130, 90, 150),
                style,
                ramp: style.ramp(),
                border: style.border(),
                border_alt: style.border_alt(),
            },
        };
        theme.style = style;
        theme.ramp = style.ramp();
        theme.border = style.border();
        theme.border_alt = style.border_alt();
        theme
    }


    /// Random palette derived from a random base hue, keeping the given
    /// style's ramp and borders. Minimal stays desaturated so it remains
    /// "minimal" no matter what the dice say.
    pub fn randomize_within(style: ThemeStyle) -> Self {
        let mut rng = rand::thread_rng();
        let base_hue: f32 = rng.gen_range(0.0..360.0);
        let sat_scale: f32 = if style == ThemeStyle::Minimal { 0.18 } else { 1.0 };

        let bg_base = hsl_to_color(base_hue, 0.3 * sat_scale, rng.gen_range(0.02..0.05));
        let bg_alt = hsl_to_color(base_hue + 5.0, 0.25 * sat_scale, rng.gen_range(0.04..0.08));
        let panel_bg = hsl_to_color(base_hue + 10.0, 0.28 * sat_scale, rng.gen_range(0.03..0.07));
        let panel_alt = hsl_to_color(base_hue + 15.0, 0.22 * sat_scale, rng.gen_range(0.05..0.10));

        let accent1 = hsl_to_color(
            base_hue + rng.gen_range(0.0..30.0),
            rng.gen_range(0.55..0.78) * sat_scale,
            rng.gen_range(0.55..0.70),
        );
        let accent2 = hsl_to_color(
            base_hue + rng.gen_range(25.0..55.0),
            rng.gen_range(0.60..0.88) * sat_scale,
            rng.gen_range(0.58..0.75),
        );
        let accent3 = hsl_to_color(
            base_hue + rng.gen_range(120.0..180.0),
            rng.gen_range(0.45..0.68) * sat_scale,
            rng.gen_range(0.40..0.58),
        );
        let accent4 = hsl_to_color(
            base_hue + rng.gen_range(150.0..210.0),
            rng.gen_range(0.50..0.78) * sat_scale,
            rng.gen_range(0.55..0.72),
        );

        let text = hsl_to_color(
            base_hue + rng.gen_range(140.0..200.0),
            rng.gen_range(0.10..0.28) * sat_scale,
            rng.gen_range(0.85..0.94),
        );

        let danger = hsl_to_color(
            rng.gen_range(348.0..382.0) % 360.0,
            rng.gen_range(0.65..0.88),
            rng.gen_range(0.48..0.62),
        );

        let muted = hsl_to_color(
            base_hue + rng.gen_range(10.0..30.0),
            rng.gen_range(0.08..0.22) * sat_scale,
            rng.gen_range(0.38..0.52),
        );

        Self {
            bg_base,
            bg_alt,
            panel_bg,
            panel_alt,
            accent1,
            accent2,
            accent3,
            accent4,
            text,
            danger,
            muted,
            style,
            ramp: style.ramp(),
            border: style.border(),
            border_alt: style.border_alt(),
        }
    }


    /// Map a 0..=1 intensity to a glyph from this theme's ramp.
    pub fn ramp_glyph(&self, intensity: f32) -> char {
        ramp_glyph(self.ramp, intensity)
    }

    /// Map an 8-bit luminance to a glyph from this theme's ramp (video path).
    /// Short ramps (blocks, braille, minimal...) get a gamma lift so dark
    /// footage still lands on a visible glyph instead of a blank cell; the
    /// 69-step classic ramp is left linear so its look is unchanged.
    pub fn ramp_glyph_u8(&self, luma: u8) -> char {
        let last = self.ramp.len() - 1;
        let idx = if self.ramp.len() >= 32 {
            (luma as usize * last) / 255
        } else {
            ((luma as f32 / 255.0).powf(0.55) * last as f32).round() as usize
        };
        self.ramp[idx.min(last)]
    }
}

pub fn ramp_glyph(ramp: &[char], intensity: f32) -> char {
    if ramp.is_empty() {
        return ' ';
    }
    let i = (intensity.clamp(0.0, 1.0) * (ramp.len() - 1) as f32).round() as usize;
    ramp[i.min(ramp.len() - 1)]
}

pub fn color_to_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (180, 180, 180),
    }
}

pub fn hsl_to_color(h: f32, s: f32, l: f32) -> Color {
    let (r, g, b) = hsl_to_rgb(h, s, l);
    Color::Rgb(r, g, b)
}

pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_style_has_a_ramp_starting_with_space() {
        for style in ThemeStyle::ALL {
            let ramp = style.ramp();
            assert!(ramp.len() >= 5, "{} ramp too short", style.name());
            assert_eq!(ramp[0], ' ');
        }
    }

    #[test]
    fn styles_round_trip_through_names() {
        for style in ThemeStyle::ALL {
            assert_eq!(ThemeStyle::from_name(style.name()), Some(style));
        }
        assert_eq!(ThemeStyle::from_name("nope"), None);
    }

    #[test]
    fn cycle_visits_every_style() {
        let mut seen = vec![ThemeStyle::Classic];
        let mut s = ThemeStyle::Classic.cycle();
        while s != ThemeStyle::Classic {
            seen.push(s);
            s = s.cycle();
        }
        assert_eq!(seen.len(), ThemeStyle::ALL.len());
    }

    #[test]
    fn randomize_within_keeps_structure() {
        for style in ThemeStyle::ALL {
            let th = Theme::randomize_within(style);
            assert_eq!(th.style, style);
            assert_eq!(th.ramp, style.ramp());
            assert_eq!(th.border, style.border());
        }
    }

    #[test]
    fn ramp_glyph_hits_both_ends() {
        assert_eq!(ramp_glyph(CLASSIC_RAMP, 0.0), ' ');
        assert_eq!(ramp_glyph(CLASSIC_RAMP, 1.0), '$');
        assert_eq!(ramp_glyph(CLASSIC_RAMP, 2.0), '$');
        assert_eq!(ramp_glyph(BLOCKS_RAMP, 1.0), '█');
    }
}
