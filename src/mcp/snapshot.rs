//! Pure, serialization-friendly projections of [`DebugState`] for the MCP layer.
//!
//! These types and functions are deliberately free of any `rmcp`/`tokio`
//! dependency so they can be unit-tested directly (see the `#[cfg(test)]` block)
//! and reused by both the resource and tool code paths.
//!
//! The cardinal rule: **never serialize raw `DebugState`**. It owns large byte
//! buffers (`fb_rgba`, `framebuffer`, `m68k_code_bytes`) and host pointers that
//! must not be dumped into a JSON payload. We map it into a compact
//! [`AiSnapshot`] under a brief lock instead.

use serde::Serialize;

use crate::debug::{DebugState, MemoryRegion};

/// One memory-region summary line: metadata only, never the bytes.
#[derive(Serialize, Clone)]
pub struct RegionSummary {
    pub name: String,
    /// "ROM" / "RAM" / "VRAM" / "SRAM" / "Unmapped".
    pub kind: String,
    pub addr_start: usize,
    pub addr_end: usize,
    pub size: usize,
    pub readonly: bool,
}

/// Counts of the various accumulating debug collections (so the AI can decide
/// whether it's worth fetching the full `app://watches` etc.).
#[derive(Serialize, Clone)]
pub struct SnapshotCounts {
    pub watches: usize,
    pub breakpoints: usize,
    pub code_regions: usize,
    pub bookmarks: usize,
    pub change_log: usize,
    pub heatmap_entries: usize,
}

/// M68K register file projection.
#[derive(Serialize, Clone)]
pub struct M68kRegs {
    pub d: [u32; 8],
    pub a: [u32; 8],
    pub pc: u32,
    pub sr: u32,
}

/// Z80 register projection (the subset the core currently exposes).
#[derive(Serialize, Clone)]
pub struct Z80Regs {
    pub pc: u16,
    pub bc: u16,
    pub de: u16,
    pub hl: u16,
}

/// A compact, JSON-safe snapshot of the live app state. This is what the
/// `app://state` resource and the `get_state` tool return.
#[derive(Serialize, Clone)]
pub struct AiSnapshot {
    pub frame_count: u64,
    pub fps: f64,
    pub av_width: u32,
    pub av_height: u32,
    pub fb_width: u32,
    pub fb_height: u32,
    /// libretro pixel format id: 0=0RGB1555, 1=XRGB8888, 2=RGB565.
    pub fb_fmt: u32,
    pub paused: bool,
    pub m68k: M68kRegs,
    pub z80: Z80Regs,
    pub regions: Vec<RegionSummary>,
    /// Up-front summary of what the agent can actually read on this core.
    pub capability: MemoryCapability,
    pub counts: SnapshotCounts,
    /// The shared navigation cursor's current address, if any.
    pub nav_address: Option<u32>,
}

impl AiSnapshot {
    /// Map a locked [`DebugState`] into a snapshot. Cheap: copies a handful of
    /// scalars and the (small) region metadata list — never the framebuffers.
    pub fn from_debug_state(ds: &DebugState) -> Self {
        let regions = ds
            .memory_regions
            .iter()
            .map(|r| RegionSummary {
                name: r.name.clone(),
                kind: r.region_type().to_string(),
                addr_start: r.addr_start,
                addr_end: r.addr_end,
                size: r.size,
                readonly: r.is_readonly(),
            })
            .collect();

        AiSnapshot {
            frame_count: ds.frame_count,
            fps: ds.fps,
            av_width: ds.av_width,
            av_height: ds.av_height,
            fb_width: ds.fb_width,
            fb_height: ds.fb_height,
            fb_fmt: ds.fb_fmt,
            paused: ds.paused,
            m68k: M68kRegs {
                d: ds.m68k_d_regs,
                a: ds.m68k_a_regs,
                pc: ds.m68k_pc,
                sr: ds.m68k_sr,
            },
            z80: Z80Regs {
                pc: ds.z80_pc,
                bc: ds.z80_bc,
                de: ds.z80_de,
                hl: ds.z80_hl,
            },
            regions,
            capability: memory_capability(ds),
            counts: SnapshotCounts {
                watches: ds.watches.len(),
                breakpoints: ds.breakpoints.len(),
                code_regions: ds.code_regions.len(),
                bookmarks: ds.bookmarks.len(),
                change_log: ds.change_log.len(),
                heatmap_entries: ds.pc_heatmap.len(),
            },
            nav_address: ds.nav.current_address,
        }
    }
}

/// A full memory-map entry for the `app://memory-map` resource. Same data as
/// [`RegionSummary`] but named for orientation use; kept distinct so the map
/// resource can evolve independently of the state-snapshot summary.
#[derive(Serialize, Clone)]
pub struct MemoryMapEntry {
    pub name: String,
    /// "ROM" / "RAM" / "VRAM" / "SRAM" / "Unmapped".
    pub kind: String,
    pub addr_start: usize,
    pub addr_end: usize,
    pub size: usize,
    pub readonly: bool,
}

/// Build the memory map: one [`MemoryMapEntry`] per mapped region, in order.
pub fn memory_map(ds: &DebugState) -> Vec<MemoryMapEntry> {
    ds.memory_regions
        .iter()
        .map(|r| MemoryMapEntry {
            name: r.name.clone(),
            kind: r.region_type().to_string(),
            addr_start: r.addr_start,
            addr_end: r.addr_end,
            size: r.size,
            readonly: r.is_readonly(),
        })
        .collect()
}

/// An up-front, honest summary of what an AI agent can actually *do* with this
/// core's memory — so it never confuses "no memory map" with "all zeros", and
/// never assumes a declared region is readable when it's a virtual/unbacked
/// descriptor.
///
/// Computed purely from `&DebugState` (no locking, no host-pointer deref beyond
/// the bounds-checked `safe_host_ptr` probe), so it is unit-testable.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct MemoryCapability {
    /// Total number of declared regions (backed OR virtual/unbacked).
    pub region_count: usize,
    /// Distinct `region_type()`s present: "RAM"/"ROM"/"VRAM"/"SRAM"/"Unmapped".
    pub kinds: Vec<String>,
    /// Sum of sizes of regions that are actually backed by readable host memory
    /// (i.e. `safe_host_ptr(addr_start, 1)` is `Some`). Virtual/unbacked
    /// descriptors contribute 0 — this is how the agent tells real memory from a
    /// declared-but-garbage descriptor.
    pub total_readable_bytes: usize,
    pub has_system_ram: bool,
    pub has_vram: bool,
    pub has_rom: bool,
    /// "core memory map" / "get_memory_data fallback" / "none".
    pub source: String,
    /// One honest, agent-facing line describing what's reachable.
    pub note: String,
}

/// Probe whether a region is backed by readable host memory. A virtual/unbacked
/// descriptor (null/garbage `ptr`, zero `size`) returns `false` without ever
/// dereferencing the pointer.
fn region_is_backed(region: &MemoryRegion) -> bool {
    region.safe_host_ptr(region.addr_start, 1).is_some()
}

