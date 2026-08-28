use bevy_egui::egui;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use crate::debug::{DebugState, DummyMode, RecordControl, StateOp};

/// GUI face of the shadow lifecycle's two app-side stages — demonstrate
/// (recorder start/stop + status) and deploy (model card, runtime model load,
/// enable/disable) — plus the training-mode controls (F1–F5). The widgets
/// read and write the same `TrainingConfig`/one-shots the hotkeys do, so the
/// panel doubles as the first visible readout of state that was stderr-only.
pub struct TrainingPanel {
    /// Cached `shadow/models/*` listing (dirs containing cases.npz).
    models: Vec<(String, PathBuf)>,
    models_refreshed: Option<Instant>,
    /// Cached `shadow/arenas/*.state` listing: (display name, path, age).
    arenas: Vec<(String, PathBuf, String)>,
    arenas_refreshed: Option<Instant>,
    /// "Save new arena" name field.
    arena_name: String,
    /// Make the newly saved arena the current one.
    arena_make_current: bool,
    /// Deferred make-current: the queued StateOp::Save drains on the emu
    /// thread, so the copy to current.state must wait until the named file
    /// actually lands (mtime after the request). (target, requested-at).
    pending_current: Option<(PathBuf, SystemTime)>,
    /// Sticky result of the last make-current copy.
    arena_note: Option<String>,
    /// Style tag for the next recording ("rushdown", "zoning", …; empty = untagged).
    record_style: String,
}

const DUMMY_MODES: [(DummyMode, &str); 6] = [
    (DummyMode::Free, "Free (human / shadow drives P2)"),
    (DummyMode::Stand, "Stand"),
    (DummyMode::Crouch, "Crouch"),
    (DummyMode::Jump, "Jump (hop cadence)"),
    (DummyMode::Block, "Block (hold away)"),
    (DummyMode::BlockPunish, "Block + punish (on contact)"),
];

/// ContinueBlock hold length offered by the panel (MACRO_ACTIONS §6).
const PUNISH_CONTINUE_FRAMES: u16 = 30;

/// Pool identity ignores ContinueBlock's frame payload — the panel offers
/// one Continue Block row, whatever N an older pool stored.
fn same_option(a: &crate::macros::PunishOption, b: &crate::macros::PunishOption) -> bool {
    use crate::macros::PunishOption::*;
    matches!((a, b), (ContinueBlock(_), ContinueBlock(_))) || a == b
}

/// Shadow data roots are PER-FAMILY (`shadow/<kind>/<family>/`) so one
/// game's models/recordings/arenas never appear under another (QA-found:
/// MK2's panel offered goat's brain). Relative to the launch cwd (repo
/// root by convention — same assumption loop.sh makes).
fn models_dir() -> PathBuf {
    PathBuf::from("shadow/models").join(&crate::profile::current().family.family)
}
fn recordings_dir() -> PathBuf {
    PathBuf::from("shadow/recordings").join(&crate::profile::current().family.family)
}
fn arenas_dir() -> PathBuf {
    PathBuf::from("shadow/arenas").join(&crate::profile::current().family.family)
}
/// The active training save: loop.sh starts fights from this if it exists
/// (ARENA env still wins). Machine-local — gitignored, unlike named arenas.
const CURRENT_ARENA: &str = "current";

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
    if let Ok(entries) = std::fs::read_dir(models_dir()) {
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

fn age_str(mtime: SystemTime) -> String {
    let age = mtime.elapsed().unwrap_or_default().as_secs();
    if age < 60 {
        format!("{age}s ago")
    } else if age < 3600 {
        format!("{}m ago", age / 60)
    } else if age < 86400 {
        format!("{}h ago", age / 3600)
    } else {
        format!("{}d ago", age / 86400)
    }
}

/// `shadow/arenas/*.state`, `current` first, then alphabetical.
fn scan_arenas() -> Vec<(String, PathBuf, String)> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(arenas_dir()) {
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("state") {
                continue;
            }
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let age = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .map(age_str)
                .unwrap_or_default();
            out.push((name, path, age));
        }
    }
    out.sort_by(|a, b| {
        (a.0 != CURRENT_ARENA).cmp(&(b.0 != CURRENT_ARENA)).then(a.0.cmp(&b.0))
    });
    out
}

