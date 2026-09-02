//! 🔬 Frame Lab — the in-app viewer and verifier for `docs/frames.md`'s
//! measured frame data (`library/<family>/<port>.frames.json`, loaded into
//! `GameProfile.frames` by `profile.rs`).
//!
//! Deliberately NOT a measurement engine. Driving the act-again probe
//! (`docs/frames.md` §4) from Rust would be a second implementation of
//! something that already needs one shared golden fixture
//! (`shadow_train.framelab`) to keep two copies in sync — measurement stays
//! in Python; this panel only reads what it already measured, and for a gap
//! it offers the exact command to fill it (§4's "Command handoff" below).
//!
//! State (the selected character/cell) lives behind a module-local
//! `OnceLock`, the `hunt.rs`/`crate::hunt` pattern — `DebugState` gets no new
//! fields for this panel.

use bevy_egui::egui;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

use crate::debug::DebugState;
use crate::profile::{FrameCell, FrameTable};

/// Sample counts below this are "sparse": a single independent measurement
/// with no repeat check yet (docs/frames.md §8 criterion 1 wants a re-run of
/// a sample of rows; a cell resting on one run hasn't had it).
const SPARSE_SAMPLE_N: i64 = 2;

/// Panel-local selection state, kept out of `DebugState` on purpose. There is
/// one debugger per process, so a single lazily-initialized cell is fine —
/// exactly `crate::hunt`'s `cell()` shape, just local to this panel instead
/// of a process-wide sampling engine.
struct FramelabState {
    /// The character currently shown; re-picked if it falls outside the
    /// loaded table's roster (e.g. after switching games).
    char: Option<String>,
    /// The clicked table cell: (move, gap_walk_frames). Drives the command
    /// handoff / provenance-detail sections below the grid.
    selected: Option<(String, Option<i64>)>,
}

fn state() -> &'static Mutex<FramelabState> {
    static CELL: OnceLock<Mutex<FramelabState>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(FramelabState { char: None, selected: None }))
}

pub struct FramelabPanel;

impl FramelabPanel {
    pub fn new() -> Self {
        FramelabPanel
    }

    pub fn show(&mut self, ui: &mut egui::Ui, _state: &mut DebugState) {
        ui.heading("🔬 Frame Lab");
        ui.label(
            egui::RichText::new(
                "Viewer and verifier for docs/frames.md's measured frame data — this panel \
                 never measures. It reads library/<family>/<port>.frames.json and, for a gap, \
                 offers the command to fill it.",
            )
            .small()
            .color(egui::Color32::DARK_GRAY),
        );
        ui.separator();

        let profile = crate::profile::current();
        match profile.frames.as_ref() {
            Some(table) => self.render(ui, table),
            None => {
                ui.label(format!(
                    "No frame lab data for {}/{} — this game has not been run through \
                     shadow_train.framelab yet (that is normal, not an error).",
                    profile.family.family, profile.port.port
                ));
            }
        }
    }

    /// Everything below the "no data" branch, factored out so it can be
    /// exercised in tests against a synthetic `FrameTable` without needing a
    /// process-wide `profile::current()` carrying frame data (the test
    /// binary's `GameProfile` is a single `OnceLock`, set once to asurabld —
    /// which ships no frames.json — by `profile::init_for_tests`).
    fn render(&mut self, ui: &mut egui::Ui, table: &FrameTable) {
        let chars = table.chars();
        if chars.is_empty() {
            ui.label("frames.json loaded but carries zero rows.");
            return;
        }

        let current_char = {
            let mut st = state().lock().unwrap();
            let stale = st.char.as_deref().map(|c| !chars.contains(&c)).unwrap_or(true);
            if stale {
                st.char = Some(chars[0].to_string());
                st.selected = None;
            }
            st.char.clone().unwrap()
        };

        ui.horizontal(|ui| {
            ui.label("Character:");
            let mut clicked_char: Option<String> = None;
            egui::ComboBox::from_id_salt("framelab_char")
                .selected_text(&current_char)
                .show_ui(ui, |ui| {
                    for c in &chars {
                        if ui.selectable_label(current_char == *c, *c).clicked() {
                            clicked_char = Some(c.to_string());
                        }
                    }
                });
            if let Some(c) = clicked_char {
                let mut st = state().lock().unwrap();
                st.char = Some(c);
                st.selected = None;
            }
            ui.label(
                egui::RichText::new(format!(
                    "{} · {} · generated {} · schema v{}",
                    table.family,
                    table.port,
                    table.generated_at.as_deref().unwrap_or("unknown"),
                    table.schema_version.map(|v| v.to_string()).unwrap_or("?".into()),
                ))
                .small()
                .color(egui::Color32::DARK_GRAY),
            );
        });

        let current_char = state().lock().unwrap().char.clone().unwrap();

        ui.separator();
        self.table_section(ui, table, &current_char);
        ui.separator();
        self.provenance_section(ui, table, &current_char);
        ui.separator();
        self.safety_section(ui, table, &current_char);
        ui.separator();
        self.command_section(ui, table, &current_char);
    }

