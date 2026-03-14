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

use crate::message::WsAsciiFrame;
use crate::video::AsciiFrame;

const PALETTE: &[u8] = b" .'`^\",:;Il!i><~+_-?][}{1)(|\\tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$";

#[derive(Debug, Clone)]
pub struct WebcamConfig {
    pub device: String,
    pub width: u16,
    pub height: u16,
    pub fps_cap: u32,
}

impl Default for WebcamConfig {
    fn default() -> Self {
        Self {
            device: "0".to_string(),
            width: 160,
            height: 48,
            fps_cap: 30,
        }
    }
}

pub struct WebcamCapture {
    receiver: Receiver<AsciiFrame>,
    active: Arc<AtomicBool>,
    error: Arc<parking_lot::Mutex<Option<String>>>,
}

impl WebcamCapture {
    pub fn start(config: WebcamConfig) -> Result<Self> {
        let (tx, rx) = bounded::<AsciiFrame>(4);
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
                *error_clone.lock() = Some("webcam thread panicked".to_string());
            }
        });

        Ok(Self {
            receiver: rx,
            active,
            error,
        })
    }

    pub fn try_recv(&self) -> Option<AsciiFrame> {
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

fn luminance(r: u8, g: u8, b: u8) -> u8 {
    (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32).min(255.0) as u8
}

fn ascii_for(r: u8, g: u8, b: u8) -> char {
    let y = luminance(r, g, b) as usize;
    let idx = (y * (PALETTE.len() - 1)) / 255;
    PALETTE[idx.min(PALETTE.len() - 1)] as char
}

fn open_webcam_device(device_spec: &str, format_name: &str, opts: ffmpeg_next::Dictionary) -> Result<ffmpeg_next::format::context::Input> {
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
            return Err(anyhow!("failed to open webcam device '{}' (code {})", device_spec, ret));
        }
        if ictx_ptr.is_null() {
            return Err(anyhow!("webcam device '{}' returned null context", device_spec));
        }
        Ok(ffmpeg_next::format::context::Input::wrap(ictx_ptr))
    }
}

fn capture_loop(config: &WebcamConfig, tx: &Sender<AsciiFrame>, active: &Arc<AtomicBool>) -> Result<()> {
    ffmpeg_next::init()?;

    let (device_spec, format_name) = if cfg!(target_os = "macos") {
        (config.device.clone(), "avfoundation")
    } else if cfg!(target_os = "linux") {
        (config.device.clone(), "v4l2")
    } else {
        (config.device.clone(), "dshow")
    };

    let mut opts = ffmpeg_next::Dictionary::new();
    if cfg!(target_os = "macos") {
        opts.set("framerate", &config.fps_cap.to_string());
        opts.set("pixel_format", "uyvy422");
    }

    let mut ictx = if !format_name.is_empty() {
        open_webcam_device(&device_spec, &format_name, opts)?
    } else {
        ffmpeg_next::format::input_with_dictionary(&device_spec, opts)
            .map_err(|e| anyhow!("failed to open webcam '{}': {}", device_spec, e))?
    };

    let video_stream = ictx
        .streams()
        .best(Type::Video)
        .ok_or_else(|| anyhow!("no video stream found"))?;
    let video_idx = video_stream.index();
    let dec_ctx = codec::context::Context::from_parameters(video_stream.parameters())
        .context("decoder context")?;
    let mut decoder = dec_ctx.decoder().video().context("video decoder")?;

    // Compute output dimensions that preserve the webcam's native aspect ratio
    // accounting for terminal cells being ~2x taller than wide.
    let src_w = decoder.width() as f32;
    let src_h = decoder.height() as f32;
    let src_aspect = src_w / src_h;
    // Terminal cell aspect ratio correction: each cell is ~2x tall as it is wide,
    // so we need ~2x the columns to look right visually.
    let target_w = config.width as f32;
    let target_h = config.height as f32;
    let (out_w, out_h) = {
        let fit_h = target_h;
        let fit_w = (fit_h * src_aspect * 2.0).round();
        if fit_w <= target_w {
            (fit_w as u32, fit_h as u32)
        } else {
            let fit_w = target_w;
            let fit_h = (fit_w / (src_aspect * 2.0)).round();
            (fit_w as u32, fit_h as u32)
        }
    };
    let out_w = out_w.max(4);
    let out_h = out_h.max(4);

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
            let frame = rgb_to_ascii(&rgb, out_w as u16, out_h as u16);
            if tx.send(frame).is_err() {
                return Ok(());
            }
            last = std::time::Instant::now();
        }
    }

    Ok(())
}

fn rgb_to_ascii(rgb: &Video, width: u16, height: u16) -> AsciiFrame {
    let stride = rgb.stride(0);
    let data = rgb.data(0);
    let mut cells = Vec::with_capacity(width as usize * height as usize);

    for y in 0..height as usize {
        let row = &data[y * stride..y * stride + width as usize * 3];
        for x in 0..width as usize {
            let i = x * 3;
            let (r, g, b) = (row[i], row[i + 1], row[i + 2]);
            cells.push((ascii_for(r, g, b), r, g, b));
        }
    }

    AsciiFrame {
        width,
        height,
        cells,
    }
}

pub fn ascii_frame_to_ws(frame: &AsciiFrame) -> WsAsciiFrame {
    let mut ws = WsAsciiFrame::new(frame.width, frame.height);
    for (i, &(ch, r, g, b)) in frame.cells.iter().enumerate() {
        let idx = i * 4;
        if idx + 3 < ws.data.len() {
            ws.data[idx] = ch as u8;
            ws.data[idx + 1] = r;
            ws.data[idx + 2] = g;
            ws.data[idx + 3] = b;
        }
    }
    ws
}

pub fn ws_frame_to_ascii(ws: &WsAsciiFrame) -> AsciiFrame {
    let mut cells = Vec::with_capacity(ws.width as usize * ws.height as usize);
    for i in 0..(ws.width as usize * ws.height as usize) {
        let idx = i * 4;
        if idx + 3 < ws.data.len() {
            cells.push((ws.data[idx] as char, ws.data[idx + 1], ws.data[idx + 2], ws.data[idx + 3]));
        } else {
            cells.push((' ', 0, 0, 0));
        }
    }
    AsciiFrame {
        width: ws.width,
        height: ws.height,
        cells,
    }
}
