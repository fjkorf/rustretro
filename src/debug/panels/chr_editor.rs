//! CHR (pattern-table) editor for NES carts.
//!
//! The running core hides a NES cart's CHR-ROM graphics — it's addressed
//! through PPU pattern tables, not exposed as readable host memory the way
//! RAM is (see `mcp::ines`'s module doc) — but the `.nes` FILE carries the
//! tiles at a known offset. This panel browses every tile via the same iNES
//! parsing + 2bpp decoder the MCP `rom_file:chr` source already uses, lets an
//! operator paint individual tile pixels in a zoomed 8×8 grid, tracks which
//! tiles were touched this session (with per-tile undo), and exports the
//! edited cart to a new `.nes` file.
//!
//! Decode/encode/span logic is NOT duplicated here: [`chr_span`] lives in
//! `mcp::ines` (shared with the MCP `rom_file:chr` source) and
//! [`decode_2bpp_planar_indices`]/[`encode_2bpp_planar_row`] live in
//! `mcp::snapshot` (shared with `render_tiles`). This module only adds the
//! interactive browse/edit/save state around those pure functions.
//!
//! NES-only, and honest about it: a non-NES core/cart, no ROM, a non-iNES
//! file, or a CHR-RAM cart (nothing in the file to edit) each render an
//! explicit message — never a fabricated or zeroed tile grid — cached once
//! per ROM-identity change so there is no per-frame retry storm.

use bevy_egui::egui;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::debug::DebugState;
use crate::mcp::ines::{chr_span, parse_ines};
use crate::mcp::snapshot::{
    decode_2bpp_planar_indices, encode_2bpp_planar_row, gray_ramp_rgba, BYTES_PER_2BPP_TILE,
    TILE_PX,
};

/// Tiles are grouped into browsing "banks" of this size purely for UI
/// pagination. This is NOT a claim about the cart's runtime CHR-bank
/// switching (a mapper bank-select register is a RUNTIME fact, not decodable
/// from the file alone) — it's just how many tiles fit a comfortable page.
/// 512 matches the common NES pattern-table half (256 tiles × 2 halves is
/// also common; 512 keeps the math simple and cart-size-agnostic since bank
/// COUNT is always computed as `ceil(tile_count / TILES_PER_BANK)`, never
/// hardcoded).
const TILES_PER_BANK: usize = 512;

/// Why the editor cannot show tiles right now, or that it can.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Status {
    NoRom,
    NotNes(String),
    NoChr(String),
    Ready,
}

/// The editor's color source, factored behind [`palette_color`] so a real NES
/// palette can plug in later without touching any painting code.
///
/// `Nes(..)` (live PPU palette RAM, `$3F00-$3F1F`) is a known future seam,
/// deliberately NOT implemented today: no NES core in this tree exposes PPU
/// palette RAM as a readable memory region (nestopia's memory capability is
/// "System RAM (fallback)" only, per the live-boot facts in this program's
/// evidence docs), so there is nothing to read it FROM yet even if the
/// variant existed.
enum EditorPalette {
    GrayscaleStructureOnly,
}

fn palette_color(index: u8, palette: &EditorPalette) -> egui::Color32 {
    match palette {
        EditorPalette::GrayscaleStructureOnly => {
            let [r, g, b, a] = gray_ramp_rgba(index, 4);
            egui::Color32::from_rgba_premultiplied(r, g, b, a)
        }
    }
}

/// Encode a whole tile's 64 palette indices (row-major, 8×8) back into its
/// 16-byte 2bpp planar form, one row at a time via
/// [`encode_2bpp_planar_row`]. `indices` must have exactly `TILE_PX*TILE_PX`
/// entries.
fn encode_tile_bytes(indices: &[u8]) -> [u8; BYTES_PER_2BPP_TILE] {
    debug_assert_eq!(indices.len(), TILE_PX * TILE_PX);
    let mut out = [0u8; BYTES_PER_2BPP_TILE];
    for y in 0..TILE_PX {
        let row: [u8; 8] = indices[y * TILE_PX..(y + 1) * TILE_PX]
            .try_into()
            .expect("TILE_PX == 8");
        let (plane0, plane1) = encode_2bpp_planar_row(&row);
        out[y] = plane0;
        out[TILE_PX + y] = plane1;
    }
    out
}

