//! Script panel — in-UI Lua script loader and status display.
//!
//! Rendered as a separate small `egui::Window` driven by a dedicated Bevy system
//! (`show_script_panel` in main.rs) that holds both `EguiContexts` and
//! `NonSendMut<LuaRes>`. This avoids threading `&mut LuaEngine` through `DebugApp`
//! (which is a normal `Resource` and therefore `Send`; `LuaEngine` is `!Send`).
//!
//! ## Wiring — see REPORT for exact snippets
//! The integrator must:
//!   1. Add `pub mod script_panel;` to `src/debug/panels/mod.rs`.
//!   2. Add `ScriptPanel` as a `Resource` in `main.rs` and insert it with
//!      `App::insert_resource(ScriptPanel::new())`.
//!   3. Add a `show_script_panel` system (snippet below) to the `Update` set,
//!      **after** `show_debug` in the existing chain so egui contexts are already
//!      open for this frame.

use bevy_egui::egui;
use std::sync::{Arc, Mutex};

use crate::debug::DebugState;
use crate::lua_engine::LuaEngine;

/// In-UI Lua script panel.  Lives as a Bevy `Resource`.
#[derive(bevy::prelude::Resource)]
pub struct ScriptPanel {
    /// Text field: the path (or inline source) the user wants to load.
    pub path_input: String,
    /// Last result of a load/reload, shown below the buttons.
    pub status: String,
    /// Whether the panel window is open.
    pub open: bool,
}

impl ScriptPanel {
    pub fn new() -> Self {
        ScriptPanel {
            path_input: String::new(),
            status: String::from("No script loaded."),
            open: false,
        }
    }

    /// Render the panel into an egui `Window`.
    ///
    /// Call this from a Bevy system that has access to both `EguiContexts` and
    /// `NonSendMut<LuaRes>` — see the wiring snippet in the module-level docs.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        engine: &mut LuaEngine,
        _state: &Arc<Mutex<DebugState>>,
    ) {
        if !self.open {
            return;
        }

        let mut open = self.open;
        egui::Window::new("Lua Script")
            .open(&mut open)
            .resizable(true)
            .default_width(480.0)
            .show(ctx, |ui| {
                self.render_contents(ui, engine);
            });
        self.open = open;
    }

    fn render_contents(&mut self, ui: &mut egui::Ui, engine: &mut LuaEngine) {
        ui.heading("Lua Script");
        ui.separator();

        // ── Path input + Load / Reload ────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label("Script path:");
            ui.add(
                egui::TextEdit::singleline(&mut self.path_input)
                    .hint_text("path/to/script.lua  or  inline Lua source")
                    .desired_width(280.0)
                    .font(egui::TextStyle::Monospace),
            );
        });

        ui.horizontal(|ui| {
            let can_act = !self.path_input.trim().is_empty();

            if ui
                .add_enabled(can_act, egui::Button::new("Load"))
                .on_hover_text("Load and execute the script (registers onframeend callbacks)")
                .clicked()
            {
                let path = self.path_input.trim().to_string();
                match engine.load_script(&path) {
                    Ok(()) => {
                        let n = engine.callback_count();
                        self.status = format!(
                            "OK: loaded '{}' ({} onframeend callback{})",
                            path,
                            n,
                            if n == 1 { "" } else { "s" }
                        );
                    }
                    Err(e) => {
                        self.status = format!("Error: {e}");
                    }
                }
            }

            if ui
                .add_enabled(can_act, egui::Button::new("Reload"))
                .on_hover_text("Discard the current VM and reload from scratch (hot-reload)")
                .clicked()
            {
                let path = self.path_input.trim().to_string();
                match engine.reload(&path) {
                    Ok(()) => {
                        let n = engine.callback_count();
                        self.status = format!(
                            "OK: reloaded '{}' ({} onframeend callback{})",
                            path,
                            n,
                            if n == 1 { "" } else { "s" }
                        );
                    }
                    Err(e) => {
                        self.status = format!("Reload error: {e}");
                    }
                }
            }

            if ui.button("Clear VM").on_hover_text("Reset the Lua VM and discard all callbacks").clicked() {
                match engine.reload("") {
                    Ok(()) | Err(_) => {}
                }
                self.status = "VM cleared.".to_string();
            }
        });

        ui.separator();

        // ── Status line ───────────────────────────────────────────────────────
        let is_error = self.status.starts_with("Error") || self.status.starts_with("Reload error");
        let status_color = if is_error {
            egui::Color32::from_rgb(255, 100, 100)
        } else if self.status.starts_with("OK") {
            egui::Color32::from_rgb(100, 220, 100)
        } else {
            egui::Color32::LIGHT_GRAY
        };
        ui.label(
            egui::RichText::new(&self.status)
                .monospace()
                .size(11.0)
                .color(status_color),
        );

        // Callback count badge
        let cb_count = engine.callback_count();
        ui.label(
            egui::RichText::new(format!("{cb_count} onframeend callback(s) registered"))
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(180, 180, 255)),
        );

        ui.separator();

        // ── API quick-reference ───────────────────────────────────────────────
        ui.collapsing("API reference", |ui| {
            ui.label(egui::RichText::new("_RUSTRETRO_API = 1  (version sentinel)").monospace().size(10.5));
            ui.separator();

            ui.label(egui::RichText::new("memory.*").strong());
            for line in [
                "memory.read_u8(addr)          -> integer",
                "memory.read_u16_be(addr)       -> integer",
                "memory.read_u32_be(addr)       -> integer",
                "memory.read_s16_be(addr)       -> integer (signed)",
                "memory.read_u16_le(addr)       -> integer",
                "memory.read_u32_le(addr)       -> integer",
            ] {
                ui.label(egui::RichText::new(line).monospace().size(10.5).color(egui::Color32::LIGHT_GRAY));
            }

            ui.add_space(4.0);
            ui.label(egui::RichText::new("gui.*").strong());
            for line in [
                "gui.drawBox(x1,y1,x2,y2, fill, line)   colors: 0xRRGGBBAA",
                "gui.drawLine(x1,y1,x2,y2, color)",
                "gui.drawPixel(x,y, color)",
                "gui.drawText(x,y, str [, color])",
            ] {
                ui.label(egui::RichText::new(line).monospace().size(10.5).color(egui::Color32::LIGHT_GRAY));
            }

            ui.add_space(4.0);
            ui.label(egui::RichText::new("event / console / emu").strong());
            for line in [
                "event.onframeend(function)    register per-frame callback",
                "console.log(str)              write to the debug event log",
                "emu.framecount()   -> integer",
            ] {
                ui.label(egui::RichText::new(line).monospace().size(10.5).color(egui::Color32::LIGHT_GRAY));
            }

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Coordinates are GAME-PIXEL space (e.g. 320x224 for Genesis).\n\
                     Colors: 0xRRGGBBAA  — AA=0xFF is fully opaque.",
                )
                .size(10.0)
                .color(egui::Color32::from_rgb(150, 150, 150)),
            );
        });
    }
}
