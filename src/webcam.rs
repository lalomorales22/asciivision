use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use ffmpeg_next::codec;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::media::Type;
use ffmpeg_next::software::scaling::Flags;
use ffmpeg_next::util::frame::Video;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use crate::render::RgbFrame;
use crate::video::to_rgb_frame;

/// Which live capture source a [`WebcamCapture`] pulls from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureSource {
    /// A physical camera device.
    Camera,
    /// The desktop / a display (screen sharing).
    Screen,
}

impl Default for CaptureSource {
    fn default() -> Self {
        CaptureSource::Camera
    }
}

#[derive(Debug, Clone)]
pub struct WebcamConfig {
    /// Device selector. For a camera this is the OS device index ("0"); for a
    /// screen it is the platform's screen selector (filled in per-OS when the
    /// source is `Screen`, so callers may leave it at the default).
    pub device: String,
    /// Maximum output pixel box (aspect ratio is preserved within it). These
    /// are PIXELS now, not cells -- the renderer handles cell-aspect + scaling.
    pub width: u16,
    pub height: u16,
    pub fps_cap: u32,
    pub source: CaptureSource,
}

impl Default for WebcamConfig {
    fn default() -> Self {
        Self {
            device: "0".to_string(),
            width: 320,
            height: 240,
            fps_cap: 30,
            source: CaptureSource::Camera,
        }
    }
}

pub struct WebcamCapture {
    receiver: Receiver<RgbFrame>,
    active: Arc<AtomicBool>,
    error: Arc<parking_lot::Mutex<Option<String>>>,
}

impl WebcamCapture {
    pub fn start(config: WebcamConfig) -> Result<Self> {
        let (tx, rx) = bounded::<RgbFrame>(4);
        let active = Arc::new(AtomicBool::new(true));
        let active_clone = active.clone();
        let error: Arc<parking_lot::Mutex<Option<String>>> =
            Arc::new(parking_lot::Mutex::new(None));
        let error_clone = error.clone();

        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let Err(e) = capture_loop(&config, &tx, &active_clone) {
                    let msg = format!("{}", e);
                    *error_clone.lock() = Some(msg);
                }
            }));
            if result.is_err() {
                *error_clone.lock() = Some("capture thread panicked".to_string());
            }
        });

        Ok(Self {
            receiver: rx,
            active,
            error,
        })
    }

    pub fn try_recv(&self) -> Option<RgbFrame> {
        self.receiver.try_recv().ok()
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().clone()
    }
}

impl Drop for WebcamCapture {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Relaxed);
    }
}

fn open_capture_device(
    device_spec: &str,
    format_name: &str,
    opts: ffmpeg_next::Dictionary,
) -> Result<ffmpeg_next::format::context::Input> {
    unsafe {
        let format_cstr = CString::new(format_name)?;
        let device_cstr = CString::new(device_spec)?;
        let fmt = ffmpeg_sys_next::av_find_input_format(format_cstr.as_ptr());
        if fmt.is_null() {
            return Err(anyhow!("input format not found: {}", format_name));
        }
        // Transfer ownership of the dictionary to FFmpeg via disown() to prevent
        // double-free: avformat_open_input takes ownership of the AVDictionary.
        let mut options_ptr = opts.disown();
        let mut ictx_ptr: *mut ffmpeg_sys_next::AVFormatContext = std::ptr::null_mut();
        let ret = ffmpeg_sys_next::avformat_open_input(
            &mut ictx_ptr,
            device_cstr.as_ptr(),
            fmt,
            &mut options_ptr,
        );
        // Free any remaining options that FFmpeg didn't consume
        if !options_ptr.is_null() {
            ffmpeg_sys_next::av_dict_free(&mut options_ptr);
        }
        if ret < 0 {
            return Err(anyhow!(
                "failed to open capture device '{}' (code {})",
                device_spec,
                ret
            ));
        }
        if ictx_ptr.is_null() {
            return Err(anyhow!("capture device '{}' returned null context", device_spec));
        }
        Ok(ffmpeg_next::format::context::Input::wrap(ictx_ptr))
    }
}

