//! 🎛 Controls panel — view and REBIND controls in the game's action
//! vocabulary (docs/game-profiles.md "Controls contract").
//!
//! Rows come from `input_config::action_rows(port, descriptors)` — the same
//! resolver every human-facing surface uses. Bindings are REVERSE-looked-up:
//! a physical control is "bound to" an action when its chord equals the
//! action's RETRO bits *as a set*. Rebinding replaces every control whose
//! chord equals the action's bits for that device kind, and warns (inline,
//! non-blocking) when the chosen physical control was stolen from a
//! different action.
//!
//! ## Integration contract (wired by main.rs — NOT this file)
//!
//! 1. `src/debug/panels/mod.rs` gets `pub mod controls;` (done in this PR).
//! 2. `main.rs` inserts the resource: `App::insert_resource(ControlsPanel::new())`.
//! 3. `main.rs` registers the system in the egui pass, after `show_debug`:
//!    `show_controls_panel` — signature below; it takes `EguiContexts`,
//!    `ResMut<ControlsPanel>`, `ResMut<input_config::InputConfig>`,
//!    `Res<ButtonInput<KeyCode>>`, and
//!    `Query<(Entity, Option<&Name>, &Gamepad)>` (the `Name` component is
//!    how Bevy 0.18 exposes the pad's reported device name — see
//!    `bevy_input/src/gamepad.rs`, gamepads spawn with `Name::new(...)`).
//! 4. `main.rs`'s `read_input` calls `panel.sync_descriptors(&ds)` while it
//!    already holds the `DebugState` lock (or any per-frame system that has
//!    both) — this fills `descriptors`, `save_dir`, `rom_stem` from
//!    `DebugState::{input_descriptors, state_dir, rom_name}`. This panel
//!    never touches the `Arc<Mutex<DebugState>>` itself.
//! 5. `main.rs`'s `read_input` must SKIP folding keyboard+gamepad into game
//!    input while `panel.capturing()` is true, so a captured control doesn't
//!    keep firing its old action. (Without that skip the captured press also
//!    fires its previous binding for one frame — acceptable, documented.)
//! 6. F11 toggles `panel.open` (a `read_input` hotkey; remember to add the
//!    row to `KEYBINDINGS` in the same commit).
//!
//! ## Capture state machine
//!
//! `capture: Option<CaptureTarget>` — `None` = idle. Clicking a binding cell
//! sets `Some((port, action bits, device kind, optional device map))`. Each
//! frame while capturing (checked BEFORE rendering, in `show_controls_panel`):
//!   * Esc just-pressed → cancel, no change.
//!   * Keyboard target: first just-pressed key passing `key_is_capturable`
//!     → `rebind_key`, capture ends.
//!   * Gamepad target: first just-pressed button on any connected pad
//!     → `rebind_pad`, capture ends. The destination map is the clicked
//!     device row's map if the click was on a device-specific sub-row, else
//!     the pressing pad's `gamepad_by_device` entry if one exists, else the
//!     generic `gamepad` map.
//! Closing the window also cancels capture. Only the targeted device kind is
//! listened to (a pad press is ignored while capturing a keyboard cell, and
//! vice versa).
//!
//! ## Keys excluded from capture (`key_is_capturable`)
//!
//! * `Escape` — reserved as the capture-cancel key.
//! * `F1`–`F35` — app hotkeys (training, save states, panels, F11 itself,
//!   debugger) and their future siblings.
//! * `Space` — the pause hotkey; binding it would toggle pause on every use.
//! * `KeyB` — the bookmark hotkey (unconditional in `read_input`).
//! * Modifiers (`Shift`/`Control`/`Alt`/`Super`, both sides) — they modify
//!   other hotkeys (Shift+F5/F6/F7). NOTE: the built-in default binds
//!   Shift → Coin; that binding still displays and works — it just cannot be
//!   *re-created* through panel capture (edit keymap.json for that).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::ecs::entity::Entity;
use bevy::ecs::name::Name;
use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::input::keyboard::KeyCode;
use bevy::input::ButtonInput;
use bevy::prelude::{Query, Res, ResMut, Resource};
use bevy_egui::{egui, EguiContexts};

