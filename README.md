# Steel Capture

Pedal steel guitar expression capture system. Captures pedal/lever/volume state, infers bar position from audio + hall sensors, detects per-string onsets, and streams everything to a browser visualization, OSC targets, or session logs.

**Zero instrument modification.** All sensors attach via velcro, tape, or putty.

## Quick Start (Simulator — No Hardware Needed)

```bash
# Install Rust if you haven't already
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Build (no GUI, works on any platform)
cd steel-capture
cargo build --release --no-default-features

# Run the simulator with browser visualization
cargo run --release --no-default-features -- --ws

# Open http://localhost:8080 in your browser
```

The simulator runs a ~15-second demo sequence exercising pedals, knee levers, bar slides, vibrato, and volume swells with synthetic audio matching the E9 copedant.

## Build Options

| Command | What you get |
|---------|-------------|
| `cargo build --no-default-features` | Headless (no GUI). Console, WebSocket, OSC, logging. |
| `cargo build` | Native GUI (egui) + everything above. Requires OpenGL. |
| `cargo build --features hardware` | + Serial port support for Teensy 4.1 |

## Run Modes

### Simulator (development/testing)
```bash
# Browser visualization on localhost:8080
cargo run --release --no-default-features -- --ws

# With audio-based string detection (tests the detector against synthetic audio)
cargo run --release --no-default-features -- --ws --detect-strings

# Console TUI (no browser needed)
cargo run --release --no-default-features -- --console

# Log session data to disk
cargo run --release --no-default-features -- --log-data --output-dir ./sessions

# OSC output (e.g., to Csound, SuperCollider, Max)
cargo run --release --no-default-features -- --osc --osc-target 127.0.0.1:9000

# Combine everything
cargo run --release --no-default-features -- --ws --osc --log-data --console
```

### Hardware (with Teensy + sensors)
```bash
cargo run --release --features hardware -- --port /dev/ttyACM0 --ws --detect-strings
```

## Tests

```bash
# Run all tests (46 total: 35 unit + 11 integration)
cargo test --no-default-features

# Just unit tests
cargo test --no-default-features --lib

# Just integration tests
cargo test --no-default-features --test integration

# Specific test
cargo test --no-default-features test_pipeline_attacks_on_string_onset
```

### Test Coverage

**Unit tests (35):**
- `bar_inference` (5): Goertzel frequency detection, sensor-only during silence, fused sensor+audio, pedal interaction, bar lift
- `bar_sensor` (8): Hall sensor readings at various frets, interpolation accuracy, smoothing, edge cases
- `copedant` (13): MIDI/Hz conversion, open strings, all pedals (A/B/C), all levers (LKL/LKR/LKV/RKL/RKR), partial engagement, combinations (A+C), two-stop lever (RKR soft/hard)
- `string_detector` (7): Single string detection, 3-string grip, pedal interaction, silence, no-bar, attack-only-on-onset, release-then-reattack

**Integration tests (11):**
- Pipeline: basic grip, pedal pitch shift, attack timing, pedal-triggered attacks, silence/no-bar, volume independence
- Audio detection validation against simulator ground truth
- Per-string spectral resolution
- Bar sensor to inference pipeline across frets 0-15
- CaptureFrame JSON serialization round-trip (frontend contract)

## Architecture

```
                         ┌──────────────┐
┌─────────────┐     ┌────┤ SerialReader │──┐    ┌─────────────┐
│  Teensy 4.1 │─USB─┤    └──────────────┘  │    │ Console TUI │
│  (sensors)  │     │         OR           │    └──────▲──────┘
└─────────────┘     │    ┌──────────────┐  │    ┌──────┴──────┐
                    └────┤  Simulator   │──┼───▸│  WS Server  │──▸ Browser
                         └──────────────┘  │    └─────────────┘
                              │            │    ┌─────────────┐
                         InputEvent ch.    ├───▸│ OSC Sender  │──▸ Csound
                              │            │    └─────────────┘
                         ┌────▼────────┐   │    ┌─────────────┐
                         │ Coordinator │───┴───▸│ Data Logger │
                         │             │        └─────────────┘
                         │ BarInference│
                         │ StringDetect│
                         │ CopedantEng │
                         └─────────────┘
```

### Key modules

