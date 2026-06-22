use bevy_egui::egui::{self, Color32, RichText, ScrollArea, TextEdit, TextureHandle, ColorImage};
use crate::debug::DebugState;

pub struct RegionsPanel {
    edit_label_idx: Option<usize>,
    edit_label_buf: String,
    edit_notes_idx: Option<usize>,
    edit_notes_buf: String,
    /// Cached egui textures for bookmark thumbnails (index → handle).
    thumb_textures: Vec<Option<TextureHandle>>,
    thumb_generations: Vec<u64>,
    heatmap_filter: String,
}

impl RegionsPanel {
    pub fn new() -> Self {
        RegionsPanel {
            edit_label_idx: None,
            edit_label_buf: String::new(),
            edit_notes_idx: None,
            edit_notes_buf: String::new(),
            thumb_textures: Vec::new(),
            thumb_generations: Vec::new(),
            heatmap_filter: String::new(),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, ds: &mut DebugState) {
        // ── Toolbar ─────────────────────────────────────────────────────────
        ui.horizontal(|ui| {
            if ui.button("📌 Bookmark now  [B]").clicked() {
                ds.create_bookmark = true;
            }
            ui.separator();
            if ui.button("💾 Save").on_hover_text("Save bookmarks and regions to sidecar JSON").clicked() {
                ds.save_regions = true;
            }
            if let Some(ref path) = ds.sidecar_path {
                ui.label(egui::RichText::new(format!("→ {}", path.display()))
                    .small().color(egui::Color32::DARK_GRAY));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(format!(
                    "{} bookmarks  ·  {} heatmap entries  ·  {} regions",
                    ds.bookmarks.len(), ds.pc_heatmap.len(), ds.code_regions.len()))
                    .small().color(egui::Color32::GRAY));
            });
        });
        ui.separator();

        // ── Bookmarks section ───────────────────────────────────────────────
        egui::CollapsingHeader::new(format!("🗂 Bookmarks ({})", ds.bookmarks.len()))
            .default_open(true)
            .show(ui, |ui| {
                self.show_bookmarks(ui, ctx, ds);
            });

        ui.add_space(4.0);

        // ── PC Heatmap section ──────────────────────────────────────────────
        egui::CollapsingHeader::new(format!("🌡 PC Heatmap ({} unique addresses)", ds.pc_heatmap.len()))
            .default_open(true)
            .show(ui, |ui| {
                self.show_heatmap(ui, ds);
            });

        ui.add_space(4.0);

