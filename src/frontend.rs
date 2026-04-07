use crate::libretro::*;
use crate::sdl_interface::{Audio, Graphics, Input};
use anyhow::{anyhow, Result};
use std::ffi::{CString, c_uint, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicPtr, Ordering};
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

        let callback_context = Box::new(CallbackContext::new(save_dir, system_dir));

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
            eprintln!(
                "AV info: {}x{} @ {:.2} FPS, {:.0} Hz audio",
                w, h, av_info.timing.fps, av_info.timing.sample_rate
            );
            frontend.graphics.resize_window(w, h);

            // Reinitialize audio with correct sample rate
            if enable_audio {
                frontend.audio = Some(Audio::new(
                    &frontend.sdl_context,
                    av_info.timing.sample_rate,
                )?);
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

            // Sync input: copy SDL key state → callback context (what the core reads)
            self.callback_context.input_state = self.input.joypad_state;

            // --- Run one emulation frame ---
            self.core
                .run()
                .map_err(|e| anyhow!("Core execution failed: {}", e))?;

            // --- Apply any AV info update from the core ---
            if let Some(new_info) = self.callback_context.pending_av_info.take() {
                let av = new_info.to_rust();
                let w = av.geometry.base_width;
                let h = av.geometry.base_height;
                self.graphics.resize_window(w, h);
                if self.enable_audio {
                    if let Ok(new_audio) = Audio::new(&self.sdl_context, av.timing.sample_rate) {
                        self.audio = Some(new_audio);
                    }
                }
                self.av_info = Some(av);
            }

            // --- Render ---
            let ctx = &self.callback_context;
            if !ctx.framebuffer.is_empty() && ctx.width > 0 && ctx.height > 0 {
                let _ = self.graphics.render_frame(
                    &ctx.framebuffer,
                    ctx.width,
                    ctx.height,
                    ctx.pitch,
                    ctx.pixel_format,
                );
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
    system_dir_buffer: Vec<u8>,
    save_dir_buffer: Vec<u8>,
}

impl CallbackContext {
    fn new(save_dir: PathBuf, system_dir: PathBuf) -> Self {
        let mut sys = system_dir.to_string_lossy().into_owned().into_bytes();
        sys.push(0); // null terminate
        let mut sav = save_dir.to_string_lossy().into_owned().into_bytes();
        sav.push(0);

        CallbackContext {
            framebuffer: Vec::new(),
            width: 0,
            height: 0,
            pitch: 0,
            pixel_format: RETRO_PIXEL_FORMAT_XRGB8888,
            input_state: [false; 12],
            pending_av_info: None,
            pending_audio: Vec::with_capacity(4096),
            system_dir_buffer: sys,
            save_dir_buffer: sav,
        }
    }

    fn environment_callback(&mut self, cmd: u32, data: *mut c_void) -> bool {
        unsafe {
            match cmd {
                RETRO_ENVIRONMENT_SET_PIXEL_FORMAT => {
                    if !data.is_null() {
                        let format = *(data as *const u32);
                        if format == RETRO_PIXEL_FORMAT_XRGB8888
                            || format == RETRO_PIXEL_FORMAT_RGB565
                        {
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
                RETRO_ENVIRONMENT_GET_LED_INTERFACE
                | RETRO_ENVIRONMENT_GET_PERF_INTERFACE
                | RETRO_ENVIRONMENT_GET_OVERSCAN
                | RETRO_ENVIRONMENT_GET_CAN_DUPE
                | RETRO_ENVIRONMENT_GET_USERNAME => false,
                _ => false,
            }
        }
    }

    fn video_callback(&mut self, data: *const c_void, width: u32, height: u32, pitch: usize) {
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