| Module | Purpose |
|--------|---------|
| `types.rs` | Core data types: SensorFrame, AudioChunk, CaptureFrame, Copedant |
| `copedant.rs` | E9 tuning model, pitch computation, bar position inference math |
| `bar_inference.rs` | Fuses hall sensors + Goertzel spectral matching for bar position |
| `bar_sensor.rs` | Hall sensor interpolation (4x SS49E at frets 0/5/10/15) |
| `string_detector.rs` | Per-string onset/release via Goertzel at copedant-derived frequencies |
| `coordinator.rs` | Central pipeline: receives inputs, runs inference, produces CaptureFrames |
| `simulator.rs` | Generates synthetic sensor data + matching audio (sine waves) |
| `ws_server.rs` | Combined HTTP + WebSocket server for browser visualization |
| `osc_sender.rs` | UDP OSC output for DAWs and synthesis environments |
| `data_logger.rs` | Session recording (JSONL frames + raw audio) |
| `console_display.rs` | ASCII terminal dashboard |
| `gui.rs` | Native egui window (optional) |

### Bar Position Inference

The system infers bar position from audio using the copedant as a constraint:

1. Copedant engine computes each string's expected open pitch given pedal/lever state
2. For each candidate fret position (0.0-24.0 in 0.1 steps), compute expected frequencies
3. Goertzel algorithm measures energy at those frequencies in the audio buffer
4. Best-scoring fret position = bar position. Parabolic refinement for sub-0.1 resolution
5. Fused with hall sensor estimate when available

**Precision:** +/-5 cents = ~0.14mm at fret 3 on a 24" scale. More precise than magnetic sensors.

### Per-String Detection

Because bar position + copedant state tells us the exact Hz of every string, we don't need polyphonic pitch detection. Instead:

1. Compute the 10 expected frequencies from copedant + bar position
2. Goertzel magnitude at each frequency (+ 2nd harmonic for noise rejection)
3. Smoothed energy tracking with hysteresis onset/release thresholds
4. Reports (string_active[10], attacks[10]) per analysis frame

## Hardware (when ready)

See `HARDWARE.md` for the shopping list (~$90). Key components:
- Teensy 4.1 ($31.50) — 9 analog inputs at 1kHz
- 10x SS49E hall sensors ($10) — pedals, levers, volume, bar position
- Neodymium magnets ($8) — attach to pedal bars and bar tip
- Firmware: `teensy/steel_capture.ino`

## OSC Address Map

| Address | Type | Range | Description |
|---------|------|-------|-------------|
| `/steel/pedal/{a,b,c}` | float | 0-1 | Pedal engagement |
| `/steel/knee/{0..4}` | float | 0-1 | Knee lever (LKL/LKR/LKV/RKL/RKR) |
| `/steel/volume` | float | 0-1 | Volume pedal |
| `/steel/bar/pos` | float | 0-24 | Bar position in frets (-1 = not detected) |
| `/steel/bar/confidence` | float | 0-1 | Inference confidence |
| `/steel/bar/source` | float | 0-3 | 0=none, 1=sensor, 2=audio, 3=fused |
| `/steel/pitch/{0..9}` | float | Hz | Per-string pitch |

## WebSocket Protocol

CaptureFrame JSON streamed at `--ws-fps` rate (default 60):

```json
{
  "timestamp_us": 1234567,
  "pedals": [0.0, 0.5, 1.0],
  "knee_levers": [0.0, 0.0, 0.0, 0.3, 0.0],
  "volume": 0.8,
  "bar_sensors": [0.1, 0.9, 0.2, 0.0],
  "bar_position": 5.0,
  "bar_confidence": 0.95,
  "bar_source": "Fused",
  "string_pitches_hz": [370.0, 311.0, 415.0, 329.0, 247.0, 207.0, 185.0, 164.0, 147.0, 123.0],
  "string_active": [false, false, true, true, true, false, false, false, false, false],
  "attacks": [false, false, true, false, false, false, false, false, false, false]
}
```

## CLI Reference

```
steel-capture [OPTIONS]

Options:
      --simulate            Run in simulator mode [default: true]
      --port <PORT>         Serial port [default: /dev/ttyACM0]
      --osc-target <ADDR>   OSC target [default: 127.0.0.1:9000]
      --osc                 Enable OSC output
      --log-data            Enable data logging
      --output-dir <DIR>    Session output directory [default: ./sessions]
      --console             Enable console TUI
      --display-hz <HZ>     Console refresh rate [default: 20]
      --no-gui              Disable native GUI
      --ws                  Enable WebSocket server
      --ws-addr <ADDR>      WebSocket bind address [default: 0.0.0.0:8080]
      --ws-fps <FPS>        WebSocket broadcast rate [default: 60]
      --sensor-rate <HZ>    Sensor sample rate [default: 1000]
      --detect-strings      Use audio-based string detection
```
