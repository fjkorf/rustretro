use bevy_egui::egui;
use std::path::PathBuf;
use std::time::Instant;

use crate::debug::{DebugState, DummyMode, RecordControl};

/// GUI face of the shadow lifecycle's two app-side stages — demonstrate
/// (recorder start/stop + status) and deploy (model card, runtime model load,
/// enable/disable) — plus the training-mode controls (F1–F5). The widgets
/// read and write the same `TrainingConfig`/one-shots the hotkeys do, so the
/// panel doubles as the first visible readout of state that was stderr-only.
pub struct TrainingPanel {
    /// Cached `shadow/models/*` listing (dirs containing cases.npz).
    models: Vec<(String, PathBuf)>,
    models_refreshed: Option<Instant>,
}

const DUMMY_MODES: [(DummyMode, &str); 5] = [
    (DummyMode::Free, "Free (human / shadow drives P2)"),
    (DummyMode::Stand, "Stand"),
    (DummyMode::Crouch, "Crouch"),
    (DummyMode::Jump, "Jump (hop cadence)"),
    (DummyMode::Block, "Block (hold away)"),
];

/// Where `loop.sh` fits models to / recordings come from, relative to the
/// launch cwd (the repo root by convention — same assumption loop.sh makes).
const MODELS_DIR: &str = "shadow/models";
const RECORDINGS_DIR: &str = "shadow/recordings";

const MODELS_REFRESH_SECS: f64 = 2.0;

/// Buckets far below the best-covered one are the drill signal: the model has
/// so few examples there that retrieval falls back to unrelated neighbors.
const DRILL_RATIO: f64 = 0.10;

/// epoch seconds → "YYYYMMDD-HHMM" (UTC), for auto-named recordings.
/// Date math per Howard Hinnant's civil_from_days.
fn stamp_from_secs(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let (h, m) = ((secs % 86400) / 3600, (secs % 3600) / 60);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if mth <= 2 { 1 } else { 0 };
    format!("{y:04}{mth:02}{d:02}-{h:02}{m:02}")
}

fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    stamp_from_secs(secs)
}

fn scan_models() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(MODELS_DIR) {
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() && path.join("cases.npz").is_file() {
                out.push((e.file_name().to_string_lossy().into_owned(), path));
            }
        }
    }
    out.sort();
    out
}

impl TrainingPanel {
    pub fn new() -> Self {
        TrainingPanel { models: Vec::new(), models_refreshed: None }
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
        self.record_section(ui, state);
        ui.separator();
        self.shadow_section(ui, state);
    }

