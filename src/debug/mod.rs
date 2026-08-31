pub mod panels;
pub mod window;
pub mod dock;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

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
    #[allow(dead_code)] // libretro memory-descriptor mirror; spec field
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
/// A named window of the emulated 68k bus, snapshotted into an ordinary
/// [`MemoryRegion`] once per frame via the core's exported bus-read API
/// (`LibretroCore::sek_read_block`, fbalpha2012). This is how cores that
/// publish no libretro memory map still get live RAM/VRAM visibility: the
/// window list comes from the `<rom>.busmap.json` sidecar or the MCP
/// `map_bus_window` tool, and every existing reader (read_memory, RAM search,
/// watches, Lua) sees the snapshot as a plain region.
///
/// Only RAM-backed bus ranges should be mapped — reads of I/O handler ranges
/// can have side effects on the emulated machine.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BusWindowCfg {
    pub name: String,
    /// Guest bus address of the window's first byte. Serialized as a hex
    /// string ("0x400000"); plain JSON numbers are also accepted on load.
    #[serde(with = "hex_u32")]
    pub addr: u32,
    /// Window length in bytes (hex string or number, like `addr`).
    #[serde(with = "hex_u32")]
    pub len: u32,
    /// Refresh every N frames (1 = every frame) — the only throttle knob.
    #[serde(default = "bus_interval_default")]
    pub interval: u32,
    /// RETRO_MEMDESC_* flags for the synthesized region.
    #[serde(default = "bus_flags_default")]
    pub flags: u64,
}

fn bus_interval_default() -> u32 {
    1
}

fn bus_flags_default() -> u64 {
    crate::libretro::RETRO_MEMDESC_SYSTEM_RAM
}

/// Serde adapter: write a u32 as "0xHEX", accept "0xHEX"/"HEX" strings or
/// plain numbers on read — busmap sidecars are hand-authored and bus
/// addresses are naturally hexadecimal.
mod hex_u32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &u32, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("0x{v:X}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Num(u64),
            Str(String),
        }
        let raw = Raw::deserialize(d)?;
        let v = match raw {
            Raw::Num(n) => n,
            Raw::Str(s) => {
                let t = s.trim();
                let (digits, radix) = match t.strip_prefix("0x").or(t.strip_prefix("0X")) {
                    Some(h) => (h, 16),
                    None => (t, 16), // bare strings are hex too: "C00000"
                };
                u64::from_str_radix(digits, radix)
                    .map_err(|e| serde::de::Error::custom(format!("bad hex '{s}': {e}")))?
            }
        };
        u32::try_from(v).map_err(|_| serde::de::Error::custom(format!("0x{v:X} exceeds u32")))
    }
}

/// A save/load-state request queued for the emulation thread. Core FFI
/// (retro_serialize / retro_unserialize) may ONLY happen on the emu thread, so
/// the UI/MCP threads set [`DebugState::pending_state_op`] and the Frontend
/// drains it in `run_frame` — the same handoff pattern as `pending_bus_writes`
/// / `pending_lua`. Slot variants are resolved to
/// `<save_dir>/<rom_stem>.state<N>` by the Frontend (only it knows save_dir).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateOp {
    Save(std::path::PathBuf),
    Load(std::path::PathBuf),
    SaveSlot(u8),
    LoadSlot(u8),
}

/// Completion record for a drained [`StateOp`] (the MCP thread polls
/// [`DebugState::state_op_result`], mirroring `pending_lua_result`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateOpDone {
    /// True for a load, false for a save.
    pub loaded: bool,
    /// The resolved on-disk state file.
    pub path: std::path::PathBuf,
    /// Size of the serialized state in bytes.
    pub bytes: usize,
}

/// Recorder control one-shot (GUI → emu thread, drained by
/// `Frontend::drain_record_ops` — same handoff as [`StateOp`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordControl {
    /// Begin a fresh recording at this path (fails softly if already
    /// recording). `style` is a free-form play-style declaration ("rushdown",
    /// "zoning", …) stored in the recording's sidecars for matchup tooling.
    Start { path: std::path::PathBuf, style: Option<String> },
    /// Flush and close the active recording.
    Stop,
}

/// Identity + fit summary of the loaded shadow model, lifted from its
/// `meta.json` at load time and published for the Training panel's model card.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShadowModelInfo {
    /// Model directory basename ("goat-v2").
    pub name: String,
    /// Case count actually in cases.npz.
    pub cases: usize,
    pub rounds: Option<u64>,
    /// Fit timestamp string as written by the trainer (ISO 8601).
    pub created: Option<String>,
    /// Fit-time per-bucket decision counts — the coverage/drill signal.
    pub buckets: Vec<(String, u64)>,
}

/// What the training-mode dummy (controller port 1) does each frame.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum DummyMode {
    /// No injection — a human (or the shadow bot) drives port 1.
    #[default]
    Free,
    Stand,
    Crouch,
    /// Repeated hops (Up tapped on a half-second cadence).
    Jump,
    /// Hold away from the other fighter (blocks everything blockable).
    Block,
    /// Guard like [`Block`](Self::Block), and on each guarded contact sample
    /// the weighted punish pool (MACRO_ACTIONS §6) — needs a contact signal
    /// (`hitstun_sources` or `contact_signal`) mapped in the profile.
    BlockPunish,
}

/// WHEN the guarding dummy takes a guard opportunity (MACRO_ACTIONS §9.4 —
/// the vocabulary SF6/GGST/fbneo-training-mode all use). Orthogonal to the
/// family's guard STYLE (button chord vs reactive back-hold): the style says
/// how to guard, the mode says whether to.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum GuardMode {
    /// Guard every opportunity.
    #[default]
    All,
    /// Stand there until a contact signal fires, then guard for the rest of
    /// the string (string end = the contact signal going quiet). Needs a
    /// contact signal mapped — greyed otherwise.
    AfterFirstHit,
    /// Guard each opportunity with probability `guard_random_pct`; the roll is
    /// sticky for the whole opportunity (no mid-attack flicker).
    Random,
    /// Never guard.
    None,
}

