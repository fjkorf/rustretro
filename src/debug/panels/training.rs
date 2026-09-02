use bevy_egui::egui;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use crate::debug::{
    DebugState, DummyMode, GuardMode, PlaybackPort, PlaybackTrigger, RecordControl, StateOp,
};

/// GUI face of the shadow lifecycle's two app-side stages — demonstrate
/// (recorder start/stop + status) and deploy (model card, runtime model load,
/// enable/disable) — plus the training-mode controls (F1–F5). The widgets
/// read and write the same `TrainingConfig`/one-shots the hotkeys do, so the
/// panel doubles as the first visible readout of state that was stderr-only.
pub struct TrainingPanel {
    /// Cached `shadow/models/*` listing (dirs containing cases.npz).
    models: Vec<(String, PathBuf)>,
    models_refreshed: Option<Instant>,
    /// Cached `shadow/arenas/*.state` listing: (display name, path, age,
    /// sidecar — `None` when the arena predates the `.meta.json` feature or
    /// the file failed to parse; rendered as "unknown", never fabricated).
    arenas: Vec<(String, PathBuf, String, Option<crate::frontend::ArenaMeta>)>,
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
    /// Whether `state.training`'s settings sidecar has been merged in for
    /// this panel instance yet (the first `show()` call does it once — see
    /// `DebugState::new()`'s note on why this can't just happen at
    /// construction: it would make every test's `DebugState::new()` sensitive
    /// to a stray sidecar in the process's cwd).
    settings_loaded: bool,
    /// JSON snapshot of the settings subset as of the last write to the
    /// sidecar (or the just-loaded value) — cheap per-frame diff so the
    /// panel only touches disk when something the user can save actually
    /// changed (dummy/guard/reversal-timing/punish pool).
    settings_last_saved: Option<String>,
    /// New-recording name field (🎮 Input slots section, task A2).
    slot_name: String,
    /// Cached `shadow/inputs/<family>/*.slot.json` listing.
    slots: Vec<crate::playback::SlotSummary>,
    slots_refreshed: Option<Instant>,
    /// Port target for the next "▶ Play" click.
    play_port: PlaybackPort,
    /// Trigger for the next "▶ Play" click.
    play_trigger: PlaybackTrigger,
}

const DUMMY_MODES: [DummyMode; 6] = [
    DummyMode::Free,
    DummyMode::Stand,
    DummyMode::Crouch,
    DummyMode::Jump,
    DummyMode::Block,
    DummyMode::BlockPunish,
];

/// The two guarding modes read differently per family guard STYLE: a button
/// family holds a chord (inert), a back-hold family guards reactively and
/// punishes on attack COMMITMENT rather than confirmed contact (§9.3).
fn dummy_label(mode: DummyMode, reactive: bool) -> &'static str {
    match (mode, reactive) {
        (DummyMode::Free, _) => "Free (human / shadow drives P2)",
        (DummyMode::Stand, _) => "Stand",
        (DummyMode::Crouch, _) => "Crouch",
        (DummyMode::Jump, _) => "Jump (hop cadence)",
        (DummyMode::Block, true) => "Block (reactive guard)",
        (DummyMode::Block, false) => "Block (hold block button)",
        (DummyMode::BlockPunish, true) => "Block + punish (on attack commit)",
        (DummyMode::BlockPunish, false) => "Block + punish (on contact)",
    }
}

/// MK-style vocabulary for `crate::debug::ReversalTiming`'s combo box.
fn reversal_timing_label(t: &crate::debug::ReversalTiming) -> &'static str {
    use crate::debug::ReversalTiming::*;
    match t {
        Fast => "Fast (first possible frame)",
        Delay { .. } => "Delay (randomized)",
        Late => "Late (last frame that still punishes)",
        Explicit(_) => "Explicit (frame count)",
    }
}

const GUARD_MODES: [GuardMode; 4] =
    [GuardMode::All, GuardMode::AfterFirstHit, GuardMode::Random, GuardMode::None];

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

