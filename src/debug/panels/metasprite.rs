//! NES metasprite / OAM viewer — watch the character assemble from hardware
//! sprites, LIVE.
//!
//! This game (tcsurfdesign) draws its characters as METASPRITES: a cluster of
//! hardware sprites (OAM entries) that share a base 2×2 tile block and GROW
//! extra sprites for active poses (Big Wave's surfer is a 5-region, ~19-sprite
//! metasprite per `library/tcsurfdesign/tcsurfdesign.md`). This panel reads
//! LIVE OAM (256 bytes = 64 sprites × 4: Y, tile, attr, X) plus PPUREG and
//! PALRAM from `DebugState` every frame, decodes each sprite's tile from the
//! cart's CHR-ROM (the `.nes` FILE, via the same `chr_span` +
//! `decode_2bpp_planar_indices` path the CHR editor / nametable viewer / MCP
//! `rom_file:chr` source share — never a second decoder), and composes the
//! active sprites into a 256×240 view so you can literally watch the
//! metasprite assemble.
//!
//! CNROM caveat (this cart is mapper 3), inherited verbatim from the
//! nametable viewer: the ACTIVE CHR bank is a RUNTIME mapper-register fact NOT
//! derivable from the file, and this core exposes no pattern-table RAM to read
//! it back from — so the bank selector makes NO "this is the current bank"
//! claim. The operator picks the bank matching the screen. The sprite SIZE
//! (8×8 vs 8×16, PPUCTRL bit 5) and the 8×8 sprite pattern-table half (PPUCTRL
//! bit 3) ARE readable here — from the PPUREG region — so they are honored
//! live when it reads, and fall back to operator picks (labelled as such) when
//! it doesn't.
//!
//! Freeze + single-step: this panel drives the SAME emulation-control fields
//! the Disasm panel uses — `DebugState::paused` and `DebugState::step_one`
//! (see `src/debug/mod.rs`). It invents no new control path. With those wired,
//! true single-step IS available: Freeze pauses, Step ► runs exactly one
//! frame. A per-frame tile-diff highlight marks sprites whose tile index
//! changed since the last EMULATION frame (keyed on `frame_count`, not the GUI
//! frame), so animation is observable frame-to-frame whether you step or watch
//! it run.
//!
//! Honest degradation, cached once per ROM-identity change (no per-frame retry
//! storm): non-NES core, no ROM, or a CHR-RAM cart each render an explicit
//! message — never a fabricated composite. No readable OAM region (a non-NES
//! core) and zero active sprites ("this mode may draw the character in the
//! background", literally true for this game's Street Skate mode) are honest
//! runtime messages too. Missing PALRAM degrades to a grayscale structure
//! ramp with a visible label.

use bevy_egui::egui;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::debug::DebugState;
use crate::mcp::ines::chr_span;
use crate::mcp::snapshot::{
    decode_2bpp_planar_indices, gray_ramp_rgba, nes_master_rgba, read_region_bytes, TILE_PX,
    BYTES_PER_2BPP_TILE,
};

/// OAM is 64 sprites × 4 bytes.
const SPRITE_COUNT: usize = 64;
const OAM_BYTES: usize = SPRITE_COUNT * 4;
/// A sprite with Y >= this is parked off the bottom of the screen — the
/// idiomatic "hidden" value games write to unused OAM slots. Active = Y < it.
const ACTIVE_Y_MAX: u8 = 0xEF;

/// One CNROM CHR bank: 8KB = 512 tiles (two 256-tile pattern tables).
const CHR_BANK_BYTES: usize = 512 * BYTES_PER_2BPP_TILE;
/// Tiles per pattern-table half ($0000 / $1000).
const TILES_PER_PATTERN_TABLE: usize = 256;

/// The composed view is one PPU frame.
const SCREEN_W: usize = 256;
const SCREEN_H: usize = 240;

/// Backdrop the composite starts from — a neutral dark gray, deliberately NOT
/// a PPU color, so that "nothing drawn here" (sprite transparency) reads as
/// distinct from any real sprite pixel. Sprites paint over it; a sprite's
/// pixel value 0 does NOT (that is the transparency this panel exists to show).
const BACKDROP_RGBA: [u8; 4] = [28, 28, 34, 255];

/// One decoded OAM entry. Kept as the four raw bytes plus the sprite's index;
/// the attribute fields are decoded on demand by the accessor methods so the
/// decode lives in exactly one place.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Sprite {
    pub index: u8,
    pub y: u8,
    pub tile: u8,
    pub attr: u8,
    pub x: u8,
}

