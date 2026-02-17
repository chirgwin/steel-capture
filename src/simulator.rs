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

    /// Run a demo sequence that exercises all gesture types.
    /// Blocks the calling thread.
    pub fn run(&mut self) {
        info!("Simulator starting demo sequence...");
        let mut state = SimState::default();
        let tick_us = 1_000_000 / self.sensor_rate_hz as u64;

        // Build the demo sequence
        let gestures = demo_sequence();

        for gesture in &gestures {
            self.execute(gesture, &mut state, tick_us);
        }

        info!("Demo sequence complete. Holding final state...");
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

// ─── Math helpers ───────────────────────────────────────────────────────────

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Smooth interpolation (ease in/out)
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