impl TrainingPanel {
    pub fn new() -> Self {
        TrainingPanel {
            models: Vec::new(),
            models_refreshed: None,
            arenas: Vec::new(),
            arenas_refreshed: None,
            arena_name: String::new(),
            arena_make_current: true,
            pending_current: None,
            arena_note: None,
            record_style: String::new(),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut DebugState) {
        ui.heading("🎯 Training mode");
        let profile = crate::profile::current();
        ui.label(
            egui::RichText::new(format!("{} ({})", profile.family.title, profile.port.port))
                .small()
                .color(egui::Color32::DARK_GRAY),
        );
        ui.separator();

        let Some(feats) = crate::training::features() else {
            ui.label(
                egui::RichText::new(
                    "Training unavailable — this game's profile has no in-fight gate yet \
                     (see the porting-a-game tutorial).",
                )
                .color(egui::Color32::DARK_GRAY),
            );
            ui.separator();
            self.shadow_section(ui, state);
            return;
        };
        let was_enabled = state.training.enabled;
        ui.checkbox(&mut state.training.enabled, "Enabled (F5)")
            .on_hover_text("The held-fight sandbox — every enforcement the profile maps");
        if state.training.enabled && !was_enabled {
            // Parity with the F5 hotkey: enabling turns refill on.
            state.training.refill = true;
        }
        let missing = feats.missing();
        if !missing.is_empty() {
            ui.label(
                egui::RichText::new(format!("Not mapped for this game: {}", missing.join(", ")))
                    .small()
                    .color(egui::Color32::DARK_GRAY),
            )
            .on_hover_text(
                "Partial memory map — these enforcements decline until the profile \
                 gains the addresses (see the game's .md evidence doc)",
            );
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
                            // BlockPunish needs a mapped contact signal —
                            // offered greyed with the reason otherwise.
                            if mode == DummyMode::BlockPunish && !feats.block_punish {
                                ui.add_enabled(
                                    false,
                                    egui::Button::selectable(false, label),
                                )
                                .on_disabled_hover_text(
                                    "Needs a contact signal (hitstun_sources or \
                                     contact_signal) in the profile — see the game's .md",
                                );
                                continue;
                            }
                            ui.selectable_value(&mut state.training.dummy, mode, label);
                        }
                    });
            });
            if state.training.dummy == DummyMode::BlockPunish {
                self.punish_section(ui, state, feats.block_punish);
            }
            ui.add_enabled(feats.refill, egui::Checkbox::new(&mut state.training.refill, "Health refill (F3)"));
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(feats.position_reset, egui::Button::new("↺ Reset positions (F2)"))
                    .clicked()
                {
                    state.training.reset_positions = true;
                }
                if ui
                    .add_enabled(feats.finish_round, egui::Button::new("🏁 Finish round (F4)"))
                    .clicked()
                {
                    state.training.finish_round = true;
                }
            });
        });

        ui.separator();
        self.record_section(ui, state);
        ui.separator();
        self.shadow_section(ui, state);
        ui.separator();
        self.arena_section(ui, state);
    }

    /// BlockPunish option pool (MACRO_ACTIONS §6): the char-aware legal list
    /// — dummy char → family∩port specials + base attack classes + Continue
    /// Block — with weight steppers writing straight into the sampled pool.
    fn punish_section(&mut self, ui: &mut egui::Ui, state: &mut DebugState, available: bool) {
        use crate::macros::PunishOption;
        if !available {
            ui.label(
                egui::RichText::new(
                    "Block-punish unavailable: no contact signal mapped \
                     (hitstun_sources or contact_signal) — the dummy just blocks.",
                )
                .small()
                .color(egui::Color32::DARK_GRAY),
            );
            return;
        }
        let profile = crate::profile::current();
        let char_id = crate::training::punish_dummy_char(state);

        // The legal option list, in offer order.
        let mut options: Vec<PunishOption> = Vec::new();
        if let Some(id) = char_id {
            for (name, _) in profile.specials_for(id) {
                options.push(PunishOption::Move(name.to_string()));
            }
        }
        let block_class = profile.family.block.class.as_deref();
        for class in &profile.family.attack_classes {
            let has_chord =
                profile.port.attack_chords.get(class).map(|c| !c.is_empty()).unwrap_or(false);
            if class != "None" && Some(class.as_str()) != block_class && has_chord {
                options.push(PunishOption::Attack(class.clone()));
            }
        }
        options.push(PunishOption::ContinueBlock(PUNISH_CONTINUE_FRAMES));

        // Default pool on first open: the first special w=3, continue w=1.
        if state.training.punish_pool.is_empty() {
            if let Some(first) = options.iter().find(|o| matches!(o, PunishOption::Move(_))) {
                state.training.punish_pool.push((first.clone(), 3));
            }
            state
                .training
                .punish_pool
                .push((PunishOption::ContinueBlock(PUNISH_CONTINUE_FRAMES), 1));
        }

        ui.label(
            egui::RichText::new(format!(
                "Punish pool ({}):",
                char_id.map(|id| profile.char_name(id)).unwrap_or_else(|| "char unknown".into())
            ))
            .small(),
        )
        .on_hover_text("On each guarded contact, one option is sampled by weight (weight 0 = never)");
        ui.indent("punish_pool", |ui| {
            for opt in &options {
                let old = state
                    .training
                    .punish_pool
                    .iter()
                    .find(|(o, _)| same_option(o, opt))
                    .map(|(_, w)| *w)
                    .unwrap_or(0);
                let mut w = old;
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut w).range(0..=9).speed(0.05));
                    let tag = match opt {
                        PunishOption::Move(_) => "special",
                        PunishOption::Attack(_) => "attack",
                        PunishOption::ContinueBlock(_) => "",
                    };
                    ui.monospace(opt.label());
                    if !tag.is_empty() {
                        ui.label(
                            egui::RichText::new(tag).small().color(egui::Color32::DARK_GRAY),
                        );
                    }
                });
                if w != old {
                    match state
                        .training
                        .punish_pool
                        .iter_mut()
                        .find(|(o, _)| same_option(o, opt))
                    {
                        Some(entry) => entry.1 = w,
                        None => state.training.punish_pool.push((opt.clone(), w)),
                    }
                }
            }
        });
    }

    /// The training save: list `shadow/arenas/*.state`, load one, promote one
    /// to `current.state` (what loop.sh starts fights from), or capture the
    /// on-screen situation as a new named arena.
    fn arena_section(&mut self, ui: &mut egui::Ui, state: &mut DebugState) {
        ui.heading("🏟 Arena");

        // Deferred make-current: wait for the queued save to land on disk
        // (mtime after the request), then copy it over current.state.
        if let Some((target, requested)) = self.pending_current.clone() {
            let landed = std::fs::metadata(&target)
                .and_then(|m| m.modified())
                .map(|mtime| mtime >= requested)
                .unwrap_or(false);
            if landed {
                self.make_current(&target);
                self.pending_current = None;
            } else if requested.elapsed().map(|e| e.as_secs() > 5).unwrap_or(true) {
                self.arena_note =
                    Some(format!("save of {} never landed — not made current", target.display()));
                self.pending_current = None;
            }
        }

        let stale = self
            .arenas_refreshed
            .map(|t| t.elapsed().as_secs_f64() > MODELS_REFRESH_SECS)
            .unwrap_or(true);
        if stale {
            self.arenas = scan_arenas();
            self.arenas_refreshed = Some(Instant::now());
        }

        if self.arenas.is_empty() {
            ui.label(
                egui::RichText::new(format!("No arenas under {}/ yet.", arenas_dir().display()))
                    .color(egui::Color32::DARK_GRAY),
            );
        }
        let mut promote: Option<PathBuf> = None;
        for (name, path, age) in &self.arenas {
            ui.horizontal(|ui| {
                if *name == CURRENT_ARENA {
                    ui.label(egui::RichText::new("📌 current").strong());
                } else {
                    ui.monospace(name);
                }
                ui.label(egui::RichText::new(age).small().color(egui::Color32::DARK_GRAY));
                if ui.small_button("📂 Load").clicked() {
                    state.pending_state_op = Some(StateOp::Load(path.clone()));
                }
                if *name != CURRENT_ARENA && ui.small_button("📌 Make current").clicked() {
                    promote = Some(path.clone());
                }
            });
        }
        if let Some(path) = promote {
            self.make_current(&path);
        }

        // ── Capture the on-screen situation as a new arena ────────────
        ui.horizontal(|ui| {
            ui.label("New:");
            ui.add(
                egui::TextEdit::singleline(&mut self.arena_name)
                    .desired_width(140.0)
                    .hint_text("e.g. goat-vs-alice"),
            );
            ui.checkbox(&mut self.arena_make_current, "make current");
            let name = self.arena_name.trim().trim_end_matches(".state").to_string();
            let ok = !name.is_empty() && !name.contains('/') && name != CURRENT_ARENA;
            if ui.add_enabled(ok, egui::Button::new("💾 Save arena")).clicked() {
                let path = arenas_dir().join(format!("{name}.state"));
                state.pending_state_op = Some(StateOp::Save(path.clone()));
                if self.arena_make_current {
                    // 1s slack so coarse fs mtime granularity can't round the
                    // landed file below the request time.
                    let requested = SystemTime::now() - std::time::Duration::from_secs(1);
                    self.pending_current = Some((path, requested));
                }
                self.arena_name.clear();
                self.arenas_refreshed = None; // relist next frame
            }
        });
        ui.label(
            egui::RichText::new(
                "loop.sh starts fights from current.state when it exists (ARENA env overrides).",
            )
            .small()
            .color(egui::Color32::DARK_GRAY),
        );
        if let Some(note) = &self.arena_note {
            let color = if note.contains("FAILED") || note.contains("never landed") {
                egui::Color32::from_rgb(230, 120, 120)
            } else {
                egui::Color32::GRAY
            };
            ui.label(egui::RichText::new(format!("Last: {note}")).small().color(color));
        }
    }

    /// Copy a named arena over `current.state` (copy, not symlink: portable,
    /// and re-saving the named arena later doesn't silently retarget current).
    fn make_current(&mut self, src: &PathBuf) {
        let dst = arenas_dir().join(format!("{CURRENT_ARENA}.state"));
        self.arena_note = Some(match std::fs::copy(src, &dst) {
            Ok(_) => format!(
                "current ← {}",
                src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
            ),
            Err(e) => format!("make current FAILED: {e}"),
        });
        self.arenas_refreshed = None; // relist next frame
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
                    ui.add(
                        egui::TextEdit::singleline(&mut self.record_style)
                            .desired_width(90.0)
                            .hint_text("style tag"),
                    )
                    .on_hover_text("Play-style declaration for this recording (rushdown, zoning, …) — stored in the sidecars, selectable at fit time");
                    if ui.button("⏺ Start").clicked() {
                        let path = recordings_dir()
                            .join(format!("session-{}.jsonl", utc_stamp()));
                        let style = self.record_style.trim();
                        state.pending_record = Some(RecordControl::Start {
                            path,
                            style: (!style.is_empty()).then(|| style.to_string()),
                        });
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
                    ui.label(format!("Nothing under {}/ — run shadow/loop.sh --fit-only.", models_dir().display()));
                } else if ui
                    .small_button("Load ALL as set")
                    .on_hover_text(
                        "Load every model as a SET: the newest per matchup key is kept and \
                         the right one is picked automatically at each round start",
                    )
                    .clicked()
                {
                    state.pending_shadow_load = Some(models_dir());
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
