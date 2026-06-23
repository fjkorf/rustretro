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
}
