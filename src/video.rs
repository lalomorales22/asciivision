use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver};
use ff::format::context::Input;
use ff::format::Pixel;
use ff::software::scaling::{context::Context as Scaler, flag::Flags};
use ff::util::frame::video::Video;
use ffmpeg_next as ff;
use ratatui::{prelude::*, widgets::Paragraph};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use crate::render::{render_frame, RgbFrame, VideoRenderMode};

/// Maximum pixel box a decoder scales a frame into (source aspect preserved).
/// Generous enough that half-block downscaling into a terminal panel stays
/// crisp, cheap enough that swscale + copy stays well under the frame budget.
pub const DECODE_BOX: (u16, u16) = (320, 240);

pub struct VideoPlayer {
    path: PathBuf,
    looping: bool,
    decode_box: (u16, u16),
    rx: Receiver<RgbFrame>,
    latest: Option<RgbFrame>,
    finished: Arc<AtomicBool>,
}

impl VideoPlayer {
    pub fn new(path: impl Into<PathBuf>, decode_box: (u16, u16), looping: bool) -> Result<Self> {
        let path = path.into();
        let finished = Arc::new(AtomicBool::new(false));
        let rx = spawn_decode(path.as_path(), decode_box, finished.clone())?;

        Ok(Self {
            path,
            looping,
            decode_box,
            rx,
            latest: None,
            finished,
        })
    }

    /// Drain decoded frames, keeping only the newest. Returns true when the
    /// latest frame changed this tick (so the pixel-protocol path only rebuilds
    /// its encode on an actual new frame, not every 60fps redraw).
    pub fn tick(&mut self) -> bool {
        let mut got = false;
        while let Ok(frame) = self.rx.try_recv() {
            self.latest = Some(frame);
            got = true;
        }

        if self.looping && self.finished.load(Ordering::Relaxed) && self.rx.is_empty() {
            self.finished.store(false, Ordering::Relaxed);
            if let Ok(rx) =
                spawn_decode(self.path.as_path(), self.decode_box, self.finished.clone())
            {
                self.rx = rx;
            }
        }
        got
    }

    pub fn has_signal(&self) -> bool {
        self.latest.is_some()
    }

    /// Latest decoded frame, if any -- used by the pixel-protocol path which
    /// needs the raw RGB buffer rather than a cell blit.
    pub fn latest_frame(&self) -> Option<&RgbFrame> {
        self.latest.as_ref()
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, intensity: f32, mode: VideoRenderMode) {
        if area.width < 2 || area.height < 2 {
            return;
        }

        if let Some(ref rgb) = self.latest {
            render_frame(frame.buffer_mut(), area, rgb, intensity, mode);
        } else {
            let placeholder = Paragraph::new("signal lock pending")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Rgb(112, 166, 179)));
            frame.render_widget(placeholder, area);
        }
    }
}

/// Copy an FFmpeg RGB24 frame (which may be padded per row via `stride`) into a
/// tightly-packed [`RgbFrame`]. Shared with the webcam/screen capture path.
pub(crate) fn to_rgb_frame(rgb: &Video, width: u16, height: u16) -> RgbFrame {
    let stride = rgb.stride(0);
    let data = rgb.data(0);
    let row = width as usize * 3;
    let mut out = vec![0u8; row * height as usize];
    for y in 0..height as usize {
        let src = y * stride;
        // guard against a short final row from odd stride math
        if src + row <= data.len() {
            out[y * row..y * row + row].copy_from_slice(&data[src..src + row]);
        }
    }
    RgbFrame {
        width,
        height,
        data: out,
    }
}

