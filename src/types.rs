use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Instant;

// ─── Sensor data from Teensy ────────────────────────────────────────────────

/// Raw sensor readings from the Teensy: pedals, knee levers, volume, bar sensors.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SensorFrame {
    /// Microseconds since capture session start
    pub timestamp_us: u64,
    /// Pedal positions: 0.0 (rest/up) to 1.0 (fully engaged/down)
    pub pedals: [f32; 3],
    /// Knee lever positions: 0.0 (rest) to 1.0 (fully engaged)
    ///   [0] LKL  [1] LKR  [2] LKV  [3] RKL  [4] RKR
    pub knee_levers: [f32; 5],
    /// Volume pedal: 0.0 (toe up / silent) to 1.0 (toe down / full volume)
    pub volume: f32,
    /// Bar position hall sensors: raw 0.0–1.0 readings from 4 SS49E sensors
    /// mounted along the treble-side rail at frets 0, 5, 10, 15.
    /// Higher value = magnet closer to sensor.
    /// All near-zero when bar is lifted off the strings.
    pub bar_sensors: [f32; 4],
    /// Which strings are currently sounding.
    /// In hardware mode: derived from pick detection or audio onset.
    /// In simulator mode: set by the gesture sequence.
    pub string_active: [bool; 10],
}

impl SensorFrame {
    pub fn at_rest(timestamp_us: u64) -> Self {
        Self {
            timestamp_us,
            pedals: [0.0; 3],
            knee_levers: [0.0; 5],
            volume: 0.7,
            bar_sensors: [0.0; 4],
            string_active: [false; 10],
        }
    }
}

impl fmt::Display for SensorFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "t={:>10}µs  P[{:.2} {:.2} {:.2}]  KL[{:.2} {:.2} {:.2} {:.2} {:.2}]  V={:.2}  BAR[{:.2} {:.2} {:.2} {:.2}]",
            self.timestamp_us,
            self.pedals[0], self.pedals[1], self.pedals[2],
            self.knee_levers[0], self.knee_levers[1], self.knee_levers[2],
            self.knee_levers[3], self.knee_levers[4],
            self.volume,
            self.bar_sensors[0], self.bar_sensors[1], self.bar_sensors[2], self.bar_sensors[3],
        )
    }
}

// ─── Audio data ─────────────────────────────────────────────────────────────

/// A chunk of audio samples from the audio interface (or simulator).
#[derive(Debug, Clone)]
pub struct AudioChunk {
    /// Microseconds since session start (timestamp of first sample)
    pub timestamp_us: u64,
    /// Mono f32 samples, normalized -1.0 to 1.0
    pub samples: Vec<f32>,
    /// Sample rate in Hz
    pub sample_rate: u32,
}

// ─── Bar state (inferred) ───────────────────────────────────────────────────

/// How the bar position was determined.
/// Serializes as plain strings ("None", "Sensor", "Audio", "Fused")
/// to match the JavaScript visualization state format.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum BarSource {
    /// No bar detected
    None,
    /// Hall sensor array only (works during silence)
    Sensor,
    /// Audio pitch inference only
    Audio,
    /// Sensor + audio fused (highest confidence)
    Fused,
}

/// Bar position inferred from hall sensors and/or audio pitch detection.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BarState {
    /// Position in fret-space: 0.0 = nut, 3.0 = 3rd fret, etc.
    /// None if no pitch detected (silence / bar off strings).
    pub position: Option<f32>,
    /// Confidence: 0.0–1.0. Based on sensor quality and/or pitch clarity.
    pub confidence: f32,
    /// How the position was determined
    pub source: BarSource,
}

impl BarState {
    pub fn unknown() -> Self {
        Self {
            position: None,
            confidence: 0.0,
            source: BarSource::None,
        }
    }
}

// ─── Unified capture frame ──────────────────────────────────────────────────

/// Complete state snapshot at a moment in time.
/// Produced by the coordinator, consumed by logger, OSC sender, and WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureFrame {
    pub timestamp_us: u64,
    /// Raw mechanical state
    pub pedals: [f32; 3],
    pub knee_levers: [f32; 5],
    pub volume: f32,
    /// Raw bar sensor readings (for diagnostics / calibration)
    pub bar_sensors: [f32; 4],
    /// Inferred bar position (fret-space), None if unknown
    pub bar_position: Option<f32>,
    pub bar_confidence: f32,
    pub bar_source: BarSource,
    /// Computed pitch for each of 10 strings (Hz).
    /// Requires bar_position to be known; otherwise these are open-string pitches.
    pub string_pitches_hz: [f64; 10],
    /// Which strings are currently sounding (picked/active).
    /// In simulator mode, derived from the gesture sequence.
    /// In hardware mode, derived from audio analysis or manual pick detection.
    pub string_active: [bool; 10],
    /// Which strings had an attack (onset) this frame.
    /// True only on the frame where a string transitions from inactive→active.
    /// Computed by the coordinator from string_active transitions.
    pub attacks: [bool; 10],
    /// Per-string amplitude, normalized 0.0-1.0 (0.0 = silent, 1.0 = peak energy).
    /// Derived from Goertzel spectral analysis at each string's expected frequency.
    /// Peak adapts over ~3.6 seconds to match current signal level.
    pub string_amplitude: [f32; 10],
}

