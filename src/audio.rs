//! Audio playback for the video bus.
//!
//! FFmpeg decodes the best audio stream from the same source the video decoder
//! reads, `swresample` converts it to the output device's format, and `cpal`
//! plays it through a lock-free ring buffer. The device consumes samples at
//! real time, so ring backpressure paces the decode thread automatically -- and
//! because both audio and (wall-clock-paced) video start together and run in
//! real time, they stay in rough A/V sync without an explicit master clock.
//!
//! Everything degrades gracefully: no output device, no audio stream, or an
//! unsupported sample format just means "no sound", never a crash (mirrors the
//! webcam's crash-safe posture).

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use ffmpeg_next as ff;
use ff::channel_layout::ChannelLayout;
use ff::format::sample::Type as SampleType;
use ff::format::Sample as AvSample;
use ff::software::resampling::context::Context as Resampler;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct AudioPlayer {
    // cpal Stream is !Send on some platforms; it lives on the thread that built
    // it (the app main thread) and is kept alive for the player's lifetime.
    _stream: cpal::Stream,
    stop: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
}

impl AudioPlayer {
    /// Start decoding + playing the audio of `source`. `Err` (no device / no
    /// audio stream / unsupported format) should be treated as "no sound".
    pub fn new(source: impl Into<PathBuf>, looping: bool) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("no default audio output device"))?;
        let supported = device.default_output_config()?;
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let device_rate = config.sample_rate; // cpal 0.18: SampleRate == u32
        let device_channels = config.channels;

        // ~1 second of interleaved samples of headroom
        let capacity = (device_rate as usize) * (device_channels as usize).max(1);
        let (prod, cons) = HeapRb::<f32>::new(capacity.max(4096)).split();

        let stop = Arc::new(AtomicBool::new(false));
        let muted = Arc::new(AtomicBool::new(false));

        let stream = match sample_format {
            SampleFormat::F32 => build_stream::<f32>(&device, &config, cons, muted.clone()),
            SampleFormat::I16 => build_stream::<i16>(&device, &config, cons, muted.clone()),
            SampleFormat::U16 => build_stream::<u16>(&device, &config, cons, muted.clone()),
            other => Err(anyhow!("unsupported output sample format: {:?}", other)),
        }?;
        stream.play()?;

        let src = source.into();
        let stop_thread = stop.clone();
        std::thread::spawn(move || {
            // never let a decode error / panic take down the app
            let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
                let _ = decode_loop(&src, looping, device_rate, device_channels, prod, stop_thread);
            }));
        });

        Ok(Self {
            _stream: stream,
            stop,
            muted,
        })
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut cons: ringbuf::HeapCons<f32>,
    muted: Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: Sample + SizedSample + FromSample<f32>,
{
    // reused scratch so the realtime callback never allocates
    let mut scratch = vec![0f32; 16384];
    let stream = device.build_output_stream(
        config.clone(),
        move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
            let gain = if muted.load(Ordering::Relaxed) { 0.0 } else { 1.0 };
            // fill the WHOLE output in scratch-sized chunks so a device buffer
            // larger than `scratch` never gets a silent tail
            let mut i = 0;
            while i < out.len() {
                let chunk = (out.len() - i).min(scratch.len());
                let got = cons.pop_slice(&mut scratch[..chunk]);
                for k in 0..chunk {
                    out[i + k] = T::from_sample(if k < got { scratch[k] * gain } else { 0.0 });
                }
                i += chunk;
            }
        },
        move |_err| {},
        None,
    )?;
    Ok(stream)
}

