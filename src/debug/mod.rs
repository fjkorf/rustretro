pub mod panels;
pub mod window;
pub mod dock;
pub mod vdp_source;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

pub type SharedDebugState = Arc<Mutex<DebugState>>;

/// A user-created snapshot of machine state at a named moment (e.g. "Title Screen", "Level 2").
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Bookmark {
    pub label: String,
    pub frame: u64,
    pub m68k_pc: u32,
    pub m68k_d_regs: [u32; 8],
    pub m68k_a_regs: [u32; 8],
    /// 64×48 RGBA thumbnail. Not persisted (regenerated during play).
    #[serde(skip)]
    pub thumbnail: Vec<u8>,
    pub notes: String,
}

/// A user-labeled range of M68K code addresses (e.g. "game_loop", "sound_driver").
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeRegion {
    pub label: String,
    pub addr_start: u32,
    pub addr_end: u32,
    /// RGB display color for this region.
    pub color: [u8; 3],
    pub notes: String,
}

/// How a watched address's bytes are interpreted for display.
#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, Debug)]
pub enum WatchFormat {
    U8,
    S8,
    U16LE,
    U16BE,
    U32LE,
    U32BE,
    Hex8,
    Hex16,
    Hex32,
}

impl WatchFormat {
    /// Number of bytes this format reads from memory.
    pub fn byte_len(&self) -> usize {
        match self {
            WatchFormat::U8 | WatchFormat::S8 | WatchFormat::Hex8 => 1,
            WatchFormat::U16LE | WatchFormat::U16BE | WatchFormat::Hex16 => 2,
            WatchFormat::U32LE | WatchFormat::U32BE | WatchFormat::Hex32 => 4,
        }
    }
}

/// Value width for a RAM search.
#[derive(Clone, Copy, PartialEq)]
pub enum SearchSize {
    U8,
    U16,
    U32,
}

impl SearchSize {
    pub fn byte_len(self) -> usize {
        match self {
            SearchSize::U8 => 1,
            SearchSize::U16 => 2,
            SearchSize::U32 => 4,
        }
    }

    /// The matching watch format for this size (hex chosen at call site).
    pub fn watch_format(self, hex: bool) -> WatchFormat {
        match (self, hex) {
            (SearchSize::U8, false) => WatchFormat::U8,
            (SearchSize::U16, false) => WatchFormat::U16LE,
            (SearchSize::U32, false) => WatchFormat::U32LE,
            (SearchSize::U8, true) => WatchFormat::Hex8,
            (SearchSize::U16, true) => WatchFormat::Hex16,
            (SearchSize::U32, true) => WatchFormat::Hex32,
        }
    }
}

/// Comparison operator applied during a RAM search step.
#[derive(Clone, Copy, PartialEq)]
pub enum SearchCompare {
    Equal,
    NotEqual,
    Less,
    Greater,
    Changed,
    Unchanged,
    Increased,
    Decreased,
    DifferentBy(i64),
}

/// What a search step compares each candidate's current value against.
#[derive(Clone)]
pub enum SearchSource {
    /// Compare against the value captured at the previous checkpoint.
    PreviousSnapshot,
    /// Compare against a fixed user-supplied value.
    SpecificValue(u32),
}

/// Pure comparison kernel for one candidate.
/// `cur` is the freshly read value; `rhs` is either the previous snapshot value
/// or the specific target value, depending on the operator/source.
/// `bits` is the value width in bits (8/16/32) used for signed interpretation.
pub fn compare_passes(cur: u32, rhs: u32, op: SearchCompare, signed: bool, bits: u32) -> bool {
    let sx = |v: u32| -> i64 {
        if signed && bits < 32 {
            let shift = 32 - bits;
            ((v << shift) as i32 >> shift) as i64
        } else if signed {
            (v as i32) as i64
        } else {
            v as i64
        }
    };
    match op {
        SearchCompare::Equal => cur == rhs,
        SearchCompare::NotEqual => cur != rhs,
        SearchCompare::Less => sx(cur) < sx(rhs),
        SearchCompare::Greater => sx(cur) > sx(rhs),
        SearchCompare::Changed => cur != rhs,
        SearchCompare::Unchanged => cur == rhs,
        SearchCompare::Increased => sx(cur) > sx(rhs),
        SearchCompare::Decreased => sx(cur) < sx(rhs),
        SearchCompare::DifferentBy(d) => (sx(cur) - sx(rhs)) == d || (sx(rhs) - sx(cur)) == d,
    }
}

/// Iterative cheat-engine-style RAM search state. Persists across frames.
pub struct RamSearch {
    pub region_idx: usize,
    pub size: SearchSize,
    pub signed: bool,
    pub hex: bool,
    /// Guest addresses still in the running.
    pub candidates: Vec<usize>,
    /// Value captured at each candidate at the last checkpoint (parallel to `candidates`).
    pub prev_values: Vec<u32>,
    pub started: bool,
}

impl RamSearch {
    pub fn new() -> Self {
        RamSearch {
            region_idx: 0,
            size: SearchSize::U8,
            signed: false,
            hex: false,
            candidates: Vec::new(),
            prev_values: Vec::new(),
            started: false,
        }
    }
}

/// A single watched memory location.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Watch {
    pub addr: usize,
    pub label: String,
    pub format: WatchFormat,
    pub frozen: bool,
    pub frozen_value: Option<u32>,
    /// When true, log every frame in which this watch's value changes, together
    /// with the CPU PC executing that frame ("what changed this address?").
    /// Frame-granular, not instruction-exact.
    #[serde(default)]
    pub track_changes: bool,
    /// Last raw little-endian value read from memory (for display). Not persisted.
    #[serde(skip)]
    pub current: Option<u32>,
    /// Last value seen for change-detection edge tracking. Not persisted.
    #[serde(skip)]
    pub prev_value: Option<u32>,
}

