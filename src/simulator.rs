use crate::bar_sensor::simulate_bar_readings;
use crate::copedant::{midi_to_hz, CopedantEngine};
use crate::types::*;
use crossbeam_channel::Sender;
use log::info;
use std::f32::consts::PI;
use std::thread;
use std::time::Duration;

/// Generates realistic simulated sensor data and synthetic audio
/// that exercises the full capture pipeline without any hardware.
pub struct Simulator {
    clock: SessionClock,
    tx: Sender<InputEvent>,
    engine: CopedantEngine,
    sample_rate: u32,
    sensor_rate_hz: u32,
    /// Monotonic sample counter for phase-continuous audio generation.
    /// Uses sample count instead of wall-clock time to avoid phase
    /// discontinuities from OS scheduling jitter.
    sample_counter: u64,
}

/// Mutable state that evolves as gestures are applied.
#[derive(Clone)]
struct SimState {
    pedals: [f32; 3],
    knee_levers: [f32; 5],
    volume: f32,
    bar_fret: Option<f32>, // None = bar not on strings
    string_active: [bool; 10], // which strings are sounding
}

impl Default for SimState {
    fn default() -> Self {
        Self {
            pedals: [0.0; 3],
            knee_levers: [0.0; 5],
            volume: 0.0,
            bar_fret: None,
            string_active: [false; 10],
        }
    }
}

impl Simulator {
    pub fn new(
        clock: SessionClock,
        tx: Sender<InputEvent>,
        copedant: Copedant,
        sensor_rate_hz: u32,
    ) -> Self {
        Self {
            clock,
            tx,
            engine: CopedantEngine::new(copedant),
            sample_rate: 48000,
            sensor_rate_hz,
            sample_counter: 0,
        }
    }

    /// Run a named demo sequence. `demo` is one of "basic" or "e9".
    /// Blocks the calling thread.
    pub fn run(&mut self, demo: &str) {
        info!("Simulator starting '{}' sequence...", demo);
        let mut state = SimState::default();
        let tick_us = 1_000_000 / self.sensor_rate_hz as u64;

        let gestures = match demo {
            "e9"    => e9_moves_sequence(),
            "improv" => improvise_sequence(0xc0ffee_u64, 200),
            _       => demo_sequence(),
        };

        for gesture in &gestures {
            self.execute(gesture, &mut state, tick_us);
        }

        info!("Sequence '{}' complete. Holding final state...", demo);
        // Hold indefinitely so the system stays alive
        loop {
            self.emit_tick(&state, tick_us);
        }
    }