impl Sprite {
    /// Sprite sub-palette select: attribute bits 0-1 → one of the four sprite
    /// sub-palettes (PALRAM groups 4-7).
    pub(crate) fn palette(&self) -> u8 {
        self.attr & 0x03
    }
    /// Attribute bit 6: flip horizontally.
    pub(crate) fn h_flip(&self) -> bool {
        self.attr & 0x40 != 0
    }
    /// Attribute bit 7: flip vertically.
    pub(crate) fn v_flip(&self) -> bool {
        self.attr & 0x80 != 0
    }
    /// Attribute bit 5: priority — set means the sprite draws BEHIND opaque
    /// background pixels (0 = in front).
    pub(crate) fn behind_bg(&self) -> bool {
        self.attr & 0x20 != 0
    }
}

/// Decode the active sprites (Y < [`ACTIVE_Y_MAX`]) from a 256-byte OAM image,
/// preserving OAM index order. A short slice yields whatever full 4-byte
/// entries it contains — never a panic, never a fabricated sprite.
pub(crate) fn decode_active_sprites(oam: &[u8]) -> Vec<Sprite> {
    let mut out = Vec::new();
    for i in 0..SPRITE_COUNT {
        let base = i * 4;
        if base + 3 >= oam.len() {
            break;
        }
        let y = oam[base];
        if y >= ACTIVE_Y_MAX {
            continue;
        }
        out.push(Sprite {
            index: i as u8,
            y,
            tile: oam[base + 1],
            attr: oam[base + 2],
            x: oam[base + 3],
        });
    }
    out
}

