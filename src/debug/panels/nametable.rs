//! NES nametable viewer — renders the PPU's background layout LIVE.
//!
//! The fceumm core (post the PPU-region read fix) exposes `NTARAM` (2KB of
//! nametable bytes: two 1KB tables under vertical mirroring) and `PALRAM`
//! (32 bytes of palette RAM, background palette in the first 16) as readable
//! memory regions. This panel reads BOTH live from `DebugState` every frame,
//! decodes each nametable cell's tile from the cart's CHR-ROM (the `.nes`
//! FILE, via the same `chr_span` + `decode_2bpp_planar_indices` path the CHR
//! editor and MCP `rom_file:chr` source share — never a second decoder), maps
//! pixels through the attribute table + PALRAM + an embedded NES master
//! palette, and shows the composed 256×240 background.
//!
//! CNROM caveat (this cart is mapper 3): the ACTIVE CHR bank is a runtime
//! mapper-register fact that is NOT derivable from the file, and fceumm
//! exposes no pattern-table RAM to read it back from. The bank selector here
//! therefore makes NO "this is the current bank" claim — the operator picks
//! the bank (and BG pattern-table half, PPUCTRL bit 4 being equally
//! unreadable) that matches the screen. Same honesty rule as the CHR editor's
//! module doc.
//!
//! Honest degradation, cached once per ROM-identity change (no per-frame
//! retry storm): non-NES core, no ROM, CHR-RAM cart, or no readable NTARAM
//! each render an explicit message — never a fabricated grid. Missing PALRAM
//! degrades to the structure-only gray ramp with a visible label.

use bevy_egui::egui;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::debug::DebugState;
use crate::mcp::ines::chr_span;
use crate::mcp::snapshot::{
    decode_2bpp_planar_indices, gray_ramp_rgba, read_region_bytes, NES_MASTER_PALETTE,
    BYTES_PER_2BPP_TILE, TILE_PX,
};

/// One nametable is 32×30 tiles (256×240 px); its last 64 bytes are the
/// attribute table.
const NT_COLS: usize = 32;
const NT_ROWS: usize = 30;
const NT_BYTES: usize = 1024;
const ATTR_OFFSET: usize = NT_COLS * NT_ROWS; // 960
const NT_PX_W: usize = NT_COLS * TILE_PX; // 256
const NT_PX_H: usize = NT_ROWS * TILE_PX; // 240

/// One CNROM CHR bank: 8KB = 512 tiles (two 256-tile pattern tables).
const CHR_BANK_BYTES: usize = 512 * BYTES_PER_2BPP_TILE;
/// Tiles per pattern-table half ($0000 / $1000).
const TILES_PER_PATTERN_TABLE: usize = 256;

/// Map a PALRAM byte (a NES master-palette index) to RGB, via the single
/// shared `NES_MASTER_PALETTE` in `mcp::snapshot`. PALRAM bytes are always
/// < 0x40 per the PPU contract; the mask guards a garbage read.
fn nes_rgb(index: u8) -> [u8; 3] {
    NES_MASTER_PALETTE[(index & 0x3F) as usize]
}

/// Select the 2-bit background sub-palette for tile `(tx, ty)` from a 64-byte
/// attribute table. Each attribute byte covers a 32×32 px (4×4 tile) block;
/// its four 2-bit fields select the sub-palette for the block's 16×16 px
/// quadrants: bits 0-1 top-left, 2-3 top-right, 4-5 bottom-left, 6-7
/// bottom-right.
pub(crate) fn attr_palette_select(attrs: &[u8], tx: usize, ty: usize) -> u8 {
    debug_assert!(attrs.len() >= 64 && tx < NT_COLS && ty < NT_ROWS);
    let byte = attrs[(ty / 4) * 8 + (tx / 4)];
    // (ty & 2) picks the bottom half (shift +4), (tx & 2) the right half (+2).
    let shift = ((ty & 2) << 1) | (tx & 2);
    (byte >> shift) & 0x3
}

/// The color source for composing: live PALRAM (real colors) or the
/// structure-only gray ramp when PALRAM is unreadable.
enum ColorSource {
    Palram([u8; 32]),
    GrayStructureOnly,
}

