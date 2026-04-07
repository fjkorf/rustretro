use crate::libretro::*;
use crate::sdl_interface::{Audio, Graphics, Input};
use anyhow::{anyhow, Result};
use std::ffi::{CString, c_uint, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

// Global static for callback context access
static CALLBACK_CONTEXT: AtomicPtr<CallbackContext> = AtomicPtr::new(std::ptr::null_mut());

// Global system/save directory strings - pre-allocated static buffers
static mut GLOBAL_SYS_DIR: [u8; 256] = [0; 256];
static mut GLOBAL_SAVE_DIR: [u8; 256] = [0; 256];
static mut STRINGS_INITIALIZED: bool = false;

pub fn initialize_env_strings() {
    unsafe {
        if !STRINGS_INITIALIZED {
            let dot = b".";
            GLOBAL_SYS_DIR[0..dot.len()].copy_from_slice(dot);
            GLOBAL_SYS_DIR[dot.len()] = 0;
            
            GLOBAL_SAVE_DIR[0..dot.len()].copy_from_slice(dot);
            GLOBAL_SAVE_DIR[dot.len()] = 0;
            
            STRINGS_INITIALIZED = true;
        }
    }
}

pub struct Frontend {
    core: RetroCore,
    graphics: Graphics,
    audio: Option<Audio>,
    input: Input,
    save_dir: PathBuf,
    system_dir: PathBuf,
    enable_audio: bool,
    av_info: Option<RetroSystemAVInfo>,
    callback_context: Box<CallbackContext>,
    _game_path_cstring: Option<std::ffi::CString>,
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
        // Load core
        let core = RetroCore::load(core_path)
            .map_err(|e| anyhow!("Failed to load core: {}", e))?;

        // Initialize SDL
        let sdl_context = sdl2::init().map_err(|e| anyhow!(e))?;

        // Create graphics (temporary 640x480 window, will be resized)
        let graphics = Graphics::new(&sdl_context, 640, 480, scale, fullscreen)?;

        // Initialize audio if enabled
        let audio = if enable_audio {
            Some(Audio::new(&sdl_context, 48000.0)?)
        } else {
            None
        };

        // Create input handler
        let input = Input::new();

        // Get system info
        let system_info = core
            .get_system_info()
            .map_err(|e| anyhow!("Failed to get system info: {}", e))?;

        eprintln!("Core: {} v{}", system_info.library_name, system_info.library_version);
        eprintln!("Valid extensions: {}", system_info.valid_extensions);
        eprintln!("Need fullpath: {}", system_info.need_fullpath);

        let callback_context = Box::new(CallbackContext::new(
            save_dir.clone(),
            system_dir.clone(),
        ));

        let mut frontend = Frontend {
            core,
            graphics,
            audio,
            input,
            save_dir,
            system_dir,
            enable_audio,
            av_info: None,
            callback_context,
            _game_path_cstring: None,
        };

        eprintln!("Setting up callbacks...");
        // Set up callbacks BEFORE init
        frontend.setup_callbacks()?;

        eprintln!("Initializing core...");
        // Initialize core
        frontend.core.init()
            .map_err(|e| anyhow!("Failed to initialize core: {}", e))?;

        eprintln!("Loading game...");
        eprintln!("System info: need_fullpath={}, valid_extensions={}", system_info.need_fullpath, system_info.valid_extensions);
        
        // Load ROM data if core doesn't need full path
        let rom_data = if system_info.need_fullpath {
            eprintln!("Core needs full path only, not loading ROM data");
            Vec::new()
        } else {
            eprintln!("Core needs ROM data in memory, loading...");
            std::fs::read(rom_path)
                .map_err(|e| anyhow!("Failed to read ROM: {}", e))?
        };
        
        let path_cstring = std::ffi::CString::new(rom_path).ok();
        
        let game_info = RetroGameInfo {
            path: rom_path.to_string(),
            data: rom_data,
            path_cstring: path_cstring.clone(),
        };

        eprintln!("Calling load_game...");
        frontend.core.load_game(&game_info)
            .map_err(|e| anyhow!("Failed to load game: {}", e))?;
        eprintln!("Game loaded successfully!");
        
        // Keep CString alive for duration of game
        frontend._game_path_cstring = path_cstring;

        Ok(frontend)
    }

    fn setup_callbacks(&mut self) -> Result<()> {
        // Store context pointer for static callbacks
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
        let sdl_context = sdl2::init().map_err(|e| anyhow!(e))?;
        let event_pump = sdl_context.event_pump().map_err(|e| anyhow!(e))?;

        let mut event_pump = event_pump;
        let mut last_frame_time = Instant::now();

        loop {
            // Handle events
            let mut should_quit = false;
            for event in event_pump.poll_iter() {
                if self.input.handle_event(&event) {
                    should_quit = true;
                }
            }

            if should_quit {
                break;
            }

            // Run core iteration
            self.core.run()
                .map_err(|e| anyhow!("Core execution failed: {}", e))?;

            // Handle audio if enabled
            if self.enable_audio {
                if let Some(ref audio) = self.audio {
                    audio.process_queue();
                }
            }

            // Frame rate limiting
            if let Some(ref av_info) = self.av_info {
                let target_frame_time =
                    std::time::Duration::from_secs_f64(1.0 / av_info.timing.fps);
                let elapsed = last_frame_time.elapsed();
                if elapsed < target_frame_time {
                    std::thread::sleep(target_frame_time - elapsed);
                }
                last_frame_time = Instant::now();
            }
        }

        // Cleanup
        self.core.unload_game()
            .map_err(|e| anyhow!("Failed to unload game: {}", e))?;
        self.core.deinit()
            .map_err(|e| anyhow!("Failed to deinitialize core: {}", e))?;

        Ok(())
    }
}

