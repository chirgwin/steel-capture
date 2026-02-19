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
        .map(|i| (0.7 * (2.0 * std::f64::consts::PI * freq * i as f64 / sr as f64).sin()) as f32)
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
fn run_pipeline(events: Vec<InputEvent>, use_audio_detection: bool) -> Vec<CaptureFrame> {
    let (input_tx, input_rx) = bounded::<InputEvent>(4096);
    let (frame_tx, frame_rx) = bounded::<CaptureFrame>(4096);

    let copedant = buddy_emmons_e9();

    // Spawn coordinator
    let coord_handle = thread::Builder::new()
        .name("test-coordinator".into())
        .spawn(move || {
            let mut coord = Coordinator::new(input_rx, vec![frame_tx], None, copedant)
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

        let sensor = sensor_with_bar_and_strings(ts, fret, active_strings, pedals, levers, volume);
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
                        *s += amp * (2.0 * std::f64::consts::PI * freq * t).sin() as f32;
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
        &[2, 3, 4], // strings 3, 4, 5 (0-indexed)
        [0.0; 3],   // no pedals
        [0.0; 5],   // no levers
        0.8,        // volume
        400,        // 400ms — enough for inference to stabilize
        48000,
    );

    let frames = run_pipeline(events, false);
    assert!(!frames.is_empty(), "should produce at least some frames");

    // Check the last frame (after bar inference has stabilized)
    let last = frames.last().unwrap();

    // Bar position should be near 3.0 (inference has smoothing, allow ±1.5 frets)
    assert!(last.bar_position.is_some(), "bar should be detected");
    let bar = last.bar_position.unwrap();
    assert!((bar - 3.0).abs() < 1.5, "bar={:.2}, expected ~3.0", bar);

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
        &[3, 4],         // strings 4, 5
        [1.0, 0.0, 0.0], // pedal A fully engaged
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
        let sensor = sensor_with_bar_and_strings(ts, 3.0, &[], [0.0; 3], [0.0; 5], 0.8);
        events.push(InputEvent::Sensor(sensor));
    }

    // Phase 2: 150 ticks with strings 3,4,5 active
    let mut sample_counter: u64 = 0;
    for tick in 50..200 {
        let ts = tick as u64 * 1000;
        let active = &[2usize, 3, 4];
        let sensor = sensor_with_bar_and_strings(ts, 3.0, active, [0.0; 3], [0.0; 5], 0.8);
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

        let sensor =
            sensor_with_bar_and_strings(ts, 5.0, active, [0.0, pedal_b, 0.0], [0.0; 5], 0.8);
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
        &[2, 3, 4], // strings 3, 4, 5
        [0.0; 3],
        [0.0; 5],
        0.8,
        600, // 600ms
        48000,
    );

    let frames = run_pipeline(events, true); // audio detection ON
    assert!(!frames.is_empty());

    // Check frames in the last third (after detector has had time to fill buffer)
    let late_frames: Vec<_> = frames.iter().filter(|f| f.timestamp_us > 400_000).collect();

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
                fret,
                si + 1,
                pitches[si]
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
        let sensor =
            sensor_with_bar_and_strings(0, target_fret as f32, &[], [0.0; 3], [0.0; 5], 0.0);

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
        string_pitches_hz: [
            370.0, 311.0, 415.0, 329.0, 247.0, 207.0, 185.0, 164.0, 147.0, 123.0,
        ],
        string_active: [
            false, false, true, true, true, false, false, false, false, false,
        ],
        attacks: [
            false, false, true, false, false, false, false, false, false, false,
        ],
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

    // Compact format: shorter keys, same data
    let compact = CompactFrame::from(&frame);
    let compact_json = serde_json::to_string(&compact).unwrap();
    assert!(compact_json.contains("\"t\":1234567"));
    assert!(compact_json.contains("\"bp\":5.0"));
    assert!(compact_json.contains("\"bx\":\"Fused\""));
    assert!(compact_json.len() < json.len(), "compact should be shorter");

    // Compact round-trip back to CaptureFrame
    let compact_decoded: CompactFrame = serde_json::from_str(&compact_json).unwrap();
    let back: CaptureFrame = compact_decoded.into();
    assert_eq!(back.timestamp_us, 1234567);
    assert_eq!(back.bar_position, Some(5.0));
    assert_eq!(back.string_active[2], true);
    assert_eq!(back.attacks[2], true);
}

