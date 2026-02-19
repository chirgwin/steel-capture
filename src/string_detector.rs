use crate::copedant::{midi_to_hz, CopedantEngine};
use crate::dsp::{compute_rms, goertzel_magnitude};
use crate::types::*;
use log::trace;

/// Per-string onset/release detector using constrained spectral analysis.
///
/// # How it works
///
/// Because we know the copedant state and bar position at every moment,
/// we know the **exact expected frequency** of each of the 10 strings.
/// This turns blind polyphonic pitch detection (an MIR nightmare) into
/// a much simpler "matched filter" problem: for each string, compute the
/// Goertzel magnitude at its expected frequency and compare to a threshold.
///
/// # Attack/release detection
///
/// Per-string energy is tracked over time with smoothing. State transitions:
///   - Energy rises above `onset_threshold`  → string active (attack)
///   - Energy falls below `release_threshold` → string inactive (release)
///
/// The hysteresis between onset and release thresholds prevents chattering
/// on signals near the threshold.
///
/// # Limitations
///
/// - Strings whose frequencies are close (e.g., octave-related harmonics)
///   may cross-detect. The copedant makes this less likely since we search
///   at exact expected frequencies, not broad bands.
/// - Only works when bar position is known (need sensor or prior audio estimate).
/// - Fast picking rolls (<5ms between attacks) may not resolve at the
///   ~42ms analysis rate.
pub struct StringDetector {
    /// Per-string smoothed Goertzel energy (raw, unnormalized)
    energy: [f64; 10],
    /// Per-string peak energy seen, for normalizing amplitude to 0.0-1.0.
    /// Slowly decays toward current max to adapt to different signal levels.
    peak_energy: [f64; 10],
    /// Per-string active state
    pub active: [bool; 10],
    /// Per-string onset thresholds — energy above this → string active
    onset_threshold: [f64; 10],
    /// Per-string release thresholds — energy below this → string inactive (hysteresis)
    release_threshold: [f64; 10],
    /// Smoothing factor for energy tracking (0.0=instant, 0.99=very smooth)
    smoothing: f64,
    /// Audio ring buffer
    audio_buf: Vec<f32>,
    /// Target analysis window (samples)
    pub analysis_window: usize,
    /// Samples accumulated since last analysis
    pub samples_since_analysis: usize,
    /// Analysis interval (samples) — how often to run detection
    pub analysis_interval: usize,
    /// Cached sample rate
    sample_rate: u32,
}

impl StringDetector {
    pub fn new() -> Self {
        Self {
            energy: [0.0; 10],
            peak_energy: [0.01; 10],
            active: [false; 10],
            onset_threshold: [0.02; 10],
            release_threshold: [0.008; 10],
            smoothing: 0.6,
            audio_buf: Vec::with_capacity(8192),
            analysis_window: 4096, // ~85ms at 48kHz — resolves B2 (123Hz, ~8ms period)
            samples_since_analysis: 0,
            analysis_interval: 2048, // run every ~42ms
            sample_rate: 48000,
        }
    }

    /// Override per-string detection thresholds (e.g., loaded from calibration.json).
    pub fn with_thresholds(mut self, onset: [f64; 10], release: [f64; 10]) -> Self {
        self.onset_threshold = onset;
        self.release_threshold = release;
        self
    }

    /// Push new audio samples into the internal buffer.
    pub fn push_audio(&mut self, chunk: &AudioChunk) {
        self.sample_rate = chunk.sample_rate;
        self.audio_buf.extend_from_slice(&chunk.samples);
        self.samples_since_analysis += chunk.samples.len();

        // Keep buffer bounded
        let max_len = self.analysis_window * 2;
        if self.audio_buf.len() > max_len {
            let excess = self.audio_buf.len() - max_len;
            self.audio_buf.drain(..excess);
        }
    }

    /// Returns true if enough audio has accumulated for analysis.
    pub fn ready(&self) -> bool {
        self.audio_buf.len() >= self.analysis_window
            && self.samples_since_analysis >= self.analysis_interval
    }

