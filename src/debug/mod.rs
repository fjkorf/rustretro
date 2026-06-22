pub mod panels;
pub mod window;

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
    /// Compute host pointer for an emulated address within this region.
    pub fn host_ptr_for_addr(&self, emu_addr: usize) -> Option<usize> {
        if emu_addr < self.addr_start || emu_addr > self.addr_end {
            return None;
        }
        // Formula from libretro spec:
        // host_addr = ptr + offset + (emu_addr & ~disconnect) - start
        Some(self.ptr + self.offset + ((emu_addr & !self.disconnect) - self.addr_start))
    }

    /// Get region type as human-readable string.
    pub fn region_type(&self) -> &'static str {
        const RETRO_MEMDESC_CONST: u64 = 1 << 0;
        const RETRO_MEMDESC_SYSTEM_RAM: u64 = 1 << 2;
        const RETRO_MEMDESC_SAVE_RAM: u64 = 1 << 3;
        const RETRO_MEMDESC_VIDEO_RAM: u64 = 1 << 4;

        if self.flags & RETRO_MEMDESC_VIDEO_RAM != 0 { "VRAM" }
        else if self.flags & RETRO_MEMDESC_SAVE_RAM != 0 { "SRAM" }
        else if self.flags & RETRO_MEMDESC_SYSTEM_RAM != 0 { "RAM" }
        else if self.flags & RETRO_MEMDESC_CONST != 0 { "ROM" }
        else { "Unmapped" }
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
}

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
        }
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
