# CLAUDE.md — Steel Capture Project Context

## What This Is

A pedal steel guitar expression capture system. Records every aspect of a performance — pedal/lever/volume positions, bar position, which strings are sounding — and streams it all in real time to a browser visualization, OSC (for Csound/SuperCollider), and session logs. Built in Rust. Zero instrument modification.

The owner is Geoff, a composer and musician. He plays pedal steel and wants to capture performances for notation, resynthesis, and analysis.

## Commands

```bash
cargo test --no-default-features        # Run all 46 tests (35 unit + 11 integration)
cargo test --no-default-features --lib   # Unit tests only
cargo test --no-default-features --test integration  # Integration tests only
cargo build --no-default-features        # Headless build (no GUI dependency)
cargo build                              # With native egui GUI (needs OpenGL)
cargo run --release --no-default-features -- --ws  # Run simulator + browser viz on :8080
```

Always use `--no-default-features` unless specifically working on the egui GUI. The default feature enables `eframe` which requires OpenGL and heavy dependencies.

## Architecture

```
Simulator ──► InputEvent channel ──► Coordinator ──► CaptureFrame channel ──► {WS, OSC, Logger, Console}
   │                                      │
   ├─ SensorFrame (pedals, levers,        ├─ BarInference (hall sensors + Goertzel spectral matching)
   │   volume, bar hall sensors,          ├─ StringDetector (per-string onset via Goertzel at known freqs)
   │   string_active ground truth)        └─ CopedantEngine (E9 tuning model, pitch computation)
   │
   └─ AudioChunk (synthetic sine waves matching copedant state)
```

**Key insight:** We don't do blind polyphonic pitch detection. Because we know the copedant state (pedals/levers) and bar position at every moment, we know the exact expected Hz of all 10 strings. String detection is just "is there energy at 370 Hz?" via Goertzel — a matched filter, not MIR.

**Bar inference** works similarly: score each candidate fret position (0.0–24.0) by summing Goertzel magnitudes at the 10 expected frequencies. Best score = bar position. Parabolic refinement for sub-0.1 fret precision.

## Module Map

| File | What it does |
|------|-------------|
| `types.rs` | SensorFrame, AudioChunk, CaptureFrame, Copedant, BarState, SessionClock |
| `copedant.rs` | Buddy Emmons E9 tuning definition, CopedantEngine (pitch computation, bar inference math) |
| `bar_inference.rs` | Fuses hall sensor + audio spectral matching for bar position |
| `bar_sensor.rs` | Hall sensor interpolation (4× SS49E at frets 0/5/10/15), magnetic field model |
| `string_detector.rs` | Per-string onset/release detection via Goertzel at copedant-derived frequencies |
| `coordinator.rs` | Central pipeline: receives InputEvents, runs inference + detection, emits CaptureFrames |
| `simulator.rs` | Generates synthetic sensor data + matching audio. Gesture-based demo sequence |
| `ws_server.rs` | Combined HTTP + WebSocket server. Serves visualization.html + static files, streams JSON |
| `osc_sender.rs` | UDP OSC output for DAWs/synthesis |
| `data_logger.rs` | Session recording (JSONL frames + raw audio binary) |
| `console_display.rs` | ASCII terminal dashboard |
| `gui.rs` | Native egui window (optional, behind `gui` feature flag) |
| `serial_reader.rs` | Teensy USB serial protocol (behind `hardware` feature flag) |
| `lib.rs` | Public module exports (enables integration tests) |
| `main.rs` | CLI (clap), thread spawning, channel wiring |

## E9 Copedant (Buddy Emmons)

Open tuning (string 1 = far from player):
```
1:F#4  2:D#4  3:G#4  4:E4  5:B3  6:G#3  7:F#3  8:E3  9:D3  10:B2
```

Pedals: A (str5,10 +2), B (str3,6 +1), C (str4,5 +2)
Levers: LKL (str4,8 +1), LKR (str4,5,8 -1), LKV (str5,10 -1), RKL (str2 +1, str6 -2), RKR (str2 -2, str9 -1)

RKR is a two-stop lever: soft stop at ~50% engagement, hard stop at 100%. Modeled via proportional engagement.

## Decisions Made

- **Audio-based bar inference + hall sensors, zero fretboard mods.** No optical sensors on the neck.
- **Tier 1 string detection (spectral analysis)** chosen over Tier 2 (individual piezos) and Tier 3 (IR sensors). Uses existing audio stream. Can upgrade to piezos later if fast picking resolution is insufficient — Teensy has ADC headroom.
- **Goertzel over FFT** for both bar inference and string detection. We only need energy at specific known frequencies, not a full spectrum. Much cheaper.
- **Crossbeam channels** for inter-thread communication. Bounded channels with backpressure.
- **Simulator generates phase-continuous audio** using a monotonic sample counter (not wall-clock time) to avoid phase discontinuities from OS scheduling jitter.
- **WebSocket server serves static files** from the visualization.html directory. Supports viz.js, CSS, JSON, SVG, etc. Path traversal rejected.
- **Attack detection** fires on: (1) string inactive→active transition, (2) pedal crossing 0.5 threshold while string active, (3) lever crossing 0.5 threshold while string active.

## Browser Visualization

`visualization.html` loads `viz.js`. The WS server at :8080 serves both files and streams CaptureFrame JSON over WebSocket. The viz has:
- Instrument view (fretboard, bar position, pedals, levers, volume)
- Staff notation (treble + bass clef, real-time noteheads)
- Tablature
- Attack strip
- Piano roll
- Sound toggle (Web Audio synthesis from the pitch data)
- File loading (can replay recorded JSON sessions)

## Hardware (Not Yet Assembled)

~$90 total. Teensy 4.1 + SS49E hall sensors + neodymium magnets. Firmware is in `teensy/steel_capture.ino`. Binary protocol: 26-byte frames with sync word, timestamp, 9 ADC values, CRC-16. All sensors attach with velcro/tape/putty — full removal in 15 minutes.

## Current State (February 2026)

**Working:**
- Full Rust pipeline, 46 tests (0 warnings)
- All modules compile and integrate
- Simulator runs the demo sequence (~15s of realistic gestures)
- Browser viz connects and displays
- Bar inference from hall sensors + audio fusion
- Per-string detection from constrained spectral analysis
- OSC, logging, console display all functional

**Needs work / next steps:**
- Hardware assembly and calibration
- Threshold tuning with real steel string audio (synthetic sines ≠ real harmonics/noise)
- Fast picking resolution testing (42ms window may miss rapid rolls)
- Session playback improvements in the viz
- Csound resynthesis patch (Geoff will likely drive this himself)
- Possible Tier 2 piezo upgrade if spectral detection is insufficient for real playing

## Style Notes

- Geoff knows music theory. Don't over-explain fundamentals.
- He prefers concise, direct communication. No filler.
- Code should compile with zero warnings. Run `cargo test --no-default-features` after changes.
- The project uses `log` + `env_logger`. Use `trace!` for high-frequency diagnostics, `debug!` for periodic stats, `info!` for lifecycle events.