/// `shadow/arenas/*.state`, `current` first, then alphabetical. Each entry
/// carries its `.meta.json` sidecar when one exists (`None` = unknown —
/// arenas saved before the sidecar feature, or under a still-unbuilt
/// probe).
fn scan_arenas() -> Vec<(String, PathBuf, String, Option<crate::frontend::ArenaMeta>)> {
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
            let meta = crate::frontend::load_arena_meta(&path);
            out.push((name, path, age, meta));
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
            settings_loaded: false,
            settings_last_saved: None,
            slot_name: String::new(),
            slots: Vec::new(),
            slots_refreshed: None,
            play_port: PlaybackPort::default(),
            play_trigger: PlaybackTrigger::default(),
        }
    }

    /// Write `state.training`'s settings subset to the sidecar iff it
    /// differs from the last write (or the just-loaded snapshot) — call at
    /// every exit point of `show()` so a settings change is captured
    /// regardless of which branch rendered this frame.
    fn autosave_settings(&mut self, state: &DebugState) {
        let snap = state.training.persisted_snapshot_json();
        if self.settings_last_saved.as_deref() != Some(snap.as_str()) {
            state.training.save();
            self.settings_last_saved = Some(snap);
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut DebugState) {
        if !self.settings_loaded {
            // First render this session: pull in whatever the user last
            // configured (see `TrainingConfig::merge_persisted`'s doc for
            // exactly what is/isn't touched — never `enabled`/`refill`, so a
            // `--training` startup flag is never clobbered).
            state.training.merge_persisted();
            self.settings_last_saved = Some(state.training.persisted_snapshot_json());
            self.settings_loaded = true;
        }
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
            self.autosave_settings(state);
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
            let reactive = crate::training::guard_is_reactive();
            ui.horizontal(|ui| {
                ui.label("Dummy (F1):");
                let current = dummy_label(state.training.dummy, reactive);
                egui::ComboBox::from_id_salt("training_dummy")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for mode in DUMMY_MODES {
                            let label = dummy_label(mode, reactive);
                            // The guarding modes need a resolvable guard, and
                            // BlockPunish additionally a trigger — offered
                            // greyed with the reason otherwise.
                            let ok = match mode {
                                DummyMode::Block => feats.block_dummy,
                                DummyMode::BlockPunish => feats.block_punish,
                                _ => true,
                            };
                            if !ok {
                                ui.add_enabled(false, egui::Button::selectable(false, label))
                                    .on_disabled_hover_text(
                                        "Needs a guard source in the profile: a block chord \
                                         (button families) or x + an attack-commitment signal \
                                         (back-hold families), plus a contact signal for the \
                                         contact-triggered punish — see the game's .md",
                                    );
                                continue;
                            }
                            ui.selectable_value(&mut state.training.dummy, mode, label);
                        }
                    });
            });
            if matches!(state.training.dummy, DummyMode::Block | DummyMode::BlockPunish) {
                self.guard_section(ui, state, &feats, reactive);
            }
            if state.training.dummy == DummyMode::BlockPunish {
                // Live phase — a silent dummy explains itself (ARMED vs
                // cooling vs mid-punish) instead of looking broken. The
                // string is computed once in training::tick; Lua reads the
                // same one via training.punish_state().
                let phase = state.training.punish_phase.clone();
                if !phase.is_empty() {
                    let color = if phase.starts_with("punishing") {
                        egui::Color32::from_rgb(0xFF, 0xC1, 0x07)
                    } else if phase.contains("ARMED") {
                        egui::Color32::from_rgb(0x4C, 0xAF, 0x50)
                    } else {
                        egui::Color32::GRAY
                    };
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("state:").small().color(egui::Color32::DARK_GRAY));
                        ui.label(egui::RichText::new(phase).small().strong().color(color));
                    })
                    .response
                    .on_hover_text(if reactive {
                        "ARMED = the next attack committed in range punishes. \
                         'punishing (commit)' means exactly that — blocked contact is \
                         undetectable on this game, so the trigger is the opponent's \
                         commitment (block-punish AND whiff-punish)."
                    } else {
                        "ARMED = the next blocked contact punishes. cooling = waiting for \
                         the contact signal to go quiet. recovering = deliberate neutral \
                         right after a punish, so the returning guard can't block-cancel \
                         the just-pressed attack. A whiffed attack never registers \
                         as contact, so the dummy correctly stays armed."
                    });
                }
                self.reversal_section(ui, state);
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
        self.playback_section(ui, state);
        ui.separator();
        self.shadow_section(ui, state);
        ui.separator();
        self.arena_section(ui, state);
        self.autosave_settings(state);
    }

    /// Reversal timing (MK-style "Block Attack: Fast / Delay / Late", plus an
    /// explicit-frames power-user knob) — WHEN the scheduled BlockPunish
    /// macro starts relative to its trigger. `training::PUNISH_DELAY` used to
    /// be a single hardcoded constant; this is that same knob, now a setting.
    fn reversal_section(&mut self, ui: &mut egui::Ui, state: &mut DebugState) {
        use crate::debug::ReversalTiming;
        ui.horizontal(|ui| {
            ui.label("Reversal timing:");
            let cur = state.training.reversal_timing;
            egui::ComboBox::from_id_salt("training_reversal_timing")
                .selected_text(reversal_timing_label(&cur))
                .show_ui(ui, |ui| {
                    if ui.selectable_label(matches!(cur, ReversalTiming::Fast), "Fast").clicked() {
                        state.training.reversal_timing = ReversalTiming::Fast;
                    }
                    if ui.selectable_label(matches!(cur, ReversalTiming::Delay { .. }), "Delay").clicked()
                        && !matches!(cur, ReversalTiming::Delay { .. })
                    {
                        state.training.reversal_timing = ReversalTiming::Delay {
                            min: crate::training::PUNISH_DELAY_FAST,
                            max: crate::training::PUNISH_DELAY_LATE,
                        };
                    }
                    if ui.selectable_label(matches!(cur, ReversalTiming::Late), "Late").clicked() {
                        state.training.reversal_timing = ReversalTiming::Late;
                    }
                    if ui
                        .selectable_label(matches!(cur, ReversalTiming::Explicit(_)), "Explicit")
                        .clicked()
                        && !matches!(cur, ReversalTiming::Explicit(_))
                    {
                        state.training.reversal_timing =
                            ReversalTiming::Explicit(crate::training::PUNISH_DELAY);
                    }
                });
        })
        .response
        .on_hover_text(
            "When the scheduled punish starts after the trigger. Fast/Late use \
             measured floor/ceiling frame counts (never below what the game \
             actually accepts); Delay re-rolls a random frame count in its \
             range on every punish; Explicit is a literal frame count.",
        );
        match &mut state.training.reversal_timing {
            ReversalTiming::Delay { min, max } => {
                ui.horizontal(|ui| {
                    ui.label("  range:");
                    ui.add(egui::DragValue::new(min).range(1..=200).suffix("f"));
                    ui.label("–");
                    ui.add(egui::DragValue::new(max).range(1..=200).suffix("f"));
                });
            }
            ReversalTiming::Explicit(frames) => {
                ui.horizontal(|ui| {
                    ui.label("  frames:");
                    ui.add(egui::DragValue::new(frames).range(0..=200));
                });
            }
            ReversalTiming::Fast | ReversalTiming::Late => {}
        }
    }

    /// Guard mode (MACRO_ACTIONS §9.4) — the selector every trainer has:
    /// Guard All / After First Hit / Random / None, plus a one-line honest
    /// description of what the family's guard STYLE actually does.
    fn guard_section(
        &mut self,
        ui: &mut egui::Ui,
        state: &mut DebugState,
        feats: &crate::training::Features,
        reactive: bool,
    ) {
        ui.horizontal(|ui| {
            ui.label("Guard:");
            egui::ComboBox::from_id_salt("training_guard_mode")
                .selected_text(state.training.guard_mode.label())
                .show_ui(ui, |ui| {
                    for mode in GUARD_MODES {
                        // After First Hit needs a hit signal to key off.
                        if mode == GuardMode::AfterFirstHit && !feats.guard_after_hit {
                            ui.add_enabled(
                                false,
                                egui::Button::selectable(false, mode.label()),
                            )
                            .on_disabled_hover_text(
                                "Needs a contact signal (hitstun_sources or contact_signal) \
                                 in the profile — see the game's .md",
                            );
                            continue;
                        }
                        ui.selectable_value(&mut state.training.guard_mode, mode, mode.label());
                    }
                });
            if state.training.guard_mode == GuardMode::Random {
                ui.add(
                    egui::DragValue::new(&mut state.training.guard_random_pct.0)
                        .range(0..=100)
                        .suffix("%"),
                )
                .on_hover_text("Chance the dummy guards each opportunity (rolled once per attack)");
            }
        });
        let hint = if reactive {
            match crate::training::guard_range() {
                Some(r) => format!(
                    "Reactive: stands neutral, holds away only while the opponent is \
                     attacking within {r} units. Never crouch-guards (down-back blocks \
                     nothing here)."
                ),
                None => "Reactive: stands neutral, holds away only while the opponent is \
                         attacking (no guard_range mapped — no distance gate)."
                    .to_string(),
            }
        } else {
            "Holds the block button while guarding — positionally inert.".to_string()
        };
        ui.label(egui::RichText::new(hint).small().color(egui::Color32::DARK_GRAY));
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
        let profile = crate::profile::current();
        for (name, path, age, meta) in &self.arenas {
            ui.horizontal(|ui| {
                if *name == CURRENT_ARENA {
                    ui.label(egui::RichText::new("📌 current").strong());
                } else {
                    ui.monospace(name);
                }
                ui.label(egui::RichText::new(age).small().color(egui::Color32::DARK_GRAY));
                match meta {
                    Some(m) => {
                        let matchup = format!(
                            "{} vs {}",
                            m.char_id_block1.map(|id| profile.char_name(id)).unwrap_or_else(|| "?".into()),
                            m.char_id_block2.map(|id| profile.char_name(id)).unwrap_or_else(|| "?".into()),
                        );
                        ui.label(egui::RichText::new(matchup).small().color(egui::Color32::GRAY));
                        if m.inputs_live.p1 == Some(false) {
                            ui.label(
                                egui::RichText::new("⚠ 1P vs CPU — the dummy cannot be driven here")
                                    .small()
                                    .color(egui::Color32::from_rgb(230, 180, 90)),
                            );
                        }
                    }
                    None => {
                        ui.label(
                            egui::RichText::new("(unknown — no sidecar)")
                                .small()
                                .color(egui::Color32::DARK_GRAY),
                        );
                    }
                }
                if ui.small_button("📂 Load").clicked() {
                    state.pending_state_op = Some(StateOp::Load(path.clone()));
                    // The dummy has nothing to drive on a CPU-owned port —
                    // warn now rather than let a later "why won't it move"
                    // verification cycle rediscover this the hard way.
                    if let Some(m) = meta {
                        if m.inputs_live.p1 == Some(false) && state.training.dummy != crate::debug::DummyMode::Free {
                            self.arena_note = Some(format!(
                                "loaded {name}: 1P-vs-CPU arena — dummy mode {:?} has nothing to drive on port 1",
                                state.training.dummy
                            ));
                        }
                    }
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
            Ok(_) => {
                // Best-effort: carry the sidecar along so current.state stays
                // self-describing too. A missing source sidecar (pre-feature
                // arena) is not an error — `current.state` just has none either.
                let _ = std::fs::copy(src.with_extension("meta.json"), dst.with_extension("meta.json"));
                format!(
                    "current ← {}",
                    src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
                )
            }
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

    /// Named input-slot record/playback (task A2): capture both ports'
    /// folded per-frame input into a slot on disk, and replay a slot
    /// deterministically onto one or both ports. Distinct from "⏺ Record
    /// demonstrations" above (that's the shadow-ML jsonl trace recorder) —
    /// this is the frame-lab / bug-repro instrument. See `playback.rs`'s
    /// module doc for the precedence rule against the training dummy and
    /// the determinism guarantees per trigger.
    fn playback_section(&mut self, ui: &mut egui::Ui, state: &mut DebugState) {
        ui.heading("🎮 Input slots");

        // ── record ──────────────────────────────────────────────────────
        // Pull what the closure needs out BEFORE matching, so the borrow of
        // `state.recording_slot` ends here — the closure below calls
        // `crate::playback::stop_recording(state)`, which needs `state`
        // whole, and that can't coexist with a live borrow of one of its
        // fields (disjoint closure capture doesn't help once a function call
        // needs the whole reference).
        let rec_info = state.recording_slot.as_ref().map(|rec| (rec.name.clone(), rec.frames.len()));
        match rec_info {
            Some((rec_name, rec_frames)) => {
                let secs = rec_frames as u64 / 60;
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("● REC")
                            .color(egui::Color32::from_rgb(230, 90, 90))
                            .strong(),
                    );
                    ui.monospace(format!(
                        "{}  {} frames ({}:{:02})",
                        rec_name,
                        rec_frames,
                        secs / 60,
                        secs % 60,
                    ));
                    if ui.button("⏹ Stop").clicked() {
                        match crate::playback::stop_recording(state) {
                            Ok((path, n)) => {
                                state.recording_note =
                                    Some(format!("stopped — {n} frames → {}", path.display()));
                                self.slots_refreshed = None; // relist next frame
                            }
                            Err(e) => state.recording_note = Some(format!("stop FAILED: {e}")),
                        }
                    }
                });
            }
            None => {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("○ not recording").color(egui::Color32::DARK_GRAY));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.slot_name)
                            .desired_width(120.0)
                            .hint_text("slot name"),
                    );
                    let name = self.slot_name.trim().to_string();
                    if ui.add_enabled(!name.is_empty(), egui::Button::new("⏺ Start")).clicked() {
                        match crate::playback::start_recording(state, &name, crate::profile::current()) {
                            Ok(()) => {
                                state.recording_note = Some(format!("recording '{name}'"));
                                self.slot_name.clear();
                            }
                            Err(e) => state.recording_note = Some(format!("start FAILED: {e}")),
                        }
                    }
                });
                ui.label(
                    egui::RichText::new(
                        "Captures BOTH ports' folded input every real frame — deterministic \
                         playback later (round-trip guaranteed for the round_start trigger; \
                         see the Play controls below).",
                    )
                    .small()
                    .color(egui::Color32::DARK_GRAY),
                );
            }
        }
        if let Some(note) = &state.recording_note {
            let color = if note.contains("FAILED") {
                egui::Color32::from_rgb(230, 120, 120)
            } else {
                egui::Color32::GRAY
            };
            ui.label(egui::RichText::new(format!("Last: {note}")).small().color(color));
        }

        ui.separator();

        // ── slot list + play controls ──────────────────────────────────
        let family = crate::profile::current().family.family.clone();
        let stale = self
            .slots_refreshed
            .map(|t| t.elapsed().as_secs_f64() > MODELS_REFRESH_SECS)
            .unwrap_or(true);
        if stale {
            self.slots = crate::playback::list_slots(&family);
            self.slots_refreshed = Some(Instant::now());
        }
        if self.slots.is_empty() {
            ui.label(
                egui::RichText::new(format!(
                    "No slots under {}/ yet.",
                    crate::playback::slots_dir(&family).display()
                ))
                .color(egui::Color32::DARK_GRAY),
            );
        }
        ui.horizontal(|ui| {
            ui.label("Port:");
            egui::ComboBox::from_id_salt("playback_port")
                .selected_text(match self.play_port {
                    PlaybackPort::P1 => "P1",
                    PlaybackPort::P2 => "P2",
                    PlaybackPort::Both => "Both",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.play_port, PlaybackPort::P1, "P1");
                    ui.selectable_value(&mut self.play_port, PlaybackPort::P2, "P2");
                    ui.selectable_value(&mut self.play_port, PlaybackPort::Both, "Both");
                });
            ui.label("Trigger:");
            egui::ComboBox::from_id_salt("playback_trigger")
                .selected_text(match self.play_trigger {
                    PlaybackTrigger::Manual => "Manual (now)",
                    PlaybackTrigger::RoundStart => "Round start",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.play_trigger, PlaybackTrigger::Manual, "Manual (now)")
                        .on_hover_text("Begins on the next real frame — pair with pause/step for frame-exact timing");
                    ui.selectable_value(&mut self.play_trigger, PlaybackTrigger::RoundStart, "Round start")
                        .on_hover_text("Begins on the fight gate's next closed→open transition — deterministic from a pre-round save state");
                });
        });
        let mut to_play: Option<String> = None;
        for s in &self.slots {
            ui.horizontal(|ui| {
                ui.monospace(&s.name);
                ui.label(
                    egui::RichText::new(format!("{} frames", s.frame_count))
                        .small()
                        .color(egui::Color32::GRAY),
                );
                if ui
                    .add_enabled(state.playback_slot.is_none(), egui::Button::new("▶ Play"))
                    .clicked()
                {
                    to_play = Some(s.name.clone());
                }
            });
        }
        if let Some(name) = to_play {
            match crate::playback::start_playback(
                state,
                &name,
                self.play_port,
                self.play_trigger,
                crate::profile::current(),
            ) {
                Ok(n) => {
                    state.playback_note =
                        Some(format!("armed '{name}' ({n} frames, {:?} / {:?})", self.play_port, self.play_trigger))
                }
                Err(e) => state.playback_note = Some(format!("play FAILED: {e}")),
            }
        }

        // ── active playback status ──────────────────────────────────────
        // Same extract-before-match reasoning as the record section above.
        let pb_info = state
            .playback_slot
            .as_ref()
            .map(|pb| (pb.name.clone(), pb.done, pb.started, pb.idx, pb.frames.len()));
        if let Some((pb_name, pb_done, pb_started, pb_idx, pb_total)) = pb_info {
            let phase = if pb_done {
                "finishing…"
            } else if pb_started {
                "▶ playing"
            } else {
                "⏳ armed — waiting for round start"
            };
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{pb_name} — {phase} ({pb_idx}/{pb_total})")).strong(),
                );
                if ui.button("⏹ Stop").clicked() {
                    let _ = crate::playback::stop_playback(state);
                    state.playback_note = Some("stopped".to_string());
                }
            });
        }
        if let Some(note) = &state.playback_note {
            let color = if note.contains("FAILED") {
                egui::Color32::from_rgb(230, 120, 120)
            } else {
                egui::Color32::GRAY
            };
            ui.label(egui::RichText::new(format!("Last: {note}")).small().color(color));
        }

        // ── who is driving each port right now (task A2 §4) ──────────────
        let active = state.playback_slot.as_ref().filter(|pb| pb.started && !pb.done);
        let (p1_drive, p2_drive) = match active {
            Some(pb) => (
                if pb.port.drives(0) { format!("▶ playback '{}'", pb.name) } else { "free".to_string() },
                if pb.port.drives(1) {
                    format!("▶ playback '{}' (dummy suppressed)", pb.name)
                } else {
                    format!("{:?}", state.training.dummy)
                },
            ),
            None => ("free".to_string(), format!("{:?}", state.training.dummy)),
        };
        ui.label(
            egui::RichText::new(format!("Driving — P1: {p1_drive}   P2: {p2_drive}"))
                .small()
                .color(egui::Color32::DARK_GRAY),
        );
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