/// Build a single-pose fragment of the sprite-swap contract from the CURRENT
/// live metasprite: the active sprites (positions normalized to the metasprite
/// bbox), the four sprite sub-palettes from live PALRAM (index 0 = transparent
/// backdrop, so only the 3 opaque colors are listed), plus sprite size and the
/// operator-picked CHR bank. Pure/serializable so it unit-tests without egui.
///
/// It is deliberately ONE pose — the full multi-pose contract (pose
/// enumeration across a ride, tile-sharing scan, bank inference) is the capture
/// agent's job; this is a one-click building block for it.
pub(crate) fn pose_contract_json(
    sprites: &[Sprite],
    palram: Option<&[u8; 32]>,
    size16: bool,
    bank: usize,
) -> String {
    let min_x = sprites.iter().map(|s| s.x).min().unwrap_or(0);
    let min_y = sprites.iter().map(|s| s.y).min().unwrap_or(0);
    let max_x = sprites.iter().map(|s| s.x as u16).max().unwrap_or(0);
    let max_y = sprites.iter().map(|s| s.y as u16).max().unwrap_or(0);
    let tile_h: u16 = if size16 { 16 } else { 8 };
    let bbox_w = (max_x + 8).saturating_sub(min_x as u16);
    let bbox_h = (max_y + tile_h).saturating_sub(min_y as u16);

    let hex = |i: u8| -> String {
        let [r, g, b, _] = nes_master_rgba(i);
        format!("#{r:02X}{g:02X}{b:02X}")
    };
    // Sprite sub-palettes live in PALRAM $3F10-$3F1F = bytes 16..32, four
    // groups of 4; index 0 of each is the shared backdrop (transparent).
    let subpalettes = palram.map(|p| {
        (4..8usize)
            .map(|g| {
                let base = g * 4; // 16,20,24,28
                format!(
                    "    \"{g}\": [\"{}\",\"{}\",\"{}\"]",
                    hex(p[base + 1]), hex(p[base + 2]), hex(p[base + 3])
                )
            })
            .collect::<Vec<_>>()
            .join(",\n")
    });

    let sprite_rows = sprites
        .iter()
        .map(|s| {
            format!(
                "      {{ \"slot\": {}, \"dx\": {}, \"dy\": {}, \"tile\": {}, \"attr\": {}, \
                 \"subpalette\": {}, \"flip_h\": {}, \"flip_v\": {}, \"behind_bg\": {} }}",
                s.index,
                s.x.saturating_sub(min_x),
                s.y.saturating_sub(min_y),
                s.tile,
                s.attr,
                4 + (s.attr & 0x3),
                s.h_flip(),
                s.v_flip(),
                s.behind_bg(),
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");

    let sub_block = match subpalettes {
        Some(s) => format!("{{\n{s}\n  }}"),
        None => "null (PALRAM unreadable — grayscale structure only)".to_string(),
    };
    format!(
        "{{\n  \"_note\": \"single live pose captured from the metasprite panel — one \
         fragment of sprite_contract.json, not the full multi-pose contract\",\n  \
         \"sprite_size\": \"{}\",\n  \"chr_bank_for_player\": {bank},\n  \
         \"bbox\": [{bbox_w}, {bbox_h}],\n  \"subpalettes\": {sub_block},\n  \
         \"sprites\": [\n{sprite_rows}\n  ]\n}}",
        if size16 { "8x16" } else { "8x8" }
    )
}

/// The color source for the composite: live PALRAM (real sprite colors) or the
/// structure-only gray ramp when PALRAM is unreadable.
enum SpriteColors {
    Palram([u8; 32]),
    GrayStructureOnly,
}

impl SpriteColors {
    /// RGBA for 2bpp pixel value `v` (0..=3) under sprite sub-palette `sub`
    /// (0..=3). Returns `None` for `v == 0`: for SPRITES, pixel value 0 is
    /// TRANSPARENT (it is NOT the universal backdrop the way a background
    /// pixel-0 is — sprites let the layer beneath show through). Sprite
    /// sub-palettes live in PALRAM groups 4-7, i.e. bytes 16..32.
    fn rgba(&self, v: u8, sub: u8) -> Option<[u8; 4]> {
        if v == 0 {
            return None;
        }
        match self {
            SpriteColors::Palram(p) => {
                let idx = p[16 + (sub as usize & 0x3) * 4 + v as usize];
                Some(nes_master_rgba(idx))
            }
            SpriteColors::GrayStructureOnly => Some(gray_ramp_rgba(v, 4)),
        }
    }
}

/// Blit one 8×8 CHR tile (`abs_tile` into the whole decoded CHR span) into the
/// `SCREEN_W × SCREEN_H` `canvas` at pixel `(dst_x, dst_y)`, honoring
/// horizontal/vertical flip, using sprite sub-palette `sub`. Pixel value 0 is
/// transparent (skipped), so the canvas beneath shows through. A tile index
/// outside the decoded CHR (short span) draws nothing — never garbage.
#[allow(clippy::too_many_arguments)]
fn blit_tile(
    canvas: &mut [egui::Color32],
    chr_indices: &[u8],
    tile_count: usize,
    abs_tile: usize,
    dst_x: usize,
    dst_y: usize,
    h_flip: bool,
    v_flip: bool,
    sub: u8,
    colors: &SpriteColors,
) {
    if abs_tile >= tile_count {
        return;
    }
    let src = &chr_indices[abs_tile * TILE_PX * TILE_PX..(abs_tile + 1) * TILE_PX * TILE_PX];
    for py in 0..TILE_PX {
        let sy = if v_flip { TILE_PX - 1 - py } else { py };
        let y = dst_y + py;
        if y >= SCREEN_H {
            continue;
        }
        for px in 0..TILE_PX {
            let sx = if h_flip { TILE_PX - 1 - px } else { px };
            let Some([r, g, b, a]) = colors.rgba(src[sy * TILE_PX + sx], sub) else {
                continue; // transparent sprite pixel — leave the canvas alone
            };
            let x = dst_x + px;
            if x >= SCREEN_W {
                continue;
            }
            canvas[y * SCREEN_W + x] = egui::Color32::from_rgba_premultiplied(r, g, b, a);
        }
    }
}

/// Compose the active sprites into a 256×240 RGBA image. `bank_base` is the
/// absolute tile index of the selected CHR bank's tile 0 (operator pick).
/// `size16` selects 8×16 sprites; for 8×8 sprites `pt8` (0 => $0000, 1 =>
/// $1000) selects the pattern-table half. Sprites are drawn HIGH-index-first
/// so OAM index 0 lands on top — the NES sprite-priority rule (lower OAM index
/// wins overlaps).
fn composite_sprites(
    sprites: &[Sprite],
    chr_indices: &[u8],
    bank_base: usize,
    size16: bool,
    pt8: usize,
    colors: &SpriteColors,
) -> egui::ColorImage {
    let mut pixels =
        vec![egui::Color32::from_rgba_premultiplied(BACKDROP_RGBA[0], BACKDROP_RGBA[1], BACKDROP_RGBA[2], BACKDROP_RGBA[3]); SCREEN_W * SCREEN_H];
    let tile_count = chr_indices.len() / (TILE_PX * TILE_PX);

    for s in sprites.iter().rev() {
        let sub = s.palette();
        let (dx, dy) = (s.x as usize, s.y as usize);
        if !size16 {
            let abs = bank_base + pt8 * TILES_PER_PATTERN_TABLE + s.tile as usize;
            blit_tile(&mut pixels, chr_indices, tile_count, abs, dx, dy, s.h_flip(), s.v_flip(), sub, colors);
        } else {
            // 8×16: pattern table is tile bit 0; top tile is tile & 0xFE, and
            // the two sub-tiles swap when flipped vertically.
            let pt = (s.tile & 0x01) as usize;
            let top = (s.tile & 0xFE) as usize;
            let (first, second) = if s.v_flip() { (top + 1, top) } else { (top, top + 1) };
            let abs_first = bank_base + pt * TILES_PER_PATTERN_TABLE + first;
            let abs_second = bank_base + pt * TILES_PER_PATTERN_TABLE + second;
            blit_tile(&mut pixels, chr_indices, tile_count, abs_first, dx, dy, s.h_flip(), s.v_flip(), sub, colors);
            blit_tile(&mut pixels, chr_indices, tile_count, abs_second, dx, dy + TILE_PX, s.h_flip(), s.v_flip(), sub, colors);
        }
    }

    egui::ColorImage {
        size: [SCREEN_W, SCREEN_H],
        source_size: egui::Vec2::new(SCREEN_W as f32, SCREEN_H as f32),
        pixels,
    }
}

/// Why the viewer cannot compose right now (ROM-side, cached per identity), or
/// that the CHR side is ready.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Status {
    NoRom,
    NotNes(String),
    NoChr(String),
    Ready,
}

/// Metasprite / OAM viewer debug panel state.
pub struct MetaspritePanel {
    /// ROM identity this CHR decode came from — same once-per-identity-change
    /// idiom as the nametable/CHR panels (no per-frame reload/retry storm).
    loaded_from: Option<PathBuf>,
    status: Status,

    /// Decoded palette indices for the whole CHR-ROM span, tile-major.
    chr_indices: Vec<u8>,
    bank_count: usize,

    /// Operator-picked CHR bank — a runtime mapper fact we cannot read back,
    /// so an explicit choice, never a claim.
    selected_bank: usize,
    /// Operator fallbacks used ONLY when PPUREG is unreadable. When PPUREG
    /// reads, sprite size + 8×8 pattern-table half come from PPUCTRL instead.
    op_size16: bool,
    op_pt8: usize,

    zoom: f32,

    /// Composed texture, rebuilt only when the input fingerprint (OAM + PALRAM
    /// + bank/size/pt choice) changes — live, but not a 61k-pixel re-upload on
    /// every static GUI frame.
    texture: Option<egui::TextureHandle>,
    fingerprint: u64,

    /// Per-frame tile-diff highlight: the tile byte last seen for each of the
    /// 64 OAM slots, the emulation `frame_count` that snapshot was taken at,
    /// and the set of slots whose tile changed on the most recent EMULATION
    /// frame (persisted for display across GUI frames within one emu frame).
    prev_tiles: [u8; SPRITE_COUNT],
    prev_frame_count: u64,
    changed: [bool; SPRITE_COUNT],

    /// Result of the last "export pose" click (path or error), shown inline.
    export_note: Option<String>,
}

impl MetaspritePanel {
    pub fn new() -> Self {
        MetaspritePanel {
            loaded_from: None,
            status: Status::NoRom,
            chr_indices: Vec::new(),
            bank_count: 0,
            selected_bank: 0,
            op_size16: false,
            op_pt8: 0,
            zoom: 2.0,
            texture: None,
            fingerprint: 0,
            prev_tiles: [0; SPRITE_COUNT],
            prev_frame_count: u64::MAX,
            changed: [false; SPRITE_COUNT],
            export_note: None,
        }
    }

    /// (Re)decode CHR from a fresh ROM snapshot. Called once per ROM-identity
    /// change — see `loaded_from`.
    fn load(&mut self, rom_system: Option<&str>, rom_bytes: Option<Vec<u8>>, rom_path: Option<&std::path::Path>) {
        self.chr_indices.clear();
        self.bank_count = 0;
        self.selected_bank = 0;
        self.texture = None;
        self.fingerprint = 0;

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
                    "No CHR-ROM in this file to render sprite tiles from: {e} (a CHR-RAM \
                     cart's live pattern tables are not exposed by this core)."
                ));
                return;
            }
        };

        self.chr_indices = decode_2bpp_planar_indices(&bytes[chr_start..chr_end]);
        self.bank_count = ((chr_end - chr_start) / CHR_BANK_BYTES).max(1);
        self.status = Status::Ready;
    }

    /// Absolute tile index of the selected bank's tile 0.
    fn bank_base(&self) -> usize {
        self.selected_bank * 512
    }

    /// Read the first `len` bytes of a named region (case-insensitive) from
    /// live DebugState. `None` when the region is absent OR unbacked OR shorter
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
        // One lock: ROM-identity check (+ clone only on change), the live PPU
        // region reads, the emulation-control flags, and frame_count for the
        // diff. Freeze/Step decisions are applied to the guard directly.
        let (oam, palram, ppureg, frame_count, paused);
        let mut want_step = false;
        let mut want_toggle_pause = false;
        {
            let ds = state.lock().unwrap();
            if ds.rom_path != self.loaded_from {
                let path = ds.rom_path.clone();
                self.load(ds.rom_system.as_deref(), ds.rom_bytes.clone(), path.as_deref());
                self.loaded_from = path;
            }
            oam = Self::read_named_region(&ds, "OAM", OAM_BYTES);
            palram = Self::read_named_region(&ds, "PALRAM", 32);
            ppureg = Self::read_named_region(&ds, "PPUREG", 4);
            frame_count = ds.frame_count;
            paused = ds.paused;
        }

        ui.heading("🐾 Sprites (OAM / metasprite)");
        ui.separator();

        match self.status.clone() {
            Status::NoRom => {
                ui.label("No ROM loaded.");
                return;
            }
            Status::NotNes(sys) => {
                ui.label(format!(
                    "Sprite viewer is NES-only (OAM/PPUREG/PALRAM PPU regions). \
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

        // ── Emulation control: Freeze + true single-step ──────────────────
        // Reuses DebugState::paused / step_one — the SAME fields the Disasm
        // panel drives. No new emulation-control path is invented here.
        ui.horizontal(|ui| {
            let freeze_label = if paused { "▶ Resume" } else { "⏸ Freeze" };
            if ui.button(freeze_label).clicked() {
                want_toggle_pause = true;
            }
            if ui
                .add_enabled(paused, egui::Button::new("⏭ Step 1 frame"))
                .on_hover_text("Runs exactly one emulation frame, then re-freezes — watch the metasprite change one frame at a time.")
                .clicked()
            {
                want_step = true;
            }
            if paused {
                ui.colored_label(egui::Color32::YELLOW, "⏸ FROZEN");
            }
            ui.separator();
            ui.label(format!("frame {frame_count}"));
        });

        if want_toggle_pause || want_step {
            let mut ds = state.lock().unwrap();
            if want_toggle_pause {
                ds.paused = !ds.paused;
            }
            if want_step {
                ds.step_one = true;
            }
        }

        let Some(oam) = oam else {
            ui.separator();
            ui.label(
                "OAM is not readable from this core right now (region absent or unbacked) — \
                 this is expected on a non-NES core. No fabricated sprites.",
            );
            return;
        };

        // Sprite size + 8×8 pattern-table half: honor live PPUCTRL when the
        // PPUREG region reads; otherwise fall back to operator picks.
        let ppuctrl = ppureg.as_ref().map(|r| r[0]);
        let (size16, pt8, ctrl_src) = match ppuctrl {
            Some(c) => ((c & 0x20) != 0, ((c >> 3) & 0x01) as usize, PpuCtrlSource::Live(c)),
            None => (self.op_size16, self.op_pt8, PpuCtrlSource::OperatorFallback),
        };

        let palram_arr: Option<[u8; 32]> = palram.as_ref().map(|p| {
            let mut a = [0u8; 32];
            a.copy_from_slice(&p[..32]);
            a
        });
        let colors = match palram {
            Some(p) => {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&p[..32]);
                SpriteColors::Palram(arr)
            }
            None => SpriteColors::GrayStructureOnly,
        };

        let sprites = decode_active_sprites(&oam);

        // ── Per-frame tile diff: recompute only when the EMULATION frame
        // advanced (GUI frames tick faster and would flicker false negatives).
        if frame_count != self.prev_frame_count {
            let mut cur = [0u8; SPRITE_COUNT];
            for i in 0..SPRITE_COUNT {
                cur[i] = oam[i * 4 + 1];
            }
            // First observation after (re)load leaves changed all-false.
            if self.prev_frame_count != u64::MAX {
                for i in 0..SPRITE_COUNT {
                    self.changed[i] = cur[i] != self.prev_tiles[i];
                }
            }
            self.prev_tiles = cur;
            self.prev_frame_count = frame_count;
        }

        // ── Controls ───────────────────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label("CHR bank (pick the one matching the screen):");
            for b in 0..self.bank_count {
                if ui.selectable_label(self.selected_bank == b, format!("{b}")).clicked() {
                    self.selected_bank = b;
                }
            }
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!sprites.is_empty(), egui::Button::new("⬇ Export pose JSON"))
                .on_hover_text(
                    "Write the current live metasprite as one pose fragment of \
                     sprite_contract.json (freeze first for a stable pose). One pose — \
                     the full multi-pose contract is the capture-agent's job.",
                )
                .clicked()
            {
                let json = pose_contract_json(&sprites, palram_arr.as_ref(), size16, self.selected_bank);
                let dir = std::path::Path::new("library/tcsurfdesign/assets/swap");
                let path = dir.join(format!("pose_export_f{frame_count}.json"));
                self.export_note = Some(match std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&path, json)) {
                    Ok(_) => format!("wrote {}", path.display()),
                    Err(e) => format!("export failed: {e}"),
                });
            }
            if let Some(note) = &self.export_note {
                let ok = note.starts_with("wrote");
                ui.colored_label(
                    if ok { egui::Color32::from_rgb(0x60, 0xC0, 0x60) } else { egui::Color32::from_rgb(0xE0, 0x60, 0x60) },
                    note,
                );
            }
        });
        ui.horizontal(|ui| {
            match ctrl_src {
                PpuCtrlSource::Live(c) => {
                    ui.label(format!(
                        "Sprite size: {} · 8×8 pattern table: {} (live PPUCTRL = ${:02X})",
                        if size16 { "8×16" } else { "8×8" },
                        if pt8 == 1 { "$1000" } else { "$0000" },
                        c,
                    ));
                }
                PpuCtrlSource::OperatorFallback => {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 140, 40),
                        "PPUREG unreadable — pick sprite size / pattern table:",
                    );
                    ui.checkbox(&mut self.op_size16, "8×16");
                    if !self.op_size16 {
                        ui.selectable_value(&mut self.op_pt8, 0, "$0000");
                        ui.selectable_value(&mut self.op_pt8, 1, "$1000");
                    }
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Zoom:");
            ui.add(egui::Slider::new(&mut self.zoom, 1.0..=4.0).step_by(0.5));
            ui.separator();
            match &colors {
                SpriteColors::Palram(_) => {
                    ui.label("Live PALRAM sprite colors (groups 4-7)");
                }
                SpriteColors::GrayStructureOnly => {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 140, 40),
                        "PALRAM unreadable — grayscale structure only, NOT real colors",
                    );
                }
            }
            ui.separator();
            ui.label(format!("{} active sprite(s)", sprites.len()));
        });
        ui.separator();

        if sprites.is_empty() {
            ui.label(
                "No sprites active (all 64 OAM slots parked at Y ≥ 0xEF) — this mode may draw \
                 the character in the BACKGROUND (nametable) rather than as sprites. For this \
                 game's Street Skate mode that is literally the case; try the 🗺 Nametables panel.",
            );
            return;
        }

        // ── Composite: rebuild texture only on input change ──────────────────
        let mut hasher = DefaultHasher::new();
        oam.hash(&mut hasher);
        if let SpriteColors::Palram(p) = &colors {
            p.hash(&mut hasher);
        } else {
            0u8.hash(&mut hasher);
        }
        self.bank_base().hash(&mut hasher);
        size16.hash(&mut hasher);
        pt8.hash(&mut hasher);
        let fp = hasher.finish();
        if self.texture.is_none() || self.fingerprint != fp {
            let image = composite_sprites(&sprites, &self.chr_indices, self.bank_base(), size16, pt8, &colors);
            self.texture = Some(ctx.load_texture("metasprite_composite", image, egui::TextureOptions::NEAREST));
            self.fingerprint = fp;
        }

        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                // Composite view.
                ui.vertical(|ui| {
                    ui.label("Composite (active sprites @ their X,Y)");
                    if let Some(tex) = &self.texture {
                        ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(
                            SCREEN_W as f32 * self.zoom,
                            SCREEN_H as f32 * self.zoom,
                        )));
                    }
                });
                ui.separator();
                // Sprite table.
                ui.vertical(|ui| {
                    self.show_sprite_table(ui, &sprites);
                });
            });
        });
    }

    fn show_sprite_table(&self, ui: &mut egui::Ui, sprites: &[Sprite]) {
        ui.label("Active sprites");
        ui.label(
            egui::RichText::new("amber row = tile index changed since last frame")
                .small()
                .color(egui::Color32::DARK_GRAY),
        );
        egui::Grid::new("metasprite_oam_table")
            .striped(true)
            .num_columns(7)
            .show(ui, |ui| {
                for h in ["#", "X", "Y", "tile", "pal", "flip", "prio"] {
                    ui.label(egui::RichText::new(h).strong());
                }
                ui.end_row();
                for s in sprites {
                    let changed = self.changed[s.index as usize];
                    let cell = |ui: &mut egui::Ui, text: String| {
                        if changed {
                            ui.colored_label(egui::Color32::from_rgb(220, 140, 40), text);
                        } else {
                            ui.monospace(text);
                        }
                    };
                    cell(ui, format!("{}", s.index));
                    cell(ui, format!("{}", s.x));
                    cell(ui, format!("{}", s.y));
                    cell(ui, format!("${:02X}", s.tile));
                    cell(ui, format!("{}", s.palette()));
                    let flip = match (s.h_flip(), s.v_flip()) {
                        (false, false) => "—",
                        (true, false) => "H",
                        (false, true) => "V",
                        (true, true) => "HV",
                    };
                    cell(ui, flip.to_string());
                    cell(ui, if s.behind_bg() { "behind".to_string() } else { "front".to_string() });
                    ui.end_row();
                }
            });
    }
}