    // ── §1: the move × gap table ────────────────────────────────────────
    fn table_section(&mut self, ui: &mut egui::Ui, table: &FrameTable, ch: &str) {
        ui.heading("Move × gap");
        let moves = table.moves_for_char(ch);
        let gaps = table.gaps_for_char(ch);
        if moves.is_empty() || gaps.is_empty() {
            ui.label("no moves measured for this character.");
            return;
        }

        let mut clicked: Option<(String, Option<i64>)> = None;
        egui::ScrollArea::horizontal().id_salt("framelab_table_scroll").show(ui, |ui| {
            egui::Grid::new("framelab_grid").spacing([8.0, 4.0]).striped(true).show(ui, |ui| {
                ui.label(egui::RichText::new("move \\ gap (walk-frames)").small());
                ui.label(egui::RichText::new("Startup (FAF)").strong()).on_hover_text(
                    "1-indexed frame on which contact can first occur (docs/frames.md §2.1) \
                     — measured at the minimum-gap row only (§4.4).",
                );
                for g in &gaps {
                    ui.label(egui::RichText::new(format!("{g}f")).strong());
                }
                ui.end_row();
                for mv in &moves {
                    let mv: &str = mv;
                    ui.label(egui::RichText::new(mv).strong());
                    ui.label(egui::RichText::new(startup_faf_text(table, ch, mv)).monospace())
                        .on_hover_text("FAF is measured at the minimum-gap row only (docs/frames.md §4.4).");
                    for g in &gaps {
                        let g: i64 = *g;
                        let cell = table.cell(ch, mv, Some(g));
                        let (label, color) = cell_style(cell);
                        let sel = state()
                            .lock()
                            .unwrap()
                            .selected
                            .as_ref()
                            .map(|(m, gg)| (m.as_str(), *gg))
                            == Some((mv, Some(g)));
                        let btn = egui::Button::new(
                            egui::RichText::new(label).color(color).monospace(),
                        )
                        .selected(sel);
                        let hover = cell_hover_text(cell);
                        if ui.add(btn).on_hover_text(hover).clicked() {
                            clicked = Some((mv.to_string(), Some(g)));
                        }
                    }
                    ui.end_row();
                }
            });
        });
        if let Some(sel) = clicked {
            state().lock().unwrap().selected = Some(sel);
        }
        ui.label(
            egui::RichText::new(
                "on-hit/on-block · — = unmeasured, · = never measured, KD = knockdown (no \
                 on-hit number — measure the wakeup window instead), amber = sparse \
                 (sample_n < 2), red = observables DISAGREED (docs/frames.md §12). \
                 Startup (FAF) = 1-indexed frame on which contact can first occur, \
                 measured at the minimum-gap row only (§2.1, §4.4).",
            )
            .small()
            .color(egui::Color32::DARK_GRAY),
        );
    }

