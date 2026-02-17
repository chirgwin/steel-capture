use crate::copedant::hz_to_midi;
use crate::types::*;
use crossbeam_channel::Receiver;
use eframe::egui;
use std::collections::VecDeque;

// ═══ COLORS ═══
const SC: [egui::Color32; 10] = [
    egui::Color32::from_rgb(231, 76, 60),   // string 1
    egui::Color32::from_rgb(230, 126, 34),   // string 2
    egui::Color32::from_rgb(241, 196, 15),   // string 3
    egui::Color32::from_rgb(46, 204, 113),   // string 4
    egui::Color32::from_rgb(26, 188, 156),   // string 5
    egui::Color32::from_rgb(52, 152, 219),   // string 6
    egui::Color32::from_rgb(41, 128, 185),   // string 7
    egui::Color32::from_rgb(155, 89, 182),   // string 8
    egui::Color32::from_rgb(142, 68, 173),   // string 9
    egui::Color32::from_rgb(233, 30, 99),    // string 10
];

const BG: egui::Color32 = egui::Color32::from_rgb(6, 6, 14);
const PANEL_BG: egui::Color32 = egui::Color32::from_rgb(11, 11, 22);
const BORDER: egui::Color32 = egui::Color32::from_rgb(22, 22, 40);
const TXT: egui::Color32 = egui::Color32::from_rgb(138, 138, 170);
const TXT_HI: egui::Color32 = egui::Color32::from_rgb(200, 200, 224);
const BAR_COL: egui::Color32 = egui::Color32::from_rgb(26, 188, 156);
const PED_COL: egui::Color32 = egui::Color32::from_rgb(230, 126, 34);
const LEV_COL: egui::Color32 = egui::Color32::from_rgb(41, 128, 185);
const VOL_COL: egui::Color32 = egui::Color32::from_rgb(155, 89, 182);
const GRID_LINE: egui::Color32 = egui::Color32::from_rgb(10, 10, 20);

// ═══ MUSIC CONSTANTS ═══
const NOTE_NAMES: [&str; 12] = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
const MIDI_LO: f64 = 40.0;
const MIDI_HI: f64 = 90.0;
const HISTORY_SECS: f64 = 24.0;
/// At ~60fps display rate, 24 seconds = ~1440 frames.
const MAX_HISTORY: usize = 1800;
/// Minimum microseconds between history entries (~60fps).
const HISTORY_INTERVAL_US: u64 = 16_000;
const PEDAL_NAMES: [&str; 3] = ["A", "B", "C"];
const LEVER_NAMES: [&str; 5] = ["LKL", "LKR", "LKV", "RKL", "RKR"];

fn midi_to_note_name(midi: f64) -> String {
    let i = midi.round() as i32;
    let pc = ((i % 12) + 12) % 12;
    let oct = i / 12 - 1;
    format!("{}{}", NOTE_NAMES[pc as usize], oct)
}

pub struct SteelCaptureApp {
    frame_rx: Receiver<CaptureFrame>,
    history: VecDeque<CaptureFrame>,
    current: Option<CaptureFrame>,
    /// Attack flash timers (seconds remaining)
    atk_flash: [f32; 10],
    fps_counter: u32,
    fps_timer: f64,
    fps_display: u32,
    /// Timestamp of last frame added to history (for decimation)
    last_history_us: u64,
    /// Accumulated attacks between decimated frames
    pending_attacks: [bool; 10],
}

