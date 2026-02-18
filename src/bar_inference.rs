use crate::bar_sensor::BarSensor;
use crate::copedant::{midi_to_hz, CopedantEngine};
use crate::types::*;
use log::trace;

/// Infers bar position by fusing two sources:
///
/// 1. **Hall sensor array** (primary): 4 SS49E sensors along the rail give
///    direct magnetic position. Works during silence. ~±0.3 fret accuracy.
///
/// 2. **Audio spectral matching** (refinement): Goertzel bins at predicted
///    string frequencies for candidate fret positions. Confirms/refines the
///    sensor estimate when strings are sounding. ~±0.1 fret when clean signal.
///
/// Fusion strategy:
///   - Sensor only (silence): use sensor position, moderate confidence
///   - Audio only (sensor fail): use audio, lower confidence
///   - Both available: weighted average biased toward audio (finer resolution),
///     with sensor providing the neighborhood to search
///
/// Audio is buffered internally — callers can feed small chunks (e.g., 48
/// samples per 1ms tick) and inference runs when enough data accumulates.
pub struct BarInference {
    silence_threshold: f32,
    smoothing: f32,
    pub last_position: Option<f32>,
    silence_count: u32,
    /// Fret candidates to test (0.0 to 24.0 in 0.1 steps)
    fret_candidates: Vec<f32>,
    /// Audio ring buffer — accumulates samples across ticks
    audio_buf: Vec<f32>,
    /// Target buffer size for analysis (~85ms at 48kHz)
    analysis_window: usize,
    /// Samples since last analysis (controls analysis rate)
    samples_since_analysis: usize,
    /// How often to run analysis (in samples). Controls CPU usage.
    analysis_interval: usize,
    /// Cached sample rate
    sample_rate: u32,
    /// Hall sensor bar position estimator
    bar_sensor: BarSensor,
}

impl BarInference {
    pub fn new() -> Self {
        let fret_candidates: Vec<f32> = (0..=240).map(|i| i as f32 / 10.0).collect();
        Self {
            silence_threshold: 0.005,
            smoothing: 0.7,
            last_position: None,
            silence_count: 0,
            fret_candidates,
            audio_buf: Vec::with_capacity(8192),
            analysis_window: 4096, // ~85ms at 48kHz — resolves B2 (123Hz)
            samples_since_analysis: 0,
            analysis_interval: 2048, // run analysis every ~42ms
            sample_rate: 48000,
            bar_sensor: BarSensor::new(),
        }
    }

    /// Push new audio samples into the buffer.
    pub fn push_audio(&mut self, chunk: &AudioChunk) {
        self.sample_rate = chunk.sample_rate;
        self.audio_buf.extend_from_slice(&chunk.samples);
        self.samples_since_analysis += chunk.samples.len();

        // Keep buffer bounded (2x analysis window)
        let max_len = self.analysis_window * 2;
        if self.audio_buf.len() > max_len {
            let excess = self.audio_buf.len() - max_len;
            self.audio_buf.drain(..excess);
        }
    }

    /// Check if we have enough audio to run analysis.
    pub fn ready(&self) -> bool {
        self.audio_buf.len() >= self.analysis_window
            && self.samples_since_analysis >= self.analysis_interval
    }