/// Where the sprite size / pattern-table half came from this frame.
enum PpuCtrlSource {
    Live(u8),
    OperatorFallback,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pose_contract_json_normalizes_and_maps_subpalettes() {
        // Two sprites forming a 2x1 metasprite at X=104/112, Y=76.
        let sprites = vec![
            Sprite { index: 60, y: 76, tile: 0x08, attr: 0x03, x: 104 },
            Sprite { index: 61, y: 76, tile: 0x09, attr: 0x43, x: 112 }, // H-flip (bit6)
        ];
        let mut pal = [0u8; 32];
        pal[0] = 0x0F; // backdrop
        // sprite subpalette 3 = group 7 = bytes 28..32
        pal[29] = 0x16; pal[30] = 0x27; pal[31] = 0x30;
        let js = pose_contract_json(&sprites, Some(&pal), false, 2);
        // Positions normalized to bbox min (dx 0 and 8; dy 0).
        assert!(js.contains("\"dx\": 0"));
        assert!(js.contains("\"dx\": 8"));
        assert!(js.contains("\"sprite_size\": \"8x8\""));
        assert!(js.contains("\"chr_bank_for_player\": 2"));
        // attr 0x43 → subpalette group 4 + 3 = 7, H-flip true.
        assert!(js.contains("\"subpalette\": 7"));
        assert!(js.contains("\"flip_h\": true"));
        // group-7 opaque colors mapped through the master palette.
        assert!(js.contains("\"7\": [\"#"));
        // Parseable, and PALRAM-absent path is honest, not fabricated.
        let js2 = pose_contract_json(&sprites, None, true, 0);
        assert!(js2.contains("PALRAM unreadable"));
        assert!(js2.contains("\"sprite_size\": \"8x16\""));
    }