    // ── §2: provenance card ─────────────────────────────────────────────
    fn provenance_section(&mut self, ui: &mut egui::Ui, table: &FrameTable, ch: &str) {
        ui.heading("Provenance");
        let cells = table.cells_for_char(ch);

        let mut by_key: BTreeMap<(String, String), Vec<i64>> = BTreeMap::new();
        let mut latency_by_key: BTreeMap<(String, String), BTreeSet<i64>> = BTreeMap::new();
        let mut core_ids: BTreeSet<String> = BTreeSet::new();
        let mut rom_ids: BTreeSet<String> = BTreeSet::new();
        let mut measured_ats: Vec<String> = Vec::new();
        for cell in &cells {
            for o in &cell.observations {
                let shape = o.rig_guard_state.clone().unwrap_or_else(|| "(unstated)".into());
                let key = (o.observable.clone(), shape);
                if let Some(n) = o.sample_n {
                    by_key.entry(key.clone()).or_default().push(n);
                } else {
                    by_key.entry(key.clone()).or_default();
                }
                if let Some(l) = o.input_latency_frames {
                    latency_by_key.entry(key).or_default().insert(l);
                }
                if let Some(c) = &o.core_id {
                    core_ids.insert(c.clone());
                }
                if let Some(r) = &o.rom_id {
                    rom_ids.insert(r.clone());
                }
                if let Some(m) = &o.measured_at {
                    measured_ats.push(m.clone());
                }
            }
        }

        if by_key.is_empty() {
            ui.label("no observations recorded for this character.");
            return;
        }

        egui::Grid::new("framelab_provenance_grid").striped(true).show(ui, |ui| {
            ui.label(egui::RichText::new("observable").strong());
            ui.label(egui::RichText::new("probe shape").strong());
            ui.label(egui::RichText::new("latency").strong());
            ui.label(egui::RichText::new("rows").strong());
            ui.label(egui::RichText::new("sample_n").strong());
            ui.end_row();
            for ((observable, shape), samples) in &by_key {
                ui.label(egui::RichText::new(observable).monospace());
                ui.label(egui::RichText::new(shape).monospace().small());
                let latencies = latency_by_key.get(&(observable.clone(), shape.clone()));
                let latency_text = match latencies {
                    Some(l) if l.len() == 1 => format!("{}", l.iter().next().unwrap()),
                    Some(l) if !l.is_empty() => {
                        format!("VARIES {:?}", l.iter().collect::<Vec<_>>())
                    }
                    _ => "—".to_string(),
                };
                let latency_color = if matches!(latencies, Some(l) if l.len() > 1) {
                    egui::Color32::from_rgb(220, 90, 90)
                } else {
                    ui.visuals().text_color()
                };
                ui.colored_label(latency_color, latency_text);
                ui.label(format!("{}", samples.len()));
                let n_text = if samples.is_empty() {
                    "—".to_string()
                } else {
                    let min = samples.iter().min().unwrap();
                    let max = samples.iter().max().unwrap();
                    if min == max { format!("{min}") } else { format!("{min}..{max}") }
                };
                ui.label(n_text);
                ui.end_row();
            }
        });

        ui.horizontal(|ui| {
            let core_text = if core_ids.len() > 1 {
                format!("⚠ MIXED core builds: {}", core_ids.iter().cloned().collect::<Vec<_>>().join(", "))
            } else {
                core_ids.iter().next().cloned().unwrap_or_else(|| "unknown".into())
            };
            ui.label(egui::RichText::new(format!("core_id: {core_text}")).small().monospace());
        });
        ui.horizontal(|ui| {
            let rom_text = if rom_ids.len() > 1 {
                format!("⚠ MIXED ROM builds: {}", rom_ids.iter().cloned().collect::<Vec<_>>().join(", "))
            } else {
                rom_ids.iter().next().cloned().unwrap_or_else(|| "unknown".into())
            };
            ui.label(egui::RichText::new(format!("rom_id: {rom_text}")).small().monospace());
        });
        measured_ats.sort();
        if let (Some(first), Some(last)) = (measured_ats.first(), measured_ats.last()) {
            ui.label(
                egui::RichText::new(format!("measured_at: {first} .. {last}"))
                    .small()
                    .color(egui::Color32::DARK_GRAY),
            );
        }
        ui.label(
            egui::RichText::new(
                "A number measured on a different core/ROM build is a different number \
                 (docs/frames.md §6) — mixed ids above are flagged, not resolved.",
            )
            .small()
            .color(egui::Color32::DARK_GRAY),
        );
    }

