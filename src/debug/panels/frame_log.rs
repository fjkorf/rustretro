use bevy_egui::egui;
use std::sync::{Arc, Mutex};
use crate::debug::DebugState;

pub struct FrameLog {
    filter: String,
    auto_scroll: bool,
}

impl FrameLog {
    pub fn new() -> Self {
        FrameLog { filter: String::new(), auto_scroll: true }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &Arc<Mutex<DebugState>>) {
        let log = { state.lock().unwrap().event_log.clone() };

        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.add(egui::TextEdit::singleline(&mut self.filter).desired_width(200.0));
            if ui.button("✕ Clear").clicked() { self.filter.clear(); }
            ui.separator();
            ui.checkbox(&mut self.auto_scroll, "Auto-scroll");
            ui.separator();
            ui.label(format!("{} entries", log.len()));
            if ui.button("🗑 Clear log").clicked() {
                state.lock().unwrap().event_log.clear();
            }
        });
        ui.separator();

        let filter_lower = self.filter.to_lowercase();
        let filtered: Vec<&str> = log.iter()
            .filter(|e| filter_lower.is_empty() || e.to_lowercase().contains(&filter_lower))
            .map(|s| s.as_str())
            .collect();

        let mut scroll = egui::ScrollArea::vertical().auto_shrink(false);
        if self.auto_scroll {
            scroll = scroll.stick_to_bottom(true);
        }
        scroll.show(ui, |ui| {
            for entry in &filtered {
                let color = if entry.contains("ERR") || entry.contains("error") {
                    egui::Color32::from_rgb(255, 100, 100)
                } else if entry.contains("WARN") {
                    egui::Color32::YELLOW
                } else if entry.contains("AV") || entry.contains("fmt") {
                    egui::Color32::from_rgb(150, 200, 255)
                } else {
                    egui::Color32::LIGHT_GRAY
                };
                ui.label(egui::RichText::new(*entry).monospace().size(11.0).color(color));
            }
        });
    }
}