/// Compute a non-colliding default Save-As path: `{stem}_edited.nes` next to
/// `rom_path`, then `{stem}_edited-2.nes`, `-3`, ... if that's already taken.
/// Never suggests an existing file as the default — the "NEVER silently
/// overwrite" law applies to the suggestion too, not just the write. The user
/// can still type an existing path in by hand (that's the in-place-save path,
/// gated separately by the overwrite checkbox).
fn default_save_path(rom_path: Option<&std::path::Path>) -> String {
    let Some(rom_path) = rom_path else {
        return String::new();
    };
    let stem = rom_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("rom");
    let mut candidate = rom_path.with_file_name(format!("{stem}_edited.nes"));
    let mut n = 2;
    while candidate.exists() {
        candidate = rom_path.with_file_name(format!("{stem}_edited-{n}.nes"));
        n += 1;
    }
    candidate.to_string_lossy().into_owned()
}

/// CHR editor debug panel state.
pub struct ChrEditor {
    /// `rom_path` this buffer was (last attempted to be) loaded from. `None`
    /// both initially and when no ROM is loaded — compared each frame against
    /// the live `DebugState` so a load is attempted exactly once per identity
    /// change, never once per frame (no retry storm on a permanent failure).
    loaded_from: Option<PathBuf>,
    status: Status,

    /// Full cart bytes, working copy — patched in place as tiles are edited.
    file_bytes: Vec<u8>,
    chr_start: usize,
    chr_len: usize,
    prg_start: usize,
    prg_len: usize,
    header: [u8; 16],
    tile_count: usize,
    bank_count: usize,

    /// Decoded palette indices for the whole CHR span, tile-major then
    /// row-major (`TILE_PX*TILE_PX` per tile) — kept in sync with
    /// `file_bytes[chr_start..]` incrementally on every edit/undo so the
    /// browser/editor never re-decodes the whole span per frame.
    chr_indices: Vec<u8>,

    /// Tiles touched this session. "Touched", not "differs from original" —
    /// undoing a tile back to its pre-session bytes does NOT clear its dirty
    /// bit. Simpler invariant (dirty = ever-edited), and it's an honest
    /// session log rather than a stale-feeling diff. This is a deliberate,
    /// documented choice — flip it if a future session wants the other one.
    dirty_tiles: HashSet<usize>,
    /// One entry per discrete edit: `(tile_idx, pre-edit 16 raw bytes)`.
    undo_stack: Vec<(usize, [u8; BYTES_PER_2BPP_TILE])>,

    selected_bank: usize,
    selected_tile: Option<usize>,
    tile_textures: Vec<Option<egui::TextureHandle>>,
    hide_blank: bool,
    zoom: f32,
    palette: EditorPalette,
    /// The palette index the next click paints with.
    draw_index: u8,

    save_path_input: String,
    save_note: Option<String>,
    /// Gate for a save that would overwrite an existing file (in-place onto
    /// the loaded ROM, or onto any other pre-existing path) — the "NEVER
    /// silently overwrite" law.
    overwrite_confirmed: bool,
}

impl ChrEditor {
    pub fn new() -> Self {
        ChrEditor {
            loaded_from: None,
            status: Status::NoRom,
            file_bytes: Vec::new(),
            chr_start: 0,
            chr_len: 0,
            prg_start: 0,
            prg_len: 0,
            header: [0u8; 16],
            tile_count: 0,
            bank_count: 0,
            chr_indices: Vec::new(),
            dirty_tiles: HashSet::new(),
            undo_stack: Vec::new(),
            selected_bank: 0,
            selected_tile: None,
            tile_textures: Vec::new(),
            hide_blank: false,
            zoom: 3.0,
            palette: EditorPalette::GrayscaleStructureOnly,
            draw_index: 3,
            save_path_input: String::new(),
            save_note: None,
            overwrite_confirmed: false,
        }
    }

    fn snapshot_rom(state: &Arc<Mutex<DebugState>>) -> (Option<String>, Option<Vec<u8>>, Option<PathBuf>) {
        let s = state.lock().unwrap();
        (s.rom_system.clone(), s.rom_bytes.clone(), s.rom_path.clone())
    }