/// One frame-granular value change recorded for a tracked watch: the value went
/// from `old` to `new` during `frame`, while the M68K PC was `pc`. Because
/// libretro has no per-access hook, this only pins the change to a frame, not to
/// the exact instruction.
#[derive(Clone, serde::Serialize)]
pub struct ChangeEvent {
    pub frame: u64,
    pub addr: usize,
    pub old: u32,
    pub new: u32,
    pub pc: u32,
}

/// True when `cur` differs from a known previous value. Used for per-frame
/// change-detection on tracked watches; a `None` prev (first sighting) is not a
/// change so we don't log a spurious event on the first frame.
pub fn detect_change(prev: Option<u32>, cur: u32) -> bool {
    matches!(prev, Some(p) if p != cur)
}

/// Shared cross-panel navigation state: a single "current location" cursor plus a
/// back/forward history stack.
///
/// ## Contract for address-aware panels
/// Every frame, an address-aware panel (Disassembly, Hex, Regions, Watch, RamSearch)
/// reads [`NavState::pending_focus`]. If it is `Some(addr)`, the panel scrolls/centers
/// its view to `addr` for this frame. Panels MUST NOT clear `pending_focus` themselves;
/// it is a one-frame pulse cleared centrally by the dispatcher AFTER all panels have
/// rendered (see `DebugApp::show`, which sets `nav.pending_focus = None` once the
/// CentralPanel closure returns). This guarantees every panel sees the same pulse for
/// exactly one frame regardless of which tab is active.
///
/// To change the current location from anywhere, call [`DebugState::goto`] (THE entry
/// point). Back/forward navigation is driven by the toolbar via [`DebugState::nav_back`]
/// / [`DebugState::nav_forward`].
#[derive(Default, serde::Serialize)]
pub struct NavState {
    /// The shared "current location" cursor (None until first `goto`).
    pub current_address: Option<u32>,
    /// Back/forward stack of visited addresses (oldest at front).
    pub history: Vec<u32>,
    /// Index into `history` of the current entry.
    pub history_idx: usize,
    /// Set whenever the address changes; address-aware panels consume it by reading
    /// (the dispatcher clears it after the frame's panels have rendered).
    pub pending_focus: Option<u32>,
}

/// Memory region descriptor (from libretro SET_MEMORY_MAPS callback)
#[derive(Clone)]
pub struct MemoryRegion {
    pub name: String,           // e.g., "System RAM", "ROM"
    pub addr_start: usize,      // emulated address start
    pub addr_end: usize,        // emulated address end (inclusive)
    pub size: usize,
    pub flags: u64,             // RETRO_MEMDESC_* flags
    pub ptr: usize,             // host pointer (cast to *const u8 for reads)
    pub offset: usize,          // offset within ptr
    pub select: usize,          // address mask
    pub disconnect: usize,      // address disconnect mask
}

impl MemoryRegion {
    /// Synthesize a flat memory region backed by a real host pointer.
    ///
    /// Used by the SET_MEMORY_MAPS fallback (see Frontend::apply_memory_map_fallback)
    /// when a core publishes no memory map but does expose
    /// retro_get_memory_data/size. The region is a simple identity mapping:
    /// guest addr `base..=base+size-1` maps to host `ptr+0..ptr+size-1`
    /// (offset/select/disconnect all 0), so `safe_host_ptr` accepts in-bounds
    /// reads and rejects out-of-bounds ones.
    pub fn synth_region(
        name: impl Into<String>,
        base: usize,
        size: usize,
        ptr: usize,
        flags: u64,
    ) -> MemoryRegion {
        MemoryRegion {
            name: name.into(),
            addr_start: base,
            addr_end: base + size.saturating_sub(1),
            size,
            flags,
            ptr,
            offset: 0,
            select: 0,
            disconnect: 0,
        }
    }

    /// Compute host pointer for an emulated address within this region.
    pub fn host_ptr_for_addr(&self, emu_addr: usize) -> Option<usize> {
        if emu_addr < self.addr_start || emu_addr > self.addr_end {
            return None;
        }
        // Formula from libretro spec:
        // host_addr = ptr + offset + (emu_addr & ~disconnect) - start
        Some(self.ptr + self.offset + ((emu_addr & !self.disconnect) - self.addr_start))
    }

    /// Validate that `len` bytes can be safely read at `emu_addr` from this
    /// region, returning the host pointer only if the read is in-bounds.
    ///
    /// Some cores declare descriptors with a null/garbage `ptr` or a `size` that
    /// doesn't actually back the address range (e.g. libretro "virtual" regions
    /// like NES NTARAM/PALRAM/OAM at 0x8000xxxx). Dereferencing those segfaults,
    /// so this guards: the region must have a non-null `ptr` and non-zero `size`,
    /// and `[host_offset, host_offset + len)` must stay within `[ptr+offset,
    /// ptr+offset+size)`.
    pub fn safe_host_ptr(&self, emu_addr: usize, len: usize) -> Option<*const u8> {
        if self.ptr == 0 || self.size == 0 || len == 0 {
            return None;
        }
        let host = self.host_ptr_for_addr(emu_addr)?;
        let base = self.ptr.checked_add(self.offset)?;
        let end = base.checked_add(self.size)?;
        let read_end = host.checked_add(len)?;
        if host < base || read_end > end {
            return None;
        }
        Some(host as *const u8)
    }

