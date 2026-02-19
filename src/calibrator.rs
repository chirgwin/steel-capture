//! Interactive per-string threshold calibrator.
//!
//! Reads `InputEvent::Audio` chunks from a channel — compatible with both
//! live cpal capture and pre-recorded WAV input (`--audio-file`).
//!
//! Protocol: bar at fret 0, no pedals/levers. For each string, press Enter
//! then pluck for 2s, then be silent for 2s. Thresholds are derived from
//! the bimodal energy distribution and written to `calibration.json`.

use crate::calibration::{Calibration, StringThreshold};
use crate::copedant::{midi_to_hz, CopedantEngine};
use crate::dsp::goertzel_magnitude;
use crate::types::{InputEvent, SensorFrame};
use crossbeam_channel::Receiver;
use log::{info, warn};
use std::io::{self, Write};
use std::thread;
use std::time::Duration;

const PLUCK_SECS: f64 = 2.0;
const SILENCE_SECS: f64 = 2.0;
const ANALYSIS_WINDOW: usize = 4096;

/// E9 open string names, index 0 = string 1 (far from player)
const STRING_NAMES: [&str; 10] = [
    "F#4", "D#4", "G#4", "E4", "B3", "G#3", "F#3", "E3", "D3", "B2",
];

pub struct Calibrator {
    audio_rx: Receiver<InputEvent>,
    engine: CopedantEngine,
}

impl Calibrator {
    pub fn new(audio_rx: Receiver<InputEvent>, engine: CopedantEngine) -> Self {
        Self { audio_rx, engine }
    }

    pub fn run(&self) -> Calibration {
        // Open pitches at rest (no pedals/levers, bar at fret 0)
        let sensor = SensorFrame::at_rest(0);
        let open_pitches = self.engine.effective_open_pitches(&sensor);
        let open_freqs: [f64; 10] = {
            let mut f = [0.0f64; 10];
            for i in 0..10 {
                f[i] = midi_to_hz(open_pitches[i]);
            }
            f
        };

        println!("\n╔═══════════════════════════════════════════════╗");
        println!("║   Steel Capture — Per-String Calibration      ║");
        println!("╠═══════════════════════════════════════════════╣");
        println!("║  Bar at fret 0 (nut), no pedals or levers.    ║");
        println!("║  For each string: press Enter, then pluck      ║");
        println!(
            "║  ({:.0}s), then be quiet ({:.0}s).                ║",
            PLUCK_SECS, SILENCE_SECS
        );
        println!("╚═══════════════════════════════════════════════╝\n");

        let mut thresholds = Vec::new();

        for si in 0..10 {
            let freq = open_freqs[si];
            let name = STRING_NAMES[si];

            println!("── String {} ({}) — {:.1} Hz", si + 1, name, freq);
            print!("   Press Enter when ready...");
            io::stdout().flush().unwrap();

            let mut line = String::new();
            io::stdin().read_line(&mut line).ok();

            // Drain audio accumulated during the stdin wait.
            while self.audio_rx.try_recv().is_ok() {}

            // Countdown so the user can get their pick into position.
            for n in [3u32, 2, 1] {
                print!("\r   Pluck in {}...   ", n);
                io::stdout().flush().unwrap();
                thread::sleep(Duration::from_millis(800));
            }

            // Drain again: audio piled up during the ~2.4s countdown.
            // Without this, collect_energy_samples reads countdown silence
            // instead of the actual pluck.
            while self.audio_rx.try_recv().is_ok() {}

            println!("\r   ► Pluck string {} now!   ", si + 1);

            // Pluck window
            let pluck_energies = self.collect_energy_samples(freq, PLUCK_SECS);
            println!("   ({} measurements)", pluck_energies.len());

            print!("   Now be silent...");
            io::stdout().flush().unwrap();

            // Silence window
            let silence_energies = self.collect_energy_samples(freq, SILENCE_SECS);
            println!(" done ({} measurements).", silence_energies.len());

            let (onset, release) = compute_thresholds(&pluck_energies, &silence_energies);
            println!("   → onset={:.5}  release={:.5}", onset, release);

            thresholds.push(StringThreshold { onset, release });
        }

        println!("\nCalibration complete!\n");

        Calibration {
            strings: thresholds,
        }
    }

    /// Drain audio from the channel for `duration_secs`, returning a Vec of
    /// Goertzel energy measurements (one per ANALYSIS_WINDOW samples).
    fn collect_energy_samples(&self, freq: f64, duration_secs: f64) -> Vec<f64> {
        // Peek at the first chunk to learn the sample rate
        let mut sample_rate = 48000u32;
        let total_target = (duration_secs * sample_rate as f64) as usize;

        let mut audio_buf: Vec<f32> = Vec::new();
        let mut collected = 0usize;
        let mut energies: Vec<f64> = Vec::new();

        while collected < total_target {
            match self.audio_rx.recv_timeout(Duration::from_millis(500)) {
                Ok(InputEvent::Audio(chunk)) => {
                    sample_rate = chunk.sample_rate;
                    audio_buf.extend_from_slice(&chunk.samples);
                    collected += chunk.samples.len();

                    // Run Goertzel on each full window in the buffer
                    while audio_buf.len() >= ANALYSIS_WINDOW {
                        let window = &audio_buf[..ANALYSIS_WINDOW];
                        let sr = sample_rate as f64;
                        let n = ANALYSIS_WINDOW;

                        let mag = goertzel_magnitude(window, freq, sr, n);
                        let mag2 = if freq * 2.0 < sr / 2.0 {
                            goertzel_magnitude(window, freq * 2.0, sr, n)
                        } else {
                            0.0
                        };
                        energies.push((mag + 0.3 * mag2) / n as f64);

                        audio_buf.drain(..ANALYSIS_WINDOW);
                    }
                }
                Ok(InputEvent::Sensor(_)) => {
                    // Ignore — calibration is audio-only
                }
                Err(_) => {
                    warn!("Audio channel closed or timed out during calibration.");
                    break;
                }
            }
        }

        energies
    }
}

