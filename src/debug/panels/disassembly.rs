use bevy_egui::egui;
use capstone::prelude::*;
use crate::debug::DebugState;

/// Disassembly panel showing live CPU code at current PC.
pub struct Disassembly;

impl Disassembly {
    pub fn show(ui: &mut egui::Ui, debug_state: &DebugState) {
        ui.vertical(|ui| {
            ui.heading("📜 Disassembly");
            ui.separator();

            // Display current M68K PC
            ui.horizontal(|ui| {
                ui.label("M68K PC:");
                ui.monospace(format!("0x{:06X}", debug_state.m68k_pc));
            });

            // Display Z80 PC if available
            if debug_state.z80_pc > 0 {
                ui.horizontal(|ui| {
                    ui.label("Z80 PC:");
                    ui.monospace(format!("0x{:04X}", debug_state.z80_pc));
                });
            }

            ui.separator();

            // Try to disassemble from current PC
            match Self::disassemble_from_pc(debug_state) {
                Ok(disasm_text) => {
                    ui.label("Current instruction and context:");
                    ui.separator();
                    
                    // Display disassembly in a monospace font within a scrollable area
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .show(ui, |ui| {
                            ui.monospace(disasm_text);
                        });
                }
                Err(err) => {
                    ui.label(egui::RichText::new(format!("⚠️ {}", err)).color(egui::Color32::YELLOW));
                    
                    // Show available memory regions for debugging
                    ui.separator();
                    ui.label(egui::RichText::new("Available Memory Regions:").color(egui::Color32::LIGHT_GRAY));
                    if debug_state.memory_regions.is_empty() {
                        ui.label(egui::RichText::new("  (No memory regions set)").italics().color(egui::Color32::DARK_GRAY));
                    } else {
                        for region in &debug_state.memory_regions {
                            ui.label(format!(
                                "  {}: 0x{:06X}—0x{:06X} ({})",
                                region.name,
                                region.addr_start,
                                region.addr_end,
                                region.region_type()
                            ));
                        }
                    }
                }
            }

            ui.separator();
            ui.label("(This shows ±10 instructions around current PC)");
        });
    }

    /// Disassemble 10 instructions before and after current PC.
    fn disassemble_from_pc(debug_state: &DebugState) -> Result<String, String> {
        let pc = debug_state.m68k_pc as usize;

        // Find memory region containing PC
        let region = debug_state
            .memory_regions
            .iter()
            .find(|r| pc >= r.addr_start && pc <= r.addr_end)
            .ok_or_else(|| "PC outside all memory regions".to_string())?;

        // Get host pointer for PC
        let host_ptr = region
            .host_ptr_for_addr(pc)
            .ok_or_else(|| "Cannot translate PC address".to_string())?;

        // Read a buffer around PC (±10 instructions ≈ up to ~80 bytes)
        let read_size = 256; // Generous buffer
        let bytes = unsafe {
            std::slice::from_raw_parts(host_ptr as *const u8, read_size)
        };

        // Create Capstone disassembler
        let cs = Capstone::new()
            .m68k()
            .mode(capstone::arch::m68k::ArchMode::M68k020)
            .build()
            .map_err(|e| format!("Capstone error: {:?}", e))?;

        // Disassemble from current PC
        let insns = cs
            .disasm_all(bytes, pc as u64)
            .map_err(|e| format!("Disassembly error: {:?}", e))?;

        // Find current instruction and build context
        let mut output = String::new();
        let mut found_pc = false;
        let mut insn_count = 0;

        for insn in insns.iter() {
            let insn_addr = insn.address() as usize;
            let is_current = insn_addr == pc;

            if is_current {
                found_pc = true;
            }

            // Show ±10 instructions around PC
            if found_pc && insn_count >= 10 {
                break;
            }
            if !found_pc && insn_count >= 10 {
                continue;
            }

            let mnem = insn.mnemonic().unwrap_or("??");
            let ops = insn.op_str().unwrap_or("");
            let marker = if is_current { "→ " } else { "  " };

            output.push_str(&format!(
                "{}{:06X}: {:<12} {}\n",
                marker, insn_addr, mnem, ops
            ));

            insn_count += 1;
        }

        if !found_pc {
            return Err("PC not found in disassembly".to_string());
        }

        if output.is_empty() {
            return Err("No instructions disassembled".to_string());
        }

        Ok(output)
    }
}
