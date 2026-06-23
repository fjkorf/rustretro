use bevy_egui::egui;

/// Help panel for the debug window.
pub struct HelpPanel;

impl HelpPanel {
    pub fn new() -> Self {
        HelpPanel
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.heading("RustRetro Debugger");
            ui.label("A debugging-first libretro frontend for reverse-engineering retro games, focused on fighting games.");
            ui.separator();

            ui.heading("Keybindings");
            ui.monospace("F12          Toggle debug overlay");
            ui.monospace("Space        Pause / unpause emulation");
            ui.monospace("B            Capture bookmark");
            ui.separator();

            ui.heading("Panels");
            ui.label("🖼 Frame      Live framebuffer, pixel inspector, zoom");
            ui.label("📋 Hex        Hex+ASCII dump of any memory region");
            ui.label("🧩 Tiles      8×8 VRAM tile browser");
            ui.label("🕹 Input      Button state + 120-frame input history");
            ui.label("🔧 CPU        M68K & Z80 registers; delta highlights");
            ui.label("📜 Disasm     Capstone M68K; breakpoints; run-to");
            ui.label("🔊 Audio      Volume, mute, sample rate display");
            ui.label("📜 Log        Scrollable event log with filter");
            ui.label("⏸ Triggers   Frame-count and pixel-value pauses");
            ui.label("🗺 Regions    Bookmarks, PC heatmap, code regions");
            ui.separator();

            ui.heading("Tutorials");
            ui.label("Task-oriented walkthroughs live in docs/tutorials/ (one per feature).");
            ui.label("Start with getting-started.md, then ram-search.md (find a health bar).");
            ui.label("Authored as litui pages — they will mount here as a Help → Tutorials screen once litui is integrated.");
            ui.separator();

            ui.heading("About");
            ui.label("RustRetro loads libretro cores (Genesis, CPS-2, NES) and provides first-class debugging facilities.");
            ui.label("Built with Bevy (rendering), egui (UI), and Capstone (disassembly).");
        });
    }
}
