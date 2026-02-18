# Steel Capture — Architecture

## How to Run

```bash
# Native GUI (wry/tao WebView loading the browser viz)
cargo run --release -- --demo improv

# Headless + browser viz on :8080
cargo run --release --no-default-features -- --ws --demo improv

# Console TUI only
cargo run --release --no-default-features -- --console --demo basic

# With hardware (when Teensy is connected)
cargo run --release --features hardware -- --port /dev/ttyACM0 --ws --detect-strings
```

The native GUI starts a WebView window that loads `http://localhost:8080` — the same page you'd see in a browser. The WS server auto-starts when the GUI is active; `--ws` adds external browser access.


## Data Flow

```
┌─────────────┐     InputEvent      ┌─────────────┐     CaptureFrame
│  Simulator   │──────────────────►│ Coordinator  │──────────────────►  Consumers
│  (or Teensy) │  SensorFrame @1kHz │             │  via channels
└─────────────┘  AudioChunk @44.1k  │  - bar infer │
                                   │  - copedant  │    ┌──────────┐
                                   │  - string det│───►│ WebView  │ (main thread, opt)
                                   └─────────────┘    │ (wry/tao)│
                                        │              └──────────┘
                                        │              ┌──────────┐
                                        ├─────────────►│ WS Server│ (auto or --ws)
                                        │              └──────────┘
                                        │              ┌──────────┐
                                        ├─────────────►│ Console  │ (--console)
                                        │              └──────────┘
                                        │              ┌──────────┐
                                        ├─────────────►│ OSC      │ (--osc)
                                        │              └──────────┘
                                        │              ┌──────────┐
                                        └─────────────►│ Logger   │ (--log-data)
                                                       └──────────┘
```

### Thread Architecture

All communication is via crossbeam channels (lock-free, bounded).

| Thread       | What it does                                        |
|-------------|-----------------------------------------------------|
| **main**     | Launches threads, then runs WebView event loop (GUI) or waits (headless) |
| simulator    | Generates SensorFrame at 1kHz + AudioChunk at 44.1kHz |
| coordinator  | Receives inputs, runs bar inference + copedant + string detection, broadcasts CaptureFrame to all consumers |
| ws-server    | HTTP+WS on single port, throttled broadcast at --ws-fps |
| console      | (opt-in) Terminal display at configurable Hz          |
| osc          | (opt-in) OSC output to DAW/plugin host               |
| logger       | (opt-in) Writes session data to disk                 |


## The CaptureFrame

Every frame broadcast to consumers contains the complete state:

```rust
CaptureFrame {
    timestamp_us: u64,             // µs since session start
    pedals: [f32; 3],              // A, B, C — 0.0 (up) to 1.0 (engaged)
    knee_levers: [f32; 5],         // LKL, LKR, LKV, RKL, RKR — 0.0 to 1.0
    volume: f32,                   // volume pedal — 0.0 to 1.0
    bar_sensors: [f32; 4],         // raw hall readings at frets 0,5,10,15
    bar_position: Option<f32>,     // fret position (None = off strings)
    bar_confidence: f32,           // 0.0–1.0
    bar_source: BarSource,         // None | Sensor | Audio | Fused
    string_pitches_hz: [f64; 10],  // computed pitch per string
    string_active: [bool; 10],     // which strings are sounding
    attacks: [bool; 10],           // NEW NOTE events this frame
}
```


## Attack Detection Model

An **attack** = "draw a new notehead." A discrete pitch event — the moment
a note begins or changes to a new pitch.

### Three triggers:

#### 1. String Pick (finger/pick)
```
string_active: false → true
```
String was silent, now sounding. The physical pick/pluck moment.

#### 2. Pedal Threshold Crossing
```
pedal engagement crosses 0.5 threshold (either direction)
while the affected string is currently active
```
- Pedal goes from <0.5 to >0.5 → **engage** → pitch shifts up → new note
- Pedal goes from >0.5 to <0.5 → **release** → pitch shifts back → new note

Both directions fire attacks because both represent discrete pitch changes.

Example: Pedal A engages while strings 5 and 10 are sounding →
attacks[4] = true, attacks[9] = true (strings 5,10 shift up 2 semitones).
When Pedal A releases → same strings get attacks again (pitch returns).

#### 3. Lever Threshold Crossing
Same logic as pedals, for knee levers (LKL, LKR, LKV, RKL, RKR).

### What is NOT an attack:

- **Bar slide** — Continuous pitch change, no new notehead. Glissando.
- **Volume change** — Same note, different loudness.
- **Vibrato** — Rapid small oscillation.
- **Sustained string** — Already active, no change.


## String Detection

Handled entirely in software via constrained spectral analysis. Because the copedant state (pedals/levers) and bar position are known at every moment, we know the exact Hz of all 10 strings. Detection is a matched Goertzel filter at each expected frequency — not blind polyphonic pitch detection.

1. Compute 10 expected frequencies from copedant + bar position
2. Goertzel magnitude at each frequency (+ 2nd harmonic for noise rejection)
3. Smoothed energy tracking with hysteresis onset/release thresholds
4. Reports `(string_active[10], attacks[10])` per analysis frame

Future hardware upgrade path: per-string piezos near the bridge for sub-ms attack timing if the spectral approach proves insufficient for fast picking.


## Copedant (Buddy Emmons E9 Standard)

| Str | Open  | A     | B     | C     | LKL  | LKR  | RKL  | RKR  |
|-----|-------|-------|-------|-------|------|------|------|------|
|  1  | F#4   |       |       |       |      |      | +1   |      |
|  2  | D#4   |       |       |       |      |      |      | +2   |
|  3  | G#4   |       | +1(A) |       | -1   | +1   |      |      |
|  4  | E4    |       |       | +2(F#)|      |      |      |      |
|  5  | B3    | +2(C#)|       | +2(C#)|      |      |      |      |
|  6  | G#3   |       | +1(A) |       |      |      |      |      |
|  7  | F#3   |       |       |       |      |      |      |      |
|  8  | E3    |       |       |       | -1   | +1   |      |      |
|  9  | D3    |       |       |       |      |      |      |      |
| 10  | B2    | +2(C#)|       |       |      |      |      |      |

Pitch = open_midi + bar_fret + Σ(pedal_delta × engagement) + Σ(lever_delta × engagement)