impl SteelCaptureApp {
    pub fn new(frame_rx: Receiver<CaptureFrame>, _cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            frame_rx,
            history: VecDeque::with_capacity(MAX_HISTORY),
            current: None,
            atk_flash: [0.0; 10],
            fps_counter: 0,
            fps_timer: 0.0,
            fps_display: 0,
            last_history_us: 0,
            pending_attacks: [false; 10],
        }
    }

    /// Drain all pending frames from the channel.
    ///
    /// The simulator sends at 1kHz but we only need ~60fps in the
    /// history buffer. We decimate by skipping frames that arrive
    /// within HISTORY_INTERVAL_US of the last stored frame, BUT
    /// we always latch attack events so no picks/pedal changes
    /// are lost. If a skipped frame had attacks, those attacks
    /// get OR'd into the next frame we do store.
    fn poll_frames(&mut self) {
        while let Ok(frame) = self.frame_rx.try_recv() {
            // Always update flash timers and current state
            for i in 0..10 {
                if frame.attacks[i] {
                    self.atk_flash[i] = 0.4;
                    self.pending_attacks[i] = true;
                }
            }
            self.current = Some(frame.clone());

            // Decimation: only add to history at ~60fps, but always
            // add frames that have attacks (so noteheads aren't lost)
            let has_attacks = frame.attacks.iter().any(|&a| a)
                || self.pending_attacks.iter().any(|&a| a);
            let elapsed = frame.timestamp_us.saturating_sub(self.last_history_us);

            if has_attacks || elapsed >= HISTORY_INTERVAL_US {
                // Merge any pending attacks from skipped frames
                let mut store_frame = frame;
                for i in 0..10 {
                    if self.pending_attacks[i] {
                        store_frame.attacks[i] = true;
                    }
                }
                self.pending_attacks = [false; 10];

                self.history.push_back(store_frame.clone());
                self.last_history_us = store_frame.timestamp_us;
                if self.history.len() > MAX_HISTORY {
                    self.history.pop_front();
                }
            }
        }
    }

    fn draw_sidebar(&mut self, ui: &mut egui::Ui) {
        let s = match &self.current {
            Some(f) => f,
            None => {
                ui.label("Waiting for data...");
                return;
            }
        };

        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);

        // Bar Position
        ui.add_space(4.0);
        ui.colored_label(egui::Color32::from_rgb(68, 68, 68), "BAR POSITION");
        ui.separator();

        let bar_text = match s.bar_position {
            Some(p) => format!("{:.2}", p),
            None => "---".to_string(),
        };
        let bar_color = if s.bar_position.is_some() { BAR_COL } else { egui::Color32::from_rgb(42, 42, 58) };
        ui.colored_label(egui::Color32::from_rgb(bar_color.r(), bar_color.g(), bar_color.b()),
            egui::RichText::new(&bar_text).size(28.0).strong());

        let src_str = match s.bar_source {
            BarSource::None => "None",
            BarSource::Sensor => "Sensor",
            BarSource::Audio => "Audio",
            BarSource::Fused => "Fused",
        };
        ui.horizontal(|ui| {
            let src_col = match s.bar_source {
                BarSource::None => egui::Color32::from_rgb(51, 51, 51),
                BarSource::Sensor => egui::Color32::from_rgb(39, 174, 96),
                BarSource::Audio => egui::Color32::from_rgb(241, 196, 15),
                BarSource::Fused => BAR_COL,
            };
            ui.colored_label(src_col, src_str);
            ui.colored_label(TXT, format!("{}%", (s.bar_confidence * 100.0) as i32));
        });

        // Bar sensor readings
        ui.horizontal(|ui| {
            let frets = [0, 5, 10, 15];
            for i in 0..4 {
                let v = s.bar_sensors[i];
                let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 28.0), egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(10, 15, 20));
                let fill_h = rect.height() * v;
                let fill_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.min.x, rect.max.y - fill_h),
                    rect.max,
                );
                painter.rect_filled(fill_rect, 0.0, egui::Color32::from_rgb(39, 174, 96));
                painter.text(
                    rect.center_bottom() + egui::vec2(0.0, -2.0),
                    egui::Align2::CENTER_BOTTOM,
                    format!("f{}", frets[i]),
                    egui::FontId::proportional(8.0),
                    egui::Color32::from_rgb(68, 68, 68),
                );
            }
        });

        ui.add_space(8.0);

        // Pedals
        ui.colored_label(egui::Color32::from_rgb(68, 68, 68), "PEDALS");
        ui.separator();
        for i in 0..3 {
            ui.horizontal(|ui| {
                ui.colored_label(PED_COL, egui::RichText::new(PEDAL_NAMES[i]).size(11.0).strong());
                let (rect, _) = ui.allocate_exact_size(egui::vec2(120.0, 6.0), egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(24, 24, 40));
                let fill_w = rect.width() * s.pedals[i];
                painter.rect_filled(
                    egui::Rect::from_min_size(rect.min, egui::vec2(fill_w, rect.height())),
                    2.0,
                    PED_COL,
                );
            });
        }

        ui.add_space(8.0);

        // Knee Levers
        ui.colored_label(egui::Color32::from_rgb(68, 68, 68), "KNEE LEVERS");
        ui.separator();
        for i in 0..5 {
            ui.horizontal(|ui| {
                ui.colored_label(LEV_COL, egui::RichText::new(LEVER_NAMES[i]).size(10.0));
                let (rect, _) = ui.allocate_exact_size(egui::vec2(100.0, 5.0), egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(24, 24, 40));
                let fill_w = rect.width() * s.knee_levers[i];
                painter.rect_filled(
                    egui::Rect::from_min_size(rect.min, egui::vec2(fill_w, rect.height())),
                    2.0,
                    LEV_COL,
                );
            });
        }

        ui.add_space(8.0);

        // Volume
        ui.colored_label(egui::Color32::from_rgb(68, 68, 68), "VOLUME");
        ui.separator();
        ui.horizontal(|ui| {
            ui.colored_label(VOL_COL, egui::RichText::new("VOL").size(10.0));
            let (rect, _) = ui.allocate_exact_size(egui::vec2(100.0, 6.0), egui::Sense::hover());
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 3.0, egui::Color32::from_rgb(24, 24, 40));
            let fill_w = rect.width() * s.volume;
            painter.rect_filled(
                egui::Rect::from_min_size(rect.min, egui::vec2(fill_w, rect.height())),
                3.0,
                VOL_COL,
            );
            ui.colored_label(VOL_COL, format!("{}%", (s.volume * 100.0) as i32));
        });

        ui.add_space(8.0);

        // Strings
        ui.colored_label(egui::Color32::from_rgb(68, 68, 68), "STRINGS");
        ui.separator();
        for i in 0..10 {
            let hz = s.string_pitches_hz[i];
            let midi = hz_to_midi(hz);
            let name = midi_to_note_name(midi);
            let active = s.string_active[i];
            let flash = self.atk_flash[i] > 0.0;

            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::from_rgb(51, 51, 51),
                    egui::RichText::new(format!("{:>2}", i + 1)).size(9.0));

                // Color dot
                let (dot_rect, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
                let painter = ui.painter_at(dot_rect);
                let dot_col = if flash {
                    SC[i]
                } else {
                    egui::Color32::from_rgba_unmultiplied(SC[i].r(), SC[i].g(), SC[i].b(), 100)
                };
                painter.circle_filled(dot_rect.center(), 3.0, dot_col);

                // Note name
                let note_col = if active { SC[i] } else { egui::Color32::from_rgb(51, 51, 51) };
                ui.colored_label(note_col, egui::RichText::new(&name).size(10.0).strong());
                ui.colored_label(egui::Color32::from_rgb(51, 51, 51),
                    egui::RichText::new(format!("{:.0}", hz)).size(8.0));
            });
        }

        ui.add_space(8.0);
        ui.colored_label(egui::Color32::from_rgb(51, 51, 51),
            egui::RichText::new(format!("{}fps", self.fps_display)).size(9.0));
    }

    fn draw_staff(&self, ui: &mut egui::Ui, width: f32) {
        let height = 260.0_f32;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, BG);

        let top_m = 16.0_f32;
        let bot_m = 16.0_f32;
        let left = rect.min.x;
        let right = rect.max.x - 4.0;
        let midi_range = MIDI_HI - MIDI_LO;

        // Map MIDI to Y using full height with margin
        let midi_to_y = |midi: f64| -> f32 {
            rect.min.y + top_m + (height - top_m - bot_m) * (1.0 - (midi - MIDI_LO) / midi_range) as f32
        };

        // Subtle C-note reference lines
        for m in MIDI_LO as i32..=MIDI_HI as i32 {
            if m % 12 == 0 {
                let y = midi_to_y(m as f64);
                painter.line_segment(
                    [egui::pos2(left, y), egui::pos2(right, y)],
                    egui::Stroke::new(0.5, egui::Color32::from_rgb(14, 14, 28)),
                );
                let oct = m / 12 - 1;
                painter.text(
                    egui::pos2(left + 2.0, y - 1.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!("C{}", oct),
                    egui::FontId::monospace(7.0),
                    egui::Color32::from_rgb(34, 34, 34),
                );
            }
        }

        if self.history.len() < 2 {
            return;
        }

        let now_us = self.history.back().unwrap().timestamp_us;
        let window_us = (HISTORY_SECS * 1_000_000.0) as u64;
        let playhead_x = left + (right - left) / 2.0;
        let half_w = playhead_x - left;
        let nh_r = 5.0_f32; // notehead radius

        // Noteheads — one per attack
        for si in 0..10 {
            let col = SC[si];
            for frame in self.history.iter() {
                if !frame.attacks[si] {
                    continue;
                }
                let age = now_us.saturating_sub(frame.timestamp_us);
                if age > window_us {
                    continue;
                }
                let hz = frame.string_pitches_hz[si];
                if hz < 20.0 || frame.volume < 0.02 {
                    continue;
                }
                let midi = hz_to_midi(hz);
                let y = midi_to_y(midi);
                if y < rect.min.y - 10.0 || y > rect.max.y + 10.0 {
                    continue;
                }
                let px = left + half_w * (1.0 - age as f32 / window_us as f32);
                let fade = 1.0 - 0.3 * (age as f32 / window_us as f32);
                let vol_alpha = 0.2 + 0.8 * frame.volume;
                let alpha = (vol_alpha * fade * 255.0) as u8;
                let col_a = egui::Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha);

                // Filled circle notehead
                painter.circle_filled(egui::pos2(px, y), nh_r, col_a);

                // Note name label
                let note_class = ((midi.round() as i32 % 12) + 12) % 12;
                let label_alpha = (fade * 0.5 * 255.0) as u8;
                let label_col = egui::Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), label_alpha);
                painter.text(
                    egui::pos2(px + nh_r + 2.0, y),
                    egui::Align2::LEFT_CENTER,
                    NOTE_NAMES[note_class as usize],
                    egui::FontId::monospace(7.0),
                    label_col,
                );
            }
        }

        // Playhead cursor
        let cursor_active = self.current.as_ref().map_or(false, |s| s.bar_position.is_some() && s.volume > 0.02);
        let cursor_col = if cursor_active {
            egui::Color32::from_rgba_unmultiplied(26, 188, 156, 40)
        } else {
            egui::Color32::from_rgba_unmultiplied(80, 80, 100, 15)
        };
        // Triangle at top
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(playhead_x - 5.0, rect.min.y + 2.0),
                egui::pos2(playhead_x + 5.0, rect.min.y + 2.0),
                egui::pos2(playhead_x, rect.min.y + 10.0),
            ],
            if cursor_active { egui::Color32::from_rgba_unmultiplied(26, 188, 156, 230) }
            else { egui::Color32::from_rgba_unmultiplied(80, 80, 100, 100) },
            egui::Stroke::NONE,
        ));
        painter.line_segment(
            [egui::pos2(playhead_x, rect.min.y + 10.0), egui::pos2(playhead_x, rect.max.y)],
            egui::Stroke::new(1.0, cursor_col),
        );

        // "PITCH" label
        painter.text(
            egui::pos2(left + 2.0, rect.max.y - 4.0),
            egui::Align2::LEFT_BOTTOM,
            "PITCH",
            egui::FontId::monospace(7.0),
            egui::Color32::from_rgb(34, 34, 34),
        );
    }

    fn draw_attacks(&self, ui: &mut egui::Ui, width: f32) {
        let height = 20.0_f32;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, BG);

        if self.history.len() < 2 {
            return;
        }

        let now_us = self.history.back().unwrap().timestamp_us;
        let window_us = (HISTORY_SECS * 1_000_000.0) as u64;
        let left = rect.min.x;
        let right = rect.max.x;
        let playhead_x = left + (right - left) / 2.0;
        let half_w = playhead_x - left;

        // Draw attack markers on timeline
        for frame in self.history.iter() {
            let age = now_us.saturating_sub(frame.timestamp_us);
            if age > window_us {
                continue;
            }
            let px = left + half_w * (1.0 - age as f32 / window_us as f32);
            for i in 0..10 {
                if frame.attacks[i] {
                    let sy = rect.min.y + 2.0 + (i as f32 / 10.0) * (height - 4.0);
                    let bh = ((height - 4.0) / 10.0 - 1.0).max(1.0);
                    let fade = 1.0 - 0.5 * (age as f32 / window_us as f32);
                    let alpha = (fade * 0.8 * 255.0) as u8;
                    let col = egui::Color32::from_rgba_unmultiplied(SC[i].r(), SC[i].g(), SC[i].b(), alpha);
                    painter.rect_filled(
                        egui::Rect::from_min_size(egui::pos2(px - 1.0, sy), egui::vec2(3.0, bh)),
                        0.0,
                        col,
                    );
                }
            }
        }

        // Current attack flash at playhead
        for i in 0..10 {
            if self.atk_flash[i] > 0.0 {
                let sy = rect.min.y + 2.0 + (i as f32 / 10.0) * (height - 4.0);
                let bh = ((height - 4.0) / 10.0 - 1.0).max(1.0);
                let alpha = (self.atk_flash[i] * 2.0 * 255.0).min(255.0) as u8;
                let col = egui::Color32::from_rgba_unmultiplied(SC[i].r(), SC[i].g(), SC[i].b(), alpha);
                painter.rect_filled(
                    egui::Rect::from_min_size(egui::pos2(playhead_x - 3.0, sy), egui::vec2(7.0, bh)),
                    0.0,
                    col,
                );
            }
        }

        // Cursor line
        painter.line_segment(
            [egui::pos2(playhead_x, rect.min.y), egui::pos2(playhead_x, rect.max.y)],
            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(26, 188, 156, 76)),
        );

        // Label
        painter.text(
            egui::pos2(left + 2.0, rect.max.y - 3.0),
            egui::Align2::LEFT_BOTTOM,
            "ATTACKS",
            egui::FontId::monospace(7.0),
            egui::Color32::from_rgb(85, 85, 85),
        );
    }

    fn draw_tab(&self, ui: &mut egui::Ui, width: f32) {
        let height = 140.0_f32;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, BG);

        let n_strings = 10;
        let top_m = 6.0;
        let bot_m = 10.0;
        let sh = (height - top_m - bot_m) / n_strings as f32;
        let left = rect.min.x;
        let right = rect.max.x - 6.0;

        // String lines and labels
        for i in 0..n_strings {
            let y = rect.min.y + top_m + i as f32 * sh + sh / 2.0;
            painter.line_segment(
                [egui::pos2(left, y), egui::pos2(right, y)],
                egui::Stroke::new(0.8, egui::Color32::from_rgb(20, 20, 37)),
            );
            painter.text(
                egui::pos2(left + 12.0, y),
                egui::Align2::RIGHT_CENTER,
                format!("{}", i + 1),
                egui::FontId::monospace(7.0),
                egui::Color32::from_rgb(51, 51, 51),
            );
            // Color dot
            let dot_col = egui::Color32::from_rgba_unmultiplied(SC[i].r(), SC[i].g(), SC[i].b(), 100);
            painter.circle_filled(egui::pos2(left + 4.0, y), 2.0, dot_col);
        }

        if self.history.len() < 2 {
            return;
        }

        let now_us = self.history.back().unwrap().timestamp_us;
        let window_us = (HISTORY_SECS * 1_000_000.0) as u64;
        let playhead_x = left + (right - left) / 2.0;
        let half_w = playhead_x - left;

        // Draw fret numbers at attacks
        for si in 0..n_strings {
            let col = SC[si];
            let sy = rect.min.y + top_m + si as f32 * sh + sh / 2.0;

            for frame in self.history.iter() {
                if !frame.attacks[si] {
                    continue;
                }
                let age = now_us.saturating_sub(frame.timestamp_us);
                if age > window_us {
                    continue;
                }
                let fret = match frame.bar_position {
                    Some(p) => (p * 2.0).round() / 2.0,
                    None => continue,
                };
                let px = left + half_w * (1.0 - age as f32 / window_us as f32);
                let fade = 1.0 - 0.25 * (age as f32 / window_us as f32);
                let vol_alpha = 0.3 + 0.7 * frame.volume;
                let alpha = (vol_alpha * fade * 0.9 * 255.0) as u8;
                let col_a = egui::Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha);

                let fret_str = if fret.fract() == 0.0 {
                    format!("{}", fret as i32)
                } else {
                    format!("{:.1}", fret)
                };

                painter.text(
                    egui::pos2(px, sy + sh * 0.05),
                    egui::Align2::CENTER_CENTER,
                    &fret_str,
                    egui::FontId::monospace(sh * 0.55),
                    col_a,
                );

                // Pedal/lever annotations
                let mut ann = String::new();
                for j in 0..3 {
                    if frame.pedals[j] > 0.5 {
                        ann.push_str(PEDAL_NAMES[j]);
                    }
                }
                for j in 0..5 {
                    if frame.knee_levers[j] > 0.5 {
                        if !ann.is_empty() {
                            ann.push('+');
                        }
                        ann.push_str(LEVER_NAMES[j]);
                    }
                }
                if !ann.is_empty() {
                    let ann_alpha = (vol_alpha * fade * 0.55 * 255.0) as u8;
                    let ann_col = egui::Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), ann_alpha);
                    painter.text(
                        egui::pos2(px, sy + sh * 0.42),
                        egui::Align2::CENTER_CENTER,
                        &ann,
                        egui::FontId::monospace(sh * 0.35),
                        ann_col,
                    );
                }
            }
        }

        // Playhead
        let cursor_active = self.current.as_ref().map_or(false, |s| s.bar_position.is_some() && s.volume > 0.02);
        let cursor_col = if cursor_active {
            egui::Color32::from_rgba_unmultiplied(26, 188, 156, 40)
        } else {
            egui::Color32::from_rgba_unmultiplied(80, 80, 100, 15)
        };
        painter.line_segment(
            [egui::pos2(playhead_x, rect.min.y), egui::pos2(playhead_x, rect.max.y)],
            egui::Stroke::new(1.0, cursor_col),
        );

        // Time grid
        for s in (2..HISTORY_SECS as i32).step_by(2) {
            let px = left + half_w * (1.0 - s as f32 / HISTORY_SECS as f32);
            if px > left {
                painter.line_segment(
                    [egui::pos2(px, rect.min.y + top_m), egui::pos2(px, rect.max.y - bot_m)],
                    egui::Stroke::new(0.5, GRID_LINE),
                );
            }
        }

        // Label
        painter.text(
            egui::pos2(left + 2.0, rect.max.y - 2.0),
            egui::Align2::LEFT_BOTTOM,
            "TAB",
            egui::FontId::monospace(7.0),
            egui::Color32::from_rgb(34, 34, 34),
        );
    }

    fn draw_piano_roll(&self, ui: &mut egui::Ui, width: f32, height: f32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, BG);

        let midi_range = MIDI_HI - MIDI_LO;
        let left = rect.min.x;
        let right = rect.max.x;

        // Grid lines at C and E
        for m in MIDI_LO as i32..=MIDI_HI as i32 {
            let pc = NOTE_NAMES[((m % 12 + 12) % 12) as usize];
            if pc == "C" || pc == "E" {
                let y = rect.max.y - ((m as f64 - MIDI_LO) / midi_range) as f32 * rect.height();
                painter.line_segment(
                    [egui::pos2(left, y), egui::pos2(right, y)],
                    egui::Stroke::new(0.5, egui::Color32::from_rgb(15, 15, 26)),
                );
            }
        }

        if self.history.len() < 2 {
            return;
        }

        let now_us = self.history.back().unwrap().timestamp_us;
        let window_us = (HISTORY_SECS * 1_000_000.0) as u64;
        let playhead_x = left + (right - left) / 2.0;
        let half_w = playhead_x - left;

        // Attack-only dots
        for si in 0..10 {
            let col = SC[si];
            for frame in self.history.iter() {
                if !frame.attacks[si] {
                    continue;
                }
                let age = now_us.saturating_sub(frame.timestamp_us);
                if age > window_us {
                    continue;
                }
                let hz = frame.string_pitches_hz[si];
                if hz < 20.0 {
                    continue;
                }
                let midi = hz_to_midi(hz);
                if midi < MIDI_LO || midi > MIDI_HI {
                    continue;
                }
                let px = left + half_w * (1.0 - age as f32 / window_us as f32);
                let py = rect.max.y - ((midi - MIDI_LO) / midi_range) as f32 * rect.height();
                let fade = 1.0 - 0.3 * (age as f32 / window_us as f32);
                let vol_alpha = 0.2 + 0.8 * frame.volume;
                let alpha = (vol_alpha * fade * 0.85 * 255.0) as u8;
                let col_a = egui::Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha);
                painter.circle_filled(egui::pos2(px, py), 3.0, col_a);
            }
        }

        // Playhead
        let cursor_active = self.current.as_ref().map_or(false, |s| s.bar_position.is_some() && s.volume > 0.02);
        let cursor_col = if cursor_active {
            egui::Color32::from_rgba_unmultiplied(26, 188, 156, 40)
        } else {
            egui::Color32::from_rgba_unmultiplied(80, 80, 100, 15)
        };
        painter.line_segment(
            [egui::pos2(playhead_x, rect.min.y), egui::pos2(playhead_x, rect.max.y)],
            egui::Stroke::new(1.0, cursor_col),
        );
    }
}