    /// (Re)load from a fresh ROM snapshot, discarding any in-progress edits.
    /// Called once per ROM-identity change (see `loaded_from`), or on demand
    /// via the "Reload from ROM" button.
    fn load(&mut self, rom_system: Option<&str>, rom_bytes: Option<Vec<u8>>, rom_path: Option<PathBuf>) {
        self.file_bytes.clear();
        self.chr_indices.clear();
        self.dirty_tiles.clear();
        self.undo_stack.clear();
        self.selected_tile = None;
        self.tile_textures.clear();
        self.save_note = None;
        self.overwrite_confirmed = false;

        let no_bytes_hint = rom_bytes.as_ref().map(|b| b.is_empty()).unwrap_or(true);
        if rom_path.is_none() && no_bytes_hint {
            self.status = Status::NoRom;
            return;
        }
        if rom_system != Some("nes") {
            self.status = Status::NotNes(
                rom_system.map(|s| s.to_string()).unwrap_or_else(|| "unrecognized".to_string()),
            );
            return;
        }

        // Prefer retained bytes; fall back to re-reading the retained path
        // (handles a hypothetical need_fullpath NES core, mirroring
        // mcp::server's rom_file_bytes fallback).
        let bytes = match rom_bytes.filter(|b| !b.is_empty()) {
            Some(b) => b,
            None => match rom_path.as_ref().and_then(|p| std::fs::read(p).ok()) {
                Some(b) => b,
                None => {
                    self.status = Status::NoRom;
                    return;
                }
            },
        };

        if parse_ines(&bytes).is_none() {
            self.status = Status::NoChr("This file has no iNES header — nothing to edit.".to_string());
            return;
        }
        let (chr_start, chr_end) = match chr_span(&bytes) {
            Ok(span) => span,
            Err(e) => {
                self.status = Status::NoChr(format!(
                    "No CHR-ROM bytes in this file to edit: {e} (the graphics may live \
                     compressed in PRG-ROM, or only in live CHR-RAM via the core)."
                ));
                return;
            }
        };
        // parse_ines already succeeded above, so this unwrap is safe — kept
        // separate from chr_span so the PRG span is available too.
        let info = parse_ines(&bytes).expect("checked above");

        let chr_len = chr_end - chr_start;
        let tile_count = chr_len / BYTES_PER_2BPP_TILE;
        let bank_count = tile_count.div_ceil(TILES_PER_BANK).max(1);

        let mut header = [0u8; 16];
        header.copy_from_slice(&bytes[0..16]);

        self.chr_indices = decode_2bpp_planar_indices(&bytes[chr_start..chr_end]);
        self.prg_start = info.prg_offset;
        self.prg_len = info.prg_rom_size;
        self.chr_start = chr_start;
        self.chr_len = chr_len;
        self.header = header;
        self.tile_count = tile_count;
        self.bank_count = bank_count;
        self.selected_bank = 0;
        self.tile_textures = vec![None; tile_count];
        self.save_path_input = default_save_path(rom_path.as_deref());
        self.file_bytes = bytes;
        self.status = Status::Ready;
    }

    fn tile_indices(&self, tile_idx: usize) -> &[u8] {
        let base = tile_idx * TILE_PX * TILE_PX;
        &self.chr_indices[base..base + TILE_PX * TILE_PX]
    }

    fn tile_color_image(&self, tile_idx: usize) -> egui::ColorImage {
        let pixels: Vec<egui::Color32> = self
            .tile_indices(tile_idx)
            .iter()
            .map(|&i| palette_color(i, &self.palette))
            .collect();
        egui::ColorImage {
            size: [TILE_PX, TILE_PX],
            source_size: egui::Vec2::new(TILE_PX as f32, TILE_PX as f32),
            pixels,
        }
    }