impl ColorSource {
    /// RGBA for 2bpp pixel value `v` (0..=3) under background sub-palette
    /// `pal` (0..=3). Pixel value 0 is always the universal backdrop ($3F00),
    /// per the PPU's palette-mirroring rule.
    fn rgba(&self, v: u8, pal: u8) -> [u8; 4] {
        match self {
            ColorSource::Palram(palram) => {
                let idx = if v == 0 {
                    palram[0]
                } else {
                    palram[(pal as usize) * 4 + v as usize]
                };
                let [r, g, b] = nes_rgb(idx);
                [r, g, b, 255]
            }
            ColorSource::GrayStructureOnly => gray_ramp_rgba(v, 4),
        }
    }
}

/// Compose one nametable (>= 1024 bytes at `nt`) into a 256×240 RGBA image.
/// `chr_indices` is the whole decoded CHR span (64 palette indices per tile);
/// `tile_base` is the absolute tile index of the selected bank + pattern
/// table's tile 0. A cell whose tile lies outside the decoded CHR (short CHR
/// span) renders as backdrop — an edge case no 8KB-banked cart hits.
fn render_nametable_image(
    nt: &[u8],
    chr_indices: &[u8],
    tile_base: usize,
    colors: &ColorSource,
) -> egui::ColorImage {
    debug_assert!(nt.len() >= NT_BYTES);
    let attrs = &nt[ATTR_OFFSET..NT_BYTES];
    let backdrop = colors.rgba(0, 0);
    let mut pixels =
        vec![egui::Color32::from_rgba_premultiplied(backdrop[0], backdrop[1], backdrop[2], backdrop[3]); NT_PX_W * NT_PX_H];

    let tile_count = chr_indices.len() / (TILE_PX * TILE_PX);
    for ty in 0..NT_ROWS {
        for tx in 0..NT_COLS {
            let tile = tile_base + nt[ty * NT_COLS + tx] as usize;
            if tile >= tile_count {
                continue; // out of decoded CHR: stays backdrop, never fabricated
            }
            let pal = attr_palette_select(attrs, tx, ty);
            let src = &chr_indices[tile * TILE_PX * TILE_PX..(tile + 1) * TILE_PX * TILE_PX];
            for py in 0..TILE_PX {
                let row = (ty * TILE_PX + py) * NT_PX_W + tx * TILE_PX;
                for px in 0..TILE_PX {
                    let [r, g, b, a] = colors.rgba(src[py * TILE_PX + px], pal);
                    pixels[row + px] = egui::Color32::from_rgba_premultiplied(r, g, b, a);
                }
            }
        }
    }

    egui::ColorImage {
        size: [NT_PX_W, NT_PX_H],
        source_size: egui::Vec2::new(NT_PX_W as f32, NT_PX_H as f32),
        pixels,
    }
}

/// Why the viewer cannot compose right now (ROM-side, cached per identity),
/// or that the CHR side is ready.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Status {
    NoRom,
    NotNes(String),
    NoChr(String),
    Ready,
}

/// Which nametable(s) to show. NTARAM holds two 1KB tables; this cart uses
/// vertical mirroring, so they are the left/right scroll halves.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Nt0,
    Nt1,
    Both,
}

/// Nametable viewer debug panel state.
pub struct NametablePanel {
    /// ROM identity this CHR decode came from — same once-per-identity-change
    /// idiom as the CHR editor's `loaded_from` (no per-frame reload/retry).
    loaded_from: Option<PathBuf>,
    status: Status,

    /// Decoded palette indices for the whole CHR-ROM span, tile-major.
    chr_indices: Vec<u8>,
    bank_count: usize,

    /// Operator-picked CHR bank / BG pattern table — runtime mapper facts we
    /// cannot read back, so these are explicit choices, never claims.
    selected_bank: usize,
    pattern_table: usize, // 0 => $0000, 1 => $1000

    view: View,
    zoom: f32,

    /// Composed textures for NT0/NT1, rebuilt only when the input fingerprint
    /// (NTARAM + PALRAM + bank/pattern choice) changes — live, but not a
    /// 61k-pixel re-upload on every static frame.
    textures: [Option<egui::TextureHandle>; 2],
    fingerprints: [u64; 2],
}

