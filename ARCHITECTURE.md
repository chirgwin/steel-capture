# Steel Capture — Architecture

## How to Run

```bash
# Default: simulator + native GUI window. That's it.
cargo run

# Headless (no window, console output only)
cargo run -- --no-gui --console

# With browser viz too (WebSocket on :8080)
cargo run -- --ws
# Then open http://localhost:8080

# With hardware (when Teensy is connected)
cargo run --features hardware -- --simulate false --port /dev/ttyACM0
```

`cargo run` does everything: starts the simulator, launches a native macOS
window (via egui/eframe), shows real-time visualization. No browser, no
WebSocket, no HTTP server, no separate processes needed.


## Data Flow

```
┌─────────────┐     InputEvent      ┌─────────────┐     CaptureFrame
│  Simulator   │──────────────────►│ Coordinator  │──────────────────►  Consumers
│  (or Teensy) │  SensorFrame @1kHz │             │  via channels
└─────────────┘                    │  - bar infer │
                                   │  - copedant  │    ┌──────────┐
                                   │  - attacks   │───►│ GUI      │ (main thread)
                                   └─────────────┘    │ (egui)   │
                                        │              └──────────┘
                                        │              ┌──────────┐
                                        └─────────────►│ Console  │ (opt-in)
                                        │              └──────────┘
                                        │              ┌──────────┐
                                        └─────────────►│ WS Server│ (opt-in, --ws)
                                        │              └──────────┘
                                        │              ┌──────────┐
                                        └─────────────►│ OSC      │ (opt-in, --osc)
                                                       └──────────┘
```

### Thread Architecture

All communication is via crossbeam channels (lock-free, bounded).

| Thread       | What it does                                        |
|-------------|-----------------------------------------------------|
| **main**     | Launches threads, then runs egui event loop (blocks) |
| simulator    | Generates SensorFrame at 1kHz (demo gesture sequence)|
| coordinator  | Receives inputs, runs bar inference + copedant, detects attacks, broadcasts CaptureFrame to all consumers |
| ws-server    | (opt-in) HTTP+WS on single port, throttled broadcast |
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

- **Bar slide** — Continuous pitch change, no new notehead. This is
  glissando. The same note bends smoothly to a new position.
- **Volume change** — Same note, different loudness. No new notehead.
- **Vibrato** — Rapid small oscillation. No new notehead.
- **Sustained string** — Already active, no change. No notehead.


## String Activation — Hardware Options

`string_active[i]` is just a boolean. The software doesn't care HOW
you detect it. Options for hardware:

**A. Individual Electromagnetic Pickups (recommended)**
- Small coil pickup under each string (10 total)
- 10 analog signals → Teensy ADC
- Simple threshold onset detection per string
- Also gives amplitude envelope (volume per string)
- Can be wound from guitar pickup wire + small magnets

**B. Audio Onset Detection**
- Mic/piezo → audio interface → cpal input
- Spectral analysis or energy-based onset detection
- Harder to isolate individual strings from mix

**C. Hall Sensor Pick Detection**
- SS49E sensors near the picking area
- Detect string displacement when plucked

**D. Capacitive/Optical**
- IR break-beam or capacitive proximity
- Non-contact, but more complex

The current software supports any approach — the hardware layer just
fills the `string_active` boolean array.


## Visualization Panels

The GUI shows four stacked views (top to bottom):

1. **Staff Notation** — Grand staff with noteheads at attack points.
   Colored by string. Ledger lines and accidentals as needed.

2. **Attack Strip** — Thin timeline showing attack markers. Quick visual
   for pick density and pedal/lever changes.

3. **Tablature** — 10 lines (one per string). Fret numbers at attacks.
   Pedal/lever annotations below each number.

4. **Piano Roll** — MIDI-style dots at attack points. Y = pitch, X = time.
   No connecting lines — discrete dots per event.

All four panels share the same centered-playhead timeline: cursor at
horizontal center, history scrolls leftward, right side empty.

**Sidebar**: real-time state display — bar position, pedal/lever bars,
volume, string list with note names and Hz, attack flash indicators.


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