// ─── Threshold math ──────────────────────────────────────────────────────────

/// Derive onset and release thresholds from measured pluck and silence energy distributions.
///
/// Uses pluck_p75 (upper quartile of pluck energy) vs silence_p75 (noise ceiling).
/// p75 is chosen over p25/median because:
///   - The first few windows after "Pluck now!" may still be partially silent
///   - Steel string energy decays; the upper quartile represents active ringing
///   - We want the onset threshold to catch sustained ringing, not just the attack spike
///
/// Always computes a threshold from actual measurements. Falls back to generic defaults
/// only if literally no pluck energy is detected (mic not picking up the instrument at all).
fn compute_thresholds(pluck: &[f64], silence: &[f64]) -> (f64, f64) {
    if pluck.is_empty() || silence.is_empty() {
        warn!("No energy samples collected — using default thresholds");
        return (0.02, 0.008);
    }

    let pluck_p75 = percentile(pluck, 75);
    let pluck_median = percentile(pluck, 50);
    let silence_p75 = percentile(silence, 75);
    let silence_median = percentile(silence, 50);
    let ratio = if silence_p75 > 1e-10 {
        pluck_p75 / silence_p75
    } else {
        f64::MAX
    };

    info!(
        "  pluck: median={:.5} p75={:.5} | silence: median={:.5} p75={:.5} | ratio={:.1}x",
        pluck_median, pluck_p75, silence_median, silence_p75, ratio
    );

    if pluck_p75 < 1e-8 {
        warn!(
            "No pluck energy detected (p75={:.2e}). Mic may not be picking up the instrument. \
             Using default thresholds — detection will likely not work.",
            pluck_p75
        );
        return (0.02, 0.008);
    }

    if pluck_p75 <= silence_p75 {
        // Pluck can't be distinguished from silence at all.
        // Best-effort: set onset just above the noise ceiling.
        let onset = silence_p75 * 1.5;
        let release = silence_p75 * 1.1;
        warn!(
            "Pluck energy ({:.5}) ≤ noise floor ({:.5}). \
             String may be too quiet or mic too far away. \
             Setting onset at noise_floor×1.5={:.5} — verify with --ws.",
            pluck_p75, silence_p75, onset
        );
        return (onset, release);
    }

    // Place onset at the midpoint between pluck floor and noise ceiling.
    let onset = (pluck_p75 + silence_p75) / 2.0;
    let release = onset * 0.4;

    if ratio < 3.0 {
        warn!(
            "Marginal separation ({:.1}x). Detection may be unreliable — \
             try plucking louder or in a quieter room.",
            ratio
        );
    } else {
        info!("  Good separation ({:.1}x).", ratio);
    }

    (onset, release)
}

fn percentile(v: &[f64], p: usize) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut sorted = v.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((p * sorted.len()) / 100).min(sorted.len() - 1);
    sorted[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_thresholds_well_separated() {
        // Clear bimodal: pluck p75 ~0.059, silence p75 ~0.0027
        let pluck: Vec<f64> = (0..20).map(|i| 0.04 + i as f64 * 0.001).collect();
        let silence: Vec<f64> = (0..20).map(|i| 0.0005 + i as f64 * 0.0001).collect();
        let (onset, release) = compute_thresholds(&pluck, &silence);
        // onset = midpoint between pluck_p75 and silence_p75
        assert!(
            onset > 0.01 && onset < 0.06,
            "onset={} not in expected range",
            onset
        );
        assert!(release < onset, "release should be below onset");
        assert!(
            (release - onset * 0.4).abs() < 1e-9,
            "release should be onset×0.4"
        );
    }

    #[test]
    fn test_compute_thresholds_poor_separation_best_effort() {
        // Marginal: pluck_p75 > silence_p75 but ratio < 3×
        let pluck: Vec<f64> = vec![0.02; 10];
        let silence: Vec<f64> = vec![0.015; 10];
        let (onset, release) = compute_thresholds(&pluck, &silence);
        // Should compute a threshold between the two values, not use the 0.02 default
        assert!(
            onset > 0.015 && onset < 0.02,
            "onset={} should be between silence and pluck",
            onset
        );
        assert!(release < onset);
    }

    #[test]
    fn test_compute_thresholds_pluck_below_noise_uses_noise_ceiling() {
        // Pluck is quieter than ambient — physically can't calibrate; set above noise
        let pluck: Vec<f64> = vec![0.001; 10];
        let silence: Vec<f64> = vec![0.005; 10];
        let (onset, release) = compute_thresholds(&pluck, &silence);
        assert!(
            onset > 0.005,
            "onset should be above noise floor, got {}",
            onset
        );
        assert!(
            release > 0.005,
            "release should also be above noise floor, got {}",
            release
        );
        assert!(release < onset);
    }

    #[test]
    fn test_percentile() {
        let v: Vec<f64> = (1..=10).map(|x| x as f64).collect();
        assert_eq!(percentile(&v, 0), 1.0);
        assert_eq!(percentile(&v, 100), 10.0);
        // Floor-based: (50*10/100)=5 → sorted[5]=6 for [1..10]
        assert_eq!(percentile(&v, 50), 6.0);
        assert_eq!(percentile(&v, 25), 3.0); // (25*10/100)=2 → sorted[2]=3
        assert_eq!(percentile(&v, 75), 8.0); // (75*10/100)=7 → sorted[7]=8
    }
}