impl GuardMode {
    pub fn label(self) -> &'static str {
        match self {
            GuardMode::All => "Guard All",
            GuardMode::AfterFirstHit => "After First Hit",
            GuardMode::Random => "Random",
            GuardMode::None => "None",
        }
    }
}

/// Training-mode control block (shadow PLAN Wave 2b). GUI hotkeys flip these
/// under the DebugState lock; `training::tick` consumes them on the emulation
/// thread. `reset_positions`/`finish_round` are one-shots (cleared by tick).
#[derive(Default)]
pub struct TrainingConfig {
    pub enabled: bool,
    pub dummy: DummyMode,
    pub refill: bool,
    pub reset_positions: bool,
    pub finish_round: bool,
    /// BlockPunish option pool: (option, weight). The panel edits it
    /// char-aware; `training::tick` samples it on each guarded contact.
    /// Weight 0 entries are dead (kept so the panel remembers the setting).
    pub punish_pool: Vec<(crate::macros::PunishOption, u8)>,
    /// BlockPunish runtime (not UI): the in-flight punish macro, contact-
    /// signal edge tracking, and the quiet-window re-arm state.
    pub punish_exec: Option<crate::macros::MacroExec>,
    pub punish_prev_signal: Option<u8>,
    pub punish_last_change: u64,
    /// Armed after the signal has been quiet ≥ HITSTUN_RECENT_FRAMES; a
    /// trigger disarms — the §6 cooldown.
    pub punish_armed: bool,
    /// ContinueBlock outcome: keep guarding (and don't re-trigger) until here.
    pub punish_hold_until: u64,
    /// Consecutive gate-closed frames survived by the in-flight punish macro
    /// (hit-freeze grace — see `training::PUNISH_GATE_GRACE`).
    pub punish_gate_grace: u64,
    /// Frames of guarding left before the scheduled punish macro starts
    /// (`training::PUNISH_DELAY` — hit-freeze + blockstun ride-out).
    pub punish_wait: u64,
    /// Human-readable BlockPunish phase, refreshed every frame the mode
    /// runs: "guarding — armed" / "cooling (Nf)" / "punishing: slide" /
    /// "unavailable …". The ONE place this is computed (panel, Lua
    /// `training.punish_state()`, and any overlay all read it) so a silent
    /// dummy explains itself instead of looking broken.
    pub punish_phase: String,
    /// WHEN the guarding dummy guards (§9.4). Applies to both guard styles.
    pub guard_mode: GuardMode,
    /// `GuardMode::Random`'s probability, in percent.
    pub guard_random_pct: GuardPct,
    /// Guard runtime (not UI): the reactive window's hold tail, the sticky
    /// Random roll for the current opportunity, and After-First-Hit's
    /// "a hit has landed in this string" latch.
    pub guard_commit_until: u64,
    pub guard_prev_commit: bool,
    pub guard_roll: Option<bool>,
    pub guard_hit_seen: bool,
    pub guard_last_hit: u64,
    /// WHEN a scheduled BlockPunish reversal starts (MACRO_ACTIONS §6's
    /// `training::PUNISH_DELAY` knob, exposed as MK-style Fast/Delay/Late/
    /// Explicit). Persisted (see [`TrainingConfig::merge_persisted`]).
    pub reversal_timing: ReversalTiming,
}

/// `GuardMode::Random`'s take-probability in percent. A newtype purely so the
/// derived `TrainingConfig::default()` yields a sane 50 % instead of "never".
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct GuardPct(pub u8);

impl Default for GuardPct {
    fn default() -> Self {
        GuardPct(50)
    }
}

/// WHEN a scheduled BlockPunish reversal starts, relative to its trigger —
/// MK11 practice mode's "Block Attack: Fast / Delay / Late" vocabulary
/// (docs/frames.md §3 calls the same idea "first possible frame", the
/// frame-measurement lab's zero point). Orthogonal to the guard STYLE/MODE:
/// this only governs the punish macro's start delay once a punish has
/// already been decided.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum ReversalTiming {
    /// First possible frame — the measured floor (`training::PUNISH_DELAY_FAST`).
    /// Not user-adjustable: a value below the floor is silently eaten by
    /// hit-freeze/blockstun (live-observed on MK2 arcade), so there is
    /// nothing lower to expose.
    Fast,
    /// Random within `[min, max]` (inclusive; order-independent — a reversed
    /// pair is swapped, not rejected), re-rolled on every scheduled punish.
    Delay { min: u64, max: u64 },
    /// The last frame that still reliably punishes
    /// (`training::PUNISH_DELAY_LATE`) — a global calibration, NOT a
    /// per-move "last safe frame" (that needs the frames.json measurement
    /// table, docs/frames.md §6, which does not exist yet).
    Late,
    /// A literal frame count — the power-user knob, unclamped.
    Explicit(u64),
}

impl Default for ReversalTiming {
    fn default() -> Self {
        // Unchanged behaviour on a fresh install: the historical fitted value.
        ReversalTiming::Explicit(crate::training::PUNISH_DELAY)
    }
}

/// The `PunishOption` pool entry can't derive Serialize/Deserialize itself
/// (it lives in `crate::macros`, outside this feature's file scope), so the
/// settings sidecar round-trips it through this tagged mirror instead of the
/// enum directly.
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind")]
enum PersistedPunishOption {
    Move { name: String },
    Attack { class: String },
    ContinueBlock { frames: u16 },
}

impl From<&crate::macros::PunishOption> for PersistedPunishOption {
    fn from(o: &crate::macros::PunishOption) -> Self {
        use crate::macros::PunishOption as P;
        match o {
            P::Move(name) => PersistedPunishOption::Move { name: name.clone() },
            P::Attack(class) => PersistedPunishOption::Attack { class: class.clone() },
            P::ContinueBlock(frames) => PersistedPunishOption::ContinueBlock { frames: *frames },
        }
    }
}