    // ── OAM decode: active-sprite list + attribute fields ─────────────────

    #[test]
    fn decode_active_sprites_filters_and_decodes_attrs() {
        // Park every slot off-screen (Y=0xFF), then activate a few — otherwise
        // the 60 untouched slots have Y=0, which is ACTIVE.
        let mut oam = [0xFFu8; OAM_BYTES];
        // Sprite 0: on-screen, tile 0x08, attr 0x03 (pal 3, no flip, front), X=100.
        oam[0] = 0x40;
        oam[1] = 0x08;
        oam[2] = 0x03;
        oam[3] = 100;
        // Sprite 1: parked off-screen at Y=0xEF exactly — INACTIVE (>= 0xEF).
        oam[4] = 0xEF;
        oam[5] = 0x11;
        oam[6] = 0x00;
        oam[7] = 0x00;
        // Sprite 2: on-screen, attr 0xE1 => pal 1, H+V flip, behind-bg priority.
        oam[8] = 0x50;
        oam[9] = 0x19;
        oam[10] = 0xE1; // bit7 V, bit6 H, bit5 prio, bits0-1 = 01
        oam[11] = 200;
        // Sprite 3: Y=0xEE is the last ACTIVE value (< 0xEF).
        oam[12] = 0xEE;
        oam[13] = 0x22;
        oam[14] = 0x00;
        oam[15] = 5;

        let sprites = decode_active_sprites(&oam);
        assert_eq!(sprites.len(), 3, "0xEF is inactive; 0xEE is active");
        assert_eq!(sprites[0].index, 0);
        assert_eq!(sprites[1].index, 2, "index order preserved, slot 1 skipped");
        assert_eq!(sprites[2].index, 3);

        // Attr decode on sprite 0.
        assert_eq!(sprites[0].palette(), 3);
        assert!(!sprites[0].h_flip() && !sprites[0].v_flip() && !sprites[0].behind_bg());
        assert_eq!(sprites[0].x, 100);
        assert_eq!(sprites[0].tile, 0x08);

        // Attr decode on sprite 2 (0xE1).
        assert_eq!(sprites[1].palette(), 1);
        assert!(sprites[1].h_flip() && sprites[1].v_flip() && sprites[1].behind_bg());
    }

