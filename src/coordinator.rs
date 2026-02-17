use crate::bar_inference::BarInference;
use crate::copedant::CopedantEngine;
use crate::string_detector::StringDetector;
use crate::types::*;
use crossbeam_channel::{Receiver, Sender};
use log::{debug, info, trace};

/// The coordinator receives InputEvents (sensor frames and audio chunks),
/// runs bar inference, string detection, and copedant computation, and
/// produces unified CaptureFrames for downstream consumers.
///
/// # String detection modes
///
/// In **simulator mode**, the simulator provides ground-truth `string_active`
/// via the SensorFrame. The StringDetector also runs on the synthetic audio
/// for validation, but the simulator's ground truth is authoritative.
///
/// In **hardware mode**, `SensorFrame.string_active` is all-false (the Teensy
/// doesn't know which strings are picked). The StringDetector analyzes real
/// audio to determine which strings are sounding.
///
/// The `use_audio_detection` flag controls which source is used for the
/// output CaptureFrame. When true, audio detection overrides sensor data.
pub struct Coordinator {
    input_rx: Receiver<InputEvent>,
    frame_txs: Vec<Sender<CaptureFrame>>,
    audio_log_tx: Option<Sender<AudioChunk>>,
    engine: CopedantEngine,
    inference: BarInference,
    string_detector: StringDetector,
    /// If true, use audio-based string detection instead of sensor.string_active.
    /// Defaults to false (simulator ground truth). Set true for hardware mode.
    pub use_audio_detection: bool,
}

impl Coordinator {
    pub fn new(
        input_rx: Receiver<InputEvent>,
        frame_txs: Vec<Sender<CaptureFrame>>,
        audio_log_tx: Option<Sender<AudioChunk>>,
        copedant: Copedant,
    ) -> Self {
        Self {
            input_rx,
            frame_txs,
            audio_log_tx,
            engine: CopedantEngine::new(copedant),
            inference: BarInference::new(),
            string_detector: StringDetector::new(),
            use_audio_detection: false,
        }
    }

    /// Enable audio-based string detection (for hardware mode).
    pub fn with_audio_detection(mut self, enabled: bool) -> Self {
        self.use_audio_detection = enabled;
        self
    }