    /// Run bar position inference, fusing hall sensor and audio sources.
    pub fn infer(&mut self, sensor: &SensorFrame, engine: &CopedantEngine) -> BarState {
        // ── 1. Hall sensor estimate (always available) ────────────────
        let sensor_est = self.bar_sensor.estimate(&sensor.bar_sensors);

        // ── 2. Audio estimate (only when enough audio buffered) ───────
        let audio_est = self.infer_audio(sensor, engine);

        // ── 3. Fuse ──────────────────────────────────────────────────
        match (sensor_est, audio_est) {
            // Both sources available: fused estimate
            (Some((s_pos, s_conf)), Some((a_pos, a_conf))) => {
                // Weighted average. Audio has finer resolution when signal
                // is good; sensor provides the coarse neighborhood.
                // If they disagree by > 2 frets, trust sensor (audio might
                // be matching a harmonic alias).
                let disagreement = (s_pos - a_pos).abs();
                let (pos, conf) = if disagreement < 2.0 {
                    // Close agreement: blend with audio weighted higher
                    let audio_weight = 0.6;
                    let blended = s_pos * (1.0 - audio_weight) + a_pos * audio_weight;
                    let conf = (s_conf * 0.5 + a_conf * 0.5).min(1.0);
                    (blended, conf)
                } else {
                    // Large disagreement: trust sensor, audio may be aliased
                    trace!(
                        "bar fusion: disagreement {:.1} frets, trusting sensor",
                        disagreement
                    );
                    (s_pos, s_conf * 0.8)
                };

                let smoothed = self.smooth(pos);
                BarState {
                    position: Some(smoothed),
                    confidence: conf,
                    source: BarSource::Fused,
                }
            }

            // Sensor only (silence, or audio not ready)
            (Some((s_pos, s_conf)), None) => {
                let smoothed = self.smooth(s_pos);
                BarState {
                    position: Some(smoothed),
                    confidence: s_conf * 0.8, // slightly less confident without audio confirm
                    source: BarSource::Sensor,
                }
            }

            // Audio only (sensor failure or not installed)
            (None, Some((a_pos, a_conf))) => {
                let smoothed = self.smooth(a_pos);
                BarState {
                    position: Some(smoothed),
                    confidence: a_conf * 0.7,
                    source: BarSource::Audio,
                }
            }

            // Nothing
            (None, None) => {
                self.last_position = None;
                BarState::unknown()
            }
        }
    }

    /// Apply position smoothing
    fn smooth(&mut self, pos: f32) -> f32 {
        let smoothed = match self.last_position {
            Some(prev) => {
                let alpha = 1.0 - self.smoothing;
                prev + alpha * (pos - prev)
            }
            None => pos,
        };
        self.last_position = Some(smoothed);
        smoothed
    }

    /// Audio-only bar position estimate via spectral template matching.
    fn infer_audio(&mut self, sensor: &SensorFrame, engine: &CopedantEngine) -> Option<(f32, f32)> {
        if !self.ready() {
            return None;
        }
        self.samples_since_analysis = 0;

        // Use the most recent analysis_window samples
        let start = self.audio_buf.len().saturating_sub(self.analysis_window);
        let samples = &self.audio_buf[start..];

        let rms = compute_rms(samples);
        if rms < self.silence_threshold {
            self.silence_count += 1;
            return None;
        }
        self.silence_count = 0;

        let open = engine.effective_open_pitches(sensor);
        let sr = self.sample_rate as f64;

        // Score each candidate fret position
        let mut best_fret: f32 = 0.0;
        let mut best_score: f64 = 0.0;
        let mut total_score: f64 = 0.0;

        for &fret in &self.fret_candidates {
            let score = score_fret(fret, &open, samples, sr);
            if score > best_score {
                best_score = score;
                best_fret = fret;
            }
            total_score += score;
        }

        if best_score < 1e-10 || total_score < 1e-10 {
            return None;
        }

        // Confidence: how much does the best stand out from average?
        let avg_score = total_score / self.fret_candidates.len() as f64;
        let confidence = ((best_score / avg_score - 1.0) / 10.0).clamp(0.1, 1.0) as f32;

        // Parabolic refinement around best candidate
        let refined = refine_fret(best_fret, &open, samples, sr);

        trace!(
            "audio: fret={:.2} (from {:.1}) conf={:.2} score={:.2e}",
            refined,
            best_fret,
            confidence,
            best_score
        );

        Some((refined, confidence))
    }
}

impl Default for BarInference {
    fn default() -> Self {
        Self::new()
    }
}