    /// Get region type as human-readable string.
    pub fn region_type(&self) -> &'static str {
        const RETRO_MEMDESC_CONST: u64 = 1 << 0;
        const RETRO_MEMDESC_SYSTEM_RAM: u64 = 1 << 2;
        const RETRO_MEMDESC_SAVE_RAM: u64 = 1 << 3;
        const RETRO_MEMDESC_VIDEO_RAM: u64 = 1 << 4;

        // PRIMARY: descriptor flags (Genesis/CPS2 cores set these).
        if self.flags & RETRO_MEMDESC_VIDEO_RAM != 0 { return "VRAM"; }
        if self.flags & RETRO_MEMDESC_SAVE_RAM != 0 { return "SRAM"; }
        if self.flags & RETRO_MEMDESC_SYSTEM_RAM != 0 { return "RAM"; }
        if self.flags & RETRO_MEMDESC_CONST != 0 { return "ROM"; }

        // FALLBACK: some cores (e.g. fceumm/NES) publish named regions
        // (OAM, PALRAM, NTARAM, PPUREG, …) without setting the flags the
        // classifier expects, so they'd otherwise all read as "Unmapped".
        // Match on the region NAME (case-insensitive substring). Order
        // matters: more-specific video/save names are checked before the
        // generic "RAM" catch so e.g. "PALRAM"/"NTARAM" land as VRAM, not RAM.
        let name = self.name.to_ascii_uppercase();
        let has = |needle: &str| name.contains(needle);

        // Save/battery RAM (check before generic RAM/ROM).
        if has("SRAM") || has("SAVE") || has("BATTERY") { return "SRAM"; }
        // Video / PPU memory: sprite OAM, palette, nametables, CHR, generic VRAM.
        if has("OAM") || has("SPRITE") || has("PAL") || has("NAM") || has("NTA")
            || has("VRAM") || has("VIDEO") || has("CHR") || has("PPU") { return "VRAM"; }
        // Program/cartridge ROM.
        if has("ROM") || has("PRG") || has("CART") { return "ROM"; }
        // Generic work/system RAM (checked last so it doesn't shadow VRAM names
        // that also contain "RAM", e.g. PALRAM/NTARAM handled above).
        if has("WRAM") || has("WORK") || has("RAM") { return "RAM"; }

        "Unmapped"
    }

    /// Get color for this region type (for UI display).
    pub fn color(&self) -> (u8, u8, u8) {
        match self.region_type() {
            "ROM" => (100, 150, 255),    // blue
            "RAM" => (200, 200, 200),    // white
            "VRAM" => (255, 200, 100),   // yellow
            "SRAM" => (200, 100, 255),   // magenta
            _ => (100, 100, 100),        // gray
        }
    }

    /// Check if region is read-only (ROM).
    pub fn is_readonly(&self) -> bool {
        const RETRO_MEMDESC_CONST: u64 = 1 << 0;
        self.flags & RETRO_MEMDESC_CONST != 0
    }
}

/// All data shared from the emulation thread → debug window.
pub struct DebugState {
    // --- Framebuffer ---
    /// Raw framebuffer bytes in the core's native pixel format.
    pub framebuffer: Vec<u8>,
    pub fb_width: u32,
    pub fb_height: u32,
    pub fb_pitch: usize,
    /// libretro pixel format: 0=0RGB1555, 1=XRGB8888, 2=RGB565
    pub fb_fmt: u32,
    /// Decoded RGBA8888 version of the last real frame (always up to date).
    pub fb_rgba: Vec<u8>,
    /// Incremented every time a new real frame arrives.
    pub fb_generation: u64,

    // --- Memory regions ---
    /// Accessible memory regions (from SET_MEMORY_MAPS callback)
    pub memory_regions: Vec<MemoryRegion>,

    // --- M68K code bytes for disassembly ---
    /// Raw bytes fetched from M68K address space starting at `m68k_code_start`.
    /// Populated each frame via SekFetchByte when available; empty otherwise.
    pub m68k_code_bytes: Vec<u8>,
    /// Guest address of the first byte in `m68k_code_bytes`.
    pub m68k_code_start: u32,

    // --- M68K CPU State ---
    pub m68k_d_regs: [u32; 8],     // D0-D7
    pub m68k_a_regs: [u32; 8],     // A0-A7
    pub m68k_pc: u32,              // Program Counter
    pub m68k_sr: u32,              // Status Register

    /// Previous-frame register values for delta highlighting.
    pub prev_m68k_d_regs: [u32; 8],
    pub prev_m68k_a_regs: [u32; 8],
    pub prev_m68k_pc: u32,

    // --- Z80 CPU State ---
    pub z80_pc: u16,               // Program Counter
    pub z80_bc: u16,               // BC register pair
    pub z80_de: u16,               // DE register pair
    pub z80_hl: u16,               // HL register pair

    // --- VDP registers ---
    /// Sega Genesis VDP registers $00–$17 (decoded by the VDP panel).
    /// Source not yet wired from the core; displays zeros until populated.
    pub vdp_regs: [u8; 24],

    // --- Frame counters ---
    pub frame_count: u64,
    pub video_frames: u64,
    pub video_real: u64,

    // --- AV info ---
    pub fps: f64,
    pub av_width: u32,
    pub av_height: u32,

    // --- Input ---
    /// Current joypad button states (12 buttons, RETRO_DEVICE_ID order).
    pub input_state: [bool; 12],
    /// Rolling history: (frame_number, button_states).
    pub input_history: VecDeque<(u64, [bool; 12])>,

    // --- Event log ---
    /// Rolling log of notable events (env callbacks, AV changes, etc.).
    pub event_log: VecDeque<String>,

    // --- Control flags (written by debug window, read by emulation loop) ---
    pub debug_open: bool,
    pub paused: bool,
    pub step_one: bool,