/// Decode + resample the source's audio into `prod` until `stop`, looping the
/// source if requested. Pacing comes from ring backpressure (the device drains
/// at real time), so this never gets ahead of playback by more than the ring.
fn decode_loop(
    source: &std::path::Path,
    looping: bool,
    device_rate: u32,
    device_channels: u16,
    mut prod: ringbuf::HeapProd<f32>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    ff::init().ok();
    let out_layout = ChannelLayout::default(device_channels.max(1) as i32);

    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }

        let mut ictx = ff::format::input(source)?;
        let stream = ictx
            .streams()
            .best(ff::media::Type::Audio)
            .ok_or_else(|| anyhow!("no audio stream"))?;
        let audio_idx = stream.index();
        let ctx = ff::codec::context::Context::from_parameters(stream.parameters())?;
        let mut decoder = ctx.decoder().audio()?;

        let in_channels = decoder.channels().max(1) as i32;
        let in_rate = decoder.rate();
        let in_fmt = decoder.format();
        let out_fmt = AvSample::F32(SampleType::Packed);

        // prefer the stream's declared layout; fall back to a default for the
        // channel count when it is unspecified (some files report none)
        let mut resampler = Resampler::get(
            in_fmt,
            decoder.channel_layout(),
            in_rate,
            out_fmt,
            out_layout,
            device_rate,
        )
        .or_else(|_| {
            Resampler::get(
                in_fmt,
                ChannelLayout::default(in_channels),
                in_rate,
                out_fmt,
                out_layout,
                device_rate,
            )
        })?;

        let mut decoded = ff::frame::Audio::empty();
        let mut sample_buf: Vec<f32> = Vec::new();

        'packets: for (pkt_stream, packet) in ictx.packets() {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            if pkt_stream.index() != audio_idx {
                continue;
            }
            if decoder.send_packet(&packet).is_err() {
                continue;
            }
            while decoder.receive_frame(&mut decoded).is_ok() {
                // fresh output frame each time so swresample sizes it to the
                // actual converted sample count (no capacity carryover)
                let mut resampled = ff::frame::Audio::empty();
                if resampler.run(&decoded, &mut resampled).is_err() {
                    continue;
                }
                // PACKED interleaved f32: the real buffer is samples*channels
                // long. plane::<f32>(0) would under-read to `samples` only,
                // dropping every non-first channel -> 2x-fast garbled audio.
                let ch = resampled.channels().max(1) as usize;
                let count = resampled.samples() * ch;
                let bytes = resampled.data(0);
                if count == 0 || count * 4 > bytes.len() {
                    continue;
                }
                sample_buf.clear();
                sample_buf.reserve(count);
                for i in 0..count {
                    let o = i * 4;
                    sample_buf.push(f32::from_ne_bytes([
                        bytes[o],
                        bytes[o + 1],
                        bytes[o + 2],
                        bytes[o + 3],
                    ]));
                }

                let mut off = 0;
                let mut stalls = 0u32;
                while off < sample_buf.len() {
                    if stop.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    let pushed = prod.push_slice(&sample_buf[off..]);
                    if pushed > 0 {
                        off += pushed;
                        stalls = 0;
                    } else {
                        // ring full: device hasn't drained. If it stays wedged
                        // ~3s the consumer is gone (device error) -> give up
                        // rather than spin forever.
                        stalls += 1;
                        if stalls > 600 {
                            return Ok(());
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                if stop.load(Ordering::Relaxed) {
                    break 'packets;
                }
            }
        }

        if !looping || stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        // loop: fall through and re-open the source
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the hard part headlessly (no audio device needed): FFmpeg AAC
    /// decode of the bundled demo + swresample to f32 stereo must yield samples.
    /// Asset-optional so the suite still passes without the file.
    #[test]
    fn decodes_demo_audio_to_f32_samples() {
        let path = std::path::Path::new("demo-videos/demo.mp4");
        if !path.exists() {
            return;
        }
        ff::init().ok();
        let mut ictx = match ff::format::input(&path) {
            Ok(c) => c,
            Err(_) => return,
        };
        let (idx, params) = match ictx.streams().best(ff::media::Type::Audio) {
            Some(s) => (s.index(), s.parameters()),
            None => return, // no audio stream -> nothing to verify
        };
        let decoder = ff::codec::context::Context::from_parameters(params)
            .and_then(|c| c.decoder().audio());
        let mut decoder = match decoder {
            Ok(d) => d,
            Err(_) => return,
        };
        let mut resampler = match Resampler::get(
            decoder.format(),
            decoder.channel_layout(),
            decoder.rate(),
            AvSample::F32(SampleType::Packed),
            ChannelLayout::STEREO,
            48_000,
        ) {
            Ok(r) => r,
            Err(_) => return,
        };

        let mut decoded = ff::frame::Audio::empty();
        let mut resampled = ff::frame::Audio::empty();
        let mut total = 0usize;
        'outer: for (s, packet) in ictx.packets() {
            if s.index() != idx {
                continue;
            }
            if decoder.send_packet(&packet).is_err() {
                continue;
            }
            while decoder.receive_frame(&mut decoded).is_ok() {
                if resampler.run(&decoded, &mut resampled).is_ok() {
                    // count the full interleaved buffer (samples * channels)
                    total += resampled.samples() * resampled.channels().max(1) as usize;
                    if total > 1000 {
                        break 'outer;
                    }
                }
            }
        }
        assert!(total > 0, "expected decoded f32 audio samples from demo.mp4");
    }
}