    /// Demonstrate stage: recorder status + start/stop.
    fn record_section(&mut self, ui: &mut egui::Ui, state: &mut DebugState) {
        ui.heading("⏺ Record demonstrations");
        match &state.record_status {
            Some((path, frames)) => {
                let secs = frames / 60;
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("● REC")
                            .color(egui::Color32::from_rgb(230, 90, 90))
                            .strong(),
                    );
                    ui.monospace(format!(
                        "{}  {} frames ({}:{:02})",
                        path.file_name().map(|s| s.to_string_lossy()).unwrap_or_default(),
                        frames,
                        secs / 60,
                        secs % 60,
                    ));
                    if ui.button("⏹ Stop").clicked() {
                        state.pending_record = Some(RecordControl::Stop);
                    }
                });
            }
            None => {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("○ not recording").color(egui::Color32::DARK_GRAY));
                    if ui.button("⏺ Start").clicked() {
                        let path = PathBuf::from(RECORDINGS_DIR)
                            .join(format!("session-{}.jsonl", utc_stamp()));
                        state.pending_record = Some(RecordControl::Start(path));
                    }
                });
                ui.label(
                    egui::RichText::new(
                        "Your play becomes training data — record every session you want the shadow to learn from.",
                    )
                    .small()
                    .color(egui::Color32::DARK_GRAY),
                );
            }
        }
        if let Some(note) = &state.record_note {
            let color = if note.contains("FAILED") {
                egui::Color32::from_rgb(230, 120, 120)
            } else {
                egui::Color32::GRAY
            };
            ui.label(egui::RichText::new(format!("Last: {note}")).small().color(color));
        }
    }

    /// Deploy stage: loaded-model card, enable toggle, runtime model picker.
    fn shadow_section(&mut self, ui: &mut egui::Ui, state: &mut DebugState) {
        ui.heading("👤 Shadow bot");

        // ── Loaded-model card + enable toggle ─────────────────────────
        match (&state.shadow_model, state.shadow_on) {
            (Some(info), on) => {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&info.name).strong().size(16.0));
                    let (txt, color) = if on == Some(true) {
                        ("● ACTIVE", egui::Color32::from_rgb(150, 220, 150))
                    } else {
                        ("○ off", egui::Color32::DARK_GRAY)
                    };
                    ui.label(egui::RichText::new(txt).color(color).strong());
                    let btn = if on == Some(true) { "Disable (⇧F5)" } else { "Enable (⇧F5)" };
                    if ui.button(btn).clicked() {
                        state.pending_shadow_toggle = true;
                    }
                });
                let mut line = format!("{} cases", info.cases);
                if let Some(r) = info.rounds {
                    line += &format!(" / {r} rounds");
                }
                if let Some(c) = &info.created {
                    // ISO timestamp → keep the date part.
                    line += &format!(", fitted {}", c.split('T').next().unwrap_or(c));
                }
                ui.label(line);
                if !info.buckets.is_empty() {
                    let max = info.buckets.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1);
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Coverage:");
                        for (bucket, n) in &info.buckets {
                            let sparse = (*n as f64) < DRILL_RATIO * max as f64;
                            let text = format!("{bucket} {n}");
                            if sparse {
                                ui.label(
                                    egui::RichText::new(format!("{text} ⚠"))
                                        .color(egui::Color32::from_rgb(230, 180, 90)),
                                )
                                .on_hover_text("Sparse bucket — demonstrate more of this (the drill list)");
                            } else {
                                ui.monospace(text);
                            }
                        }
                    });
                }
                ui.label(
                    egui::RichText::new("Drives P2 (controller port 1) while the fight gate is open.")
                        .small()
                        .color(egui::Color32::DARK_GRAY),
                );
            }
            (None, _) => {
                ui.label(
                    egui::RichText::new("No model loaded — pick one below or launch with --shadow.")
                        .color(egui::Color32::DARK_GRAY),
                );
            }
        }
        if let Some(note) = &state.shadow_note {
            let color = if note.contains("FAILED") {
                egui::Color32::from_rgb(230, 120, 120)
            } else {
                egui::Color32::GRAY
            };
            ui.label(egui::RichText::new(format!("Last: {note}")).small().color(color));
        }

        // ── Model picker ──────────────────────────────────────────────
        let stale = self
            .models_refreshed
            .map(|t| t.elapsed().as_secs_f64() > MODELS_REFRESH_SECS)
            .unwrap_or(true);
        if stale {
            self.models = scan_models();
            self.models_refreshed = Some(Instant::now());
        }
        egui::CollapsingHeader::new(format!("Models ({})", self.models.len()))
            .default_open(false)
            .show(ui, |ui| {
                if self.models.is_empty() {
                    ui.label(format!("Nothing under {MODELS_DIR}/ — run shadow/loop.sh --fit-only."));
                }
                for (name, path) in &self.models {
                    ui.horizontal(|ui| {
                        let loaded = state
                            .shadow_model
                            .as_ref()
                            .map(|i| i.name == *name)
                            .unwrap_or(false);
                        ui.monospace(name);
                        if loaded {
                            ui.label(egui::RichText::new("(loaded)").color(egui::Color32::DARK_GRAY));
                        } else if ui.small_button("Load").clicked() {
                            state.pending_shadow_load = Some(path.clone());
                        }
                    });
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::stamp_from_secs;

    #[test]
    fn stamp_matches_python_strftime_goldens() {
        assert_eq!(stamp_from_secs(0), "19700101-0000");
        assert_eq!(stamp_from_secs(1756123440), "20250825-1204");
        assert_eq!(stamp_from_secs(4102444799), "20991231-2359");
    }
}
