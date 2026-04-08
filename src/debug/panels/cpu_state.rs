use crate::debug::DebugState;
use bevy_egui::egui;
use std::sync::{Arc, Mutex};

pub struct CpuState;

impl CpuState {
    pub fn new() -> Self { CpuState }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &Arc<Mutex<DebugState>>) {
        let (m68k_pc, m68k_sr, m68k_d, m68k_a, z80_pc, z80_bc, z80_de, z80_hl, frame_count) = {
            let s = state.lock().unwrap();
            (s.m68k_pc, s.m68k_sr, s.m68k_d_regs, s.m68k_a_regs, 
             s.z80_pc, s.z80_bc, s.z80_de, s.z80_hl, s.frame_count)
        };

        // M68K Section
        ui.horizontal(|ui| {
            ui.heading("🟦 M68000");
            ui.monospace(format!("PC: ${:06X}  SR: ${:04X}  [f:{}]", m68k_pc, m68k_sr, frame_count));
        });

        // Data registers
        ui.label(egui::RichText::new("Data Registers (D0-D7)").strong());
        ui.horizontal(|ui| {
            for i in 0..4 {
                ui.vertical(|ui| {
                    ui.monospace(format!("D{}: ${:08X}", i, m68k_d[i]));
                    ui.monospace(format!("D{}: ${:08X}", i + 4, m68k_d[i + 4]));
                });
            }
        });

        // Address registers
        ui.label(egui::RichText::new("Address Registers (A0-A7)").strong());
        ui.horizontal(|ui| {
            for i in 0..4 {
                ui.vertical(|ui| {
                    ui.monospace(format!("A{}: ${:08X}", i, m68k_a[i]));
                    ui.monospace(format!("A{}: ${:08X}", i + 4, m68k_a[i + 4]));
                });
            }
        });

        // Status register breakdown
        ui.separator();
        ui.label(egui::RichText::new("Status Register Flags").strong());
        let sr = m68k_sr;
        ui.horizontal(|ui| {
            let t = (sr >> 15) & 1;
            let s = (sr >> 13) & 1;
            let m = (sr >> 12) & 1;
            let i_level = (sr >> 8) & 0x7;
            ui.monospace(format!("T:{}  S:{}  M:{}  I:{}", t, s, m, i_level));
        });

        ui.horizontal(|ui| {
            let x = (sr >> 4) & 1;
            let n = (sr >> 3) & 1;
            let z = (sr >> 2) & 1;
            let v = (sr >> 1) & 1;
            let c = sr & 1;
            ui.monospace(format!("X:{}  N:{}  Z:{}  V:{}  C:{}", x, n, z, v, c));
        });

        // Z80 Section
        ui.separator();
        ui.horizontal(|ui| {
            ui.heading("🟩 Z80");
            ui.monospace(format!("PC: ${:04X}", z80_pc));
        });

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("Register Pairs").strong());
                ui.monospace(format!("BC: ${:04X}", z80_bc));
                ui.monospace(format!("DE: ${:04X}", z80_de));
                ui.monospace(format!("HL: ${:04X}", z80_hl));
            });
            ui.vertical(|ui| {
                ui.label("");
                ui.small("(Audio coprocessor)");
            });
        });

        ui.separator();
        ui.small("💡 CPU state captured every frame (if core supports debug API)");
    }
}