        // ── Code Regions section ────────────────────────────────────────────
        egui::CollapsingHeader::new(format!("🏷 Code Regions ({})", ds.code_regions.len()))
            .default_open(true)
            .show(ui, |ui| {
                self.show_code_regions(ui, ds);
            });
    }

    // ── Bookmarks ─────────────────────────────────────────────────────────────

    fn show_bookmarks(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, ds: &mut DebugState) {
        if ds.bookmarks.is_empty() {
            ui.label(RichText::new("No bookmarks yet. Press B during gameplay to capture one.")
                .italics().color(Color32::GRAY));
            return;
        }

        // Grow texture cache to match bookmark count
        while self.thumb_textures.len() < ds.bookmarks.len() {
            self.thumb_textures.push(None);
            self.thumb_generations.push(u64::MAX);
        }

        let mut delete_idx: Option<usize> = None;

        ScrollArea::vertical().max_height(300.0).id_salt("bookmark_scroll").show(ui, |ui| {
            for (i, bm) in ds.bookmarks.iter_mut().enumerate() {
                ui.push_id(i, |ui| {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // Thumbnail
                            if !bm.thumbnail.is_empty() {
                                let tex = &mut self.thumb_textures[i];
                                let gen = &mut self.thumb_generations[i];
                                if tex.is_none() || *gen != bm.frame {
                                    let img = ColorImage::from_rgba_unmultiplied([64, 48], &bm.thumbnail);
                                    *tex = Some(ctx.load_texture(
                                        format!("bm_thumb_{}", i), img, Default::default()));
                                    *gen = bm.frame;
                                }
                                if let Some(ref t) = tex {
                                    ui.image((t.id(), egui::vec2(64.0, 48.0)));
                                }
                            } else {
                                ui.allocate_exact_size(egui::vec2(64.0, 48.0), egui::Sense::hover());
                            }

                            ui.vertical(|ui| {
                                // Label (editable inline)
                                if self.edit_label_idx == Some(i) {
                                    let resp = ui.text_edit_singleline(&mut self.edit_label_buf);
                                    if resp.lost_focus() || ui.input(|r| r.key_pressed(egui::Key::Enter)) {
                                        bm.label = self.edit_label_buf.clone();
                                        self.edit_label_idx = None;
                                    }
                                } else if ui.label(RichText::new(&bm.label).strong().color(Color32::WHITE)).double_clicked() {
                                    self.edit_label_idx = Some(i);
                                    self.edit_label_buf = bm.label.clone();
                                }

                                ui.label(RichText::new(format!(
                                    "Frame {:>8}  |  PC: ${:06X}", bm.frame, bm.m68k_pc))
                                    .monospace().color(Color32::LIGHT_GRAY));

                                // Register summary: D0/A0 as quick peek
                                ui.label(RichText::new(format!(
                                    "D0=${:08X} D1=${:08X}  A6=${:08X} A7=${:08X}",
                                    bm.m68k_d_regs[0], bm.m68k_d_regs[1],
                                    bm.m68k_a_regs[6], bm.m68k_a_regs[7]))
                                    .monospace().small().color(Color32::DARK_GRAY));

                                // Notes
                                if self.edit_notes_idx == Some(i) {
                                    let resp = ui.add(TextEdit::singleline(&mut self.edit_notes_buf)
                                        .hint_text("Notes…").desired_width(250.0));
                                    if resp.lost_focus() {
                                        bm.notes = self.edit_notes_buf.clone();
                                        self.edit_notes_idx = None;
                                    }
                                } else {
                                    let notes_text = if bm.notes.is_empty() { "📝 add notes…" } else { &bm.notes };
                                    if ui.label(RichText::new(notes_text).small()
                                        .color(if bm.notes.is_empty() { Color32::DARK_GRAY } else { Color32::LIGHT_GRAY }))
                                        .clicked()
                                    {
                                        self.edit_notes_idx = Some(i);
                                        self.edit_notes_buf = bm.notes.clone();
                                    }
                                }
                            });

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                                if ui.small_button("🗑").on_hover_text("Delete bookmark").clicked() {
                                    delete_idx = Some(i);
                                }
                            });
                        });
                    });
                    ui.add_space(2.0);
                });
            }
        });

        if let Some(i) = delete_idx {
            ds.bookmarks.remove(i);
            self.thumb_textures.remove(i);
            self.thumb_generations.remove(i);
            if self.edit_label_idx == Some(i) { self.edit_label_idx = None; }
            if self.edit_notes_idx == Some(i) { self.edit_notes_idx = None; }
        }
    }

    // ── PC Heatmap ────────────────────────────────────────────────────────────

    fn show_heatmap(&mut self, ui: &mut egui::Ui, ds: &mut DebugState) {
        if ds.pc_heatmap.is_empty() {
            ui.label(RichText::new("No data yet — heatmap fills automatically as the game runs.")
                .italics().color(Color32::GRAY));
            return;
        }

        ui.horizontal(|ui| {
            ui.label("Filter address:");
            ui.add(TextEdit::singleline(&mut self.heatmap_filter)
                .hint_text("e.g. 0x04 or 02").desired_width(120.0));
            if ui.small_button("✖").clicked() { self.heatmap_filter.clear(); }
            if ui.button("🗑 Clear Heatmap").clicked() { ds.pc_heatmap.clear(); }
        });

        // Sort by count descending, apply filter, show top 100
        let filter = self.heatmap_filter.trim().to_lowercase();
        let mut entries: Vec<(u32, u64)> = ds.pc_heatmap.iter()
            .map(|(&a, &c)| (a, c))
            .filter(|(a, _)| filter.is_empty() || format!("{:06x}", a).contains(&filter))
            .collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        entries.truncate(100);

        let max_count = entries.first().map(|e| e.1).unwrap_or(1).max(1);

        ui.label(format!("Top {} addresses (of {} unique):", entries.len(), ds.pc_heatmap.len()));

        // Collect a goto target into a local to avoid borrowing ds during the grid loop.
        let mut goto_addr: Option<u32> = None;

        ScrollArea::vertical().max_height(250.0).id_salt("heatmap_scroll").show(ui, |ui| {
            egui::Grid::new("heatmap_grid")
                .num_columns(4)
                .striped(true)
                .spacing([8.0, 2.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Address").strong());
                    ui.label(RichText::new("Visits").strong());
                    ui.label(RichText::new("Heat").strong());
                    ui.label(RichText::new("").strong());
                    ui.end_row();

                    for (addr, count) in &entries {
                        let heat = *count as f32 / max_count as f32;
                        // Color: cool blue → hot red via orange
                        let r = (heat * 255.0) as u8;
                        let g = ((1.0 - heat) * 180.0) as u8;
                        let b = ((1.0 - heat) * 255.0) as u8;
                        let color = Color32::from_rgb(r, g, b);

                        ui.label(RichText::new(format!("${:06X}", addr)).monospace().color(color));
                        ui.label(RichText::new(format!("{:>8}", count)).monospace().color(color));

                        // Heat bar
                        let bar_width = (heat * 120.0).max(2.0);
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(120.0, 12.0), egui::Sense::hover());
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(rect.min, egui::vec2(bar_width, 12.0)),
                            2.0, color);

                        // Navigation button — jump Disasm/Hex to this address.
                        if ui.small_button("→").on_hover_text("Navigate Disasm/Hex to this address").clicked() {
                            goto_addr = Some(*addr);
                        }
                        ui.end_row();
                    }
                });
        });

        // Apply goto after the borrow of ds.pc_heatmap has ended.
        if let Some(addr) = goto_addr {
            ds.goto(addr);
        }
    }

    // ── Code Regions ─────────────────────────────────────────────────────────

    fn show_code_regions(&mut self, ui: &mut egui::Ui, ds: &mut DebugState) {
        if ds.code_regions.is_empty() {
            ui.label(RichText::new("No labeled regions yet. (Address range labeling coming in Phase 2.)")
                .italics().color(Color32::GRAY));
            return;
        }

        let mut delete_idx: Option<usize> = None;
        // Collect goto target into a local to avoid borrowing ds.code_regions during the grid loop.
        let mut goto_addr: Option<u32> = None;

        ScrollArea::vertical().max_height(200.0).id_salt("regions_scroll").show(ui, |ui| {
            egui::Grid::new("regions_grid")
                .num_columns(5)
                .striped(true)
                .show(ui, |ui| {
                    ui.label(RichText::new("Label").strong());
                    ui.label(RichText::new("Start").strong());
                    ui.label(RichText::new("End").strong());
                    ui.label(RichText::new("").strong());
                    ui.label(RichText::new("").strong());
                    ui.end_row();

                    for (i, region) in ds.code_regions.iter().enumerate() {
                        let c = region.color;
                        let color = Color32::from_rgb(c[0], c[1], c[2]);
                        ui.label(RichText::new(&region.label).color(color).strong());

                        // Clickable start address — navigates to region start.
                        if ui.small_button(
                            RichText::new(format!("${:06X}", region.addr_start)).monospace()
                        ).on_hover_text("Navigate Disasm/Hex to region start").clicked() {
                            goto_addr = Some(region.addr_start);
                        }

                        ui.label(RichText::new(format!("${:06X}", region.addr_end)).monospace());
                        if ui.small_button("→").on_hover_text("Navigate to region start").clicked() {
                            goto_addr = Some(region.addr_start);
                        }
                        if ui.small_button("🗑").clicked() { delete_idx = Some(i); }
                        ui.end_row();
                    }
                });
        });

        if let Some(i) = delete_idx { ds.code_regions.remove(i); }
        // Apply goto after the borrow of ds.code_regions has ended.
        if let Some(addr) = goto_addr {
            ds.goto(addr);
        }
    }
}
