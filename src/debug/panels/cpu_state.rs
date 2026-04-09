use crate::debug::DebugState;
use bevy_egui::egui;
use std::sync::{Arc, Mutex};

pub struct CpuState;

const CHANGED: egui::Color32 = egui::Color32::from_rgb(255, 220, 60);
const UNCHANGED: egui::Color32 = egui::Color32::LIGHT_GRAY;

impl CpuState {
    pub fn new() -> Self { CpuState }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &Arc<Mutex<DebugState>>) {
        let s = state.lock().unwrap();

        // M68K header with PC delta
        ui.horizontal(|ui| {
            ui.heading("🟦 M68000");
            let pc_color = if s.m68k_pc != s.prev_m68k_pc { CHANGED } else { UNCHANGED };
            ui.label(egui::RichText::new(format!("PC: ${:06X}", s.m68k_pc))
                .monospace().color(pc_color));
            ui.monospace(format!("  SR: ${:04X}  [f:{}]", s.m68k_sr, s.frame_count));
        });

        // Data registers with delta highlight
        ui.label(egui::RichText::new("Data Registers (D0-D7)").strong());
        ui.horizontal(|ui| {
            for i in 0..4 {
                ui.vertical(|ui| {
                    for offset in [0, 4] {
                        let idx = i + offset;
                        let changed = s.m68k_d_regs[idx] != s.prev_m68k_d_regs[idx];
                        let color = if changed { CHANGED } else { UNCHANGED };
                        ui.label(egui::RichText::new(format!("D{}: ${:08X}", idx, s.m68k_d_regs[idx]))
                            .monospace().color(color));
                    }
                });
            }
        });

        // Address registers with delta highlight
        ui.label(egui::RichText::new("Address Registers (A0-A7)").strong());
        ui.horizontal(|ui| {
            for i in 0..4 {
                ui.vertical(|ui| {
                    for offset in [0, 4] {
                        let idx = i + offset;
                        let changed = s.m68k_a_regs[idx] != s.prev_m68k_a_regs[idx];
                        let color = if changed { CHANGED } else { UNCHANGED };
                        ui.label(egui::RichText::new(format!("A{}: ${:08X}", idx, s.m68k_a_regs[idx]))
                            .monospace().color(color));
                    }
                });
            }
        });

        // Status register breakdown
        ui.separator();
        ui.label(egui::RichText::new("Status Register Flags").strong());
        let sr = s.m68k_sr;
        ui.horizontal(|ui| {
            let t = (sr >> 15) & 1;
            let sup = (sr >> 13) & 1;
            let m = (sr >> 12) & 1;
            let i_level = (sr >> 8) & 0x7;
            ui.monospace(format!("T:{}  S:{}  M:{}  I:{}", t, sup, m, i_level));
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
            ui.monospace(format!("PC: ${:04X}", s.z80_pc));
        });
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("Register Pairs").strong());
                ui.monospace(format!("BC: ${:04X}", s.z80_bc));
                ui.monospace(format!("DE: ${:04X}", s.z80_de));
                ui.monospace(format!("HL: ${:04X}", s.z80_hl));
            });
            ui.vertical(|ui| {
                ui.label("");
                ui.small("(Audio coprocessor)");
            });
        });

        ui.separator();
        ui.small("💡 Changed registers highlighted in yellow  ·  CPU state captured every frame");
    }
}

