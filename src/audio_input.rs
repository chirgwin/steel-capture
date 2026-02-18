use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use crossbeam_channel::Sender;
use log::{error, info};
use std::thread;

use crate::types::{AudioChunk, InputEvent, SessionClock};

const CHUNK_SIZE: usize = 1024;

/// Live audio capture via cpal.
///
/// Holds the cpal `Stream` alive. Drop this to stop capture.
/// Samples are mixed to mono f32 and sent as `InputEvent::Audio`
/// chunks of `CHUNK_SIZE` samples to the provided channel.
pub struct AudioCapture {
    _stream: Stream,
}

impl AudioCapture {
    /// Open the default input device and start streaming.
    /// Returns immediately — audio arrives on a background thread.
    pub fn start(tx: Sender<InputEvent>, clock: SessionClock) -> Result<Self, String> {
        let host = cpal::default_host();

        let device = host
            .default_input_device()
            .ok_or_else(|| "No default audio input device found".to_string())?;

        info!(
            "Audio input: {}",
            device.name().unwrap_or_else(|_| "unknown".into())
        );

        let supported = device
            .default_input_config()
            .map_err(|e| format!("No supported input config: {e}"))?;

        // Prefer 48kHz — matches the string detector's analysis window sizing.
        // Check if any supported range covers it (with the same channel count);
        // fall back to the device default if not.
        let preferred = cpal::SampleRate(48000);
        let config_48k = device.supported_input_configs().ok().and_then(|configs| {
            configs
                .filter(|c| {
                    c.channels() == supported.channels()
                        && c.min_sample_rate() <= preferred
                        && c.max_sample_rate() >= preferred
                })
                .max_by_key(|c| c.max_sample_rate()) // prefer highest-quality match
                .map(|c| c.with_sample_rate(preferred))
        });

        let (config, sample_rate, format): (StreamConfig, u32, SampleFormat) =
            if let Some(cfg) = config_48k {
                let sr = cfg.sample_rate().0;
                let fmt = cfg.sample_format();
                (cfg.into(), sr, fmt)
            } else {
                let sr = supported.sample_rate().0;
                let fmt = supported.sample_format();
                (supported.into(), sr, fmt)
            };

        let channels = config.channels as usize;

        info!(
            "Capture config: {}Hz  {} ch  {:?}",
            sample_rate, channels, format
        );

        // Inner channel: realtime callback → processing thread
        // try_send prevents blocking the audio callback on backpressure
        let (raw_tx, raw_rx) = crossbeam_channel::bounded::<Vec<f32>>(64);

        let err_fn = |e: cpal::StreamError| error!("Audio stream error: {e}");

        let stream = match format {
            SampleFormat::F32 => {
                let raw_tx = raw_tx.clone();
                device
                    .build_input_stream(
                        &config,
                        move |data: &[f32], _| {
                            let mono = mix_mono_f32(data, channels);
                            let _ = raw_tx.try_send(mono);
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| e.to_string())?
            }
            SampleFormat::I16 => {
                let raw_tx = raw_tx.clone();
                device
                    .build_input_stream(
                        &config,
                        move |data: &[i16], _| {
                            let mono = mix_mono_i16(data, channels);
                            let _ = raw_tx.try_send(mono);
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| e.to_string())?
            }
            SampleFormat::U16 => {
                let raw_tx = raw_tx.clone();
                device
                    .build_input_stream(
                        &config,
                        move |data: &[u16], _| {
                            let mono = mix_mono_u16(data, channels);
                            let _ = raw_tx.try_send(mono);
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| e.to_string())?
            }
            fmt => {
                return Err(format!(
                    "Unsupported sample format {fmt:?}. Use an F32 or I16 device."
                ))
            }
        };

        stream.play().map_err(|e| e.to_string())?;

        // Processing thread: accumulate callback chunks → CHUNK_SIZE events
        thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                let mut accum: Vec<f32> = Vec::with_capacity(CHUNK_SIZE * 4);
                for chunk in raw_rx {
                    accum.extend_from_slice(&chunk);
                    while accum.len() >= CHUNK_SIZE {
                        let samples: Vec<f32> = accum.drain(..CHUNK_SIZE).collect();
                        let event = InputEvent::Audio(AudioChunk {
                            timestamp_us: clock.now_us(),
                            samples,
                            sample_rate,
                        });
                        if tx.send(event).is_err() {
                            return; // Receiver dropped (calibration finished)
                        }
                    }
                }
            })
            .unwrap();

        Ok(Self { _stream: stream })
    }
}

// ─── Per-format mono mixdown helpers ─────────────────────────────────────────

fn mix_mono_f32(data: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

fn mix_mono_i16(data: &[i16], channels: usize) -> Vec<f32> {
    const SCALE: f32 = i16::MAX as f32;
    if channels == 1 {
        return data.iter().map(|&s| s as f32 / SCALE).collect();
    }
    data.chunks(channels)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|&s| s as i32).sum();
            sum as f32 / (channels as f32 * SCALE)
        })
        .collect()
}

fn mix_mono_u16(data: &[u16], channels: usize) -> Vec<f32> {
    // U16: 0 = -1.0, 32768 = 0.0, 65535 = +1.0
    const MID: f32 = 32768.0;
    const SCALE: f32 = 32768.0;
    if channels == 1 {
        return data.iter().map(|&s| (s as f32 - MID) / SCALE).collect();
    }
    data.chunks(channels)
        .map(|frame| {
            let sum: f32 = frame.iter().map(|&s| (s as f32 - MID) / SCALE).sum();
            sum / channels as f32
        })
        .collect()
}