    // --- Breakpoints ---
    /// List of M68K PC addresses that will pause execution when hit.
    pub breakpoints: Vec<u32>,
    /// Set to Some(addr) when execution paused due to a breakpoint.
    pub hit_breakpoint: Option<u32>,
    /// When Some(addr), run until PC reaches that address then pause.
    pub run_to_addr: Option<u32>,

    // --- Triggers ---
    pub trigger_frame: Option<u64>,
    pub trigger_pixel: Option<(u32, u32)>,

    // --- Region Discovery ---
    /// Accumulated PC visit counts (address → frame count). Grows every frame automatically.
    pub pc_heatmap: HashMap<u32, u64>,
    /// User-created game state bookmarks (press B or click Bookmark button).
    pub bookmarks: Vec<Bookmark>,
    /// User-labeled M68K address ranges shown inline in the disassembly panel.
    pub code_regions: Vec<CodeRegion>,
    /// Signal from UI or keyboard: capture a bookmark on the next emulation frame.
    pub create_bookmark: bool,
    /// Signal from UI: write regions sidecar to disk on the next emulation frame.
    pub save_regions: bool,
    /// Path of the regions sidecar file (set by Frontend on startup).
    pub sidecar_path: Option<std::path::PathBuf>,
    /// Path of the literate ROM-map Markdown file, `library/<slug>/<slug>.md`,
    /// where `<slug>` is the ROM file stem (set by Frontend on startup). The MCP
    /// `add_rom_map_region`/`get_rom_map` tools read/scaffold this file so an AI
    /// RE session can persist confirmed findings across sessions (see
    /// `ROM_MAP_FORMAT.md`). `None` until a ROM is loaded with a library path.
    pub rom_map_path: Option<std::path::PathBuf>,
    /// The ROM file stem (e.g. "mvsc"), used as the map slug and to seed the
    /// scaffolded frontmatter `rom.name`. Set by Frontend on startup.
    pub rom_name: Option<String>,
    /// SHA-1 of the loaded ROM bytes (lowercase hex), used to seed the scaffolded
    /// frontmatter `rom.sha1` identity key (§3). `None` for need_fullpath cores
    /// where the bytes aren't read into memory.
    pub rom_sha1: Option<String>,
    /// Byte length of the loaded ROM, used to seed the scaffolded frontmatter
    /// `rom.size`. `None` for need_fullpath cores where the bytes aren't read.
    pub rom_size: Option<usize>,
    /// The ROM-map `system` slug (e.g. "nes", "megadrive") inferred from the
    /// loaded core's `library_name`. `None` when the core can't be confidently
    /// mapped (e.g. multi-system FBNeo) — left blank rather than guessed wrong.
    /// Seeds the scaffolded frontmatter `rom.system`. Set by Frontend on startup.
    pub rom_system: Option<String>,
    /// The raw ROM-file bytes, retained so the MCP `rom_file` source can decode
    /// content the running core does NOT expose in memory (e.g. NES CHR-ROM
    /// graphics). `None` for need_fullpath cores (which never read the bytes here)
    /// — those fall back to re-reading [`rom_path`](Self::rom_path) on demand.
    pub rom_bytes: Option<Vec<u8>>,
    /// Absolute path to the loaded ROM file, kept so the `rom_file` source can
    /// re-read it when the bytes weren't retained (need_fullpath cores).
    pub rom_path: Option<std::path::PathBuf>,

    // --- Watches ---
    /// User-created memory watches (displayed in the Watch panel).
    pub watches: Vec<Watch>,
    /// Iterative RAM-search state (cheat-engine-style value narrowing).
    pub ram_search: RamSearch,
    /// Rolling log of value changes for tracked watches (capped, newest at back).
    pub change_log: VecDeque<ChangeEvent>,

    // --- Navigation ---
    /// Shared cross-panel navigation cursor + back/forward history.
    pub nav: NavState,

    // --- AI Wave 1: deferred Lua bridge (MCP run_lua round-trip) ---
    /// Lua source submitted by the MCP `run_lua` tool, waiting for the main
    /// thread to execute it. The MCP thread sets this under lock; the Bevy
    /// `drain_lua_requests` system (which owns the NonSend LuaEngine) picks it
    /// up, runs it, and clears it back to `None`.
    pub pending_lua: Option<String>,
    /// Result of the most recently drained `pending_lua` request: `Ok(output)`
    /// or `Err(message)`. The MCP thread polls this and clears it on read.
    pub pending_lua_result: Option<Result<String, String>>,
}

/// Maximum number of change events retained in `change_log`.
const CHANGE_LOG_CAP: usize = 200;

impl DebugState {
    pub fn new() -> Self {
        DebugState {
            framebuffer: Vec::new(),
            fb_width: 0,
            fb_height: 0,
            fb_pitch: 0,
            fb_fmt: 0,
            fb_rgba: Vec::new(),
            fb_generation: 0,
            memory_regions: Vec::new(),
            m68k_code_bytes: Vec::new(),
            m68k_code_start: 0,
            m68k_d_regs: [0; 8],
            m68k_a_regs: [0; 8],
            m68k_pc: 0,
            m68k_sr: 0,
            prev_m68k_d_regs: [0; 8],
            prev_m68k_a_regs: [0; 8],
            prev_m68k_pc: 0,
            z80_pc: 0,
            z80_bc: 0,
            z80_de: 0,
            z80_hl: 0,
            vdp_regs: [0u8; 24],
            frame_count: 0,
            video_frames: 0,
            video_real: 0,
            fps: 60.0,
            av_width: 0,
            av_height: 0,
            input_state: [false; 12],
            input_history: VecDeque::with_capacity(120),
            event_log: VecDeque::with_capacity(500),
            debug_open: false,
            paused: false,
            step_one: false,
            breakpoints: Vec::new(),
            hit_breakpoint: None,
            run_to_addr: None,
            trigger_frame: None,
            trigger_pixel: None,
            pc_heatmap: HashMap::new(),
            bookmarks: Vec::new(),
            code_regions: Vec::new(),
            create_bookmark: false,
            save_regions: false,
            sidecar_path: None,
            rom_map_path: None,
            rom_name: None,
            rom_sha1: None,
            rom_size: None,
            rom_system: None,
            rom_bytes: None,
            rom_path: None,
            watches: Vec::new(),
            ram_search: RamSearch::new(),
            change_log: VecDeque::new(),
            nav: NavState::default(),
            pending_lua: None,
            pending_lua_result: None,
        }
    }