use crate::input_config::{self, action_rows, Chord, InputConfig, PortMap, RetroButton};

// ─── Resource ────────────────────────────────────────────────────────────────

/// Which device kind a capture is listening to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Keyboard,
    Gamepad,
}

/// An in-progress rebind: which cell was clicked.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureTarget {
    /// Port index (0 = P1, 1 = P2).
    pub port: usize,
    /// Display name of the action (for status messages / cell matching).
    pub action: String,
    /// The RETRO bits this action performs — the chord the new control will emit.
    pub bits: Vec<RetroButton>,
    pub kind: DeviceKind,
    /// `Some(name)` when a device-specific sub-row was clicked: the rebind
    /// goes into `gamepad_by_device[name]`. `None` = generic cell (the
    /// pressing pad may still be routed to its own device map — see module
    /// docs). Always `None` for keyboard captures.
    pub device: Option<String>,
}

/// 🎛 Controls panel state. Lives as a Bevy `Resource`; all cross-thread data
/// arrives via [`sync_descriptors`](ControlsPanel::sync_descriptors).
#[derive(Resource)]
pub struct ControlsPanel {
    /// Window visibility — F11 toggles this (integrator's wiring).
    pub open: bool,
    /// `Some` while waiting for a key/button press. **Integration contract:**
    /// `read_input` must skip game-input folding while this is `Some` (use
    /// [`capturing`](ControlsPanel::capturing)).
    pub capture: Option<CaptureTarget>,
    /// Cache of `DebugState::input_descriptors`, filled by `sync_descriptors`.
    pub descriptors: [[Option<String>; 12]; 2],
    /// Where keymap.json lives — mirrored from `DebugState::state_dir`
    /// (the Frontend publishes save_dir there).
    pub save_dir: PathBuf,
    /// ROM file stem for the per-game sidecar — mirrored from
    /// `DebugState::rom_name`.
    pub rom_stem: Option<String>,
    /// Sticky one-line result of the last save / revert / capture.
    pub status: String,
    /// Inline non-blocking warning from the last capture (yellow), e.g.
    /// "South was bound to Light — overwritten". Cleared on the next capture.
    pub warning: Option<String>,
}

impl ControlsPanel {
    pub fn new() -> Self {
        ControlsPanel {
            open: false,
            capture: None,
            descriptors: Default::default(),
            save_dir: PathBuf::from("."),
            rom_stem: None,
            status: String::new(),
            warning: None,
        }
    }

    /// True while a rebind capture is armed. `read_input` (integrator-owned)
    /// checks this to suppress game-input folding for the frame.
    pub fn capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// Mirror the panel's per-session inputs out of `DebugState`. The
    /// integrator calls this from a system that already holds the lock
    /// (e.g. inside `read_input`); cheap enough to call every frame.
    pub fn sync_descriptors(&mut self, ds: &crate::debug::DebugState) {
        self.descriptors = ds.input_descriptors.clone();
        if let Some(dir) = &ds.state_dir {
            self.save_dir = dir.clone();
        }
        if ds.rom_name.is_some() {
            self.rom_stem = ds.rom_name.clone();
        }
    }
}

// ─── Pure rebind / lookup logic (unit-tested, no Bevy runtime) ───────────────

/// Chord → bitmask, so chord equality is SET equality (order/dup insensitive).
pub(crate) fn chord_mask(bits: &[RetroButton]) -> u16 {
    bits.iter().fold(0u16, |m, b| m | (1 << b.idx()))
}

/// Controls in `map` whose chord equals `bits` as a set (reverse lookup).
pub(crate) fn bindings_in_map<K: Ord + Copy>(
    map: &BTreeMap<K, Chord>,
    bits: &[RetroButton],
) -> Vec<K> {
    let want = chord_mask(bits);
    map.iter()
        .filter(|(_, c)| chord_mask(&c.0) == want)
        .map(|(k, _)| *k)
        .collect()
}