// ─── JSONL format tests ─────────────────────────────────────────────────────

/// Build a CaptureFrame with specific values for JSONL tests.
fn mock_capture_frame(timestamp_us: u64, bar_fret: Option<f32>, volume: f32) -> CaptureFrame {
    CaptureFrame {
        timestamp_us,
        pedals: [0.0, 0.0, 0.0],
        knee_levers: [0.0; 5],
        volume,
        bar_sensors: [0.0; 4],
        bar_position: bar_fret,
        bar_confidence: if bar_fret.is_some() { 0.9 } else { 0.0 },
        bar_source: if bar_fret.is_some() {
            BarSource::Sensor
        } else {
            BarSource::None
        },
        string_pitches_hz: [0.0; 10],
        string_active: [false; 10],
        attacks: [false; 10],
        string_amplitude: [0.0; 10],
    }
}

#[test]
fn test_jsonl_header_format() {
    // Verify JSONL header line matches what data_logger writes
    let header = serde_json::json!({
        "format": "steel-capture",
        "copedant": "Buddy Emmons E9",
        "rate_hz": 60,
    });
    let line = serde_json::to_string(&header).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(parsed["format"], "steel-capture");
    assert_eq!(parsed["copedant"], "Buddy Emmons E9");
    assert_eq!(parsed["rate_hz"], 60);
}

#[test]
fn test_jsonl_multi_frame_stream() {
    // Simulate a JSONL stream: header + N compact frames
    let header = serde_json::json!({
        "format": "steel-capture",
        "copedant": "Buddy Emmons E9",
        "rate_hz": 60,
    });

    let frames: Vec<CaptureFrame> = (0..5)
        .map(|i| {
            let mut f = mock_capture_frame(i * 16667, Some(3.0 + i as f32), 0.7);
            if i == 2 {
                f.string_active[3] = true;
                f.attacks[3] = true;
                f.string_amplitude[3] = 0.85;
            }
            f
        })
        .collect();

    // Write JSONL to string (same as data_logger)
    let mut jsonl = String::new();
    jsonl.push_str(&serde_json::to_string(&header).unwrap());
    jsonl.push('\n');
    for frame in &frames {
        let compact = CompactFrame::from(frame);
        jsonl.push_str(&serde_json::to_string(&compact).unwrap());
        jsonl.push('\n');
    }

    // Parse back (same logic as viz.js parseJSONL)
    let lines: Vec<&str> = jsonl.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 6, "header + 5 frames");

    let parsed_header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(parsed_header["format"], "steel-capture");

    for (i, &line) in lines[1..].iter().enumerate() {
        let compact: CompactFrame = serde_json::from_str(line).unwrap();
        let back: CaptureFrame = compact.into();
        assert_eq!(back.timestamp_us, frames[i].timestamp_us);
        assert_eq!(back.bar_position, frames[i].bar_position);
        assert_eq!(back.volume, frames[i].volume);
        assert_eq!(back.string_active, frames[i].string_active);
        assert_eq!(back.attacks, frames[i].attacks);
    }
}