    /// THE entry point other panels call to change the shared current location.
    ///
    /// Sets the cursor to `addr`, pushes it onto the back/forward history (truncating
    /// any forward entries first, so a new jump from the middle of history discards the
    /// "forward" branch), and arms `pending_focus` so address-aware panels scroll to it
    /// on the next render.
    pub fn goto(&mut self, addr: u32) {
        self.nav.current_address = Some(addr);
        // Truncate any forward entries before appending the new location.
        if !self.nav.history.is_empty() && self.nav.history_idx + 1 < self.nav.history.len() {
            self.nav.history.truncate(self.nav.history_idx + 1);
        }
        self.nav.history.push(addr);
        self.nav.history_idx = self.nav.history.len() - 1;
        self.nav.pending_focus = Some(addr);
    }

    /// Move one step back in history. Updates the cursor + `pending_focus` from the new
    /// entry. Returns true if it moved.
    pub fn nav_back(&mut self) -> bool {
        if !self.can_go_back() {
            return false;
        }
        self.nav.history_idx -= 1;
        let addr = self.nav.history[self.nav.history_idx];
        self.nav.current_address = Some(addr);
        self.nav.pending_focus = Some(addr);
        true
    }

    /// Move one step forward in history. Updates the cursor + `pending_focus` from the
    /// new entry. Returns true if it moved.
    pub fn nav_forward(&mut self) -> bool {
        if !self.can_go_forward() {
            return false;
        }
        self.nav.history_idx += 1;
        let addr = self.nav.history[self.nav.history_idx];
        self.nav.current_address = Some(addr);
        self.nav.pending_focus = Some(addr);
        true
    }

    /// True if there is an earlier entry in history to navigate back to.
    pub fn can_go_back(&self) -> bool {
        !self.nav.history.is_empty() && self.nav.history_idx > 0
    }

    /// True if there is a later entry in history to navigate forward to.
    pub fn can_go_forward(&self) -> bool {
        !self.nav.history.is_empty() && self.nav.history_idx + 1 < self.nav.history.len()
    }

    /// Push a change event to the rolling log, capping at `CHANGE_LOG_CAP`.
    pub fn push_change(&mut self, ev: ChangeEvent) {
        if self.change_log.len() >= CHANGE_LOG_CAP {
            self.change_log.pop_front();
        }
        self.change_log.push_back(ev);
    }

    /// Read up to 4 bytes from the emulated address space, returning them as a
    /// little-endian u32. Walks `memory_regions` to find the containing region
    /// and reads through its host pointer. Returns None if no region contains
    /// the address or the host pointer is null.
    pub fn read_addr(&self, addr: usize, len: usize) -> Option<u32> {
        let len = len.min(4);
        for region in &self.memory_regions {
            // Skip regions that contain the address but whose backing memory is
            // null/too-small (libretro "virtual" descriptors) — see safe_host_ptr.
            if region.host_ptr_for_addr(addr).is_none() {
                continue;
            }
            let Some(ptr) = region.safe_host_ptr(addr, len) else {
                continue;
            };
            let mut value: u32 = 0;
            unsafe {
                for i in 0..len {
                    value |= (*ptr.add(i) as u32) << (8 * i);
                }
            }
            return Some(value);
        }
        None
    }

    /// Read a single byte from the emulated address space (convenience wrapper
    /// over `read_addr`, used by the Lua `memory.read_u8` binding).
    pub fn read_u8(&self, addr: u32) -> Option<u8> {
        self.read_addr(addr as usize, 1).map(|v| v as u8)
    }

    /// Write `len` little-endian bytes of `value` back to the emulated address
    /// space. Returns false if no region contains the address, the host pointer
    /// is null, or the containing region is read-only.
    pub fn write_addr(&self, addr: usize, len: usize, value: u32) -> bool {
        let len = len.min(4);
        for region in &self.memory_regions {
            if region.host_ptr_for_addr(addr).is_none() {
                continue;
            }
            if region.is_readonly() {
                return false;
            }
            let Some(cptr) = region.safe_host_ptr(addr, len) else {
                continue;
            };
            let ptr = cptr as *mut u8;
            {
                unsafe {
                    for i in 0..len {
                        *ptr.add(i) = ((value >> (8 * i)) & 0xFF) as u8;
                    }
                }
                return true;
            }
        }
        false
    }

    /// Reset the RAM search: enumerate every aligned address in the selected
    /// region, snapshot each value, and mark the search as started.
    pub fn reset_search(&mut self) {
        let stride = self.ram_search.size.byte_len();
        let mut candidates = Vec::new();
        let mut prev_values = Vec::new();

        if let Some(region) = self.memory_regions.get(self.ram_search.region_idx) {
            let mut addr = region.addr_start;
            while addr + stride <= region.addr_end + 1 {
                if let Some(v) = read_le(region, addr, stride) {
                    candidates.push(addr);
                    prev_values.push(v);
                }
                addr += stride;
            }
        }

        self.ram_search.candidates = candidates;
        self.ram_search.prev_values = prev_values;
        self.ram_search.started = true;
    }