/// Is the action bound to ANYTHING on this port (keyboard, generic pad, or
/// any device-specific map)? Directions are additionally always reachable
/// through the structural left stick, but that is not a *binding*.
pub(crate) fn action_is_bound(port: &PortMap, bits: &[RetroButton]) -> bool {
    !bindings_in_map(&port.keyboard, bits).is_empty()
        || !bindings_in_map(&port.gamepad, bits).is_empty()
        || port
            .gamepad_by_device
            .values()
            .any(|m| !bindings_in_map(m, bits).is_empty())
}

/// Result of a rebind in one map.
pub(crate) struct RebindOutcome<K> {
    /// Controls that previously emitted this action's chord and were removed
    /// (the replace-binding semantics). Does not include `control` itself.
    pub replaced: Vec<K>,
    /// `control`'s previous chord when it was bound to a DIFFERENT action —
    /// the "stolen from" warning payload. `None` if it was unbound or
    /// already bound to this same action.
    pub stolen: Option<Chord>,
}

/// Core rebind semantics on one map: remove every control whose chord equals
/// `bits` (set-equal), then bind `control` → `bits`, reporting what was
/// replaced and whether `control` was stolen from another action.
pub(crate) fn rebind_in_map<K: Ord + Copy>(
    map: &mut BTreeMap<K, Chord>,
    bits: &[RetroButton],
    control: K,
) -> RebindOutcome<K> {
    let want = chord_mask(bits);
    let replaced: Vec<K> = map
        .iter()
        .filter(|(k, c)| **k != control && chord_mask(&c.0) == want)
        .map(|(k, _)| *k)
        .collect();
    for k in &replaced {
        map.remove(k);
    }
    let old = map.insert(control, Chord(bits.to_vec()));
    let stolen = old.filter(|c| chord_mask(&c.0) != want);
    RebindOutcome { replaced, stolen }
}

/// Rebind `key` to an action on a port's KEYBOARD map.
pub(crate) fn rebind_key(
    port: &mut PortMap,
    bits: &[RetroButton],
    key: KeyCode,
) -> RebindOutcome<KeyCode> {
    rebind_in_map(&mut port.keyboard, bits, key)
}

/// Rebind `btn` to an action on a port's gamepad side. `device`:
/// `Some(name)` targets `gamepad_by_device[name]` **when that map exists**,
/// otherwise (or on `None`) the generic `gamepad` map — a capture never
/// creates a device map implicitly (the "＋ device map" button does that).
pub(crate) fn rebind_pad(
    port: &mut PortMap,
    bits: &[RetroButton],
    btn: GamepadButton,
    device: Option<&str>,
) -> RebindOutcome<GamepadButton> {
    let map = device
        .and_then(|n| port.gamepad_by_device.get_mut(n))
        .unwrap_or(&mut port.gamepad);
    rebind_in_map(map, bits, btn)
}

/// Clone the generic gamepad map into a device-specific map for `name`.
/// Returns false (no-op) when one already exists.
pub(crate) fn add_device_map(port: &mut PortMap, name: &str) -> bool {
    if port.gamepad_by_device.contains_key(name) {
        return false;
    }
    let generic = port.gamepad.clone();
    port.gamepad_by_device.insert(name.to_string(), generic);
    true
}

/// May this key become a binding via capture? (Exclusion list in module docs.)
pub(crate) fn key_is_capturable(k: KeyCode) -> bool {
    use KeyCode::*;
    !matches!(
        k,
        Escape
            | Space
            | KeyB
            | ShiftLeft | ShiftRight
            | ControlLeft | ControlRight
            | AltLeft | AltRight
            | SuperLeft | SuperRight
            | F1 | F2 | F3 | F4 | F5 | F6 | F7 | F8 | F9 | F10 | F11 | F12
            | F13 | F14 | F15 | F16 | F17 | F18 | F19 | F20 | F21 | F22 | F23
            | F24 | F25 | F26 | F27 | F28 | F29 | F30 | F31 | F32 | F33 | F34
            | F35
    )
}

/// Serialize the active config to `path` (pretty JSON, same shape
/// `InputConfig::load` reads back). Returns a status line either way.
pub(crate) fn save_to(cfg: &InputConfig, path: &Path) -> Result<String, String> {
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("serialize failed: {e}"))?;
    std::fs::write(path, &json)
        .map(|()| format!("saved {}", path.display()))
        .map_err(|e| format!("write {} failed: {e}", path.display()))
}

