use bevy_egui::egui;
use std::sync::{Arc, Mutex};

use crate::audio::AudioOutput;
use crate::debug::DebugState;
use crate::debug::dock::{self, Panels, Tab};
use egui_dock::DockState;

pub struct DebugApp {
    state: Arc<Mutex<DebugState>>,
    audio: Option<Arc<Mutex<AudioOutput>>>,
    /// Hex address buffer backing the toolbar "Go to:" field.
    goto_input: String,
    /// Per-panel UI state, owned by the dock workspace.
    panels: Panels,
    /// The draggable/splittable dock layout. Loaded from disk on construction
    /// (see `dock::LAYOUT_PATH`), falling back to `dock::default_layout()`.
    dock_state: DockState<Tab>,
}

impl DebugApp {
    pub fn new(state: Arc<Mutex<DebugState>>) -> Self {
        DebugApp {
            state,
            audio: None,
            goto_input: String::new(),
            panels: Panels::new(),
            // Try to restore the saved layout; fall back to the default layout.
            dock_state: dock::load_layout(),
        }
    }

    pub fn set_audio(&mut self, audio: Arc<Mutex<AudioOutput>>) {
        self.audio = Some(audio);
    }

    /// Render the debug overlay into the given egui context.
    /// Call this from a Bevy system that holds `EguiContexts`.
    pub fn show(&mut self, ctx: &egui::Context) {
        let state_snapshot = self.state.lock().ok().map(|s| (
            s.frame_count, s.video_frames, s.video_real,
            s.fps, s.fb_width, s.fb_height, s.fb_fmt, s.paused,
        ));

        // --- Persistent global toolbar (always visible above the dock) ---
        egui::TopBottomPanel::top("debug_toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if let Ok(mut ds) = self.state.lock() {
                    // Back / Forward history navigation.
                    let can_back = ds.can_go_back();
                    let can_fwd = ds.can_go_forward();
                    if ui.add_enabled(can_back, egui::Button::new("◀ Back")).clicked() {
                        ds.nav_back();
                    }
                    if ui.add_enabled(can_fwd, egui::Button::new("▶ Fwd")).clicked() {
                        ds.nav_forward();
                    }

                    ui.separator();

                    // Run / Pause toggle.
                    let (run_lbl, run_col) = if ds.paused {
                        ("▶ Run", egui::Color32::GREEN)
                    } else {
                        ("⏸ Pause", egui::Color32::YELLOW)
                    };
                    if ui.button(egui::RichText::new(run_lbl).color(run_col)).clicked() {
                        ds.paused = !ds.paused;
                    }
                    if ui.button("⏭ Step").clicked() {
                        ds.step_one = true;
                    }
                    // Frame-step: advances one instruction for now (same as Step until a
                    // true run-to-next-frame mechanism exists).
                    if ui.button("⏯ Step Frame").clicked() {
                        ds.step_one = true;
                    }

                    ui.separator();

                    // Go-to-address (hex). Enter in the field or the Go button both jump.
                    ui.label("Go to:");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.goto_input)
                            .desired_width(80.0)
                            .font(egui::TextStyle::Monospace)
                            .hint_text("hex"),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Go").clicked() || enter {
                        let txt = self.goto_input.trim().trim_start_matches('$').trim_start_matches("0x");
                        if let Ok(addr) = u32::from_str_radix(txt, 16) {
                            ds.goto(addr);
                        }
                    }

                    ui.separator();

                    // PC + current-location readout.
                    let pc = ds.m68k_pc;
                    let cur = ds.nav.current_address;
                    ui.label(egui::RichText::new(format!("PC: ${pc:06X}")).monospace());
                    if let Some(addr) = cur {
                        ui.label(egui::RichText::new(format!("@ ${addr:06X}")).monospace());
                    }
                } else {
                    ui.label("Error: Could not acquire debug state lock");
                }

                ui.separator();

                // --- Layout persistence controls ---
                // Save writes the current dock layout to `dock::LAYOUT_PATH`;
                // Reset restores the built-in default layout.
                if ui.button("💾 Save layout").clicked() {
                    dock::save_layout(&self.dock_state);
                }
                if ui.button("⟲ Reset layout").clicked() {
                    self.dock_state = dock::default_layout();
                }

                // --- Status readout ---
                if let Some((fc, vf, vr, fps, w, h, fmt, paused)) = state_snapshot {
                    ui.separator();
                    ui.label(egui::RichText::new(if paused { "⏸ PAUSED" } else { "▶ running" })
                        .color(if paused { egui::Color32::YELLOW } else { egui::Color32::GREEN }));
                    ui.separator();
                    ui.label(format!("run:{fc} vid:{vf} real:{vr} | {w}×{h} fmt={fmt} @ {fps:.1}fps"));
                }
            });
        });

        // The dock workspace lives in the CentralPanel, below the toolbar. The
        // address-aware panels (Disasm/Hex/Regions/Watch/RamSearch) read
        // `nav.pending_focus` while rendering here.
        egui::CentralPanel::default().show(ctx, |ui| {
            dock::show_dock(ui, &mut self.dock_state, &mut self.panels, &self.state, &self.audio);
        });

        // pending_focus is a one-frame pulse: address-aware panels read it during this
        // frame's CentralPanel render above, then we clear it here (after that closure
        // returns) so it fires exactly once. Must run AFTER the central panel.
        if let Ok(mut ds) = self.state.lock() {
            ds.nav.pending_focus = None;
        }
    }
}
