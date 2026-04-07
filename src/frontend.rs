use crate::debug::{SharedDebugState, DebugState};
use crate::debug::window as debug_window;
use crate::libretro::*;
use crate::sdl_interface::{Audio, Graphics, Input};
use anyhow::{anyhow, Result};
use std::ffi::{CString, c_uint, c_void};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, atomic::{AtomicPtr, Ordering}};
use std::time::Instant;

// Global static for callback context access during libretro callbacks
static CALLBACK_CONTEXT: AtomicPtr<CallbackContext> = AtomicPtr::new(std::ptr::null_mut());

pub struct Frontend {
    sdl_context: sdl2::Sdl,
    core: RetroCore,
    graphics: Graphics,
    audio: Option<Audio>,
    input: Input,
    enable_audio: bool,
    av_info: Option<RetroSystemAVInfo>,
    callback_context: Box<CallbackContext>,
    _game_path_cstring: Option<CString>,
    frame_count: u64,
    debug_state: SharedDebugState,
    debug_spawned: bool,
}

impl Frontend {
    pub fn new(
        core_path: &str,
        rom_path: &str,
        save_dir: PathBuf,
        system_dir: PathBuf,
        scale: u32,
        fullscreen: bool,
        enable_audio: bool,
    ) -> Result<Self> {
        let core = RetroCore::load(core_path)
            .map_err(|e| anyhow!("Failed to load core: {}", e))?;

        let sdl_context = sdl2::init().map_err(|e| anyhow!(e))?;

        // Placeholder window — will be resized once AV info is known
        let graphics = Graphics::new(&sdl_context, 640, 480, scale, fullscreen)?;

        let audio = if enable_audio {
            Some(Audio::new(&sdl_context, 48000.0)?)
        } else {
            None
        };

        let input = Input::new();

        let system_info = core
            .get_system_info()
            .map_err(|e| anyhow!("Failed to get system info: {}", e))?;

        eprintln!("Core: {} v{}", system_info.library_name, system_info.library_version);
        eprintln!("Valid extensions: {}", system_info.valid_extensions);

        let debug_state = Arc::new(Mutex::new(DebugState::new()));
        let callback_context = Box::new(CallbackContext::new(save_dir, system_dir, Arc::clone(&debug_state)));

        let mut frontend = Frontend {
            sdl_context,
            core,
            graphics,
            audio,
            input,
            enable_audio,
            av_info: None,
            callback_context,
            _game_path_cstring: None,
            frame_count: 0,
            debug_state,
            debug_spawned: false,
        };

        frontend.setup_callbacks()?;
        frontend
            .core
            .init()
            .map_err(|e| anyhow!("Failed to initialize core: {}", e))?;

        let rom_data = if system_info.need_fullpath {
            Vec::new()
        } else {
            std::fs::read(rom_path).map_err(|e| anyhow!("Failed to read ROM: {}", e))?
        };

        let path_cstring = CString::new(rom_path).ok();

        let game_info = RetroGameInfo {
            path: rom_path.to_string(),
            data: rom_data,
            path_cstring: path_cstring.clone(),
        };

        frontend
            .core
            .load_game(&game_info)
            .map_err(|e| anyhow!("Failed to load game: {}", e))?;

        frontend._game_path_cstring = path_cstring;

        // Query AV info now that game is loaded
        if let Ok(av_info) = frontend.core.get_av_info() {
            let w = av_info.geometry.base_width;
            let h = av_info.geometry.base_height;
            let sr = av_info.timing.sample_rate;
            eprintln!(
                "AV info: {}x{} @ {:.2} FPS, {:.0} Hz audio",
                w, h, av_info.timing.fps, sr
            );
            frontend.graphics.resize_window(w, h);

            // Reinitialize audio with correct sample rate (guard against 0)
            if enable_audio {
                let effective_rate = if sr > 0.0 { sr } else { 48000.0 };
                frontend.audio =
                    Some(Audio::new(&frontend.sdl_context, effective_rate)?);
            }
            frontend.av_info = Some(av_info);
        }

        Ok(frontend)
    }

