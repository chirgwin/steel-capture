use crate::types::*;
use crossbeam_channel::Receiver;
use std::io::{self, Write};

/// Renders a live ASCII dashboard of the capture state.
pub struct ConsoleDisplay {
    rx: Receiver<CaptureFrame>,
    update_hz: u32,
}

impl ConsoleDisplay {
    pub fn new(rx: Receiver<CaptureFrame>, update_hz: u32) -> Self {
        Self { rx, update_hz }
    }

    pub fn run(&self) {
        let skip = if self.update_hz == 0 { 50 } else { (1000 / self.update_hz).max(1) as u64 };
        let mut count: u64 = 0;
        let mut stdout = io::stdout();

        for frame in self.rx.iter() {
            count += 1;
            if count % skip != 0 {
                continue;
            }

            // Clear screen and move cursor home
            print!("\x1b[2J\x1b[H");

            println!("╔══════════════════════════════════════════════════════════╗");
            println!("║  STEEL CAPTURE — Live Monitor                           ║");
            println!("╠══════════════════════════════════════════════════════════╣");

            // Timestamp
            let secs = frame.timestamp_us as f64 / 1_000_000.0;
            println!("║  Time: {:.2}s                                          ║", secs);

            // Pedals
            println!("║                                                          ║");
            println!("║  Pedals:                                                 ║");
            for (i, &val) in frame.pedals.iter().enumerate() {
                let bar = make_bar(val, 30);
                println!("║    {}: {} {:.0}%{}", PEDAL_NAMES[i], bar, val * 100.0,
                    " ".repeat(20 - format!("{:.0}", val * 100.0).len()));
            }

            // Knee levers
            println!("║                                                          ║");
            println!("║  Knee Levers:                                            ║");
            for (i, &val) in frame.knee_levers.iter().enumerate() {
                let bar = make_bar(val, 30);
                println!("║    {:>3}: {} {:.0}%{}",
                    LEVER_NAMES[i], bar, val * 100.0,
                    " ".repeat(18 - format!("{:.0}", val * 100.0).len()));
            }

            // Volume
            println!("║                                                          ║");
            let vbar = make_bar(frame.volume, 30);
            println!("║  Volume: {} {:.0}%                             ║",
                vbar, frame.volume * 100.0);

            // Bar position
            println!("║                                                          ║");
            match frame.bar_position {
                Some(pos) => {
                    let fret_display = make_fretboard(pos, 24);
                    let src = match frame.bar_source {
                        BarSource::None => "---",
                        BarSource::Sensor => "sensor",
                        BarSource::Audio => "audio",
                        BarSource::Fused => "fused",
                    };
                    println!("║  Bar: fret {:.2} (conf: {:.0}%, src: {:6})         ║",
                        pos, frame.bar_confidence * 100.0, src);
                    println!("║  {} ║", fret_display);
                }
                None => {
                    println!("║  Bar: --- (not detected)                               ║");
                    println!("║  {:54} ║", "");
                }
            }

            // String pitches
            println!("║                                                          ║");
            println!("║  String Pitches:                                         ║");
            for (i, &hz) in frame.string_pitches_hz.iter().enumerate() {
                let note = hz_to_note_name(hz);
                println!("║    {:>6}: {:>7.1} Hz  ({:>4})                        ║",
                    E9_STRING_NAMES[i], hz, note);
            }

            println!("╚══════════════════════════════════════════════════════════╝");
            let _ = stdout.flush();
        }
    }
}

fn make_bar(val: f32, width: usize) -> String {
    let filled = (val * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

fn make_fretboard(pos: f32, max_fret: usize) -> String {
    // Simple fretboard visualization
    let mut fb = String::new();
    fb.push_str("Nut ");
    for fret in 0..=max_fret {
        if (pos - fret as f32).abs() < 0.3 {
            fb.push('▼'); // bar is here
        } else {
            fb.push('│');
        }
        fb.push(' ');
    }
    // Pad to fixed width (char count, not byte count)
    while fb.chars().count() < 54 {
        fb.push(' ');
    }
    // Truncate by char count to avoid splitting multi-byte chars
    fb.chars().take(54).collect()
}

fn hz_to_note_name(hz: f64) -> String {
    if hz < 20.0 {
        return "---".to_string();
    }
    let midi = 69.0 + 12.0 * (hz / 440.0).log2();
    let note_num = midi.round() as i32;
    let cents = ((midi - note_num as f64) * 100.0).round() as i32;

    let note_names = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let name = note_names[((note_num % 12 + 12) % 12) as usize];
    let octave = (note_num / 12) - 1;

    if cents == 0 {
        format!("{}{}", name, octave)
    } else if cents > 0 {
        format!("{}{}+{}", name, octave, cents)
    } else {
        format!("{}{}{}", name, octave, cents)
    }
}