// ─── Display helpers ─────────────────────────────────────────────────────────

/// Delegates to `input_config::key_name` (single display-name source).
fn key_label(k: KeyCode) -> String {
    crate::input_config::key_name(k)
}

fn chord_label(bits: &[RetroButton]) -> String {
    bits.iter()
        .map(|b| format!("{b:?}"))
        .collect::<Vec<_>>()
        .join("+")
}

/// Resolve a stolen chord back to an action name for the warning text
/// ("…was bound to Light"), falling back to the raw chord ("B+A").
fn action_name_for_chord(
    port: usize,
    descriptors: &[[Option<String>; 12]; 2],
    chord: &Chord,
) -> String {
    let want = chord_mask(&chord.0);
    action_rows(port, descriptors)
        .into_iter()
        .find(|r| chord_mask(&r.bits) == want)
        .map(|r| r.name)
        .unwrap_or_else(|| chord_label(&chord.0))
}

const AMBER: egui::Color32 = egui::Color32::from_rgb(255, 190, 70);
const YELLOW: egui::Color32 = egui::Color32::from_rgb(255, 220, 80);
const GRAY: egui::Color32 = egui::Color32::from_rgb(150, 150, 150);

// ─── The Bevy system ─────────────────────────────────────────────────────────

/// Render the 🎛 Controls window and run the capture state machine.
///
/// Integrator: register in the egui pass after `show_debug`; see the module
/// docs for the full wiring contract (resource insertion, `sync_descriptors`
/// call site, the `capturing()` read_input skip, F11).
pub fn show_controls_panel(
    mut ctx: EguiContexts,
    mut panel: ResMut<ControlsPanel>,
    mut cfg: ResMut<InputConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<(Entity, Option<&Name>, &Gamepad)>,
) {
    let panel = &mut *panel;
    let cfg = &mut *cfg;

    if !panel.open {
        panel.capture = None; // closing the window cancels capture
        return;
    }

    // Connected pads, lowest entity id first (same order read_input folds them).
    let mut pad_list: Vec<(Entity, Option<&Name>, &Gamepad)> = pads.iter().collect();
    pad_list.sort_by_key(|(e, _, _)| *e);

    step_capture(panel, cfg, &keys, &pad_list);

    let Ok(ctx) = ctx.ctx_mut() else { return };
    let mut open = panel.open;
    egui::Window::new("🎛 Controls")
        .open(&mut open)
        .resizable(true)
        .default_width(560.0)
        .show(ctx, |ui| {
            render_contents(ui, panel, cfg, &pad_list);
        });
    panel.open = open;
    if !panel.open {
        panel.capture = None;
    }
}

/// One tick of the capture state machine (module docs). Runs BEFORE render so
/// the completed rebind is visible the same frame.
fn step_capture(
    panel: &mut ControlsPanel,
    cfg: &mut InputConfig,
    keys: &ButtonInput<KeyCode>,
    pad_list: &[(Entity, Option<&Name>, &Gamepad)],
) {
    let Some(target) = panel.capture.clone() else { return };
    if target.port >= cfg.ports.len() {
        panel.capture = None;
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        panel.capture = None;
        panel.status = "capture cancelled".into();
        return;
    }
    match target.kind {
        DeviceKind::Keyboard => {
            let Some(&key) = keys.get_just_pressed().find(|k| key_is_capturable(**k))
            else {
                return;
            };
            let out = rebind_key(&mut cfg.ports[target.port], &target.bits, key);
            finish_capture(
                panel,
                &target,
                key_label(key),
                out.replaced.iter().map(|k| key_label(*k)).collect(),
                out.stolen,
                None,
            );
        }
        DeviceKind::Gamepad => {
            // First just-pressed button on any connected pad wins.
            let Some((pad_name, btn)) = pad_list.iter().find_map(|(_, name, pad)| {
                pad.get_just_pressed()
                    .next()
                    .map(|b| (name.map(|n| n.as_str().to_string()), *b))
            }) else {
                return;
            };
            let port_map = &mut cfg.ports[target.port];
            // Destination map: clicked device sub-row > pressing pad's own
            // device map (if one exists) > generic. See rebind_pad docs.
            let device: Option<String> = target.device.clone().or_else(|| {
                pad_name.filter(|n| port_map.gamepad_by_device.contains_key(n))
            });
            let out = rebind_pad(port_map, &target.bits, btn, device.as_deref());
            finish_capture(
                panel,
                &target,
                format!("{btn:?}"),
                out.replaced.iter().map(|b| format!("{b:?}")).collect(),
                out.stolen,
                device,
            );
        }
    }
}

