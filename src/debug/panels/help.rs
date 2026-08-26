use bevy_egui::egui;

use crate::debug::DebugState;

/// Help panel for the debug window. Keybindings render from the single
/// `crate::KEYBINDINGS` table (defined next to the hotkey handler in main.rs)
/// and the game-control rows from `DebugState::keymap_lines` (the ACTIVE
/// resolved keymap) — nothing here is a hand-kept copy.
pub struct HelpPanel;

impl HelpPanel {
    pub fn new() -> Self {
        HelpPanel
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &DebugState) {
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.heading("RustRetro Debugger");
            ui.label("A debugging-first libretro frontend for reverse-engineering retro games, focused on fighting games.");
            ui.separator();

            ui.heading("Keybindings");
            for (group, binds) in crate::KEYBINDINGS {
                ui.label(egui::RichText::new(*group).strong());
                egui::Grid::new(format!("help_keys_{group}"))
                    .num_columns(2)
                    .spacing([16.0, 2.0])
                    .show(ui, |ui| {
                        for (key, action) in *binds {
                            ui.monospace(*key);
                            ui.label(*action);
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
            }

            ui.heading("Game controls (active keymap)");
            if state.keymap_lines.is_empty() {
                ui.label("No keymap loaded.");
            } else {
                for (label, mapping) in &state.keymap_lines {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(format!("{label}:")).strong());
                        ui.monospace(mapping);
                    });
                }
            }
            ui.label("Custom bindings: keymap.json / --keymap PATH; run --calibrate for the wizard, --dump-keymap to inspect.");
            ui.separator();

            ui.heading("Panel regions");
            ui.label("The default layout groups panels by how they're used (drag tabs to taste; ☰ menu saves/restores layouts and reopens closed panels):");
            ui.label("• Canvas (center) — big things you look at: Frame, Disasm, Hex, Tiles");
            ui.label("• Live (top right) — glanceable readouts: Watch, CPU, Input");
            ui.label("• Control (bottom right) — things you operate: State, Training, Audio");
            ui.label("• Tools (bottom) — on-demand: Search, Triggers, Regions, VDP, Log, Help");
            ui.separator();

            ui.heading("Panels");
            ui.label("🖼 Frame      Live framebuffer, pixel inspector, zoom");
            ui.label("📜 Disasm     Capstone M68K; breakpoints; run-to");
            ui.label("📋 Hex        Hex+ASCII dump of any memory region");
            ui.label("🧩 Tiles      8×8 VRAM tile browser");
            ui.label("👁 Watch      Address watches: freeze, track-changes log");
            ui.label("🔧 CPU        M68K & Z80 registers; delta highlights");
            ui.label("🕹 Input      Button state + 120-frame input history");
            ui.label("💾 State      Save-state slots: save/load/inspect");
            ui.label("🎯 Training   Training mode + shadow bot controls");
            ui.label("🔊 Audio      Volume, mute, sample rate display");
            ui.label("🔍 Search     Iterative RAM value narrowing");
            ui.label("⏸ Triggers   Frame-count and pixel-value pauses");
            ui.label("🗺 Regions    Bookmarks, PC heatmap, code regions");
            ui.label("📺 VDP        Genesis VDP register decoder");
            ui.label("🧾 Log        Scrollable event log with filter");
            ui.separator();

            ui.heading("Tutorials");
            ui.label("Task-oriented walkthroughs live in docs/tutorials/ (one per feature).");
            ui.label("Start with getting-started.md, then ram-search.md (find a health bar).");
            ui.label("Press F8 to open them in-app: each is a litui page rendered as a Help → Tutorials screen.");
            ui.separator();

            ui.heading("About");
            ui.label("RustRetro loads libretro cores (Genesis, CPS-2, NES) and provides first-class debugging facilities.");
            ui.label("Built with Bevy (rendering), egui (UI), and Capstone (disassembly).");
            let profile = crate::profile::current();
            ui.label(
                egui::RichText::new(format!(
                    "Loaded game profile: {} ({}) — {}",
                    profile.family.title,
                    profile.port.port,
                    profile.dir.display(),
                ))
                .small()
                .color(egui::Color32::DARK_GRAY),
            );
        });
    }
}
