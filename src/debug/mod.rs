pub mod panels;
pub mod window;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub type SharedDebugState = Arc<Mutex<DebugState>>;

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

    // --- Triggers ---
    pub trigger_frame: Option<u64>,
    pub trigger_pixel: Option<(u32, u32)>,
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
            trigger_frame: None,
            trigger_pixel: None,
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