    fn execute(&mut self, gesture: &Gesture, state: &mut SimState, tick_us: u64) {
        match gesture {
            Gesture::Hold { ms } => {
                info!("  hold {}ms", ms);
                let ticks = (*ms as u64 * 1000) / tick_us;
                for _ in 0..ticks {
                    self.emit_tick(state, tick_us);
                }
            }

            Gesture::VolumeSwell { from, to, ms } => {
                info!("  volume {:.2} → {:.2} over {}ms", from, to, ms);
                let ticks = (*ms as u64 * 1000) / tick_us;
                let start = *from;
                for i in 0..ticks {
                    let t = i as f32 / ticks as f32;
                    state.volume = lerp(start, *to, smoothstep(t));
                    self.emit_tick(state, tick_us);
                }
            }

            Gesture::BarPlace { fret } => {
                info!("  bar placed at fret {:.1}", fret);
                state.bar_fret = Some(*fret);
            }

            Gesture::BarLift => {
                info!("  bar lifted");
                state.bar_fret = None;
            }

            Gesture::BarSlide { to, ms } => {
                let from = state.bar_fret.unwrap_or(0.0);
                info!("  bar slide {:.1} → {:.1} over {}ms", from, to, ms);
                let ticks = (*ms as u64 * 1000) / tick_us;
                for i in 0..ticks {
                    let t = i as f32 / ticks as f32;
                    state.bar_fret = Some(lerp(from, *to, smoothstep(t)));
                    self.emit_tick(state, tick_us);
                }
            }

            Gesture::BarVibrato { width, rate_hz, ms } => {
                let center = state.bar_fret.unwrap_or(3.0);
                info!("  vibrato center={:.1} width={:.2} rate={}Hz for {}ms",
                      center, width, rate_hz, ms);
                let ticks = (*ms as u64 * 1000) / tick_us;
                for i in 0..ticks {
                    let t_sec = (i as f32 * tick_us as f32) / 1_000_000.0;
                    let offset = width * (2.0 * PI * rate_hz * t_sec).sin();
                    state.bar_fret = Some(center + offset);
                    self.emit_tick(state, tick_us);
                }
                state.bar_fret = Some(center);
            }

            Gesture::PedalEngage { index, ms } => {
                info!("  pedal {} engage over {}ms", PEDAL_NAMES[*index], ms);
                let from = state.pedals[*index];
                let ticks = (*ms as u64 * 1000) / tick_us;
                for i in 0..ticks {
                    let t = i as f32 / ticks as f32;
                    state.pedals[*index] = lerp(from, 1.0, smoothstep(t));
                    self.emit_tick(state, tick_us);
                }
                state.pedals[*index] = 1.0;
            }

            Gesture::PedalRelease { index, ms } => {
                info!("  pedal {} release over {}ms", PEDAL_NAMES[*index], ms);
                let from = state.pedals[*index];
                let ticks = (*ms as u64 * 1000) / tick_us;
                for i in 0..ticks {
                    let t = i as f32 / ticks as f32;
                    state.pedals[*index] = lerp(from, 0.0, smoothstep(t));
                    self.emit_tick(state, tick_us);
                }
                state.pedals[*index] = 0.0;
            }

            Gesture::LeverEngage { index, ms } => {
                info!("  lever {} engage over {}ms", LEVER_NAMES[*index], ms);
                let from = state.knee_levers[*index];
                let ticks = (*ms as u64 * 1000) / tick_us;
                for i in 0..ticks {
                    let t = i as f32 / ticks as f32;
                    state.knee_levers[*index] = lerp(from, 1.0, smoothstep(t));
                    self.emit_tick(state, tick_us);
                }
                state.knee_levers[*index] = 1.0;
            }

            Gesture::LeverRelease { index, ms } => {
                info!("  lever {} release over {}ms", LEVER_NAMES[*index], ms);
                let from = state.knee_levers[*index];
                let ticks = (*ms as u64 * 1000) / tick_us;
                for i in 0..ticks {
                    let t = i as f32 / ticks as f32;
                    state.knee_levers[*index] = lerp(from, 0.0, smoothstep(t));
                    self.emit_tick(state, tick_us);
                }
                state.knee_levers[*index] = 0.0;
            }

            Gesture::PickStrings { strings } => {
                let names: Vec<String> = strings.iter().map(|s| format!("{}", s + 1)).collect();
                info!("  pick strings [{}]", names.join(", "));
                state.string_active = [false; 10];
                for &si in strings {
                    if si < 10 {
                        state.string_active[si] = true;
                    }
                }
            }

            Gesture::MuteAll => {
                info!("  mute all strings");
                state.string_active = [false; 10];
            }
        }
    }

    /// Emit one tick: send a SensorFrame and a corresponding AudioChunk.
    fn emit_tick(&mut self, state: &SimState, tick_us: u64) {
        let ts = self.clock.now_us();

        // Sensor frame (what the Teensy would send)
        let bar_sensors = match state.bar_fret {
            Some(fret) => simulate_bar_readings(fret),
            None => [0.0; 4],
        };
        let sensor = SensorFrame {
            timestamp_us: ts,
            pedals: state.pedals,
            knee_levers: state.knee_levers,
            volume: state.volume,
            bar_sensors,
            string_active: state.string_active,
        };
        let _ = self.tx.send(InputEvent::Sensor(sensor));

        // Synthetic audio: generate sine waves matching the current pitch state.
        // Uses sample_counter for phase-continuous audio across ticks.
        let any_active = state.string_active.iter().any(|&a| a);
        if state.bar_fret.is_some() && state.volume > 0.01 && any_active {
            let chunk = self.generate_audio(state, ts);
            let _ = self.tx.send(InputEvent::Audio(chunk));
        }

        thread::sleep(Duration::from_micros(tick_us));
    }