    #[test]
    fn decode_active_sprites_all_parked_is_empty() {
        let oam = [0xF0u8; OAM_BYTES]; // every Y = 0xF0 (>= 0xEF)
        assert!(decode_active_sprites(&oam).is_empty());
    }

    // ── sprite transparency: pixel index 0 does NOT paint ─────────────────

    #[test]
    fn sprite_pixel_index_zero_is_transparent() {
        let mut palram = [0u8; 32];
        // Sprite sub-palette 0 (group 4): bytes 16..20. Give value-3 a real color.
        palram[16 + 0 * 4 + 3] = 0x21; // sky blue
        let colors = SpriteColors::Palram(palram);
        assert_eq!(colors.rgba(0, 0), None, "index 0 is transparent for sprites");
        assert_eq!(colors.rgba(3, 0), Some(nes_master_rgba(0x21)));
    }

    #[test]
    fn composite_leaves_backdrop_where_sprite_pixels_are_transparent() {
        // CHR: tile 0 has a single opaque pixel (value 3) at (0,0); the rest
        // are value 0 (transparent). Composited, only (0,0) should differ from
        // the backdrop.
        let mut chr = vec![0u8; TILE_PX * TILE_PX];
        chr[0] = 3;
        let mut palram = [0u8; 32];
        palram[16 + 3] = 0x20; // sub-pal 0, value 3 => white
        let colors = SpriteColors::Palram(palram);

        let sprite = Sprite { index: 0, y: 10, tile: 0, attr: 0x00, x: 20 };
        let img = composite_sprites(&[sprite], &chr, 0, false, 0, &colors);
        assert_eq!(img.size, [SCREEN_W, SCREEN_H]);

        let backdrop = egui::Color32::from_rgba_premultiplied(
            BACKDROP_RGBA[0], BACKDROP_RGBA[1], BACKDROP_RGBA[2], BACKDROP_RGBA[3],
        );
        let px = |x: usize, y: usize| img.pixels[y * SCREEN_W + x];

        // The one opaque pixel landed at (x=20, y=10) as white.
        let [r, g, b, a] = nes_master_rgba(0x20);
        assert_eq!(px(20, 10), egui::Color32::from_rgba_premultiplied(r, g, b, a));
        // Its neighbor (transparent pixel value 0) is still backdrop.
        assert_eq!(px(21, 10), backdrop);
        assert_eq!(px(20, 11), backdrop);
        // Far away: backdrop.
        assert_eq!(px(0, 0), backdrop);
    }

