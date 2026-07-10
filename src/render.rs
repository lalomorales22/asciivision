//! Shared frame representation + terminal blitters.
//!
//! `RgbFrame` is the one canonical high-resolution frame in the app: a raw
//! RGB24 pixel grid with *square* pixels. Every video surface -- local file
//! playback, live webcam, screen share, and remote video-chat feeds -- decodes
//! into an `RgbFrame`, and every rendering mode consumes it:
//!
//!   * [`VideoRenderMode::HalfBlock`] -- the default "video that pops" path.
//!     Each terminal cell becomes TWO stacked pixels via the upper-half-block
//!     glyph `▀` (fg = top pixel, bg = bottom pixel), doubling vertical
//!     resolution for free. Truecolor, no glyph quantization.
//!   * [`VideoRenderMode::Glyph`] -- the classic "asciivision" look: one
//!     luminance-ramp glyph per cell, colored, with a faint CRT scanline.
//!   * `VideoRenderMode::Pixel` -- true pixels via a terminal graphics protocol
//!     (added in the graphics-protocol sprint); falls back to HalfBlock.
//!
//! Keeping ONE frame type + ONE blitter here means fidelity work happens once
//! instead of being duplicated across `video.rs`, `webcam.rs`, and `main.rs`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

/// Luminance ramp for the classic glyph look (dark -> bright). 70 steps of
/// gradation -- the same ramp the app has always used for ASCII video.
const RAMP: &[u8] = b" .'`^\",:;Il!i><~+_-?][}{1)(|\\tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$";

/// Upper half block: renders `fg` in the top half of the cell, `bg` in the
/// bottom half. The single most important glyph in the app.
const HALF_BLOCK: char = '\u{2580}'; // ▀

/// A raw RGB24 pixel frame with square-ish pixels. `data.len() == width*height*3`.
#[derive(Clone, Debug, Default)]
pub struct RgbFrame {
    pub width: u16,
    pub height: u16,
    pub data: Vec<u8>,
}

impl RgbFrame {
    #[allow(dead_code)] // public constructor; used by tests + future frame sources
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            data: vec![0u8; width as usize * height as usize * 3],
        }
    }

    /// Sample a pixel. `x`/`y` MUST be in bounds; returns black if not so a
    /// bad sample can never panic mid-render.
    #[inline]
    pub fn pixel(&self, x: usize, y: usize) -> (u8, u8, u8) {
        let i = (y * self.width as usize + x) * 3;
        match self.data.get(i..i + 3) {
            Some(p) => (p[0], p[1], p[2]),
            None => (0, 0, 0),
        }
    }

    /// True when dimensions are non-zero and the buffer length matches exactly.
    pub fn is_valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.data.len() == self.width as usize * self.height as usize * 3
    }
}

/// How a video/webcam/feed surface is drawn into the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoRenderMode {
    /// Classic colored ASCII glyphs (one glyph per cell).
    Glyph,
    /// Truecolor upper-half-block subpixels (2x vertical resolution). Default.
    HalfBlock,
    /// True pixels via a terminal graphics protocol; falls back to HalfBlock
    /// where unsupported. Wired up in the graphics-protocol sprint.
    Pixel,
}

impl Default for VideoRenderMode {
    fn default() -> Self {
        VideoRenderMode::HalfBlock
    }
}

impl VideoRenderMode {
    /// Cycle order for the user-facing toggle. `Pixel` is only offered when the
    /// terminal supports it; the caller filters (see `App::cycle_video_mode`).
    pub fn cycle(self) -> Self {
        match self {
            VideoRenderMode::Glyph => VideoRenderMode::HalfBlock,
            VideoRenderMode::HalfBlock => VideoRenderMode::Pixel,
            VideoRenderMode::Pixel => VideoRenderMode::Glyph,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoRenderMode::Glyph => "ascii",
            VideoRenderMode::HalfBlock => "half-block",
            VideoRenderMode::Pixel => "pixel",
        }
    }
}

/// Placement of a source frame inside a cell `area`, letterboxed to preserve
/// the source's (square-pixel) aspect ratio. Width is in cells; height is in
/// cells but represents `rows*2` stacked pixels for the half-block grid.
struct Fit {
    ox: u16,
    oy: u16,
    cols: u16,
    rows: u16,
}