    /// Generate a short audio chunk (one tick's worth of samples) containing
    /// sine waves at the pitches implied by the current state.
    /// Uses sample_counter for phase continuity across chunks.
    fn generate_audio(&mut self, state: &SimState, ts: u64) -> AudioChunk {
        let sensor = SensorFrame {
            timestamp_us: ts,
            pedals: state.pedals,
            knee_levers: state.knee_levers,
            volume: state.volume,
            bar_sensors: [0.0; 4], // not used for audio generation
            string_active: state.string_active,
        };
        let bar_fret = state.bar_fret.unwrap_or(0.0);
        let open = self.engine.effective_open_pitches(&sensor);

        let samples_per_tick = self.sample_rate / self.sensor_rate_hz;
        let mut samples = vec![0.0f32; samples_per_tick as usize];

        // Use string_active to determine which strings are sounding
        let active_count = state.string_active.iter().filter(|&&a| a).count();
        let amp_per_string = if active_count > 0 {
            state.volume * 0.6 / active_count as f32
        } else {
            0.0
        };

        for si in 0..10 {
            if !state.string_active[si] { continue; }
            let freq = midi_to_hz(open[si] + bar_fret as f64);
            for (j, sample) in samples.iter_mut().enumerate() {
                // Use sample_counter for monotonic, jitter-free phase
                let t = (self.sample_counter + j as u64) as f64 / self.sample_rate as f64;
                *sample += amp_per_string * (2.0 * std::f64::consts::PI * freq * t).sin() as f32;
            }
        }

        self.sample_counter += samples_per_tick as u64;

        AudioChunk {
            timestamp_us: ts,
            samples,
            sample_rate: self.sample_rate,
        }
    }
}

// ─── Gesture types ──────────────────────────────────────────────────────────

enum Gesture {
    Hold { ms: u32 },
    VolumeSwell { from: f32, to: f32, ms: u32 },
    BarPlace { fret: f32 },
    BarLift,
    BarSlide { to: f32, ms: u32 },
    BarVibrato { width: f32, rate_hz: f32, ms: u32 },
    PedalEngage { index: usize, ms: u32 },
    PedalRelease { index: usize, ms: u32 },
    LeverEngage { index: usize, ms: u32 },
    LeverRelease { index: usize, ms: u32 },
    /// Pick specific strings (0-indexed). Sets them active.
    PickStrings { strings: Vec<usize> },
    /// Mute all strings.
    MuteAll,
}

/// A demo sequence that exercises all the major pedal steel gestures.
/// This simulates roughly 15 seconds of playing with specific string picks.
fn demo_sequence() -> Vec<Gesture> {
    vec![
        // Start: silence, volume down
        Gesture::Hold { ms: 200 },

        // Place bar at 3rd fret, pick strings 3-4-5 (G#, E, B — a basic E chord grip)
        Gesture::BarPlace { fret: 3.0 },
        Gesture::PickStrings { strings: vec![2, 3, 4] },
        Gesture::VolumeSwell { from: 0.0, to: 0.9, ms: 400 },
        Gesture::Hold { ms: 500 },

        // Classic country move: engage pedal A (B→C#) with bar at 3
        Gesture::PedalEngage { index: 0, ms: 150 },
        Gesture::Hold { ms: 600 },
        Gesture::PedalRelease { index: 0, ms: 200 },
        Gesture::Hold { ms: 300 },

        // Pick a wider grip: strings 3-4-5-6 for the B pedal move
        Gesture::PickStrings { strings: vec![2, 3, 4, 5] },
        Gesture::PedalEngage { index: 1, ms: 150 },
        Gesture::Hold { ms: 400 },

        // Slide up to 5th fret while B is engaged
        Gesture::BarSlide { to: 5.0, ms: 600 },
        Gesture::PedalRelease { index: 1, ms: 200 },
        Gesture::Hold { ms: 400 },

        // Vibrato at 5th fret
        Gesture::BarVibrato { width: 0.15, rate_hz: 5.5, ms: 1200 },

        // Volume swell down, slide to 8
        Gesture::VolumeSwell { from: 0.9, to: 0.3, ms: 300 },
        Gesture::BarSlide { to: 8.0, ms: 800 },
        Gesture::PickStrings { strings: vec![4, 5, 7] },  // B, G#, E — lower grip
        Gesture::VolumeSwell { from: 0.3, to: 0.9, ms: 300 },
        Gesture::Hold { ms: 500 },

        // Knee lever: LKL (lower E strings)
        Gesture::LeverEngage { index: 0, ms: 200 },
        Gesture::Hold { ms: 600 },
        Gesture::LeverRelease { index: 0, ms: 200 },

        // Knee lever: RKL (raise F# to G)
        Gesture::LeverEngage { index: 3, ms: 200 },
        Gesture::Hold { ms: 500 },
        Gesture::LeverRelease { index: 3, ms: 200 },

        // Pedals A+B together (common combination) — pick melody strings
        Gesture::PickStrings { strings: vec![3, 4, 5] },
        Gesture::PedalEngage { index: 0, ms: 100 },
        Gesture::PedalEngage { index: 1, ms: 120 },
        Gesture::Hold { ms: 600 },

        // Slide down from 8 to 3 with both pedals engaged
        Gesture::BarSlide { to: 3.0, ms: 1000 },

        // Release pedals
        Gesture::PedalRelease { index: 1, ms: 150 },
        Gesture::PedalRelease { index: 0, ms: 180 },

        // Final vibrato and fade
        Gesture::BarVibrato { width: 0.2, rate_hz: 5.0, ms: 1500 },
        Gesture::VolumeSwell { from: 0.9, to: 0.0, ms: 800 },

        // Mute and lift bar
        Gesture::MuteAll,
        Gesture::BarLift,
        Gesture::Hold { ms: 500 },
    ]
}

