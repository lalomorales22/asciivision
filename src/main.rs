use anyhow::{Context, Result};
use clap::Parser;
use crossbeam_channel::{bounded, Receiver};
use crossterm::event::{self, Event, KeyCode};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};
use std::{
    cmp::min,
    time::{Duration, Instant},
};
use tachyonfx::{fx, EffectManager, Interpolation};

use ffmpeg_next as ff;
use ff::format::context::Input;
use ff::format::Pixel;
use ff::software::scaling::{context::Context as Scaler, flag::Flags};
use ff::util::frame::video::Video;

/// ASCII palette from light→dark (tune to taste)
const PALETTE: &[u8] = b" .'`^\",:;Il!i><~+_-?][}{1)(|\\tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$";

#[derive(Parser, Debug)]
#[command(name = "GPT-5 ASCIIVision", about = "Play MP4s as ASCII inside a CRT-styled terminal")]
struct Args {
    /// Path to an .mp4 (H.264/H.265/etc supported by system FFmpeg)
    input: String,
    /// Target max width in terminal cells (height auto)
    #[arg(long, default_value_t = 140)]
    max_width: u16,
    /// Limit FPS (0 = use stream rate)
    #[arg(long, default_value_t = 0)]
    fps_cap: u32,
    /// Force monochrome
    #[arg(long, default_value_t = false)]
    mono: bool,
}

struct AsciiFrame {
    w: u16,
    h: u16,
    /// Packed cells: (ch, r, g, b) row-major
    cells: Vec<(char, u8, u8, u8)>,
}

fn luminance(r: u8, g: u8, b: u8) -> u8 {
    // Rec. 601-ish luma
    let y = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    y as u8
}

fn ascii_for(r: u8, g: u8, b: u8) -> char {
    let y = luminance(r, g, b) as usize;
    let idx = (y * (PALETTE.len() - 1)) / 255;
    PALETTE[idx] as char
}

fn to_ascii_frame(rgb: &Video, mono: bool) -> AsciiFrame {
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
            if mono {
                let y = luminance(r, g, b);
                cells.push((ch, y, y, y));
            } else {
                cells.push((ch, r, g, b));
            }
        }
    }

    AsciiFrame { w: w as u16, h: h as u16, cells }
}

fn open_decoder(path: &str) -> Result<(Input, usize, ff::codec::decoder::Video, (u32, u32), Option<(u32, u32)>)> {
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
    Scaler::get(src_fmt, src_w, src_h, Pixel::RGB24, dst_w, dst_h, Flags::BILINEAR)
        .context("create scaler")
}

/// Decode thread: emits already-resized RGB frames as ASCII-ready Video frames
fn spawn_decode(
    path: String,
    target_w: u16,
    target_h: u16,
) -> Result<Receiver<AsciiFrame>> {
    let (tx, rx) = bounded::<AsciiFrame>(8);

    std::thread::spawn(move || -> Result<()> {
        let (mut ictx, v_idx, mut dec, (src_w, src_h), _) = open_decoder(&path)?;
        let mut scaler = build_scaler(dec.format(), src_w, src_h, target_w as u32, target_h as u32)?;

        let mut rgb = Video::new(Pixel::RGB24, target_w as u32, target_h as u32);
        let mut frame = Video::empty();

        for (stream, packet) in ictx.packets() {
            if stream.index() != v_idx {
                continue;
            }
            dec.send_packet(&packet)?;

            while dec.receive_frame(&mut frame).is_ok() {
                scaler.run(&frame, &mut rgb)?;
                let ascii = to_ascii_frame(&rgb, false);
                if tx.send(ascii).is_err() {
                    return Ok(()); // UI gone
                }
            }
        }
        // flush
        dec.send_eof()?;
        while dec.receive_frame(&mut frame).is_ok() {
            scaler.run(&frame, &mut rgb)?;
            let ascii = to_ascii_frame(&rgb, false);
            let _ = tx.send(ascii);
        }
        Ok(())
    });

    Ok(rx)
}

fn size_for_terminal(area: Rect, max_w: u16, src_aspect: f32) -> (u16, u16) {
    // account for terminal cell aspect (roughly 2:1 height:width) → scale height down
    let usable_w = min(max_w, area.width.saturating_sub(4));
    let w = usable_w;
    let h_float = (w as f32 / src_aspect) * 0.5; // cell aspect correction
    let h = min(area.height.saturating_sub(4), h_float.max(4.0) as u16);
    (w, h)
}