/// Resolve the (device_spec, format_name, options) triple for the configured
/// source + OS. Screen capture and camera capture differ only here; everything
/// downstream (decode -> swscale -> RGB24 -> RgbFrame) is source-agnostic.
fn resolve_input(config: &WebcamConfig) -> (String, &'static str, ffmpeg_next::Dictionary<'static>) {
    let mut opts = ffmpeg_next::Dictionary::new();
    match config.source {
        CaptureSource::Camera => {
            if cfg!(target_os = "macos") {
                opts.set("framerate", &config.fps_cap.to_string());
                opts.set("pixel_format", "uyvy422");
                (config.device.clone(), "avfoundation", opts)
            } else if cfg!(target_os = "linux") {
                (config.device.clone(), "v4l2", opts)
            } else {
                (config.device.clone(), "dshow", opts)
            }
        }
        CaptureSource::Screen => {
            // Screen sources deliver BGRA-family formats; do NOT force a camera
            // pixel_format. swscale converts whatever the device reports.
            if cfg!(target_os = "macos") {
                // avfoundation exposes each display as "Capture screen N",
                // indexed AFTER the cameras. Default device "0" here means the
                // caller didn't override; the app fills in the real screen
                // index. capture_cursor makes it feel like a share.
                opts.set("framerate", &config.fps_cap.to_string());
                opts.set("capture_cursor", "1");
                (config.device.clone(), "avfoundation", opts)
            } else if cfg!(target_os = "linux") {
                opts.set("framerate", &config.fps_cap.to_string());
                opts.set("draw_mouse", "1");
                let dev = if config.device == "0" || config.device.is_empty() {
                    ":0.0".to_string()
                } else {
                    config.device.clone()
                };
                (dev, "x11grab", opts)
            } else {
                opts.set("framerate", &config.fps_cap.to_string());
                opts.set("draw_mouse", "1");
                ("desktop".to_string(), "gdigrab", opts)
            }
        }
    }
}

fn capture_loop(
    config: &WebcamConfig,
    tx: &Sender<RgbFrame>,
    active: &Arc<AtomicBool>,
) -> Result<()> {
    ffmpeg_next::init()?;

    let (device_spec, format_name, opts) = resolve_input(config);
    let mut ictx = open_capture_device(&device_spec, format_name, opts)?;

    let video_stream = ictx
        .streams()
        .best(Type::Video)
        .ok_or_else(|| anyhow!("no video stream found"))?;
    let video_idx = video_stream.index();
    let dec_ctx = codec::context::Context::from_parameters(video_stream.parameters())
        .context("decoder context")?;
    let mut decoder = dec_ctx.decoder().video().context("video decoder")?;

    // Output box preserving the source's SQUARE-pixel aspect ratio. The renderer
    // corrects for terminal-cell aspect, so unlike the old glyph path we do NOT
    // apply a 2x horizontal stretch here.
    let src_w = decoder.width() as f32;
    let src_h = decoder.height() as f32;
    let src_aspect = if src_h > 0.0 { src_w / src_h } else { 4.0 / 3.0 };
    let (bw, bh) = (config.width as f32, config.height as f32);
    let mut out_w = bw;
    let mut out_h = (out_w / src_aspect).round();
    if out_h > bh {
        out_h = bh;
        out_w = (out_h * src_aspect).round();
    }
    let out_w = out_w.max(2.0) as u32;
    let out_h = out_h.max(2.0) as u32;

    let mut scaler = ffmpeg_next::software::scaling::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        Pixel::RGB24,
        out_w,
        out_h,
        Flags::BILINEAR,
    )
    .context("scaler")?;

    let mut decoded = ffmpeg_next::frame::Video::empty();
    let mut rgb = Video::new(Pixel::RGB24, out_w, out_h);

    let frame_dur = if config.fps_cap > 0 {
        std::time::Duration::from_millis(1000 / config.fps_cap as u64)
    } else {
        std::time::Duration::ZERO
    };
    let mut last = std::time::Instant::now();

    for (_stream, packet) in ictx.packets() {
        if !active.load(Ordering::Relaxed) {
            break;
        }
        if _stream.index() != video_idx {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            continue;
        }
        while decoder.receive_frame(&mut decoded).is_ok() {
            let elapsed = last.elapsed();
            if frame_dur > std::time::Duration::ZERO && elapsed < frame_dur {
                thread::sleep(frame_dur - elapsed);
            }
            scaler.run(&decoded, &mut rgb)?;
            let frame = to_rgb_frame(&rgb, out_w as u16, out_h as u16);
            if tx.send(frame).is_err() {
                return Ok(());
            }
            last = std::time::Instant::now();
        }
    }

    Ok(())
}
