use bevy_egui::egui;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use crate::debug::{DebugState, Watch, WatchFormat};
use crate::record::{
    char_name, matchup_slug, opponent_for_stage_value, stage_value_for_opponent,
};

/// The matchup coverage grid: rows = your character, columns = the opponent,
/// cells = how much demonstration data exists (from the `.rounds.jsonl`
/// sidecars the recorder writes) plus whether a fitted model covers the cell
/// (from `shadow/models/*/meta.json`). Click a cell for details and actions —
/// the fill-the-gaps / test-the-unknowns view of a fixed-roster fighting game.
pub struct MatchupPanel {
    cells: BTreeMap<(u8, u8), Cell>,
    /// Newest fitted model per matchup key: (me, opp) → (dir name, path).
    models: BTreeMap<(Option<u8>, Option<u8>), (String, PathBuf, String)>,
    refreshed: Option<Instant>,
    selected: Option<(u8, u8)>,
}

#[derive(Default, Clone)]
struct Cell {
    rounds: u32,
    frames: u64,
    /// frames per style tag ("(untagged)" for none).
    styles: BTreeMap<String, u64>,
}

fn recordings_dir() -> PathBuf {
    PathBuf::from("shadow/recordings").join(&crate::profile::current().family.family)
}
fn models_dir() -> PathBuf {
    PathBuf::from("shadow/models").join(&crate::profile::current().family.family)
}
fn arenas_dir() -> PathBuf {
    PathBuf::from("shadow/arenas").join(&crate::profile::current().family.family)
}
const REFRESH_SECS: f64 = 3.0;
/// Decisions ≈ live frames / P (the 8-frame decision cadence).
const FRAMES_PER_DECISION: u64 = 8;
/// Below this many (approximate) decisions a cell counts as sparse — enough
/// for the kNN to answer, nowhere near enough to capture a matchup.
const SPARSE_DECISIONS: u64 = 1000;

/// The profile's stage/opponent selector global, resolved by name (None if
/// this game has no such selector — a future game may not have one).
fn stage_select_addr() -> Option<u32> {
    let profile = crate::profile::current();
    let ss = profile.port.stage_select.as_ref()?;
    profile.global(&ss.global)
}

/// The active force, if any: a frozen watch on the stage/opponent selector.
fn current_force(state: &DebugState, addr: u32) -> Option<u8> {
    state
        .watches
        .iter()
        .find(|w| w.addr == addr as usize && w.frozen)
        .and_then(|w| w.frozen_value)
        .map(|v| v as u8)
}

/// Freeze the selector to `v` — the emu thread re-writes it every frame, so
/// every upcoming fight is `v`'s home matchup until cleared. Same mechanism
/// as the Watch panel's freeze checkbox (UI-local, no write gate).
fn set_force(state: &mut DebugState, addr: u32, v: u8) {
    clear_force(state, addr);
    state.watches.push(Watch {
        addr: addr as usize,
        label: format!("⚔ force {} fight", opponent_for_stage_value(v)
            .map(char_name).unwrap_or_default()),
        format: WatchFormat::Hex8,
        frozen: true,
        frozen_value: Some(v as u32),
        track_changes: false,
        current: None,
        prev_value: None,
    });
}

fn clear_force(state: &mut DebugState, addr: u32) {
    state.watches.retain(|w| w.addr != addr as usize || !w.frozen);
}

fn scan_rounds() -> BTreeMap<(u8, u8), Cell> {
    let mut cells: BTreeMap<(u8, u8), Cell> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(recordings_dir()) else { return cells };
    for e in entries.flatten() {
        let path = e.path();
        if !path.to_string_lossy().ends_with(".rounds.jsonl") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
            if v["demo"].as_bool().unwrap_or(true) {
                continue;
            }
            let b1 = v["block1_char"].as_u64().unwrap_or(0) as u8;
            let b2 = v["block2_char"].as_u64().unwrap_or(0) as u8;
            let (me, opp) = match v["p1_block"].as_u64() {
                Some(2) => (b2, b1),
                _ => (b1, b2), // p1_block 1 (or unresolved: block1 = left = P1 start)
            };
            let cell = cells.entry((me, opp)).or_default();
            let frames = v["frames"].as_u64().unwrap_or(0);
            cell.rounds += 1;
            cell.frames += frames;
            let style = v["style"].as_str().unwrap_or("(untagged)").to_string();
            *cell.styles.entry(style).or_default() += frames;
        }
    }
    cells
}

