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

use crate::debug::DebugState;

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
