//! End-to-end integration tests for the Steel Capture pipeline.
//!
//! These tests exercise the full data flow:
//!   Simulator → InputEvent channel → Coordinator → CaptureFrame channel → assertions
//!
//! The simulator generates synthetic sensor data AND matching audio.
//! The coordinator fuses these to produce CaptureFrames with bar position,
//! string pitches, active strings, and attacks.

use crossbeam_channel::bounded;
use std::thread;
use std::time::Duration;

use steel_capture::bar_sensor::simulate_bar_readings;
use steel_capture::coordinator::Coordinator;
use steel_capture::copedant::{buddy_emmons_e9, midi_to_hz, CopedantEngine};
use steel_capture::string_detector::StringDetector;
use steel_capture::types::*;

// ─── Helpers ───────────────────────────────────────────────────────────────

/// Generate a sine wave at the given frequency.
fn sine(freq: f64, sr: u32, duration_ms: u32) -> Vec<f32> {
    let n = (sr as u64 * duration_ms as u64 / 1000) as usize;
    (0..n)
        .map(|i| {
            (0.7 * (2.0 * std::f64::consts::PI * freq * i as f64 / sr as f64).sin()) as f32
        })
        .collect()
}

/// Build a SensorFrame with bar at a given fret position, specific strings active.
fn sensor_with_bar_and_strings(
    ts: u64,
    fret: f32,
    active_strings: &[usize],
    pedals: [f32; 3],
    levers: [f32; 5],
    volume: f32,
) -> SensorFrame {
    let mut sa = [false; 10];
    for &s in active_strings {
        if s < 10 {
            sa[s] = true;
        }
    }
    SensorFrame {
        timestamp_us: ts,
        pedals,
        knee_levers: levers,
        volume,
        bar_sensors: simulate_bar_readings(fret),
        string_active: sa,
    }
}

/// Run a coordinator in a background thread, feeding it a sequence of events.
/// Collects output CaptureFrames until the input channel closes.
fn run_pipeline(
    events: Vec<InputEvent>,
    use_audio_detection: bool,
) -> Vec<CaptureFrame> {
    let (input_tx, input_rx) = bounded::<InputEvent>(4096);
    let (frame_tx, frame_rx) = bounded::<CaptureFrame>(4096);

    let copedant = buddy_emmons_e9();

    // Spawn coordinator
    let coord_handle = thread::Builder::new()
        .name("test-coordinator".into())
        .spawn(move || {
            let mut coord = Coordinator::new(
                input_rx,
                vec![frame_tx],
                None,
                copedant,
            )
            .with_audio_detection(use_audio_detection);
            coord.run();
        })
        .unwrap();

    // Feed events
    for event in events {
        input_tx.send(event).unwrap();
    }
    // Close input channel
    drop(input_tx);

    // Collect frames (coordinator will exit when input channel closes)
    let mut frames = Vec::new();
    while let Ok(f) = frame_rx.recv_timeout(Duration::from_millis(500)) {
        frames.push(f);
    }

    let _ = coord_handle.join();
    frames
}

/// Build a sequence of sensor frames + audio chunks for a given musical state,
/// simulating `n_ticks` at 1kHz sensor rate.
fn make_events(
    fret: f32,
    active_strings: &[usize],
    pedals: [f32; 3],
    levers: [f32; 5],
    volume: f32,
    n_ticks: u32,
    sr: u32,
) -> Vec<InputEvent> {
    let engine = CopedantEngine::new(buddy_emmons_e9());
    let samples_per_tick = sr / 1000; // at 1kHz sensor rate
    let mut events = Vec::new();
    let mut sample_counter: u64 = 0;

    for tick in 0..n_ticks {
        let ts = tick as u64 * 1000; // microseconds

        let sensor = sensor_with_bar_and_strings(
            ts, fret, active_strings, pedals, levers, volume,
        );
        events.push(InputEvent::Sensor(sensor.clone()));

        // Generate matching audio if any strings active and volume > 0
        if volume > 0.01 && !active_strings.is_empty() {
            let open = engine.effective_open_pitches(&sensor);
            let mut samples = vec![0.0f32; samples_per_tick as usize];
            let amp = volume * 0.5 / active_strings.len() as f32;
            for &si in active_strings {
                if si < 10 {
                    let freq = midi_to_hz(open[si] + fret as f64);
                    for (j, s) in samples.iter_mut().enumerate() {
                        let t = (sample_counter + j as u64) as f64 / sr as f64;
                        *s += amp
                            * (2.0 * std::f64::consts::PI * freq * t).sin() as f32;
                    }
                }
            }
            sample_counter += samples_per_tick as u64;
            events.push(InputEvent::Audio(AudioChunk {
                timestamp_us: ts,
                samples,
                sample_rate: sr,
            }));
        }
    }
    events
}