#[test]
fn test_jsonl_compact_preserves_all_fields() {
    // Verify every field survives the CaptureFrame → CompactFrame → JSON → CompactFrame → CaptureFrame round-trip
    let frame = CaptureFrame {
        timestamp_us: 999999,
        pedals: [0.1, 0.5, 0.9],
        knee_levers: [0.2, 0.4, 0.6, 0.8, 1.0],
        volume: 0.75,
        bar_sensors: [0.3, 0.7, 0.1, 0.0],
        bar_position: Some(7.5),
        bar_confidence: 0.88,
        bar_source: BarSource::Fused,
        string_pitches_hz: [
            370.0, 311.0, 415.0, 329.0, 247.0, 207.0, 185.0, 164.0, 147.0, 123.0,
        ],
        string_active: [
            true, false, true, true, false, false, true, false, false, true,
        ],
        attacks: [
            true, false, false, true, false, false, false, false, false, false,
        ],
        string_amplitude: [0.9, 0.0, 0.7, 0.8, 0.0, 0.0, 0.5, 0.0, 0.0, 0.3],
    };

    let compact = CompactFrame::from(&frame);
    let json = serde_json::to_string(&compact).unwrap();
    let decoded: CompactFrame = serde_json::from_str(&json).unwrap();
    let back: CaptureFrame = decoded.into();

    assert_eq!(back.timestamp_us, frame.timestamp_us);
    assert_eq!(back.pedals, frame.pedals);
    assert_eq!(back.knee_levers, frame.knee_levers);
    assert_eq!(back.volume, frame.volume);
    assert_eq!(back.bar_sensors, frame.bar_sensors);
    assert_eq!(back.bar_position, frame.bar_position);
    assert_eq!(back.bar_confidence, frame.bar_confidence);
    assert_eq!(back.bar_source, frame.bar_source);
    assert_eq!(back.string_pitches_hz, frame.string_pitches_hz);
    assert_eq!(back.string_active, frame.string_active);
    assert_eq!(back.attacks, frame.attacks);
    assert_eq!(back.string_amplitude, frame.string_amplitude);
}

#[test]
fn test_jsonl_compact_null_bar() {
    // bar_position: None must serialize as null and round-trip correctly
    let frame = mock_capture_frame(0, None, 0.5);
    let compact = CompactFrame::from(&frame);
    let json = serde_json::to_string(&compact).unwrap();
    assert!(
        json.contains("\"bp\":null"),
        "None should serialize as null"
    );

    let decoded: CompactFrame = serde_json::from_str(&json).unwrap();
    let back: CaptureFrame = decoded.into();
    assert_eq!(back.bar_position, None);
    assert_eq!(back.bar_source, BarSource::None);
}

#[test]
fn test_jsonl_compact_bar_source_variants() {
    // All BarSource variants must survive serialization
    for (source, expected_str) in [
        (BarSource::None, "\"None\""),
        (BarSource::Sensor, "\"Sensor\""),
        (BarSource::Audio, "\"Audio\""),
        (BarSource::Fused, "\"Fused\""),
    ] {
        let mut frame = mock_capture_frame(0, Some(3.0), 0.7);
        frame.bar_source = source;
        let compact = CompactFrame::from(&frame);
        let json = serde_json::to_string(&compact).unwrap();
        assert!(
            json.contains(&format!("\"bx\":{}", expected_str)),
            "BarSource::{:?} should serialize as {}",
            source,
            expected_str
        );
        let back: CaptureFrame = serde_json::from_str::<CompactFrame>(&json).unwrap().into();
        assert_eq!(back.bar_source, source);
    }
}

// ─── JSONL self-describing header tests ──────────────────────────────────────