fn render_ascii(frame: &mut Frame, area: Rect, af: &AsciiFrame) {
    // Center the ASCII inside area
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

fn main() -> Result<()> {
    let args = Args::parse();

    // probe source to compute aspect & target size
    ff::init()?;
    let (ictx, _v_idx, _dec, (src_w, src_h), fps) = open_decoder(&args.input)?;
    let src_aspect = src_w as f32 / src_h as f32;

    // init terminal
    let mut term = ratatui::init();
    ratatui::crossterm::terminal::enable_raw_mode()?;
    let mut last = Instant::now();

    // UI bezel + effects
    let mut effects: EffectManager<()> = EffectManager::default();
    // Power-on sweep + slight HSL drift + occasional glitch + term256 palette flicker
    let boot = fx::sequence(&[
        fx::fade_from(Color::Black, Color::Reset, (800, Interpolation::Linear)),
        fx::coalesce(500),
    ]);
    let drift = fx::hsl_shift(Some([0.0, 0.0, 0.05]), None, (6_000, Interpolation::SineInOut));
    // Glitch effect - commented out as the function is not public
    // let glitch = fx::glitch_fx(0.02, 0.06, (1200, Interpolation::Linear));
    effects.add_effect(fx::parallel(&[boot, drift]));

    // calc playable area + spawn decode at that resolution
    let area0 = term.get_frame().area();
    let tv_inner = area0.inner(Margin { horizontal: 3, vertical: 2 });
    let (tw, th) = size_for_terminal(tv_inner, args.max_width, src_aspect);
    drop(ictx); // we re-open inside decoder thread at chosen size
    let rx = spawn_decode(args.input.clone(), tw, th)?;

    // timing
    let frame_ns = if args.fps_cap > 0 {
        (1_000_000_000u64 / args.fps_cap as u64) as u64
    } else if let Some((num, den)) = fps {
        if den == 0 { 33_000_000 } else { (1_000_000_000u64 * den as u64) / num as u64 }
    } else {
        33_000_000 // ~30fps fallback
    };
    let frame_dt = Duration::from_nanos(frame_ns);

    let mut paused = false;
    let mut glitch_on = true;
    let mut mono = args.mono;
    let mut latest: Option<AsciiFrame> = None;

    'outer: loop {
        // input
        while event::poll(Duration::from_millis(1))? {
            match event::read()? {
                Event::Key(k) if k.code == KeyCode::Char('q') || k.code == KeyCode::Esc => break 'outer,
                Event::Key(k) if k.code == KeyCode::Char(' ') => paused = !paused,
                Event::Key(k) if k.code == KeyCode::Char('g') => {
                    glitch_on = !glitch_on;
                    // Glitch effect not available
                }
                Event::Key(k) if k.code == KeyCode::Char('c') => mono = !mono,
                _ => {}
            }
        }

        // receive next frame (or reuse latest if paused)
        if !paused {
            if let Ok(mut af) = rx.try_recv() {
                if mono {
                    // quick mono remap without re-scaling
                    for cell in &mut af.cells {
                        let y = luminance(cell.1, cell.2, cell.3);
                        cell.1 = y; cell.2 = y; cell.3 = y;
                    }
                }
                latest = Some(af);
            }
        }

        // draw
        term.draw(|f| {
            let area = f.area();
            let block = Block::default()
                .title(" GPT-5 // ASCIIVision ")
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Thick);
            let inner = area.inner(Margin { horizontal: 2, vertical: 1 });
            f.render_widget(block, area);

            // bezel labels
            let footer = Paragraph::new("Q quit  Space pause  G glitch  C color/mono")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Gray));
            f.render_widget(footer, Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1));

            if let Some(ref af) = latest {
                render_ascii(f, inner, af);
            } else {
                let msg = Paragraph::new("Loading…").alignment(Alignment::Center);
                f.render_widget(msg, inner);
            }

            // apply TV-ish post: we process on whole area to affect bezel + content
            let elapsed = last.elapsed();
            last = Instant::now();
            effects.process_effects(elapsed.into(), f.buffer_mut(), area);
        })?;

        // pacing
        std::thread::sleep(frame_dt);
    }

    ratatui::restore();
    Ok(())
}

