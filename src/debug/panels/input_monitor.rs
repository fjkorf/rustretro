use eframe::egui;
use std::sync::{Arc, Mutex};
use crate::debug::DebugState;

const BUTTON_NAMES: [&str; 12] = ["B", "Y", "SEL", "START", "↑", "↓", "←", "→", "A", "X", "L", "R"];

pub struct InputMonitor;

impl InputMonitor {
    pub fn new() -> Self { InputMonitor }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &Arc<Mutex<DebugState>>) {
        let (current, history, frame) = {
            let s = state.lock().unwrap();
            (s.input_state, s.input_history.clone(), s.frame_count)
        };

        // Live button display
        ui.heading("Live Buttons");
        ui.horizontal_wrapped(|ui| {
            for (i, name) in BUTTON_NAMES.iter().enumerate() {
                let pressed = current[i];
                let color = if pressed {
                    egui::Color32::from_rgb(80, 220, 80)
                } else {
                    egui::Color32::from_rgb(60, 60, 60)
                };
                let text_color = if pressed { egui::Color32::BLACK } else { egui::Color32::GRAY };
                egui::Frame::default()
                    .fill(color)
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(*name).monospace().color(text_color));
                    });
            }
        });

        ui.separator();

        // Last press per button
        ui.heading("Last Press (frame #)");
        ui.horizontal_wrapped(|ui| {
            for (btn, name) in BUTTON_NAMES.iter().enumerate() {
                let last = history.iter().rev()
                    .find(|(_, s)| s[btn])
                    .map(|(f, _)| f.to_string())
                    .unwrap_or_else(|| "-".to_string());
                ui.label(format!("{name}:{last}"));
                ui.separator();
            }
        });

        ui.separator();

        // Timeline grid: last 60 frames × 12 buttons
        ui.heading(format!("Input Timeline (last {} frames @ frame {})", history.len(), frame));
        egui::ScrollArea::horizontal().show(ui, |ui| {
            // Column headers
            ui.horizontal(|ui| {
                ui.add_space(30.0); // frame# column
                for name in &BUTTON_NAMES {
                    ui.label(egui::RichText::new(*name).monospace().size(9.0));
                    ui.add_space(2.0);
                }
            });

            for (f, btns) in &history {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("{f:5}")).monospace().size(9.0));
                    for pressed in btns.iter() {
                        let color = if *pressed {
                            egui::Color32::from_rgb(80, 220, 80)
                        } else {
                            egui::Color32::from_rgb(40, 40, 40)
                        };
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(14.0, 12.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(rect, 1.0, color);
                    }
                });
            }
        });
    }
}
