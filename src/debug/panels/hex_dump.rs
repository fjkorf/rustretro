use bevy_egui::egui;
use std::sync::{Arc, Mutex};
use crate::debug::{DebugState, MemoryRegion};

pub struct HexDump {
    goto_addr: String,
    scroll_to: Option<usize>,
    current_region_idx: usize,
}

impl HexDump {
    pub fn new() -> Self {
        HexDump { goto_addr: String::new(), scroll_to: None, current_region_idx: 0 }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &Arc<Mutex<DebugState>>) {
        let (regions, current_fb) = {
            let s = state.lock().unwrap();
            (s.memory_regions.clone(), (s.framebuffer.clone(), s.fb_width, s.fb_height))
        };

        // Clamp region index
        if self.current_region_idx >= regions.len() {
            self.current_region_idx = 0;
        }

        // Top bar: controls and region selection
        ui.horizontal(|ui| {
            ui.label("Memory:");
            if regions.is_empty() {
                ui.label("(no regions available)");
            } else {
                let names: Vec<String> = regions.iter().map(|r| r.name.clone()).collect();
                egui::ComboBox::from_label("")
                    .selected_text(names[self.current_region_idx].clone())
                    .show_index(ui, &mut self.current_region_idx, regions.len(), |i| {
                        egui::WidgetText::from(names[i].clone())
                    });
            }

            ui.separator();
            ui.label("Address:");
            let resp = ui.add(egui::TextEdit::singleline(&mut self.goto_addr)
                .desired_width(100.0)
                .hint_text("0x..."));
            if (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                || ui.button("→").clicked()
            {
                if let Ok(addr) = usize::from_str_radix(
                    self.goto_addr.trim_start_matches("0x").trim(),
                    16,
                ) {
                    self.scroll_to = Some(addr / 16);
                }
            }
        });

        ui.separator();

        // Get data from selected region or framebuffer
        let (region, buf_to_display) = if !regions.is_empty() {
            let region = &regions[self.current_region_idx];
            // Try to read from host memory
            if let Some(buf) = self.read_region(region) {
                (Some(region.clone()), buf)
            } else {
                (None, Vec::new())
            }
        } else {
            (None, current_fb.0)
        };

        if let Some(ref r) = region {
            let (rc, gc, bc) = r.color();
            ui.horizontal(|ui| {
                ui.colored_label(
                    egui::Color32::from_rgb(rc, gc, bc),
                    format!("● {} — {}", r.name, r.region_type()),
                );
                ui.separator();
                ui.label(format!(
                    "0x{:X}–0x{:X} ({} bytes) {}",
                    r.addr_start,
                    r.addr_end,
                    r.size,
                    if r.is_readonly() { "[RO]" } else { "[RW]" }
                ));
            });
        }

        ui.separator();

        if buf_to_display.is_empty() {
            ui.label("No data to display.");
            return;
        }

        // Hex dump grid
        let bytes_per_row = 16;
        let num_rows = buf_to_display.len().div_ceil(bytes_per_row);
        let row_height = 16.0;

        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show_rows(ui, row_height, num_rows, |ui, row_range| {
                egui::Grid::new("hex_grid")
                    .num_columns(3)
                    .spacing([8.0, 2.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for row in row_range {
                            let start = row * bytes_per_row;
                            let end = (start + bytes_per_row).min(buf_to_display.len());
                            let slice = &buf_to_display[start..end];

                            // Calculate actual address
                            let addr = if let Some(ref r) = region {
                                r.addr_start + start
                            } else {
                                start
                            };

                            // Address column
                            ui.label(
                                egui::RichText::new(format!("{:06X}", addr))
                                    .monospace()
                                    .color(egui::Color32::from_rgb(150, 150, 200)),
                            );

                            // Hex bytes column
                            let hex: String = slice
                                .iter()
                                .enumerate()
                                .map(|(i, b)| {
                                    if i > 0 && i % 8 == 0 {
                                        format!(" {:02X}", b)
                                    } else {
                                        format!(" {:02X}", b)
                                    }
                                })
                                .collect();
                            let has_nonzero = slice.iter().any(|&b| b != 0);
                            let color = if has_nonzero {
                                egui::Color32::WHITE
                            } else {
                                egui::Color32::DARK_GRAY
                            };
                            ui.label(egui::RichText::new(hex).monospace().color(color));

                            // ASCII column
                            let ascii: String = slice
                                .iter()
                                .map(|&b| {
                                    if (0x20..0x7F).contains(&b) {
                                        b as char
                                    } else {
                                        '.'
                                    }
                                })
                                .collect();
                            ui.label(
                                egui::RichText::new(ascii)
                                    .monospace()
                                    .color(egui::Color32::from_rgb(150, 200, 150)),
                            );

                            ui.end_row();
                        }
                    });
            });
    }

    /// Try to read a memory region directly from host memory.
    fn read_region(&self, region: &MemoryRegion) -> Option<Vec<u8>> {
        unsafe {
            let ptr = region.ptr as *const u8;
            if ptr.is_null() {
                return None;
            }
            let buf = std::slice::from_raw_parts(ptr.add(region.offset), region.size);
            Some(buf.to_vec())
        }
    }
}