/// Shared capture epilogue: status line, stolen-binding warning, disarm.
fn finish_capture(
    panel: &mut ControlsPanel,
    target: &CaptureTarget,
    control_label: String,
    replaced: Vec<String>,
    stolen: Option<Chord>,
    device: Option<String>,
) {
    let where_ = match device {
        Some(d) => format!(" [{d}]"),
        None => String::new(),
    };
    let mut status = format!("{} = {control_label}{where_}", target.action);
    if !replaced.is_empty() {
        status.push_str(&format!("  (was {})", replaced.join(" / ")));
    }
    panel.status = status;
    panel.warning = stolen.map(|old| {
        format!(
            "{control_label} was bound to {} — overwritten",
            action_name_for_chord(target.port, &panel.descriptors, &old)
        )
    });
    panel.capture = None;
}

// ─── Rendering ───────────────────────────────────────────────────────────────

fn render_contents(
    ui: &mut egui::Ui,
    panel: &mut ControlsPanel,
    cfg: &mut InputConfig,
    pad_list: &[(Entity, Option<&Name>, &Gamepad)],
) {
    ui.label(
        egui::RichText::new(
            "Click a binding to rebind — then press the new key / pad button. \
             Esc cancels. The old binding for that action is replaced.",
        )
        .size(11.0)
        .color(GRAY),
    );
    ui.separator();

    let ports = cfg.ports.len().min(2);
    for port_idx in 0..ports {
        let rows = action_rows(port_idx, &panel.descriptors);
        egui::CollapsingHeader::new(format!("Player {}", port_idx + 1))
            .default_open(port_idx == 0)
            .show(ui, |ui| {
                render_port(ui, panel, cfg, port_idx, &rows, pad_list);
            });
    }

    ui.separator();
    if let Some(w) = &panel.warning {
        ui.label(egui::RichText::new(format!("⚠ {w}")).color(YELLOW).size(11.5));
    }

    // ── Persistence row ──────────────────────────────────────────────────
    ui.horizontal(|ui| {
        if ui
            .button("Save (global keymap.json)")
            .on_hover_text(format!(
                "Write {}",
                InputConfig::global_path(&panel.save_dir).display()
            ))
            .clicked()
        {
            let path = InputConfig::global_path(&panel.save_dir);
            panel.status = save_to(cfg, &path).unwrap_or_else(|e| e);
        }
        let stem = panel.rom_stem.clone();
        let label = match &stem {
            Some(s) => format!("Save for this game ({s}.keymap.json)"),
            None => "Save for this game".into(),
        };
        if ui
            .add_enabled(stem.is_some(), egui::Button::new(label))
            .on_hover_text("Per-ROM sidecar — loaded in preference to the global keymap")
            .clicked()
        {
            if let Some(s) = stem {
                let path = panel.save_dir.join(format!("{s}.keymap.json"));
                panel.status = save_to(cfg, &path).unwrap_or_else(|e| e);
            }
        }
        if ui
            .button("Revert to defaults")
            .on_hover_text("Discard all edits — built-in default maps (unsaved)")
            .clicked()
        {
            *cfg = InputConfig::default();
            panel.warning = None;
            panel.status = "reverted to built-in defaults (not saved)".into();
        }
    });
    if !panel.status.is_empty() {
        ui.label(
            egui::RichText::new(&panel.status)
                .monospace()
                .size(11.0)
                .color(egui::Color32::LIGHT_GRAY),
        );
    }
}

