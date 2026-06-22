use bevy_egui::egui;
use std::sync::{Arc, Mutex};
use crate::debug::{DebugState, MemoryRegion};
use super::hex_tint;

pub struct HexDump {
    goto_addr: String,
    scroll_to: Option<usize>,
    current_region_idx: usize,
    /// Previous frame's displayed bytes — used for per-byte change tinting.
    prev_buf: Vec<u8>,
    /// Index of the region (or usize::MAX for framebuffer) that `prev_buf` belongs to.
    prev_region_key: usize,
    /// When true, bytes that changed since the last frame are tinted amber.
    highlight_changes: bool,
}

impl HexDump {
    pub fn new() -> Self {
        HexDump {
            goto_addr: String::new(),
            scroll_to: None,
            current_region_idx: 0,
            prev_buf: Vec::new(),
            prev_region_key: usize::MAX,
            highlight_changes: true,
        }
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

            ui.separator();
            ui.checkbox(&mut self.highlight_changes, "highlight changes");
        });

        ui.separator();

        // Get data from selected region or framebuffer.
        // region_key distinguishes which source is active so we can reset
        // prev_buf on a region switch (avoiding false-positive tints).
        let (region, buf_to_display, region_key) = if !regions.is_empty() {
            let region = &regions[self.current_region_idx];
            if let Some(buf) = self.read_region(region) {
                (Some(region.clone()), buf, self.current_region_idx)
            } else {
                (None, Vec::new(), usize::MAX)
            }
        } else {
            // Framebuffer path: use usize::MAX - 1 as a stable key distinct from
            // any valid region index.
            (None, current_fb.0, usize::MAX - 1)
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

        // Compute per-byte changed mask.
        // Reset prev_buf when the active region changes or lengths differ
        // (length may change on the same region during init); tint nothing that frame.
        let changed_mask: Vec<bool> = if self.highlight_changes
            && self.prev_region_key == region_key
            && self.prev_buf.len() == buf_to_display.len()
        {
            hex_tint::diff_changed(&self.prev_buf, &buf_to_display)
        } else {
            // Reset: no tint this frame, just snapshot.
            vec![false; buf_to_display.len()]
        };

        // Store snapshot for next frame.
        self.prev_buf = buf_to_display.clone();
        self.prev_region_key = region_key;

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

                            // Hex bytes column — each byte gets its own colored label
                            // so changed bytes can be tinted amber independently.
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                for (i, &b) in slice.iter().enumerate() {
                                    let byte_idx = start + i;
                                    let changed = changed_mask
                                        .get(byte_idx)
                                        .copied()
                                        .unwrap_or(false);

                                    let color = if changed {
                                        // Amber tint for changed bytes (highlight_changes
                                        // is implicitly true here because the mask is only
                                        // non-false when highlight_changes was true).
                                        hex_tint::changed_color(true)
                                    } else {
                                        // Existing white/dark-gray logic based on non-zero value.
                                        if b != 0 {
                                            egui::Color32::WHITE
                                        } else {
                                            egui::Color32::DARK_GRAY
                                        }
                                    };

                                    // Add a thin space before each byte except the first,
                                    // and an extra space at the 8-byte mid-group boundary.
                                    if i > 0 {
                                        let gap = if i == 8 { "  " } else { " " };
                                        ui.label(egui::RichText::new(gap).monospace());
                                    }
                                    ui.label(
                                        egui::RichText::new(format!("{:02X}", b))
                                            .monospace()
                                            .color(color),
                                    );
                                }
                            });

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