/// Build the [`MemoryCapability`] summary for the current debug state.
///
/// Pure: only calls `region_type()` / `safe_host_ptr()` (a bounds check, not a
/// deref), so it is safe even when the map contains virtual descriptors.
pub fn memory_capability(ds: &DebugState) -> MemoryCapability {
    let regions = &ds.memory_regions;
    let region_count = regions.len();

    let mut kinds: Vec<String> = Vec::new();
    for r in regions {
        let k = r.region_type().to_string();
        if !kinds.contains(&k) {
            kinds.push(k);
        }
    }

    let total_readable_bytes: usize = regions
        .iter()
        .filter(|r| region_is_backed(r))
        .map(|r| r.size)
        .sum();

    let has_system_ram = regions.iter().any(|r| r.region_type() == "RAM");
    let has_vram = regions.iter().any(|r| r.region_type() == "VRAM");
    let has_rom = regions.iter().any(|r| r.region_type() == "ROM");

    // Source: "none" if empty; "get_memory_data fallback" if every region looks
    // synthesized (name contains "(fallback)"); otherwise "core memory map".
    let source = if region_count == 0 {
        "none".to_string()
    } else if regions.iter().all(|r| r.name.contains("(fallback)")) {
        "get_memory_data fallback".to_string()
    } else {
        "core memory map".to_string()
    };

    // A one-line, honest hint tailored to the situation.
    let note = if region_count == 0 {
        "No memory map: only the framebuffer (app://screen) and execution control \
         are available; byte-level reads return nothing."
            .to_string()
    } else if total_readable_bytes == 0 {
        "Regions are declared but none are backed by readable host memory \
         (virtual/unbacked descriptors); byte-level reads return nothing."
            .to_string()
    } else if has_system_ram && !has_vram && !has_rom {
        "Work RAM only (no VRAM/ROM): sprite/ROM provenance unavailable on this core."
            .to_string()
    } else if !has_system_ram && (has_vram || has_rom) {
        "VRAM/ROM available but no system work RAM exposed; game-state reads may be \
         limited."
            .to_string()
    } else if source == "get_memory_data fallback" {
        "Synthesized fallback map (core published no descriptors): flat readable \
         block(s) only; offset/select layout is approximate."
            .to_string()
    } else {
        "Core memory map present: named regions are readable for byte-level inspection \
         and content search."
            .to_string()
    };

    MemoryCapability {
        region_count,
        kinds,
        total_readable_bytes,
        has_system_ram,
        has_vram,
        has_rom,
        source,
        note,
    }
}

/// Clone up to `len` bytes from a region starting at byte `offset` within the
/// region, via the bounds-checked `safe_host_ptr` path.
///
/// Returns `None` when the region is unbacked (a virtual/garbage descriptor) or
/// `offset` lies past the region — so callers can treat "declared but not
/// readable" distinctly from "read some bytes". `len` is clamped to the region
/// size; the returned vec may be shorter than `len` if a hole is hit mid-range.
/// NEVER does `from_raw_parts(region.ptr, ..)` blindly — that would segfault on
/// a virtual descriptor.
pub fn read_region_bytes(region: &MemoryRegion, offset: usize, len: usize) -> Option<Vec<u8>> {
    // Unbacked / virtual descriptor: refuse without dereferencing.
    if !region_is_backed(region) {
        return None;
    }
    // Clamp the readable window to the region's declared size.
    let region_size = region
        .size
        .min(region.addr_end.saturating_sub(region.addr_start) + 1);
    if offset >= region_size {
        return None;
    }
    let avail = region_size - offset;
    let want = len.min(avail);
    let start = region.addr_start + offset;

    let mut out = Vec::with_capacity(want);
    for i in 0..want {
        // Per-byte bounds-checked read. A mid-range hole (e.g. a select/disconnect
        // gap) stops the copy at what we safely have.
        match region.safe_host_ptr(start + i, 1) {
            Some(p) => out.push(unsafe { *p }),
            None => break,
        }
    }
    Some(out)
}

/// Find every occurrence of `needle` in `haystack`, returning the BYTE OFFSETS
/// (relative to the start of `haystack`) of each match, capped at `max_hits`.
///
/// Pure and allocation-light so it can be unit-tested with synthetic buffers and
/// run UNLOCKED by the MCP layer (which clones region bytes out under a brief
/// lock, then scans here without holding the mutex). Returns an empty vec if the
/// needle is empty or longer than the haystack.
pub fn search_bytes(haystack: &[u8], needle: &[u8], max_hits: usize) -> Vec<usize> {
    let mut hits = Vec::new();
    if needle.is_empty() || needle.len() > haystack.len() || max_hits == 0 {
        return hits;
    }
    let last = haystack.len() - needle.len();
    let first = needle[0];
    let mut i = 0;
    while i <= last {
        if haystack[i] == first && &haystack[i..i + needle.len()] == needle {
            hits.push(i);
            if hits.len() >= max_hits {
                break;
            }
        }
        i += 1;
    }
    hits
}

/// Parse a whitespace-or-comma-tolerant hex byte string (e.g. "DE AD BE EF",
/// "deadbeef", "DE,AD,BE,EF") into a byte vector. Returns `None` on any
/// non-hex-digit or an odd number of nibbles.
pub fn parse_hex_bytes(s: &str) -> Option<Vec<u8>> {
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ',' && *c != '_')
        .collect();
    if cleaned.is_empty() || cleaned.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    let bytes = cleaned.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    Some(out)
}

/// One row of the PC heatmap, sorted hottest-first by the caller.
#[derive(Serialize, Clone)]
pub struct HeatmapEntry {
    pub pc: u32,
    pub hits: u64,
}

/// Return the top-`n` hottest PCs from the heatmap, sorted by hit count
/// descending (ties broken by ascending address for determinism).
pub fn top_heatmap(ds: &DebugState, n: usize) -> Vec<HeatmapEntry> {
    let mut v: Vec<HeatmapEntry> = ds
        .pc_heatmap
        .iter()
        .map(|(&pc, &hits)| HeatmapEntry { pc, hits })
        .collect();
    v.sort_by(|a, b| b.hits.cmp(&a.hits).then(a.pc.cmp(&b.pc)));
    v.truncate(n);
    v
}

/// Encode an RGBA8888 buffer (`width`×`height`, 4 bytes/pixel, row-major,
/// top-down) to PNG bytes. Returns `None` if the buffer length doesn't match
/// the dimensions or encoding fails. Pure — no locking, no globals — so it can
/// be unit-tested with a tiny synthetic buffer.
pub fn rgba_to_png(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let expected = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
    if width == 0 || height == 0 || rgba.len() != expected {
        return None;
    }
    let img = image::RgbaImage::from_raw(width, height, rgba.to_vec())?;
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).ok()?;
    Some(buf.into_inner())
}