    /// Analyze buffered audio and update per-string active states.
    ///
    /// Returns (string_active, attacks) where attacks[i] is true only on
    /// the frame where string i transitions from inactive → active.
    ///
    /// Requires `bar_position` to compute expected frequencies. If bar
    /// position is unknown, all strings are marked inactive.
    pub fn detect(
        &mut self,
        sensor: &SensorFrame,
        bar_position: Option<f32>,
        engine: &CopedantEngine,
    ) -> ([bool; 10], [bool; 10], [f32; 10]) {
        if !self.ready() {
            // Not enough audio yet — return current state, no new attacks
            return (self.active, [false; 10], self.amplitude());
        }
        self.samples_since_analysis = 0;

        let bar_fret = match bar_position {
            Some(f) => f,
            None => {
                // No bar position → can't determine frequencies → all inactive
                self.active = [false; 10];
                self.energy = [0.0; 10];
                return (self.active, [false; 10], [0.0; 10]);
            }
        };

        // Use the most recent analysis_window samples
        let start = self.audio_buf.len().saturating_sub(self.analysis_window);
        let samples = &self.audio_buf[start..];
        let n = samples.len();
        let sr = self.sample_rate as f64;

        // Global silence threshold. RMS below this is indistinguishable from
        // quantization/electronic noise in a typical audio interface.
        let rms = compute_rms(samples);
        if rms < 0.003 {
            // Silence — all strings inactive
            for i in 0..10 {
                self.energy[i] *= 0.5; // Decay energy toward zero
            }
            self.active = [false; 10];
            return (self.active, [false; 10], self.amplitude());
        }

        // Compute expected frequency for each string
        let open = engine.effective_open_pitches(sensor);

        let mut attacks = [false; 10];

        for si in 0..10 {
            let freq = midi_to_hz(open[si] + bar_fret as f64);

            // Skip frequencies outside audible/Nyquist range
            if freq < 20.0 || freq > sr / 2.0 {
                self.energy[si] = 0.0;
                self.active[si] = false;
                continue;
            }

            // Goertzel magnitude at the expected fundamental
            let mag = goertzel_magnitude(samples, freq, sr, n);

            // Also check 2nd harmonic (helps distinguish from noise)
            let mag2 = if freq * 2.0 < sr / 2.0 {
                goertzel_magnitude(samples, freq * 2.0, sr, n)
            } else {
                0.0
            };

            // Combined energy: fundamental + weighted 2nd harmonic.
            // The 0.3 weight is empirical: harmonic confirms string identity
            // without dominating (real strings have strong 2nd harmonics, noise does not).
            let raw_energy = mag + 0.3 * mag2;

            // Normalize by number of samples for consistent thresholds
            let normalized = raw_energy / n as f64;

            // Smooth the energy
            self.energy[si] =
                self.energy[si] * self.smoothing + normalized * (1.0 - self.smoothing);

            // Track peak energy for normalizing amplitude to 0.0-1.0.
            // Adapts over ~3.6s half-life at ~24Hz analysis rate.
            if self.energy[si] > self.peak_energy[si] {
                self.peak_energy[si] = self.energy[si];
            } else {
                self.peak_energy[si] = (self.peak_energy[si] * 0.992).max(0.01);
            }

            // Threshold with hysteresis (per-string calibrated values)
            if self.active[si] {
                // Currently active — need to drop below release threshold
                if self.energy[si] < self.release_threshold[si] {
                    self.active[si] = false;
                }
            } else {
                // Currently inactive — need to rise above onset threshold
                if self.energy[si] > self.onset_threshold[si] {
                    self.active[si] = true;
                    attacks[si] = true;
                }
            }
        }

        trace!(
            "string_det: active=[{}] energy=[{}]",
            self.active
                .iter()
                .map(|&a| if a { "█" } else { "·" })
                .collect::<Vec<_>>()
                .join(""),
            self.energy
                .iter()
                .map(|e| format!("{:.3}", e))
                .collect::<Vec<_>>()
                .join(" "),
        );

        (self.active, attacks, self.amplitude())
    }