    fn setup_callbacks(&mut self) -> Result<()> {
        let ctx_ptr = &mut *self.callback_context as *mut CallbackContext;
        CALLBACK_CONTEXT.store(ctx_ptr, Ordering::SeqCst);

        self.core
            .set_callbacks(
                static_environment_callback,
                static_video_callback,
                static_input_poll_callback,
                static_input_state_callback,
                static_audio_callback,
                static_audio_batch_callback,
            )
            .map_err(|e| anyhow!("Failed to set callbacks: {}", e))?;

        Ok(())
    }

    pub fn run(&mut self) -> Result<()> {
        let mut event_pump = self
            .sdl_context
            .event_pump()
            .map_err(|e| anyhow!(e))?;

        let mut last_frame = Instant::now();

        loop {
            // --- Event polling ---
            let mut should_quit = false;
            for event in event_pump.poll_iter() {
                if self.input.handle_event(&event) {
                    should_quit = true;
                }
            }
            if should_quit {
                break;
            }

            // F12: toggle debug window
            if self.input.f12_pressed {
                self.input.f12_pressed = false;
                if !self.debug_spawned {
                    debug_window::spawn(Arc::clone(&self.debug_state));
                    self.debug_spawned = true;
                }
                // Toggle pause-on-open or just open; window manages itself
                self.debug_state.lock().unwrap().debug_open = true;
            }

            // Sync input → callback context AND debug state
            self.callback_context.input_state = self.input.joypad_state;
            {
                let mut ds = self.debug_state.lock().unwrap();
                ds.push_input(self.input.joypad_state, self.frame_count);
                ds.frame_count = self.frame_count;
            }

            // --- Check pause / triggers ---
            let paused = {
                let mut ds = self.debug_state.lock().unwrap();

                // Frame trigger
                if let Some(tf) = ds.trigger_frame {
                    if tf < u64::MAX - 12 && self.frame_count >= tf {
                        ds.paused = true;
                        ds.trigger_frame = None;
                        ds.log(format!("⏸ Paused at frame {}", self.frame_count));
                    }
                    // Button trigger (encoded as u64::MAX - btn_index)
                    if tf >= u64::MAX - 12 {
                        let btn = (u64::MAX - tf) as usize;
                        if btn < 12 && self.input.joypad_state[btn] {
                            ds.paused = true;
                            ds.trigger_frame = None;
                            ds.log(format!("⏸ Button trigger fired: btn={}", btn));
                        }
                    }
                }

                // Pixel trigger — checked after video_callback populates fb_rgba
                if let Some((px, py)) = ds.trigger_pixel {
                    if px < ds.fb_width && py < ds.fb_height && !ds.fb_rgba.is_empty() {
                        let idx = (py as usize * ds.fb_width as usize + px as usize) * 4;
                        if idx + 2 < ds.fb_rgba.len() {
                            let r = ds.fb_rgba[idx];
                            let g = ds.fb_rgba[idx + 1];
                            let b = ds.fb_rgba[idx + 2];
                            if r != 0 || g != 0 || b != 0 {
                                ds.paused = true;
                                ds.trigger_pixel = None;
                                ds.log(format!("⏸ Pixel trigger ({px},{py}) = #{r:02X}{g:02X}{b:02X}"));
                            }
                        }
                    }
                }

                let p = ds.paused;
                // Handle step-one: run one frame then re-pause
                if ds.step_one {
                    ds.step_one = false;
                    false // run this frame
                } else {
                    p
                }
            };

            if paused {
                // Sleep briefly and loop without running the core
                std::thread::sleep(std::time::Duration::from_millis(16));
                continue;
            }

            // --- Run one emulation frame ---
            self.core
                .run()
                .map_err(|e| anyhow!("Core execution failed: {}", e))?;

            self.frame_count += 1;

            // --- Apply any AV info update from the core ---
            if let Some(new_info) = self.callback_context.pending_av_info.take() {
                let av = new_info.to_rust();
                let w = av.geometry.base_width;
                let h = av.geometry.base_height;
                self.graphics.resize_window(w, h);
                {
                    let mut ds = self.debug_state.lock().unwrap();
                    ds.av_width = w;
                    ds.av_height = h;
                    ds.fps = av.timing.fps;
                    ds.log(format!("AV info updated: {}×{} @ {:.2}fps", w, h, av.timing.fps));
                }
                if self.enable_audio {
                    let sr = if av.timing.sample_rate > 0.0 {
                        av.timing.sample_rate
                    } else {
                        48000.0
                    };
                    if let Ok(new_audio) = Audio::new(&self.sdl_context, sr) {
                        self.audio = Some(new_audio);
                    }
                }
                self.av_info = Some(av);
            }

            // --- Render ---
            let has_frame = {
                let ctx = &self.callback_context;
                !ctx.framebuffer.is_empty() && ctx.width > 0 && ctx.height > 0
            };
            if has_frame {
                let ctx = &self.callback_context;
                if let Err(e) = self.graphics.render_frame(
                    &ctx.framebuffer,
                    ctx.width,
                    ctx.height,
                    ctx.pitch,
                    ctx.pixel_format,
                ) {
                    if self.frame_count % 60 == 1 {
                        eprintln!("[RENDER ERR] {}", e);
                    }
                }
            }

            // Update debug state video counters
            {
                let ctx = &self.callback_context;
                let mut ds = self.debug_state.lock().unwrap();
                ds.video_frames = ctx.video_frames;
                ds.video_real = ctx.video_real;
            }

            // Update window title every 60 frames
            if self.frame_count % 60 == 0 {
                let ctx = &self.callback_context;
                let title = format!(
                    "RustRetro | run:{} vid:{} real:{} | {}x{} fmt={} [F12=debug]",
                    self.frame_count, ctx.video_frames, ctx.video_real,
                    ctx.width, ctx.height, ctx.pixel_format
                );
                let _ = self.graphics.set_title(&title);
            }

            // --- Audio ---
            if self.enable_audio {
                if let Some(ref audio) = self.audio {
                    let samples = &self.callback_context.pending_audio;
                    if !samples.is_empty() {
                        audio.queue_audio(samples);
                    }
                }
            }
            self.callback_context.pending_audio.clear();

            // --- Frame timing ---
            if let Some(ref av_info) = self.av_info {
                let target = std::time::Duration::from_secs_f64(1.0 / av_info.timing.fps);
                let elapsed = last_frame.elapsed();
                if elapsed < target {
                    std::thread::sleep(target - elapsed);
                }
            }
            last_frame = Instant::now();
        }

        self.core
            .unload_game()
            .map_err(|e| anyhow!("Failed to unload game: {}", e))?;
        self.core
            .deinit()
            .map_err(|e| anyhow!("Failed to deinitialize core: {}", e))?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Callback context — data shared between Frontend and libretro callbacks
// ---------------------------------------------------------------------------

pub struct CallbackContext {
    pub framebuffer: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub pitch: usize,
    pub pixel_format: u32,
    pub input_state: [bool; 12],
    pub pending_av_info: Option<RetroSystemAVInfoC>,
    pub pending_audio: Vec<i16>,
    pub video_frames: u64,
    pub video_real: u64,
    system_dir_buffer: Vec<u8>,
    save_dir_buffer: Vec<u8>,
    debug_state: SharedDebugState,
}

impl CallbackContext {
    fn new(save_dir: PathBuf, system_dir: PathBuf, debug_state: SharedDebugState) -> Self {
        let mut sys = system_dir.to_string_lossy().into_owned().into_bytes();
        sys.push(0);
        let mut sav = save_dir.to_string_lossy().into_owned().into_bytes();
        sav.push(0);

        CallbackContext {
            framebuffer: Vec::new(),
            width: 0,
            height: 0,
            pitch: 0,
            pixel_format: RETRO_PIXEL_FORMAT_0RGB1555,
            input_state: [false; 12],
            pending_av_info: None,
            pending_audio: Vec::with_capacity(4096),
            video_frames: 0,
            video_real: 0,
            system_dir_buffer: sys,
            save_dir_buffer: sav,
            debug_state,
        }
    }

    fn environment_callback(&mut self, cmd: u32, data: *mut c_void) -> bool {
        unsafe {
            match cmd {
                RETRO_ENVIRONMENT_SET_PIXEL_FORMAT => {
                    if !data.is_null() {
                        let format = *(data as *const u32);
                        // Accept all three libretro pixel formats
                        if format <= RETRO_PIXEL_FORMAT_RGB565 {
                            self.pixel_format = format;
                            return true;
                        }
                    }
                    false
                }
                RETRO_ENVIRONMENT_SET_SYSTEM_AV_INFO => {
                    if !data.is_null() {
                        self.pending_av_info = Some(*(data as *const RetroSystemAVInfoC));
                    }
                    true
                }
                RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY => {
                    if !data.is_null() {
                        let ptr = data as *mut *const i8;
                        *ptr = self.system_dir_buffer.as_ptr() as *const i8;
                        return true;
                    }
                    false
                }
                RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY => {
                    if !data.is_null() {
                        let ptr = data as *mut *const i8;
                        *ptr = self.save_dir_buffer.as_ptr() as *const i8;
                        return true;
                    }
                    false
                }
                RETRO_ENVIRONMENT_GET_VARIABLE => false,
                RETRO_ENVIRONMENT_GET_VFS_INTERFACE => false,
                RETRO_ENVIRONMENT_GET_LOG_INTERFACE => {
                    if !data.is_null() {
                        unsafe extern "C" fn core_log(level: u32, msg: *const std::ffi::c_char) {
                            let prefix = match level {
                                0 => "[CORE DBG]",
                                1 => "[CORE INF]",
                                2 => "[CORE WRN]",
                                _ => "[CORE ERR]",
                            };
                            if !msg.is_null() {
                                let s = std::ffi::CStr::from_ptr(msg).to_string_lossy();
                                eprintln!("{} {}", prefix, s.trim_end());
                            }
                        }
                        (*(data as *mut RetroLogCallback)).log =
                            core_log as *const std::ffi::c_void;
                        return true;
                    }
                    false
                }
                RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION => {
                    if !data.is_null() {
                        *(data as *mut u32) = 0;
                    }
                    true
                }
                RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE => {
                    if !data.is_null() {
                        *(data as *mut bool) = false;
                    }
                    true
                }
                RETRO_ENVIRONMENT_GET_LANGUAGE => {
                    if !data.is_null() {
                        *(data as *mut u32) = 0; // English
                    }
                    true
                }
                RETRO_ENVIRONMENT_GET_AUDIO_VIDEO_ENABLE => {
                    if !data.is_null() {
                        *(data as *mut i32) = 1 | 2; // audio + video
                    }
                    true
                }
                RETRO_ENVIRONMENT_SET_MESSAGE => {
                    if !data.is_null() {
                        let msg = *(data as *const RetroMessage);
                        if !msg.msg.is_null() {
                            let s = std::ffi::CStr::from_ptr(msg.msg as *const _)
                                .to_string_lossy();
                            eprintln!("[CORE MSG] {}", s.trim_end());
                        }
                    }
                    true
                }
                RETRO_ENVIRONMENT_SHUTDOWN => {
                    eprintln!("[CORE] Shutdown requested");
                    false
                }
                RETRO_ENVIRONMENT_SET_VARIABLES
                | RETRO_ENVIRONMENT_SET_CORE_OPTIONS
                | RETRO_ENVIRONMENT_SET_CORE_OPTIONS_INTL
                | RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2
                | RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2_INTL
                | RETRO_ENVIRONMENT_SET_CORE_OPTIONS_DISPLAY
                | RETRO_ENVIRONMENT_SET_AUDIO_BUFFER_STATUS_CALLBACK
                | RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS
                | RETRO_ENVIRONMENT_SET_ROTATION
                | RETRO_ENVIRONMENT_SET_GEOMETRY
                | RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME
                | RETRO_ENVIRONMENT_SET_SUBSYSTEM_INFO
                | RETRO_ENVIRONMENT_SET_CONTROLLER_INFO
                | RETRO_ENVIRONMENT_SET_SERIALIZATION_QUIRKS => true,
                RETRO_ENVIRONMENT_GET_CAN_DUPE => {
                    // Tell core it may submit NULL to video_refresh to repeat last frame.
                    // MAME does this; returning false doesn't stop it, just breaks timing.
                    if !data.is_null() {
                        *(data as *mut bool) = true;
                    }
                    true
                }
                RETRO_ENVIRONMENT_GET_LED_INTERFACE
                | RETRO_ENVIRONMENT_GET_PERF_INTERFACE
                | RETRO_ENVIRONMENT_GET_OVERSCAN
                | RETRO_ENVIRONMENT_GET_USERNAME => false,
                _ => false,
            }
        }
    }

    fn video_callback(&mut self, data: *const c_void, width: u32, height: u32, pitch: usize) {
        self.video_frames += 1;
        if !data.is_null() && width > 0 && height > 0 && pitch > 0 {
            let bytes = pitch * height as usize;
            unsafe {
                let slice = std::slice::from_raw_parts(data as *const u8, bytes);
                self.framebuffer.resize(bytes, 0);
                self.framebuffer.copy_from_slice(slice);
            }
            self.width = width;
            self.height = height;
            self.pitch = pitch;
            self.video_real += 1;

            // Push real frame to debug state
            if let Ok(mut ds) = self.debug_state.try_lock() {
                unsafe {
                    let slice = std::slice::from_raw_parts(data as *const u8, bytes);
                    ds.update_frame(slice, width, height, pitch, self.pixel_format);
                }
            }
        }
    }

    fn input_state_callback(&self, port: u32, device: u32, _index: u32, id: u32) -> i16 {
        if port == 0 && device == RETRO_DEVICE_JOYPAD && (id as usize) < 12 {
            self.input_state[id as usize] as i16
        } else {
            0
        }
    }

    fn audio_batch_callback(&mut self, data: *const i16, frames: usize) -> usize {
        if !data.is_null() && frames > 0 {
            unsafe {
                let samples = std::slice::from_raw_parts(data, frames * 2);
                self.pending_audio.extend_from_slice(samples);
            }
        }
        frames
    }
}

// ---------------------------------------------------------------------------
// Static C-ABI callbacks (called by the libretro core)
// ---------------------------------------------------------------------------

extern "C" fn static_environment_callback(cmd: c_uint, data: *mut c_void) -> bool {
    let ctx_ptr = CALLBACK_CONTEXT.load(Ordering::SeqCst);
    if ctx_ptr.is_null() {
        return false;
    }
    unsafe { (*ctx_ptr).environment_callback(cmd as u32, data) }
}

extern "C" fn static_video_callback(
    data: *const c_void,
    width: u32,
    height: u32,
    pitch: usize,
) {
    let ctx_ptr = CALLBACK_CONTEXT.load(Ordering::SeqCst);
    if !ctx_ptr.is_null() {
        unsafe { (*ctx_ptr).video_callback(data, width, height, pitch) };
    }
}

extern "C" fn static_input_poll_callback() {}

extern "C" fn static_input_state_callback(port: u32, device: u32, index: u32, id: u32) -> i16 {
    let ctx_ptr = CALLBACK_CONTEXT.load(Ordering::SeqCst);
    if ctx_ptr.is_null() {
        return 0;
    }
    unsafe { (*ctx_ptr).input_state_callback(port, device, index, id) }
}

extern "C" fn static_audio_callback(_left: i16, _right: i16) {}

extern "C" fn static_audio_batch_callback(data: *const i16, frames: usize) -> usize {
    let ctx_ptr = CALLBACK_CONTEXT.load(Ordering::SeqCst);
    if ctx_ptr.is_null() {
        return frames;
    }
    unsafe { (*ctx_ptr).audio_batch_callback(data, frames) }
}