    // ── §3: safety readout ──────────────────────────────────────────────
    fn safety_section(&mut self, ui: &mut egui::Ui, table: &FrameTable, ch: &str) {
        ui.heading("Safety readout");
        let (most_unsafe, safest) = table.safety_extremes(ch);
        if most_unsafe.is_none() && safest.is_none() {
            ui.label("no on_block data measured for this character yet.");
        } else {
            if let Some(c) = most_unsafe {
                ui.colored_label(
                    egui::Color32::from_rgb(220, 90, 90),
                    format!(
                        "Most unsafe on block: {} @ {} ({:+})",
                        c.move_name,
                        gap_label(c.gap_walk_frames),
                        c.measurement.on_block.unwrap(),
                    ),
                );
            }
            if let Some(c) = safest {
                ui.colored_label(
                    egui::Color32::from_rgb(140, 200, 140),
                    format!(
                        "Safest on block: {} @ {} ({:+})",
                        c.move_name,
                        gap_label(c.gap_walk_frames),
                        c.measurement.on_block.unwrap(),
                    ),
                );
            }
        }
        ui.label(
            egui::RichText::new(
                "\"Safe\" (docs/frames.md §1) means the defender cannot reach with their own \
                 fastest first_active_frame inside connect range — this readout is the raw \
                 on_block extremes, not a computed punishability verdict.",
            )
            .small()
            .color(egui::Color32::DARK_GRAY),
        );
    }

    // ── §4: command handoff ─────────────────────────────────────────────
    fn command_section(&mut self, ui: &mut egui::Ui, table: &FrameTable, ch: &str) {
        ui.heading("Command handoff");
        let selected = state().lock().unwrap().selected.clone();
        let Some((mv, gap)) = selected else {
            ui.label(
                egui::RichText::new(
                    "Click a cell above — a missing one (·) gets the exact command to measure \
                     it; a measured one shows its per-observable provenance here.",
                )
                .small()
                .color(egui::Color32::DARK_GRAY),
            );
            return;
        };

        match table.cell(ch, &mv, gap) {
            Some(cell) => {
                ui.label(format!("{mv} @ {} — already measured:", gap_label(gap)));
                egui::Grid::new("framelab_cell_obs_grid").striped(true).show(ui, |ui| {
                    for h in
                        ["observable", "method", "latency", "n", "confidence", "measured_at"]
                    {
                        ui.label(egui::RichText::new(h).strong().small());
                    }
                    ui.end_row();
                    for o in &cell.observations {
                        ui.label(egui::RichText::new(&o.observable).monospace().small());
                        ui.label(egui::RichText::new(&o.method).small());
                        ui.label(opt_i64(o.input_latency_frames));
                        ui.label(opt_i64(o.sample_n));
                        ui.label(o.confidence.as_deref().unwrap_or("—"));
                        ui.label(
                            egui::RichText::new(o.measured_at.as_deref().unwrap_or("—")).small(),
                        );
                        ui.end_row();
                    }
                });
                if !cell.agrees() {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 90, 90),
                        format!("⚠ observables disagreed on: {}", cell.disagreements.join(", ")),
                    );
                }
                for (field, reference_obs) in &cell.one_sided_reference {
                    ui.label(
                        egui::RichText::new(format!(
                            "{field} is one-sided: collapsed value is in {reference_obs}'s frame \
                             of reference (docs/frames.md §4.2) — the other observable's raw \
                             reading legitimately differs by its own input_latency_frames.",
                        ))
                        .small()
                        .color(egui::Color32::DARK_GRAY),
                    );
                }
            }
            None => {
                ui.label(format!("{mv} @ {} is not measured.", gap_label(gap)));
                let arena = format!(
                    "shadow/arenas/{}/gap-{}.state",
                    table.family,
                    gap.map(|g| g.to_string()).unwrap_or_else(|| "K".into())
                );
                let cmd = format!(
                    "python -m shadow_train.framelab.kit --game library/{} --core <path-to-core> \
                     --rom <path-to-rom> --db shadow/framelab/frames.db --char {ch} --cell {mv}:{arena}",
                    table.family,
                );
                ui.add(
                    egui::TextEdit::multiline(&mut cmd.as_str())
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY),
                );
                if ui.button("⎘ Copy command").clicked() {
                    ui.ctx().copy_text(cmd.clone());
                }
                ui.label(
                    egui::RichText::new(format!(
                        "assumes an arena already saved at {arena} — see docs/frames.md §5 for \
                         the spacing-ladder capture if it doesn't exist yet.",
                    ))
                    .small()
                    .color(egui::Color32::DARK_GRAY),
                );
            }
        }
    }
}