/// Fit a `fw x fh` square-pixel frame into a `aw x ah` CELL area, treating the
/// destination as an `aw x (ah*2)` grid of square pixels (half-block physics).
fn letterbox(fw: u16, fh: u16, area: Rect) -> Fit {
    let aw = area.width;
    let ah = area.height;
    if aw == 0 || ah == 0 || fw == 0 || fh == 0 {
        return Fit {
            ox: area.x,
            oy: area.y,
            cols: 0,
            rows: 0,
        };
    }
    let src = fw as f32 / fh as f32; // square-pixel aspect
    let dst = aw as f32 / (ah as f32 * 2.0); // destination pixel-grid aspect
    let (cols, rows) = if src >= dst {
        // width-limited
        let cols = aw;
        let rows_px = (cols as f32 / src).round().max(1.0);
        let rows = ((rows_px / 2.0).round() as u16).clamp(1, ah);
        (cols, rows)
    } else {
        // height-limited
        let rows = ah;
        let cols = ((rows as f32 * 2.0 * src).round() as u16).clamp(1, aw);
        (cols, rows)
    };
    Fit {
        ox: area.x + (aw - cols) / 2,
        oy: area.y + (ah - rows) / 2,
        cols,
        rows,
    }
}

#[inline]
fn scale8(c: u8, factor: f32) -> u8 {
    (c as f32 * factor).clamp(0.0, 255.0) as u8
}

/// Blit an `RgbFrame` into `area` using `mode`. `intensity` scales brightness
/// (video panels dim slightly). `Pixel` mode is handled by the caller before
/// reaching here; if it arrives it degrades to `HalfBlock`.
pub fn render_frame(
    buffer: &mut Buffer,
    area: Rect,
    frame: &RgbFrame,
    intensity: f32,
    mode: VideoRenderMode,
) {
    if !frame.is_valid() || area.width == 0 || area.height == 0 {
        return;
    }
    match mode {
        VideoRenderMode::Glyph => render_glyph(buffer, area, frame, intensity),
        _ => render_half_block(buffer, area, frame, intensity),
    }
}

/// Nearest-neighbor source column for output cell `cx`.
#[inline]
fn src_x(cx: u16, cols: u16, fw: u16) -> usize {
    ((cx as usize * fw as usize) / cols.max(1) as usize).min(fw as usize - 1)
}

/// Nearest-neighbor source row for half-pixel row `py` of a `rows*2` grid.
#[inline]
fn src_y(py: usize, rows: u16, fh: u16) -> usize {
    ((py * fh as usize) / (rows as usize * 2).max(1)).min(fh as usize - 1)
}

fn render_half_block(buffer: &mut Buffer, area: Rect, frame: &RgbFrame, intensity: f32) {
    let Fit {
        ox,
        oy,
        cols,
        rows,
    } = letterbox(frame.width, frame.height, area);
    for cy in 0..rows {
        for cx in 0..cols {
            let sx = src_x(cx, cols, frame.width);
            let (tr, tg, tb) = frame.pixel(sx, src_y(cy as usize * 2, rows, frame.height));
            let (br, bg, bb) = frame.pixel(sx, src_y(cy as usize * 2 + 1, rows, frame.height));
            if let Some(cell) = buffer.cell_mut((ox + cx, oy + cy)) {
                cell.set_char(HALF_BLOCK);
                cell.set_fg(Color::Rgb(
                    scale8(tr, intensity),
                    scale8(tg, intensity),
                    scale8(tb, intensity),
                ));
                cell.set_bg(Color::Rgb(
                    scale8(br, intensity),
                    scale8(bg, intensity),
                    scale8(bb, intensity),
                ));
            }
        }
    }
}

