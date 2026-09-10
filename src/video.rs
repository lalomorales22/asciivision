use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver};
use ff::format::context::Input;
use ff::format::Pixel;
use ff::software::scaling::{context::Context as Scaler, flag::Flags};
use ff::util::frame::video::Video;
use ffmpeg_next as ff;
use ratatui::{prelude::*, widgets::Paragraph};
use std::{
    cmp::min,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};


#[derive(Clone)]
pub struct AsciiFrame {
    pub width: u16,
    pub height: u16,
    pub cells: Vec<(char, u8, u8, u8)>,
}

pub struct VideoPlayer {
    path: PathBuf,
    looping: bool,
    decode_size: (u16, u16),
    rx: Receiver<AsciiFrame>,
    latest: Option<AsciiFrame>,
    finished: Arc<AtomicBool>,
}

impl VideoPlayer {
    pub fn new(path: impl Into<PathBuf>, decode_size: (u16, u16), looping: bool) -> Result<Self> {
        let path = path.into();
        let finished = Arc::new(AtomicBool::new(false));
        let rx = spawn_decode(path.as_path(), decode_size, finished.clone())?;

        Ok(Self {
            path,
            looping,
            decode_size,
            rx,
            latest: None,
            finished,
        })
    }

    pub fn tick(&mut self) {
        while let Ok(frame) = self.rx.try_recv() {
            self.latest = Some(frame);
        }

        if self.looping && self.finished.load(Ordering::Relaxed) && self.rx.is_empty() {
            self.finished.store(false, Ordering::Relaxed);
            if let Ok(rx) =
                spawn_decode(self.path.as_path(), self.decode_size, self.finished.clone())
            {
                self.rx = rx;
            }
        }
    }

    pub fn has_signal(&self) -> bool {
        self.latest.is_some()
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, intensity: f32) {
        if area.width < 4 || area.height < 4 {
            return;
        }

        if let Some(ref ascii) = self.latest {
            render_ascii(frame.buffer_mut(), area, ascii, intensity);
        } else {
            let placeholder = Paragraph::new("signal lock pending")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Rgb(112, 166, 179)));
            frame.render_widget(placeholder, area);
        }
    }
}

fn luminance(r: u8, g: u8, b: u8) -> u8 {
    let value = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    value as u8
}

/// Glyph for a pixel under the active theme's ramp (classic = the original
/// 69-step ASCII ramp; blocks/braille/etc. restyle the whole video feed).
fn ascii_for(theme: &crate::theme::Theme, r: u8, g: u8, b: u8) -> char {
    theme.ramp_glyph_u8(luminance(r, g, b))
}

fn to_ascii_frame(rgb: &Video) -> AsciiFrame {
    let width = rgb.width() as usize;
    let height = rgb.height() as usize;
    let stride = rgb.stride(0);
    let data = rgb.data(0);
    let mut cells = Vec::with_capacity(width * height);
    let theme = crate::theme::t().clone();

    for y in 0..height {
        let row = &data[(y * stride) as usize..((y * stride) as usize + width * 3)];
        for x in 0..width {
            let index = x * 3;
            let (r, g, b) = (row[index], row[index + 1], row[index + 2]);
            cells.push((ascii_for(&theme, r, g, b), r, g, b));
        }
    }

    AsciiFrame {
        width: width as u16,
        height: height as u16,
        cells,
    }
}

fn open_decoder(
    path: &Path,
) -> Result<(
    Input,
    usize,
    ff::codec::decoder::Video,
    (u32, u32),
    Option<(u32, u32)>,
)> {
    ff::init().context("init ffmpeg")?;
    // suppress all FFmpeg log output -- it writes to stderr and corrupts the TUI
    unsafe { ffmpeg_sys_next::av_log_set_level(ffmpeg_sys_next::AV_LOG_QUIET) };
    let input =
        ff::format::input(path).with_context(|| format!("open input {}", path.display()))?;
    let stream = input
        .streams()
        .best(ff::media::Type::Video)
        .context("no video stream found")?;
    let index = stream.index();
    let context = ff::codec::context::Context::from_parameters(stream.parameters())?;
    let decoder = context.decoder().video()?;
    let fps = if stream.avg_frame_rate() != ff::Rational(0, 0) {
        let rate = stream.avg_frame_rate();
        Some((rate.numerator() as u32, rate.denominator() as u32))
    } else {
        None
    };

    let dimensions = (decoder.width(), decoder.height());
    Ok((input, index, decoder, dimensions, fps))
}

fn build_scaler(
    src_format: Pixel,
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
) -> Result<Scaler> {
    Scaler::get(
        src_format,
        src_width,
        src_height,
        Pixel::RGB24,
        dst_width,
        dst_height,
        Flags::BILINEAR,
    )
    .context("create scaler")
}