fn render_port(
    ui: &mut egui::Ui,
    panel: &mut ControlsPanel,
    cfg: &mut InputConfig,
    port_idx: usize,
    rows: &[input_config::ActionRow],
    pad_list: &[(Entity, Option<&Name>, &Gamepad)],
) {
    // ── ＋ device-specific map for the first connected pad lacking one ────
    let first_unmapped: Option<String> = pad_list
        .iter()
        .filter_map(|(_, name, _)| name.map(|n| n.as_str().to_string()))
        .find(|n| !cfg.ports[port_idx].gamepad_by_device.contains_key(n));
    if let Some(name) = first_unmapped {
        if ui
            .button(format!("＋ device-specific map for {name}"))
            .on_hover_text(
                "Clone the generic gamepad map into a map that applies only \
                 to this device (lets a fightstick and a normal pad differ)",
            )
            .clicked()
        {
            add_device_map(&mut cfg.ports[port_idx], &name);
            panel.status = format!("added device map for {name} (P{})", port_idx + 1);
        }
    }

    let device_names: Vec<String> =
        cfg.ports[port_idx].gamepad_by_device.keys().cloned().collect();

    egui::Grid::new(format!("controls_grid_p{port_idx}"))
        .striped(true)
        .min_col_width(110.0)
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Action").strong());
            ui.label(egui::RichText::new("Keyboard").strong());
            ui.label(egui::RichText::new("Gamepad").strong());
            ui.end_row();

            for row in rows {
                let port = &cfg.ports[port_idx];
                let bound = action_is_bound(port, &row.bits);

                // Action name (+ RETRO bits, small gray). Amber when unbound.
                ui.horizontal(|ui| {
                    let name_color = if bound {
                        ui.visuals().text_color()
                    } else {
                        AMBER
                    };
                    ui.label(egui::RichText::new(&row.name).color(name_color));
                    ui.label(
                        egui::RichText::new(format!("({})", chord_label(&row.bits)))
                            .size(10.0)
                            .color(GRAY),
                    );
                });

                // Keyboard cell.
                let kb = bindings_in_map(&port.keyboard, &row.bits);
                let kb_text = if kb.is_empty() {
                    "—".to_string()
                } else {
                    kb.iter().map(|k| key_label(*k)).collect::<Vec<_>>().join(" / ")
                };
                binding_cell(
                    ui,
                    panel,
                    kb_text,
                    CaptureTarget {
                        port: port_idx,
                        action: row.name.clone(),
                        bits: row.bits.clone(),
                        kind: DeviceKind::Keyboard,
                        device: None,
                    },
                );

                // Gamepad cell: generic row + one sub-row per device map.
                ui.vertical(|ui| {
                    let generic = bindings_in_map(&port.gamepad, &row.bits);
                    let gen_text = if generic.is_empty() {
                        "—".to_string()
                    } else {
                        generic
                            .iter()
                            .map(|b| format!("{b:?}"))
                            .collect::<Vec<_>>()
                            .join(" / ")
                    };
                    binding_cell(
                        ui,
                        panel,
                        gen_text,
                        CaptureTarget {
                            port: port_idx,
                            action: row.name.clone(),
                            bits: row.bits.clone(),
                            kind: DeviceKind::Gamepad,
                            device: None,
                        },
                    );
                    for dev in &device_names {
                        let map = &cfg.ports[port_idx].gamepad_by_device[dev];
                        let found = bindings_in_map(map, &row.bits);
                        let text = if found.is_empty() {
                            format!("[{dev}] —")
                        } else {
                            format!(
                                "[{dev}] {}",
                                found
                                    .iter()
                                    .map(|b| format!("{b:?}"))
                                    .collect::<Vec<_>>()
                                    .join(" / ")
                            )
                        };
                        binding_cell(
                            ui,
                            panel,
                            text,
                            CaptureTarget {
                                port: port_idx,
                                action: row.name.clone(),
                                bits: row.bits.clone(),
                                kind: DeviceKind::Gamepad,
                                device: Some(dev.clone()),
                            },
                        );
                    }
                });
                ui.end_row();
            }
        });
}