/// The move's collapsed `first_active_frame` for the Startup (FAF) column.
/// FAF is deliberately stored only on the minimum-gap (collision-floor) row
/// and NULL elsewhere (docs/frames.md §4.4), so at most one distinct value
/// exists per move — `—` for none measured (matching the panel's other
/// absent measurements, never 0), and `⚠` if the contract is violated by
/// disagreeing values across rows (flagged, not resolved).
fn startup_faf_text(table: &FrameTable, ch: &str, mv: &str) -> String {
    let values: BTreeSet<i64> = table
        .cells_for_char(ch)
        .into_iter()
        .filter(|c| c.move_name == mv)
        .filter_map(|c| c.measurement.first_active_frame)
        .collect();
    match values.len() {
        0 => "—".to_string(),
        1 => values.iter().next().unwrap().to_string(),
        _ => "⚠".to_string(),
    }
}

fn gap_label(gap: Option<i64>) -> String {
    match gap {
        Some(g) => format!("{g}f gap"),
        None => "unstated gap".to_string(),
    }
}

fn opt_i64(v: Option<i64>) -> String {
    v.map(|v| v.to_string()).unwrap_or_else(|| "—".to_string())
}

/// Cell text + color: `·` / dark-gray for never measured (matches
/// `matchup.rs`'s coverage-grid convention), amber for sparse (only one
/// independent sample behind it), red for a cross-observable disagreement,
/// green otherwise. `KD` replaces the on-hit half for a knockdown — distinct
/// from `—` (an on-hit that was simply never measured).
fn cell_style(cell: Option<&FrameCell>) -> (String, egui::Color32) {
    let Some(c) = cell else {
        return ("·".to_string(), egui::Color32::DARK_GRAY);
    };
    let hit_text = if c.measurement.knockdown == Some(true) {
        "KD".to_string()
    } else if c.disagreements.contains(&"on_hit") {
        "⚠".to_string()
    } else {
        match c.measurement.on_hit {
            Some(v) => format!("{v:+}"),
            None => "—".to_string(),
        }
    };
    let block_text = if c.disagreements.contains(&"on_block") {
        "⚠".to_string()
    } else {
        match c.measurement.on_block {
            Some(v) => format!("{v:+}"),
            None => "—".to_string(),
        }
    };
    let text = format!("{hit_text}/{block_text}");
    let sparse = c.min_sample_n().map(|n| n < SPARSE_SAMPLE_N).unwrap_or(true);
    let color = if !c.agrees() {
        egui::Color32::from_rgb(220, 90, 90)
    } else if sparse {
        egui::Color32::from_rgb(230, 180, 90)
    } else {
        egui::Color32::from_rgb(150, 220, 150)
    };
    (text, color)
}