impl NametablePanel {
    pub fn new() -> Self {
        NametablePanel {
            loaded_from: None,
            status: Status::NoRom,
            chr_indices: Vec::new(),
            bank_count: 0,
            selected_bank: 0,
            pattern_table: 0,
            view: View::Nt0,
            zoom: 2.0,
            textures: [None, None],
            fingerprints: [0, 0],
        }
    }

    /// (Re)decode CHR from a fresh ROM snapshot. Called once per ROM-identity
    /// change — see `loaded_from`.
    fn load(&mut self, rom_system: Option<&str>, rom_bytes: Option<Vec<u8>>, rom_path: Option<&std::path::Path>) {
        self.chr_indices.clear();
        self.bank_count = 0;
        self.selected_bank = 0;
        self.textures = [None, None];
        self.fingerprints = [0, 0];

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
        // (same fallback the CHR editor and mcp::server use).
        let bytes = match rom_bytes.filter(|b| !b.is_empty()) {
            Some(b) => b,
            None => match rom_path.and_then(|p| std::fs::read(p).ok()) {
                Some(b) => b,
                None => {
                    self.status = Status::NoRom;
                    return;
                }
            },
        };

        let (chr_start, chr_end) = match chr_span(&bytes) {
            Ok(span) => span,
            Err(e) => {
                self.status = Status::NoChr(format!(
                    "No CHR-ROM in this file to render tiles from: {e} (a CHR-RAM cart's \
                     live pattern tables are not exposed by this core)."
                ));
                return;
            }
        };

        self.chr_indices = decode_2bpp_planar_indices(&bytes[chr_start..chr_end]);
        self.bank_count = ((chr_end - chr_start) / CHR_BANK_BYTES).max(1);
        self.status = Status::Ready;
    }

    /// Absolute tile index of the selected bank+pattern-table's tile 0.
    fn tile_base(&self) -> usize {
        self.selected_bank * 512 + self.pattern_table * TILES_PER_PATTERN_TABLE
    }