impl From<PersistedPunishOption> for crate::macros::PunishOption {
    fn from(o: PersistedPunishOption) -> Self {
        use crate::macros::PunishOption as P;
        match o {
            PersistedPunishOption::Move { name } => P::Move(name),
            PersistedPunishOption::Attack { class } => P::Attack(class),
            PersistedPunishOption::ContinueBlock { frames } => P::ContinueBlock(frames),
        }
    }
}

/// The user-facing SETTINGS subset of [`TrainingConfig`] — `enabled`,
/// `refill`, one-shots, and all punish/guard RUNTIME bookkeeping are
/// deliberately excluded (CLI-flag/hotkey-driven or session-scoped, not a
/// "preference"), mirroring how `dock::LAYOUT_PATH` only stores what the
/// user arranged, not live debugger state.
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
struct PersistedTrainingConfig {
    dummy: DummyMode,
    guard_mode: GuardMode,
    guard_random_pct: GuardPct,
    reversal_timing: ReversalTiming,
    #[serde(default)]
    punish_pool: Vec<(PersistedPunishOption, u8)>,
}

/// Cwd-relative sidecar for training-mode SETTINGS (not layout) — same
/// pattern as `dock::LAYOUT_PATH`: a fixed name in the launch directory,
/// gitignored, absent on a fresh install.
pub const TRAINING_CONFIG_PATH: &str = "rustretro_training_v1.json";

impl TrainingConfig {
    fn to_persisted(&self) -> PersistedTrainingConfig {
        PersistedTrainingConfig {
            dummy: self.dummy,
            guard_mode: self.guard_mode,
            guard_random_pct: self.guard_random_pct,
            reversal_timing: self.reversal_timing,
            punish_pool: self.punish_pool.iter().map(|(o, w)| (o.into(), *w)).collect(),
        }
    }

    fn apply_persisted(&mut self, p: PersistedTrainingConfig) {
        self.dummy = p.dummy;
        self.guard_mode = p.guard_mode;
        self.guard_random_pct = p.guard_random_pct;
        self.reversal_timing = p.reversal_timing;
        self.punish_pool = p.punish_pool.into_iter().map(|(o, w)| (o.into(), w)).collect();
    }

    /// JSON snapshot of the persisted subset, used by the Training panel to
    /// detect "did any SAVED setting actually change" without re-reading the
    /// sidecar every frame (cheap string compare against the last write).
    pub fn persisted_snapshot_json(&self) -> String {
        serde_json::to_string(&self.to_persisted()).unwrap_or_default()
    }

    /// Load [`TRAINING_CONFIG_PATH`] and overlay it onto `self`'s settings
    /// fields — leaves `enabled`/`refill`/one-shots/runtime bookkeeping
    /// untouched, so a `--training` flag or F5/F3 hotkey applied before this
    /// call is never clobbered. A missing sidecar is not an error (fresh
    /// install: `self` keeps whatever it already had, i.e. defaults, if
    /// called right after `TrainingConfig::default()`); a present-but-corrupt
    /// one logs a warning and falls back the same way — never panics.
    pub fn merge_persisted(&mut self) {
        self.merge_persisted_from(std::path::Path::new(TRAINING_CONFIG_PATH));
    }

    fn merge_persisted_from(&mut self, path: &std::path::Path) {
        let json = match std::fs::read_to_string(path) {
            Ok(j) => j,
            Err(_) => return, // no sidecar yet — keep current settings
        };
        match serde_json::from_str::<PersistedTrainingConfig>(&json) {
            Ok(p) => self.apply_persisted(p),
            Err(e) => {
                eprintln!("[training] failed to parse {}: {e}; keeping defaults", path.display());
            }
        }
    }

    /// Persist the settings subset to [`TRAINING_CONFIG_PATH`]. Errors are
    /// logged, not fatal — same posture as `dock::save_layout`.
    pub fn save(&self) {
        self.save_to(std::path::Path::new(TRAINING_CONFIG_PATH));
    }

    fn save_to(&self, path: &std::path::Path) {
        match serde_json::to_string_pretty(&self.to_persisted()) {
            Ok(json) => {
                if let Err(e) = std::fs::write(path, json) {
                    eprintln!("[training] failed to write {}: {e}", path.display());
                }
            }
            Err(e) => eprintln!("[training] failed to serialize training settings: {e}"),
        }
    }
}

// ── input-slot record/playback (task A2) ────────────────────────────────────
//
// Control/runtime state for named record/playback slots — see `playback.rs`
// for the logic (`playback::tick`, called from `training::tick` every REAL
// emulated frame) and its module doc for the determinism argument and the
// precedence rule against the training dummy. This lives on `DebugState`
// itself (not a process-wide static like `debug/panels/input_log.rs`'s ring)
// because every test builds its own isolated `DebugState`, and this feature's
// start/stop/tick MUST be exercisable test-by-test without a shared global
// leaking frames between unrelated concurrently-running tests — the same
// reasoning that put `TrainingConfig`/`RecordControl` here rather than in a
// singleton.

/// Which controller port(s) an input-slot playback drives. A RECORDING always
/// captures BOTH ports regardless of what a later playback targets (task A2
/// §1) — this enum only exists for playback's §2 "chosen port" knob.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum PlaybackPort {
    P1,
    P2,
    Both,
}

impl PlaybackPort {
    /// Whether this target includes the given zero-indexed RETRO port (0/1),
    /// matching `hold_buttons`'/`press_buttons`' port convention.
    pub fn drives(self, port: usize) -> bool {
        matches!(
            (self, port),
            (PlaybackPort::P1 | PlaybackPort::Both, 0) | (PlaybackPort::P2 | PlaybackPort::Both, 1)
        )
    }
}

impl Default for PlaybackPort {
    fn default() -> Self {
        PlaybackPort::Both
    }
}