/// Score how well audio matches expected spectrum at a given fret.
/// Includes a weak prior favoring typical playing range (frets 0-15)
/// to break ties between harmonically equivalent positions (e.g.,
/// fret 5 vs fret 17 can match the same audio in E9 tuning).
fn score_fret(fret: f32, open_midi: &[f64; 10], samples: &[f32], sr: f64) -> f64 {
    let mut score = 0.0f64;
    let n = samples.len();
    for midi in open_midi.iter().take(10) {
        let freq = midi_to_hz(*midi + fret as f64);
        if freq > sr / 2.0 || freq < 20.0 {
            continue;
        }
        score += goertzel_magnitude(samples, freq, sr, n);
    }
    // Gentle prior: prefer typical playing range. Frets 0-12 get full score,
    // 12-15 slight penalty, 15+ increasingly penalized.
    let prior = if fret <= 12.0 {
        1.0
    } else if fret <= 15.0 {
        1.0 - (fret - 12.0) as f64 * 0.02
    } else {
        0.94 - (fret - 15.0) as f64 * 0.03
    };
    score * prior
}

/// Parabolic interpolation around best fret for sub-0.1 precision.
fn refine_fret(best: f32, open: &[f64; 10], samples: &[f32], sr: f64) -> f32 {
    let step = 0.1f32;
    let below = (best - step).max(0.0);
    let above = (best + step).min(24.0);
    let s_below = score_fret(below, open, samples, sr);
    let s_center = score_fret(best, open, samples, sr);
    let s_above = score_fret(above, open, samples, sr);
    let denom = s_below - 2.0 * s_center + s_above;
    if denom.abs() < 1e-20 {
        return best;
    }
    let offset = 0.5 * (s_below - s_above) / denom;
    (best + (offset as f32) * step).clamp(0.0, 24.0)
}

/// Goertzel algorithm: compute magnitude of a single frequency bin.
/// Much cheaper than FFT when you only need specific frequencies.
fn goertzel_magnitude(samples: &[f32], freq: f64, sample_rate: f64, n: usize) -> f64 {
    let k = (freq * n as f64 / sample_rate).round();
    let w = 2.0 * std::f64::consts::PI * k / n as f64;
    let coeff = 2.0 * w.cos();
    let mut s1 = 0.0f64;
    let mut s2 = 0.0f64;
    for sample in samples.iter().take(n) {
        let s0 = *sample as f64 + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - coeff * s1 * s2).abs().sqrt()
}

fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn sine_wave(freq_hz: f64, sr: u32, ms: u32) -> Vec<f32> {
        let n = (sr as u64 * ms as u64 / 1000) as usize;
        (0..n)
            .map(|i| (0.8 * (2.0 * PI * freq_hz * i as f64 / sr as f64).sin()) as f32)
            .collect()
    }

    fn multi_sine(freqs: &[f64], sr: u32, ms: u32) -> Vec<f32> {
        let n = (sr as u64 * ms as u64 / 1000) as usize;
        let amp = 0.6 / freqs.len() as f64;
        (0..n)
            .map(|i| {
                let t = i as f64 / sr as f64;
                freqs
                    .iter()
                    .map(|&f| amp * (2.0 * PI * f * t).sin())
                    .sum::<f64>() as f32
            })
            .collect()
    }

    fn feed_and_infer(
        inf: &mut BarInference,
        samples: &[f32],
        sr: u32,
        sensor: &SensorFrame,
        engine: &CopedantEngine,
    ) -> BarState {
        let chunk = AudioChunk {
            timestamp_us: 0,
            samples: samples.to_vec(),
            sample_rate: sr,
        };
        inf.push_audio(&chunk);
        // Force the analysis window to be satisfied
        inf.analysis_window = samples.len().min(inf.analysis_window);
        inf.samples_since_analysis = inf.analysis_interval;
        inf.infer(sensor, engine)
    }

    /// Create a SensorFrame with bar sensors simulating bar at given fret
    fn sensor_at_fret(fret: f32) -> SensorFrame {
        use crate::bar_sensor::simulate_bar_readings;
        let mut s = SensorFrame::at_rest(0);
        s.bar_sensors = simulate_bar_readings(fret);
        s
    }

    #[test]
    fn test_goertzel_finds_frequency() {
        let samples = sine_wave(440.0, 48000, 100);
        let n = samples.len();
        let m440 = goertzel_magnitude(&samples, 440.0, 48000.0, n);
        let m300 = goertzel_magnitude(&samples, 300.0, 48000.0, n);
        assert!(
            m440 > m300 * 5.0,
            "440={:.1} should >> 300={:.1}",
            m440,
            m300
        );
    }

    #[test]
    fn test_sensor_only_during_silence() {
        use crate::copedant::buddy_emmons_e9;
        let engine = CopedantEngine::new(buddy_emmons_e9());
        let mut inf = BarInference::new();
        // Bar at fret 3, but no audio (silence)
        let sensor = sensor_at_fret(3.0);
        let r = inf.infer(&sensor, &engine);
        assert!(
            r.position.is_some(),
            "sensor should detect bar during silence"
        );
        let p = r.position.unwrap();
        assert!((p - 3.0).abs() < 1.0, "pos={:.2}, want ~3.0", p);
        assert_eq!(r.source, BarSource::Sensor);
    }

    #[test]
    fn test_fused_with_audio() {
        use crate::copedant::buddy_emmons_e9;
        let engine = CopedantEngine::new(buddy_emmons_e9());
        let mut inf = BarInference::new();
        // Bar at fret 3 with matching audio
        let sensor = sensor_at_fret(3.0);
        let open = engine.effective_open_pitches(&sensor);
        let freqs: Vec<f64> = [2, 3, 4]
            .iter()
            .map(|&si| midi_to_hz(open[si] + 3.0))
            .collect();
        let samples = multi_sine(&freqs, 48000, 100);
        let r = feed_and_infer(&mut inf, &samples, 48000, &sensor, &engine);
        assert!(r.position.is_some(), "should detect fused");
        let p = r.position.unwrap();
        assert!((p - 3.0).abs() < 0.5, "pos={:.2}, want ~3.0", p);
        assert_eq!(r.source, BarSource::Fused);
    }

    #[test]
    fn test_fused_with_pedal_a() {
        use crate::copedant::buddy_emmons_e9;
        let engine = CopedantEngine::new(buddy_emmons_e9());
        let mut inf = BarInference::new();
        let mut sensor = sensor_at_fret(5.0);
        sensor.pedals[0] = 1.0;
        let open = engine.effective_open_pitches(&sensor);
        let freqs: Vec<f64> = [2, 3, 4]
            .iter()
            .map(|&si| midi_to_hz(open[si] + 5.0))
            .collect();
        let samples = multi_sine(&freqs, 48000, 100);
        let r = feed_and_infer(&mut inf, &samples, 48000, &sensor, &engine);
        assert!(r.position.is_some());
        let p = r.position.unwrap();
        assert!((p - 5.0).abs() < 0.5, "pos={:.2}, want ~5.0", p);
    }

    #[test]
    fn test_silence_with_no_bar() {
        use crate::copedant::buddy_emmons_e9;
        let engine = CopedantEngine::new(buddy_emmons_e9());
        let mut inf = BarInference::new();
        // No bar sensors, no audio
        let sensor = SensorFrame::at_rest(0);
        let r = inf.infer(&sensor, &engine);
        assert!(r.position.is_none());
        assert_eq!(r.source, BarSource::None);
    }

    #[test]
    fn test_bar_lifted_returns_none() {
        use crate::copedant::buddy_emmons_e9;
        let engine = CopedantEngine::new(buddy_emmons_e9());
        let mut inf = BarInference::new();
        // First place bar
        let sensor = sensor_at_fret(3.0);
        let _ = inf.infer(&sensor, &engine);
        assert!(inf.last_position.is_some());
        // Then lift it (all sensors zero, no audio)
        let sensor = SensorFrame::at_rest(0);
        let r = inf.infer(&sensor, &engine);
        assert!(r.position.is_none());
    }
}
