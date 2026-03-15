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

const PALETTE: &[u8] = b" .'`^\",:;Il!i><~+_-?][}{1)(|\\tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$";

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

fn ascii_for(r: u8, g: u8, b: u8) -> char {
    let y = luminance(r, g, b) as usize;
    let index = (y * (PALETTE.len() - 1)) / 255;
    PALETTE[index] as char
}

fn to_ascii_frame(rgb: &Video) -> AsciiFrame {
    let width = rgb.width() as usize;
    let height = rgb.height() as usize;
    let stride = rgb.stride(0);
    let data = rgb.data(0);
    let mut cells = Vec::with_capacity(width * height);

    for y in 0..height {
        let row = &data[(y * stride) as usize..((y * stride) as usize + width * 3)];
        for x in 0..width {
            let index = x * 3;
            let (r, g, b) = (row[index], row[index + 1], row[index + 2]);
            cells.push((ascii_for(r, g, b), r, g, b));
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

fn spawn_decode(
    path: &Path,
    decode_size: (u16, u16),
    finished: Arc<AtomicBool>,
) -> Result<Receiver<AsciiFrame>> {
    let path = path.to_path_buf();
    let (tx, rx) = bounded(8);
    let (target_width, target_height) = decode_size;

    std::thread::spawn(move || {
        let _result: Result<()> = (|| {
        let (mut input, video_index, mut decoder, (src_width, src_height), _) =
            open_decoder(path.as_path())?;
        let mut scaler = build_scaler(
            decoder.format(),
            src_width,
            src_height,
            target_width as u32,
            target_height as u32,
        )?;
        let mut rgb = Video::new(Pixel::RGB24, target_width as u32, target_height as u32);
        let mut decoded = Video::empty();

        for (stream, packet) in input.packets() {
            if stream.index() != video_index {
                continue;
            }

            decoder.send_packet(&packet)?;
            while decoder.receive_frame(&mut decoded).is_ok() {
                scaler.run(&decoded, &mut rgb)?;
                if tx.send(to_ascii_frame(&rgb)).is_err() {
                    return Ok(());
                }
            }
        }

        decoder.send_eof()?;
        while decoder.receive_frame(&mut decoded).is_ok() {
            scaler.run(&decoded, &mut rgb)?;
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

    for y in 0..content_height {
        for x in 0..content_width {
            let index = y as usize * ascii.width as usize + x as usize;
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
