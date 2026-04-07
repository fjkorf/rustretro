use eframe::egui;
use std::sync::{Arc, Mutex};
use crate::debug::DebugState;

pub struct HexDump {
    goto_addr: String,
    scroll_to: Option<usize>,
}

impl HexDump {
    pub fn new() -> Self {
        HexDump { goto_addr: String::new(), scroll_to: None }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &Arc<Mutex<DebugState>>) {
        let (buf, width, height, pitch, fmt) = {
            let s = state.lock().unwrap();
            (s.framebuffer.clone(), s.fb_width, s.fb_height, s.fb_pitch, s.fb_fmt)
        };

        ui.horizontal(|ui| {
            ui.label(format!("{}×{} pitch={} fmt={} — {} bytes",
                width, height, pitch, fmt, buf.len()));
            ui.separator();
            ui.label("Go to:");
            let resp = ui.add(egui::TextEdit::singleline(&mut self.goto_addr).desired_width(80.0));
            if (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                || ui.button("→").clicked()
            {
                if let Ok(addr) = usize::from_str_radix(self.goto_addr.trim_start_matches("0x"), 16) {
                    self.scroll_to = Some(addr / 16);
                }
            }
        });
        ui.separator();

        if buf.is_empty() {
            ui.label("No framebuffer data yet.");
            return;
        }

        let bytes_per_row: usize = 16;
        let num_rows = buf.len().div_ceil(bytes_per_row);
        let row_height = 16.0;

        egui::ScrollArea::vertical().auto_shrink(false).show_rows(
            ui, row_height, num_rows,
            |ui, row_range| {
                egui::Grid::new("hex_grid")
                    .num_columns(3)
                    .spacing([8.0, 2.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for row in row_range {
                            let start = row * bytes_per_row;
                            let end = (start + bytes_per_row).min(buf.len());
                            let slice = &buf[start..end];

                            // Address
                            ui.label(egui::RichText::new(format!("{:06X}", start))
                                .monospace()
                                .color(egui::Color32::from_rgb(150, 150, 200)));

                            // Hex bytes
                            let hex: String = slice.iter().enumerate().map(|(i, b)| {
                                if i > 0 && i % 8 == 0 { format!(" {:02X}", b) }
                                else { format!(" {:02X}", b) }
                            }).collect();
                            let has_nonzero = slice.iter().any(|&b| b != 0);
                            let color = if has_nonzero {
                                egui::Color32::WHITE
                            } else {
                                egui::Color32::DARK_GRAY
                            };
                            ui.label(egui::RichText::new(hex).monospace().color(color));

                            // ASCII
                            let ascii: String = slice.iter().map(|&b| {
                                if (0x20..0x7F).contains(&b) { b as char } else { '.' }
                            }).collect();
                            ui.label(egui::RichText::new(ascii)
                                .monospace()
                                .color(egui::Color32::from_rgb(150, 200, 150)));

                            ui.end_row();
                        }
                    });
            },
        );
    }
}
