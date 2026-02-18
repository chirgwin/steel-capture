use crate::types::*;

/// Computes the theoretical pitch of each string given mechanical state.
pub struct CopedantEngine {
    copedant: Copedant,
}

impl CopedantEngine {
    pub fn new(copedant: Copedant) -> Self {
        Self { copedant }
    }

    /// Given the current pedal/lever engagement, compute each string's
    /// effective open pitch (MIDI note number). "Open" here means the pitch
    /// the string would produce if the bar were at the nut (fret 0).
    /// Partial pedal engagement produces proportional pitch bending.
    pub fn effective_open_pitches(&self, sensor: &SensorFrame) -> [f64; 10] {
        let mut midi = self.copedant.open_strings;

        // Apply pedal contributions
        for (i, pedal_def) in self.copedant.pedals.iter().enumerate() {
            if i < 3 {
                let engagement = sensor.pedals[i] as f64;
                for &(string_idx, delta) in &pedal_def.changes {
                    if string_idx < 10 {
                        midi[string_idx] += delta * engagement;
                    }
                }
            }
        }

        // Apply knee lever contributions
        for (i, lever_def) in self.copedant.levers.iter().enumerate() {
            if i < 5 {
                let engagement = sensor.knee_levers[i] as f64;
                for &(string_idx, delta) in &lever_def.changes {
                    if string_idx < 10 {
                        midi[string_idx] += delta * engagement;
                    }
                }
            }
        }

        midi
    }

    /// Given effective open pitches and a bar position (in frets), compute
    /// the sounding pitch of each string in Hz.
    ///
    /// Bar at fret N raises each string by N semitones.
    /// Bar slant (if known) applies a per-string offset, but for now
    /// we assume slant=0 (bar perpendicular to strings).
    pub fn pitches_at_bar(&self, effective_open: &[f64; 10], bar_fret: f32) -> [f64; 10] {
        let mut hz = [0.0f64; 10];
        for i in 0..10 {
            hz[i] = midi_to_hz(effective_open[i] + bar_fret as f64);
        }
        hz
    }

    /// Convenience: compute pitches from sensor frame + bar position.
    pub fn compute_pitches(&self, sensor: &SensorFrame, bar_fret: Option<f32>) -> [f64; 10] {
        let open = self.effective_open_pitches(sensor);
        match bar_fret {
            Some(fret) => self.pitches_at_bar(&open, fret),
            None => {
                // No bar detected — return open string pitches
                let mut hz = [0.0f64; 10];
                for i in 0..10 {
                    hz[i] = midi_to_hz(open[i]);
                }
                hz
            }
        }
    }

    /// Given a detected pitch (Hz) and the effective open pitch of a string,
    /// infer the bar position in fret-space.
    ///
    /// bar_fret = 12 * log2(detected_hz / open_hz)
    ///
    /// Returns None if the math doesn't make sense (e.g., detected < open,
    /// which would mean the bar is behind the nut).
    pub fn infer_bar_position(
        &self,
        detected_hz: f64,
        string_idx: usize,
        sensor: &SensorFrame,
    ) -> Option<f32> {
        let open = self.effective_open_pitches(sensor);
        if string_idx >= 10 {
            return None;
        }
        let open_hz = midi_to_hz(open[string_idx]);
        if detected_hz <= 0.0 || open_hz <= 0.0 {
            return None;
        }
        let ratio = detected_hz / open_hz;
        if ratio < 0.5 {
            return None; // More than an octave below open — nonsensical
        }
        let fret = 12.0 * ratio.log2();
        if !(-0.5..=30.0).contains(&fret) {
            return None; // Out of reasonable range
        }
        Some(fret as f32)
    }

    pub fn copedant(&self) -> &Copedant {
        &self.copedant
    }
}

