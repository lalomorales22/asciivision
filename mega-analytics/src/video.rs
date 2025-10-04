use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};
use std::{
    cmp::min,
    time::Instant,
};
use tachyonfx::{fx, EffectManager, Interpolation};

use ffmpeg_next as ff;
use ff::format::context::Input;
use ff::format::Pixel;
use ff::software::scaling::{context::Context as Scaler, flag::Flags};
use ff::util::frame::video::Video;

/// ASCII palette from light→dark
const PALETTE: &[u8] = b" .'`^\",:;Il!i><~+_-?][}{1)(|\\tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$";

pub struct AsciiFrame {
    w: u16,
    h: u16,
    /// Packed cells: (ch, r, g, b) row-major
    cells: Vec<(char, u8, u8, u8)>,
}

fn luminance(r: u8, g: u8, b: u8) -> u8 {
    let y = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    y as u8
}

fn ascii_for(r: u8, g: u8, b: u8) -> char {
    let y = luminance(r, g, b) as usize;
    let idx = (y * (PALETTE.len() - 1)) / 255;
    PALETTE[idx] as char
}

fn to_ascii_frame(rgb: &Video) -> AsciiFrame {
    let w = rgb.width() as usize;
    let h = rgb.height() as usize;
    let stride = rgb.stride(0);
    let data = rgb.data(0);

    let mut cells = Vec::with_capacity(w * h);
    for y in 0..h {
        let row = &data[(y * stride) as usize..((y * stride) as usize + w * 3)];
        for x in 0..w {
            let i = x * 3;
            let (r, g, b) = (row[i], row[i + 1], row[i + 2]);
            let ch = ascii_for(r, g, b);
            cells.push((ch, r, g, b));
        }
    }

    AsciiFrame {
        w: w as u16,
        h: h as u16,
        cells,
    }
}

fn open_decoder(
    path: &str,
) -> Result<(
    Input,
    usize,
    ff::codec::decoder::Video,
    (u32, u32),
    Option<(u32, u32)>,
)> {
    ff::init().context("init ffmpeg")?;
    let ictx = ff::format::input(&path).with_context(|| format!("open input {path}"))?;

    let stream = ictx
        .streams()
        .best(ff::media::Type::Video)
        .context("no video stream")?;
    let idx = stream.index();

    let dec_ctx = ff::codec::context::Context::from_parameters(stream.parameters())?;
    let decoder = dec_ctx.decoder().video()?;

    let src_wh = (decoder.width(), decoder.height());
    let fps = if stream.avg_frame_rate() != ff::Rational(0, 0) {
        let r = stream.avg_frame_rate();
        Some((r.numerator() as u32, r.denominator() as u32))
    } else {
        None
    };
    Ok((ictx, idx, decoder, src_wh, fps))
}

