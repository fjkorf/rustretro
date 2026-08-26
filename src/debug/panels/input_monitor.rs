use bevy_egui::egui;
use std::sync::{Arc, Mutex};
use crate::debug::DebugState;

/// RETRO_DEVICE_ID_JOYPAD raw names, in bit-index order. Fallback label when
/// the core sends no `RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS` entry for a
/// button (fbalpha2012 sends none at all — see docs/game-profiles.md's
/// Controls contract).
const RETRO_NAMES: [&str; 12] =
    ["B", "Y", "Select", "Start", "Up", "Down", "Left", "Right", "A", "X", "L", "R"];

/// Longest label kept whole in the compact per-button grid before eliding —
/// core descriptors like "Weak attack" run long, so the full name only shows
/// on hover.
const GRID_LABEL_MAX: usize = 10;

pub struct InputMonitor;

impl InputMonitor {
    pub fn new() -> Self { InputMonitor }

    /// Resolve RETRO id `i`'s live display label for `port` (0/1): the
    /// core's descriptor if present, else the raw RETRO name. Returns
    /// (short-for-grid, full-for-hover).
    fn label(descriptors: &[[Option<String>; 12]; 2], port: usize, i: usize) -> (String, String) {
        let full = descriptors[port][i]
            .clone()
            .unwrap_or_else(|| RETRO_NAMES[i].to_string());
        let short = if full.chars().count() > GRID_LABEL_MAX {
            let head: String = full.chars().take(GRID_LABEL_MAX.saturating_sub(1)).collect();
            format!("{head}…")
        } else {
            full.clone()
        };
        (short, full)
    }

    fn button_grid(ui: &mut egui::Ui, state: &[bool; 12], descriptors: &[[Option<String>; 12]; 2], port: usize) {
        ui.horizontal_wrapped(|ui| {
            for i in 0..12 {
                let (short, full) = Self::label(descriptors, port, i);
                let pressed = state[i];
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
                        ui.label(egui::RichText::new(short).monospace().color(text_color));
                    })
                    .response
                    .on_hover_text(full);
            }
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &Arc<Mutex<DebugState>>) {
        let (current, current2, history, frame, descriptors) = {
            let s = state.lock().unwrap();
            (s.input_state, s.input_state2, s.input_history.clone(), s.frame_count, s.input_descriptors.clone())
        };

        // Live button display — both ports, each labeled from its own
        // core descriptors (falls back to raw RETRO names per-button).
        ui.heading("Live Buttons");
        ui.label(egui::RichText::new("P1").strong());
        Self::button_grid(ui, &current, &descriptors, 0);
        ui.label(egui::RichText::new("P2").strong());
        Self::button_grid(ui, &current2, &descriptors, 1);

        ui.separator();

        // Last press per button (P1 only: the rolling history is P1-only).
        ui.heading("Last Press (frame #) — P1");
        ui.horizontal_wrapped(|ui| {
            for btn in 0..12 {
                let (short, full) = Self::label(&descriptors, 0, btn);
                let last = history.iter().rev()
                    .find(|(_, s)| s[btn])
                    .map(|(f, _)| f.to_string())
                    .unwrap_or_else(|| "-".to_string());
                ui.label(format!("{short}:{last}")).on_hover_text(full);
                ui.separator();
            }
        });

        ui.separator();

        // Timeline grid: last 60 frames × 12 buttons (P1; ids and semantics
        // unchanged — only the column labels now resolve through the name
        // chain).
        ui.heading(format!("Input Timeline (last {} frames @ frame {})", history.len(), frame));
        egui::ScrollArea::horizontal().show(ui, |ui| {
            // Column headers
            ui.horizontal(|ui| {
                ui.add_space(30.0); // frame# column
                for i in 0..12 {
                    let (short, full) = Self::label(&descriptors, 0, i);
                    ui.label(egui::RichText::new(short).monospace().size(9.0))
                        .on_hover_text(full);
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