    fn paint_pixel(&mut self, tile_idx: usize, cx: usize, cy: usize) {
        let base = tile_idx * TILE_PX * TILE_PX;
        let pos = base + cy * TILE_PX + cx;
        if self.chr_indices[pos] == self.draw_index {
            return; // no-op click — don't spam undo/dirty for it
        }

        let tile_off = self.chr_start + tile_idx * BYTES_PER_2BPP_TILE;
        let pre_edit: [u8; BYTES_PER_2BPP_TILE] = self.file_bytes
            [tile_off..tile_off + BYTES_PER_2BPP_TILE]
            .try_into()
            .expect("BYTES_PER_2BPP_TILE-sized slice");
        self.undo_stack.push((tile_idx, pre_edit));

        self.chr_indices[pos] = self.draw_index;
        let new_bytes = encode_tile_bytes(&self.chr_indices[base..base + TILE_PX * TILE_PX]);
        self.file_bytes[tile_off..tile_off + BYTES_PER_2BPP_TILE].copy_from_slice(&new_bytes);

        self.dirty_tiles.insert(tile_idx);
        self.tile_textures[tile_idx] = None; // invalidate cached thumbnail
    }

    fn undo(&mut self) {
        let Some((tile_idx, pre_edit)) = self.undo_stack.pop() else {
            return;
        };
        let tile_off = self.chr_start + tile_idx * BYTES_PER_2BPP_TILE;
        self.file_bytes[tile_off..tile_off + BYTES_PER_2BPP_TILE].copy_from_slice(&pre_edit);
        let restored = decode_2bpp_planar_indices(&pre_edit);
        let base = tile_idx * TILE_PX * TILE_PX;
        self.chr_indices[base..base + TILE_PX * TILE_PX].copy_from_slice(&restored);
        self.tile_textures[tile_idx] = None;
        // dirty_tiles is intentionally left untouched — see its doc comment.
    }

    /// Reconstruct header+PRG+CHR explicitly (rather than writing
    /// `file_bytes` wholesale) so PRG is provably untouched BY CONSTRUCTION,
    /// then verify the length invariant (iNES has no checksum, length is the
    /// only cheap one) before writing. `in_place` selects the timestamped
    /// `.bak` step first.
    fn do_save(&mut self, target: &std::path::Path, in_place: bool) -> String {
        if in_place {
            if let Some(orig) = &self.loaded_from {
                let unixtime = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let orig_name = orig
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("rom.nes");
                let bak = orig.with_file_name(format!("{orig_name}.bak.{unixtime}"));
                if let Err(e) = std::fs::copy(orig, &bak) {
                    return format!("FAILED: could not create backup {}: {e}", bak.display());
                }
            }
        }

        let mut out = Vec::with_capacity(16 + self.prg_len + self.chr_len);
        out.extend_from_slice(&self.header);
        out.extend_from_slice(&self.file_bytes[self.prg_start..self.prg_start + self.prg_len]);
        out.extend_from_slice(&self.file_bytes[self.chr_start..self.chr_start + self.chr_len]);

        if out.len() != self.file_bytes.len() {
            return format!(
                "FAILED: reconstructed length {} != source length {} — refusing to write",
                out.len(),
                self.file_bytes.len()
            );
        }

        match std::fs::write(target, &out) {
            Ok(()) => format!(
                "Saved {} ({} tile(s) patched, {} bytes)",
                target.display(),
                self.dirty_tiles.len(),
                out.len()
            ),
            Err(e) => format!("FAILED: {e}"),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, state: &Arc<Mutex<DebugState>>) {
        let (rom_system, rom_bytes, rom_path) = Self::snapshot_rom(state);
        if rom_path != self.loaded_from {
            self.load(rom_system.as_deref(), rom_bytes, rom_path.clone());
            self.loaded_from = rom_path;
        }

        ui.heading("🎨 CHR Editor");
        ui.separator();

        match self.status.clone() {
            Status::NoRom => {
                ui.label("No ROM loaded.");
                return;
            }
            Status::NotNes(sys) => {
                ui.label(format!(
                    "CHR Editor is NES-only (mapper CHR-ROM editing). This core/cart is `{sys}`."
                ));
                return;
            }
            Status::NoChr(reason) => {
                ui.label(reason);
                return;
            }
            Status::Ready => {}
        }

        // ── Top bar ──────────────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label("Bank:");
            for b in 0..self.bank_count {
                if ui
                    .selectable_label(self.selected_bank == b, format!("{b}"))
                    .clicked()
                {
                    self.selected_bank = b;
                }
            }
            ui.separator();
            ui.checkbox(&mut self.hide_blank, "Hide blank");
            ui.separator();
            ui.label("Zoom:");
            ui.add(egui::Slider::new(&mut self.zoom, 1.0..=8.0).step_by(1.0));
            ui.separator();
            ui.label(format!(
                "{} tile(s), {} bank(s)",
                self.tile_count, self.bank_count
            ));
            ui.separator();
            ui.label(format!("{} dirty", self.dirty_tiles.len()));
            if ui
                .add_enabled(!self.undo_stack.is_empty(), egui::Button::new("↩ Undo"))
                .clicked()
            {
                self.undo();
            }
            if ui
                .button("🔄 Reload from ROM (discards edits)")
                .on_hover_text("Re-reads the currently loaded ROM, discarding all edits/undo history this session.")
                .clicked()
            {
                let (rom_system, rom_bytes, rom_path) = Self::snapshot_rom(state);
                self.load(rom_system.as_deref(), rom_bytes, rom_path.clone());
                self.loaded_from = rom_path;
            }
        });
        ui.separator();