impl eframe::App for SteelCaptureApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll for new frames from the coordinator
        self.poll_frames();

        // Decay flash timers
        let dt = ctx.input(|i| i.predicted_dt) as f32;
        for i in 0..10 {
            if self.atk_flash[i] > 0.0 {
                self.atk_flash[i] = (self.atk_flash[i] - dt).max(0.0);
            }
        }

        // FPS counter
        self.fps_counter += 1;
        self.fps_timer += dt as f64;
        if self.fps_timer >= 0.5 {
            self.fps_display = (self.fps_counter as f64 / self.fps_timer) as u32;
            self.fps_counter = 0;
            self.fps_timer = 0.0;
        }

        // Set dark background
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = PANEL_BG;
        visuals.window_fill = PANEL_BG;
        visuals.extreme_bg_color = BG;
        ctx.set_visuals(visuals);

        // Sidebar
        egui::SidePanel::left("sidebar")
            .exact_width(220.0)
            .resizable(false)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.draw_sidebar(ui);
                });
            });

        // Main visualization area
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG))
            .show(ctx, |ui| {
                let avail = ui.available_size();
                let w = avail.x;

                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 1.0);

                    self.draw_staff(ui, w);

                    // Separator
                    let (sep, _) = ui.allocate_exact_size(egui::vec2(w, 1.0), egui::Sense::hover());
                    ui.painter_at(sep).rect_filled(sep, 0.0, BORDER);

                    self.draw_attacks(ui, w);

                    let (sep2, _) = ui.allocate_exact_size(egui::vec2(w, 1.0), egui::Sense::hover());
                    ui.painter_at(sep2).rect_filled(sep2, 0.0, BORDER);

                    self.draw_tab(ui, w);

                    let (sep3, _) = ui.allocate_exact_size(egui::vec2(w, 1.0), egui::Sense::hover());
                    ui.painter_at(sep3).rect_filled(sep3, 0.0, BORDER);

                    // Piano roll takes remaining space
                    let remaining = (avail.y - 260.0 - 20.0 - 140.0 - 4.0).max(60.0);
                    self.draw_piano_roll(ui, w, remaining);
                });
            });

        // Request continuous repainting for animation
        ctx.request_repaint();
    }
}

/// Launch the GUI application with a frame receiver
pub fn run(frame_rx: Receiver<CaptureFrame>) -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([800.0, 500.0])
            .with_title("Steel Capture"),
        ..Default::default()
    };

    eframe::run_native(
        "Steel Capture",
        options,
        Box::new(move |cc| {
            Ok(Box::new(SteelCaptureApp::new(frame_rx, cc)))
        }),
    )
}
