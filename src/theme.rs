use parking_lot::RwLock;
use rand::Rng;
use ratatui::prelude::Color;
use std::sync::OnceLock;

static THEME: OnceLock<RwLock<Theme>> = OnceLock::new();

fn theme_lock() -> &'static RwLock<Theme> {
    THEME.get_or_init(|| RwLock::new(Theme::default_theme()))
}

pub fn t() -> parking_lot::RwLockReadGuard<'static, Theme> {
    theme_lock().read()
}

pub fn set_random_theme() {
    *theme_lock().write() = Theme::randomize();
}

pub fn reset_theme() {
    *theme_lock().write() = Theme::default_theme();
}

#[derive(Clone)]
pub struct Theme {
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
        }
    }

    pub fn randomize() -> Self {
        let mut rng = rand::thread_rng();
        let base_hue: f32 = rng.gen_range(0.0..360.0);

        let bg_base = hsl_to_color(base_hue, 0.3, rng.gen_range(0.02..0.05));
        let bg_alt = hsl_to_color(base_hue + 5.0, 0.25, rng.gen_range(0.04..0.08));
        let panel_bg = hsl_to_color(base_hue + 10.0, 0.28, rng.gen_range(0.03..0.07));
        let panel_alt = hsl_to_color(base_hue + 15.0, 0.22, rng.gen_range(0.05..0.10));

        let accent1 = hsl_to_color(
            base_hue + rng.gen_range(0.0..30.0),
            rng.gen_range(0.55..0.78),
            rng.gen_range(0.55..0.70),
        );
        let accent2 = hsl_to_color(
            base_hue + rng.gen_range(25.0..55.0),
            rng.gen_range(0.60..0.88),
            rng.gen_range(0.58..0.75),
        );
        let accent3 = hsl_to_color(
            base_hue + rng.gen_range(120.0..180.0),
            rng.gen_range(0.45..0.68),
            rng.gen_range(0.40..0.58),
        );
        let accent4 = hsl_to_color(
            base_hue + rng.gen_range(150.0..210.0),
            rng.gen_range(0.50..0.78),
            rng.gen_range(0.55..0.72),
        );

        let text = hsl_to_color(
            base_hue + rng.gen_range(140.0..200.0),
            rng.gen_range(0.10..0.28),
            rng.gen_range(0.85..0.94),
        );

        let danger = hsl_to_color(
            rng.gen_range(348.0..382.0) % 360.0,
            rng.gen_range(0.65..0.88),
            rng.gen_range(0.48..0.62),
        );

        let muted = hsl_to_color(
            base_hue + rng.gen_range(10.0..30.0),
            rng.gen_range(0.08..0.22),
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
        }
    }
}

pub fn color_to_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (180, 180, 180),
    }
}

fn hsl_to_color(h: f32, s: f32, l: f32) -> Color {
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
    Color::Rgb(
        ((r1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).clamp(0.0, 255.0) as u8,
    )
}