// ── tile decoders (the image-recognition / RE evidence stream) ───────────────
//
// These decode a span of raw ROM/VRAM bytes AS tiles into an RGBA grid so a
// Claude agent can SEE them (via `render_tiles` → `rgba_to_png` → a PNG image
// content) and VISUALLY compare a candidate ROM region to what's on screen.
// This is the convergent-evidence complement to `vram_to_rom`: that one is a
// raw byte-content match (which fails on compressed / re-bitplaned graphics);
// this one sidesteps the byte layout by rendering pixels for vision to judge.
//
// Every tile is 8×8 px. The decoders are PURE (bytes in, palette-index/RGBA
// out) so they unit-test against hand-computed tile bytes.

/// Side length, in pixels, of one tile. All supported formats use 8×8 tiles.
pub const TILE_PX: usize = 8;
/// Bytes per NES/2bpp planar tile (2 bitplanes × 8 bytes).
pub const BYTES_PER_2BPP_TILE: usize = 16;
/// Bytes per Genesis/4bpp planar tile (4 bitplanes × 8 bytes).
pub const BYTES_PER_4BPP_TILE: usize = 32;

/// Map a palette index in `0..levels` to an evenly-spaced gray level (0=black,
/// max=white), so structure is visible WITHOUT knowing the real palette. With
/// `levels == 4` (2bpp) the ramp is 0/85/170/255; with 16 (4bpp) it is the 16
/// evenly-spaced steps. Returned as an opaque RGBA quad.
fn gray_ramp_rgba(index: u8, levels: u8) -> [u8; 4] {
    let levels = levels.max(2);
    let max = (levels - 1) as u32;
    let v = ((index.min(levels - 1) as u32) * 255 / max) as u8;
    [v, v, v, 255]
}

/// Decode 2bpp PLANAR tiles in the NES CHR layout into a flat vector of palette
/// indices (0..=3), tile-major then row-major within each tile (8×8 indices per
/// tile, in screen order). Each 16-byte tile is two bitplanes: bytes 0..8 are
/// plane 0 (the low bit) and bytes 8..16 are plane 1 (the high bit); row `y`'s
/// pixel `x` takes bit `(7 - x)` from `plane0[y]` and `plane1[y]`. A trailing
/// partial tile (fewer than 16 bytes) is ignored.
///
/// PURE and unit-testable: feed known tile bytes, assert the indices.
pub fn decode_2bpp_planar_indices(bytes: &[u8]) -> Vec<u8> {
    let tiles = bytes.len() / BYTES_PER_2BPP_TILE;
    let mut out = Vec::with_capacity(tiles * TILE_PX * TILE_PX);
    for t in 0..tiles {
        let base = t * BYTES_PER_2BPP_TILE;
        for y in 0..TILE_PX {
            let p0 = bytes[base + y];
            let p1 = bytes[base + TILE_PX + y];
            for x in 0..TILE_PX {
                let bit = 7 - x;
                let lo = (p0 >> bit) & 1;
                let hi = (p1 >> bit) & 1;
                out.push((hi << 1) | lo);
            }
        }
    }
    out
}

/// Decode 4bpp PLANAR tiles (Genesis VDP layout) into palette indices (0..=15),
/// tile-major then row-major within each tile. Each 32-byte tile is FOUR
/// bitplanes of 8 bytes each (planes 0..3, in order); row `y`'s pixel `x` takes
/// bit `(7 - x)` from each of `plane0[y]..plane3[y]`, assembled LSB-first
/// (plane0 = bit 0 … plane3 = bit 3). A trailing partial tile is ignored.
///
/// NOTE: this is the GENERIC 4bpp planar / Genesis target. CPS2 stores 4bpp
/// graphics with a different plane interleave, so this may not match CPS2 tiles
/// exactly; use it for Genesis and treat CPS2 output as approximate.
pub fn decode_4bpp_planar_indices(bytes: &[u8]) -> Vec<u8> {
    let tiles = bytes.len() / BYTES_PER_4BPP_TILE;
    let mut out = Vec::with_capacity(tiles * TILE_PX * TILE_PX);
    for t in 0..tiles {
        let base = t * BYTES_PER_4BPP_TILE;
        for y in 0..TILE_PX {
            let planes = [
                bytes[base + y],
                bytes[base + TILE_PX + y],
                bytes[base + 2 * TILE_PX + y],
                bytes[base + 3 * TILE_PX + y],
            ];
            for x in 0..TILE_PX {
                let bit = 7 - x;
                let mut idx = 0u8;
                for (plane_no, p) in planes.iter().enumerate() {
                    idx |= ((p >> bit) & 1) << plane_no;
                }
                out.push(idx);
            }
        }
    }
    out
}

/// The pixel format `render_tiles` understands. Carries the bytes/tile and the
/// number of palette levels for the grayscale ramp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileFormat {
    /// NES CHR / 2bpp planar, 16 bytes/tile, 4 levels.
    Nes2bpp,
    /// Genesis / generic 4bpp planar, 32 bytes/tile, 16 levels.
    Genesis4bpp,
}

impl TileFormat {
    /// Parse a user-facing format string. Accepts the system names and the
    /// bit-depth aliases. Returns `None` for an unknown format (callers reject
    /// with the valid list).
    pub fn parse(s: &str) -> Option<TileFormat> {
        match s.trim().to_ascii_lowercase().as_str() {
            "2bpp" | "nes" | "nes_chr" | "neschr" | "chr" => Some(TileFormat::Nes2bpp),
            "4bpp" | "genesis" | "megadrive" | "md" => Some(TileFormat::Genesis4bpp),
            _ => None,
        }
    }

    /// The comma-separated list of accepted format strings, for error messages.
    pub fn valid_list() -> &'static str {
        "2bpp|nes_chr, 4bpp|genesis"
    }

    fn bytes_per_tile(self) -> usize {
        match self {
            TileFormat::Nes2bpp => BYTES_PER_2BPP_TILE,
            TileFormat::Genesis4bpp => BYTES_PER_4BPP_TILE,
        }
    }

    fn levels(self) -> u8 {
        match self {
            TileFormat::Nes2bpp => 4,
            TileFormat::Genesis4bpp => 16,
        }
    }

    fn decode_indices(self, bytes: &[u8]) -> Vec<u8> {
        match self {
            TileFormat::Nes2bpp => decode_2bpp_planar_indices(bytes),
            TileFormat::Genesis4bpp => decode_4bpp_planar_indices(bytes),
        }
    }
}

/// The result of decoding a byte span into a tile-grid image: a row-major
/// top-down RGBA8888 buffer plus its pixel dimensions and the tile count.
pub struct TileImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub tile_count: usize,
}