    /// Run one search step, keeping only candidates that pass `compare` against
    /// `source`. Survivors' snapshots are refreshed to the just-read values so
    /// the next step compares against this checkpoint.
    pub fn step_search(&mut self, compare: SearchCompare, source: SearchSource) {
        if !self.ram_search.started {
            return;
        }
        let len = self.ram_search.size.byte_len();
        let bits = (len * 8) as u32;
        let signed = self.ram_search.signed;

        let region = match self.memory_regions.get(self.ram_search.region_idx) {
            Some(r) => r.clone(),
            None => return,
        };

        let candidates = std::mem::take(&mut self.ram_search.candidates);
        let prev_values = std::mem::take(&mut self.ram_search.prev_values);

        let mut new_candidates = Vec::new();
        let mut new_prev = Vec::new();

        for (i, &addr) in candidates.iter().enumerate() {
            let cur = match read_le(&region, addr, len) {
                Some(v) => v,
                None => continue,
            };
            let rhs = match compare {
                SearchCompare::Changed
                | SearchCompare::Unchanged
                | SearchCompare::Increased
                | SearchCompare::Decreased => prev_values[i],
                _ => match &source {
                    SearchSource::PreviousSnapshot => prev_values[i],
                    SearchSource::SpecificValue(v) => *v,
                },
            };
            if compare_passes(cur, rhs, compare, signed, bits) {
                new_candidates.push(addr);
                new_prev.push(cur);
            }
        }

        self.ram_search.candidates = new_candidates;
        self.ram_search.prev_values = new_prev;
    }

    /// Push an event to the rolling log (capped at 500 entries).
    pub fn log(&mut self, msg: String) {
        if self.event_log.len() >= 500 {
            self.event_log.pop_front();
        }
        self.event_log.push_back(format!("[{}] {}", self.frame_count, msg));
    }

    /// Update framebuffer and decode to RGBA. Called from video_callback.
    pub fn update_frame(&mut self, data: &[u8], width: u32, height: u32, pitch: usize, fmt: u32) {
        self.framebuffer.resize(data.len(), 0);
        self.framebuffer.copy_from_slice(data);
        self.fb_width = width;
        self.fb_height = height;
        self.fb_pitch = pitch;
        self.fb_fmt = fmt;
        self.fb_rgba = decode_to_rgba(data, width, height, pitch, fmt);
        self.fb_generation += 1;
        self.video_real += 1;
    }

    /// Update input history (call once per frame from the run loop).
    pub fn push_input(&mut self, state: [bool; 12], frame: u64) {
        if self.input_history.len() >= 120 {
            self.input_history.pop_front();
        }
        self.input_history.push_back((frame, state));
        self.input_state = state;
    }
}

/// Read `len` (1-4) bytes little-endian from a region at a guest address.
/// Bounds-checked via `safe_host_ptr` so unbacked/virtual descriptors (which
/// would otherwise segfault on deref) return None instead.
fn read_le(region: &MemoryRegion, addr: usize, len: usize) -> Option<u32> {
    let ptr = region.safe_host_ptr(addr, len)?;
    unsafe {
        let mut value: u32 = 0;
        for i in 0..len {
            value |= (*ptr.add(i) as u32) << (8 * i);
        }
        Some(value)
    }
}

/// Decode any libretro pixel format to packed RGBA8888 (R,G,B,A bytes).
pub fn decode_to_rgba(src: &[u8], width: u32, height: u32, pitch: usize, fmt: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0u8; w * h * 4];

    for y in 0..h {
        let row = &src[y * pitch..];
        let out_row = &mut out[y * w * 4..];
        match fmt {
            2 => {
                // RGB565
                for x in 0..w {
                    let lo = row[x * 2] as u16;
                    let hi = row[x * 2 + 1] as u16;
                    let p = lo | (hi << 8);
                    out_row[x * 4]     = (((p >> 11) & 0x1F) as u8) << 3; // R
                    out_row[x * 4 + 1] = (((p >> 5)  & 0x3F) as u8) << 2; // G
                    out_row[x * 4 + 2] = ((p & 0x1F) as u8) << 3;          // B
                    out_row[x * 4 + 3] = 0xFF;
                }
            }
            1 => {
                // XRGB8888: memory layout [B, G, R, X]
                for x in 0..w {
                    out_row[x * 4]     = row[x * 4 + 2]; // R
                    out_row[x * 4 + 1] = row[x * 4 + 1]; // G
                    out_row[x * 4 + 2] = row[x * 4];     // B
                    out_row[x * 4 + 3] = 0xFF;
                }
            }
            _ => {
                // 0RGB1555
                for x in 0..w {
                    let lo = row[x * 2] as u16;
                    let hi = row[x * 2 + 1] as u16;
                    let p = lo | (hi << 8);
                    out_row[x * 4]     = (((p >> 10) & 0x1F) as u8) << 3; // R
                    out_row[x * 4 + 1] = (((p >> 5)  & 0x1F) as u8) << 3; // G
                    out_row[x * 4 + 2] = ((p & 0x1F) as u8) << 3;          // B
                    out_row[x * 4 + 3] = 0xFF;
                }
            }
        }
    }
    out
}