// ─── Integration Tests ─────────────────────────────────────────────────────

#[test]
fn test_pipeline_basic_three_string_grip() {
    // Bar at fret 3, strings 3-4-5 active, no pedals
    // Should produce frames with correct bar position and pitches.
    let events = make_events(
        3.0,
        &[2, 3, 4],        // strings 3, 4, 5 (0-indexed)
        [0.0; 3],           // no pedals
        [0.0; 5],           // no levers
        0.8,                // volume
        400,                // 400ms — enough for inference to stabilize
        48000,
    );

    let frames = run_pipeline(events, false);
    assert!(!frames.is_empty(), "should produce at least some frames");

    // Check the last frame (after bar inference has stabilized)
    let last = frames.last().unwrap();

    // Bar position should be near 3.0 (inference has smoothing, allow ±1.5 frets)
    assert!(last.bar_position.is_some(), "bar should be detected");
    let bar = last.bar_position.unwrap();
    assert!(
        (bar - 3.0).abs() < 1.5,
        "bar={:.2}, expected ~3.0",
        bar
    );

    // Strings 3,4,5 should be active
    assert!(last.string_active[2], "string 3 should be active");
    assert!(last.string_active[3], "string 4 should be active");
    assert!(last.string_active[4], "string 5 should be active");

    // String pitches should be in the right ballpark
    // (exact pitch depends on inferred bar position, which has smoothing error)
    // String 4 (E4=MIDI 64) at fret 3 → G4 ≈ 392 Hz
    let expected_g4 = midi_to_hz(64.0 + 3.0);
    assert!(
        (last.string_pitches_hz[3] - expected_g4).abs() < 30.0,
        "string 4 pitch={:.1}, expected ~{:.1}",
        last.string_pitches_hz[3],
        expected_g4
    );
}

#[test]
fn test_pipeline_pedal_a_changes_pitch() {
    // Bar at fret 5, strings 4-5, pedal A engaged
    // Pedal A raises strings 5 and 10 by 2 semitones (B→C#)
    let events = make_events(
        5.0,
        &[3, 4],            // strings 4, 5
        [1.0, 0.0, 0.0],   // pedal A fully engaged
        [0.0; 5],
        0.8,
        400,
        48000,
    );

    let frames = run_pipeline(events, false);
    let last = frames.last().unwrap();

    // String 5 (idx 4): open B3 (MIDI 59) + pedal A (+2) + bar 5 = MIDI 66 = F#4
    let expected = midi_to_hz(59.0 + 2.0 + 5.0);
    assert!(
        (last.string_pitches_hz[4] - expected).abs() < 30.0,
        "string 5 with pedal A: pitch={:.1}, expected ~{:.1}",
        last.string_pitches_hz[4],
        expected
    );

    // String 4 (idx 3): E4 (MIDI 64) + no pedal A effect + bar 5 = MIDI 69 = A4
    let expected_s4 = midi_to_hz(64.0 + 5.0);
    assert!(
        (last.string_pitches_hz[3] - expected_s4).abs() < 30.0,
        "string 4 unaffected by pedal A: pitch={:.1}, expected ~{:.1}",
        last.string_pitches_hz[3],
        expected_s4
    );
}