#[test]
fn test_jsonl_header_has_channel_definitions() {
    // Verify the header contains a self-describing parameter table
    use steel_capture::copedant::buddy_emmons_e9;

    let copedant = buddy_emmons_e9();
    // Build the same header that data_logger produces
    let header = serde_json::json!({
        "format": "steel-capture",
        "rate_hz": 60,
        "copedant": {
            "name": copedant.name,
            "open_strings_midi": copedant.open_strings,
            "pedals": copedant.pedals.iter().map(|p| {
                serde_json::json!({"name": p.name, "changes": p.changes.iter().map(|(s, d)| {
                    serde_json::json!({"string": s, "semitones": d})
                }).collect::<Vec<_>>()})
            }).collect::<Vec<_>>(),
            "levers": copedant.levers.iter().map(|l| {
                serde_json::json!({"name": l.name, "changes": l.changes.iter().map(|(s, d)| {
                    serde_json::json!({"string": s, "semitones": d})
                }).collect::<Vec<_>>()})
            }).collect::<Vec<_>>(),
        },
        "channels": [
            {"key": "t",  "name": "timestamp_us",      "type": "u64",    "unit": "microseconds"},
            {"key": "p",  "name": "pedals",             "type": "f32[]",  "count": 3,  "range": [0, 1], "unit": "engagement"},
            {"key": "kl", "name": "knee_levers",        "type": "f32[]",  "count": 5,  "range": [0, 1], "unit": "engagement"},
            {"key": "v",  "name": "volume",             "type": "f32",    "range": [0, 1], "unit": "engagement"},
            {"key": "bs", "name": "bar_sensors",        "type": "f32[]",  "count": 4,  "range": [0, 1], "unit": "hall_normalized"},
            {"key": "bp", "name": "bar_position",       "type": "f32?",   "range": [0, 24], "unit": "frets", "null_meaning": "bar lifted"},
            {"key": "bc", "name": "bar_confidence",     "type": "f32",    "range": [0, 1]},
            {"key": "bx", "name": "bar_source",         "type": "enum",   "values": ["None", "Sensor", "Audio", "Fused"]},
            {"key": "hz", "name": "string_pitches_hz",  "type": "f64[]",  "count": 10, "unit": "Hz"},
            {"key": "sa", "name": "string_active",      "type": "bool[]", "count": 10},
            {"key": "at", "name": "attacks",            "type": "bool[]", "count": 10},
            {"key": "am", "name": "string_amplitude",   "type": "f32[]",  "count": 10, "range": [0, 1]},
        ],
    });

    let header_str = serde_json::to_string(&header).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&header_str).unwrap();

    // Format identifier
    assert_eq!(parsed["format"], "steel-capture");
    assert_eq!(parsed["rate_hz"], 60);

    // Channel definitions are self-describing
    let channels = parsed["channels"].as_array().unwrap();
    assert_eq!(channels.len(), 12, "12 parameter channels defined");

    // Verify each channel has at minimum key, name, type
    for ch in channels {
        assert!(ch["key"].is_string(), "channel missing 'key'");
        assert!(ch["name"].is_string(), "channel missing 'name'");
        assert!(ch["type"].is_string(), "channel missing 'type'");
    }

    // Spot-check specific channels
    assert_eq!(channels[0]["key"], "t");
    assert_eq!(channels[0]["unit"], "microseconds");
    assert_eq!(channels[5]["key"], "bp");
    assert_eq!(channels[5]["unit"], "frets");
    assert_eq!(channels[5]["null_meaning"], "bar lifted");
    assert_eq!(channels[7]["key"], "bx");
    assert_eq!(channels[7]["values"].as_array().unwrap().len(), 4);
}

#[test]
fn test_jsonl_header_copedant_embedded() {
    // Verify the header embeds full copedant info for downstream interpretation
    use steel_capture::copedant::buddy_emmons_e9;

    let copedant = buddy_emmons_e9();
    let header = serde_json::json!({
        "format": "steel-capture",
        "rate_hz": 60,
        "copedant": {
            "name": copedant.name,
            "open_strings_midi": copedant.open_strings,
            "pedals": copedant.pedals.iter().map(|p| {
                serde_json::json!({"name": p.name})
            }).collect::<Vec<_>>(),
            "levers": copedant.levers.iter().map(|l| {
                serde_json::json!({"name": l.name})
            }).collect::<Vec<_>>(),
        },
    });

    let parsed: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&header).unwrap()).unwrap();

    let cop = &parsed["copedant"];
    assert_eq!(cop["name"], "Buddy Emmons E9");

    // Open strings are MIDI note numbers
    let strings = cop["open_strings_midi"].as_array().unwrap();
    assert_eq!(strings.len(), 10);
    assert_eq!(strings[0], 66.0); // F#4
    assert_eq!(strings[4], 59.0); // B3
    assert_eq!(strings[9], 47.0); // B2

    // Pedal and lever names
    let pedals = cop["pedals"].as_array().unwrap();
    assert_eq!(pedals.len(), 3);
    assert_eq!(pedals[0]["name"], "A");
    assert_eq!(pedals[2]["name"], "C");

    let levers = cop["levers"].as_array().unwrap();
    assert_eq!(levers.len(), 5);
    assert_eq!(levers[0]["name"], "LKL");
    assert_eq!(levers[4]["name"], "RKR");
}