fn scan_model_keys() -> BTreeMap<(Option<u8>, Option<u8>), (String, PathBuf, String)> {
    let mut best: BTreeMap<(Option<u8>, Option<u8>), (String, PathBuf, String)> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(models_dir()) else { return best };
    for e in entries.flatten() {
        let path = e.path();
        let meta_path = path.join("meta.json");
        if !path.join("cases.npz").is_file() || !meta_path.is_file() {
            continue;
        }
        let Ok(meta) = std::fs::read_to_string(&meta_path)
            .map_err(|_| ())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|_| ()))
        else {
            continue;
        };
        let key = (
            meta["char_filter"].as_u64().map(|n| n as u8),
            meta["opp_filter"].as_u64().map(|n| n as u8),
        );
        let created = meta["created"].as_str().unwrap_or("").to_string();
        let name = e.file_name().to_string_lossy().into_owned();
        let newer = best.get(&key).map(|(_, _, c)| created > *c).unwrap_or(true);
        if newer {
            best.insert(key, (name, path, created));
        }
    }
    best
}

impl MatchupPanel {
    pub fn new() -> Self {
        MatchupPanel {
            cells: BTreeMap::new(),
            models: BTreeMap::new(),
            refreshed: None,
            selected: None,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut DebugState) {
        let stale = self
            .refreshed
            .map(|t| t.elapsed().as_secs_f64() > REFRESH_SECS)
            .unwrap_or(true);
        if stale {
            self.cells = scan_rounds();
            self.models = scan_model_keys();
            self.refreshed = Some(Instant::now());
        }

        ui.heading("🥊 Matchup coverage");

        // Force-matchup depends on a stage/opponent selector in the profile
        // — a future game may not have one, in which case the whole
        // mechanism (indicator, boss row, force buttons) is hidden below.
        let stage_addr = stage_select_addr();

        // Active forced matchup (frozen stage/opponent selector) indicator.
        if let Some(addr) = stage_addr {
            if let Some(v) = current_force(state, addr) {
                let who = opponent_for_stage_value(v)
                    .map(char_name)
                    .unwrap_or_else(|| format!("stage {v}"));
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("⚔ forcing every next fight: vs {who}"))
                            .color(egui::Color32::from_rgb(230, 180, 90))
                            .strong(),
                    );
                    if ui.small_button("✕ Clear").clicked() {
                        clear_force(state, addr);
                    }
                });
            }
        }

        if self.cells.is_empty() {
            ui.label(
                "No indexed rounds yet — recordings made from now on are indexed \
                 automatically; run `python -m shadow_train index` to backfill old ones.",
            );
            return;
        }

        // Rows: chars you've played. Columns: the full playable roster (the
        // gaps are the point) plus any extra ids seen (bosses).
        let mes: Vec<u8> = {
            let mut v: Vec<u8> = self.cells.keys().map(|(m, _)| *m).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let opps: Vec<u8> = {
            let mut v: Vec<u8> = crate::profile::current()
                .family
                .roster
                .iter()
                .filter(|r| !r.boss)
                .map(|r| r.id)
                .collect();
            let extra: Vec<u8> =
                self.cells.keys().map(|(_, o)| *o).filter(|o| !v.contains(o)).collect();
            v.extend(extra);
            v.sort_unstable();
            v.dedup();
            v
        };

        egui::Grid::new("matchup_grid").spacing([6.0, 4.0]).show(ui, |ui| {
            ui.label(egui::RichText::new("me \\ opp").small());
            for o in &opps {
                ui.label(egui::RichText::new(char_name(*o)).strong());
            }
            ui.end_row();
            for m in &mes {
                ui.label(egui::RichText::new(char_name(*m)).strong());
                for o in &opps {
                    let cell = self.cells.get(&(*m, *o));
                    let decisions =
                        cell.map(|c| c.frames / FRAMES_PER_DECISION).unwrap_or(0);
                    let has_model = self.models.contains_key(&(Some(*m), Some(*o)));
                    let label = if decisions == 0 {
                        "·".to_string()
                    } else {
                        format!("{decisions}{}", if has_model { " ✓" } else { "" })
                    };
                    let color = if decisions == 0 {
                        egui::Color32::DARK_GRAY
                    } else if decisions < SPARSE_DECISIONS {
                        egui::Color32::from_rgb(230, 180, 90)
                    } else {
                        egui::Color32::from_rgb(150, 220, 150)
                    };
                    let sel = self.selected == Some((*m, *o));
                    let btn = egui::Button::new(egui::RichText::new(label).color(color))
                        .selected(sel);
                    if ui.add(btn).clicked() {
                        self.selected = Some((*m, *o));
                    }
                }
                ui.end_row();
            }
        });
        ui.label(
            egui::RichText::new(format!(
                "≈decisions per cell (frames/{FRAMES_PER_DECISION}, demo rounds excluded); \
                 amber < {SPARSE_DECISIONS}, ✓ = fitted matchup model, · = never demonstrated"
            ))
            .small()
            .color(egui::Color32::DARK_GRAY),
        );

        // Boss fights have no grid column until first fought — quick-force
        // row. Hidden entirely when the profile has no stage selector (a
        // future game may not have one) or the roster has no bosses.
        if let Some(addr) = stage_addr {
            let bosses: Vec<u8> = crate::profile::current()
                .family
                .roster
                .iter()
                .filter(|r| r.boss)
                .map(|r| r.id)
                .collect();
            if !bosses.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Bosses:").small());
                    for boss in bosses {
                        let Some(v) = stage_value_for_opponent(boss) else { continue };
                        if current_force(state, addr) == Some(v) {
                            if ui.small_button(format!("✕ {}", char_name(boss))).clicked() {
                                clear_force(state, addr);
                            }
                        } else if ui
                            .small_button(format!("⚔ {}", char_name(boss)))
                            .on_hover_text("Force the next fight against this boss")
                            .clicked()
                        {
                            set_force(state, addr, v);
                        }
                    }
                });
            }
        }

        // ── Selected-cell detail + actions ────────────────────────────
        let Some((m, o)) = self.selected else { return };
        ui.separator();
        let slug = matchup_slug(m, o);
        ui.label(egui::RichText::new(&slug).strong().size(15.0));
        match self.cells.get(&(m, o)) {
            Some(c) => {
                let styles: Vec<String> = c
                    .styles
                    .iter()
                    .map(|(s, f)| format!("{s} ≈{}", f / FRAMES_PER_DECISION))
                    .collect();
                ui.label(format!(
                    "{} round(s), ≈{} decisions ({})",
                    c.rounds,
                    c.frames / FRAMES_PER_DECISION,
                    styles.join(", "),
                ));
            }
            None => {
                ui.label(
                    egui::RichText::new(
                        "Gap — set up this matchup, save an arena, and record some rounds.",
                    )
                    .color(egui::Color32::from_rgb(230, 180, 90)),
                );
            }
        }
        ui.horizontal(|ui| {
            match self.models.get(&(Some(m), Some(o))) {
                Some((name, path, _)) => {
                    ui.label(format!("model: {name}"));
                    if ui.small_button("Load model").clicked() {
                        state.pending_shadow_load = Some(path.clone());
                    }
                }
                None => {
                    ui.label(
                        egui::RichText::new(format!(
                            "no matchup model — shadow/loop.sh --fit-only --me {m} --opp {o}"
                        ))
                        .small()
                        .color(egui::Color32::DARK_GRAY),
                    );
                }
            }
            let arena = arenas_dir().join(format!("{slug}.state"));
            if arena.is_file() {
                if ui.small_button("Load arena").clicked() {
                    state.pending_state_op = Some(crate::debug::StateOp::Load(arena));
                }
            } else {
                ui.label(
                    egui::RichText::new(format!("(no {slug}.state arena yet)"))
                        .small()
                        .color(egui::Color32::DARK_GRAY),
                );
            }
        });

        // ── Force this matchup (freeze the stage/opponent selector) ───
        // Hidden entirely when the profile has no stage selector.
        if let Some(addr) = stage_addr {
            ui.horizontal(|ui| {
                match stage_value_for_opponent(o) {
                    Some(v) => {
                        if current_force(state, addr) == Some(v) {
                            if ui.small_button("✕ Clear forced matchup").clicked() {
                                clear_force(state, addr);
                            }
                        } else if ui
                            .small_button(format!("⚔ Force next fight vs {}", char_name(o)))
                            .on_hover_text(
                                "Freezes the stage/opponent selector so the NEXT fight \
                                 (after the current one ends) is this opponent on their \
                                 home stage — and every fight after, until cleared. Pick \
                                 your own character normally; this only chooses the \
                                 other side.",
                            )
                            .clicked()
                        {
                            set_force(state, addr, v);
                        }
                    }
                    None => {
                        ui.label(
                            egui::RichText::new(
                                "this opponent has no selector value — reach them via the ladder",
                            )
                            .small()
                            .color(egui::Color32::DARK_GRAY),
                        );
                    }
                }
            });
        }
    }
}
