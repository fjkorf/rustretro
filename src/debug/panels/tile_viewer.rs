use eframe::egui;
use std::sync::{Arc, Mutex};
use crate::debug::DebugState;

const TILE_SIZE: usize = 16;

pub struct TileViewer {
    last_generation: u64,
    tiles: Vec<egui::ColorImage>,
    tile_textures: Vec<Option<egui::TextureHandle>>,
    selected: Option<usize>,
    hide_blank: bool,
    zoom: f32,
}

impl TileViewer {
    pub fn new() -> Self {
        TileViewer {
            last_generation: u64::MAX,
            tiles: Vec::new(),
            tile_textures: Vec::new(),
            selected: None,
            hide_blank: true,
            zoom: 3.0,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, state: &Arc<Mutex<DebugState>>) {
        let (generation, width, height, rgba) = {
            let s = state.lock().unwrap();
            (s.fb_generation, s.fb_width, s.fb_height, s.fb_rgba.clone())
        };

        // Re-slice into tiles when frame changes
        if generation != self.last_generation && width > 0 && height > 0 && !rgba.is_empty() {
            self.tiles = slice_tiles(&rgba, width as usize, height as usize, TILE_SIZE);
            self.tile_textures = vec![None; self.tiles.len()];
            self.last_generation = generation;
        }

        ui.horizontal(|ui| {
            ui.label("Tile size: 16×16");
            ui.separator();
            ui.checkbox(&mut self.hide_blank, "Hide blank tiles");
            ui.separator();
            ui.label("Zoom:");
            ui.add(egui::Slider::new(&mut self.zoom, 1.0..=8.0).step_by(1.0));
            ui.separator();
            ui.label(format!("{} tiles total", self.tiles.len()));
        });
        ui.separator();

        if self.tiles.is_empty() {
            ui.label("No frame data yet.");
            return;
        }

        egui::SidePanel::right("tile_detail").min_width(200.0).show_inside(ui, |ui| {
            ui.heading("Selected Tile");
            if let Some(idx) = self.selected {
                ui.label(format!("Tile #{idx}"));
                let tile = &self.tiles[idx];
                // Lazy-load texture
                if self.tile_textures[idx].is_none() {
                    self.tile_textures[idx] = Some(ctx.load_texture(
                        format!("tile_{idx}"),
                        tile.clone(),
                        egui::TextureOptions::NEAREST,
                    ));
                }
                if let Some(tex) = &self.tile_textures[idx] {
                    let size = egui::vec2(TILE_SIZE as f32 * 6.0, TILE_SIZE as f32 * 6.0);
                    ui.add(egui::Image::new(tex).fit_to_exact_size(size));
                }
                // Show raw pixel values
                ui.separator();
                ui.label("Pixels (RGBA):");
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    for (i, px) in tile.pixels.iter().enumerate() {
                        let x = i % TILE_SIZE;
                        let y = i / TILE_SIZE;
                        let [r, g, b, _] = px.to_array();
                        ui.label(egui::RichText::new(
                            format!("({x:2},{y:2}) #{r:02X}{g:02X}{b:02X}")
                        ).monospace().size(10.0));
                    }
                });
            } else {
                ui.label("Click a tile to inspect it.");
            }
        });

        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            let tile_px = TILE_SIZE as f32 * self.zoom;
            let padding = 2.0;
            let available = ui.available_width();
            let cols = ((available / (tile_px + padding)).floor() as usize).max(1);

            let mut col = 0;
            ui.horizontal_wrapped(|ui| {
                for (idx, tile) in self.tiles.iter().enumerate() {
                    let is_blank = tile.pixels.iter().all(|p| *p == egui::Color32::BLACK);
                    if self.hide_blank && is_blank { continue; }

                    // Lazy-load texture
                    if self.tile_textures[idx].is_none() {
                        self.tile_textures[idx] = Some(ctx.load_texture(
                            format!("tile_{idx}"),
                            tile.clone(),
                            egui::TextureOptions::NEAREST,
                        ));
                    }

                    if let Some(tex) = &self.tile_textures[idx] {
                        let selected = self.selected == Some(idx);
                        let size = egui::vec2(tile_px, tile_px);
                        let border_color = if selected {
                            egui::Color32::YELLOW
                        } else {
                            egui::Color32::DARK_GRAY
                        };

                        let frame = egui::Frame::default()
                            .stroke(egui::Stroke::new(if selected { 2.0 } else { 1.0 }, border_color))
                            .inner_margin(egui::Margin::ZERO);

                        let resp = frame.show(ui, |ui| {
                            ui.add(egui::Image::new(tex)
                                .fit_to_exact_size(size)
                                .sense(egui::Sense::click()))
                        }).inner;

                        if resp.clicked() {
                            self.selected = Some(idx);
                        }
                        resp.on_hover_text(format!("Tile #{idx}"));
                    }

                    col += 1;
                    if col >= cols { col = 0; }
                }
            });
        });
    }
}

fn slice_tiles(rgba: &[u8], width: usize, height: usize, tile_size: usize) -> Vec<egui::ColorImage> {
    let cols = width / tile_size;
    let rows = height / tile_size;
    let mut tiles = Vec::with_capacity(cols * rows);

    for ty in 0..rows {
        for tx in 0..cols {
            let mut pixels = Vec::with_capacity(tile_size * tile_size);
            for py in 0..tile_size {
                for px in 0..tile_size {
                    let sx = tx * tile_size + px;
                    let sy = ty * tile_size + py;
                    let idx = (sy * width + sx) * 4;
                    if idx + 3 < rgba.len() {
                        pixels.push(egui::Color32::from_rgba_premultiplied(
                            rgba[idx], rgba[idx+1], rgba[idx+2], rgba[idx+3]
                        ));
                    } else {
                        pixels.push(egui::Color32::BLACK);
                    }
                }
            }
            tiles.push(egui::ColorImage {
                size: [tile_size, tile_size],
                pixels,
                source_size: egui::Vec2::new(tile_size as f32, tile_size as f32),
            });
        }
    }
    tiles
}