/// Decode `bytes` as `format` tiles, laid out `tiles_per_row` tiles wide, into
/// an RGBA grid using the default grayscale ramp (since the real palette is
/// usually unknown). The final row is padded with black tiles so the buffer is
/// a clean rectangle. Returns `None` if there isn't even one complete tile or
/// `tiles_per_row == 0`.
///
/// PURE — no locking, no globals — so the grid layout is unit-testable.
pub fn decode_tiles_to_rgba(
    bytes: &[u8],
    format: TileFormat,
    tiles_per_row: usize,
) -> Option<TileImage> {
    if tiles_per_row == 0 {
        return None;
    }
    let tile_count = bytes.len() / format.bytes_per_tile();
    if tile_count == 0 {
        return None;
    }
    let indices = format.decode_indices(bytes); // tile-major, 64 indices/tile
    let levels = format.levels();

    let cols = tiles_per_row;
    let rows = tile_count.div_ceil(cols);
    let width = cols * TILE_PX;
    let height = rows * TILE_PX;

    // Allocate the full grid as black; fill in the tiles we have.
    let mut rgba = vec![0u8; width * height * 4];
    for t in 0..tile_count {
        let tile_col = t % cols;
        let tile_row = t / cols;
        let px0 = tile_col * TILE_PX; // top-left pixel of this tile
        let py0 = tile_row * TILE_PX;
        for ty in 0..TILE_PX {
            for tx in 0..TILE_PX {
                let idx = indices[t * TILE_PX * TILE_PX + ty * TILE_PX + tx];
                let [r, g, b, a] = gray_ramp_rgba(idx, levels);
                let dst = ((py0 + ty) * width + (px0 + tx)) * 4;
                rgba[dst] = r;
                rgba[dst + 1] = g;
                rgba[dst + 2] = b;
                rgba[dst + 3] = a;
            }
        }
    }

    Some(TileImage {
        rgba,
        width: width as u32,
        height: height as u32,
        tile_count,
    })
}

// ── major-region discovery (the "structure" / statistical-signature stream) ──
//
// This is the convergent-evidence stream that lets a Claude agent ORIENT inside
// an unknown ROM before zeroing in: scan the bytes with cheap statistical
// signatures (Shannon entropy, byte-histogram features, printable/padding
// fractions) and propose what KIND each span looks like — packed/compressed,
// code, graphics, a low-entropy table, text, or padding.
//
// These are HEURISTICS, deliberately coarse and honestly labelled `guess`/
// `likely`. No single signature is authoritative; the agent corroborates a
// proposal with the other streams (`render_tiles` to eyeball graphics,
// `vram_to_rom` for content match, the PC heatmap for code) — convergence, not
// any one method, is what promotes a finding to `confirmed`. Everything here is
// PURE (bytes in, stats/labels out) so it unit-tests against synthetic buffers.

/// Shannon entropy of a byte buffer in BITS PER BYTE (range `0.0..=8.0`): 0 for a
/// constant buffer, ~8 for uniformly-random/compressed data. Empty input → 0.
pub fn shannon_entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for &b in bytes {
        counts[b as usize] += 1;
    }
    let n = bytes.len() as f64;
    let mut h = 0.0;
    for &c in counts.iter() {
        if c != 0 {
            let p = c as f64 / n;
            h -= p * p.log2();
        }
    }
    h
}

/// Cheap statistical fingerprint of one window of bytes. All fractions are in
/// `0.0..=1.0`. Pure and `Serialize` so it rides along in the tool's JSON output
/// for the agent to inspect the raw evidence behind a classification.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct WindowStats {
    /// Shannon entropy, bits/byte (0..8).
    pub entropy: f64,
    /// Fraction of bytes == 0x00 (padding / background fill).
    pub zero_frac: f64,
    /// Fraction of bytes == 0xFF (the other common fill value).
    pub ff_frac: f64,
    /// Fraction of bytes that are printable ASCII (0x20..=0x7E, plus \t\n\r) —
    /// the text-table signal.
    pub printable_frac: f64,
    /// The single most-common byte value in the window.
    pub top_byte: u8,
    /// Fraction the most-common byte accounts for (histogram spikiness).
    pub top_byte_frac: f64,
    /// Count of distinct byte values present (1..=256). Flat histograms (gfx /
    /// packed) trend high; spiky ones (code / sparse tables) trend low.
    pub distinct_bytes: u16,
}

/// Compute the [`WindowStats`] fingerprint of a byte window. Empty input yields
/// an all-zero fingerprint (entropy 0, no distinct bytes).
pub fn window_stats(bytes: &[u8]) -> WindowStats {
    if bytes.is_empty() {
        return WindowStats {
            entropy: 0.0,
            zero_frac: 0.0,
            ff_frac: 0.0,
            printable_frac: 0.0,
            top_byte: 0,
            top_byte_frac: 0.0,
            distinct_bytes: 0,
        };
    }
    let mut counts = [0u32; 256];
    let mut printable = 0u32;
    for &b in bytes {
        counts[b as usize] += 1;
        if (0x20..=0x7E).contains(&b) || b == b'\t' || b == b'\n' || b == b'\r' {
            printable += 1;
        }
    }
    let n = bytes.len() as f64;
    let mut top_byte = 0u8;
    let mut top_count = 0u32;
    let mut distinct = 0u16;
    for (v, &c) in counts.iter().enumerate() {
        if c != 0 {
            distinct += 1;
            if c > top_count {
                top_count = c;
                top_byte = v as u8;
            }
        }
    }
    WindowStats {
        entropy: shannon_entropy(bytes),
        zero_frac: counts[0x00] as f64 / n,
        ff_frac: counts[0xFF] as f64 / n,
        printable_frac: printable as f64 / n,
        top_byte,
        top_byte_frac: top_count as f64 / n,
        distinct_bytes: distinct,
    }
}

/// A proposed kind for a window, with how much to trust it and the one-line
/// reasoning. `kind` is drawn from a small controlled set the agent can act on:
/// `padding`, `text_table`, `packed_data`, `lookup_table`, `graphics`, `code`.
/// `confidence` mirrors the ROM-map vocabulary (`likely` | `guess`).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct WindowClass {
    pub kind: &'static str,
    pub confidence: &'static str,
    pub reason: String,
}

/// Classify one window from its [`WindowStats`]. A waterfall of cheap, documented
/// heuristics — checked most-specific first:
///
/// 1. **padding** — ≥97% a single fill value (0x00 or 0xFF). Highest confidence.
/// 2. **text_table** — ≥80% printable ASCII.
/// 3. **packed_data** — entropy ≥ 7.3 (compressed / encrypted / packed graphics
///    or audio; near-uniform histogram). A `guess`: high entropy alone can't say
///    *what* was packed.
/// 4. **lookup_table** — low entropy (< 3.5) that ISN'T pure padding: a sparse,
///    structured table (pointer lists, small data records).
/// 5. **graphics vs. code** — the mid-entropy band, the genuinely ambiguous one.
///    Uncompressed tile/bitplane graphics carry large stretches of background
///    fill, so a meaningful zero fraction (≥ 0.20) with a flat-ish histogram
///    leans **graphics**; otherwise the spiky-opcode shape leans **code**. Both
///    are `guess` — this split is exactly what `render_tiles` / the PC heatmap
///    exist to corroborate.
pub fn classify_window(s: &WindowStats) -> WindowClass {
    let mk = |kind, confidence, reason: String| WindowClass {
        kind,
        confidence,
        reason,
    };
    if s.zero_frac >= 0.97 || s.ff_frac >= 0.97 {
        let fill = if s.zero_frac >= s.ff_frac { "0x00" } else { "0xFF" };
        return mk(
            "padding",
            "likely",
            format!("{:.0}% {fill} fill", s.zero_frac.max(s.ff_frac) * 100.0),
        );
    }
    if s.printable_frac >= 0.80 {
        return mk(
            "text_table",
            "likely",
            format!("{:.0}% printable ASCII", s.printable_frac * 100.0),
        );
    }
    if s.entropy >= 7.3 {
        return mk(
            "packed_data",
            "guess",
            format!(
                "entropy {:.2} b/byte (compressed/packed — could be gfx or audio)",
                s.entropy
            ),
        );
    }
    if s.entropy < 3.5 {
        return mk(
            "lookup_table",
            "guess",
            format!(
                "low entropy {:.2} b/byte, top byte 0x{:02X} = {:.0}% (structured table)",
                s.entropy,
                s.top_byte,
                s.top_byte_frac * 100.0
            ),
        );
    }
    // Mid-entropy band: the graphics-vs-code coin-flip. Background fill is the
    // most reliable cheap discriminator we have here.
    if s.zero_frac >= 0.20 {
        mk(
            "graphics",
            "guess",
            format!(
                "mid entropy {:.2} b/byte, {:.0}% background (0x00) — tile/bitplane shape",
                s.entropy,
                s.zero_frac * 100.0
            ),
        )
    } else {
        mk(
            "code",
            "guess",
            format!(
                "mid entropy {:.2} b/byte, spiky histogram (top byte {:.0}%) — opcode shape",
                s.entropy,
                s.top_byte_frac * 100.0
            ),
        )
    }
}