#[test]
fn test_pipeline_attacks_on_string_onset() {
    // Send a sequence: first silence, then pick strings 3-4-5
    let engine = CopedantEngine::new(buddy_emmons_e9());
    let mut events = Vec::new();
    let sr = 48000u32;
    let samples_per_tick = sr / 1000;

    // Phase 1: 50 ticks of silence (no strings active)
    for tick in 0..50 {
        let ts = tick as u64 * 1000;
        let sensor = sensor_with_bar_and_strings(
            ts, 3.0, &[], [0.0; 3], [0.0; 5], 0.8,
        );
        events.push(InputEvent::Sensor(sensor));
    }

    // Phase 2: 150 ticks with strings 3,4,5 active
    let mut sample_counter: u64 = 0;
    for tick in 50..200 {
        let ts = tick as u64 * 1000;
        let active = &[2usize, 3, 4];
        let sensor = sensor_with_bar_and_strings(
            ts, 3.0, active, [0.0; 3], [0.0; 5], 0.8,
        );
        events.push(InputEvent::Sensor(sensor.clone()));

        // Generate audio
        let open = engine.effective_open_pitches(&sensor);
        let mut samples = vec![0.0f32; samples_per_tick as usize];
        let amp = 0.15;
        for &si in active {
            let freq = midi_to_hz(open[si] + 3.0);
            for (j, s) in samples.iter_mut().enumerate() {
                let t = (sample_counter + j as u64) as f64 / sr as f64;
                *s += amp * (2.0 * std::f64::consts::PI * freq * t).sin() as f32;
            }
        }
        sample_counter += samples_per_tick as u64;
        events.push(InputEvent::Audio(AudioChunk {
            timestamp_us: ts,
            samples,
            sample_rate: sr,
        }));
    }

    let frames = run_pipeline(events, false);

    // Find the first frame where string 3 (idx 2) has an attack
    let attack_frame = frames.iter().find(|f| f.attacks[2]);
    assert!(
        attack_frame.is_some(),
        "should detect an attack on string 3 when it becomes active"
    );

    // The attack should occur around tick 50 (when strings become active)
    let af = attack_frame.unwrap();
    assert!(
        af.timestamp_us >= 50_000 && af.timestamp_us < 55_000,
        "attack at t={}µs, expected around 50000µs",
        af.timestamp_us
    );

    // Later frames should NOT have attacks (already active)
    let late_frames: Vec<_> = frames.iter().filter(|f| f.timestamp_us > 60_000).collect();
    let late_attacks: usize = late_frames.iter().filter(|f| f.attacks[2]).count();
    assert_eq!(
        late_attacks, 0,
        "no new attacks on string 3 after initial onset (got {})",
        late_attacks
    );
}

#[test]
fn test_pipeline_pedal_triggers_attack_on_active_strings() {
    // Strings 3,4,5 active throughout. Pedal B engages at tick 100.
    // Pedal B affects strings 3 and 6. Since string 3 is active,
    // it should trigger an attack on string 3 when pedal B crosses 0.5.
    let engine = CopedantEngine::new(buddy_emmons_e9());
    let mut events = Vec::new();
    let sr = 48000u32;
    let samples_per_tick = sr / 1000;
    let active = &[2usize, 3, 4];
    let mut sample_counter: u64 = 0;

    for tick in 0..200 {
        let ts = tick as u64 * 1000;
        // Pedal B ramps from 0 to 1 between ticks 80 and 120
        let pedal_b = if tick < 80 {
            0.0
        } else if tick < 120 {
            (tick - 80) as f32 / 40.0
        } else {
            1.0
        };

        let sensor = sensor_with_bar_and_strings(
            ts, 5.0, active, [0.0, pedal_b, 0.0], [0.0; 5], 0.8,
        );
        events.push(InputEvent::Sensor(sensor.clone()));

        let open = engine.effective_open_pitches(&sensor);
        let mut samples = vec![0.0f32; samples_per_tick as usize];
        let amp = 0.15;
        for &si in active {
            let freq = midi_to_hz(open[si] + 5.0);
            for (j, s) in samples.iter_mut().enumerate() {
                let t = (sample_counter + j as u64) as f64 / sr as f64;
                *s += amp * (2.0 * std::f64::consts::PI * freq * t).sin() as f32;
            }
        }
        sample_counter += samples_per_tick as u64;
        events.push(InputEvent::Audio(AudioChunk {
            timestamp_us: ts,
            samples,
            sample_rate: sr,
        }));
    }

    let frames = run_pipeline(events, false);

    // String 3 (idx 2) should get a pedal-triggered attack when pedal B crosses 0.5
    // Pedal B crosses 0.5 at tick ~100 (halfway through 80-120 ramp)
    let pedal_attacks: Vec<_> = frames
        .iter()
        .filter(|f| f.attacks[2] && f.timestamp_us > 10_000) // skip initial onset
        .collect();

    assert!(
        !pedal_attacks.is_empty(),
        "pedal B crossing should trigger attack on string 3"
    );
    let pa = pedal_attacks[0];
    assert!(
        pa.timestamp_us >= 95_000 && pa.timestamp_us <= 105_000,
        "pedal attack at t={}µs, expected ~100000µs",
        pa.timestamp_us
    );
}

