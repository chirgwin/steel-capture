use crate::types::{AudioChunk, CaptureFrame, CompactFrame, Copedant};
use crossbeam_channel::Receiver;
use log::{error, info};
use serde_json::json;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Build the JSONL header line — self-describing parameter table.
/// Every field's key, type, range, and unit is documented so any reader
/// can parse frames without knowing the steel-capture codebase.
pub fn build_jsonl_header(copedant: &Copedant) -> serde_json::Value {
    json!({
        "format": "steel-capture",
        "rate_hz": 60,
        "copedant": {
            "name": copedant.name,
            "open_strings_midi": copedant.open_strings,
            "pedals": copedant.pedals.iter().map(|p| {
                json!({"name": p.name, "changes": p.changes.iter().map(|(s, d)| {
                    json!({"string": s, "semitones": d})
                }).collect::<Vec<_>>()})
            }).collect::<Vec<_>>(),
            "levers": copedant.levers.iter().map(|l| {
                json!({"name": l.name, "changes": l.changes.iter().map(|(s, d)| {
                    json!({"string": s, "semitones": d})
                }).collect::<Vec<_>>()})
            }).collect::<Vec<_>>(),
        },
        "channels": [
            {"key": "t",  "name": "timestamp_us",      "type": "u64",    "unit": "microseconds"},
            {"key": "p",  "name": "pedals",             "type": "f32[]",  "count": 3,  "range": [0, 1], "unit": "engagement"},
            {"key": "kl", "name": "knee_levers",        "type": "f32[]",  "count": 5,  "range": [0, 1], "unit": "engagement"},
            {"key": "v",  "name": "volume",             "type": "f32",    "range": [0, 1], "unit": "engagement"},
            {"key": "bs", "name": "bar_sensors",        "type": "f32[]",  "count": 4,  "range": [0, 1], "unit": "hall_normalized"},
            {"key": "bp", "name": "bar_position",       "type": "f32?",   "range": [0, 24], "unit": "frets", "null_meaning": "bar lifted"},
            {"key": "bc", "name": "bar_confidence",     "type": "f32",    "range": [0, 1]},
            {"key": "bx", "name": "bar_source",         "type": "enum",   "values": ["None", "Sensor", "Audio", "Fused"]},
            {"key": "hz", "name": "string_pitches_hz",  "type": "f64[]",  "count": 10, "unit": "Hz"},
            {"key": "sa", "name": "string_active",      "type": "bool[]", "count": 10},
            {"key": "at", "name": "attacks",            "type": "bool[]", "count": 10},
            {"key": "am", "name": "string_amplitude",   "type": "f32[]",  "count": 10, "range": [0, 1]},
        ],
    })
}

pub struct DataLogger {
    rx: Receiver<CaptureFrame>,
    audio_rx: Receiver<AudioChunk>,
    session_dir: PathBuf,
    copedant: Copedant,
}

impl DataLogger {
    pub fn new(
        rx: Receiver<CaptureFrame>,
        audio_rx: Receiver<AudioChunk>,
        output_dir: &Path,
        copedant: Copedant,
    ) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let session_dir = output_dir.join(format!("session_{}", timestamp));
        fs::create_dir_all(&session_dir).expect("create session dir");

        Self {
            rx,
            audio_rx,
            session_dir,
            copedant,
        }
    }

    /// Run the logger. Blocks the calling thread.
    pub fn run(&self) {
        info!("Data logger → {:?}", self.session_dir);

        // Write manifest
        self.write_manifest();

        // Open JSONL output file — header line then compact frames
        let frames_path = self.session_dir.join("frames.jsonl");
        let frames_file = File::create(&frames_path).expect("create frames file");
        let mut frames_writer = BufWriter::new(frames_file);

        let header = build_jsonl_header(&self.copedant);
        let _ = writeln!(frames_writer, "{}", serde_json::to_string(&header).unwrap());

        // Audio accumulator (we'll write a WAV at the end, or incrementally)
        let audio_path = self.session_dir.join("audio_raw.bin");
        let audio_file = File::create(&audio_path).expect("create audio file");
        let mut audio_writer = BufWriter::new(audio_file);
        let mut audio_sample_count: u64 = 0;
        let mut audio_sample_rate: u32 = 48000;

        let mut frame_count: u64 = 0;

        loop {
            // Non-blocking drain of audio chunks
            while let Ok(chunk) = self.audio_rx.try_recv() {
                audio_sample_rate = chunk.sample_rate;
                for &s in &chunk.samples {
                    let bytes = s.to_le_bytes();
                    let _ = audio_writer.write_all(&bytes);
                    audio_sample_count += 1;
                }
            }

            // Blocking receive of capture frames
            match self.rx.recv() {
                Ok(frame) => {
                    let compact = CompactFrame::from(&frame);
                    let line = serde_json::to_string(&compact).unwrap();
                    let _ = writeln!(frames_writer, "{}", line);
                    frame_count += 1;

                    if frame_count.is_multiple_of(1000) {
                        let _ = frames_writer.flush();
                        let _ = audio_writer.flush();
                        info!(
                            "Logged {} frames, {} audio samples",
                            frame_count, audio_sample_count
                        );
                    }
                }
                Err(_) => break,
            }
        }

        let _ = frames_writer.flush();
        let _ = audio_writer.flush();

        // Write final stats to manifest
        let stats_path = self.session_dir.join("stats.json");
        let stats = json!({
            "total_frames": frame_count,
            "total_audio_samples": audio_sample_count,
            "audio_sample_rate": audio_sample_rate,
        });
        fs::write(&stats_path, serde_json::to_string_pretty(&stats).unwrap())
            .unwrap_or_else(|e| error!("Failed to write stats: {}", e));

        info!(
            "Session saved: {} frames, {} audio samples → {:?}",
            frame_count, audio_sample_count, self.session_dir
        );
    }

    fn write_manifest(&self) {
        let manifest = json!({
            "system": "steel-capture",
            "copedant": {
                "name": self.copedant.name,
                "open_strings": self.copedant.open_strings,
                "pedals": self.copedant.pedals.iter().map(|p| {
                    json!({
                        "name": p.name,
                        "changes": p.changes.iter().map(|(s, d)| {
                            json!({"string": s, "semitones": d})
                        }).collect::<Vec<_>>()
                    })
                }).collect::<Vec<_>>(),
                "levers": self.copedant.levers.iter().map(|l| {
                    json!({
                        "name": l.name,
                        "changes": l.changes.iter().map(|(s, d)| {
                            json!({"string": s, "semitones": d})
                        }).collect::<Vec<_>>()
                    })
                }).collect::<Vec<_>>(),
            },
            "sensor_config": {
                "channels": 13,
                "rate_hz": 1000,
                "pedals": ["A", "B", "C"],
                "knee_levers": ["LKL", "LKR", "LKV", "RKL", "RKR"],
            },
            "audio_config": {
                "format": "f32le",
                "channels": 1,
                "sample_rate": 48000,
                "bit_depth": 32,
            },
        });

        let path = self.session_dir.join("manifest.json");
        fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).expect("write manifest");
    }
}