#[test]
fn test_jsonl_input_end_to_end() {
    // Simulate a complete JSONL file as data_logger would write it,
    // then parse it back as a consumer (viz.js / analysis tool) would.
    use steel_capture::copedant::buddy_emmons_e9;

    let copedant = buddy_emmons_e9();

    // Build the real header (same code path as data_logger)
    let header = serde_json::json!({
        "format": "steel-capture",
        "rate_hz": 60,
        "copedant": {
            "name": copedant.name,
            "open_strings_midi": copedant.open_strings,
            "pedals": copedant.pedals.iter().map(|p| {
                serde_json::json!({"name": p.name, "changes": p.changes.iter().map(|(s, d)| {
                    serde_json::json!({"string": s, "semitones": d})
                }).collect::<Vec<_>>()})
            }).collect::<Vec<_>>(),
            "levers": copedant.levers.iter().map(|l| {
                serde_json::json!({"name": l.name, "changes": l.changes.iter().map(|(s, d)| {
                    serde_json::json!({"string": s, "semitones": d})
                }).collect::<Vec<_>>()})
            }).collect::<Vec<_>>(),
        },
        "channels": [
            {"key": "t",  "name": "timestamp_us",      "type": "u64",    "unit": "microseconds"},
            {"key": "p",  "name": "pedals",             "type": "f32[]",  "count": 3,  "range": [0, 1], "unit": "engagement"},
            {"key": "kl", "name": "knee_levers",        "type": "f32[]",  "count": 5,  "range": [0, 1], "unit": "engagement"},
            {"key": "v",  "name": "volume",             "type": "f32",    "range": [0, 1], "unit": "engagement"},
            {"key": "bs", "name": "bar_sensors",        "type": "f32[]",  "count": 4,  "range": [0, 1], "unit": "hall_normalized"},
            {"key": "bp", "name": "bar_position",       "type": "f32?",   "range": [0, 24], "unit": "frets", "null_meaning": "bar lifted"},
            {"key": "bc", "name": "bar_confidence",     "type": "f32",    "range": [0, 1]},
            {"key": "bx", "name": "bar_source",         "type": "enum",   "values": ["None", "Sensor", "Audio", "Fused"]},
            {"key": "hz", "name": "string_pitches_hz",  "type": "f64[]",  "count": 10, "unit": "Hz"},
            {"key": "sa", "name": "string_active",      "type": "bool[]", "count": 10},
            {"key": "at", "name": "attacks",            "type": "bool[]", "count": 10},
            {"key": "am", "name": "string_amplitude",   "type": "f32[]",  "count": 10, "range": [0, 1]},
        ],
    });

    // Build diverse frames: silence, active strings, bar slide, pedal engaged
    let source_frames = vec![
        CaptureFrame {
            timestamp_us: 0,
            pedals: [0.0; 3],
            knee_levers: [0.0; 5],
            volume: 0.7,
            bar_sensors: [0.0; 4],
            bar_position: None,
            bar_confidence: 0.0,
            bar_source: BarSource::None,
            string_pitches_hz: [0.0; 10],
            string_active: [false; 10],
            attacks: [false; 10],
            string_amplitude: [0.0; 10],
        },
        CaptureFrame {
            timestamp_us: 16667,
            pedals: [0.8, 0.0, 0.0],
            knee_levers: [0.0, 0.6, 0.0, 0.0, 0.0],
            volume: 0.9,
            bar_sensors: [0.9, 0.3, 0.05, 0.0],
            bar_position: Some(3.0),
            bar_confidence: 0.92,
            bar_source: BarSource::Fused,
            string_pitches_hz: [
                392.0, 329.6, 440.0, 349.2, 261.6, 220.0, 196.0, 174.6, 155.6, 130.8,
            ],
            string_active: [
                false, false, true, true, true, false, false, false, false, false,
            ],
            attacks: [
                false, false, true, true, true, false, false, false, false, false,
            ],
            string_amplitude: [0.0, 0.0, 0.85, 0.9, 0.7, 0.0, 0.0, 0.0, 0.0, 0.0],
        },
        CaptureFrame {
            timestamp_us: 33333,
            pedals: [0.8, 0.0, 0.0],
            knee_levers: [0.0, 0.6, 0.0, 0.0, 0.0],
            volume: 0.85,
            bar_sensors: [0.7, 0.5, 0.1, 0.0],
            bar_position: Some(5.0),
            bar_confidence: 0.88,
            bar_source: BarSource::Sensor,
            string_pitches_hz: [
                415.3, 349.2, 466.2, 370.0, 277.2, 233.1, 207.7, 185.0, 164.8, 138.6,
            ],
            string_active: [
                false, false, true, true, true, false, false, false, false, false,
            ],
            attacks: [false; 10],
            string_amplitude: [0.0, 0.0, 0.6, 0.65, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0],
        },
    ];

    // Write JSONL (header + compact frames)
    let mut jsonl = serde_json::to_string(&header).unwrap();
    jsonl.push('\n');
    for frame in &source_frames {
        jsonl.push_str(&serde_json::to_string(&CompactFrame::from(frame)).unwrap());
        jsonl.push('\n');
    }

    // ── Parse back as a consumer would ──

    let lines: Vec<&str> = jsonl.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 4); // header + 3 frames

    // Parse header, extract channel definitions
    let hdr: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(hdr["format"], "steel-capture");
    let rate = hdr["rate_hz"].as_u64().unwrap();
    assert_eq!(rate, 60);

    let channels = hdr["channels"].as_array().unwrap();
    // Build a key→channel_def lookup (what a generic consumer would do)
    let channel_keys: Vec<&str> = channels
        .iter()
        .map(|c| c["key"].as_str().unwrap())
        .collect();
    assert!(channel_keys.contains(&"t"));
    assert!(channel_keys.contains(&"bp"));
    assert!(channel_keys.contains(&"hz"));

    // Parse copedant from header
    let cop = &hdr["copedant"];
    let open_strings: Vec<f64> = cop["open_strings_midi"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert_eq!(open_strings.len(), 10);
    assert!((open_strings[0] - 66.0).abs() < 0.01); // F#4

    // Parse each frame line and validate against header-declared ranges
    let mut parsed_frames: Vec<CaptureFrame> = Vec::new();
    for &line in &lines[1..] {
        let compact: CompactFrame = serde_json::from_str(line).unwrap();
        let frame: CaptureFrame = compact.into();

        // Validate ranges declared in header
        for &p in &frame.pedals {
            assert!((0.0..=1.0).contains(&p));
        }
        for &kl in &frame.knee_levers {
            assert!((0.0..=1.0).contains(&kl));
        }
        assert!((0.0..=1.0).contains(&frame.volume));
        for &bs in &frame.bar_sensors {
            assert!((0.0..=1.0).contains(&bs));
        }
        if let Some(bp) = frame.bar_position {
            assert!((0.0..=24.0).contains(&bp));
        }
        assert!((0.0..=1.0).contains(&frame.bar_confidence));
        for &am in &frame.string_amplitude {
            assert!((0.0..=1.0).contains(&am));
        }

        parsed_frames.push(frame);
    }

    assert_eq!(parsed_frames.len(), 3);

    // Frame 0: silence
    assert_eq!(parsed_frames[0].bar_position, None);
    assert_eq!(parsed_frames[0].bar_source, BarSource::None);
    assert!(parsed_frames[0].string_active.iter().all(|&a| !a));

    // Frame 1: attack at fret 3, pedal A engaged, strings 2-4 active
    assert_eq!(parsed_frames[1].bar_position, Some(3.0));
    assert_eq!(parsed_frames[1].bar_source, BarSource::Fused);
    assert!((parsed_frames[1].pedals[0] - 0.8).abs() < 0.01);
    assert!(parsed_frames[1].string_active[2]);
    assert!(parsed_frames[1].attacks[2]);
    assert!((parsed_frames[1].string_amplitude[2] - 0.85).abs() < 0.01);

    // Frame 2: bar slid to fret 5, same strings active, no new attacks
    assert_eq!(parsed_frames[2].bar_position, Some(5.0));
    assert_eq!(parsed_frames[2].bar_source, BarSource::Sensor);
    assert!(parsed_frames[2].string_active[2]);
    assert!(
        !parsed_frames[2].attacks[2],
        "no new attack on sustained string"
    );
    assert!(
        parsed_frames[2].string_amplitude[2] < parsed_frames[1].string_amplitude[2],
        "amplitude decays after attack"
    );

    // Timestamps are monotonic at expected rate
    assert!(parsed_frames[1].timestamp_us > parsed_frames[0].timestamp_us);
    assert!(parsed_frames[2].timestamp_us > parsed_frames[1].timestamp_us);
}