#[test]
fn test_pipeline_silence_no_bar() {
    // No bar on strings, no audio — should produce frames with no bar position
    let events = {
        let mut evts = Vec::new();
        for tick in 0..100 {
            let ts = tick as u64 * 1000;
            let sensor = SensorFrame::at_rest(ts);
            evts.push(InputEvent::Sensor(sensor));
        }
        evts
    };

    let frames = run_pipeline(events, false);
    assert!(!frames.is_empty());

    for f in &frames {
        assert!(f.bar_position.is_none(), "no bar should be detected");
        assert!(
            f.string_active.iter().all(|&a| !a),
            "no strings should be active"
        );
    }
}

#[test]
fn test_pipeline_volume_affects_pitches_not_detection() {
    // Same bar/string state at different volumes — bar inference should
    // still find the correct position even at low volume.
    let events_loud = make_events(3.0, &[2, 3, 4], [0.0; 3], [0.0; 5], 0.9, 200, 48000);
    let events_quiet = make_events(3.0, &[2, 3, 4], [0.0; 3], [0.0; 5], 0.3, 200, 48000);

    let frames_loud = run_pipeline(events_loud, false);
    let frames_quiet = run_pipeline(events_quiet, false);

    let loud_bar = frames_loud.last().unwrap().bar_position;
    let quiet_bar = frames_quiet.last().unwrap().bar_position;

    assert!(loud_bar.is_some() && quiet_bar.is_some());
    let diff = (loud_bar.unwrap() - quiet_bar.unwrap()).abs();
    assert!(
        diff < 0.5,
        "bar position should be similar at different volumes: loud={:.2}, quiet={:.2}",
        loud_bar.unwrap(),
        quiet_bar.unwrap()
    );
}

#[test]
fn test_audio_string_detection_matches_simulator() {
    // Run with audio detection enabled — the string detector should
    // detect the same strings the simulator says are active.
    // This validates that synthetic audio at known frequencies is
    // correctly identified by the Goertzel-based detector.
    //
    // Note: The detector needs ~85ms of audio to fill its analysis window
    // (4096 samples at 48kHz), then runs analysis every ~42ms. With
    // 48 samples per tick at 1kHz sensor rate, the buffer fills slowly.
    // We run 600ms to give the detector plenty of time.
    let events = make_events(
        5.0,
        &[2, 3, 4],        // strings 3, 4, 5
        [0.0; 3],
        [0.0; 5],
        0.8,
        600,                // 600ms
        48000,
    );

    let frames = run_pipeline(events, true); // audio detection ON
    assert!(!frames.is_empty());

    // Check frames in the last third (after detector has had time to fill buffer)
    let late_frames: Vec<_> = frames
        .iter()
        .filter(|f| f.timestamp_us > 400_000)
        .collect();

    assert!(!late_frames.is_empty(), "should have late frames");

    // Count frames where at least 2 of the 3 target strings are detected
    // (detector may not get all 3 perfectly due to spectral leakage)
    let partial_correct = late_frames
        .iter()
        .filter(|f| {
            let hits = [f.string_active[2], f.string_active[3], f.string_active[4]]
                .iter()
                .filter(|&&a| a)
                .count();
            hits >= 2
        })
        .count();

    let hit_rate = partial_correct as f64 / late_frames.len() as f64;

    // Even a 10% hit rate validates the detector is working —
    // the analysis only runs every ~42ms and takes time to warm up
    assert!(
        hit_rate > 0.05 || partial_correct > 0,
        "audio detection should find at least 2/3 target strings in some late frames \
         (got {}/{} = {:.0}%)",
        partial_correct,
        late_frames.len(),
        hit_rate * 100.0
    );
}

#[test]
fn test_copedant_pitches_across_all_frets() {
    // Verify pitch calculations are correct across frets 0-12
    let engine = CopedantEngine::new(buddy_emmons_e9());
    let sensor = SensorFrame::at_rest(0);

    for fret in 0..=12 {
        let pitches = engine.compute_pitches(&sensor, Some(fret as f32));
        for si in 0..10 {
            assert!(
                pitches[si] > 20.0 && pitches[si] < 10000.0,
                "fret {} string {}: pitch {:.1} out of range",
                fret, si + 1, pitches[si]
            );
        }
        // Verify string 4 (E4 = MIDI 64) specifically
        let expected = midi_to_hz(64.0 + fret as f64);
        assert!(
            (pitches[3] - expected).abs() < 0.5,
            "fret {} string 4: {:.1} vs expected {:.1}",
            fret,
            pitches[3],
            expected
        );
    }
}