/// Convert MIDI note number (fractional) to Hz. A4 = MIDI 69 = 440 Hz.
pub fn midi_to_hz(midi: f64) -> f64 {
    440.0 * 2.0_f64.powf((midi - 69.0) / 12.0)
}

/// Convert Hz to MIDI note number (fractional).
pub fn hz_to_midi(hz: f64) -> f64 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

/// Buddy Emmons E9 copedant.
///
/// Source: b0b.com/wp/copedents/buddy-emmons-e9th/ (primary)
///         Wikipedia "Copedent" article, adapted from GFI Music Company.
///
/// Open tuning (string 1=far from player, string 10=near):
///   1:F#4  2:D#4  3:G#4  4:E4  5:B3  6:G#3  7:F#3  8:E3  9:D3  10:B2
///
/// Per b0b.com: Buddy's copedent is "very typical of how most new guitars
/// are configured today, except for the lack of a 1st string raise on RKL."
/// We follow Buddy's actual setup (no str1 raise on RKL).
///
/// RKR has a two-stop mechanism:
///   Soft stop (-1): str2 D#→D, str9 D→C#
///   Hard stop (-2): str2 D#→C#, str9 D→C# (same)
/// We model full engagement as the hard stop for now.
pub fn buddy_emmons_e9() -> Copedant {
    Copedant {
        name: "Buddy Emmons E9".to_string(),

        // MIDI note numbers. String 1 is index 0.
        //       str1   str2   str3   str4   str5   str6   str7   str8   str9  str10
        //       F#4    D#4    G#4    E4     B3     G#3    F#3    E3     D3    B2
        open_strings: [66.0, 63.0, 68.0, 64.0, 59.0, 56.0, 54.0, 52.0, 50.0, 47.0],

        pedals: vec![
            // Pedal A (P1): raises str5 and str10 by 2 semitones (B→C#)
            ChangeDef {
                name: "A".into(),
                changes: vec![
                    (4, 2.0), // str5: B3 → C#4
                    (9, 2.0), // str10: B2 → C#3
                ],
            },
            // Pedal B (P2): raises str3 and str6 by 1 semitone (G#→A)
            ChangeDef {
                name: "B".into(),
                changes: vec![
                    (2, 1.0), // str3: G#4 → A4
                    (5, 1.0), // str6: G#3 → A3
                ],
            },
            // Pedal C (P3): raises str4 by 2 (E→F#) and str5 by 2 (B→C#)
            ChangeDef {
                name: "C".into(),
                changes: vec![
                    (3, 2.0), // str4: E4 → F#4
                    (4, 2.0), // str5: B3 → C#4
                ],
            },
        ],

        levers: vec![
            // LKL: raises str4 and str8 by 1 semitone (E→F)
            ChangeDef {
                name: "LKL".into(),
                changes: vec![
                    (3, 1.0), // str4: E4 → F4
                    (7, 1.0), // str8: E3 → F3
                ],
            },
            // LKR: lowers str4, str5, and str8 by 1 semitone
            ChangeDef {
                name: "LKR".into(),
                changes: vec![
                    (3, -1.0), // str4: E4 → D#4/Eb4
                    (4, -1.0), // str5: B3 → A#3/Bb3
                    (7, -1.0), // str8: E3 → D#3/Eb3
                ],
            },
            // LKV (vertical): lowers str5 and str10 by 1 semitone (B→A#/Bb)
            ChangeDef {
                name: "LKV".into(),
                changes: vec![
                    (4, -1.0), // str5: B3 → A#3/Bb3
                    (9, -1.0), // str10: B2 → A#2/Bb2
                ],
            },
            // RKL: raises str2 by 1 (D#→E), lowers str6 by 2 (G#→F#)
            // NOTE: Buddy Emmons did NOT raise str1 on RKL.
            //       Modern "Nashville standard" adds str1 +2 (F#→G#).
            ChangeDef {
                name: "RKL".into(),
                changes: vec![
                    (1, 1.0),  // str2: D#4 → E4
                    (5, -2.0), // str6: G#3 → F#3
                ],
            },
            // RKR: Two-stop lever.
            //   Soft stop: str2 -1 (D#→D), str9 -1 (D→C#)
            //   Hard stop: str2 -2 (D#→C#), str9 -1 (same)
            // Modeled as: full engagement = hard stop.
            // At ~50% engagement, str2 ≈ -1 (the soft stop feel).
            ChangeDef {
                name: "RKR".into(),
                changes: vec![
                    (1, -2.0), // str2: D#4 → C#4 (full push)
                    (8, -1.0), // str9: D3 → C#3
                ],
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> CopedantEngine {
        CopedantEngine::new(buddy_emmons_e9())
    }

    #[test]
    fn test_midi_to_hz_roundtrip() {
        assert!((midi_to_hz(69.0) - 440.0).abs() < 0.01);
        assert!((midi_to_hz(60.0) - 261.63).abs() < 0.1);
        assert!((hz_to_midi(440.0) - 69.0).abs() < 0.001);
    }

    #[test]
    fn test_open_string_pitches() {
        let e = engine();
        let s = SensorFrame::at_rest(0);
        let open = e.effective_open_pitches(&s);
        // String 5 (idx 4) = B3 = MIDI 59
        assert!((open[4] - 59.0).abs() < 0.001);
    }

    #[test]
    fn test_pedal_a_raises() {
        let e = engine();
        let mut s = SensorFrame::at_rest(0);
        s.pedals[0] = 1.0; // Pedal A fully engaged
        let open = e.effective_open_pitches(&s);
        // String 5: B3 + 2 = C#4 = MIDI 61
        assert!((open[4] - 61.0).abs() < 0.001);
        // String 10: B2 + 2 = C#3 = MIDI 49
        assert!((open[9] - 49.0).abs() < 0.001);
    }

    #[test]
    fn test_partial_pedal() {
        let e = engine();
        let mut s = SensorFrame::at_rest(0);
        s.pedals[0] = 0.5; // Pedal A half engaged
        let open = e.effective_open_pitches(&s);
        // String 5: B3 + 1.0 = C4 = MIDI 60
        assert!((open[4] - 60.0).abs() < 0.001);
    }

    #[test]
    fn test_bar_position_inference() {
        let e = engine();
        let s = SensorFrame::at_rest(0);
        // String 4 open = E4 = MIDI 64 = 329.63 Hz
        // Bar at fret 3 → E4 + 3 = G4 = MIDI 67 = 392.00 Hz
        let detected = midi_to_hz(67.0);
        let inferred = e.infer_bar_position(detected, 3, &s);
        assert!(inferred.is_some());
        assert!((inferred.unwrap() - 3.0).abs() < 0.01);
    }

    #[test]
    fn test_bar_inference_with_pedal() {
        let e = engine();
        let mut s = SensorFrame::at_rest(0);
        s.pedals[0] = 1.0; // Pedal A: string 5 is now C#4 (MIDI 61)
                           // Bar at fret 5 → C#4 + 5 = F#4 = MIDI 66
        let detected = midi_to_hz(66.0);
        let inferred = e.infer_bar_position(detected, 4, &s);
        assert!(inferred.is_some());
        assert!((inferred.unwrap() - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_compute_pitches_at_fret() {
        let e = engine();
        let s = SensorFrame::at_rest(0);
        let pitches = e.compute_pitches(&s, Some(3.0));
        // String 4 at fret 3: E4+3 = G4 ≈ 392 Hz
        assert!((pitches[3] - 392.0).abs() < 1.0);
    }

    #[test]
    fn test_lkl_raises_e_to_f() {
        let e = engine();
        let mut s = SensorFrame::at_rest(0);
        s.knee_levers[0] = 1.0; // LKL fully engaged
        let open = e.effective_open_pitches(&s);
        // str4: E4 (64) + 1 = F4 (65)
        assert!((open[3] - 65.0).abs() < 0.001);
        // str8: E3 (52) + 1 = F3 (53)
        assert!((open[7] - 53.0).abs() < 0.001);
    }

    #[test]
    fn test_lkr_lowers_e_to_eb() {
        let e = engine();
        let mut s = SensorFrame::at_rest(0);
        s.knee_levers[1] = 1.0; // LKR fully engaged
        let open = e.effective_open_pitches(&s);
        // str4: E4 (64) - 1 = D#4/Eb4 (63)
        assert!((open[3] - 63.0).abs() < 0.001);
        // str5: B3 (59) - 1 = A#3/Bb3 (58)
        assert!((open[4] - 58.0).abs() < 0.001);
        // str8: E3 (52) - 1 = D#3/Eb3 (51)
        assert!((open[7] - 51.0).abs() < 0.001);
    }

    #[test]
    fn test_pedal_c_raises_e_and_b() {
        let e = engine();
        let mut s = SensorFrame::at_rest(0);
        s.pedals[2] = 1.0; // Pedal C fully engaged
        let open = e.effective_open_pitches(&s);
        // str4: E4 (64) + 2 = F#4 (66)
        assert!((open[3] - 66.0).abs() < 0.001);
        // str5: B3 (59) + 2 = C#4 (61)
        assert!((open[4] - 61.0).abs() < 0.001);
    }

    #[test]
    fn test_rkl_changes() {
        let e = engine();
        let mut s = SensorFrame::at_rest(0);
        s.knee_levers[3] = 1.0; // RKL fully engaged
        let open = e.effective_open_pitches(&s);
        // str1: F#4 (66) — NO CHANGE (Buddy Emmons didn't raise str1 on RKL)
        assert!((open[0] - 66.0).abs() < 0.001);
        // str2: D#4 (63) + 1 = E4 (64)
        assert!((open[1] - 64.0).abs() < 0.001);
        // str6: G#3 (56) - 2 = F#3 (54)
        assert!((open[5] - 54.0).abs() < 0.001);
    }

    #[test]
    fn test_rkr_hard_stop() {
        let e = engine();
        let mut s = SensorFrame::at_rest(0);
        s.knee_levers[4] = 1.0; // RKR fully engaged (hard stop)
        let open = e.effective_open_pitches(&s);
        // str2: D#4 (63) - 2 = C#4 (61)
        assert!((open[1] - 61.0).abs() < 0.001);
        // str9: D3 (50) - 1 = C#3 (49)
        assert!((open[8] - 49.0).abs() < 0.001);
    }

    #[test]
    fn test_rkr_soft_stop() {
        let e = engine();
        let mut s = SensorFrame::at_rest(0);
        s.knee_levers[4] = 0.5; // RKR half engaged (soft stop)
        let open = e.effective_open_pitches(&s);
        // str2: D#4 (63) - 1 = D4 (62) — soft stop approximation
        assert!((open[1] - 62.0).abs() < 0.001);
        // str9: D3 (50) - 0.5 = between D and C#
        assert!((open[8] - 49.5).abs() < 0.001);
    }

    #[test]
    fn test_pedal_a_plus_c() {
        // A classic technique: A+C together raises string 5 by 4 semitones
        let e = engine();
        let mut s = SensorFrame::at_rest(0);
        s.pedals[0] = 1.0; // A
        s.pedals[2] = 1.0; // C
        let open = e.effective_open_pitches(&s);
        // str5: B3 (59) + 2 (A) + 2 (C) = D#4 (63)
        assert!((open[4] - 63.0).abs() < 0.001);
        // str4: E4 (64) + 2 (C only) = F#4 (66)
        assert!((open[3] - 66.0).abs() < 0.001);
        // str10: B2 (47) + 2 (A only) = C#3 (49)
        assert!((open[9] - 49.0).abs() < 0.001);
    }
}