    #[test]
    fn composite_honors_horizontal_flip() {
        // A tile opaque only in its left column; flipped, it must appear in the
        // sprite's RIGHT column.
        let mut chr = vec![0u8; TILE_PX * TILE_PX];
        for row in 0..TILE_PX {
            chr[row * TILE_PX] = 3; // column 0 of every row
        }
        let mut palram = [0u8; 32];
        palram[16 + 3] = 0x20;
        let colors = SpriteColors::Palram(palram);
        let backdrop = egui::Color32::from_rgba_premultiplied(
            BACKDROP_RGBA[0], BACKDROP_RGBA[1], BACKDROP_RGBA[2], BACKDROP_RGBA[3],
        );

        let s = Sprite { index: 0, y: 0, tile: 0, attr: 0x40, x: 0 }; // H-flip
        let img = composite_sprites(&[s], &chr, 0, false, 0, &colors);
        let px = |x: usize, y: usize| img.pixels[y * SCREEN_W + x];
        // Column 7 opaque, column 0 backdrop after the flip.
        let [r, g, b, a] = nes_master_rgba(0x20);
        assert_eq!(px(7, 0), egui::Color32::from_rgba_premultiplied(r, g, b, a));
        assert_eq!(px(0, 0), backdrop);
    }

    #[test]
    fn composite_out_of_chr_tile_stays_backdrop() {
        // One tile of CHR only; a sprite asks for tile 200 -> draws nothing.
        let chr = vec![3u8; TILE_PX * TILE_PX];
        let mut palram = [0u8; 32];
        palram[16 + 3] = 0x21;
        let colors = SpriteColors::Palram(palram);
        let s = Sprite { index: 0, y: 0, tile: 200, attr: 0, x: 0 };
        let img = composite_sprites(&[s], &chr, 0, false, 0, &colors);
        let backdrop = egui::Color32::from_rgba_premultiplied(
            BACKDROP_RGBA[0], BACKDROP_RGBA[1], BACKDROP_RGBA[2], BACKDROP_RGBA[3],
        );
        assert_eq!(img.pixels[0], backdrop);
    }

