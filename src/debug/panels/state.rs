use bevy_egui::egui;
use std::path::PathBuf;
use std::time::Instant;

use crate::debug::{DebugState, StateOp};

/// How long slot-file metadata stays cached before re-statting (the panel
/// renders every UI frame; 9 × fs::metadata at 60+ Hz is pointless work).
const SLOT_REFRESH_SECS: f64 = 1.0;

/// One slot's on-disk facts, refreshed lazily.
#[derive(Clone, Default)]
struct SlotInfo {
    exists: bool,
    bytes: u64,
    /// Modification time rendered at refresh (avoids per-frame formatting).
    modified: String,
}

/// Save-state panel: slot grid + free-path ops over the existing
/// `pending_state_op` queue. It never touches `state_op_result` (that is the
/// MCP poller's consumable) — status comes from the sticky `state_note`.
pub struct StatePanel {
    slots: Vec<SlotInfo>,
    last_refresh: Option<Instant>,
    /// The state_note the current cache was built under; a new note means an
    /// op just completed, so slot files may have changed.
    cached_note: Option<String>,
    path_input: String,
}

impl StatePanel {
    pub fn new() -> Self {
        StatePanel {
            slots: vec![SlotInfo::default(); 9],
            last_refresh: None,
            cached_note: None,
            path_input: String::new(),
        }
    }

    fn slot_path(state: &DebugState, slot: u8) -> Option<PathBuf> {
        let dir = state.state_dir.as_ref()?;
        let stem = state.rom_name.as_deref().unwrap_or("game");
        Some(crate::frontend::state_slot_path(dir, stem, slot))
    }

    fn refresh_slots(&mut self, state: &DebugState) {
        for (i, info) in self.slots.iter_mut().enumerate() {
            *info = SlotInfo::default();
            let Some(path) = Self::slot_path(state, (i + 1) as u8) else { continue };
            if let Ok(meta) = std::fs::metadata(&path) {
                info.exists = true;
                info.bytes = meta.len();
                if let Ok(mtime) = meta.modified() {
                    let age = mtime.elapsed().unwrap_or_default().as_secs();
                    info.modified = if age < 60 {
                        format!("{age}s ago")
                    } else if age < 3600 {
                        format!("{}m ago", age / 60)
                    } else if age < 86400 {
                        format!("{}h ago", age / 3600)
                    } else {
                        format!("{}d ago", age / 86400)
                    };
                }
            }
        }
        self.last_refresh = Some(Instant::now());
        self.cached_note = state.state_note.clone();
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut DebugState) {
        let stale = self
            .last_refresh
            .map(|t| t.elapsed().as_secs_f64() > SLOT_REFRESH_SECS)
            .unwrap_or(true)
            || self.cached_note != state.state_note;
        if stale {
            self.refresh_slots(state);
        }

        ui.heading("💾 Save states");
        ui.separator();

        if state.state_dir.is_none() {
            ui.label("State directory unknown (frontend not initialized yet).");
            return;
        }

        // ── Slot grid ─────────────────────────────────────────────────
        egui::Grid::new("state_slots")
            .num_columns(5)
            .spacing([12.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Slot").strong());
                ui.label(egui::RichText::new("On disk").strong());
                ui.label(egui::RichText::new("Saved").strong());
                ui.label("");
                ui.label("");
                ui.end_row();
                for i in 0..9u8 {
                    let slot = i + 1;
                    let info = &self.slots[i as usize];
                    let hotkey = match slot {
                        1 => " (F6/F7)",
                        2 => " (⇧F6/⇧F7)",
                        _ => "",
                    };
                    ui.label(egui::RichText::new(format!("{slot}{hotkey}")).monospace());
                    if info.exists {
                        ui.label(
                            egui::RichText::new(format!("{} KB", info.bytes / 1024))
                                .monospace()
                                .color(egui::Color32::from_rgb(150, 220, 150)),
                        );
                        ui.label(egui::RichText::new(&info.modified).monospace());
                    } else {
                        ui.label(egui::RichText::new("—").color(egui::Color32::DARK_GRAY));
                        ui.label("");
                    }
                    if ui.small_button("💾 Save").clicked() {
                        state.pending_state_op = Some(StateOp::SaveSlot(slot));
                    }
                    if ui
                        .add_enabled(info.exists, egui::Button::new("📂 Load").small())
                        .clicked()
                    {
                        state.pending_state_op = Some(StateOp::LoadSlot(slot));
                    }
                    ui.end_row();
                }
            });

        ui.separator();

        // ── Free-path ops ─────────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label("Path:");
            ui.add(
                egui::TextEdit::singleline(&mut self.path_input)
                    .desired_width(260.0)
                    .hint_text("e.g. shadow/arenas/goat-vs-rosemary.state"),
            );
            let path_ok = !self.path_input.trim().is_empty();
            if ui.add_enabled(path_ok, egui::Button::new("💾 Save")).clicked() {
                state.pending_state_op =
                    Some(StateOp::Save(PathBuf::from(self.path_input.trim())));
            }
            if ui.add_enabled(path_ok, egui::Button::new("📂 Load")).clicked() {
                state.pending_state_op =
                    Some(StateOp::Load(PathBuf::from(self.path_input.trim())));
            }
        });

        ui.separator();

        // ── Status ────────────────────────────────────────────────────
        if state.pending_state_op.is_some() {
            ui.label(
                egui::RichText::new("⏳ op queued (drains at the next frame boundary)")
                    .color(egui::Color32::YELLOW),
            );
        }
        match &state.state_note {
            Some(note) => {
                let color = if note.contains("FAILED") {
                    egui::Color32::from_rgb(230, 120, 120)
                } else {
                    egui::Color32::from_rgb(150, 220, 150)
                };
                ui.label(egui::RichText::new(format!("Last: {note}")).color(color));
            }
            None => {
                ui.label(
                    egui::RichText::new("No state ops yet this session.")
                        .color(egui::Color32::DARK_GRAY),
                );
            }
        }
    }
}
