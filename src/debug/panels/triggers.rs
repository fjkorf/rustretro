use eframe::egui;
use std::sync::{Arc, Mutex};
use crate::debug::DebugState;

const BUTTON_NAMES: [&str; 12] = ["B", "Y", "SEL", "START", "↑", "↓", "←", "→", "A", "X", "L", "R"];

pub struct Triggers {
    frame_input: String,
    pixel_x: String,
    pixel_y: String,
    input_btn: usize,
}

impl Triggers {
    pub fn new() -> Self {
        Triggers {
            frame_input: String::new(),
            pixel_x: String::new(),
            pixel_y: String::new(),
            input_btn: 0,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &Arc<Mutex<DebugState>>) {
        let (paused, trigger_frame, trigger_pixel, frame_count) = {
            let s = state.lock().unwrap();
            (s.paused, s.trigger_frame, s.trigger_pixel, s.frame_count)
        };

        // --- Manual controls ---
        ui.heading("Manual Control");
        ui.horizontal(|ui| {
            if paused {
                if ui.button("▶ Resume").clicked() {
                    state.lock().unwrap().paused = false;
                }
                if ui.button("⏭ Step 1 Frame").clicked() {
                    let mut s = state.lock().unwrap();
                    s.step_one = true;
                    s.paused = true;
                }
            } else {
                if ui.button("⏸ Pause").clicked() {
                    state.lock().unwrap().paused = true;
                }
            }
            ui.separator();
            ui.label(egui::RichText::new(
                if paused { format!("⏸ PAUSED @ frame {frame_count}") }
                else { format!("▶ Running @ frame {frame_count}") }
            ).color(if paused { egui::Color32::YELLOW } else { egui::Color32::GREEN }));
        });

        ui.separator();

        // --- Pause at frame N ---
        ui.heading("Pause at Frame");
        ui.horizontal(|ui| {
            ui.label("Frame #:");
            ui.add(egui::TextEdit::singleline(&mut self.frame_input).desired_width(100.0));
            let cur = trigger_frame.map(|f| f.to_string()).unwrap_or_default();
            if ui.button("Set").clicked() {
                if let Ok(n) = self.frame_input.trim().parse::<u64>() {
                    state.lock().unwrap().trigger_frame = Some(n);
                }
            }
            if ui.button("Clear").clicked() {
                state.lock().unwrap().trigger_frame = None;
            }
            if let Some(f) = trigger_frame {
                ui.label(egui::RichText::new(format!("Active: pause @ {f}")).color(egui::Color32::YELLOW));
            }
        });

        ui.separator();

        // --- Pause on pixel change ---
        ui.heading("Pause When Pixel Changes");
        ui.horizontal(|ui| {
            ui.label("X:");
            ui.add(egui::TextEdit::singleline(&mut self.pixel_x).desired_width(60.0));
            ui.label("Y:");
            ui.add(egui::TextEdit::singleline(&mut self.pixel_y).desired_width(60.0));
            if ui.button("Set").clicked() {
                if let (Ok(x), Ok(y)) = (
                    self.pixel_x.trim().parse::<u32>(),
                    self.pixel_y.trim().parse::<u32>(),
                ) {
                    state.lock().unwrap().trigger_pixel = Some((x, y));
                }
            }
            if ui.button("Clear").clicked() {
                state.lock().unwrap().trigger_pixel = None;
            }
            if let Some((x, y)) = trigger_pixel {
                ui.label(egui::RichText::new(format!("Active: watch ({x},{y})")).color(egui::Color32::YELLOW));
            }
        });

        ui.separator();

        // --- Pause on input event ---
        ui.heading("Pause on Button Press");
        ui.horizontal(|ui| {
            egui::ComboBox::from_label("Button")
                .selected_text(BUTTON_NAMES[self.input_btn])
                .show_ui(ui, |ui| {
                    for (i, name) in BUTTON_NAMES.iter().enumerate() {
                        ui.selectable_value(&mut self.input_btn, i, *name);
                    }
                });
            if ui.button("Pause on next press").clicked() {
                // Store as negative frame offset convention: we use a special
                // trigger_frame value of u64::MAX - btn_index to signal button trigger
                state.lock().unwrap().trigger_frame = Some(u64::MAX - self.input_btn as u64);
            }
        });

        ui.separator();
        ui.weak("Triggers are checked in the emulation run loop each frame.");
    }
}