pub struct CallbackContext {
    pub save_dir: PathBuf,
    pub system_dir: PathBuf,
    pub framebuffer: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub input_state: [bool; 12],
    pub system_dir_cstring: CString,
    pub save_dir_cstring: CString,
    pub system_dir_buffer: Vec<u8>,
    pub save_dir_buffer: Vec<u8>,
}

impl CallbackContext {
    fn new(save_dir: PathBuf, system_dir: PathBuf) -> Self {
        let system_dir_cstring = CString::new(system_dir.to_string_lossy().as_bytes())
            .unwrap_or_else(|_| CString::new("").unwrap());
        let save_dir_cstring = CString::new(save_dir.to_string_lossy().as_bytes())
            .unwrap_or_else(|_| CString::new("").unwrap());
        
        // Create mutable buffers
        let system_dir_buffer = system_dir_cstring.as_bytes_with_nul().to_vec();
        let save_dir_buffer = save_dir_cstring.as_bytes_with_nul().to_vec();
        
        CallbackContext {
            save_dir,
            system_dir,
            framebuffer: vec![0; 640 * 480 * 4],
            width: 640,
            height: 480,
            input_state: [false; 12],
            system_dir_cstring,
            save_dir_cstring,
            system_dir_buffer,
            save_dir_buffer,
        }
    }
}

// Static callback functions
extern "C" fn static_environment_callback(cmd: c_uint, data: *mut c_void) -> bool {
    unsafe {
        let ctx_ptr = CALLBACK_CONTEXT.load(Ordering::SeqCst);
        if ctx_ptr.is_null() {
            return false;
        }
        (*ctx_ptr).environment_callback(cmd as u32, data)
    }
}

extern "C" fn static_video_callback(data: *const std::ffi::c_void, width: u32, height: u32, pitch: usize) {
    unsafe {
        let ctx_ptr = CALLBACK_CONTEXT.load(Ordering::SeqCst);
        if !ctx_ptr.is_null() {
            (*ctx_ptr).video_callback(data, width, height, pitch);
        }
    }
}

extern "C" fn static_input_poll_callback() {
    unsafe {
        let ctx_ptr = CALLBACK_CONTEXT.load(Ordering::SeqCst);
        if !ctx_ptr.is_null() {
            (*ctx_ptr).input_poll_callback();
        }
    }
}

extern "C" fn static_input_state_callback(port: u32, device: u32, _index: u32, id: u32) -> i16 {
    unsafe {
        let ctx_ptr = CALLBACK_CONTEXT.load(Ordering::SeqCst);
        if ctx_ptr.is_null() {
            return 0;
        }
        (*ctx_ptr).input_state_callback(port, device, _index, id)
    }
}

extern "C" fn static_audio_callback(left: i16, right: i16) {
    unsafe {
        let ctx_ptr = CALLBACK_CONTEXT.load(Ordering::SeqCst);
        if !ctx_ptr.is_null() {
            (*ctx_ptr).audio_callback(left, right);
        }
    }
}