    // ── panel-level status honesty (same idioms as the sibling panels) ────

    #[test]
    fn non_nes_core_shows_explicit_status() {
        let mut p = MetaspritePanel::new();
        p.load(Some("megadrive"), Some(vec![1, 2, 3]), Some(std::path::Path::new("mk2.bin")));
        assert_eq!(p.status, Status::NotNes("megadrive".to_string()));
        assert!(p.chr_indices.is_empty());
    }

    #[test]
    fn no_rom_shows_explicit_status() {
        let mut p = MetaspritePanel::new();
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
        let mut p = MetaspritePanel::new();
        p.load(Some("nes"), Some(rom), Some(std::path::Path::new("ram.nes")));
        match &p.status {
            Status::NoChr(msg) => assert!(msg.contains("CHR-RAM"), "msg: {msg}"),
            other => panic!("expected NoChr, got {other:?}"),
        }
    }

    #[test]
    fn cnrom_shaped_rom_computes_bank_count_and_base() {
        // Mapper 3, PRG=32KiB, CHR=32KiB — the tcsurfdesign shape: 4 banks.
        let mut rom = vec![0u8; 16];
        rom[0..4].copy_from_slice(&[0x4E, 0x45, 0x53, 0x1A]);
        rom[4] = 2;
        rom[5] = 4;
        rom[6] = 0x30;
        rom.extend(std::iter::repeat(0xAA).take(0x8000));
        rom.extend(std::iter::repeat(0xCC).take(0x8000));
        let mut p = MetaspritePanel::new();
        p.load(Some("nes"), Some(rom), Some(std::path::Path::new("tcsurfdesign.nes")));
        assert_eq!(p.status, Status::Ready);
        assert_eq!(p.bank_count, 0x8000 / CHR_BANK_BYTES);
        assert_eq!(p.bank_count, 4);
        p.selected_bank = 2;
        assert_eq!(p.bank_base(), 2 * 512);
    }
}