    /// Per-string amplitude normalized to 0.0-1.0 (energy / peak_energy).
    fn amplitude(&self) -> [f32; 10] {
        let mut out = [0.0f32; 10];
        for (i, val) in out.iter_mut().enumerate() {
            *val = (self.energy[i] / self.peak_energy[i]).clamp(0.0, 1.0) as f32;
        }
        out
    }

    /// Reset all state (e.g., on session restart).
    pub fn reset(&mut self) {
        self.energy = [0.0; 10];
        self.peak_energy = [0.01; 10];
        self.active = [false; 10];
        self.audio_buf.clear();
        self.samples_since_analysis = 0;
    }
}

impl Default for StringDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copedant::buddy_emmons_e9;
    use crate::dsp::test_helpers::{multi_sine, sine_wave};

    fn make_engine() -> CopedantEngine {
        CopedantEngine::new(buddy_emmons_e9())
    }

    /// Feed audio and run detection, returning (active, attacks).
    fn feed_and_detect(
        det: &mut StringDetector,
        samples: &[f32],
        sr: u32,
        sensor: &SensorFrame,
        bar_pos: Option<f32>,
        engine: &CopedantEngine,
    ) -> ([bool; 10], [bool; 10], [f32; 10]) {
        let chunk = AudioChunk {
            timestamp_us: 0,
            samples: samples.to_vec(),
            sample_rate: sr,
        };
        det.push_audio(&chunk);
        // Force analysis readiness
        det.analysis_window = samples.len().min(det.analysis_window);
        det.samples_since_analysis = det.analysis_interval;
        det.detect(sensor, bar_pos, engine)
    }

    #[test]
    fn test_detects_single_string() {
        let engine = make_engine();
        let mut det = StringDetector::new();
        let sensor = SensorFrame::at_rest(0);

        // String 3 (idx 2) = G#4 = MIDI 68. At fret 3 → B4 = MIDI 71
        let freq = midi_to_hz(68.0 + 3.0);
        let samples = sine_wave(freq, 0.7, 48000, 100);

        let (active, attacks, _) =
            feed_and_detect(&mut det, &samples, 48000, &sensor, Some(3.0), &engine);
        assert!(
            active[2],
            "string 3 (G#4 at fret 3) should be detected as active"
        );
        assert!(attacks[2], "should register as attack on first detection");
        // Other strings should NOT be active (their frequencies differ)
        let other_active: usize = active
            .iter()
            .enumerate()
            .filter(|&(i, &a)| i != 2 && a)
            .count();
        assert!(
            other_active <= 1,
            "at most 1 other string should be active (harmonic coincidence), got {}",
            other_active
        );
    }

    #[test]
    fn test_detects_three_string_grip() {
        let engine = make_engine();
        let mut det = StringDetector::new();
        let sensor = SensorFrame::at_rest(0);

        // Strings 3,4,5 (idx 2,3,4) at fret 3: G#4+3, E4+3, B3+3
        let open = engine.effective_open_pitches(&sensor);
        let freqs: Vec<f64> = [2, 3, 4]
            .iter()
            .map(|&si| midi_to_hz(open[si] + 3.0))
            .collect();
        let samples = multi_sine(&freqs, 0.17, 48000, 100);

        let (active, _attacks, _) =
            feed_and_detect(&mut det, &samples, 48000, &sensor, Some(3.0), &engine);
        assert!(active[2], "string 3 should be active");
        assert!(active[3], "string 4 should be active");
        assert!(active[4], "string 5 should be active");
    }

    #[test]
    fn test_detects_with_pedal_a() {
        let engine = make_engine();
        let mut det = StringDetector::new();
        let mut sensor = SensorFrame::at_rest(0);
        sensor.pedals[0] = 1.0; // Pedal A: str5 B→C#, str10 B→C#

        // String 5 (idx 4) with pedal A at fret 5:
        // Open C#4 (MIDI 61) + 5 frets = F#4 (MIDI 66)
        let open = engine.effective_open_pitches(&sensor);
        let freq = midi_to_hz(open[4] + 5.0);
        let samples = sine_wave(freq, 0.7, 48000, 100);

        let (active, _, _) =
            feed_and_detect(&mut det, &samples, 48000, &sensor, Some(5.0), &engine);
        assert!(active[4], "string 5 with pedal A should be detected");
    }

    #[test]
    fn test_silence_all_inactive() {
        let engine = make_engine();
        let mut det = StringDetector::new();
        let sensor = SensorFrame::at_rest(0);

        let samples = vec![0.0f32; 4800]; // 100ms of silence
        let (active, attacks, _) =
            feed_and_detect(&mut det, &samples, 48000, &sensor, Some(3.0), &engine);
        assert!(
            active.iter().all(|&a| !a),
            "all strings should be inactive during silence"
        );
        assert!(attacks.iter().all(|&a| !a), "no attacks during silence");
    }

    #[test]
    fn test_no_bar_all_inactive() {
        let engine = make_engine();
        let mut det = StringDetector::new();
        let sensor = SensorFrame::at_rest(0);

        let samples = sine_wave(440.0, 0.7, 48000, 100);
        let (active, _, _) = feed_and_detect(&mut det, &samples, 48000, &sensor, None, &engine);
        assert!(
            active.iter().all(|&a| !a),
            "all inactive when bar position unknown"
        );
    }

    #[test]
    fn test_attack_only_on_onset() {
        let engine = make_engine();
        let mut det = StringDetector::new();
        let sensor = SensorFrame::at_rest(0);

        let open = engine.effective_open_pitches(&sensor);
        let freq = midi_to_hz(open[3] + 3.0); // string 4 at fret 3
        let samples = sine_wave(freq, 0.7, 48000, 100);

        // First detection: should have attack
        let (_, attacks1, _) =
            feed_and_detect(&mut det, &samples, 48000, &sensor, Some(3.0), &engine);
        assert!(attacks1[3], "first detection should be an attack");

        // Second detection with same signal: NO new attack
        let (active2, attacks2, _) =
            feed_and_detect(&mut det, &samples, 48000, &sensor, Some(3.0), &engine);
        assert!(active2[3], "still active");
        assert!(!attacks2[3], "no new attack — string was already active");
    }

    #[test]
    fn test_release_then_reattack() {
        let engine = make_engine();
        let mut det = StringDetector::new();
        let sensor = SensorFrame::at_rest(0);

        let open = engine.effective_open_pitches(&sensor);
        let freq = midi_to_hz(open[3] + 3.0);

        // Attack
        let samples = sine_wave(freq, 0.7, 48000, 100);
        let (_, attacks1, _) =
            feed_and_detect(&mut det, &samples, 48000, &sensor, Some(3.0), &engine);
        assert!(attacks1[3]);

        // Release (silence)
        let silence = vec![0.0f32; 4800];
        // Need multiple silence frames to decay energy below release threshold
        for _ in 0..3 {
            feed_and_detect(&mut det, &silence, 48000, &sensor, Some(3.0), &engine);
        }
        assert!(!det.active[3], "should be released after silence");

        // Re-attack
        let (_, attacks3, _) =
            feed_and_detect(&mut det, &samples, 48000, &sensor, Some(3.0), &engine);
        assert!(attacks3[3], "should register as new attack after release");
    }

    #[test]
    fn test_amplitude_normalized_range() {
        let engine = make_engine();
        let mut det = StringDetector::new();
        let sensor = SensorFrame::at_rest(0);
        let open = engine.effective_open_pitches(&sensor);
        let freq = midi_to_hz(open[3] + 3.0);
        let samples = sine_wave(freq, 0.7, 48000, 100);

        let (_, _, amplitude) =
            feed_and_detect(&mut det, &samples, 48000, &sensor, Some(3.0), &engine);

        for &a in &amplitude {
            assert!(
                (0.0..=1.0).contains(&a),
                "amplitude {} out of [0,1] range",
                a
            );
        }
        assert!(
            amplitude[3] > 0.0,
            "active string should have positive amplitude"
        );
    }
}