/// WHEN an armed playback begins consuming its recorded frames (task A2 §2).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum PlaybackTrigger {
    /// Begin on the very next real emulated frame after `play_inputs`/the
    /// panel's Play is called. NOT guaranteed frame-exact across repeated
    /// runs unless the call itself is frame-pinned (e.g. issued while
    /// paused, then stepped) — see `playback.rs`'s module doc.
    Manual,
    /// Begin on the fight gate's next closed→open transition (the SAME gate
    /// `training`/`record`/Lua `game.controllable()` share). Deterministic
    /// across replays from an identical PRE-round save state. If armed while
    /// already mid-round, it waits for the NEXT round, not the current one —
    /// see `playback.rs`'s module doc for why that is the honest behaviour.
    RoundStart,
}

impl Default for PlaybackTrigger {
    fn default() -> Self {
        PlaybackTrigger::Manual
    }
}

/// An in-progress capture of both ports' folded per-frame input (task A2).
/// `None` on `DebugState::recording_slot` = not recording.
pub struct RecordingSlot {
    pub name: String,
    pub family: String,
    pub port: String,
    /// `DebugState::state_note` as of the moment recording started — best-
    /// effort provenance of what save state (if any) this was recorded
    /// against ("loaded shadow/arenas/…" or a save note, or `None` if no
    /// state op had run yet this session). A human-readable breadcrumb, not
    /// a content hash or a guarantee the file is unchanged.
    pub state_note_at_start: Option<String>,
    /// `[p1_mask, p2_mask]` per frame, RETRO_DEVICE_ID bit order
    /// (`record::pack_mask`'s layout).
    pub frames: Vec<[u16; 2]>,
}