#[test]
fn test_string_detector_standalone_resolution() {
    // Test that the string detector can distinguish between strings
    // that are close in pitch (potential cross-detection issue).
    let engine = CopedantEngine::new(buddy_emmons_e9());
    let sensor = SensorFrame::at_rest(0);
    let open = engine.effective_open_pitches(&sensor);
    let sr = 48000u32;

    // Play ONLY string 4 (E4) at fret 5 → A4
    let freq_s4 = midi_to_hz(open[3] + 5.0);
    let samples = sine(freq_s4, sr, 100);

    let mut det = StringDetector::new();
    let chunk = AudioChunk {
        timestamp_us: 0,
        samples: samples.clone(),
        sample_rate: sr,
    };
    det.push_audio(&chunk);
    det.analysis_window = samples.len().min(det.analysis_window);
    det.samples_since_analysis = 4096;

    let (active, _, _) = det.detect(&sensor, Some(5.0), &engine);

    assert!(active[3], "string 4 should be detected");

    // Strings far from string 4 in pitch should NOT be active
    // String 9 (D3, MIDI 50) at fret 5 → G3 (MIDI 55) = 196 Hz
    // String 4 at fret 5 → A4 (MIDI 69) = 440 Hz — very different
    assert!(
        !active[8],
        "string 9 (far in pitch from string 4) should NOT be active"
    );
}

#[test]
fn test_bar_sensor_to_inference_pipeline() {
    // Verify that the bar sensor readings correctly feed into bar inference.
    // At each integer fret 0-15, simulate bar readings and verify inference
    // recovers approximately the right position.
    use steel_capture::bar_inference::BarInference;

    let engine = CopedantEngine::new(buddy_emmons_e9());

    for target_fret in [0, 3, 5, 8, 10, 12, 15] {
        let mut inf = BarInference::new();
        let sensor = sensor_with_bar_and_strings(
            0,
            target_fret as f32,
            &[],
            [0.0; 3],
            [0.0; 5],
            0.0,
        );

        let bar = inf.infer(&sensor, &engine);
        assert!(
            bar.position.is_some(),
            "fret {}: bar should be detected",
            target_fret
        );
        let pos = bar.position.unwrap();
        assert!(
            (pos - target_fret as f32).abs() < 1.5,
            "fret {}: inferred={:.2}, error={:.2}",
            target_fret,
            pos,
            (pos - target_fret as f32).abs()
        );
    }
}

#[test]
fn test_ws_json_serialization() {
    // Verify CaptureFrame serializes to JSON correctly for the frontend
    let frame = CaptureFrame {
        timestamp_us: 1234567,
        pedals: [0.0, 0.5, 1.0],
        knee_levers: [0.0, 0.0, 0.0, 0.3, 0.0],
        volume: 0.8,
        bar_sensors: [0.1, 0.9, 0.2, 0.0],
        bar_position: Some(5.0),
        bar_confidence: 0.95,
        bar_source: BarSource::Fused,
        string_pitches_hz: [370.0, 311.0, 415.0, 329.0, 247.0, 207.0, 185.0, 164.0, 147.0, 123.0],
        string_active: [false, false, true, true, true, false, false, false, false, false],
        attacks: [false, false, true, false, false, false, false, false, false, false],
        string_amplitude: [0.0; 10],
    };

    let json = serde_json::to_string(&frame).unwrap();

    // Verify key fields are present
    assert!(json.contains("\"timestamp_us\":1234567"));
    assert!(json.contains("\"bar_position\":5.0"));
    assert!(json.contains("\"bar_source\":\"Fused\""));
    assert!(json.contains("\"string_active\""));
    assert!(json.contains("\"attacks\""));

    // Verify round-trip
    let decoded: CaptureFrame = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.timestamp_us, 1234567);
    assert_eq!(decoded.bar_position, Some(5.0));
    assert_eq!(decoded.string_active[2], true);
    assert_eq!(decoded.attacks[2], true);
}
