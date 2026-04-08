use bevy_egui::egui;
use std::sync::{Arc, Mutex};
use crate::debug::DebugState;

pub struct FrameInspector {
    zoom: f32,
    last_generation: u64,
    texture: Option<egui::TextureHandle>,
    hover_pixel: Option<(u32, u32, [u8; 4])>,
}

impl FrameInspector {
    pub fn new() -> Self {
        FrameInspector { zoom: 2.0, last_generation: u64::MAX, texture: None, hover_pixel: None }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, state: &Arc<Mutex<DebugState>>) {
        let (generation, width, height, rgba) = {
            let s = state.lock().unwrap();
            (s.fb_generation, s.fb_width, s.fb_height, s.fb_rgba.clone())
        };

        // Upload texture only when frame changes
        if generation != self.last_generation && width > 0 && height > 0 && !rgba.is_empty() {
            let pixels: Vec<egui::Color32> = rgba.chunks_exact(4).map(|p| {
                egui::Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3])
            }).collect();
            let image = egui::ColorImage {
                size: [width as usize, height as usize],
                pixels,
            };
            self.texture = Some(ctx.load_texture(
                "framebuffer",
                image,
                egui::TextureOptions::NEAREST,
            ));
            self.last_generation = generation;
        }

        // Controls bar
        ui.horizontal(|ui| {
            ui.label("Zoom:");
            ui.add(egui::Slider::new(&mut self.zoom, 1.0..=8.0).step_by(0.5));
            if ui.button("1×").clicked() { self.zoom = 1.0; }
            if ui.button("2×").clicked() { self.zoom = 2.0; }
            if ui.button("4×").clicked() { self.zoom = 4.0; }
            ui.separator();
            if let Some((px, py, [r, g, b, _])) = self.hover_pixel {
                ui.label(format!("({px},{py}) R:{r} G:{g} B:{b}"));
            }
            ui.separator();
            if ui.button("💾 Save PNG").clicked() {
                if let Ok(s) = state.lock() {
                    save_png(&s.fb_rgba, s.fb_width, s.fb_height, s.frame_count);
                }
            }
        });

        ui.separator();

        egui::ScrollArea::both().show(ui, |ui| {
            if let Some(tex) = &self.texture {
                let display_size = egui::vec2(
                    width as f32 * self.zoom,
                    height as f32 * self.zoom,
                );
                let response = ui.add(
                    egui::Image::new(tex)
                        .fit_to_exact_size(display_size)
                        .sense(egui::Sense::hover()),
                );
                // Pixel under cursor
                if let Some(pos) = response.hover_pos() {
                    let rect = response.rect;
                    let rel = pos - rect.min;
                    let px = (rel.x / self.zoom) as u32;
                    let py = (rel.y / self.zoom) as u32;
                    if let Ok(s) = state.lock() {
                        if px < s.fb_width && py < s.fb_height && !s.fb_rgba.is_empty() {
                            let idx = (py as usize * s.fb_width as usize + px as usize) * 4;
                            if idx + 3 < s.fb_rgba.len() {
                                let p = &s.fb_rgba[idx..idx+4];
                                self.hover_pixel = Some((px, py, [p[0], p[1], p[2], p[3]]));
                            }
                        }
                    }
                }
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("No frame yet — start emulation and press F12");
                });
            }
        });
    }
}

fn save_png(rgba: &[u8], width: u32, height: u32, frame: u64) {
    if rgba.is_empty() { return; }
    let path = format!("frame_{frame:06}.png");
    if let Some(img) = image::RgbaImage::from_raw(width, height, rgba.to_vec()) {
        let _ = img.save(&path);
        eprintln!("[DEBUG] Saved {path}");
    }
}