/// Infer the ROM-map `system` slug (ROM_MAP_FORMAT §3 controlled vocabulary:
/// `nes` | `megadrive` | `cps2` | …) from a libretro core's `library_name`.
///
/// Only cores that map to exactly ONE system are recognized — single-system
/// cores (fceumm/nestopia/mesen → nes; genesis_plus_gx/picodrive/blastem →
/// megadrive). Multi-system cores like FBNeo/MAME run many systems, so their
/// library name alone can't pin the system; those return `None` and the scaffold
/// leaves `system` blank — an honest "human, fill this in" over a wrong guess.
///
/// Match is case-insensitive substring so version/branding suffixes don't break
/// it (e.g. "Genesis Plus GX", "Nestopia UE").
pub fn system_slug_from_library(library_name: &str) -> Option<&'static str> {
    let n = library_name.to_ascii_lowercase();
    let has = |needle: &str| n.contains(needle);

    // NES.
    if has("fceumm") || has("nestopia") || has("mesen") || has("quicknes") {
        return Some("nes");
    }
    // Sega Mega Drive / Genesis.
    if has("genesis plus") || has("genesis_plus") || has("picodrive") || has("blastem") {
        return Some("megadrive");
    }
    // Multi-system arcade cores (FBNeo/FB Alpha/MAME) and anything else: the
    // library name doesn't identify a single system — leave it for a human.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_slug_maps_single_system_cores_only() {
        // Single-system cores → confident slug (case/branding tolerant).
        assert_eq!(system_slug_from_library("FCEUmm"), Some("nes"));
        assert_eq!(system_slug_from_library("Nestopia UE"), Some("nes"));
        assert_eq!(system_slug_from_library("Mesen"), Some("nes"));
        assert_eq!(system_slug_from_library("Genesis Plus GX"), Some("megadrive"));
        assert_eq!(system_slug_from_library("PicoDrive"), Some("megadrive"));
        // Multi-system arcade cores → None (can't pin the system from the name).
        assert_eq!(system_slug_from_library("FinalBurn Neo"), None);
        assert_eq!(system_slug_from_library("FB Alpha 2012"), None);
        assert_eq!(system_slug_from_library("MAME 2003"), None);
        assert_eq!(system_slug_from_library(""), None);
    }

    fn region(name: &str, start: usize, size: usize, ptr: usize) -> MemoryRegion {
        MemoryRegion {
            name: name.into(),
            addr_start: start,
            addr_end: start + size - 1,
            size,
            flags: 0,
            ptr,
            offset: 0,
            select: 0,
            disconnect: 0,
        }
    }

    #[test]
    fn safe_host_ptr_rejects_unbacked_and_out_of_bounds() {
        // A real backing buffer.
        let buf = [1u8, 2, 3, 4];
        let p = buf.as_ptr() as usize;
        let r = region("RAM", 0x0000, 4, p);
        // In-bounds reads OK.
        assert!(r.safe_host_ptr(0x0000, 1).is_some());
        assert!(r.safe_host_ptr(0x0003, 1).is_some());
        // Reading 4 bytes from offset 3 runs past size=4 -> rejected (no segfault).
        assert!(r.safe_host_ptr(0x0003, 4).is_none());

        // A "virtual"/unbacked descriptor (null ptr) like NES NTARAM/OAM:
        // contains the address but must NOT be dereferenced.
        let virt = region("OAM", 0x80004000, 0x100, 0);
        assert!(virt.host_ptr_for_addr(0x80004000).is_some()); // address is "in" the region
        assert!(virt.safe_host_ptr(0x80004000, 1).is_none()); // but no safe read

        // A descriptor with a non-null but bogus pointer and zero size -> rejected.
        let bogus = region("Bogus", 0x6000, 0, 0xdeadbeef);
        assert!(bogus.safe_host_ptr(0x6000, 1).is_none());
    }

    #[test]
    fn synth_region_accepts_in_bounds_rejects_out_of_bounds() {
        // A real backing buffer standing in for the core's work-RAM block.
        let buf = [0xAAu8; 64];
        let p = buf.as_ptr() as usize;
        const RETRO_MEMDESC_SYSTEM_RAM: u64 = 1 << 2;

        // Mirror the fallback: System RAM at guest base 0, identity-mapped.
        let r = MemoryRegion::synth_region("System RAM (fallback)", 0, buf.len(), p, RETRO_MEMDESC_SYSTEM_RAM);
        assert_eq!(r.addr_start, 0);
        assert_eq!(r.addr_end, buf.len() - 1);
        assert_eq!(r.region_type(), "RAM");

        // In-bounds reads resolve to the real host pointer.
        assert_eq!(r.safe_host_ptr(0, 1), Some(p as *const u8));
        assert_eq!(r.safe_host_ptr(buf.len() - 1, 1), Some((p + buf.len() - 1) as *const u8));
        assert!(r.safe_host_ptr(0, buf.len()).is_some());

        // Out-of-bounds reads are refused (no segfault).
        assert!(r.safe_host_ptr(buf.len(), 1).is_none());          // past end addr
        assert!(r.safe_host_ptr(buf.len() - 1, 2).is_none());      // straddles end
        assert!(r.safe_host_ptr(0, buf.len() + 1).is_none());      // len overruns

        // A VRAM region at a high non-overlapping base also reads correctly.
        const RETRO_MEMDESC_VIDEO_RAM: u64 = 1 << 4;
        let v = MemoryRegion::synth_region("Video RAM (fallback)", 0x1000_0000, buf.len(), p, RETRO_MEMDESC_VIDEO_RAM);
        assert_eq!(v.region_type(), "VRAM");
        assert_eq!(v.safe_host_ptr(0x1000_0000, 1), Some(p as *const u8));
        assert!(v.safe_host_ptr(0, 1).is_none()); // base-0 addr not in VRAM region
    }

    #[test]
    fn region_type_name_fallback_classifies_unflagged_regions() {
        // NES cores (fceumm) publish these named regions but DON'T set the
        // RETRO_MEMDESC_* flags, so flag-only classification yields "Unmapped".
        // The name fallback should recover the intended kind. `region()` builds
        // a region with flags = 0.
        assert_eq!(region("OAM", 0, 0x100, 0).region_type(), "VRAM");
        assert_eq!(region("PALRAM", 0, 0x20, 0).region_type(), "VRAM");
        assert_eq!(region("NTARAM", 0, 0x800, 0).region_type(), "VRAM");
        assert_eq!(region("PPUREG", 0, 0x8, 0).region_type(), "VRAM");
        assert_eq!(region("Work RAM", 0, 0x800, 0).region_type(), "RAM");
        assert_eq!(region("WRAM", 0, 0x2000, 0).region_type(), "RAM");
        assert_eq!(region("PRG ROM", 0, 0x8000, 0).region_type(), "ROM");
        assert_eq!(region("Battery SRAM", 0, 0x2000, 0).region_type(), "SRAM");
        // Unrecognized name with no flags still falls through to Unmapped.
        assert_eq!(region("weird", 0, 0x10, 0).region_type(), "Unmapped");

        // Flags remain the PRIMARY signal: a flagged region classifies the same
        // as before regardless of its name (Genesis/CPS2 cores rely on this).
        const RETRO_MEMDESC_SYSTEM_RAM: u64 = 1 << 2;
        let mut flagged = region("anything", 0, 0x10, 0);
        flagged.flags = RETRO_MEMDESC_SYSTEM_RAM;
        assert_eq!(flagged.region_type(), "RAM");
    }

    /// Narrow a synthetic candidate set by applying `compare_passes` against a
    /// per-candidate snapshot, mirroring step_search's kernel without real memory.
    fn narrow(cur: &[u32], prev: &[u32], op: SearchCompare, signed: bool, bits: u32) -> Vec<usize> {
        (0..cur.len())
            .filter(|&i| compare_passes(cur[i], prev[i], op, signed, bits))
            .collect()
    }

    #[test]
    fn equal_narrows_to_matching() {
        let cur = [10, 99, 30, 99];
        let survivors: Vec<usize> = (0..4)
            .filter(|&i| compare_passes(cur[i], 30, SearchCompare::Equal, false, 8))
            .collect();
        assert_eq!(survivors, vec![2]);
    }

    #[test]
    fn changed_and_unchanged_split() {
        let prev = [1u32, 2, 3, 4];
        let cur = [1u32, 5, 3, 9];
        assert_eq!(narrow(&cur, &prev, SearchCompare::Changed, false, 8), vec![1, 3]);
        assert_eq!(narrow(&cur, &prev, SearchCompare::Unchanged, false, 8), vec![0, 2]);
    }

    #[test]
    fn increased_decreased() {
        let prev = [10u32, 10, 10];
        let cur = [11u32, 9, 10];
        assert_eq!(narrow(&cur, &prev, SearchCompare::Increased, false, 8), vec![0]);
        assert_eq!(narrow(&cur, &prev, SearchCompare::Decreased, false, 8), vec![1]);
    }

    #[test]
    fn signed_less_than_handles_high_bit() {
        assert!(compare_passes(0xFF, 0x01, SearchCompare::Less, true, 8));
        assert!(!compare_passes(0xFF, 0x01, SearchCompare::Less, false, 8));
    }

    #[test]
    fn different_by_is_symmetric() {
        assert!(compare_passes(15, 10, SearchCompare::DifferentBy(5), false, 8));
        assert!(compare_passes(10, 15, SearchCompare::DifferentBy(5), false, 8));
        assert!(!compare_passes(10, 15, SearchCompare::DifferentBy(4), false, 8));
    }

    #[test]
    fn nav_history_back_forward_and_truncate() {
        let mut ds = DebugState::new();
        assert!(!ds.can_go_back());
        assert!(!ds.can_go_forward());

        // Push 3 addresses.
        ds.goto(0x100);
        ds.goto(0x200);
        ds.goto(0x300);
        assert_eq!(ds.nav.current_address, Some(0x300));
        assert_eq!(ds.nav.history, vec![0x100, 0x200, 0x300]);
        assert_eq!(ds.nav.history_idx, 2);
        assert_eq!(ds.nav.pending_focus, Some(0x300));
        assert!(ds.can_go_back());
        assert!(!ds.can_go_forward());

        // Back twice -> 0x100.
        assert!(ds.nav_back());
        assert_eq!(ds.nav.current_address, Some(0x200));
        assert!(ds.nav_back());
        assert_eq!(ds.nav.current_address, Some(0x100));
        assert_eq!(ds.nav.pending_focus, Some(0x100));
        assert!(!ds.can_go_back());
        assert!(ds.can_go_forward());

        // Forward once -> 0x200.
        assert!(ds.nav_forward());
        assert_eq!(ds.nav.current_address, Some(0x200));
        assert_eq!(ds.nav.history_idx, 1);

        // goto a 4th address from the middle truncates the forward branch (0x300).
        ds.goto(0x400);
        assert_eq!(ds.nav.history, vec![0x100, 0x200, 0x400]);
        assert_eq!(ds.nav.history_idx, 2);
        assert_eq!(ds.nav.current_address, Some(0x400));
        assert!(!ds.can_go_forward());
    }

    #[test]
    fn detect_change_edge_logic() {
        // First sighting (no prior value) is never a change.
        assert!(!detect_change(None, 42));
        // Same value held across frames is not a change.
        assert!(!detect_change(Some(42), 42));
        // A differing value is a change.
        assert!(detect_change(Some(42), 43));
        assert!(detect_change(Some(0), 1));
    }
}