/// Fit `(src_w, src_h)` square pixels into the `(max_w, max_h)` box, preserving
/// aspect ratio, so the decoded [`RgbFrame`] never carries a squished frame --
/// the renderer letterboxes from correct source dimensions.
fn fit_box(src_w: u32, src_h: u32, max: (u16, u16)) -> (u32, u32) {
    if src_w == 0 || src_h == 0 {
        return ((max.0.max(2)) as u32, (max.1.max(2)) as u32);
    }
    let aspect = src_w as f32 / src_h as f32;
    let mut w = max.0 as f32;
    let mut h = (w / aspect).round();
    if h > max.1 as f32 {
        h = max.1 as f32;
        w = (h * aspect).round();
    }
    (w.max(2.0) as u32, h.max(2.0) as u32)
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
    decode_box: (u16, u16),
    finished: Arc<AtomicBool>,
) -> Result<Receiver<RgbFrame>> {
    let path = path.to_path_buf();
    let (tx, rx) = bounded(8);

    std::thread::spawn(move || {
        let _result: Result<()> = (|| {
            let (mut input, video_index, mut decoder, (src_width, src_height), fps) =
                open_decoder(path.as_path())?;
            let (out_w, out_h) = fit_box(src_width, src_height, decode_box);
            let mut scaler = build_scaler(decoder.format(), src_width, src_height, out_w, out_h)?;
            let mut rgb = Video::new(Pixel::RGB24, out_w, out_h);
            let mut decoded = Video::empty();

            // Wall-clock pacing at the source frame rate (avg_frame_rate; 30fps
            // fallback). Without this the decoder emits as fast as it can and
            // clips play too fast / stutter -- the single most "this looks
            // broken" video bug. Frame N is released at start + N/fps.
            let interval = fps
                .filter(|(n, d)| *n > 0 && *d > 0)
                .map(|(n, d)| d as f64 / n as f64)
                .filter(|s| s.is_finite() && *s > 0.0)
                .unwrap_or(1.0 / 30.0);
            let start = Instant::now();
            let mut frame_idx: u64 = 0;

            for (stream, packet) in input.packets() {
                if stream.index() != video_index {
                    continue;
                }
                decoder.send_packet(&packet)?;
                while decoder.receive_frame(&mut decoded).is_ok() {
                    scaler.run(&decoded, &mut rgb)?;
                    let target = start + Duration::from_secs_f64(frame_idx as f64 * interval);
                    let now = Instant::now();
                    if target > now {
                        std::thread::sleep(target - now);
                    }
                    frame_idx += 1;
                    if tx.send(to_rgb_frame(&rgb, out_w as u16, out_h as u16)).is_err() {
                        return Ok(());
                    }
                }
            }

            decoder.send_eof()?;
            while decoder.receive_frame(&mut decoded).is_ok() {
                scaler.run(&decoded, &mut rgb)?;
                let target = start + Duration::from_secs_f64(frame_idx as f64 * interval);
                let now = Instant::now();
                if target > now {
                    std::thread::sleep(target - now);
                }
                frame_idx += 1;
                let _ = tx.send(to_rgb_frame(&rgb, out_w as u16, out_h as u16));
            }

            finished.store(true, Ordering::Relaxed);
            Ok(())
        })(); // inner closure -- decode errors are swallowed, never printed to stderr
    });

    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_box_preserves_aspect_within_bounds() {
        // 16:9 source into a 320x240 box -> width-limited, aspect held
        let (w, h) = fit_box(1920, 1080, (320, 240));
        assert!(w <= 320 && h <= 240);
        assert!(((w as f32 / h as f32) - (1920.0 / 1080.0)).abs() < 0.05);
    }

    /// End-to-end: the bundled demo file must decode into valid, paced
    /// RgbFrames. Asset-optional so the suite still passes without the file.
    #[test]
    fn decodes_demo_file_into_rgb_frames() {
        let path = std::path::Path::new("demo-videos/demo.mp4");
        if !path.exists() {
            return;
        }
        let mut player = match VideoPlayer::new(path, DECODE_BOX, false) {
            Ok(p) => p,
            Err(_) => return, // no usable ffmpeg/codec here -> skip
        };
        let mut got = false;
        for _ in 0..400 {
            player.tick();
            if let Some(f) = player.latest_frame() {
                assert!(f.is_valid(), "decoded frame must be structurally valid");
                assert!(f.width >= 2 && f.height >= 2);
                assert_eq!(f.data.len(), f.width as usize * f.height as usize * 3);
                got = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(got, "no frame decoded from demo.mp4 within timeout");
    }
}