/// Slow, idiomatic E9 movement sequence — tertian harmony, 5-string voicings,
/// standard copedant combinations. ~90 seconds at 72 BPM.
///
/// Chord map (all verified against E9 MIDI open_midi):
///   Fret 0, no pedals → E major   (str 3-6, 8)
///   Fret 0, B+C       → F# minor  (str 1, 3-6)
///   Fret 0, A+B       → A major   (str 3-6, 10)
///   Fret 0, A+LKL     → C# major  (str 3-6, 8)
///   Fret 5, no pedals → A(add9)   (str 3-7)
///   Fret 5, A         → F# minor  (str 3-6, 10)
///   Fret 5, B+C       → B minor   (str 1, 3-6)
///   Fret 7, no pedals → B major   (str 3-6, 8)
///   Fret 7, A+B       → E major   (str 3-6, 10)
///   Fret 7, A         → G# minor  (str 3-6, 10)
///   Fret 7, LKL       → C dim     (str 3-6, 8)
///   Fret 9, no pedals → Db(add9)  (str 3-7)
///   Fret 9, A+B       → F# major  (str 3-6, 10)
///   Fret 9, B+C       → Eb minor  (str 1, 3-6)
fn e9_moves_sequence() -> Vec<Gesture> {
    let beat = 833_u32; // 72 BPM: 60_000 / 72 ≈ 833 ms
    let sq   = beat * 2; // 2-beat slow squeeze

    // String indices (0-based): str3=2, str4=3, str5=4, str6=5, str7=6, str8=7, str10=9
    // Pedal indices: A=0, B=1, C=2
    // Lever indices: LKL=0, LKV=1, LKR=2, RKL=3, RKR=4

    vec![
        // ── Intro: place bar at fret 0, volume swell ──
        Gesture::Hold { ms: 400 },
        Gesture::BarPlace { fret: 0.0 },
        Gesture::PickStrings { strings: vec![2, 3, 4, 5, 7] }, // E major: str3-6,8
        Gesture::VolumeSwell { from: 0.0, to: 0.83, ms: beat + beat / 2 },
        Gesture::Hold { ms: beat },

        // ── Section A: fret 0 ──

        // E major open (2 beats)
        Gesture::Hold { ms: beat * 2 },

        // → F# minor (B+C quick engage — chord changes when both down)
        Gesture::PickStrings { strings: vec![0, 2, 3, 4, 5] }, // str1,3-6
        Gesture::PedalEngage { index: 1, ms: 120 }, // B
        Gesture::PedalEngage { index: 2, ms: 120 }, // C
        Gesture::Hold { ms: beat * 3 },
        Gesture::PedalRelease { index: 2, ms: 120 },
        Gesture::PedalRelease { index: 1, ms: 120 },
        Gesture::Hold { ms: beat },

        // → A major (A+B quick engage)
        Gesture::PickStrings { strings: vec![2, 3, 4, 5, 9] }, // str3-6,10
        Gesture::PedalEngage { index: 0, ms: 120 }, // A
        Gesture::PedalEngage { index: 1, ms: 120 }, // B
        Gesture::Hold { ms: beat * 3 },
        Gesture::PedalRelease { index: 1, ms: 120 },
        Gesture::PedalRelease { index: 0, ms: 120 },
        Gesture::Hold { ms: beat },

        // → C# major (A slow squeeze + LKL slow squeeze)
        Gesture::PickStrings { strings: vec![2, 3, 4, 5, 7] }, // str3-6,8
        Gesture::PedalEngage { index: 0, ms: sq },   // A: 2-beat squeeze
        Gesture::LeverEngage { index: 0, ms: sq },   // LKL: 2-beat squeeze (sequential)
        Gesture::Hold { ms: beat * 2 },
        Gesture::LeverRelease { index: 0, ms: beat },
        Gesture::PedalRelease { index: 0, ms: beat },
        Gesture::Hold { ms: beat },

        // ── Section B: slide to fret 5 ──
        Gesture::BarSlide { to: 5.0, ms: beat + beat / 2 },
        Gesture::PickStrings { strings: vec![2, 3, 4, 5, 6] }, // A(add9): str3-7
        Gesture::Hold { ms: beat * 2 },

        // → F# minor (A slow squeeze + slow release)
        Gesture::PickStrings { strings: vec![2, 3, 4, 5, 9] }, // str3-6,10
        Gesture::PedalEngage { index: 0, ms: sq },
        Gesture::Hold { ms: beat * 2 },
        Gesture::PedalRelease { index: 0, ms: sq },
        Gesture::PickStrings { strings: vec![2, 3, 4, 5, 6] }, // back to A(add9)
        Gesture::Hold { ms: beat },

        // → B minor (B+C quick engage)
        Gesture::PickStrings { strings: vec![0, 2, 3, 4, 5] }, // str1,3-6
        Gesture::PedalEngage { index: 1, ms: 120 },
        Gesture::PedalEngage { index: 2, ms: 120 },
        Gesture::Hold { ms: beat * 3 },
        Gesture::PedalRelease { index: 2, ms: 120 },
        Gesture::PedalRelease { index: 1, ms: 120 },
        Gesture::Hold { ms: beat },

        // ── Section C: slide to fret 7 ──
        Gesture::BarSlide { to: 7.0, ms: beat + beat / 2 },
        Gesture::PickStrings { strings: vec![2, 3, 4, 5, 7] }, // B major: str3-6,8
        Gesture::Hold { ms: beat * 2 },

        // → E major at fret 7 (A+B quick engage)
        Gesture::PickStrings { strings: vec![2, 3, 4, 5, 9] }, // str3-6,10
        Gesture::PedalEngage { index: 0, ms: 120 },
        Gesture::PedalEngage { index: 1, ms: 120 },
        Gesture::Hold { ms: beat * 3 },
        Gesture::PedalRelease { index: 1, ms: 120 },
        Gesture::PedalRelease { index: 0, ms: 120 },
        Gesture::Hold { ms: beat },

        // → G# minor (A slow squeeze + slow release)
        Gesture::PedalEngage { index: 0, ms: sq },
        Gesture::Hold { ms: beat * 2 },
        Gesture::PedalRelease { index: 0, ms: sq },
        Gesture::PickStrings { strings: vec![2, 3, 4, 5, 7] }, // back to B major grip
        Gesture::Hold { ms: beat },

        // → C dim (LKL slow squeeze — brief passing chord)
        Gesture::LeverEngage { index: 0, ms: sq },
        Gesture::Hold { ms: beat + beat / 2 },
        Gesture::LeverRelease { index: 0, ms: beat },

        // ── Section D: slide to fret 9 ──
        Gesture::BarSlide { to: 9.0, ms: beat * 2 },
        Gesture::PickStrings { strings: vec![2, 3, 4, 5, 6] }, // Db(add9): str3-7
        Gesture::Hold { ms: beat * 2 },

        // → F# major (A+B quick engage)
        Gesture::PickStrings { strings: vec![2, 3, 4, 5, 9] }, // str3-6,10
        Gesture::PedalEngage { index: 0, ms: 120 },
        Gesture::PedalEngage { index: 1, ms: 120 },
        Gesture::Hold { ms: beat * 3 },
        Gesture::PedalRelease { index: 1, ms: 120 },
        Gesture::PedalRelease { index: 0, ms: 120 },
        Gesture::PickStrings { strings: vec![2, 3, 4, 5, 6] }, // Db(add9)
        Gesture::Hold { ms: beat * 2 },

        // → Eb minor (B+C quick engage)
        Gesture::PickStrings { strings: vec![0, 2, 3, 4, 5] }, // str1,3-6
        Gesture::PedalEngage { index: 1, ms: 120 },
        Gesture::PedalEngage { index: 2, ms: 120 },
        Gesture::Hold { ms: beat * 3 },
        Gesture::PedalRelease { index: 2, ms: 120 },
        Gesture::PedalRelease { index: 1, ms: 120 },
        Gesture::Hold { ms: beat },

        // ── Outro: slide back to fret 0, E major, fade ──
        Gesture::BarSlide { to: 0.0, ms: beat * 2 },
        Gesture::PickStrings { strings: vec![2, 3, 4, 5, 7] }, // E major: str3-6,8
        Gesture::Hold { ms: beat * 2 },
        Gesture::BarVibrato { width: 0.10, rate_hz: 5.2, ms: beat * 4 },
        Gesture::VolumeSwell { from: 0.83, to: 0.0, ms: beat * 5 },
        Gesture::MuteAll,
        Gesture::BarLift,
        Gesture::Hold { ms: beat },
    ]
}