/// One clickable binding cell. Shows the capture prompt while this exact cell
/// is being captured; a click arms capture for it (and clears any warning).
fn binding_cell(
    ui: &mut egui::Ui,
    panel: &mut ControlsPanel,
    text: String,
    target: CaptureTarget,
) {
    if panel.capture.as_ref() == Some(&target) {
        let what = match target.kind {
            DeviceKind::Keyboard => "key",
            DeviceKind::Gamepad => "button",
        };
        ui.label(
            egui::RichText::new(format!("press a {what}… (Esc cancels)"))
                .italics()
                .color(YELLOW),
        );
        return;
    }
    if ui
        .button(egui::RichText::new(text).monospace().size(11.5))
        .on_hover_text("Click, then press the new binding")
        .clicked()
    {
        panel.warning = None;
        panel.capture = Some(target);
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use GamepadButton as G;
    use KeyCode as K;
    use RetroButton::*;

    #[test]
    fn chord_mask_is_set_equality() {
        assert_eq!(chord_mask(&[B, A, Y]), chord_mask(&[Y, B, A]));
        assert_eq!(chord_mask(&[B, B, A]), chord_mask(&[A, B]));
        assert_ne!(chord_mask(&[B, A]), chord_mask(&[B, A, Y]));
        assert_eq!(chord_mask(&[]), 0);
    }

    #[test]
    fn reverse_lookup_finds_chords_and_multiples() {
        let cfg = InputConfig::default();
        let p1 = &cfg.ports[0];
        // Toss chord B+A+Y → RightTrigger on the default F300 map,
        // regardless of the order the bits are asked in.
        assert_eq!(bindings_in_map(&p1.gamepad, &[Y, A, B]), vec![G::RightTrigger]);
        // Coin: two keyboard keys share the binding (Shift L/R → Select).
        let coins = bindings_in_map(&p1.keyboard, &[Select]);
        assert_eq!(coins, vec![K::ShiftLeft, K::ShiftRight]);
        // Start on the pad: two physical triggers.
        assert_eq!(
            bindings_in_map(&p1.gamepad, &[Start]),
            vec![G::LeftTrigger2, G::RightTrigger2]
        );
        // Launcher (B+A) must NOT match Toss (B+A+Y) or Light (B).
        assert_eq!(bindings_in_map(&p1.gamepad, &[B, A]), vec![G::Mode]);
    }

    #[test]
    fn rebind_key_replaces_same_action_binding() {
        let mut cfg = InputConfig::default();
        // Light = [B], currently KeyZ. Rebind to KeyC.
        let out = rebind_key(&mut cfg.ports[0], &[B], K::KeyC);
        assert_eq!(out.replaced, vec![K::KeyZ]);
        assert!(out.stolen.is_none(), "KeyC was unbound — nothing stolen");
        assert!(!cfg.ports[0].keyboard.contains_key(&K::KeyZ));
        assert_eq!(cfg.ports[0].keyboard[&K::KeyC].0, vec![B]);
        // key_bits agrees end-to-end.
        let bits = input_config::key_bits(|k| k == K::KeyC, &cfg.ports[0]);
        assert!(bits[B.idx()]);
    }

    #[test]
    fn rebind_key_reports_stolen_binding() {
        let mut cfg = InputConfig::default();
        // KeyZ is Light ([B]); steal it for Medium ([A]).
        let out = rebind_key(&mut cfg.ports[0], &[A], K::KeyZ);
        assert_eq!(out.stolen.as_ref().map(|c| c.0.clone()), Some(vec![B]));
        // Old Medium key removed, KeyZ now emits A, Light left keyboard-unbound.
        assert_eq!(out.replaced, vec![K::KeyX]);
        assert_eq!(cfg.ports[0].keyboard[&K::KeyZ].0, vec![A]);
        assert!(bindings_in_map(&cfg.ports[0].keyboard, &[B]).is_empty());
        // Rebinding to the SAME action is not "stolen".
        let again = rebind_key(&mut cfg.ports[0], &[A], K::KeyZ);
        assert!(again.stolen.is_none());
        assert!(again.replaced.is_empty());
    }

    #[test]
    fn rebind_pad_targets_device_map_when_present_else_generic() {
        let mut cfg = InputConfig::default();
        assert!(add_device_map(&mut cfg.ports[0], "TestStick"));
        assert!(!add_device_map(&mut cfg.ports[0], "TestStick"), "no clobber");
        // Device map starts as a clone of generic.
        assert_eq!(
            cfg.ports[0].gamepad_by_device["TestStick"],
            cfg.ports[0].gamepad
        );
        // Rebind Light → East inside the device map only.
        let out = rebind_pad(&mut cfg.ports[0], &[B], G::East, Some("TestStick"));
        assert_eq!(out.replaced, vec![G::South]);
        // East was X in the clone — stolen warning fires.
        assert_eq!(out.stolen.as_ref().map(|c| c.0.clone()), Some(vec![X]));
        let dev = &cfg.ports[0].gamepad_by_device["TestStick"];
        assert_eq!(bindings_in_map(dev, &[B]), vec![G::East]);
        // Generic map untouched: South still Light.
        assert_eq!(bindings_in_map(&cfg.ports[0].gamepad, &[B]), vec![G::South]);
        // Unknown device name falls back to the generic map (never creates).
        let out2 = rebind_pad(&mut cfg.ports[0], &[B], G::East, Some("Nope"));
        assert_eq!(out2.replaced, vec![G::South]);
        assert_eq!(bindings_in_map(&cfg.ports[0].gamepad, &[B]), vec![G::East]);
        assert!(!cfg.ports[0].gamepad_by_device.contains_key("Nope"));
    }

    #[test]
    fn action_is_bound_scans_all_maps() {
        let mut cfg = InputConfig::default();
        assert!(action_is_bound(&cfg.ports[0], &[B, A, Y])); // toss: pad only
        // Unbind toss everywhere → unbound (amber in the UI).
        cfg.ports[0].gamepad.remove(&G::RightTrigger);
        assert!(!action_is_bound(&cfg.ports[0], &[B, A, Y]));
        // A device-specific map alone counts as bound.
        let mut m = std::collections::BTreeMap::new();
        m.insert(G::North, Chord(vec![B, A, Y]));
        cfg.ports[0].gamepad_by_device.insert("Stick".into(), m);
        assert!(action_is_bound(&cfg.ports[0], &[B, A, Y]));
    }

    #[test]
    fn capture_exclusions() {
        for k in [
            K::Escape, K::Space, K::KeyB, K::F1, K::F5, K::F11, K::F12, K::F35,
            K::ShiftLeft, K::ShiftRight, K::ControlLeft, K::AltRight, K::SuperLeft,
        ] {
            assert!(!key_is_capturable(k), "{k:?} must be excluded");
        }
        for k in [K::KeyZ, K::Enter, K::ArrowUp, K::Digit1, K::Comma, K::Tab] {
            assert!(key_is_capturable(k), "{k:?} must be capturable");
        }
    }

    #[test]
    fn save_round_trips_through_load_format() {
        let mut cfg = InputConfig::default();
        rebind_key(&mut cfg.ports[0], &[B], K::KeyC);
        rebind_pad(&mut cfg.ports[1], &[Start], G::Select, None);
        add_device_map(&mut cfg.ports[0], "TestStick");
        let path = std::env::temp_dir().join(format!(
            "rustretro-controls-test-{}.keymap.json",
            std::process::id()
        ));
        let msg = save_to(&cfg, &path).expect("save should succeed");
        assert!(msg.contains("saved"));
        let text = std::fs::read_to_string(&path).unwrap();
        let back: InputConfig = serde_json::from_str(&text).unwrap();
        assert_eq!(
            serde_json::to_string(&back).unwrap(),
            serde_json::to_string(&cfg).unwrap()
        );
        assert_eq!(back.ports[0].keyboard[&K::KeyC].0, vec![B]);
        assert!(back.ports[0].gamepad_by_device.contains_key("TestStick"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stolen_chord_resolves_to_action_name() {
        crate::profile::init_for_tests();
        let no_desc: [[Option<String>; 12]; 2] = Default::default();
        assert_eq!(action_name_for_chord(0, &no_desc, &Chord(vec![B])), "Light");
        assert_eq!(
            action_name_for_chord(0, &no_desc, &Chord(vec![Y, A, B])),
            "Toss"
        );
        // A chord no action owns falls back to the raw bit names.
        assert_eq!(
            action_name_for_chord(0, &no_desc, &Chord(vec![L, R])),
            "L+R"
        );
    }
}