/// Decode dimensions that preserve the source aspect ratio inside
/// `(max_w, max_h)` cells. Terminal cells are ~2x taller than wide, so the
/// source's width:height ratio is doubled before fitting.
pub fn fit_aspect(src_w: u32, src_h: u32, max_w: u16, max_h: u16) -> (u16, u16) {
    if src_w == 0 || src_h == 0 || max_w == 0 || max_h == 0 {
        return (max_w.max(1), max_h.max(1));
    }
    let char_aspect: f64 = 2.0;
    let effective_ratio = (src_w as f64 / src_h as f64) * char_aspect;
    let (max_wf, max_hf) = (max_w as f64, max_h as f64);
    let (w, h) = {
        let h = max_wf / effective_ratio;
        if h <= max_hf {
            (max_wf, h)
        } else {
            (max_hf * effective_ratio, max_hf)
        }
    };
    (
        (w.round() as u16).clamp(1, max_w),
        (h.round() as u16).clamp(1, max_h),
    )
}

fn spawn_decode(
    path: &Path,
    decode_size: (u16, u16),
    finished: Arc<AtomicBool>,
) -> Result<Receiver<AsciiFrame>> {
    let path = path.to_path_buf();
    let (tx, rx) = bounded(8);
    let (max_width, max_height) = decode_size;

    std::thread::spawn(move || {
        let _result: Result<()> = (|| {
        let (mut input, video_index, mut decoder, (src_width, src_height), fps) =
            open_decoder(path.as_path())?;

        // Keep the picture's proportions instead of stretching to the panel.
        let (target_width, target_height) = fit_aspect(src_width, src_height, max_width, max_height);

        // Pace delivery to the source frame rate (24 fps if unknown) so
        // playback runs at real speed instead of as fast as decode allows.
        let frame_duration = fps
            .filter(|&(n, d)| n > 0 && d > 0)
            .map(|(n, d)| std::time::Duration::from_secs_f64(d as f64 / n as f64))
            .unwrap_or(std::time::Duration::from_millis(42));

        let mut scaler = build_scaler(
            decoder.format(),
            src_width,
            src_height,
            target_width as u32,
            target_height as u32,
        )?;
        let mut rgb = Video::new(Pixel::RGB24, target_width as u32, target_height as u32);
        let mut decoded = Video::empty();
        let mut last_frame_time = std::time::Instant::now();
        let pace = |last: &mut std::time::Instant| {
            let elapsed = last.elapsed();
            if elapsed < frame_duration {
                std::thread::sleep(frame_duration - elapsed);
            }
            *last = std::time::Instant::now();
        };

        for (stream, packet) in input.packets() {
            if stream.index() != video_index {
                continue;
            }

            decoder.send_packet(&packet)?;
            while decoder.receive_frame(&mut decoded).is_ok() {
                scaler.run(&decoded, &mut rgb)?;
                pace(&mut last_frame_time);
                if tx.send(to_ascii_frame(&rgb)).is_err() {
                    return Ok(());
                }
            }
        }

        decoder.send_eof()?;
        while decoder.receive_frame(&mut decoded).is_ok() {
            scaler.run(&decoded, &mut rgb)?;
            pace(&mut last_frame_time);
            let _ = tx.send(to_ascii_frame(&rgb));
        }

        finished.store(true, Ordering::Relaxed);
        Ok(())
        })(); // end inner closure -- errors are silently swallowed, never printed to stderr
    });

    Ok(rx)
}

fn render_ascii(buffer: &mut Buffer, area: Rect, ascii: &AsciiFrame, intensity: f32) {
    let content_width = min(ascii.width, area.width);
    let content_height = min(ascii.height, area.height);
    let offset_x = area.x + (area.width - content_width) / 2;
    let offset_y = area.y + (area.height - content_height) / 2;
    // Crop from the centre of the source when it is larger than the panel.
    let src_x = ascii.width.saturating_sub(content_width) / 2;
    let src_y = ascii.height.saturating_sub(content_height) / 2;

    for y in 0..content_height {
        for x in 0..content_width {
            let index = (src_y + y) as usize * ascii.width as usize + (src_x + x) as usize;
            let (glyph, r, g, b) = ascii.cells[index];
            let scanline = if y % 2 == 0 { 0.84 } else { 1.0 };
            let factor = (intensity * scanline).clamp(0.1, 1.2);
            let fg = scale_rgb(r, g, b, factor);
            let bg = scale_rgb(r, g, b, factor * 0.16);

            if let Some(cell) = buffer.cell_mut((offset_x + x, offset_y + y)) {
                cell.set_char(glyph);
                cell.set_fg(fg);
                cell.set_bg(bg);
            }
        }
    }
}

fn scale_rgb(r: u8, g: u8, b: u8, factor: f32) -> Color {
    Color::Rgb(
        (r as f32 * factor).clamp(0.0, 255.0) as u8,
        (g as f32 * factor).clamp(0.0, 255.0) as u8,
        (b as f32 * factor).clamp(0.0, 255.0) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::fit_aspect;

    #[test]
    fn widescreen_fits_width_and_shrinks_height() {
        // 16:9 into a 120x60 panel: width-bound, height = 120 / 3.56 ≈ 34.
        let (w, h) = fit_aspect(1920, 1080, 120, 60);
        assert_eq!(w, 120);
        assert!((32..=36).contains(&h), "h = {}", h);
    }

    #[test]
    fn tall_source_fits_height() {
        let (w, h) = fit_aspect(1080, 1920, 120, 30);
        assert_eq!(h, 30);
        assert!(w < 120);
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        assert_eq!(fit_aspect(0, 0, 10, 5), (10, 5));
        assert_eq!(fit_aspect(100, 100, 0, 0), (1, 1));
    }
}