fn render_glyph(buffer: &mut Buffer, area: Rect, frame: &RgbFrame, intensity: f32) {
    let Fit {
        ox,
        oy,
        cols,
        rows,
    } = letterbox(frame.width, frame.height, area);
    for cy in 0..rows {
        // faint CRT scanline for the retro look
        let scan = if cy % 2 == 0 { 0.86 } else { 1.0 };
        for cx in 0..cols {
            let sx = src_x(cx, cols, frame.width);
            let (tr, tg, tb) = frame.pixel(sx, src_y(cy as usize * 2, rows, frame.height));
            let (br, bg, bb) = frame.pixel(sx, src_y(cy as usize * 2 + 1, rows, frame.height));
            // average the two stacked pixels into one cell
            let r = ((tr as u16 + br as u16) / 2) as u8;
            let g = ((tg as u16 + bg as u16) / 2) as u8;
            let b = ((tb as u16 + bb as u16) / 2) as u8;
            let luma = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) as usize;
            let glyph = RAMP[(luma * (RAMP.len() - 1)) / 255] as char;
            let f = (intensity * scan).clamp(0.1, 1.2);
            if let Some(cell) = buffer.cell_mut((ox + cx, oy + cy)) {
                cell.set_char(glyph);
                cell.set_fg(Color::Rgb(scale8(r, f), scale8(g, f), scale8(b, f)));
                cell.set_bg(Color::Rgb(
                    scale8(r, f * 0.16),
                    scale8(g, f * 0.16),
                    scale8(b, f * 0.16),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgbframe_validity() {
        assert!(RgbFrame::new(4, 3).is_valid());
        let bad = RgbFrame {
            width: 4,
            height: 3,
            data: vec![0; 10],
        };
        assert!(!bad.is_valid());
        assert!(!RgbFrame::new(0, 0).is_valid());
    }

    #[test]
    fn pixel_sampling_in_and_out_of_bounds() {
        let mut f = RgbFrame::new(2, 1);
        f.data = vec![10, 20, 30, 40, 50, 60];
        assert_eq!(f.pixel(0, 0), (10, 20, 30));
        assert_eq!(f.pixel(1, 0), (40, 50, 60));
        assert_eq!(f.pixel(5, 5), (0, 0, 0)); // out of bounds -> black, no panic
    }

    #[test]
    fn letterbox_preserves_aspect_and_fits() {
        // wide source into a square-ish cell area stays within bounds
        let area = Rect::new(0, 0, 40, 20);
        let fit = letterbox(160, 90, area);
        assert!(fit.cols <= area.width && fit.rows <= area.height);
        assert!(fit.cols > 0 && fit.rows > 0);
        // centered
        assert!(fit.ox >= area.x && fit.oy >= area.y);
    }

    #[test]
    fn render_into_tiny_area_is_safe() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 2));
        let mut f = RgbFrame::new(8, 8);
        f.data = vec![128; 8 * 8 * 3];
        // must not panic on a tiny target
        render_frame(&mut buf, Rect::new(0, 0, 3, 2), &f, 1.0, VideoRenderMode::HalfBlock);
        render_frame(&mut buf, Rect::new(0, 0, 3, 2), &f, 1.0, VideoRenderMode::Glyph);
    }

    #[test]
    fn half_block_maps_top_to_fg_and_bottom_to_bg() {
        // a 1-wide, 2-tall frame: red over blue. Into a single cell it must
        // become '▀' with fg = top (red) and bg = bottom (blue).
        let frame = RgbFrame {
            width: 1,
            height: 2,
            data: vec![255, 0, 0, /* row0 red */ 0, 0, 255 /* row1 blue */],
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        render_frame(&mut buf, Rect::new(0, 0, 1, 1), &frame, 1.0, VideoRenderMode::HalfBlock);
        let cell = buf.cell((0, 0)).unwrap();
        assert_eq!(cell.symbol(), "\u{2580}", "must be the upper half block");
        assert_eq!(cell.fg, Color::Rgb(255, 0, 0), "top pixel -> fg");
        assert_eq!(cell.bg, Color::Rgb(0, 0, 255), "bottom pixel -> bg");
    }

    #[test]
    fn mode_cycle_and_labels() {
        assert_eq!(VideoRenderMode::default(), VideoRenderMode::HalfBlock);
        assert_eq!(VideoRenderMode::Glyph.cycle(), VideoRenderMode::HalfBlock);
        assert_eq!(VideoRenderMode::HalfBlock.cycle(), VideoRenderMode::Pixel);
        assert_eq!(VideoRenderMode::Pixel.cycle(), VideoRenderMode::Glyph);
        assert_eq!(VideoRenderMode::HalfBlock.label(), "half-block");
    }
}