        // ── Body: zoomed editor (right) + tile browser (center) ────────
        egui::SidePanel::right("chr_editor_detail")
            .min_width(240.0)
            .show_inside(ui, |ui| {
                self.show_editor(ui);
            });

        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            self.show_browser(ui, ctx);
        });

        self.show_save_bar(ui);

        ui.separator();
        ui.label(
            egui::RichText::new(
                "To preview in-game: F6 (save state) → relaunch RustRetro with the edited \
                 .nes → F7 (load state) → verify visually. TO-VERIFY: whether that round-trip \
                 cleanly re-applies CHR bytes on this core has not been confirmed live — treat \
                 it as an empirical workaround, not a guaranteed mechanism. This build has no \
                 \"Apply to running game\" hot-reload.",
            )
            .color(egui::Color32::DARK_GRAY)
            .small(),
        );
    }

    fn show_editor(&mut self, ui: &mut egui::Ui) {
        ui.heading("Selected tile");
        let Some(tile_idx) = self.selected_tile else {
            ui.label("Click a tile to edit it.");
            return;
        };
        ui.label(format!("Tile #{tile_idx}"));
        if self.dirty_tiles.contains(&tile_idx) {
            ui.colored_label(egui::Color32::from_rgb(220, 140, 40), "edited this session");
        }

        ui.separator();
        ui.label("Draw color:");
        ui.horizontal(|ui| {
            for idx in 0u8..4 {
                let color = palette_color(idx, &self.palette);
                let selected = self.draw_index == idx;
                let border = if selected {
                    egui::Color32::YELLOW
                } else {
                    egui::Color32::DARK_GRAY
                };
                let frame = egui::Frame::default()
                    .stroke(egui::Stroke::new(if selected { 2.0_f32 } else { 1.0_f32 }, border))
                    .inner_margin(egui::Margin::same(2));
                let resp = frame
                    .show(ui, |ui| {
                        let (rect, resp) =
                            ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
                        ui.painter().rect_filled(rect, 0.0, color);
                        resp
                    })
                    .inner;
                if resp.clicked() {
                    self.draw_index = idx;
                }
                resp.on_hover_text(format!("index {idx}"));
            }
        });

        ui.separator();

        // 8×8 zoomed pixel grid, painted directly (no per-edit texture
        // reupload — 64 rects/frame is trivial), same rect-relative
        // hit-testing idiom as frame_inspector's pixel hover.
        let cell_px = 24.0;
        let size = egui::vec2(cell_px * TILE_PX as f32, cell_px * TILE_PX as f32);
        let indices: Vec<u8> = self.tile_indices(tile_idx).to_vec();
        let frame = egui::Frame::default()
            .stroke(egui::Stroke::new(1.0_f32, egui::Color32::DARK_GRAY))
            .inner_margin(egui::Margin::ZERO);
        let response = frame
            .show(ui, |ui| {
                let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
                let painter = ui.painter();
                for y in 0..TILE_PX {
                    for x in 0..TILE_PX {
                        let cell = egui::Rect::from_min_size(
                            rect.min + egui::vec2(x as f32 * cell_px, y as f32 * cell_px),
                            egui::vec2(cell_px, cell_px),
                        );
                        let idx = indices[y * TILE_PX + x];
                        painter.rect_filled(cell, 0.0, palette_color(idx, &self.palette));
                    }
                }
                resp
            })
            .inner;

        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let rel = pos - response.rect.min;
                let cx = (rel.x / cell_px) as usize;
                let cy = (rel.y / cell_px) as usize;
                if cx < TILE_PX && cy < TILE_PX {
                    self.paint_pixel(tile_idx, cx, cy);
                }
            }
        }
    }

    fn show_browser(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let bank_start = self.selected_bank * TILES_PER_BANK;
        let bank_end = (bank_start + TILES_PER_BANK).min(self.tile_count);
        if bank_start >= bank_end {
            ui.label("Empty bank.");
            return;
        }

        let tile_px = TILE_PX as f32 * self.zoom;
        let padding = 2.0;
        let available = ui.available_width();
        let cols = ((available / (tile_px + padding)).floor() as usize).max(1);

        let grid = egui::Grid::new("chr_tile_grid").spacing([padding, padding]);
        grid.show(ui, |ui| {
            let mut col = 0;
            for tile_idx in bank_start..bank_end {
                let is_blank = self.tile_indices(tile_idx).iter().all(|&i| i == 0);
                if self.hide_blank && is_blank {
                    continue;
                }

                if self.tile_textures[tile_idx].is_none() {
                    let image = self.tile_color_image(tile_idx);
                    self.tile_textures[tile_idx] = Some(ctx.load_texture(
                        format!("chr_tile_{tile_idx}"),
                        image,
                        egui::TextureOptions::NEAREST,
                    ));
                }

                if let Some(tex) = &self.tile_textures[tile_idx] {
                    let selected = self.selected_tile == Some(tile_idx);
                    let is_dirty = self.dirty_tiles.contains(&tile_idx);
                    let border_color = if selected {
                        egui::Color32::YELLOW
                    } else if is_dirty {
                        egui::Color32::from_rgb(220, 140, 40)
                    } else {
                        egui::Color32::DARK_GRAY
                    };
                    let frame = egui::Frame::default()
                        .stroke(egui::Stroke::new(if selected { 2.0_f32 } else { 1.0_f32 }, border_color))
                        .inner_margin(egui::Margin::ZERO);
                    let resp = frame
                        .show(ui, |ui| {
                            ui.add(
                                egui::Image::new(tex)
                                    .fit_to_exact_size(egui::vec2(tile_px, tile_px))
                                    .sense(egui::Sense::click()),
                            )
                        })
                        .inner;
                    if resp.clicked() {
                        self.selected_tile = Some(tile_idx);
                    }
                    resp.on_hover_text(format!("Tile #{tile_idx}"));
                }

                col += 1;
                if col >= cols {
                    col = 0;
                    ui.end_row();
                }
            }
        });
    }

    fn show_save_bar(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Save to:");
            ui.add(
                egui::TextEdit::singleline(&mut self.save_path_input)
                    .desired_width(320.0)
                    .hint_text("e.g. tcsurfdesign_edited.nes"),
            );
        });

        let trimmed = self.save_path_input.trim();
        let path_ok = !trimmed.is_empty();
        let target = PathBuf::from(trimmed);
        let is_in_place = path_ok && self.loaded_from.as_deref() == Some(target.as_path());
        let exists = path_ok && target.exists();
        let needs_confirm = path_ok && (is_in_place || exists);

        ui.horizontal(|ui| {
            if needs_confirm {
                ui.checkbox(
                    &mut self.overwrite_confirmed,
                    "I understand this will overwrite an existing file",
                );
            } else {
                self.overwrite_confirmed = false;
            }
            let enabled = path_ok && (!needs_confirm || self.overwrite_confirmed);
            if ui
                .add_enabled(enabled, egui::Button::new("💾 Save As"))
                .clicked()
            {
                self.save_note = Some(self.do_save(&target, is_in_place));
            }
        });

        match &self.save_note {
            Some(note) => {
                let color = if note.starts_with("FAILED") {
                    egui::Color32::from_rgb(230, 120, 120)
                } else {
                    egui::Color32::from_rgb(150, 220, 150)
                };
                ui.label(egui::RichText::new(note).color(color));
            }
            None => {
                ui.label(
                    egui::RichText::new("No save yet this session.")
                        .color(egui::Color32::DARK_GRAY),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── encode/decode round-trip ─────────────────────────────────────────

    #[test]
    fn encode_tile_bytes_round_trips_through_decode() {
        let indices: Vec<u8> = (0..64).map(|i| (i % 4) as u8).collect();
        let bytes = encode_tile_bytes(&indices);
        let decoded = decode_2bpp_planar_indices(&bytes);
        assert_eq!(decoded, indices);
    }

    #[test]
    fn encode_tile_bytes_all_same_index_round_trips() {
        for idx in 0u8..4 {
            let indices = vec![idx; 64];
            let bytes = encode_tile_bytes(&indices);
            let decoded = decode_2bpp_planar_indices(&bytes);
            assert_eq!(decoded, indices, "index {idx} did not round-trip");
        }
    }

    // ── chr_span on a tcsurfdesign-shaped header (via the panel's own load) ─

    fn tcsurfdesign_shaped_rom() -> Vec<u8> {
        // Mapper 3 (CNROM), PRG=32KiB (0xAA-filled), CHR=32KiB (0xCC-filled),
        // no trainer — the shape of the real tcsurfdesign.nes cart.
        let mut rom = vec![0u8; 16];
        rom[0..4].copy_from_slice(&[0x4E, 0x45, 0x53, 0x1A]);
        rom[4] = 2; // PRG: 2 × 16KiB = 32KiB
        rom[5] = 4; // CHR: 4 × 8KiB = 32KiB
        rom[6] = 0x30; // mapper low nibble 3, no trainer/battery
        rom.extend(std::iter::repeat(0xAA).take(0x8000)); // PRG
        rom.extend(std::iter::repeat(0xCC).take(0x8000)); // CHR
        rom
    }

    #[test]
    fn chr_span_matches_tcsurfdesign_shaped_header() {
        let rom = tcsurfdesign_shaped_rom();
        assert_eq!(chr_span(&rom), Ok((0x8010, 0x10010)));
    }

    #[test]
    fn load_computes_generic_tile_and_bank_counts() {
        let rom = tcsurfdesign_shaped_rom();
        let mut editor = ChrEditor::new();
        editor.load(Some("nes"), Some(rom), Some(PathBuf::from("tcsurfdesign.nes")));
        assert_eq!(editor.status, Status::Ready);
        // Generic invariant, never hardcoded: tile_count == chr_len/16,
        // banks == ceil(tile_count/512).
        assert_eq!(editor.tile_count, editor.chr_len / BYTES_PER_2BPP_TILE);
        assert_eq!(editor.tile_count, 0x8000 / 16); // 2048 tiles for THIS cart
        assert_eq!(
            editor.bank_count,
            editor.tile_count.div_ceil(TILES_PER_BANK)
        );
        assert_eq!(editor.bank_count, 4); // 2048/512 for THIS cart
    }

    #[test]
    fn non_nes_core_shows_explicit_message_not_a_crash() {
        let mut editor = ChrEditor::new();
        editor.load(Some("megadrive"), Some(vec![1, 2, 3]), Some(PathBuf::from("mk2.bin")));
        assert_eq!(editor.status, Status::NotNes("megadrive".to_string()));
        assert!(editor.file_bytes.is_empty());
    }

    #[test]
    fn no_rom_shows_explicit_message() {
        let mut editor = ChrEditor::new();
        editor.load(None, None, None);
        assert_eq!(editor.status, Status::NoRom);
    }

    #[test]
    fn chr_ram_cart_shows_explicit_message_no_fabricated_tiles() {
        let mut rom = vec![0u8; 16];
        rom[0..4].copy_from_slice(&[0x4E, 0x45, 0x53, 0x1A]);
        rom[4] = 1; // PRG
        rom[5] = 0; // CHR-RAM
        rom.extend(std::iter::repeat(0u8).take(0x4000));
        let mut editor = ChrEditor::new();
        editor.load(Some("nes"), Some(rom), Some(PathBuf::from("ram.nes")));
        match &editor.status {
            Status::NoChr(msg) => assert!(msg.contains("CHR-RAM"), "msg: {msg}"),
            other => panic!("expected NoChr, got {other:?}"),
        }
        assert!(editor.tile_count == 0 && editor.chr_indices.is_empty());
    }

    // ── save reconstruction: header+PRG untouched, length invariant ────────

    #[test]
    fn save_reconstruction_leaves_prg_untouched_and_preserves_length() {
        let rom = tcsurfdesign_shaped_rom();
        let mut editor = ChrEditor::new();
        editor.load(Some("nes"), Some(rom.clone()), Some(PathBuf::from("tcsurfdesign.nes")));
        assert_eq!(editor.status, Status::Ready);

        let prg_before = editor.file_bytes[editor.prg_start..editor.prg_start + editor.prg_len].to_vec();

        // Patch tile 5's pixel (0,0) to index 1 (was 0b11 == 3 → CHR filled 0xCC).
        editor.draw_index = 1;
        editor.paint_pixel(5, 0, 0);
        assert!(editor.dirty_tiles.contains(&5));

        let out_path = std::env::temp_dir().join(format!(
            "chr_editor_test_{}.nes",
            std::process::id()
        ));
        let note = editor.do_save(&out_path, false);
        assert!(!note.starts_with("FAILED"), "save failed: {note}");

        let out = std::fs::read(&out_path).expect("save wrote a file");
        let _ = std::fs::remove_file(&out_path);

        // Length invariant: unchanged from the source file.
        assert_eq!(out.len(), rom.len());
        // Header untouched.
        assert_eq!(&out[0..16], &rom[0..16]);
        // PRG span untouched (byte-identical to the pre-edit slice).
        assert_eq!(&out[editor.prg_start..editor.prg_start + editor.prg_len], &prg_before[..]);
        // Exactly the edited tile's 16-byte span differs in CHR; everything
        // else in CHR is still 0xCC.
        let tile_off = editor.chr_start + 5 * BYTES_PER_2BPP_TILE;
        assert_ne!(&out[tile_off..tile_off + BYTES_PER_2BPP_TILE], &rom[tile_off..tile_off + BYTES_PER_2BPP_TILE]);
        let mut diffs = 0usize;
        for i in editor.chr_start..editor.chr_start + editor.chr_len {
            if out[i] != rom[i] {
                diffs += 1;
                assert!(i >= tile_off && i < tile_off + BYTES_PER_2BPP_TILE, "unexpected diff outside edited tile at {i}");
            }
        }
        assert!(diffs > 0, "expected at least one byte to differ in the edited tile span");
    }

    #[test]
    fn undo_restores_pre_edit_bytes_but_leaves_dirty_flag() {
        let rom = tcsurfdesign_shaped_rom();
        let mut editor = ChrEditor::new();
        editor.load(Some("nes"), Some(rom.clone()), Some(PathBuf::from("tcsurfdesign.nes")));

        let tile_off = editor.chr_start; // tile 0
        let before = editor.file_bytes[tile_off..tile_off + BYTES_PER_2BPP_TILE].to_vec();

        // CHR is filled with 0xCC in the fixture (index 3 for every pixel);
        // paint with a different index so the edit is a real change.
        editor.draw_index = 0;
        editor.paint_pixel(0, 0, 0);
        assert!(editor.dirty_tiles.contains(&0));
        assert_ne!(editor.file_bytes[tile_off..tile_off + BYTES_PER_2BPP_TILE], before[..]);

        editor.undo();
        assert_eq!(editor.file_bytes[tile_off..tile_off + BYTES_PER_2BPP_TILE], before[..]);
        // "touched this session" semantics: dirty bit is NOT cleared by undo.
        assert!(editor.dirty_tiles.contains(&0));
    }

    #[test]
    fn default_save_path_never_suggests_an_existing_file() {
        let dir = std::env::temp_dir().join(format!("chr_editor_defsave_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let rom_path = dir.join("game.nes");
        let edited = dir.join("game_edited.nes");
        std::fs::write(&edited, b"x").unwrap();

        let suggested = default_save_path(Some(&rom_path));
        assert_ne!(suggested, edited.to_string_lossy());
        assert!(suggested.ends_with("game_edited-2.nes"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
