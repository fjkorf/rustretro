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

            ui.horizontal(|ui| {
                ui.label("M68K PC:");
                ui.monospace(format!("0x{:06X}", debug_state.m68k_pc));
            });

            if debug_state.z80_pc > 0 {
                ui.horizontal(|ui| {
                    ui.label("Z80 PC:");
                    ui.monospace(format!("0x{:04X}", debug_state.z80_pc));
                });
            }

            ui.separator();

            match Self::disassemble_from_pc(debug_state) {
                Ok(disasm_text) => {
                    ui.label("Current instruction and context:");
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .show(ui, |ui| {
                            ui.monospace(disasm_text);
                        });
                }
                Err(err) => {
                    ui.label(egui::RichText::new(format!("⚠️ {}", err)).color(egui::Color32::YELLOW));
                }
            }

            ui.separator();
            ui.label("(Shows ±10 instructions around current PC)");
        });
    }

    fn disassemble_from_pc(debug_state: &DebugState) -> Result<String, String> {
        let pc = debug_state.m68k_pc;

        // Primary path: use pre-fetched code bytes from SekFetchByte (fbalpha2012)
        let bytes: &[u8] = if !debug_state.m68k_code_bytes.is_empty()
            && debug_state.m68k_code_start == pc
        {
            &debug_state.m68k_code_bytes
        } else if !debug_state.memory_regions.is_empty() {
            // Fallback: memory_regions from SET_MEMORY_MAPS (other cores)
            let region = debug_state
                .memory_regions
                .iter()
                .find(|r| pc as usize >= r.addr_start && pc as usize <= r.addr_end)
                .ok_or_else(|| format!("PC 0x{:06X} outside all memory regions", pc))?;
            let host_ptr = region
                .host_ptr_for_addr(pc as usize)
                .ok_or_else(|| "Cannot translate PC address".to_string())?;
            return Self::disassemble_bytes(
                unsafe { std::slice::from_raw_parts(host_ptr as *const u8, 256) },
                pc,
            );
        } else {
            return Err(
                "No code bytes available — core does not expose memory via SekFetchByte or SET_MEMORY_MAPS".to_string()
            );
        };

        Self::disassemble_bytes(bytes, pc)
    }

    fn disassemble_bytes(bytes: &[u8], pc: u32) -> Result<String, String> {
        let cs = Capstone::new()
            .m68k()
            .mode(capstone::arch::m68k::ArchMode::M68k020)
            .build()
            .map_err(|e| format!("Capstone error: {:?}", e))?;

        let insns = cs
            .disasm_all(bytes, pc as u64)
            .map_err(|e| format!("Disassembly error: {:?}", e))?;

        let mut output = String::new();
        let mut found_pc = false;
        let mut shown_after = 0;

        for insn in insns.iter() {
            let insn_addr = insn.address() as u32;
            let is_current = insn_addr == pc;

            if is_current {
                found_pc = true;
            }

            if found_pc {
                if shown_after >= 12 {
                    break;
                }
                shown_after += 1;
            }

            let mnem = insn.mnemonic().unwrap_or("??");
            let ops = insn.op_str().unwrap_or("");
            let marker = if is_current { "→ " } else { "  " };
            output.push_str(&format!("{}{:06X}: {:<12} {}\n", marker, insn_addr, mnem, ops));
        }

        if !found_pc {
            return Err(format!("PC 0x{:06X} not found in disassembled output", pc));
        }

        Ok(output)
    }
}