    pub fn run(&mut self) {
        info!("Coordinator running (audio string detection: {})",
              if self.use_audio_detection { "ON" } else { "OFF (simulator ground truth)" });

        let mut prev_active = [false; 10];
        let mut prev_pedal_engaged = [false; 3];
        let mut prev_lever_engaged = [false; 5];
        let mut frame_count: u64 = 0;

        // Which strings are affected by each pedal/lever (from copedant)
        let pedal_strings = pedal_string_map();
        let lever_strings = lever_string_map();

        for event in self.input_rx.iter() {
            match event {
                InputEvent::Sensor(sensor) => {
                    

                    let bar_state = self.inference.infer(&sensor, &self.engine);
                    let pitches = self.engine.compute_pitches(
                        &sensor,
                        bar_state.position,
                    );

                    // === STRING DETECTION ===
                    // Determine which strings are active and detect attacks.
                    let (string_active, audio_attacks) = if self.use_audio_detection {
                        // Hardware mode: use audio-based detection
                        self.string_detector.detect(
                            &sensor,
                            bar_state.position,
                            &self.engine,
                        )
                    } else {
                        // Simulator mode: use ground truth from sensor frame.
                        // Still run the detector for diagnostics but don't use its output.
                        let _ = self.string_detector.detect(
                            &sensor,
                            bar_state.position,
                            &self.engine,
                        );
                        (sensor.string_active, [false; 10])
                    };

                    // === ATTACK DETECTION ===
                    // An "attack" = new notehead needed. Triggers:
                    //   1. String pick: inactive → active
                    //   2. Pedal state change: crosses 0.5 threshold while string active
                    //   3. Lever state change: crosses 0.5 threshold while string active
                    let mut attacks = if self.use_audio_detection {
                        // Audio detector already provides attacks
                        audio_attacks
                    } else {
                        // Compute from string_active transitions
                        let mut a = [false; 10];
                        for i in 0..10 {
                            if string_active[i] && !prev_active[i] {
                                a[i] = true;
                            }
                        }
                        a
                    };

                    // 2. Pedal state changes (applies in both modes)
                    let pedal_engaged: [bool; 3] = [
                        sensor.pedals[0] > 0.5,
                        sensor.pedals[1] > 0.5,
                        sensor.pedals[2] > 0.5,
                    ];
                    for j in 0..3 {
                        if pedal_engaged[j] != prev_pedal_engaged[j] {
                            for i in 0..10 {
                                if string_active[i] && pedal_strings[j][i] {
                                    attacks[i] = true;
                                }
                            }
                        }
                    }
                    prev_pedal_engaged = pedal_engaged;

                    // 3. Lever state changes
                    let lever_engaged: [bool; 5] = [
                        sensor.knee_levers[0] > 0.5,
                        sensor.knee_levers[1] > 0.5,
                        sensor.knee_levers[2] > 0.5,
                        sensor.knee_levers[3] > 0.5,
                        sensor.knee_levers[4] > 0.5,
                    ];
                    for j in 0..5 {
                        if lever_engaged[j] != prev_lever_engaged[j] {
                            for i in 0..10 {
                                if string_active[i] && lever_strings[j][i] {
                                    attacks[i] = true;
                                }
                            }
                        }
                    }
                    prev_lever_engaged = lever_engaged;
                    prev_active = string_active;

                    let frame = CaptureFrame {
                        timestamp_us: sensor.timestamp_us,
                        pedals: sensor.pedals,
                        knee_levers: sensor.knee_levers,
                        volume: sensor.volume,
                        bar_sensors: sensor.bar_sensors,
                        bar_position: bar_state.position,
                        bar_confidence: bar_state.confidence,
                        bar_source: bar_state.source,
                        string_pitches_hz: pitches,
                        string_active,
                        attacks,
                    };

                    for tx in &self.frame_txs {
                        let _ = tx.send(frame.clone());
                    }

                    frame_count += 1;
                    if frame_count % 1000 == 0 {
                        debug!("Coordinator: {} frames processed", frame_count);
                        trace!("Latest: {}", frame);
                    }
                }

                InputEvent::Audio(chunk) => {
                    if let Some(ref tx) = self.audio_log_tx {
                        let _ = tx.send(chunk.clone());
                    }
                    // Feed audio to BOTH inference and string detector
                    self.inference.push_audio(&chunk);
                    self.string_detector.push_audio(&chunk);
                }
            }
        }

        info!("Coordinator shutting down after {} frames", frame_count);
    }
}

/// Maps each pedal to the strings it affects (from Buddy Emmons E9 copedant).
fn pedal_string_map() -> [[bool; 10]; 3] {
    [
        // Pedal A: strings 5,10
        [false, false, false, false, true, false, false, false, false, true],
        // Pedal B: strings 3,6
        [false, false, true, false, false, true, false, false, false, false],
        // Pedal C: strings 4,5
        [false, false, false, true, true, false, false, false, false, false],
    ]
}

/// Maps each lever to the strings it affects.
fn lever_string_map() -> [[bool; 10]; 5] {
    [
        // LKL: strings 4,8
        [false, false, false, true, false, false, false, true, false, false],
        // LKR: strings 4,5,8
        [false, false, false, true, true, false, false, true, false, false],
        // LKV: strings 5,10
        [false, false, false, false, true, false, false, false, false, true],
        // RKL: strings 2,6
        [false, true, false, false, false, true, false, false, false, false],
        // RKR: strings 2,9
        [false, true, false, false, false, false, false, false, true, false],
    ]
}