// ─── Xorshift64 PRNG (no external crates) ────────────────────────────────────

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self { Rng(seed.max(1)) }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform in [lo, hi] inclusive.
    fn range(&mut self, lo: u32, hi: u32) -> u32 {
        lo + (self.next() % (hi - lo + 1) as u64) as u32
    }
    /// True with probability 1/n.
    fn one_in(&mut self, n: u64) -> bool { self.next() % n == 0 }
}

// ─── E9 chord vocabulary (shared by improv + future uses) ────────────────────

struct ChordV {
    label: &'static str,
    fret:    f32,
    ped:     [bool; 3],   // A, B, C
    lev:     [bool; 5],   // LKL, LKV, LKR, RKL, RKR
    strings: &'static [usize],
    passing: bool,        // dim / transitional — shorter holds
}

static CHORD_VOCAB: &[ChordV] = &[
    // Fret 0 ──────────────────────────────────────────────────────────────────
    ChordV { label:"E",    fret:0., ped:[false,false,false], lev:[false,false,false,false,false], strings:&[2,3,4,5,7], passing:false },
    ChordV { label:"F#m",  fret:0., ped:[false,true, true ], lev:[false,false,false,false,false], strings:&[0,2,3,4,5], passing:false },
    ChordV { label:"A",    fret:0., ped:[true, true, false], lev:[false,false,false,false,false], strings:&[2,3,4,5,9], passing:false },
    ChordV { label:"C#",   fret:0., ped:[true, false,false], lev:[true, false,false,false,false], strings:&[2,3,4,5,7], passing:false },
    // G#m = open+LKR: E4→Eb4, E3→Eb3 → G#,Eb,B,G#,Eb = G# minor
    ChordV { label:"G#m",  fret:0., ped:[false,false,false], lev:[false,false,true, false,false], strings:&[2,3,4,5,7], passing:false },
    // Db7 = open+LKL: E4→F4, E3→F3 → G#,F,B,G#,F = rootless Db dominant 7th
    ChordV { label:"Db7",  fret:0., ped:[false,false,false], lev:[true, false,false,false,false], strings:&[2,3,4,5,7], passing:false },
    // Fret 5 ──────────────────────────────────────────────────────────────────
    ChordV { label:"A9",   fret:5., ped:[false,false,false], lev:[false,false,false,false,false], strings:&[2,3,4,5,6], passing:false },
    ChordV { label:"F#m5", fret:5., ped:[true, false,false], lev:[false,false,false,false,false], strings:&[2,3,4,5,9], passing:false },
    ChordV { label:"Bm",   fret:5., ped:[false,true, true ], lev:[false,false,false,false,false], strings:&[0,2,3,4,5], passing:false },
    // Fret 7 ──────────────────────────────────────────────────────────────────
    ChordV { label:"B",    fret:7., ped:[false,false,false], lev:[false,false,false,false,false], strings:&[2,3,4,5,7], passing:false },
    ChordV { label:"E@7",  fret:7., ped:[true, true, false], lev:[false,false,false,false,false], strings:&[2,3,4,5,9], passing:false },
    ChordV { label:"G#m",  fret:7., ped:[true, false,false], lev:[false,false,false,false,false], strings:&[2,3,4,5,9], passing:false },
    // Ab7 = fret7+LKL: chart-labeled Ab7 (rootless dominant 7th, not a true diminished)
    ChordV { label:"Ab7",  fret:7., ped:[false,false,false], lev:[true, false,false,false,false], strings:&[2,3,4,5,7], passing:false },
    // Ebm7 = fret7+LKR: F#4→C#5,Eb5,Bb4,F#4,Eb4 = Db,Eb,Gb,Bb = Eb minor 7th
    ChordV { label:"Ebm7", fret:7., ped:[false,false,false], lev:[false,false,true, false,false], strings:&[0,2,3,4,5], passing:false },
    // Fret 9 ──────────────────────────────────────────────────────────────────
    ChordV { label:"Db9",  fret:9., ped:[false,false,false], lev:[false,false,false,false,false], strings:&[2,3,4,5,6], passing:false },
    ChordV { label:"F#9",  fret:9., ped:[true, true, false], lev:[false,false,false,false,false], strings:&[2,3,4,5,9], passing:false },
    ChordV { label:"Ebm",  fret:9., ped:[false,true, true ], lev:[false,false,false,false,false], strings:&[0,2,3,4,5], passing:false },
];