/// One proposed major region: a run of adjacent windows that classified the same
/// way, coalesced into a single span. Offsets are RELATIVE to the start of the
/// scanned buffer; the MCP layer adds the region base to report absolute guest
/// addresses.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct RegionCandidate {
    pub kind: &'static str,
    pub confidence: &'static str,
    /// Byte offset of the span start within the scanned buffer.
    pub start: usize,
    /// Exclusive byte offset of the span end within the scanned buffer.
    pub end: usize,
    pub len: usize,
    /// How many windows were coalesced into this span.
    pub windows: usize,
    /// Mean entropy across the span's windows (bits/byte).
    pub mean_entropy: f64,
    /// Reasoning from the first window of the span (representative).
    pub reason: String,
}

/// Scan `bytes` window-by-window, classify each window, and coalesce adjacent
/// same-kind windows into [`RegionCandidate`] spans — so a 512 KB run of packed
/// data reports as ONE candidate, not 128 windows. `window` is clamped to a sane
/// floor (256 B) so tiny windows don't make entropy meaningless. A trailing
/// partial window (< `window` bytes) is still classified so the tail isn't lost.
///
/// PURE — no locking, no globals — so the whole scan/coalesce is unit-testable.
pub fn scan_buffer(bytes: &[u8], window: usize) -> Vec<RegionCandidate> {
    let window = window.max(256);
    let mut out: Vec<RegionCandidate> = Vec::new();
    if bytes.is_empty() {
        return out;
    }
    let mut off = 0usize;
    while off < bytes.len() {
        let end = (off + window).min(bytes.len());
        let stats = window_stats(&bytes[off..end]);
        let class = classify_window(&stats);
        match out.last_mut() {
            // Extend the open span if this window is the same kind.
            Some(last) if last.kind == class.kind => {
                last.end = end;
                last.len = end - last.start;
                last.windows += 1;
                last.mean_entropy += stats.entropy;
            }
            _ => out.push(RegionCandidate {
                kind: class.kind,
                confidence: class.confidence,
                start: off,
                end,
                len: end - off,
                windows: 1,
                mean_entropy: stats.entropy,
                reason: class.reason,
            }),
        }
        off = end;
    }
    // Convert the accumulated entropy sums into means.
    for c in out.iter_mut() {
        c.mean_entropy /= c.windows as f64;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug::{ChangeEvent, DebugState, MemoryRegion};

    #[test]
    fn snapshot_maps_core_fields_and_region_summary() {
        let mut ds = DebugState::new();
        ds.frame_count = 1234;
        ds.fps = 59.94;
        ds.av_width = 320;
        ds.av_height = 224;
        ds.fb_width = 320;
        ds.fb_height = 224;
        ds.fb_fmt = 1;
        ds.paused = true;
        ds.m68k_d_regs[0] = 0xDEAD_BEEF;
        ds.m68k_pc = 0x0000_0400;
        ds.z80_pc = 0x1234;
        ds.nav.current_address = Some(0x0000_0400);

        // A ROM region (read-only via the CONST flag, bit 0).
        ds.memory_regions.push(MemoryRegion {
            name: "ROM".to_string(),
            addr_start: 0,
            addr_end: 0x3F_FFFF,
            size: 0x40_0000,
            flags: 1 << 0, // RETRO_MEMDESC_CONST
            ptr: 0,
            offset: 0,
            select: 0,
            disconnect: 0,
        });
        // A couple of accumulating-collection entries so counts are non-trivial.
        ds.breakpoints.push(0x0400);
        ds.push_change(ChangeEvent { frame: 1, addr: 0xFF00, old: 0, new: 1, pc: 0x0400 });

        let snap = AiSnapshot::from_debug_state(&ds);

        assert_eq!(snap.frame_count, 1234);
        assert_eq!(snap.av_width, 320);
        assert_eq!(snap.fb_fmt, 1);
        assert!(snap.paused);
        assert_eq!(snap.m68k.d[0], 0xDEAD_BEEF);
        assert_eq!(snap.m68k.pc, 0x0000_0400);
        assert_eq!(snap.z80.pc, 0x1234);
        assert_eq!(snap.nav_address, Some(0x0000_0400));

        assert_eq!(snap.regions.len(), 1);
        let r = &snap.regions[0];
        assert_eq!(r.name, "ROM");
        assert_eq!(r.kind, "ROM");
        assert!(r.readonly);
        assert_eq!(r.size, 0x40_0000);

        assert_eq!(snap.counts.breakpoints, 1);
        assert_eq!(snap.counts.change_log, 1);

        // And it actually serializes to JSON without panicking.
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("\"frame_count\":1234"));
    }

    #[test]
    fn png_encoder_emits_valid_png_for_2x2() {
        // 2×2 RGBA: red, green, blue, white.
        let rgba = vec![
            255, 0, 0, 255, //
            0, 255, 0, 255, //
            0, 0, 255, 255, //
            255, 255, 255, 255,
        ];
        let png = rgba_to_png(&rgba, 2, 2).expect("encode should succeed");
        assert!(!png.is_empty());
        // PNG magic number: 89 50 4E 47 0D 0A 1A 0A.
        assert_eq!(&png[..8], &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
    }

    #[test]
    fn png_encoder_rejects_size_mismatch() {
        // 3 bytes can't be a 1×1 RGBA pixel (needs 4).
        assert!(rgba_to_png(&[1, 2, 3], 1, 1).is_none());
        assert!(rgba_to_png(&[], 0, 0).is_none());
    }

    #[test]
    fn search_bytes_finds_known_needle_offsets() {
        // Haystack with the needle "BE EF" at offsets 2 and 7.
        let hay = [0x00, 0x11, 0xBE, 0xEF, 0x22, 0x33, 0x44, 0xBE, 0xEF, 0x55];
        let needle = [0xBEu8, 0xEF];
        let hits = search_bytes(&hay, &needle, 16);
        assert_eq!(hits, vec![2, 7]);
    }

    #[test]
    fn search_bytes_respects_max_hits_and_edges() {
        let hay = [0xAAu8, 0xAA, 0xAA, 0xAA];
        // single-byte needle 0xAA appears at every offset; cap at 2.
        assert_eq!(search_bytes(&hay, &[0xAA], 2), vec![0, 1]);
        // needle longer than haystack -> no hits.
        assert!(search_bytes(&[0x01, 0x02], &[0x01, 0x02, 0x03], 8).is_empty());
        // empty needle -> no hits.
        assert!(search_bytes(&hay, &[], 8).is_empty());
        // needle at the very end of the haystack is found.
        let hay2 = [0x00u8, 0x01, 0xCA, 0xFE];
        assert_eq!(search_bytes(&hay2, &[0xCA, 0xFE], 8), vec![2]);
    }

    #[test]
    fn parse_hex_bytes_tolerates_separators() {
        assert_eq!(parse_hex_bytes("DEADBEEF").unwrap(), vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(parse_hex_bytes("DE AD BE EF").unwrap(), vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(parse_hex_bytes("de,ad").unwrap(), vec![0xDE, 0xAD]);
        // Odd nibble count is rejected.
        assert!(parse_hex_bytes("ABC").is_none());
        // Non-hex char is rejected.
        assert!(parse_hex_bytes("ZZ").is_none());
        assert!(parse_hex_bytes("").is_none());
    }

    #[test]
    fn memory_map_resolves_region_by_name_and_kind() {
        let mut ds = DebugState::new();
        ds.memory_regions.push(MemoryRegion {
            name: "GFX ROM".to_string(),
            addr_start: 0x10_0000,
            addr_end: 0x1F_FFFF,
            size: 0x10_0000,
            flags: 1 << 0, // CONST -> ROM, readonly
            ptr: 0,
            offset: 0,
            select: 0,
            disconnect: 0,
        });
        ds.memory_regions.push(MemoryRegion {
            name: "VRAM".to_string(),
            addr_start: 0,
            addr_end: 0xFFFF,
            size: 0x1_0000,
            flags: 1 << 4, // VIDEO_RAM -> VRAM
            ptr: 0,
            offset: 0,
            select: 0,
            disconnect: 0,
        });
        let map = memory_map(&ds);
        assert_eq!(map.len(), 2);
        // Region-name resolution: find "GFX ROM" and confirm its classification.
        let rom = map.iter().find(|m| m.name == "GFX ROM").expect("GFX ROM present");
        assert_eq!(rom.kind, "ROM");
        assert!(rom.readonly);
        assert_eq!(rom.addr_start, 0x10_0000);
        assert_eq!(rom.addr_end, 0x1F_FFFF);
        let vram = map.iter().find(|m| m.name == "VRAM").expect("VRAM present");
        assert_eq!(vram.kind, "VRAM");
        assert!(!vram.readonly);
    }

    // memdesc flag bits (mirrors region_type()'s constants).
    const RETRO_MEMDESC_CONST: u64 = 1 << 0;
    const RETRO_MEMDESC_SYSTEM_RAM: u64 = 1 << 2;

    #[test]
    fn capability_none_when_no_regions() {
        let ds = DebugState::new();
        let cap = memory_capability(&ds);
        assert_eq!(cap.region_count, 0);
        assert_eq!(cap.source, "none");
        assert!(!cap.has_system_ram);
        assert!(!cap.has_vram);
        assert!(!cap.has_rom);
        assert_eq!(cap.total_readable_bytes, 0);
        assert!(cap.kinds.is_empty());
        assert!(cap.note.contains("No memory map"));
    }

    #[test]
    fn capability_backed_system_ram_counts_bytes_and_detects_fallback() {
        // A real stack buffer standing in for the core's work RAM.
        let buf = [0xAAu8; 64];
        let p = buf.as_ptr() as usize;
        let mut ds = DebugState::new();
        ds.memory_regions.push(MemoryRegion::synth_region(
            "System RAM (fallback)",
            0,
            buf.len(),
            p,
            RETRO_MEMDESC_SYSTEM_RAM,
        ));

        let cap = memory_capability(&ds);
        assert_eq!(cap.region_count, 1);
        assert!(cap.has_system_ram);
        assert!(!cap.has_vram);
        assert!(!cap.has_rom);
        assert_eq!(cap.total_readable_bytes, buf.len());
        assert_eq!(cap.kinds, vec!["RAM".to_string()]);
        // Name contains "(fallback)" -> source reflects the synthesized map.
        assert_eq!(cap.source, "get_memory_data fallback");
        assert!(cap.note.contains("Work RAM only"));
    }

    #[test]
    fn capability_virtual_region_counted_but_not_readable() {
        // A virtual/unbacked descriptor (null ptr), like NES OAM at 0x8000xxxx.
        let mut ds = DebugState::new();
        ds.memory_regions.push(MemoryRegion {
            name: "OAM".to_string(),
            addr_start: 0x8000_4000,
            addr_end: 0x8000_40FF,
            size: 0x100,
            flags: 1 << 4, // VIDEO_RAM
            ptr: 0,        // unbacked
            offset: 0,
            select: 0,
            disconnect: 0,
        });

        let cap = memory_capability(&ds);
        // Counted in region_count, but contributes ZERO readable bytes.
        assert_eq!(cap.region_count, 1);
        assert_eq!(cap.total_readable_bytes, 0);
        assert!(cap.has_vram);
        assert!(!cap.has_system_ram);
        // Declared-but-unreadable note.
        assert!(cap.note.contains("none are backed") || cap.note.contains("declared"));
        // Serializes cleanly.
        let json = serde_json::to_string(&cap).unwrap();
        assert!(json.contains("\"total_readable_bytes\":0"));
    }

    #[test]
    fn capability_core_map_with_ram_and_rom() {
        let ram = [0u8; 32];
        let rom = [0u8; 16];
        let mut ds = DebugState::new();
        ds.memory_regions.push(MemoryRegion::synth_region(
            "Work RAM",
            0,
            ram.len(),
            ram.as_ptr() as usize,
            RETRO_MEMDESC_SYSTEM_RAM,
        ));
        ds.memory_regions.push(MemoryRegion::synth_region(
            "Program ROM",
            0x10_0000,
            rom.len(),
            rom.as_ptr() as usize,
            RETRO_MEMDESC_CONST,
        ));
        let cap = memory_capability(&ds);
        assert_eq!(cap.source, "core memory map");
        assert!(cap.has_system_ram && cap.has_rom);
        assert_eq!(cap.total_readable_bytes, ram.len() + rom.len());
    }

    #[test]
    fn read_region_bytes_reads_backed_and_clamps() {
        let buf = [0u8, 1, 2, 3, 4, 5, 6, 7];
        let r = MemoryRegion::synth_region(
            "RAM",
            0,
            buf.len(),
            buf.as_ptr() as usize,
            RETRO_MEMDESC_SYSTEM_RAM,
        );
        // Full read.
        assert_eq!(read_region_bytes(&r, 0, 8).unwrap(), vec![0, 1, 2, 3, 4, 5, 6, 7]);
        // Offset + clamp past end.
        assert_eq!(read_region_bytes(&r, 6, 100).unwrap(), vec![6, 7]);
        // Offset at/past the end -> None.
        assert!(read_region_bytes(&r, 8, 4).is_none());
    }

    #[test]
    fn read_region_bytes_returns_none_for_virtual_region_no_panic() {
        // Null-ptr virtual descriptor: must return None, NEVER dereference/panic.
        let virt = MemoryRegion {
            name: "NTARAM".to_string(),
            addr_start: 0x8000_2000,
            addr_end: 0x8000_27FF,
            size: 0x800,
            flags: 1 << 4,
            ptr: 0,
            offset: 0,
            select: 0,
            disconnect: 0,
        };
        assert!(read_region_bytes(&virt, 0, 16).is_none());
        // Bogus non-null ptr with zero size is also refused.
        let bogus = MemoryRegion {
            name: "Bogus".to_string(),
            addr_start: 0x6000,
            addr_end: 0x6000,
            size: 0,
            flags: 0,
            ptr: 0xdead_beef,
            offset: 0,
            select: 0,
            disconnect: 0,
        };
        assert!(read_region_bytes(&bogus, 0, 4).is_none());
    }

    // ── tile decoders ───────────────────────────────────────────────────────

    #[test]
    fn decode_2bpp_nes_matches_hand_computed_tile() {
        // One NES CHR tile (16 bytes). Plane 0 (low bit) = rows 0..8,
        // plane 1 (high bit) = rows 8..16.
        //
        // Row 0: plane0 = 0b1000_0001 (0x81), plane1 = 0b0000_0000 → pixels:
        //   x0: hi0 lo1 = index 1; x1..6: 0; x7: hi0 lo1 = index 1.
        // Row 1: plane0 = 0b0000_0000, plane1 = 0b1111_1111 (0xFF) → all index 2.
        // Row 2: plane0 = 0b1111_1111, plane1 = 0b1111_1111 → all index 3.
        // Rows 3..8: all zero → index 0.
        let mut tile = [0u8; 16];
        tile[0] = 0x81; // plane0 row0
        tile[2] = 0xFF; // plane0 row2
        tile[8 + 1] = 0xFF; // plane1 row1
        tile[8 + 2] = 0xFF; // plane1 row2

        let idx = decode_2bpp_planar_indices(&tile);
        assert_eq!(idx.len(), 64);
        // Row 0.
        assert_eq!(&idx[0..8], &[1, 0, 0, 0, 0, 0, 0, 1]);
        // Row 1: all high bit set → index 2.
        assert_eq!(&idx[8..16], &[2, 2, 2, 2, 2, 2, 2, 2]);
        // Row 2: both planes set → index 3.
        assert_eq!(&idx[16..24], &[3, 3, 3, 3, 3, 3, 3, 3]);
        // Row 3: empty → index 0.
        assert_eq!(&idx[24..32], &[0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn decode_4bpp_genesis_matches_simple_pattern() {
        // One Genesis 4bpp tile (32 bytes): 4 planes of 8 bytes.
        // Row 0: plane0=0xFF, others 0 → every pixel index 0b0001 = 1.
        // Row 1: plane3=0xFF, others 0 → every pixel index 0b1000 = 8.
        // Row 2: all planes=0xFF → index 0b1111 = 15.
        // Row 3: plane0 row = 0b1000_0000 (0x80) only → only x0 gets bit0 → 1,
        //        rest 0.
        let mut tile = [0u8; 32];
        tile[0] = 0xFF; // plane0 row0
        tile[3 * 8 + 1] = 0xFF; // plane3 row1
        // plane0..3 row2 all 0xFF
        for p in 0..4 {
            tile[p * 8 + 2] = 0xFF;
        }
        tile[3] = 0x80; // plane0 row3

        let idx = decode_4bpp_planar_indices(&tile);
        assert_eq!(idx.len(), 64);
        assert_eq!(&idx[0..8], &[1, 1, 1, 1, 1, 1, 1, 1]); // row0 → 1
        assert_eq!(&idx[8..16], &[8, 8, 8, 8, 8, 8, 8, 8]); // row1 → 8
        assert_eq!(&idx[16..24], &[15, 15, 15, 15, 15, 15, 15, 15]); // row2 → 15
        assert_eq!(&idx[24..32], &[1, 0, 0, 0, 0, 0, 0, 0]); // row3 → x0=1
    }

    #[test]
    fn decode_2bpp_ignores_trailing_partial_tile() {
        // 16 + 5 bytes: only one complete tile decoded.
        let bytes = vec![0u8; 16 + 5];
        let idx = decode_2bpp_planar_indices(&bytes);
        assert_eq!(idx.len(), 64);
    }

    #[test]
    fn grid_layout_places_multiple_tiles_in_rows() {
        // 3 NES tiles, 2 per row → 2 rows, 4 cols-worth of pixels wide (2 tiles
        // = 16 px), 2 rows tall (16 px). Tile count = 3.
        // Make tile 0 solid index-3 (white), tiles 1,2 zero (black).
        let mut bytes = vec![0u8; 16 * 3];
        for i in 0..16 {
            bytes[i] = 0xFF; // tile 0: both planes all-ones → index 3 everywhere
        }
        let img = decode_tiles_to_rgba(&bytes, TileFormat::Nes2bpp, 2).unwrap();
        assert_eq!(img.tile_count, 3);
        assert_eq!(img.width, 16); // 2 tiles per row × 8 px
        assert_eq!(img.height, 16); // ceil(3/2) = 2 rows × 8 px
        // Top-left pixel (tile 0, index 3 → white).
        assert_eq!(&img.rgba[0..4], &[255, 255, 255, 255]);
        // Tile 1 starts at x=8: top-left pixel index 0 → black.
        let t1 = (0 * 16 + 8) * 4;
        assert_eq!(&img.rgba[t1..t1 + 4], &[0, 0, 0, 255]);
        // Padding tile (4th slot, row1 col1) is never written → transparent black.
        let pad = (8 * 16 + 8) * 4;
        assert_eq!(&img.rgba[pad..pad + 4], &[0, 0, 0, 0]);
        // Buffer size matches dimensions (so rgba_to_png will accept it).
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
        assert!(rgba_to_png(&img.rgba, img.width, img.height).is_some());
    }

    #[test]
    fn decode_tiles_rejects_too_few_bytes_and_zero_width() {
        // Fewer than one tile's worth of bytes → None.
        assert!(decode_tiles_to_rgba(&[0u8; 8], TileFormat::Nes2bpp, 16).is_none());
        // Zero tiles_per_row → None.
        assert!(decode_tiles_to_rgba(&[0u8; 16], TileFormat::Nes2bpp, 0).is_none());
    }

    #[test]
    fn tile_format_parse_accepts_aliases_and_rejects_unknown() {
        assert_eq!(TileFormat::parse("2bpp"), Some(TileFormat::Nes2bpp));
        assert_eq!(TileFormat::parse("nes_chr"), Some(TileFormat::Nes2bpp));
        assert_eq!(TileFormat::parse("CHR"), Some(TileFormat::Nes2bpp));
        assert_eq!(TileFormat::parse("4bpp"), Some(TileFormat::Genesis4bpp));
        assert_eq!(TileFormat::parse(" genesis "), Some(TileFormat::Genesis4bpp));
        // Unknown is rejected.
        assert_eq!(TileFormat::parse("8bpp"), None);
        assert_eq!(TileFormat::parse(""), None);
    }

    #[test]
    fn top_heatmap_sorts_hottest_first_and_truncates() {
        let mut ds = DebugState::new();
        ds.pc_heatmap.insert(0x100, 5);
        ds.pc_heatmap.insert(0x200, 50);
        ds.pc_heatmap.insert(0x300, 50);
        ds.pc_heatmap.insert(0x400, 1);
        let top = top_heatmap(&ds, 2);
        assert_eq!(top.len(), 2);
        // Both have 50 hits; tie broken by ascending address → 0x200 first.
        assert_eq!(top[0].pc, 0x200);
        assert_eq!(top[0].hits, 50);
        assert_eq!(top[1].pc, 0x300);
    }

    // ── major-region discovery ──────────────────────────────────────────────

    #[test]
    fn entropy_spans_zero_to_eight() {
        // Constant buffer → 0 bits.
        assert_eq!(shannon_entropy(&[0u8; 1000]), 0.0);
        // Empty → 0.
        assert_eq!(shannon_entropy(&[]), 0.0);
        // All 256 values once → exactly 8 bits/byte (uniform).
        let uniform: Vec<u8> = (0..=255u8).collect();
        assert!((shannon_entropy(&uniform) - 8.0).abs() < 1e-9);
        // Two equally-likely values → exactly 1 bit/byte.
        let mut two = vec![0u8; 500];
        two.extend(vec![1u8; 500]);
        assert!((shannon_entropy(&two) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn window_stats_captures_histogram_features() {
        // Half 0x00, half 0xFF.
        let mut b = vec![0x00u8; 100];
        b.extend(vec![0xFFu8; 100]);
        let s = window_stats(&b);
        assert!((s.zero_frac - 0.5).abs() < 1e-9);
        assert!((s.ff_frac - 0.5).abs() < 1e-9);
        assert_eq!(s.distinct_bytes, 2);
        assert!((s.top_byte_frac - 0.5).abs() < 1e-9);
        assert!((s.entropy - 1.0).abs() < 1e-9);

        // Printable ASCII.
        let txt = b"HELLO WORLD the quick brown fox 12345".to_vec();
        let st = window_stats(&txt);
        assert!(st.printable_frac > 0.99, "printable={}", st.printable_frac);

        // Empty → all-zero fingerprint, no distinct bytes.
        assert_eq!(window_stats(&[]).distinct_bytes, 0);
    }

    #[test]
    fn classify_window_buckets_synthetic_inputs() {
        // Padding: all zeros.
        assert_eq!(classify_window(&window_stats(&[0u8; 1024])).kind, "padding");
        // Padding: all 0xFF.
        assert_eq!(classify_window(&window_stats(&[0xFFu8; 1024])).kind, "padding");
        // Text: a printable string repeated to fill a window.
        let txt: Vec<u8> = b"The quick brown fox jumps over the lazy dog. "
            .iter()
            .cycle()
            .take(1024)
            .copied()
            .collect();
        assert_eq!(classify_window(&window_stats(&txt)).kind, "text_table");
        // Packed/high-entropy: all 256 values cycled (entropy ~8).
        let packed: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        assert_eq!(classify_window(&window_stats(&packed)).kind, "packed_data");
        // Low-entropy structured table: mostly one value with a few others —
        // low entropy but NOT 97% padding.
        let mut table = vec![0x01u8; 1024];
        for i in (0..1024).step_by(8) {
            table[i] = (i % 251) as u8; // sprinkle distinct bytes, ~12.5% non-fill
        }
        assert_eq!(classify_window(&window_stats(&table)).kind, "lookup_table");
        // Graphics: mid entropy WITH lots of background fill.
        let mut gfx = vec![0u8; 4096];
        for i in 0..4096 {
            // ~40% background, the rest a varied-but-not-uniform pattern.
            if i % 5 != 0 {
                gfx[i] = ((i * 37) % 64) as u8;
            }
        }
        let gc = classify_window(&window_stats(&gfx));
        assert_eq!(gc.kind, "graphics", "stats={:?}", window_stats(&gfx));
        // Code: mid entropy, little background, spiky-ish histogram.
        let mut code = vec![0u8; 4096];
        for i in 0..4096 {
            // Dominant "opcode" byte 0x4E, varied operands, very little 0x00.
            code[i] = if i % 3 == 0 { 0x4E } else { ((i * 53) % 200 + 1) as u8 };
        }
        let cc = classify_window(&window_stats(&code));
        assert_eq!(cc.kind, "code", "stats={:?}", window_stats(&code));
    }

    #[test]
    fn scan_buffer_coalesces_adjacent_same_kind_windows() {
        // 4 KB padding ++ 4 KB packed ++ 4 KB padding → 3 candidates.
        let mut buf = vec![0u8; 4096];
        buf.extend((0..=255u8).cycle().take(4096));
        buf.extend(vec![0u8; 4096]);
        let cands = scan_buffer(&buf, 1024);
        assert_eq!(cands.len(), 3, "{cands:?}");
        assert_eq!(cands[0].kind, "padding");
        assert_eq!(cands[0].start, 0);
        assert_eq!(cands[0].end, 4096);
        assert_eq!(cands[0].windows, 4); // 4×1024 coalesced
        assert_eq!(cands[1].kind, "packed_data");
        assert_eq!(cands[1].start, 4096);
        assert_eq!(cands[2].kind, "padding");
        assert_eq!(cands[2].end, buf.len());
        // mean_entropy of the all-zero spans is 0.
        assert_eq!(cands[0].mean_entropy, 0.0);
        assert!(cands[1].mean_entropy > 7.0);
    }

    #[test]
    fn scan_buffer_handles_trailing_partial_window_and_empty() {
        assert!(scan_buffer(&[], 256).is_empty());
        // 300 bytes with a 256 window → one full + one 44-byte partial, both
        // padding, coalesced into a single span covering all 300 bytes.
        let cands = scan_buffer(&vec![0u8; 300], 256);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].len, 300);
        assert_eq!(cands[0].windows, 2);
    }

    #[test]
    fn scan_buffer_clamps_tiny_window_to_floor() {
        // A window below the 256 floor is clamped; a 256-byte all-zero buffer
        // becomes a single window / single padding candidate.
        let cands = scan_buffer(&vec![0u8; 256], 16);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].windows, 1);
    }
}
