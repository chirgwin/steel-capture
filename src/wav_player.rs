use crate::types::*;
use crossbeam_channel::Sender;
use hound::{SampleFormat, WavReader};
use log::{error, info, warn};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

/// Reads a WAV file and streams it as AudioChunk events at real-time pace.
///
/// Intended for pre-hardware testing: record yourself playing and pipe the
/// audio through the full coordinator pipeline to validate Goertzel thresholds
/// before assembling any hardware.
///
/// Typical use: `--simulate --audio-file my_playing.wav`
/// The simulator provides pedal/lever/bar ground truth; the WAV provides audio.
/// The coordinator runs audio detection on the real signal.
pub struct WavPlayer {
    path: PathBuf,
    tx: Sender<InputEvent>,
    clock: SessionClock,
}

/// Samples sent per AudioChunk. ~21ms at 48kHz — gives the string detector
/// enough granularity without saturating the channel.
const CHUNK_SIZE: usize = 1024;

impl WavPlayer {
    pub fn new(path: PathBuf, tx: Sender<InputEvent>, clock: SessionClock) -> Self {
        Self { path, tx, clock }
    }

    pub fn run(&self) {
        let reader = match WavReader::open(&self.path) {
            Ok(r) => r,
            Err(e) => {
                error!("Failed to open WAV file {:?}: {}", self.path, e);
                return;
            }
        };

        let spec = reader.spec();
        let sample_rate = spec.sample_rate;
        let channels = spec.channels as usize;

        info!(
            "WAV: {:?}  {} Hz  {} ch  {:?}  {} bit",
            self.path.file_name().unwrap_or_default(),
            sample_rate,
            channels,
            spec.sample_format,
            spec.bits_per_sample,
        );

        if sample_rate != 48000 {
            warn!(
                "WAV sample rate is {} Hz; string detector expects 48000 Hz. \
                 Goertzel thresholds may be off — resample before use.",
                sample_rate
            );
        }

        // Read all samples as f32
        let samples_f32: Vec<f32> = match spec.sample_format {
            SampleFormat::Float => reader
                .into_samples::<f32>()
                .filter_map(|s| s.ok())
                .collect(),
            SampleFormat::Int => {
                let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .into_samples::<i32>()
                    .filter_map(|s| s.ok())
                    .map(|s| s as f32 / max)
                    .collect()
            }
        };

        // Mix down to mono
        let mono: Vec<f32> = if channels == 1 {
            samples_f32
        } else {
            samples_f32
                .chunks(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                .collect()
        };

        let total_samples = mono.len();
        let duration_secs = total_samples as f64 / sample_rate as f64;
        info!(
            "WAV: {:.2}s, {} samples → streaming at real-time pace",
            duration_secs, total_samples
        );

        let chunk_dur = Duration::from_secs_f64(CHUNK_SIZE as f64 / sample_rate as f64);
        let start = Instant::now();

        for (i, chunk) in mono.chunks(CHUNK_SIZE).enumerate() {
            // Pace to real time: wait until this chunk's expected send time
            let target = chunk_dur * i as u32;
            let elapsed = start.elapsed();
            if elapsed < target {
                thread::sleep(target - elapsed);
            }

            let event = InputEvent::Audio(AudioChunk {
                timestamp_us: self.clock.now_us(),
                samples: chunk.to_vec(),
                sample_rate,
            });

            if self.tx.send(event).is_err() {
                // Coordinator shut down — stop streaming
                break;
            }
        }

        info!("WAV playback complete.");
    }
}