// ─── Compact serialization ──────────────────────────────────────────────────

/// Short-key representation for efficient WS streaming and JSONL logging.
/// Field mapping: t=timestamp_us, p=pedals, kl=knee_levers, v=volume,
/// bs=bar_sensors, bp=bar_position, bc=bar_confidence, bx=bar_source,
/// hz=string_pitches_hz, sa=string_active, at=attacks, am=string_amplitude
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactFrame {
    pub t: u64,
    pub p: [f32; 3],
    pub kl: [f32; 5],
    pub v: f32,
    pub bs: [f32; 4],
    pub bp: Option<f32>,
    pub bc: f32,
    pub bx: BarSource,
    pub hz: [f64; 10],
    pub sa: [bool; 10],
    pub at: [bool; 10],
    pub am: [f32; 10],
}

impl From<&CaptureFrame> for CompactFrame {
    fn from(f: &CaptureFrame) -> Self {
        Self {
            t: f.timestamp_us,
            p: f.pedals,
            kl: f.knee_levers,
            v: f.volume,
            bs: f.bar_sensors,
            bp: f.bar_position,
            bc: f.bar_confidence,
            bx: f.bar_source,
            hz: f.string_pitches_hz,
            sa: f.string_active,
            at: f.attacks,
            am: f.string_amplitude,
        }
    }
}

impl From<CompactFrame> for CaptureFrame {
    fn from(c: CompactFrame) -> Self {
        Self {
            timestamp_us: c.t,
            pedals: c.p,
            knee_levers: c.kl,
            volume: c.v,
            bar_sensors: c.bs,
            bar_position: c.bp,
            bar_confidence: c.bc,
            bar_source: c.bx,
            string_pitches_hz: c.hz,
            string_active: c.sa,
            attacks: c.at,
            string_amplitude: c.am,
        }
    }
}

impl fmt::Display for CaptureFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bar_str = match self.bar_position {
            Some(p) => format!("{:.2}", p),
            None => "---".to_string(),
        };
        let src = match self.bar_source {
            BarSource::None => "---",
            BarSource::Sensor => "sen",
            BarSource::Audio => "aud",
            BarSource::Fused => "fus",
        };
        write!(
            f,
            "t={:>10}µs  bar={:<6} conf={:.2} src={}  P[{:.2} {:.2} {:.2}]  V={:.2}",
            self.timestamp_us,
            bar_str,
            self.bar_confidence,
            src,
            self.pedals[0],
            self.pedals[1],
            self.pedals[2],
            self.volume,
        )
    }
}

// ─── Copedant types ─────────────────────────────────────────────────────────

/// Copedant definition: open tuning + all pedal/lever pitch changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Copedant {
    pub name: String,
    /// Open string pitches as MIDI note numbers (fractional for sweetened tuning).
    /// Index 0 = string 1 (furthest from player).
    pub open_strings: [f64; 10],
    /// Pedal definitions (index 0=A, 1=B, 2=C)
    pub pedals: Vec<ChangeDef>,
    /// Knee lever definitions (index 0=LKL, 1=LKR, 2=LKV, 3=RKL, 4=RKR)
    pub levers: Vec<ChangeDef>,
}

/// Defines what one pedal or lever does: a list of (string_index, semitone_delta).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeDef {
    pub name: String,
    /// (string_index 0–9, semitone_delta when fully engaged)
    pub changes: Vec<(usize, f64)>,
}

// ─── Inter-thread messages ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum InputEvent {
    Sensor(SensorFrame),
    Audio(AudioChunk),
}

// ─── Session clock ──────────────────────────────────────────────────────────

/// Monotonic clock for the capture session.
#[derive(Clone)]
pub struct SessionClock {
    start: Instant,
}

impl SessionClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    pub fn now_us(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }
}

impl Default for SessionClock {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Constants ──────────────────────────────────────────────────────────────

pub const PEDAL_NAMES: [&str; 3] = ["A", "B", "C"];
pub const LEVER_NAMES: [&str; 5] = ["LKL", "LKR", "LKV", "RKL", "RKR"];
pub const E9_STRING_NAMES: [&str; 10] = [
    "1:F#4", "2:D#4", "3:G#4", "4:E4", "5:B3", "6:G#3", "7:F#3", "8:E3", "9:D3", "10:B2",
];

/// Fret positions where bar hall sensors are mounted.
/// SS49E sensors on treble-side rail, magnet on bar tip.
pub const BAR_SENSOR_FRETS: [f32; 4] = [0.0, 5.0, 10.0, 15.0];
