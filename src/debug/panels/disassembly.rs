use bevy_egui::egui;
use capstone::prelude::*;
use crate::debug::{CodeRegion, DebugState};

pub struct Disassembly {
    /// Pending "label range" form: (start_addr, end_addr_input, label_text, color_rgb)
    pending_label: Option<(u32, String, String, [f32; 3])>,
}

impl Disassembly {
    pub fn new() -> Self {
        Disassembly { pending_label: None }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, debug_state: &mut DebugState) {
        ui.vertical(|ui| {
            ui.heading("📜 Disassembly");
            ui.separator();

            // Status banner
            if let Some(bp_addr) = debug_state.hit_breakpoint {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("🔴 BREAKPOINT HIT at ${:06X}", bp_addr))
                        .color(egui::Color32::RED).strong());
                    if ui.small_button("Dismiss").clicked() {
                        debug_state.hit_breakpoint = None;
                    }
                });
            } else if debug_state.paused {
                ui.label(egui::RichText::new("⏸ PAUSED").color(egui::Color32::YELLOW).strong());
            }

            // PC info + controls
            ui.horizontal(|ui| {
                ui.label("M68K PC:");
                ui.monospace(format!("${:06X}", debug_state.m68k_pc));
                ui.separator();
                if ui.add_enabled(debug_state.paused, egui::Button::new("▶ Step")).clicked() {
                    debug_state.step_one = true;
                }
                let pause_label = if debug_state.paused { "▶ Resume" } else { "⏸ Pause" };
                if ui.button(pause_label).clicked() {
                    debug_state.paused = !debug_state.paused;
                    debug_state.hit_breakpoint = None;
                }
                if !debug_state.breakpoints.is_empty() && ui.small_button("Clear BPs").clicked() {
                    debug_state.breakpoints.clear();
                }
            });

            if debug_state.z80_pc > 0 {
                ui.horizontal(|ui| {
                    ui.label("Z80 PC:");
                    ui.monospace(format!("${:04X}", debug_state.z80_pc));
                });
            }

            ui.separator();

            // Prefer pending_focus over PC when the navigator has a target this frame.
            // Do NOT clear pending_focus here — the dispatcher owns that.
            let focus_addr: u32 = debug_state.nav.pending_focus.unwrap_or(debug_state.m68k_pc);

            match Self::decode_instructions_at(debug_state, focus_addr) {
                Ok(insns) => {
                    let pc = debug_state.m68k_pc;
                    let breakpoints = debug_state.breakpoints.clone();
                    let code_regions = debug_state.code_regions.clone();
                    let mut toggle_bp: Option<u32> = None;
                    let mut run_to: Option<u32> = None;
                    let mut label_addr: Option<u32> = None;

                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        for (addr, text) in &insns {
                            // Region header — show banner when this address starts a labeled region
                            for region in &code_regions {
                                if *addr >= region.addr_start && *addr <= region.addr_end {
                                    if *addr == region.addr_start {
                                        let c = region.color;
                                        let color = egui::Color32::from_rgb(c[0], c[1], c[2]);
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(format!("▼ {}", region.label))
                                                .strong().color(color).small());
                                        });
                                    }
                                }
                            }

                            let is_current = *addr == pc;
                            let has_bp = breakpoints.contains(addr);

                            // Determine tint from any enclosing region
                            let region_color = code_regions.iter()
                                .find(|r| *addr >= r.addr_start && *addr <= r.addr_end)
                                .map(|r| egui::Color32::from_rgba_unmultiplied(r.color[0], r.color[1], r.color[2], 40));

                            ui.horizontal(|ui| {
                                let dot = if has_bp { "🔴" } else { "⚫" };
                                let dot_resp = ui.small_button(dot)
                                    .on_hover_text(if has_bp { "Clear breakpoint" } else { "Set breakpoint" });
                                if dot_resp.clicked() { toggle_bp = Some(*addr); }

                                let color = if is_current {
                                    egui::Color32::from_rgb(100, 220, 100)
                                } else if has_bp {
                                    egui::Color32::from_rgb(255, 100, 100)
                                } else if let Some(rc) = region_color {
                                    // blend region tint with light gray
                                    egui::Color32::from_rgb(
                                        180u8.saturating_add(rc.r() / 4),
                                        180u8.saturating_add(rc.g() / 4),
                                        180u8.saturating_add(rc.b() / 4),
                                    )
                                } else {
                                    egui::Color32::LIGHT_GRAY
                                };
                                let resp = ui.label(egui::RichText::new(text).monospace().color(color));
                                if resp.secondary_clicked() {
                                    // Right-click: show context menu
                                    run_to = Some(*addr);
                                }
                                resp.context_menu(|ui| {
                                    if ui.button("▶ Run to here").clicked() {
                                        run_to = Some(*addr);
                                        ui.close_menu();
                                    }
                                    if ui.button("🏷 Label range starting here…").clicked() {
                                        label_addr = Some(*addr);
                                        ui.close_menu();
                                    }
                                });
                            });
                        }
                    });

                    if let Some(addr) = toggle_bp {
                        if debug_state.breakpoints.contains(&addr) {
                            debug_state.breakpoints.retain(|&a| a != addr);
                        } else if debug_state.breakpoints.len() < 8 {
                            debug_state.breakpoints.push(addr);
                        }
                    }
                    if let Some(addr) = run_to {
                        debug_state.run_to_addr = Some(addr);
                        debug_state.paused = false;
                    }
                    if let Some(addr) = label_addr {
                        self.pending_label = Some((
                            addr,
                            format!("{:06X}", addr.saturating_add(31)),
                            String::new(),
                            [0.0, 0.8, 0.4],
                        ));
                    }

                    if !debug_state.breakpoints.is_empty() {
                        ui.separator();
                        ui.horizontal_wrapped(|ui| {
                            ui.label("BPs:");
                            for bp in &debug_state.breakpoints {
                                ui.monospace(egui::RichText::new(format!("${:06X} ", bp))
                                    .color(egui::Color32::from_rgb(255, 120, 120)));
                            }
                        });
                    }
                }
                Err(err) => {
                    ui.label(egui::RichText::new(format!("⚠️ {}", err)).color(egui::Color32::YELLOW));
                }
            }

            // ── Inline label-range form ──────────────────────────────────────
            // Decisions are collected into locals so we can clear `self.pending_label`
            // after the mutable borrow from the form bindings has ended.
            let mut new_region: Option<CodeRegion> = None;
            let mut close_form = false;
            if let Some((start, end_str, label_text, color)) = self.pending_label.as_mut() {
                let start = *start;
                ui.separator();
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(egui::RichText::new("🏷 Label Address Range").strong());
                    ui.horizontal(|ui| {
                        ui.label(format!("Start: ${:06X}", start));
                        ui.label("  End:");
                        ui.add(egui::TextEdit::singleline(end_str).desired_width(80.0).hint_text("hex addr"));
                        ui.label("  Label:");
                        ui.add(egui::TextEdit::singleline(label_text).desired_width(150.0).hint_text("e.g. game_loop"));
                        ui.label("Color:");
                        ui.color_edit_button_rgb(color);
                    });
                    ui.horizontal(|ui| {
                        let can_add = !label_text.is_empty();
                        if ui.add_enabled(can_add, egui::Button::new("✅ Add Region")).clicked() {
                            let end_addr = u32::from_str_radix(end_str.trim_start_matches("0x"), 16)
                                .unwrap_or(start.saturating_add(31));
                            let c = [
                                (color[0] * 255.0) as u8,
                                (color[1] * 255.0) as u8,
                                (color[2] * 255.0) as u8,
                            ];
                            new_region = Some(CodeRegion {
                                label: label_text.clone(),
                                addr_start: start,
                                addr_end: end_addr,
                                color: c,
                                notes: String::new(),
                            });
                            close_form = true;
                        }
                        if ui.button("❌ Cancel").clicked() {
                            close_form = true;
                        }
                    });
                });
            }
            if let Some(region) = new_region {
                debug_state.code_regions.push(region);
            }
            if close_form {
                self.pending_label = None;
            }

            ui.separator();
            ui.label(egui::RichText::new("Click ⚫ to set BP  ·  Right-click → Run to / Label range")
                .small().color(egui::Color32::DARK_GRAY));
        });
    }

    /// Decode instructions starting from `focus_addr` (which may differ from PC when the
    /// navigation cursor has been set by another panel via `goto`).
    fn decode_instructions_at(debug_state: &DebugState, focus_addr: u32) -> Result<Vec<(u32, String)>, String> {
        let pc = debug_state.m68k_pc;
        // If cached bytes cover the focus address, use them.
        let bytes: &[u8] = if !debug_state.m68k_code_bytes.is_empty()
            && debug_state.m68k_code_start == focus_addr
        {
            &debug_state.m68k_code_bytes
        } else if !debug_state.memory_regions.is_empty() {
            let region = debug_state
                .memory_regions
                .iter()
                .find(|r| focus_addr as usize >= r.addr_start && focus_addr as usize <= r.addr_end)
                .ok_or_else(|| format!("Address ${:06X} outside all memory regions", focus_addr))?;
            let host_ptr = region
                .host_ptr_for_addr(focus_addr as usize)
                .ok_or_else(|| "Cannot translate focus address".to_string())?;
            return Self::decode_bytes_at(
                unsafe { std::slice::from_raw_parts(host_ptr as *const u8, 256) },
                focus_addr,
                pc,
            );
        } else {
            return Err("No code bytes — core does not expose memory via SekFetchByte or SET_MEMORY_MAPS".to_string());
        };
        Self::decode_bytes_at(bytes, focus_addr, pc)
    }

    /// Legacy entry point retained for any callers that relied on PC-only decoding.
    #[allow(dead_code)]
    fn decode_instructions(debug_state: &DebugState) -> Result<Vec<(u32, String)>, String> {
        Self::decode_instructions_at(debug_state, debug_state.m68k_pc)
    }

    /// Decode bytes starting at `start_addr`; mark `real_pc` with the "→" arrow.
    /// `start_addr` may equal `real_pc` (normal case) or differ when the
    /// navigation cursor has jumped elsewhere.
    fn decode_bytes_at(bytes: &[u8], start_addr: u32, real_pc: u32) -> Result<Vec<(u32, String)>, String> {
        let cs = Capstone::new()
            .m68k()
            .mode(capstone::arch::m68k::ArchMode::M68k020)
            .build()
            .map_err(|e| format!("Capstone error: {:?}", e))?;

        let insns = cs
            .disasm_all(bytes, start_addr as u64)
            .map_err(|e| format!("Disassembly error: {:?}", e))?;

        let mut result = Vec::new();

        for (shown, insn) in insns.iter().enumerate() {
            if shown >= 14 { break; }
            let addr = insn.address() as u32;
            let mnem = insn.mnemonic().unwrap_or("??");
            let ops  = insn.op_str().unwrap_or("");
            let marker = if addr == real_pc { "→ " } else { "  " };
            result.push((addr, format!("{}{:06X}:  {:<10} {}", marker, addr, mnem, ops)));
        }

        if result.is_empty() {
            return Err(format!("No instructions decoded at ${:06X}", start_addr));
        }
        Ok(result)
    }

    /// Legacy byte-decoding entry point (PC == start address).
    #[allow(dead_code)]
    fn decode_bytes(bytes: &[u8], pc: u32) -> Result<Vec<(u32, String)>, String> {
        Self::decode_bytes_at(bytes, pc, pc)
    }
}