    /// Read the first `len` bytes of a named region from live DebugState.
    /// `None` when the region is absent OR declared-but-unbacked OR shorter
    /// than `len` — callers treat all three as "not readable", honestly.
    fn read_named_region(ds: &DebugState, name: &str, len: usize) -> Option<Vec<u8>> {
        let region = ds.memory_regions.iter().find(|r| r.name.eq_ignore_ascii_case(name))?;
        let bytes = read_region_bytes(region, 0, len)?;
        if bytes.len() < len {
            return None;
        }
        Some(bytes)
    }

    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, state: &Arc<Mutex<DebugState>>) {
        // One lock: ROM identity check (+ clone only on change) and the live
        // PPU region reads (2KB + 32B copies — cheap).
        let (ntaram, palram) = {
            let ds = state.lock().unwrap();
            if ds.rom_path != self.loaded_from {
                let path = ds.rom_path.clone();
                self.load(ds.rom_system.as_deref(), ds.rom_bytes.clone(), path.as_deref());
                self.loaded_from = path;
            }
            (
                Self::read_named_region(&ds, "NTARAM", 2 * NT_BYTES),
                Self::read_named_region(&ds, "PALRAM", 32),
            )
        };

        ui.heading("🗺 Nametables");
        ui.separator();

        match self.status.clone() {
            Status::NoRom => {
                ui.label("No ROM loaded.");
                return;
            }
            Status::NotNes(sys) => {
                ui.label(format!(
                    "Nametable viewer is NES-only (NTARAM/PALRAM PPU regions). \
                     This core/cart is `{sys}`."
                ));
                return;
            }
            Status::NoChr(reason) => {
                ui.label(reason);
                return;
            }
            Status::Ready => {}
        }

        let Some(ntaram) = ntaram else {
            ui.label(
                "NTARAM is not readable from this core right now (region absent or \
                 unbacked) — nothing live to render. No fabricated grid.",
            );
            return;
        };

        let colors = match palram {
            Some(p) => {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&p[..32]);
                ColorSource::Palram(arr)
            }
            None => ColorSource::GrayStructureOnly,
        };

        // ── Controls ─────────────────────────────────────────────────────
        ui.horizontal(|ui| {
            // A CNROM bank register is a RUNTIME fact the file cannot tell us
            // and fceumm exposes no pattern-table RAM — so this is a pick, not
            // a claim. Keep the label honest.
            ui.label("CHR bank (pick the one matching the screen):");
            for b in 0..self.bank_count {
                if ui.selectable_label(self.selected_bank == b, format!("{b}")).clicked() {
                    self.selected_bank = b;
                }
            }
            ui.separator();
            ui.label("BG pattern table:")
                .on_hover_text("PPUCTRL bit 4 is also a runtime fact — pick the half matching the screen.");
            for pt in 0..2usize {
                let label = if pt == 0 { "$0000" } else { "$1000" };
                if ui.selectable_label(self.pattern_table == pt, label).clicked() {
                    self.pattern_table = pt;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Nametable:");
            for (v, label) in [(View::Nt0, "NT 0"), (View::Nt1, "NT 1"), (View::Both, "Both")] {
                if ui.selectable_label(self.view == v, label).clicked() {
                    self.view = v;
                }
            }
            ui.separator();
            ui.label("Zoom:");
            ui.add(egui::Slider::new(&mut self.zoom, 1.0..=4.0).step_by(0.5));
            ui.separator();
            match colors {
                ColorSource::Palram(_) => {
                    ui.label("Live PALRAM colors");
                }
                ColorSource::GrayStructureOnly => {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 140, 40),
                        "PALRAM unreadable — grayscale structure only, NOT real colors",
                    );
                }
            }
        });
        ui.separator();

        // ── Compose + draw ───────────────────────────────────────────────
        let wanted: &[usize] = match self.view {
            View::Nt0 => &[0],
            View::Nt1 => &[1],
            View::Both => &[0, 1],
        };
        for &nt in wanted {
            let slice = &ntaram[nt * NT_BYTES..(nt + 1) * NT_BYTES];
            let mut hasher = DefaultHasher::new();
            slice.hash(&mut hasher);
            if let ColorSource::Palram(p) = &colors {
                p.hash(&mut hasher);
            } else {
                0u8.hash(&mut hasher);
            }
            self.tile_base().hash(&mut hasher);
            let fp = hasher.finish();
            if self.textures[nt].is_none() || self.fingerprints[nt] != fp {
                let image = render_nametable_image(slice, &self.chr_indices, self.tile_base(), &colors);
                self.textures[nt] = Some(ctx.load_texture(
                    format!("nametable_{nt}"),
                    image,
                    egui::TextureOptions::NEAREST,
                ));
                self.fingerprints[nt] = fp;
            }
        }

        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for &nt in wanted {
                    if let Some(tex) = &self.textures[nt] {
                        ui.vertical(|ui| {
                            ui.label(format!("NT {nt} (${:04X})", 0x2000 + nt * 0x400));
                            ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(
                                NT_PX_W as f32 * self.zoom,
                                NT_PX_H as f32 * self.zoom,
                            )));
                        });
                    }
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── attribute-table decode ───────────────────────────────────────────

    #[test]
    fn attr_palette_select_picks_the_right_quadrant() {
        // Attribute byte 0b_11_10_01_00: TL=0, TR=1, BL=2, BR=3. Put it in
        // attr slot 0 (tiles 0..4 × 0..4) and a different byte in slot 1
        // (tiles 4..8) to prove the byte addressing too.
        let mut attrs = [0u8; 64];
        attrs[0] = 0b11_10_01_00;
        attrs[1] = 0b00_01_10_11; // TL=3, TR=2, BL=1, BR=0
        // Bottom-left block of the screen: attr row 7 covers tile rows 28-29
        // (only the TOP quadrants exist on a 30-row screen).
        attrs[7 * 8] = 0b11_10_01_00;

        // Slot 0, all four quadrants (16×16 px = 2×2 tiles each).
        assert_eq!(attr_palette_select(&attrs, 0, 0), 0); // top-left
        assert_eq!(attr_palette_select(&attrs, 1, 1), 0); // still top-left
        assert_eq!(attr_palette_select(&attrs, 2, 0), 1); // top-right
        assert_eq!(attr_palette_select(&attrs, 3, 1), 1);
        assert_eq!(attr_palette_select(&attrs, 0, 2), 2); // bottom-left
        assert_eq!(attr_palette_select(&attrs, 1, 3), 2);
        assert_eq!(attr_palette_select(&attrs, 2, 2), 3); // bottom-right
        assert_eq!(attr_palette_select(&attrs, 3, 3), 3);

        // Slot 1 (tiles 4..8, same rows).
        assert_eq!(attr_palette_select(&attrs, 4, 0), 3);
        assert_eq!(attr_palette_select(&attrs, 6, 0), 2);
        assert_eq!(attr_palette_select(&attrs, 4, 2), 1);
        assert_eq!(attr_palette_select(&attrs, 6, 2), 0);

        // Last attribute row: tile rows 28/29 are that row's TOP quadrants.
        assert_eq!(attr_palette_select(&attrs, 0, 28), 0);
        assert_eq!(attr_palette_select(&attrs, 2, 29), 1);
    }

    #[test]
    fn attr_palette_select_zero_attrs_is_palette_zero_everywhere() {
        let attrs = [0u8; 64];
        for ty in 0..NT_ROWS {
            for tx in 0..NT_COLS {
                assert_eq!(attr_palette_select(&attrs, tx, ty), 0);
            }
        }
    }

    // ── master palette ───────────────────────────────────────────────────

    #[test]
    fn master_palette_has_64_entries_and_known_anchors() {
        assert_eq!(NES_MASTER_PALETTE.len(), 64);
        // Canonical anchors of the NesDev NTSC table.
        assert_eq!(nes_rgb(0x00), [84, 84, 84]); // dark gray
        assert_eq!(nes_rgb(0x0F), [0, 0, 0]); // canonical black
        assert_eq!(nes_rgb(0x20), [236, 238, 236]); // white
        assert_eq!(nes_rgb(0x21), [76, 154, 236]); // sky blue
        // Out-of-contract byte (>= 0x40) wraps via the &0x3F guard instead of
        // panicking — PALRAM should never hold one, but a garbage read might.
        assert_eq!(nes_rgb(0x40), nes_rgb(0x00));
        assert_eq!(nes_rgb(0xFF), nes_rgb(0x3F));
    }

    // ── full compose: tiles + attrs + PALRAM → pixels ────────────────────

    #[test]
    fn render_nametable_composes_attr_palram_and_chr() {
        // CHR: tile 0 = all pixel-value 0, tile 1 = all pixel-value 3.
        let mut chr_indices = vec![0u8; 2 * TILE_PX * TILE_PX];
        for p in &mut chr_indices[TILE_PX * TILE_PX..] {
            *p = 3;
        }

        // Nametable: tile 1 at cell (0,0), tile 0 at (2,0); attrs put
        // sub-palette 2 in the top-left quadrant, 1 in the top-right quadrant
        // of attribute block 0.
        let mut nt = vec![0u8; NT_BYTES];
        nt[0] = 1; // cell (0,0) -> tile 1
        nt[2] = 1; // cell (2,0) -> tile 1, other quadrant
        nt[ATTR_OFFSET] = 0b00_00_01_10; // TL=2, TR=1

        // PALRAM: backdrop $0F (black); sub-pal 1 color 3 = $21 (sky blue);
        // sub-pal 2 color 3 = $20 (white).
        let mut palram = [0u8; 32];
        palram[0] = 0x0F;
        palram[1 * 4 + 3] = 0x21;
        palram[2 * 4 + 3] = 0x20;
        let colors = ColorSource::Palram(palram);

        let img = render_nametable_image(&nt, &chr_indices, 0, &colors);
        assert_eq!(img.size, [NT_PX_W, NT_PX_H]);

        let px = |x: usize, y: usize| img.pixels[y * NT_PX_W + x];
        let rgb = |c: egui::Color32| [c.r(), c.g(), c.b()];

        // Cell (0,0): tile 1 (value 3) under sub-palette 2 -> $20 white.
        assert_eq!(rgb(px(0, 0)), nes_rgb(0x20));
        assert_eq!(rgb(px(7, 7)), nes_rgb(0x20));
        // Cell (2,0): tile 1 under sub-palette 1 -> $21 sky blue.
        assert_eq!(rgb(px(16, 0)), nes_rgb(0x21));
        // Cell (1,0): tile 0 (value 0) -> universal backdrop $0F, regardless
        // of the sub-palette its quadrant selects.
        assert_eq!(rgb(px(8, 0)), nes_rgb(0x0F));
        // Far corner: tile 0, backdrop.
        assert_eq!(rgb(px(255, 239)), nes_rgb(0x0F));
    }

    #[test]
    fn render_nametable_out_of_chr_tile_stays_backdrop() {
        // One tile of CHR only; the nametable asks for tile 200 -> backdrop,
        // never a fabricated/garbage tile.
        let chr_indices = vec![3u8; TILE_PX * TILE_PX];
        let mut nt = vec![0u8; NT_BYTES];
        nt[0] = 200;
        let mut palram = [0u8; 32];
        palram[0] = 0x21;
        let img = render_nametable_image(&nt, &chr_indices, 0, &ColorSource::Palram(palram));
        let c = img.pixels[0];
        assert_eq!([c.r(), c.g(), c.b()], nes_rgb(0x21));
    }

    #[test]
    fn gray_fallback_uses_structure_ramp_not_colors() {
        let chr_indices = vec![3u8; TILE_PX * TILE_PX];
        let nt = vec![0u8; NT_BYTES]; // tile 0 everywhere, attrs 0
        let img = render_nametable_image(&nt, &chr_indices, 0, &ColorSource::GrayStructureOnly);
        // Pixel value 3 on the 4-level gray ramp is white.
        let c = img.pixels[0];
        assert_eq!([c.r(), c.g(), c.b()], [255, 255, 255]);
    }

    // ── panel-level status honesty (same idioms as the CHR editor) ───────

    #[test]
    fn non_nes_core_shows_explicit_status() {
        let mut p = NametablePanel::new();
        p.load(Some("megadrive"), Some(vec![1, 2, 3]), Some(std::path::Path::new("mk2.bin")));
        assert_eq!(p.status, Status::NotNes("megadrive".to_string()));
        assert!(p.chr_indices.is_empty());
    }

    #[test]
    fn no_rom_shows_explicit_status() {
        let mut p = NametablePanel::new();
        p.load(None, None, None);
        assert_eq!(p.status, Status::NoRom);
    }

    #[test]
    fn chr_ram_cart_shows_explicit_status() {
        let mut rom = vec![0u8; 16];
        rom[0..4].copy_from_slice(&[0x4E, 0x45, 0x53, 0x1A]);
        rom[4] = 1; // PRG
        rom[5] = 0; // CHR-RAM
        rom.extend(std::iter::repeat(0u8).take(0x4000));
        let mut p = NametablePanel::new();
        p.load(Some("nes"), Some(rom), Some(std::path::Path::new("ram.nes")));
        match &p.status {
            Status::NoChr(msg) => assert!(msg.contains("CHR-RAM"), "msg: {msg}"),
            other => panic!("expected NoChr, got {other:?}"),
        }
    }

    #[test]
    fn cnrom_shaped_rom_computes_bank_count_from_chr_len() {
        // Mapper 3, PRG=32KiB, CHR=32KiB — the tcsurfdesign shape: 4 banks.
        let mut rom = vec![0u8; 16];
        rom[0..4].copy_from_slice(&[0x4E, 0x45, 0x53, 0x1A]);
        rom[4] = 2;
        rom[5] = 4;
        rom[6] = 0x30;
        rom.extend(std::iter::repeat(0xAA).take(0x8000));
        rom.extend(std::iter::repeat(0xCC).take(0x8000));
        let mut p = NametablePanel::new();
        p.load(Some("nes"), Some(rom), Some(std::path::Path::new("tcsurfdesign.nes")));
        assert_eq!(p.status, Status::Ready);
        // Generic invariant: bank_count == chr_len / 8KB, never hardcoded.
        assert_eq!(p.bank_count, 0x8000 / CHR_BANK_BYTES);
        assert_eq!(p.bank_count, 4); // for THIS cart shape
        // tile_base: bank 2, pattern table $1000 -> 2*512 + 256.
        p.selected_bank = 2;
        p.pattern_table = 1;
        assert_eq!(p.tile_base(), 1280);
    }
}