extern "C" fn static_audio_batch_callback(data: *const i16, frames: usize) -> usize {
    unsafe {
        let ctx_ptr = CALLBACK_CONTEXT.load(Ordering::SeqCst);
        if !ctx_ptr.is_null() {
            (*ctx_ptr).audio_batch_callback(data, frames)
        } else {
            0
        }
    }
}


impl CallbackContext {
    fn environment_callback(&self, cmd: u32, data: *mut std::ffi::c_void) -> bool {
        unsafe {
            match cmd {
                RETRO_ENVIRONMENT_SET_PIXEL_FORMAT => {
                    if !data.is_null() {
                        let format = *(data as *mut u32);
                        // Accept XRGB8888 and RGB565; reject 0RGB1555 (legacy)
                        return format == RETRO_PIXEL_FORMAT_XRGB8888
                            || format == RETRO_PIXEL_FORMAT_RGB565;
                    }
                    false
                }
                RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY => {
                    if !data.is_null() {
                        let ptr = data as *mut *const i8;
                        static SYS_DIR: &[u8] = b".\0";
                        *ptr = SYS_DIR.as_ptr() as *const i8;
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
                RETRO_ENVIRONMENT_SET_SYSTEM_AV_INFO => true,
                RETRO_ENVIRONMENT_GET_VARIABLE => false,
                // VFS: return false so core falls back to stdio file I/O.
                // Returning true without filling the struct would crash.
                RETRO_ENVIRONMENT_GET_VFS_INTERFACE => false,
                RETRO_ENVIRONMENT_GET_LOG_INTERFACE => {
                    if !data.is_null() {
                        unsafe extern "C" fn core_log(level: u32, msg: *const std::ffi::c_char) {
                            let prefix = match level {
                                0 => "[CORE DEBUG]",
                                1 => "[CORE INFO]",
                                2 => "[CORE WARN]",
                                _ => "[CORE ERROR]",
                            };
                            if !msg.is_null() {
                                let s = std::ffi::CStr::from_ptr(msg as *const _).to_string_lossy();
                                eprintln!("{} {}", prefix, s.trim_end());
                            }
                        }
                        (*(data as *mut RetroLogCallback)).log = core_log as *const std::ffi::c_void;
                        return true;
                    }
                    false
                }
                RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION => {
                    if !data.is_null() {
                        *(data as *mut u32) = 0; // version 0 = don't send options
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
                        *(data as *mut u32) = 0; // RETRO_LANGUAGE_ENGLISH
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
                            let s = std::ffi::CStr::from_ptr(msg.msg as *const _).to_string_lossy();
                            eprintln!("[CORE MSG] {}", s.trim_end());
                        }
                    }
                    true
                }
                RETRO_ENVIRONMENT_SHUTDOWN => {
                    eprintln!("[CORE] Shutdown requested");
                    false
                }
                // Accept but ignore these
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
                // Unsupported
                RETRO_ENVIRONMENT_GET_LED_INTERFACE
                | RETRO_ENVIRONMENT_GET_PERF_INTERFACE
                | RETRO_ENVIRONMENT_GET_OVERSCAN
                | RETRO_ENVIRONMENT_GET_CAN_DUPE
                | RETRO_ENVIRONMENT_GET_USERNAME => false,
                _ => {
                    eprintln!("[ENV] Unhandled cmd={}", cmd);
                    false
                }
            }
        }
    }

    fn video_callback(&mut self, data: *const std::ffi::c_void, width: u32, height: u32, pitch: usize) {
        if !data.is_null() && width > 0 && height > 0 && pitch > 0 {
            unsafe {
                let slice = std::slice::from_raw_parts(data as *const u8, pitch * height as usize);
                self.framebuffer = slice.to_vec();
                self.width = width;
                self.height = height;
            }
        }
    }

    fn input_poll_callback(&self) {
        // Input polling is handled by SDL2 event loop
    }

    fn input_state_callback(&self, port: u32, device: u32, _index: u32, id: u32) -> i16 {
        if port == 0 && device == RETRO_DEVICE_JOYPAD && id < 12 {
            if self.input_state[id as usize] {
                1
            } else {
                0
            }
        } else {
            0
        }
    }

    fn audio_callback(&self, _left: i16, _right: i16) {
        // Audio samples would be queued here
    }

    fn audio_batch_callback(&self, _data: *const i16, frames: usize) -> usize {
        // Batch audio samples - just accept them for now
        // In a real implementation, would queue to SDL audio device
        frames
    }
}