fn build_scaler(
    src_fmt: Pixel,
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Result<Scaler> {
    Scaler::get(
        src_fmt,
        src_w,
        src_h,
        Pixel::RGB24,
        dst_w,
        dst_h,
        Flags::BILINEAR,
    )
    .context("create scaler")
}

fn spawn_decode(path: String, target_w: u16, target_h: u16, finished_flag: Arc<AtomicBool>) -> Result<Receiver<AsciiFrame>> {
    let (tx, rx) = bounded::<AsciiFrame>(8);

    std::thread::spawn(move || -> Result<()> {
        let (mut ictx, v_idx, mut dec, (src_w, src_h), _) = open_decoder(&path)?;
        let mut scaler = build_scaler(
            dec.format(),
            src_w,
            src_h,
            target_w as u32,
            target_h as u32,
        )?;

        let mut rgb = Video::new(Pixel::RGB24, target_w as u32, target_h as u32);
        let mut frame = Video::empty();

        for (stream, packet) in ictx.packets() {
            if stream.index() != v_idx {
                continue;
            }
            dec.send_packet(&packet)?;

            while dec.receive_frame(&mut frame).is_ok() {
                scaler.run(&frame, &mut rgb)?;
                let ascii = to_ascii_frame(&rgb);
                if tx.send(ascii).is_err() {
                    return Ok(()); // UI gone
                }
            }
        }
        // flush
        dec.send_eof()?;
        while dec.receive_frame(&mut frame).is_ok() {
            scaler.run(&frame, &mut rgb)?;
            let ascii = to_ascii_frame(&rgb);
            let _ = tx.send(ascii);
        }

        // Mark as finished only after all frames are sent
        finished_flag.store(true, Ordering::Relaxed);
        Ok(())
    });

    Ok(rx)
}

#[allow(dead_code)]
fn size_for_terminal(area: Rect, max_w: u16, src_aspect: f32) -> (u16, u16) {
    let usable_w = min(max_w, area.width.saturating_sub(4));
    let w = usable_w;
    let h_float = (w as f32 / src_aspect) * 0.5; // cell aspect correction
    let h = min(area.height.saturating_sub(4), h_float.max(4.0) as u16);
    (w, h)
}

fn render_ascii(frame: &mut Frame, area: Rect, af: &AsciiFrame) {
    let content_w = min(af.w, area.width);
    let content_h = min(af.h, area.height);

    let x0 = area.x + (area.width - content_w) / 2;
    let y0 = area.y + (area.height - content_h) / 2;

    let buf = frame.buffer_mut();
    for y in 0..content_h {
        for x in 0..content_w {
            let i = (y as usize * af.w as usize + x as usize) as usize;
            let (ch, r, g, b) = af.cells[i];
            if let Some(cell) = buf.cell_mut((x0 + x, y0 + y)) {
                cell.set_char(ch);
                cell.set_fg(Color::Rgb(r, g, b));
            }
        }
    }
}

pub struct VideoPlayer {
    rx: Receiver<AsciiFrame>,
    latest: Option<AsciiFrame>,
    effects: EffectManager<()>,
    last_update: Instant,
    finished_flag: Arc<AtomicBool>,
    decoding_finished: bool,
}

impl VideoPlayer {
    pub fn new(path: &str) -> Result<Self> {
        ff::init()?;
        let (ictx, _v_idx, _dec, (src_w, src_h), _fps) = open_decoder(path)?;
        let _src_aspect = src_w as f32 / src_h as f32;

        // Use a reasonable default size
        let (tw, th) = (120, 30);
        drop(ictx);

        let finished_flag = Arc::new(AtomicBool::new(false));
        let rx = spawn_decode(path.to_string(), tw, th, finished_flag.clone())?;

        let effects: EffectManager<()> = EffectManager::default();
        // No effects - display video at natural brightness

        Ok(Self {
            rx,
            latest: None,
            effects,
            last_update: Instant::now(),
            finished_flag,
            decoding_finished: false,
        })
    }

    pub fn is_finished(&self) -> bool {
        // Only finished when decoding is done AND channel is empty (all frames rendered)
        self.decoding_finished && self.rx.is_empty()
    }

    pub fn render(&mut self, frame: &mut Frame) -> Result<()> {
        // Update decoding finished flag
        if !self.decoding_finished && self.finished_flag.load(Ordering::Relaxed) {
            self.decoding_finished = true;
        }

        // Try to receive ONE new frame (not all frames at once!)
        if let Ok(af) = self.rx.try_recv() {
            self.latest = Some(af);
        }

        let area = frame.area();
        let block = Block::default()
            .title(" MEGA-Analytics // Loading ")
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Thick);
        let inner = area.inner(Margin {
            horizontal: 2,
            vertical: 1,
        });
        frame.render_widget(block, area);

        if let Some(ref af) = self.latest {
            render_ascii(frame, inner, af);
        } else {
            let msg = Paragraph::new("Loading…").alignment(Alignment::Center);
            frame.render_widget(msg, inner);
        }

        // Apply effects
        let elapsed = self.last_update.elapsed();
        self.last_update = Instant::now();
        self.effects
            .process_effects(elapsed.into(), frame.buffer_mut(), area);

        Ok(())
    }
}