fn cell_hover_text(cell: Option<&FrameCell>) -> String {
    match cell {
        None => "not measured — click for the command to measure it".to_string(),
        Some(c) => {
            let mut lines = vec![
                format!("variant: {}", c.variant.as_deref().unwrap_or("—")),
                format!(
                    "first_active_frame: {}",
                    opt_i64(c.measurement.first_active_frame)
                ),
                format!("damage: {}", opt_i64(c.measurement.damage)),
                format!("hits: {}", opt_i64(c.measurement.hits)),
                format!("min sample_n: {}", opt_i64(c.min_sample_n())),
            ];
            // wakeup_window/total/recovery are ONE-SIDED (docs/frames.md
            // §4.2/§8.4): each number means "frames until actionable" only
            // relative to the observable that produced it, so name that
            // observable rather than printing a bare, ambiguous frame count.
            for (field, value) in [
                ("wakeup_window", c.measurement.wakeup_window),
                ("total", c.measurement.total),
                ("recovery", c.measurement.recovery),
            ] {
                if let Some(v) = value {
                    let frame = c
                        .one_sided_reference
                        .get(field)
                        .map(|o| format!(" ({o}'s frame of reference)"))
                        .unwrap_or_default();
                    lines.push(format!("{field}: {v}{frame}"));
                }
            }
            if !c.agrees() {
                lines.push(format!("⚠ disagreement: {}", c.disagreements.join(", ")));
            }
            lines.join("\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{FrameMeasurement, FrameObservation};

    fn obs(observable: &str, shape: &str, latency: i64, n: i64, raw: FrameMeasurement) -> FrameObservation {
        FrameObservation {
            observable: observable.to_string(),
            method: "linear_sweep".to_string(),
            input_latency_frames: Some(latency),
            sample_n: Some(n),
            confidence: Some("high".to_string()),
            measured_at: Some("2026-08-30T00:00:00Z".to_string()),
            core_id: Some("core:abc".to_string()),
            rom_id: Some("rom:def".to_string()),
            rig_guard_state: Some(shape.to_string()),
            raw,
        }
    }

    fn measurement(on_hit: Option<i64>, on_block: Option<i64>, knockdown: Option<bool>) -> FrameMeasurement {
        FrameMeasurement {
            on_hit,
            on_block,
            knockdown,
            first_active_frame: Some(11),
            damage: Some(16),
            hits: Some(1),
            ..Default::default()
        }
    }

    /// A synthetic table covering every case the panel needs to render
    /// distinctly: an agreeing measured cell, a disagreeing cell, a
    /// knockdown cell (NULL on_hit, not 0), and a gap with no row at all
    /// (the missing-cell / command-handoff path).
    fn synthetic_table() -> FrameTable {
        let agree_raw = measurement(Some(-7), Some(-14), Some(false));
        let agree = FrameCell {
            char: "reptile".to_string(),
            move_name: "HK".to_string(),
            variant: Some("close".to_string()),
            gap_walk_frames: Some(60),
            measurement: agree_raw.clone(),
            disagreements: vec![],
            one_sided_reference: BTreeMap::new(),
            observations: vec![
                obs("struct_velocity", "held+none", 1, 2, agree_raw.clone()),
                obs("pointer_x", "held+none", 2, 2, agree_raw),
            ],
        };

        let a = measurement(Some(7), Some(-16), Some(false));
        let mut b = a.clone();
        b.on_block = Some(-9); // the two observables disagree here
        let mut collapsed = a.clone();
        collapsed.on_block = None; // never silently resolved to either value
        let disagree = FrameCell {
            char: "reptile".to_string(),
            move_name: "HP".to_string(),
            variant: Some("far".to_string()),
            gap_walk_frames: Some(45),
            measurement: collapsed,
            disagreements: vec!["on_block"],
            one_sided_reference: BTreeMap::new(),
            observations: vec![
                obs("struct_velocity", "held+none", 1, 1, a),
                obs("pointer_x", "held+none", 2, 1, b),
            ],
        };

        let kd_raw = measurement(None, Some(-5), Some(true));
        let knockdown = FrameCell {
            char: "reptile".to_string(),
            move_name: "cHP".to_string(),
            variant: Some("close".to_string()),
            gap_walk_frames: Some(60),
            measurement: kd_raw.clone(),
            disagreements: vec![],
            one_sided_reference: BTreeMap::new(),
            observations: vec![obs("struct_velocity", "held", 1, 1, kd_raw)],
        };

        // Mileena's roll: a ONE-SIDED field (`wakeup_window`) that agrees
        // within the observables' latency delta (77 @ latency 1, 78 @
        // latency 2) rather than to the exact frame — docs/frames.md
        // §4.2/§8.4. This is the case the collapse rule was fixed for.
        let mut struct_raw = FrameMeasurement { wakeup_window: Some(77), ..Default::default() };
        let mut pointer_raw = FrameMeasurement { wakeup_window: Some(78), ..Default::default() };
        struct_raw.knockdown = Some(true);
        pointer_raw.knockdown = Some(true);
        let mut roll_measurement = struct_raw.clone();
        roll_measurement.wakeup_window = Some(77); // collapsed: struct_velocity's frame
        let mut one_sided_reference = BTreeMap::new();
        one_sided_reference.insert("wakeup_window", "struct_velocity".to_string());
        let roll = FrameCell {
            char: "mileena".to_string(),
            move_name: "roll".to_string(),
            variant: None,
            gap_walk_frames: Some(0),
            measurement: roll_measurement,
            disagreements: vec![],
            one_sided_reference,
            observations: vec![
                obs("struct_velocity", "held", 1, 1, struct_raw),
                obs("pointer_x", "held", 2, 1, pointer_raw),
            ],
        };

        FrameTable {
            family: "mk2".to_string(),
            port: "arcade".to_string(),
            generated_at: Some("2026-08-30T00:00:00Z".to_string()),
            schema_version: Some(1),
            // `HK` at gap 45 is intentionally absent — the missing-cell path.
            cells: vec![agree, disagree, knockdown, roll],
        }
    }

    #[test]
    fn cell_style_marks_missing_sparse_disagreement_and_knockdown_distinctly() {
        let table = synthetic_table();
        let missing = table.cell("reptile", "HK", Some(45));
        assert!(missing.is_none());
        let (text, color) = cell_style(missing);
        assert_eq!(text, "·");
        assert_eq!(color, egui::Color32::DARK_GRAY);

        let kd = table.cell("reptile", "cHP", Some(60)).unwrap();
        let (text, _) = cell_style(Some(kd));
        assert!(text.starts_with("KD"), "{text}");
        assert_ne!(text, "0/−5", "knockdown must never render as a numeric 0");

        let disagreed = table.cell("reptile", "HP", Some(45)).unwrap();
        assert!(!disagreed.agrees());
        let (text, color) = cell_style(Some(disagreed));
        assert!(text.contains('⚠'), "{text}");
        assert_eq!(color, egui::Color32::from_rgb(220, 90, 90));
    }

    /// The Startup (FAF) column: a measured move shows its collapsed
    /// `first_active_frame`; a move with no FAF anywhere renders `—` exactly
    /// like the panel's other absent measurements (never 0); disagreeing
    /// values across rows — a §4.4 contract violation — render `⚠`, never a
    /// silently picked winner.
    #[test]
    fn startup_faf_text_renders_value_dash_and_disagreement() {
        let mut table = synthetic_table();
        assert_eq!(startup_faf_text(&table, "reptile", "HK"), "11");
        assert_eq!(startup_faf_text(&table, "mileena", "roll"), "—");

        let mut clash = table.cell("reptile", "HK", Some(60)).unwrap().clone();
        clash.gap_walk_frames = Some(45);
        clash.measurement.first_active_frame = Some(9);
        table.cells.push(clash);
        assert_eq!(startup_faf_text(&table, "reptile", "HK"), "⚠");
    }

    /// A one-sided field's collapsed value must never render as a bare,
    /// ambiguous number — the hover text names the observable whose frame of
    /// reference it's in (docs/frames.md §4.2/§8.4).
    #[test]
    fn cell_hover_text_names_one_sided_reference_observable() {
        let table = synthetic_table();
        let roll = table.cell("mileena", "roll", Some(0)).unwrap();
        assert!(roll.agrees(), "77/78 at latencies 1/2 is agreement");
        let hover = cell_hover_text(Some(roll));
        assert!(hover.contains("wakeup_window: 77"), "{hover}");
        assert!(hover.contains("struct_velocity"), "{hover}");
    }

    /// Render the panel's real entry point headlessly against the process
    /// profile (asurabld via `init_for_tests`, which ships no frames.json —
    /// the "no data" branch) — the only branch reachable through
    /// `profile::current()` in a shared test binary — and separately drive
    /// `render()` directly against a synthetic table with every case above,
    /// including selecting both a present and a missing cell, to hit the
    /// full section set without needing a second global profile.
    #[test]
    fn panel_renders_headlessly_with_and_without_frame_data() {
        crate::profile::init_for_tests();
        let ctx = egui::Context::default();
        let mut panel = FramelabPanel::new();
        let mut ds = DebugState::new();

        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| panel.show(ui, &mut ds));
        });

        let table = synthetic_table();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| panel.render(ui, &table));
        });

        // Selected + missing (command handoff renders the measure command).
        state().lock().unwrap().selected = Some(("HK".to_string(), Some(45)));
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| panel.render(ui, &table));
        });

        // Selected + present (command handoff renders provenance instead).
        state().lock().unwrap().selected = Some(("HK".to_string(), Some(60)));
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| panel.render(ui, &table));
        });

        // Don't leak selection into any other test sharing this static.
        let mut st = state().lock().unwrap();
        st.selected = None;
        st.char = None;
    }
}