/// Pick next chord index: weighted by fret proximity, never same chord twice.
fn pick_next_chord(cur: usize, rng: &mut Rng) -> usize {
    let cur_fret = CHORD_VOCAB[cur].fret;
    let weights: Vec<u32> = CHORD_VOCAB.iter().enumerate().map(|(i, v)| {
        if i == cur { return 0; }
        let d = (v.fret - cur_fret).abs();
        if d < 0.1 { 3 } else if d <= 4.0 { 2 } else { 1 }
    }).collect();
    let total: u32 = weights.iter().sum();
    let mut pick = (rng.next() % total as u64) as u32;
    for (i, &w) in weights.iter().enumerate() {
        if pick < w { return i; }
        pick -= w;
    }
    0
}

/// Append release + slide + engage gestures for transitioning old → new chord.
fn chord_transition(g: &mut Vec<Gesture>, old: &ChordV, new: &ChordV,
                    _rng: &mut Rng, beat: u32) {
    let sq = beat * 2;

    // Release current pedals / levers (quick)
    for i in 0..3 { if old.ped[i] { g.push(Gesture::PedalRelease { index: i, ms: 150 }); } }
    for i in 0..5 { if old.lev[i] { g.push(Gesture::LeverRelease { index: i, ms: 150 }); } }

    // Bar slide (speed proportional to fret distance)
    let dist = (new.fret - old.fret).abs();
    if dist > 0.01 {
        let slide_ms = (dist * 180.0) as u32 + 200;
        g.push(Gesture::BarSlide { to: new.fret, ms: slide_ms });
    }

    g.push(Gesture::PickStrings { strings: new.strings.to_vec() });

    // Engage new pedals / levers.
    // Slow (2-beat squeeze) for solo pedal or solo lever; quick for pairs.
    let n_ped = new.ped.iter().filter(|&&x| x).count();
    let n_lev = new.lev.iter().filter(|&&x| x).count();
    let slow_ped = n_ped == 1 && n_lev == 0;
    let slow_lev = n_lev >= 1 && n_ped == 0;
    for i in 0..3 {
        if new.ped[i] { g.push(Gesture::PedalEngage { index: i, ms: if slow_ped { sq } else { 120 } }); }
    }
    for i in 0..5 {
        if new.lev[i] { g.push(Gesture::LeverEngage { index: i, ms: if slow_lev { sq } else { 120 } }); }
    }

}

