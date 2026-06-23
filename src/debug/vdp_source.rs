//! VDP register source investigation and (stub) population helper.
//!
//! # Findings: VDP Register Availability
//!
//! ## Why a clean source does NOT exist
//!
//! On a real Sega Genesis, the 24 VDP registers ($00–$17) are **write-only**:
//! the CPU writes them via the control port at $C00004/$C00006, but there is no
//! hardware read-back path. The VDP holds the register values internally, but they
//! never appear in the 68K address space.
//!
//! This has two consequences for the libretro layer:
//!
//! 1. **SET_MEMORY_MAPS does not expose VDP registers.**
//!    The memory descriptors that Genesis Plus GX (genesis_plus_gx_libretro) and
//!    fbalpha2012 push via `RETRO_ENVIRONMENT_SET_MEMORY_MAPS` describe RAM (64 KB),
//!    SRAM, and ROM regions. The VDP register file is internal chip state; it is not
//!    mapped into any descriptor. Confirmed: the `MemoryRegion` list in DebugState
//!    has no descriptor whose `flags` include `RETRO_MEMDESC_VIDEO_RAM` for the
//!    *register* file (VRAM is the pattern/tilemap data, a separate 64 KB block, not
//!    the 24-byte register array).
//!
//! 2. **fbalpha2012 exports no VDP peek symbol.**
//!    The fbalpha2012 debug API that is already wired into libretro.rs exposes M68K
//!    registers via `SekDbgGetRegister` / `SekFetchByte` and Z80 state via `ZetGetPC`
//!    etc. There is no exported symbol such as `VdpGetRegister`, `vdp_reg_read`, or
//!    anything similar in the fbalpha2012 source tree or its libretro port. The Sek
//!    (68K) and Zet (Z80) debug APIs are the only debug hooks this core exposes.
//!
//! Genesis Plus GX (the other common Genesis core) similarly does not export a
//! per-register peek function in its libretro API. It does expose VRAM/CRAM/VSRAM
//! blocks via `retro_get_memory_data(RETRO_MEMORY_VIDEO_RAM)`, but that returns the
//! 64 KB tile/map memory, not the 24-byte register file.
//!
//! ## Path forward / recommendations
//!
//! Three routes exist, roughly in order of effort:
//!
//! **A. Intercept control-port writes (the goal — but NOT feasible on stock cores).**
//!    The 68K writes VDP registers (and arms DMA) by writing a 16-bit control word
//!    to $C00004 where the top two bits are `10`. Logging each such write is what
//!    would give *true* DMA-source→VRAM-dest provenance. The catch (confirmed by the
//!    2026-06 spike below): **libretro exposes no per-memory-access / per-write
//!    callback at all**, and neither stock core exports a write-intercept symbol.
//!    `SekFetchByte` is an *instruction* fetch (for disassembly), not a data-write
//!    hook. fbalpha2012's `BurnWriteByte` / `BurnWriteWord` are INTERNAL and are not
//!    exported by the stock `.dylib`; a custom `SekMapHandler` likewise lives inside
//!    the core. So this route requires a **patched / custom-built core**, not the
//!    stock cores we run — it is out of scope until/unless we ship our own core.
//!
//! **B. Read the core's internal register array by pointer.**
//!    Genesis Plus GX keeps its VDP registers in a global array `vdp_reg[24]` (in
//!    `vdp_ctrl.c`). If the core is built as a `.so` with exported symbols (or with
//!    a known module base), the host can obtain the address of that array and read it
//!    directly. This is fragile (symbol must be exported, ASLR must be accounted for)
//!    but is the lowest-overhead option if the symbol is available. In practice the
//!    stock cores do NOT export it, so this shares Option A's blocker.
//!    For fbalpha2012 the equivalent is `sFd.m_VDP.regs[24]` inside the
//!    `MD_VDP` struct, also not exported by default.
//!
//! **C. Use a save-state / serialisation snapshot (the only stock-core route).**
//!    Both cores support `retro_serialize`. Parsing the save-state binary for the
//!    24-byte register block is brittle but requires no new symbols — the offset is
//!    stable across builds of the same core version. This yields a *snapshot* of the
//!    VDP registers (incl. the currently-armed DMA source/length/dest), NOT a history
//!    of write events — so it can decode "a DMA is configured ROM $X → VRAM $Y right
//!    now", but cannot attribute an arbitrary on-screen tile to its loader after the
//!    fact. It is the realistic deliverable if a VDP-register panel is wanted.
//!
//! ## Spike conclusion (2026-06)
//!
//! A focused feasibility spike (intercept $C00004/$C00006 for ROM→VRAM provenance)
//! reached this verdict:
//!
//! - **True DMA-event provenance is INFEASIBLE on the stock cores we run**
//!   (genesis_plus_gx, fbalpha2012). libretro has no memory-write callback; the PC
//!   heatmap and breakpoints are *post-frame polling* of `SekDbgGetRegister(PC)`
//!   (see `frontend.rs::capture_cpu_state`), not a per-instruction/per-access hook
//!   that could be extended to watch writes. Routes A and B both need a patched core.
//! - **What we ship instead is convergent CONTENT/STRUCTURE evidence, not a trace:**
//!   `vram_to_rom` (frame-granular VRAM byte → ROM content match), `render_tiles`
//!   (decode a candidate ROM span as tiles and eyeball it), and `scan_regions`
//!   (entropy/histogram structure scan). Agreement between these is the provenance
//!   story on stock cores — honestly NOT DMA-traced, and the MCP layer says so.
//! - **`read_vdp_regs` stays a `None` stub.** If a VDP-register panel is later
//!   wanted, implement Option C (save-state parse) — that is the only stock-core
//!   route, and it gives a snapshot of the armed DMA, not write history.
//!
//! ## Current deliverable
//!
//! `read_vdp_regs` below returns `None` unconditionally. It exists so that
//! `frontend.rs::capture_cpu_state` can call it and safely populate
//! `DebugState.vdp_regs` once a real source is available — the call site does not
//! change when the implementation is filled in.

/// Attempt to read the 24 Genesis VDP hardware registers ($00–$17).
///
/// # Current status
///
/// Returns `None` because no clean read-back path is currently available
/// (see module-level documentation for a full explanation and recommended
/// implementation routes).
///
/// # Future implementation
///
/// Replace the body with whichever approach is chosen (control-port intercept,
/// direct pointer read, or save-state parse). The call site in `frontend.rs`
/// does not need to change.
///
/// # Parameters
///
/// `regions` is passed for future use by approach A/B above (e.g. locating the
/// memory region that contains the VDP register file once it is mapped).
pub fn read_vdp_regs(_regions: &[crate::debug::MemoryRegion]) -> Option<[u8; 24]> {
    None
}
