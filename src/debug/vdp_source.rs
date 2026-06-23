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
//! **A. Intercept control-port writes (recommended, no core modification needed).**
//!    The 68K writes VDP registers by writing a 16-bit control word to $C00004 where
//!    the top two bits are `10`. Hooking `SekFetchByte` is already available, but
//!    what is needed is a *data write* intercept. fbalpha2012 exposes
//!    `BurnWriteByte` / `BurnWriteWord` hooks in some configurations; alternatively,
//!    the data read at $C00004/$C00006 could be intercepted via a custom `SekMapHandler`.
//!    This approach stays entirely in-process and requires no changes to the core .so.
//!
//! **B. Read the core's internal register array by pointer.**
//!    Genesis Plus GX keeps its VDP registers in a global array `vdp_reg[24]` (in
//!    `vdp_ctrl.c`). If the core is built as a `.so` with exported symbols (or with
//!    a known module base), the host can obtain the address of that array and read it
//!    directly. This is fragile (symbol must be exported, ASLR must be accounted for)
//!    but is the lowest-overhead option if the symbol is available.
//!    For fbalpha2012 the equivalent is `sFd.m_VDP.regs[24]` inside the
//!    `MD_VDP` struct, also not exported by default.
//!
//! **C. Use a save-state / serialisation snapshot.**
//!    Both cores support `retro_serialize`. Parsing the save-state binary for the
//!    24-byte register block is brittle but requires no new symbols — the offset is
//!    stable across builds of the same core version.
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