/// Algorithmic improvisation — weighted random walk over the E9 chord vocabulary.
///
/// Generates `total_beats` beats of musically coherent gestures at 72 BPM.
/// Volume pedal is active throughout: duck-and-swell attacks, phrase arcs, breathing.
/// Pass different `seed` values for different performances.
fn improvise_sequence(seed: u64, total_beats: u32) -> Vec<Gesture> {
    let beat = 833_u32;   // 72 BPM
    let mut rng = Rng::new(seed);
    let mut g = Vec::<Gesture>::new();

    // Intro: silence → place bar → pick → swell in
    g.push(Gesture::Hold { ms: 300 });
    g.push(Gesture::BarPlace { fret: 0.0 });
    g.push(Gesture::PickStrings { strings: CHORD_VOCAB[0].strings.to_vec() });
    g.push(Gesture::VolumeSwell { from: 0.0, to: 0.82, ms: beat + beat / 2 });
    g.push(Gesture::Hold { ms: beat * 2 });

    let mut cur_vol: f32 = 0.82;    // volume pedal position we last set
    let mut elapsed_beats: u32 = 5;
    let mut cur_idx: usize = 0;
    let end_beats = total_beats.saturating_sub(12);

    // Phrase tracking: arc vol up to a peak then trail off at phrase end
    let mut phrase_beats_left: u32 = rng.range(8, 13);
    let mut phrase_peak: f32 = 0.82 + rng.range(0, 8) as f32 * 0.01;

    while elapsed_beats < end_beats {
        // ── Phrase boundary: trail off, reset peak ──────────────────────
        if phrase_beats_left == 0 {
            let trail = 0.50 + rng.range(0, 18) as f32 * 0.01; // 0.50–0.67
            if cur_vol > trail + 0.04 {
                g.push(Gesture::VolumeSwell { from: cur_vol, to: trail, ms: beat });
                cur_vol = trail;
                elapsed_beats += 1;
            }
            phrase_peak = 0.78 + rng.range(0, 10) as f32 * 0.01; // 0.78–0.87
            phrase_beats_left = rng.range(8, 13);
        }

        let next_idx = pick_next_chord(cur_idx, &mut rng);
        let cur  = &CHORD_VOCAB[cur_idx];
        let next = &CHORD_VOCAB[next_idx];

        // ── Duck-and-swell attack (~70% of transitions) ──────────────────
        // The most idiomatic steel move: dip vol before the chord "speaks",
        // then swell up as it blooms. Absent on fast passing-chord changes.
        let do_duck = !next.passing && !rng.one_in(3);
        if do_duck && cur_vol > 0.25 {
            let duck_to  = 0.06 + rng.range(0, 12) as f32 * 0.01; // 0.06–0.17
            let duck_ms  = beat / 3 + rng.range(0, beat / 5);
            g.push(Gesture::VolumeSwell { from: cur_vol, to: duck_to, ms: duck_ms });
            cur_vol = duck_to;
        }

        chord_transition(&mut g, cur, next, &mut rng, beat);

        // Swell up to near phrase peak (slight randomness per chord)
        let target = (phrase_peak + rng.range(0, 8) as f32 * 0.01 - 0.04)
            .clamp(0.60, 0.92);
        let swell_ms = beat / 2 + rng.range(0, beat / 2);
        if cur_vol < target - 0.03 {
            g.push(Gesture::VolumeSwell { from: cur_vol, to: target, ms: swell_ms });
            cur_vol = target;
        }

        // ── Hold ──────────────────────────────────────────────────────────
        let hold_beats = if next.passing {
            rng.range(1, 2)
        } else {
            rng.range(2, 5)
        };
        g.push(Gesture::Hold { ms: beat * hold_beats });

        // ── Mid-hold breathing (≥3-beat holds, 40% chance) ───────────────
        // Gentle dip and return — the pedal never truly rests.
        if !next.passing && hold_beats >= 3 && rng.one_in(2) {
            let dip_frac = 0.62 + rng.range(0, 15) as f32 * 0.01; // 0.62–0.77
            let dip_to   = cur_vol * dip_frac;
            let dip_ms   = beat * 2 / 3;
            g.push(Gesture::VolumeSwell { from: cur_vol, to: dip_to, ms: dip_ms });
            g.push(Gesture::Hold { ms: beat / 3 });
            g.push(Gesture::VolumeSwell { from: dip_to, to: cur_vol, ms: dip_ms });
        }

        // ── Occasional peak swell mid-hold (expressive push, ~1 in 6) ────
        if !next.passing && hold_beats >= 3 && rng.one_in(6) {
            let push_to = (cur_vol + 0.08).clamp(0.0, 0.95);
            g.push(Gesture::VolumeSwell { from: cur_vol, to: push_to, ms: beat });
            g.push(Gesture::VolumeSwell { from: push_to, to: cur_vol, ms: beat });
        }

        // ── Vibrato (25% chance, non-passing only) ───────────────────────
        if !next.passing && rng.one_in(4) {
            let width = 0.08 + rng.range(0, 7) as f32 * 0.01;
            g.push(Gesture::BarVibrato { width, rate_hz: 5.2, ms: beat * 2 });
            elapsed_beats += 2;
        }

        elapsed_beats += hold_beats + 2;
        phrase_beats_left = phrase_beats_left.saturating_sub(hold_beats + 2);
        cur_idx = next_idx;
        info!("  improv: {} → {} (hold {} beats, vol {:.2})", cur.label, next.label, hold_beats, cur_vol);
    }

    // Outro: resolve to E major, long fade
    chord_transition(&mut g, &CHORD_VOCAB[cur_idx], &CHORD_VOCAB[0], &mut rng, beat);
    g.push(Gesture::Hold { ms: beat * 2 });
    g.push(Gesture::BarVibrato { width: 0.10, rate_hz: 5.2, ms: beat * 4 });
    g.push(Gesture::VolumeSwell { from: cur_vol, to: 0.0, ms: beat * 5 });
    g.push(Gesture::MuteAll);
    g.push(Gesture::BarLift);
    g.push(Gesture::Hold { ms: beat });
    g
}

// ─── Math helpers ───────────────────────────────────────────────────────────

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Smooth interpolation (ease in/out)
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
