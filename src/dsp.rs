//! Shared DSP primitives: Goertzel single-bin magnitude, RMS, and test signal generators.

/// Goertzel algorithm: compute magnitude of a single frequency bin.
/// Much cheaper than FFT when you only need specific frequencies.
pub fn goertzel_magnitude(samples: &[f32], freq: f64, sample_rate: f64, n: usize) -> f64 {
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

/// Root mean square of an audio buffer.
pub fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

/// Test signal generators — available to unit and integration tests.
#[cfg(test)]
pub mod test_helpers {
    use std::f64::consts::PI;

    /// Generate a mono sine wave.
    pub fn sine_wave(freq_hz: f64, amp: f64, sr: u32, ms: u32) -> Vec<f32> {
        let n = (sr as u64 * ms as u64 / 1000) as usize;
        (0..n)
            .map(|i| (amp * (2.0 * PI * freq_hz * i as f64 / sr as f64).sin()) as f32)
            .collect()
    }

    /// Generate a mix of sine waves at equal amplitude per voice.
    pub fn multi_sine(freqs: &[f64], amp_per_voice: f64, sr: u32, ms: u32) -> Vec<f32> {
        let n = (sr as u64 * ms as u64 / 1000) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / sr as f64;
                freqs
                    .iter()
                    .map(|&f| amp_per_voice * (2.0 * PI * f * t).sin())
                    .sum::<f64>() as f32
            })
            .collect()
    }
}
