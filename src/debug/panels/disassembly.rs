use bevy_egui::egui;
use capstone::prelude::*;
use crate::debug::DebugState;

pub struct Disassembly;

impl Disassembly {
    pub fn show(ui: &mut egui::Ui, debug_state: &mut DebugState) {
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

            match Self::decode_instructions(debug_state) {
                Ok(insns) => {
                    let pc = debug_state.m68k_pc;
                    let breakpoints = debug_state.breakpoints.clone();
                    let mut toggle_bp: Option<u32> = None;
                    let mut run_to: Option<u32> = None;

                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for (addr, text) in &insns {
                            let is_current = *addr == pc;
                            let has_bp = breakpoints.contains(addr);

                            ui.horizontal(|ui| {
                                let dot = if has_bp { "🔴" } else { "⚫" };
                                let dot_resp = ui.small_button(dot)
                                    .on_hover_text(if has_bp { "Clear breakpoint" } else { "Set breakpoint" });
                                if dot_resp.clicked() { toggle_bp = Some(*addr); }

                                let color = if is_current {
                                    egui::Color32::from_rgb(100, 220, 100)
                                } else if has_bp {
                                    egui::Color32::from_rgb(255, 100, 100)
                                } else {
                                    egui::Color32::LIGHT_GRAY
                                };
                                let resp = ui.label(egui::RichText::new(text).monospace().color(color));
                                if resp.secondary_clicked() { run_to = Some(*addr); }
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

            ui.separator();
            ui.label(egui::RichText::new("Click ⚫ to set BP  ·  Right-click instruction → Run to address")
                .small().color(egui::Color32::DARK_GRAY));
        });
    }

    fn decode_instructions(debug_state: &DebugState) -> Result<Vec<(u32, String)>, String> {
        let pc = debug_state.m68k_pc;
        let bytes: &[u8] = if !debug_state.m68k_code_bytes.is_empty()
            && debug_state.m68k_code_start == pc
        {
            &debug_state.m68k_code_bytes
        } else if !debug_state.memory_regions.is_empty() {
            let region = debug_state
                .memory_regions
                .iter()
                .find(|r| pc as usize >= r.addr_start && pc as usize <= r.addr_end)
                .ok_or_else(|| format!("PC ${:06X} outside all memory regions", pc))?;
            let host_ptr = region
                .host_ptr_for_addr(pc as usize)
                .ok_or_else(|| "Cannot translate PC address".to_string())?;
            return Self::decode_bytes(
                unsafe { std::slice::from_raw_parts(host_ptr as *const u8, 256) }, pc);
        } else {
            return Err("No code bytes — core does not expose memory via SekFetchByte or SET_MEMORY_MAPS".to_string());
        };
        Self::decode_bytes(bytes, pc)
    }

    fn decode_bytes(bytes: &[u8], pc: u32) -> Result<Vec<(u32, String)>, String> {
        let cs = Capstone::new()
            .m68k()
            .mode(capstone::arch::m68k::ArchMode::M68k020)
            .build()
            .map_err(|e| format!("Capstone error: {:?}", e))?;

        let insns = cs
            .disasm_all(bytes, pc as u64)
            .map_err(|e| format!("Disassembly error: {:?}", e))?;

        let mut result = Vec::new();
        let mut found_pc = false;
        let mut shown_after = 0;

        for insn in insns.iter() {
            let addr = insn.address() as u32;
            if addr == pc { found_pc = true; }
            if found_pc {
                if shown_after >= 14 { break; }
                shown_after += 1;
            }
            let mnem = insn.mnemonic().unwrap_or("??");
            let ops  = insn.op_str().unwrap_or("");
            let marker = if addr == pc { "→ " } else { "  " };
            result.push((addr, format!("{}{:06X}:  {:<10} {}", marker, addr, mnem, ops)));
        }

        if !found_pc {
            return Err(format!("PC ${:06X} not found in decoded output", pc));
        }
        Ok(result)
    }
}
