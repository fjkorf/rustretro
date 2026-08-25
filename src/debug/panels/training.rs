use bevy_egui::egui;

use crate::debug::{DebugState, DummyMode};

/// GUI face of the training mode (F1–F5) and the shadow bot (Shift+F5): the
/// widgets read and write the same `TrainingConfig` the hotkeys do, so the
/// panel doubles as the first visible readout of the current training state
/// (previously stderr-only).
pub struct TrainingPanel;

const DUMMY_MODES: [(DummyMode, &str); 5] = [
    (DummyMode::Free, "Free (human / shadow drives P2)"),
    (DummyMode::Stand, "Stand"),
    (DummyMode::Crouch, "Crouch"),
    (DummyMode::Jump, "Jump (hop cadence)"),
    (DummyMode::Block, "Block (hold away)"),
];

impl TrainingPanel {
    pub fn new() -> Self {
        TrainingPanel
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut DebugState) {
        ui.heading("🎯 Training mode");
        ui.separator();

        let was_enabled = state.training.enabled;
        ui.checkbox(&mut state.training.enabled, "Enabled (F5)")
            .on_hover_text("Credits topped up, round timer held, health refill — the held-fight sandbox");
        if state.training.enabled && !was_enabled {
            // Parity with the F5 hotkey: enabling turns refill on.
            state.training.refill = true;
        }

        ui.add_enabled_ui(state.training.enabled, |ui| {
            ui.horizontal(|ui| {
                ui.label("Dummy (F1):");
                let current = DUMMY_MODES
                    .iter()
                    .find(|(m, _)| *m == state.training.dummy)
                    .map(|(_, label)| *label)
                    .unwrap_or("?");
                egui::ComboBox::from_id_salt("training_dummy")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (mode, label) in DUMMY_MODES {
                            ui.selectable_value(&mut state.training.dummy, mode, label);
                        }
                    });
            });
            ui.checkbox(&mut state.training.refill, "Health refill (F3)");
            ui.horizontal(|ui| {
                if ui.button("↺ Reset positions (F2)").clicked() {
                    state.training.reset_positions = true;
                }
                if ui.button("🏁 Finish round (F4)").clicked() {
                    state.training.finish_round = true;
                }
            });
        });

        ui.separator();
        ui.heading("👤 Shadow bot");
        match state.shadow_on {
            None => {
                ui.label(
                    egui::RichText::new(
                        "No model loaded — launch with --shadow shadow/models/<name>",
                    )
                    .color(egui::Color32::DARK_GRAY),
                );
            }
            Some(on) => {
                ui.horizontal(|ui| {
                    let (txt, color) = if on {
                        ("● ACTIVE", egui::Color32::from_rgb(150, 220, 150))
                    } else {
                        ("○ off", egui::Color32::DARK_GRAY)
                    };
                    ui.label(egui::RichText::new(txt).color(color).strong());
                    let btn = if on { "Disable (⇧F5)" } else { "Enable (⇧F5)" };
                    if ui.button(btn).clicked() {
                        state.pending_shadow_toggle = true;
                    }
                });
                ui.label(
                    egui::RichText::new(
                        "Drives P2 (controller port 1) from the kNN model while the fight gate is open.",
                    )
                    .small()
                    .color(egui::Color32::DARK_GRAY),
                );
            }
        }
    }
}