/// An in-progress (or armed) replay of a loaded slot (task A2). `None` on
/// `DebugState::playback_slot` = nothing playing.
pub struct PlaybackSlot {
    pub name: String,
    pub port: PlaybackPort,
    pub trigger: PlaybackTrigger,
    pub frames: Vec<[u16; 2]>,
    /// True once the trigger has fired and frames are actively being played.
    /// While `false` (armed, waiting), the training dummy is UNAFFECTED —
    /// the precedence rule only suppresses the dummy once playback is
    /// actually asserting bits (see `playback::active_on_port`).
    pub started: bool,
    /// Index of the NEXT frame to play.
    pub idx: usize,
    /// True once every frame has been played; `playback::tick` then releases
    /// both driven ports and clears this slot on the same tick.
    pub done: bool,
    /// `RoundStart` edge-detection: the gate's value as of the last tick this
    /// playback was armed-but-not-started (`None` until first observed —
    /// the first observation only seeds the baseline, it can never itself be
    /// "the start", so arming mid-round correctly waits for the NEXT round).
    pub gate_baseline: Option<bool>,
}

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

    // --- Bus windows (Sek snapshot bridge) ---
    /// Installed bus windows, index-aligned with `bus_buffers`. Windows are
    /// only ever appended (no replace/remove) — see `bus_buffers`.
    pub bus_windows: Vec<BusWindowCfg>,
    /// Owned snapshot buffers backing the bus-window regions. APPEND-ONLY for
    /// the life of the process: MCP readers clone a `MemoryRegion` and deref
    /// its `ptr` after dropping this state's lock, so a published buffer must
    /// never be freed or resized.
    pub bus_buffers: Vec<Box<[u8]>>,
    /// Windows added by the MCP thread, waiting for the emulation thread to
    /// allocate and install them (same handoff pattern as `save_regions`).
    pub pending_bus_windows: Vec<BusWindowCfg>,
    /// Bus-window writes queued for the emulation thread to push to the live 68k
    /// bus via `sek_write_block` (DebugState can't reach the core). Same handoff
    /// pattern as `pending_bus_windows`; each entry is (guest addr, LE bytes).
    pub pending_bus_writes: Vec<(u32, Vec<u8>)>,
    /// Signal: persist `bus_windows` to the busmap sidecar on the emu thread.
    pub save_busmap: bool,

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
    /// Port-1 (P2) counterpart of [`input_state`](Self::input_state), mirrored
    /// from the frontend each frame so `input.get(1)` in Lua is a cheap read.
    pub input_state2: [bool; 12],
    /// Rolling history: (frame_number, button_states).
    pub input_history: VecDeque<(u64, [bool; 12])>,

    // --- Event log ---
    /// Rolling log of notable events (env callbacks, AV changes, etc.).
    pub event_log: VecDeque<String>,

    // --- Control flags (written by debug window, read by emulation loop) ---
    pub debug_open: bool,
    pub paused: bool,
    pub step_one: bool,
    /// `run_frames`' N-deep counterpart to `step_one`: while > 0, `Frontend::
    /// run_frame` bypasses `paused` for exactly one frame (same as `step_one`)
    /// and decrements this instead of clearing a bool, so a whole batch runs
    /// through the identical one-frame-at-a-time pause bypass `step` already
    /// used — just driven by the emulation thread across many of its own
    /// iterations instead of by N separate MCP round trips.
    pub step_batch_remaining: u32,
    /// Incremented by `Frontend::run_frame` exactly once per frame that ran
    /// all the way to completion (full post-processing done, right before
    /// `run_frame` returns) — never on the early-return-because-paused path.
    /// `step`/`run_frames` wait on `frame_cv` for this to advance rather than
    /// polling `frame_count`: waking here means a specific frame's core.run
    /// *and* everything `run_frame` does after it (bus-window refresh, state
    /// ops, training/shadow ticks, recording) already happened, which is the
    /// honest definition of "the frame finished" (docs/frames.md §3 precondition 6 ("let the frame finish")).
    pub step_generation: u64,
    /// Condvar paired with the outer `Mutex<DebugState>` (there is only ever
    /// one such mutex per session) and signaled alongside `step_generation`
    /// AND [`fold_generation`](Self::fold_generation) — one condvar, two
    /// independent counters, each waiter re-checks its own predicate on wake.
    /// `Arc`-wrapped so a waiter can clone it out of the very guard it is
    /// about to move into `wait_timeout` — cloning first avoids borrowing
    /// `guard.frame_cv` and moving `guard` in the same expression.
    pub frame_cv: Arc<Condvar>,
    /// Incremented by the host loop's input-fold site — `main.rs`'s headless
    /// loop step (a0) and the windowed `read_input` Bevy system — exactly
    /// once per fold, i.e. once per call to `take_injected_input`/
    /// `take_injected_input2` for that tick, and notified on `frame_cv`
    /// alongside the bump. This is a DIFFERENT event than `step_generation`:
    /// the fold runs on EVERY host-loop tick regardless of `paused`, while
    /// `step_generation` only advances for a frame that actually ran
    /// `core.run()`. `run_frames` waits on this to confirm a fold has
    /// observed newly-set `held_input`/`held_input2` BEFORE arming
    /// `step_batch_remaining` — see `RetroMcpServer::run_frames`'s doc for
    /// the race this closes (two lock acquisitions where one ordering
    /// guarantee was needed).
    pub fold_generation: u64,
    /// MCP-injected controller input for port 0: per-button countdown of frames
    /// still to hold (index = RETRO_DEVICE_ID_JOYPAD: 0=B 1=Y 2=Select 3=Start
    /// 4=Up 5=Down 6=Left 7=Right 8=A 9=X 10=L 11=R). The emulation loop calls
    /// [`take_injected_input`](Self::take_injected_input) each frame to fold these
    /// into the controller and decrement them. Lets an agent drive the game
    /// (menus, moves) in headless mode where there's no keyboard.
    pub injected_input: [u16; 12],
    /// Same as [`injected_input`](Self::injected_input) but for controller port 1
    /// (P2), consumed via [`take_injected_input2`](Self::take_injected_input2).
    /// Drives the second fighter slot (e.g. the shadow bot / a training dummy).
    pub injected_input2: [u16; 12],
    /// HELD injection for port 0 — asserted on EVERY fold until explicitly
    /// released (MCP `release_buttons` / Lua `input.release`), independent of
    /// [`injected_input`](Self::injected_input)'s countdown. The countdown
    /// idiom (`press_buttons(frames=N)`) does not reliably sustain a continuous
    /// hold: it decrements on every fold INCLUDING GUI frames while the
    /// emulation is paused, so it can drain to zero before a paused
    /// pause→step sequence ever consumes it, and a long single-shot hold can
    /// read as a drop-then-reassert to game logic that needs a true hold (e.g.
    /// guard checks). `held_input` never decrements — it is OR'd into the
    /// countdown's output at fold time ([`take_injected_input`]) and stays
    /// asserted across any number of takes.
    pub held_input: [bool; 12],
    /// Port-1 (P2) counterpart of [`held_input`](Self::held_input).
    pub held_input2: [bool; 12],
    /// What `Frontend::run_frame` fed the core on the LAST frame that
    /// actually ran `core.run()` — set atomically, inside the SAME lock
    /// acquisition that decides a frame will run (not a later one), from
    /// `callback_context.input_state`. Unlike [`input_state`](Self::input_state)
    /// (overwritten every host-loop TICK, executed or not — see its own
    /// doc), this is sticky: it changes only when a real frame ran, so it
    /// cannot be raced back to "correct" by a later non-executing tick's
    /// re-fold before an observer reads it. Exists so `get_input`/tests can
    /// observe exactly what an executed frame saw, closing the observability
    /// gap that made task F4's `run_frames` mask race hard to catch from
    /// outside (two independent observables — contact frame, `input_state`
    /// polling — both agreed with each other on the wrong answer).
    pub last_executed_input: [bool; 12],
    /// Port-1 (P2) counterpart of [`last_executed_input`](Self::last_executed_input).
    pub last_executed_input2: [bool; 12],
    /// Training-mode controls (shadow PLAN Wave 2b) — flipped by `--training`
    /// and hotkeys in the GUI, consumed by `training::tick` each frame.
    pub training: TrainingConfig,
    /// Gate for the Lua `memory.writebyte`/`memory.writeword` bindings. Scripts
    /// are user-authored, but memory pokes can corrupt a session, so they are
    /// opt-in: defaults to TRUE when launched with `--training` (a training
    /// session exists to poke RAM), FALSE otherwise; the MCP `enable_writes` /
    /// `disable_writes` tools also arm/lock it (one write switch for the whole
    /// app). A blocked Lua write raises an error naming this gate.
    pub lua_writes_enabled: bool,

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

    // --- Save states ---
    /// Save/load-state request queued for the emulation thread (hotkeys, the
    /// --load-state flag, and the MCP save_state/load_state tools all set this;
    /// `Frontend::drain_state_op` resolves slots and performs the core FFI).
    pub pending_state_op: Option<StateOp>,
    /// Set alongside `pending_state_op` (same lock acquisition) when the
    /// caller wants load-and-pause to be ATOMIC: on a successfully drained
    /// LOAD, `Frontend::drain_state_op` forces `paused = true` in the same
    /// critical section that publishes `state_op_result`, instead of the
    /// caller racing to pause afterwards (`docs/frames.md` §4.6 — the
    /// `resume → load → poll → pause` protocol measured a variable 14/15/17
    /// free frames because those are three separate round trips, each
    /// racing an uncapped emu loop). Ignored for saves and for a failed
    /// load. Always consumed (reset to `false`) by the same drain that
    /// takes `pending_state_op`, so a later plain load/save never
    /// accidentally inherits a stale request.
    pub pending_state_op_pause_after: bool,
    /// Result of the most recently drained `pending_state_op`: `Ok(done)` or
    /// `Err(message)`. The MCP thread polls this and clears it on read.
    pub state_op_result: Option<Result<StateOpDone, String>>,
    /// Sticky one-line description of the last state op ("saved …",
    /// "load FAILED: …") for the State panel. Unlike `state_op_result` it is
    /// never consumed, so the GUI can't race the MCP poller for it.
    pub state_note: Option<String>,
    /// Where slot save-state files live (published by the Frontend, which is
    /// the only thing that knows `save_dir`); lets the State panel stat slot
    /// files. Slot path = `state_dir/<rom_name>.state<N>`.
    pub state_dir: Option<std::path::PathBuf>,

    // --- Shadow bot GUI bridge ---
    /// One-shot request to toggle the shadow bot (GUI panel → emu thread;
    /// equivalent to Shift+F5, drained by `Frontend::drain_shadow_ops`).
    pub pending_shadow_toggle: bool,
    /// Shadow bot status published by the Frontend: `None` = no model loaded
    /// (`--shadow` absent), `Some(enabled)` otherwise.
    pub shadow_on: Option<bool>,
    /// One-shot request to (re)load a shadow model directory at runtime.
    /// Unlike the fatal `--shadow` startup path, a failed load here becomes a
    /// `shadow_note` and the previous model (if any) keeps running.
    pub pending_shadow_load: Option<std::path::PathBuf>,
    /// The loaded model's identity card (published on set/load).
    pub shadow_model: Option<ShadowModelInfo>,
    /// Sticky one-line result of the last shadow load ("loaded goat-v3 …" /
    /// "load FAILED: …") — GUI-facing, never consumed.
    pub shadow_note: Option<String>,
    /// Consumable twin of `shadow_note` for the MCP `load_shadow` roundtrip
    /// (mirrors the `state_op_result` / `state_note` split).
    pub shadow_load_result: Option<Result<String, String>>,

    // --- Recorder GUI bridge ---
    /// Recorder start/stop one-shot (drained by `Frontend::drain_record_ops`).
    pub pending_record: Option<RecordControl>,
    /// Active recording published every frame: (path, frames written so far).
    /// `None` = not recording.
    pub record_status: Option<(std::path::PathBuf, u64)>,
    /// Sticky one-line result of the last start/stop ("recording …" /
    /// "stopped — N frames").
    pub record_note: Option<String>,

    // --- Input-slot record/playback (task A2) ---
    /// Active capture; `playback::start_recording`/`stop_recording` set/clear
    /// this directly under the shared lock — no cross-thread queue is needed
    /// (unlike `pending_record` above) because `playback::tick` runs on the
    /// emulation thread already, via `training::tick`, every real frame.
    pub recording_slot: Option<RecordingSlot>,
    pub recording_note: Option<String>,
    /// Active or armed replay; `None` = nothing playing.
    pub playback_slot: Option<PlaybackSlot>,
    pub playback_note: Option<String>,

    // --- Input descriptors (core-provided per-game button names) ---
    /// `input_descriptors[port][retro_id]` = the core's label for that button
    /// in THIS game (RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS), e.g. FBNeo's
    /// "Weak attack". Feeds the action-vocabulary resolver + Input monitor.
    pub input_descriptors: [[Option<String>; 12]; 2],

    // --- Help panel ---
    /// Human-readable ACTIVE input bindings (label, mapping) published once at
    /// startup from the resolved `InputConfig` — see `input_config::summary`.
    pub keymap_lines: Vec<(String, String)>,
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
            bus_windows: Vec::new(),
            bus_buffers: Vec::new(),
            pending_bus_windows: Vec::new(),
            pending_bus_writes: Vec::new(),
            save_busmap: false,
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
            frame_count: 0,
            video_frames: 0,
            video_real: 0,
            fps: 60.0,
            av_width: 0,
            av_height: 0,
            input_state: [false; 12],
            input_state2: [false; 12],
            input_history: VecDeque::with_capacity(120),
            event_log: VecDeque::with_capacity(500),
            debug_open: false,
            paused: false,
            step_one: false,
            step_batch_remaining: 0,
            step_generation: 0,
            frame_cv: Arc::new(Condvar::new()),
            fold_generation: 0,
            injected_input: [0; 12],
            injected_input2: [0; 12],
            held_input: [false; 12],
            held_input2: [false; 12],
            last_executed_input: [false; 12],
            last_executed_input2: [false; 12],
            // Deliberately `default()`, not `merge_persisted()`: this
            // constructor is also every test's "give me a blank DebugState",
            // and a real settings sidecar sitting in the test process's cwd
            // would make test behaviour depend on the machine it runs on.
            // The Training panel merges the sidecar in on its first render
            // instead (see `panels/training.rs`'s `settings_loaded`), which
            // is also the only place the settings can change.
            training: TrainingConfig::default(),
            lua_writes_enabled: false,
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
            pending_state_op: None,
            pending_state_op_pause_after: false,
            state_op_result: None,
            state_note: None,
            state_dir: None,
            pending_shadow_toggle: false,
            shadow_on: None,
            pending_shadow_load: None,
            shadow_model: None,
            shadow_note: None,
            shadow_load_result: None,
            pending_record: None,
            record_status: None,
            record_note: None,
            recording_slot: None,
            recording_note: None,
            playback_slot: None,
            playback_note: None,
            keymap_lines: Vec::new(),
            input_descriptors: Default::default(),
        }
    }

    /// Install a bus window: allocate its stable snapshot buffer and publish a
    /// [`MemoryRegion`] over it at the window's guest address. The buffer is
    /// zero-filled until the emulation thread's first refresh. Rejects (false)
    /// a name that's already taken by any region — windows are append-only, so
    /// there is no replace path for a colliding name to mean.
    pub fn install_bus_window(&mut self, cfg: BusWindowCfg) -> bool {
        if cfg.len == 0 || self.memory_regions.iter().any(|r| r.name == cfg.name) {
            return false;
        }
        let buf: Box<[u8]> = vec![0u8; cfg.len as usize].into_boxed_slice();
        let ptr = buf.as_ptr() as usize;
        self.bus_buffers.push(buf);
        self.memory_regions.push(MemoryRegion::synth_region(
            cfg.name.clone(),
            cfg.addr as usize,
            cfg.len as usize,
            ptr,
            cfg.flags,
        ));
        self.bus_windows.push(cfg);
        true
    }

    /// Whether any region came from somewhere other than the bus-window bridge.
    /// The SET_MEMORY_MAPS fallback uses this instead of `memory_regions.is_empty()`
    /// so installed bus windows don't suppress a core's own (better) map.
    pub fn has_non_bus_regions(&self) -> bool {
        self.memory_regions
            .iter()
            .any(|r| !self.bus_windows.iter().any(|w| w.name == r.name))
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
    ///
    /// For a **bus-window** region (Sek snapshot bridge) the host pointer is an
    /// owned snapshot Box that `refresh_bus_windows` overwrites every frame, so a
    /// write there alone would be a no-op. We still poke the Box (so a same-lock
    /// `read_addr` sees the value this frame) AND queue the real write in
    /// `pending_bus_writes` for the emulation thread to push to the live 68k bus
    /// via `sek_write_block` — DebugState has no handle to the core itself.
    pub fn write_addr(&mut self, addr: usize, len: usize, value: u32) -> bool {
        let len = len.min(4);
        // Resolve the region + whether it's a bus window, copying what we need
        // into locals so the immutable region borrow ends before the &mut push.
        let mut hit: Option<(*mut u8, bool)> = None;
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
            let is_bus = self.bus_windows.iter().any(|w| w.name == region.name);
            hit = Some((cptr as *mut u8, is_bus));
            break;
        }
        let Some((ptr, is_bus)) = hit else {
            return false;
        };
        let mut bytes = [0u8; 4];
        unsafe {
            for k in 0..len {
                let b = ((value >> (8 * k)) & 0xFF) as u8;
                *ptr.add(k) = b;
                bytes[k] = b;
            }
        }
        if is_bus {
            self.pending_bus_writes.push((addr as u32, bytes[..len].to_vec()));
        }
        true
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

    /// Fold the MCP-injected input into a controller bitmap for THIS frame:
    /// decrement the per-button countdown (a button is "held" while its counter
    /// is > 0) and OR in [`held_input`](Self::held_input), which never
    /// decrements. Call once per frame from the run loop; returns the 12-button
    /// state to hand to the core (OR it with any keyboard input in GUI mode).
    pub fn take_injected_input(&mut self) -> [bool; 12] {
        let mut out = [false; 12];
        for i in 0..12 {
            if self.injected_input[i] > 0 {
                out[i] = true;
                self.injected_input[i] -= 1;
            }
            out[i] |= self.held_input[i];
        }
        out
    }

    /// Port-1 (P2) counterpart of [`take_injected_input`](Self::take_injected_input).
    pub fn take_injected_input2(&mut self) -> [bool; 12] {
        let mut out = [false; 12];
        for i in 0..12 {
            if self.injected_input2[i] > 0 {
                out[i] = true;
                self.injected_input2[i] -= 1;
            }
            out[i] |= self.held_input2[i];
        }
        out
    }

    /// Peek at what the NEXT [`take_injected_input`](Self::take_injected_input)
    /// fold would assert for `port` (0/1), WITHOUT consuming anything: countdown
    /// entries still > 0, OR'd with the held mask. Used by the MCP `get_input`
    /// tool to report "what's currently queued" alongside `input_state`/
    /// `input_state2` ("what the game actually received on the last fold").
    pub fn peek_injected_input(&self, port: usize) -> [bool; 12] {
        let (countdown, held) = if port == 1 {
            (&self.injected_input2, &self.held_input2)
        } else {
            (&self.injected_input, &self.held_input)
        };
        let mut out = [false; 12];
        for i in 0..12 {
            out[i] = countdown[i] > 0 || held[i];
        }
        out
    }

    /// Replace the HELD set for `port` (0/1) with `bits` wholesale — the MCP
    /// `hold_buttons` tool and Lua `input.hold` are idempotent: calling again
    /// with a different set simply replaces the previous one, it does not OR
    /// with it. Held buttons are asserted on every fold until released; see
    /// [`held_input`](Self::held_input) for why this differs from the
    /// countdown path.
    pub fn set_held_input(&mut self, port: usize, bits: [bool; 12]) {
        let arr = if port == 1 { &mut self.held_input2 } else { &mut self.held_input };
        *arr = bits;
    }

    /// Clear buttons from `port`'s held set: the buttons at `only`'s indices,
    /// or the whole set when `only` is `None`. Used by MCP `release_buttons`
    /// (bare call = release all) and Lua `input.release`.
    pub fn clear_held_input(&mut self, port: usize, only: Option<&[usize]>) {
        let arr = if port == 1 { &mut self.held_input2 } else { &mut self.held_input };
        match only {
            Some(idxs) => {
                for &i in idxs {
                    if i < 12 {
                        arr[i] = false;
                    }
                }
            }
            None => *arr = [false; 12],
        }
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
    fn write_addr_routes_bus_window_to_pending_and_pokes_snapshot() {
        let mut ds = DebugState::new();
        // A Work-RAM bus window (Sek snapshot bridge).
        assert!(ds.install_bus_window(BusWindowCfg {
            name: "Work RAM".into(),
            addr: 0x400000,
            len: 0x1000,
            interval: 1,
            flags: crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        }));
        // Write 0xDEADBEEF (LE bytes EF BE AD DE) at base+0x100.
        let a = 0x400100usize;
        assert!(ds.write_addr(a, 4, 0xDEAD_BEEF));
        // Queued for the live 68k bus, exact LE byte order.
        assert_eq!(
            ds.pending_bus_writes,
            vec![(a as u32, vec![0xEF, 0xBE, 0xAD, 0xDE])]
        );
        // Snapshot Box poked too, so a same-lock read sees it this frame.
        assert_eq!(ds.read_addr(a, 4), Some(0xDEAD_BEEF));

        // A non-bus synth region must NOT enqueue — it pokes its host buffer.
        let mut buf = [0u8; 16];
        let p = buf.as_mut_ptr() as usize;
        ds.memory_regions.push(MemoryRegion::synth_region(
            "Plain",
            0x900000,
            buf.len(),
            p,
            crate::libretro::RETRO_MEMDESC_SYSTEM_RAM,
        ));
        let before = ds.pending_bus_writes.len();
        assert!(ds.write_addr(0x900000, 2, 0x1234));
        assert_eq!(
            ds.pending_bus_writes.len(),
            before,
            "non-bus write must not enqueue a bus write"
        );
        assert_eq!(buf[0], 0x34);
        assert_eq!(buf[1], 0x12);
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
    fn held_input_persists_while_countdown_expires() {
        let mut ds = DebugState::new();
        ds.injected_input[3] = 2; // start: 2-frame countdown
        ds.held_input[7] = true; // right: held
        // Both asserted while the countdown is still alive.
        let f = ds.take_injected_input();
        assert!(f[3] && f[7]);
        let f = ds.take_injected_input();
        assert!(f[3] && f[7]);
        // Countdown has now expired; held keeps asserting across MANY more
        // takes (simulating GUI frames folded while paused) with no decay.
        for _ in 0..50 {
            let f = ds.take_injected_input();
            assert!(!f[3], "countdown must expire and stay released");
            assert!(f[7], "held must persist indefinitely until released");
        }
        assert_eq!(ds.injected_input[3], 0);
        assert!(ds.held_input[7], "held_input itself is never decremented");
    }

    #[test]
    fn held_input_ports_are_independent_and_replace_not_or() {
        let mut ds = DebugState::new();
        ds.set_held_input(0, {
            let mut b = [false; 12];
            b[7] = true; // right
            b
        });
        ds.set_held_input(1, {
            let mut b = [false; 12];
            b[6] = true; // left
            b
        });
        assert!(ds.take_injected_input()[7]);
        assert!(!ds.take_injected_input()[6]);
        assert!(!ds.take_injected_input2()[7]);
        assert!(ds.take_injected_input2()[6]);

        // Replacing port 0's held set drops the old button (not OR'd).
        ds.set_held_input(0, {
            let mut b = [false; 12];
            b[4] = true; // up
            b
        });
        let f = ds.take_injected_input();
        assert!(f[4] && !f[7], "set_held_input replaces, it does not OR");
    }

    #[test]
    fn clear_held_input_releases_named_or_all() {
        let mut ds = DebugState::new();
        ds.held_input[7] = true; // right
        ds.held_input[4] = true; // up
        ds.clear_held_input(0, Some(&[7]));
        let f = ds.take_injected_input();
        assert!(!f[7] && f[4], "only `right` released");
        ds.clear_held_input(0, None);
        let f = ds.take_injected_input();
        assert!(!f[4], "bare release clears the whole port");
    }

    #[test]
    fn peek_injected_input_does_not_consume() {
        let mut ds = DebugState::new();
        ds.injected_input[3] = 2; // countdown
        ds.held_input2[7] = true; // held, port 1
        // Peeking repeatedly must not decrement the countdown or clear held.
        for _ in 0..5 {
            assert!(ds.peek_injected_input(0)[3]);
            assert!(ds.peek_injected_input(1)[7]);
        }
        assert_eq!(ds.injected_input[3], 2, "peek must not decrement");
        // A real take still sees the untouched countdown.
        assert!(ds.take_injected_input()[3]);
        assert_eq!(ds.injected_input[3], 1);
    }

    /// Round-trip the SETTINGS subset (dummy/guard/reversal-timing/punish
    /// pool) through the sidecar file, and confirm the excluded runtime
    /// fields (enabled, punish_armed) are NOT part of what comes back —
    /// they must stay whatever the loading `TrainingConfig` already had.
    #[test]
    fn training_config_settings_round_trip() {
        let path = std::env::temp_dir()
            .join(format!("rustretro_training_test_{}_roundtrip.json", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let mut cfg = TrainingConfig::default();
        cfg.dummy = DummyMode::BlockPunish;
        cfg.guard_mode = GuardMode::Random;
        cfg.guard_random_pct = GuardPct(73);
        cfg.reversal_timing = ReversalTiming::Delay { min: 10, max: 40 };
        cfg.punish_pool = vec![
            (crate::macros::PunishOption::Move("slide".into()), 3),
            (crate::macros::PunishOption::ContinueBlock(30), 1),
        ];
        // Distinctive runtime state that must NOT round-trip.
        cfg.enabled = true;
        cfg.punish_armed = true;
        cfg.save_to(&path);

        let mut loaded = TrainingConfig::default();
        loaded.merge_persisted_from(&path);

        assert_eq!(loaded.dummy, DummyMode::BlockPunish);
        assert_eq!(loaded.guard_mode, GuardMode::Random);
        assert_eq!(loaded.guard_random_pct, GuardPct(73));
        assert_eq!(loaded.reversal_timing, ReversalTiming::Delay { min: 10, max: 40 });
        assert_eq!(loaded.punish_pool, cfg.punish_pool);
        assert!(!loaded.enabled, "enabled is not part of the persisted settings subset");
        assert!(!loaded.punish_armed, "runtime bookkeeping must not be persisted");

        let _ = std::fs::remove_file(&path);
    }

    /// A missing sidecar (fresh install) is not an error: `merge_persisted`
    /// leaves the config exactly as it found it.
    #[test]
    fn training_config_missing_sidecar_keeps_current_settings() {
        let path = std::env::temp_dir()
            .join(format!("rustretro_training_test_{}_missing.json", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let mut cfg = TrainingConfig::default();
        cfg.merge_persisted_from(&path);
        assert_eq!(cfg.dummy, DummyMode::Free);
        assert_eq!(cfg.reversal_timing, ReversalTiming::default());
    }

    /// A corrupt/unreadable sidecar must fall back to whatever the config
    /// already had (defaults, in the real startup path) instead of panicking.
    #[test]
    fn training_config_corrupt_sidecar_falls_back_without_panicking() {
        let path = std::env::temp_dir()
            .join(format!("rustretro_training_test_{}_corrupt.json", std::process::id()));
        std::fs::write(&path, b"{ this is not valid json").unwrap();

        let mut cfg = TrainingConfig::default();
        cfg.dummy = DummyMode::Crouch; // distinctive pre-existing value
        cfg.merge_persisted_from(&path); // must not panic
        assert_eq!(cfg.dummy, DummyMode::Crouch, "corrupt sidecar must not clobber current settings");

        let _ = std::fs::remove_file(&path);
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
